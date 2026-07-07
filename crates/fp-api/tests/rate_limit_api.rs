//! Adversarial, black-box HTTP integration tests for the rate-limit REST surface
//! (`/api/v1/teams/{team}/rate-limit-domains` and its nested policy/override resources,
//! feature fpv2-4ht slice S3).
//!
//! Written by a DIFFERENT author from the implementer: the surface is treated as opaque and
//! exercised ONLY through HTTP requests against the real `build_router` stack (dev-issuer
//! tokens through the production validation + authz path). Assertions are written to CATCH
//! bugs — tenant leaks, missing version checks, wrong status codes — not to pass.
//!
//! Every test uses a UNIQUE uuid-v7 prefix and asserts only on its own entries; the suite runs
//! against a shared external PostgreSQL and must never assert on global totals.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("json")
}

/// Connect, migrate, and return the pool — or `None` when the test DB is not configured (the
/// caller early-returns, like every other DB-backed test in this crate).
async fn pool_or_skip() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 6).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(pool)
}

/// Mint a bearer token for a brand-new user that is an org member of `org_id` and (optionally)
/// holds a full RateLimits grant set on `team_id`. Returns `(issuer-less token)`.
///
/// We must mint each principal with its OWN issuer-validated token, but `build_router` already
/// holds a validator bound to one issuer. So instead we mint all principals from a SINGLE shared
/// issuer created here and return both the app and tokens.
struct Issuer {
    inner: DevIssuer,
}

impl Issuer {
    fn new() -> Self {
        Self {
            inner: DevIssuer::generate().expect("issuer"),
        }
    }

    fn validator(&self) -> fp_core::OidcValidator {
        fp_core::OidcValidator::new(self.inner.oidc_config())
    }

    fn jwks(&self) -> &str {
        self.inner.jwks_json()
    }

    fn mint(&self, subject: &str) -> String {
        self.inner
            .mint(subject, &format!("{subject}@test"), "Test", 600)
            .expect("mint")
    }
}

/// Build a router whose validator trusts `issuer`.
async fn build_app_with(pool: sqlx::PgPool, issuer: &Issuer) -> axum::Router {
    let validator = issuer.validator();
    validator
        .load_jwks_json(issuer.jwks())
        .await
        .expect("jwks 2");
    fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        egress_policy: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    })
}

/// Create an org + team, and a fully-RateLimits-granted member of that team. Returns
/// `(org_id, team_name, member_subject)`.
async fn org_team_granted_member(
    pool: &sqlx::PgPool,
    sub_prefix: &str,
) -> (fp_domain::OrgId, String, String) {
    let org = identity::create_org(pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let subject = unique(sub_prefix);
    let user = identity::upsert_user_by_subject(pool, &subject, "g@t.test", "G")
        .await
        .expect("user");
    identity::add_org_membership(pool, user, org.id, OrgRole::Member)
        .await
        .expect("membership");
    for action in [Action::Create, Action::Read, Action::Update, Action::Delete] {
        identity::add_grant(
            pool,
            user,
            org.id,
            team.id,
            Resource::RateLimits,
            action,
            None,
        )
        .await
        .expect("grant");
    }
    (org.id, team.name, subject)
}

/// Convenience: build a request with bearer + optional If-Match + optional JSON body.
fn req(
    token: &str,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    revision: Option<i64>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    if let Some(revision) = revision {
        builder = builder.header("if-match", revision.to_string());
    }
    match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request")
}

fn policy_spec(client: &str, rpu: u64, unit: &str) -> serde_json::Value {
    serde_json::json!({
        "descriptors": { "client_id": client },
        "requests_per_unit": rpu,
        "unit": unit,
    })
}

// ============================================================================================
// 1. Full CRUD happy path: domain -> policy -> override.
// ============================================================================================

#[tokio::test]
async fn full_crud_domain_policy_override_over_http() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;
    let (_org, team, subject) = org_team_granted_member(&pool, "crud").await;
    let token = issuer.mint(&subject);

    let base = format!("/api/v1/teams/{team}/rate-limit-domains");

    // --- Create domain ---
    let domain_name = unique("checkout");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &base,
            Some(serde_json::json!({"name": domain_name})),
            None,
        ))
        .await
        .expect("create domain");
    assert_eq!(resp.status(), StatusCode::CREATED, "domain create -> 201");
    let body = json_of(resp).await;
    assert_eq!(body["name"], domain_name);
    assert_eq!(body["revision"], 1, "first revision is 1");
    assert!(body["id"].is_string());
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());
    let domain_rev = body["revision"].as_i64().expect("revision int");

    // --- Get domain ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{domain_name}"),
            None,
            None,
        ))
        .await
        .expect("get domain");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await["name"], domain_name);

    // --- List contains it (assert only on MY entry, never the global total) ---
    let resp = app
        .clone()
        .oneshot(req(&token, "GET", &base, None, None))
        .await
        .expect("list domains");
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_of(resp).await;
    assert!(listed["items"].is_array());
    assert!(
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == serde_json::json!(domain_name)),
        "created domain appears in the list"
    );

    // --- PATCH domain bumps revision ---
    let renamed = unique("checkout-2");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &format!("{base}/{domain_name}"),
            Some(serde_json::json!({"name": renamed})),
            Some(domain_rev),
        ))
        .await
        .expect("patch domain");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await;
    assert_eq!(body["name"], renamed);
    assert_eq!(body["revision"], 2, "patch bumps revision to 2");
    let domain_rev = body["revision"].as_i64().unwrap();

    // The old name no longer resolves.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{domain_name}"),
            None,
            None,
        ))
        .await
        .expect("get old name");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "renamed-away name -> 404"
    );

    // --- Create policy under the (renamed) domain ---
    let pol_base = format!("{base}/{renamed}/policies");
    let policy_name = unique("p1");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(
                serde_json::json!({"name": policy_name, "spec": policy_spec("bob", 100, "minute")}),
            ),
            None,
        ))
        .await
        .expect("create policy");
    assert_eq!(resp.status(), StatusCode::CREATED, "policy create -> 201");
    let body = json_of(resp).await;
    assert_eq!(body["name"], policy_name);
    assert_eq!(body["revision"], 1);
    assert_eq!(body["spec"]["requests_per_unit"], 100);
    assert_eq!(body["spec"]["unit"], "minute");
    assert_eq!(body["domain_id"], body["domain_id"]); // present
    assert!(body["domain_id"].is_string());
    assert!(
        body["descriptors_canonical"].is_string(),
        "policy view carries the read-only canonical descriptor key"
    );
    let policy_rev = body["revision"].as_i64().unwrap();

    // --- Get policy / list policies ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{pol_base}/{policy_name}"),
            None,
            None,
        ))
        .await
        .expect("get policy");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await["name"], policy_name);

    let resp = app
        .clone()
        .oneshot(req(&token, "GET", &pol_base, None, None))
        .await
        .expect("list policies");
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_of(resp).await;
    assert!(
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == serde_json::json!(policy_name)),
        "policy appears in its domain's list"
    );

    // --- PATCH policy bumps revision ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &format!("{pol_base}/{policy_name}"),
            Some(serde_json::json!({"spec": policy_spec("bob", 250, "hour")})),
            Some(policy_rev),
        ))
        .await
        .expect("patch policy");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await;
    assert_eq!(body["revision"], 2, "policy patch bumps to 2");
    assert_eq!(body["spec"]["requests_per_unit"], 250);
    assert_eq!(body["spec"]["unit"], "hour");

    // --- Override: create / get / patch ---
    let ovr_path = format!("{pol_base}/{policy_name}/override");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &ovr_path,
            Some(serde_json::json!({"spec": {"requests_per_unit": 50}})),
            None,
        ))
        .await
        .expect("create override");
    assert_eq!(resp.status(), StatusCode::CREATED, "override create -> 201");
    let body = json_of(resp).await;
    assert_eq!(body["revision"], 1);
    assert_eq!(body["spec"]["requests_per_unit"], 50);
    assert!(body["policy_id"].is_string());
    let ovr_rev = body["revision"].as_i64().unwrap();

    let resp = app
        .clone()
        .oneshot(req(&token, "GET", &ovr_path, None, None))
        .await
        .expect("get override");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await["spec"]["requests_per_unit"], 50);

    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &ovr_path,
            Some(serde_json::json!({"spec": {"requests_per_unit": 75}})),
            Some(ovr_rev),
        ))
        .await
        .expect("patch override");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await;
    assert_eq!(body["revision"], 2);
    assert_eq!(body["spec"]["requests_per_unit"], 75);
    let ovr_rev = body["revision"].as_i64().unwrap();

    // --- Tear down in dependency order: override -> policy -> domain ---
    let resp = app
        .clone()
        .oneshot(req(&token, "DELETE", &ovr_path, None, Some(ovr_rev)))
        .await
        .expect("delete override");
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "override delete -> 204"
    );
    let resp = app
        .clone()
        .oneshot(req(&token, "GET", &ovr_path, None, None))
        .await
        .expect("get override after delete");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "override gone -> 404");

    // policy is now at revision 2 after the earlier patch.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "DELETE",
            &format!("{pol_base}/{policy_name}"),
            None,
            Some(2),
        ))
        .await
        .expect("delete policy");
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "policy delete -> 204"
    );
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{pol_base}/{policy_name}"),
            None,
            None,
        ))
        .await
        .expect("get policy after delete");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // domain is at revision 2 after the rename patch.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "DELETE",
            &format!("{base}/{renamed}"),
            None,
            Some(domain_rev),
        ))
        .await
        .expect("delete domain");
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "domain delete -> 204"
    );
    let resp = app
        .clone()
        .oneshot(req(&token, "GET", &format!("{base}/{renamed}"), None, None))
        .await
        .expect("get domain after delete");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ============================================================================================
// 2a. Cross-ORG isolation (acceptance #3): a principal in a DIFFERENT org sees team A's domain
//     as 404 (disclosure rule, spec/02:327), never 403. Mutations likewise 404, and the
//     resource is untouched for the legitimate owner afterwards.
// ============================================================================================

#[tokio::test]
async fn cross_org_principal_sees_not_found_not_forbidden() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;

    // Team A in org A, with a granted owner.
    let (_org_a, team_a, sub_a) = org_team_granted_member(&pool, "owner-a").await;
    let token_a = issuer.mint(&sub_a);

    // Team B in a DIFFERENT org, with its own granted owner.
    let (_org_b, _team_b, sub_b) = org_team_granted_member(&pool, "owner-b").await;
    let token_b = issuer.mint(&sub_b);

    let base_a = format!("/api/v1/teams/{team_a}/rate-limit-domains");

    // A creates a domain + policy.
    let domain = unique("a-secret");
    let resp = app
        .clone()
        .oneshot(req(
            &token_a,
            "POST",
            &base_a,
            Some(serde_json::json!({"name": domain})),
            None,
        ))
        .await
        .expect("a create domain");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let dbody = json_of(resp).await;
    let domain_rev = dbody["revision"].as_i64().unwrap();

    let policy = unique("a-pol");
    let resp = app
        .clone()
        .oneshot(req(
            &token_a,
            "POST",
            &format!("{base_a}/{domain}/policies"),
            Some(serde_json::json!({"name": policy, "spec": policy_spec("x", 5, "second")})),
            None,
        ))
        .await
        .expect("a create policy");
    assert_eq!(resp.status(), StatusCode::CREATED);

    // B, in a different org, references team A by NAME. resolve_team scopes by B's org, so the
    // team name is not even in B's org -> 404 on the collection list.
    let resp = app
        .clone()
        .oneshot(req(&token_b, "GET", &base_a, None, None))
        .await
        .expect("b list a domains");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "foreign team name -> 404 (no org leak)"
    );

    // B GETs A's specific domain -> 404, NOT 403 (disclosure rule).
    let resp = app
        .clone()
        .oneshot(req(
            &token_b,
            "GET",
            &format!("{base_a}/{domain}"),
            None,
            None,
        ))
        .await
        .expect("b get a domain");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-org GET must be 404 not 403"
    );
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);

    // B PATCH of A's domain -> 404.
    let resp = app
        .clone()
        .oneshot(req(
            &token_b,
            "PATCH",
            &format!("{base_a}/{domain}"),
            Some(serde_json::json!({"name": unique("hijack")})),
            Some(domain_rev),
        ))
        .await
        .expect("b patch a domain");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-org PATCH must be 404"
    );

    // B DELETE of A's domain -> 404.
    let resp = app
        .clone()
        .oneshot(req(
            &token_b,
            "DELETE",
            &format!("{base_a}/{domain}"),
            None,
            Some(domain_rev),
        ))
        .await
        .expect("b delete a domain");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-org DELETE must be 404"
    );

    // The domain still exists for A, untouched (still revision 1).
    let resp = app
        .clone()
        .oneshot(req(
            &token_a,
            "GET",
            &format!("{base_a}/{domain}"),
            None,
            None,
        ))
        .await
        .expect("a re-get domain");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "A's resource survives B's attempts"
    );
    assert_eq!(json_of(resp).await["revision"], 1, "B changed nothing");
}

// ============================================================================================
// 2b. Same-org, different-team isolation. A same-org member lacking a grant on team A is denied
//     at the TEAM level (`Reason::NoMatchingGrant` -> 403) by the shared `deny_to_error`, before
//     any per-resource lookup. The security property that matters is NO NAME-EXISTENCE ORACLE:
//     the denial must be IDENTICAL whether the named domain exists or not. This test proves that
//     invariant (existing == absent == 403) rather than over-claiming a leak. The cosmetic
//     403-vs-404 question for the no-grant case (deny_to_error's doc comment says no-grant reads
//     render as not_found, but the code returns forbidden) is a pre-existing cross-cutting authz
//     mismatch tracked in bead fpv2-6y3 — out of scope for this surface slice.
// ============================================================================================

#[tokio::test]
async fn same_org_other_team_member_denied_with_no_existence_oracle() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;

    // One org, two teams. team_a has a granted owner; team_b has a member granted ONLY on team_b.
    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");

    let sub_a = unique("a-owner");
    let user_a = identity::upsert_user_by_subject(&pool, &sub_a, "a@t.test", "A")
        .await
        .expect("user a");
    identity::add_org_membership(&pool, user_a, org.id, OrgRole::Member)
        .await
        .expect("m a");
    for action in [Action::Create, Action::Read, Action::Update, Action::Delete] {
        identity::add_grant(
            &pool,
            user_a,
            org.id,
            team_a.id,
            Resource::RateLimits,
            action,
            None,
        )
        .await
        .expect("grant a");
    }
    let token_a = issuer.mint(&sub_a);

    let sub_b = unique("b-member");
    let user_b = identity::upsert_user_by_subject(&pool, &sub_b, "b@t.test", "B")
        .await
        .expect("user b");
    identity::add_org_membership(&pool, user_b, org.id, OrgRole::Member)
        .await
        .expect("m b");
    for action in [Action::Create, Action::Read, Action::Update, Action::Delete] {
        identity::add_grant(
            &pool,
            user_b,
            org.id,
            team_b.id,
            Resource::RateLimits,
            action,
            None,
        )
        .await
        .expect("grant b");
    }
    let token_b = issuer.mint(&sub_b);

    let base_a = format!("/api/v1/teams/{}/rate-limit-domains", team_a.name);

    // A creates a domain.
    let domain = unique("a-dom");
    let resp = app
        .clone()
        .oneshot(req(
            &token_a,
            "POST",
            &base_a,
            Some(serde_json::json!({"name": domain})),
            None,
        ))
        .await
        .expect("a create domain");
    assert_eq!(resp.status(), StatusCode::CREATED);

    // B (same org, granted on team_b only) GETs A's EXISTING domain.
    let resp = app
        .clone()
        .oneshot(req(
            &token_b,
            "GET",
            &format!("{base_a}/{domain}"),
            None,
            None,
        ))
        .await
        .expect("b get a existing domain");
    let status_existing = resp.status();

    // B GETs a domain under team_a that NEVER existed.
    let resp = app
        .clone()
        .oneshot(req(
            &token_b,
            "GET",
            &format!("{base_a}/{}", unique("ghost")),
            None,
            None,
        ))
        .await
        .expect("b get a ghost domain");
    let status_absent = resp.status();

    // The load-bearing invariant: the denial does not vary with resource existence, so B cannot
    // use the status to probe which domain names team A owns. (Today both are 403 from the
    // team-level grant check; the point is they are EQUAL, whatever the code.)
    assert_eq!(
        status_existing, status_absent,
        "same-org cross-team denial must be identical for existing vs absent names (no oracle)"
    );
    assert!(
        status_existing == StatusCode::FORBIDDEN || status_existing == StatusCode::NOT_FOUND,
        "cross-team read must be denied; got {status_existing}"
    );

    // The denial must leave A's row untouched.
    let resp = app
        .clone()
        .oneshot(req(
            &token_a,
            "GET",
            &format!("{base_a}/{domain}"),
            None,
            None,
        ))
        .await
        .expect("a re-get");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await["revision"], 1);
}

// ============================================================================================
// 3. Version conflict (acceptance #5): stale If-Match -> 409, missing If-Match -> 400.
//    Distinguish the two on both PATCH and DELETE, for a domain and a policy.
// ============================================================================================

#[tokio::test]
async fn version_conflicts_stale_409_missing_400() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;
    let (_org, team, subject) = org_team_granted_member(&pool, "ver").await;
    let token = issuer.mint(&subject);

    let base = format!("/api/v1/teams/{team}/rate-limit-domains");
    let domain = unique("ver-dom");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &base,
            Some(serde_json::json!({"name": domain})),
            None,
        ))
        .await
        .expect("create domain");
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(json_of(resp).await["revision"], 1);

    // --- Missing If-Match on PATCH -> 400 (revision_from -> ValidationFailed). ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &format!("{base}/{domain}"),
            Some(serde_json::json!({"name": unique("x")})),
            None, // no If-Match
        ))
        .await
        .expect("patch no if-match");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "missing If-Match on PATCH -> 400"
    );
    assert_eq!(json_of(resp).await["code"], "validation_failed");

    // --- Missing If-Match on DELETE -> 400. ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "DELETE",
            &format!("{base}/{domain}"),
            None,
            None,
        ))
        .await
        .expect("delete no if-match");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "missing If-Match on DELETE -> 400"
    );

    // --- Stale If-Match on PATCH -> 409 (RevisionMismatch). ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &format!("{base}/{domain}"),
            Some(serde_json::json!({"name": unique("x")})),
            Some(99),
        ))
        .await
        .expect("patch stale");
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "stale If-Match on PATCH -> 409"
    );

    // --- Stale If-Match on DELETE -> 409. ---
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "DELETE",
            &format!("{base}/{domain}"),
            None,
            Some(99),
        ))
        .await
        .expect("delete stale");
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "stale If-Match on DELETE -> 409"
    );

    // The domain must be untouched: still revision 1.
    let resp = app
        .clone()
        .oneshot(req(&token, "GET", &format!("{base}/{domain}"), None, None))
        .await
        .expect("re-get");
    assert_eq!(
        json_of(resp).await["revision"],
        1,
        "rejected writes leave revision intact"
    );

    // --- Repeat the stale/missing matrix on a POLICY. ---
    let pol_base = format!("{base}/{domain}/policies");
    let policy = unique("ver-pol");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({"name": policy, "spec": policy_spec("c", 10, "minute")})),
            None,
        ))
        .await
        .expect("create policy");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &format!("{pol_base}/{policy}"),
            Some(serde_json::json!({"spec": policy_spec("c", 11, "minute")})),
            None,
        ))
        .await
        .expect("policy patch no if-match");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "policy missing If-Match -> 400"
    );

    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "PATCH",
            &format!("{pol_base}/{policy}"),
            Some(serde_json::json!({"spec": policy_spec("c", 11, "minute")})),
            Some(42),
        ))
        .await
        .expect("policy patch stale");
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "policy stale If-Match -> 409"
    );
}

// ============================================================================================
// 4. Validation: rpu=0 -> 400, empty descriptors -> 400, unknown unit -> 400, unknown JSON
//    field -> 400 (deny_unknown_fields), empty domain name -> 400.
// ============================================================================================

#[tokio::test]
async fn validation_rejects_bad_bodies() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;
    let (_org, team, subject) = org_team_granted_member(&pool, "val").await;
    let token = issuer.mint(&subject);

    let base = format!("/api/v1/teams/{team}/rate-limit-domains");

    // Empty domain name -> 400.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &base,
            Some(serde_json::json!({"name": ""})),
            None,
        ))
        .await
        .expect("empty domain name");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty domain name -> 400"
    );
    assert_eq!(json_of(resp).await["code"], "validation_failed");

    // Unknown field on the domain body (deny_unknown_fields) -> 400.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &base,
            Some(serde_json::json!({"name": unique("d"), "bogus": true})),
            None,
        ))
        .await
        .expect("unknown domain field");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown domain field -> 400"
    );

    // Seed a valid domain to host policy-body validation.
    let domain = unique("val-dom");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &base,
            Some(serde_json::json!({"name": domain})),
            None,
        ))
        .await
        .expect("seed domain");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let pol_base = format!("{base}/{domain}/policies");

    // requests_per_unit = 0 -> 400.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({"name": unique("p"), "spec": policy_spec("c", 0, "minute")})),
            None,
        ))
        .await
        .expect("rpu zero");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "requests_per_unit=0 -> 400"
    );
    assert_eq!(json_of(resp).await["code"], "validation_failed");

    // Empty descriptors -> 400.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({
                "name": unique("p"),
                "spec": {"descriptors": {}, "requests_per_unit": 10, "unit": "minute"}
            })),
            None,
        ))
        .await
        .expect("empty descriptors");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty descriptors -> 400"
    );

    // Unknown unit -> 400 (rejected at JSON deserialization, lowercase enum).
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({"name": unique("p"), "spec": policy_spec("c", 10, "week")})),
            None,
        ))
        .await
        .expect("unknown unit");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown unit -> 400"
    );

    // Uppercase unit ("MINUTE") must also be rejected — the enum is rename_all=lowercase.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({"name": unique("p"), "spec": policy_spec("c", 10, "MINUTE")})),
            None,
        ))
        .await
        .expect("uppercase unit");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "uppercase unit -> 400 (lowercase only)"
    );

    // Unknown field inside the policy spec (deny_unknown_fields on RateLimitPolicySpec) -> 400.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({
                "name": unique("p"),
                "spec": {
                    "descriptors": {"c": "x"},
                    "requests_per_unit": 10,
                    "unit": "minute",
                    "extra": 1
                }
            })),
            None,
        ))
        .await
        .expect("unknown spec field");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown spec field -> 400"
    );

    // Override with requests_per_unit = 0 -> 400. (Seed a valid policy first.)
    let policy = unique("ovr-target");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({"name": policy, "spec": policy_spec("c", 10, "minute")})),
            None,
        ))
        .await
        .expect("seed policy");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &format!("{pol_base}/{policy}/override"),
            Some(serde_json::json!({"spec": {"requests_per_unit": 0}})),
            None,
        ))
        .await
        .expect("override rpu zero");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "override requests_per_unit=0 -> 400"
    );
}

// ============================================================================================
// 5. Not-found: policy under a non-existent domain -> 404; override GET when none set -> 404.
// ============================================================================================

#[tokio::test]
async fn not_found_paths() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;
    let (_org, team, subject) = org_team_granted_member(&pool, "nf").await;
    let token = issuer.mint(&subject);

    let base = format!("/api/v1/teams/{team}/rate-limit-domains");

    // List policies under a domain that never existed -> 404.
    let ghost = unique("ghost-domain");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{ghost}/policies"),
            None,
            None,
        ))
        .await
        .expect("policies under ghost domain");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "policies under non-existent domain -> 404"
    );

    // GET a specific policy under a ghost domain -> 404.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{ghost}/policies/{}", unique("p")),
            None,
            None,
        ))
        .await
        .expect("policy under ghost domain");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Create a real domain + policy, then GET the override when none is set -> 404.
    let domain = unique("nf-dom");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &base,
            Some(serde_json::json!({"name": domain})),
            None,
        ))
        .await
        .expect("create domain");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let pol_base = format!("{base}/{domain}/policies");
    let policy = unique("nf-pol");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &pol_base,
            Some(serde_json::json!({"name": policy, "spec": policy_spec("c", 10, "minute")})),
            None,
        ))
        .await
        .expect("create policy");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{pol_base}/{policy}/override"),
            None,
            None,
        ))
        .await
        .expect("get unset override");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "override GET with none set -> 404"
    );

    // GET a policy that never existed under a real domain -> 404.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{pol_base}/{}", unique("never")),
            None,
            None,
        ))
        .await
        .expect("get ghost policy");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ============================================================================================
// 6. Nesting integrity: a policy created under domain X is NOT retrievable under domain Y
//    (same team).
// ============================================================================================

#[tokio::test]
async fn policy_does_not_leak_across_sibling_domains() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;
    let (_org, team, subject) = org_team_granted_member(&pool, "nest").await;
    let token = issuer.mint(&subject);

    let base = format!("/api/v1/teams/{team}/rate-limit-domains");

    // Two sibling domains in the same team.
    let domain_x = unique("dom-x");
    let domain_y = unique("dom-y");
    for d in [&domain_x, &domain_y] {
        let resp = app
            .clone()
            .oneshot(req(
                &token,
                "POST",
                &base,
                Some(serde_json::json!({"name": d})),
                None,
            ))
            .await
            .expect("create domain");
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // Policy under X.
    let policy = unique("nest-pol");
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &format!("{base}/{domain_x}/policies"),
            Some(serde_json::json!({"name": policy, "spec": policy_spec("c", 10, "minute")})),
            None,
        ))
        .await
        .expect("create policy under X");
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Visible under X.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{domain_x}/policies/{policy}"),
            None,
            None,
        ))
        .await
        .expect("get under X");
    assert_eq!(resp.status(), StatusCode::OK);

    // NOT visible under Y -> 404.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{domain_y}/policies/{policy}"),
            None,
            None,
        ))
        .await
        .expect("get under Y");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "policy must not resolve under a sibling domain"
    );

    // And Y's policy list does not contain it.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "GET",
            &format!("{base}/{domain_y}/policies"),
            None,
            None,
        ))
        .await
        .expect("list Y policies");
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_of(resp).await;
    assert!(
        !listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == serde_json::json!(policy)),
        "X's policy must not appear in Y's list"
    );

    // The override path is also nested: an override on the policy under Y -> 404 (policy not
    // under Y), even though it exists under X.
    let resp = app
        .clone()
        .oneshot(req(
            &token,
            "POST",
            &format!("{base}/{domain_y}/policies/{policy}/override"),
            Some(serde_json::json!({"spec": {"requests_per_unit": 5}})),
            None,
        ))
        .await
        .expect("override under wrong domain");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "override on a policy under the wrong domain -> 404"
    );
}

// ============================================================================================
// 7. Auth: no bearer -> 401; a member with NO RateLimits grant -> 403 (missing-grant denial).
// ============================================================================================

#[tokio::test]
async fn auth_no_token_401_and_grantless_member_403() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let issuer = Issuer::new();
    let app = build_app_with(pool.clone(), &issuer).await;
    let (org_id, team, _granted_sub) = org_team_granted_member(&pool, "auth").await;

    let base = format!("/api/v1/teams/{team}/rate-limit-domains");

    // --- No bearer token -> 401. ---
    let no_auth = Request::builder()
        .method("POST")
        .uri(&base)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"name": unique("d")}).to_string(),
        ))
        .expect("request");
    let resp = app
        .clone()
        .oneshot(no_auth)
        .await
        .expect("no-token request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "missing bearer -> 401"
    );
    assert_eq!(json_of(resp).await["code"], "unauthorized");

    // Garbage bearer (not a valid JWT) -> 401.
    let resp = app
        .clone()
        .oneshot(req("not-a-real-token", "GET", &base, None, None))
        .await
        .expect("garbage token request");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "invalid bearer -> 401"
    );

    // --- Grantless member of the SAME org tries to create -> 403 (missing grant). ---
    // The user is JIT-provisioned and an org member but has NO RateLimits grant on the team.
    let grantless_sub = unique("grantless");
    let user = identity::upsert_user_by_subject(&pool, &grantless_sub, "g@x.test", "G")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org_id, OrgRole::Member)
        .await
        .expect("membership");
    let grantless_token = issuer.mint(&grantless_sub);

    let resp = app
        .clone()
        .oneshot(req(
            &grantless_token,
            "POST",
            &base,
            Some(serde_json::json!({"name": unique("forbidden")})),
            None,
        ))
        .await
        .expect("grantless create");
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "same-org member without a RateLimits grant -> 403"
    );
    assert_eq!(json_of(resp).await["code"], "forbidden");
}
