//! Black-box integration tests for the CP→RLS sync layer (`fp_core::services::rls_sync`,
//! feature fpv2-4ht slice S5; acceptance bead fpv2-4ht.5).
//!
//! The module is treated as opaque: we exercise only its public surface
//! (`namespace_uuid`, `compose_domain`, `build_push`, `reconcile_once`, `authorize_repush`)
//! plus a stub HTTP server standing in for the RLS admin endpoint, and we set rate-limit data
//! up through the realistic `fp_core::services::rate_limit` service path. Assertions are
//! adversarial — written to surface a sync that drops overrides, leaks tenants, keeps
//! soft-deleted rows, mishandles non-2xx/unreachable endpoints, or hands the repush trigger to
//! a non-platform principal. We never assert on totals over the shared DB; we find OUR entries.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use fp_core::services::{rate_limit as rl, rls_sync as sync};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{
    OrgRole, RateLimitPolicySpec, RateLimitTeamOverrideSpec, RateLimitUnit, RequestId,
};
use fp_storage::repos::identity;
use sqlx::PgPool;

// ============================================================================================
// Harness (setup patterns mirrored from tests/rate_limit.rs / tests/tenancy.rs).
// ============================================================================================

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

fn policy_spec(client: &str, rpu: u64) -> RateLimitPolicySpec {
    let mut descriptors = BTreeMap::new();
    descriptors.insert("client_id".to_string(), client.to_string());
    RateLimitPolicySpec {
        descriptors,
        requests_per_unit: rpu,
        unit: RateLimitUnit::Minute,
    }
}

/// A team with a grant-holding member who can mutate rate limits on it.
struct Tenant {
    team: TeamRef,
    granted: PrincipalCtx,
}

struct Harness {
    pool: PgPool,
}

async fn harness() -> Option<Harness> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(Harness { pool })
}

/// Create a fresh org + team + a Member holding full RateLimits grants on that team.
async fn make_tenant(pool: &PgPool) -> Tenant {
    let org = identity::create_org(pool, &unique("org"), "")
        .await
        .expect("org");
    let team_row = identity::create_team(pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };

    let sub = unique("sub-granted");
    let uid = identity::upsert_user_by_subject(pool, &sub, "g@t.test", "G")
        .await
        .expect("user");
    identity::add_org_membership(pool, uid, org.id, OrgRole::Member)
        .await
        .expect("membership");
    for action in [Action::Create, Action::Read, Action::Update, Action::Delete] {
        identity::add_grant(
            pool,
            uid,
            org.id,
            team.id,
            Resource::RateLimits,
            action,
            None,
        )
        .await
        .expect("grant");
    }
    let granted = principal_ctx(pool, &sub).await;

    Tenant { team, granted }
}

/// Load a principal the way the auth middleware would (D-014 single-org inference).
async fn principal_ctx(pool: &PgPool, subject: &str) -> PrincipalCtx {
    let loaded = identity::load_principal(pool, subject)
        .await
        .expect("load principal")
        .expect("principal exists");
    let candidates: Vec<_> = loaded
        .memberships
        .iter()
        .copied()
        .filter(|(org_id, _)| Some(*org_id) != loaded.platform_org_id)
        .collect();
    let (org, org_selector_required) = match candidates.as_slice() {
        [one] => (Some(*one), false),
        [] => (None, false),
        _ => (None, true),
    };
    PrincipalCtx::User {
        user_id: loaded.user_id,
        platform_admin: loaded.platform_admin,
        org_selector_required,
        org,
        grants: GrantSet::new(loaded.grants),
    }
}

/// Find the single push entry whose `domain` equals the expected composed namespace, if any.
fn find_entry<'a>(body: &'a sync::PushBody, composed: &str) -> Option<&'a sync::PushPolicy> {
    body.policies.iter().find(|p| p.domain == composed)
}

// ============================================================================================
// Acceptance 1: namespace derivation — deterministic, non-identity, collision-resistant; and
// compose_domain's three-part shape is glued from namespace_uuid + the raw domain.
// ============================================================================================

#[tokio::test]
async fn namespace_uuid_is_deterministic_opaque_and_injective_on_distinct_ids() {
    let a = uuid::Uuid::now_v7();
    let b = uuid::Uuid::now_v7();
    assert_ne!(a, b, "two now_v7 ids should differ (test precondition)");

    // Deterministic: same input -> same output, every call.
    assert_eq!(sync::namespace_uuid(a), sync::namespace_uuid(a));
    assert_eq!(sync::namespace_uuid(b), sync::namespace_uuid(b));

    // Not an identity / echo of the raw id — the whole point is to never trust caller identity.
    assert_ne!(
        sync::namespace_uuid(a),
        a,
        "namespace must transform, not echo, the raw id"
    );

    // Distinct ids must not collapse to the same namespace.
    assert_ne!(
        sync::namespace_uuid(a),
        sync::namespace_uuid(b),
        "distinct ids must map to distinct namespaces"
    );
}

#[tokio::test]
async fn compose_domain_is_three_parts_ending_in_raw_domain_with_namespaced_prefixes() {
    let org = fp_domain::OrgId::from(uuid::Uuid::now_v7());
    let team = fp_domain::TeamId::from(uuid::Uuid::now_v7());
    let raw = "checkout";

    let composed = sync::compose_domain(org, team, raw);
    let parts: Vec<&str> = composed.split('|').collect();
    assert_eq!(
        parts.len(),
        3,
        "exactly three pipe-separated parts: {composed}"
    );

    // Org and team prefixes are the namespaced (not raw) uuids.
    assert_eq!(parts[0], sync::namespace_uuid(org.as_uuid()).to_string());
    assert_eq!(parts[1], sync::namespace_uuid(team.as_uuid()).to_string());
    assert_ne!(
        parts[0],
        org.as_uuid().to_string(),
        "org prefix must be namespaced, not the raw org uuid"
    );
    assert_ne!(
        parts[1],
        team.as_uuid().to_string(),
        "team prefix must be namespaced, not the raw team uuid"
    );

    // Final part is the raw domain verbatim.
    assert_eq!(parts[2], raw, "final part is the literal domain name");
}

// ============================================================================================
// Acceptance 2: build_push reflects a real policy created through the service, with the right
// composed domain, descriptors, rpu, and unit.
// ============================================================================================

#[tokio::test]
async fn build_push_contains_the_policy_created_through_the_service() {
    let Some(h) = harness().await else { return };
    let rid = RequestId::generate;
    let t = make_tenant(&h.pool).await;

    let domain = unique("checkout");
    rl::create_domain(&h.pool, &t.granted, t.team, &domain, rid())
        .await
        .expect("create domain");
    rl::create_policy(
        &h.pool,
        &t.granted,
        t.team,
        &domain,
        &unique("per-client"),
        policy_spec("bob", 100),
        rid(),
    )
    .await
    .expect("create policy");

    let composed = sync::compose_domain(t.team.org_id, t.team.id, &domain);
    let body = sync::build_push(&h.pool).await.expect("build push");
    let entry = find_entry(&body, &composed).expect("our policy must appear in the push");

    let mut want = BTreeMap::new();
    want.insert("client_id".to_string(), "bob".to_string());
    assert_eq!(entry.descriptors, want, "descriptors carried verbatim");
    assert_eq!(entry.requests_per_unit, 100, "base rpu");
    assert_eq!(entry.unit, RateLimitUnit::Minute, "unit carried");
}

// ============================================================================================
// Acceptance 3: a team override is the EFFECTIVE rpu in the push (override wins over the base).
// ============================================================================================

#[tokio::test]
async fn team_override_wins_over_base_rpu_in_the_push() {
    let Some(h) = harness().await else { return };
    let rid = RequestId::generate;
    let t = make_tenant(&h.pool).await;

    let domain = unique("checkout");
    let policy = unique("per-client");
    rl::create_domain(&h.pool, &t.granted, t.team, &domain, rid())
        .await
        .expect("domain");
    rl::create_policy(
        &h.pool,
        &t.granted,
        t.team,
        &domain,
        &policy,
        policy_spec("bob", 100),
        rid(),
    )
    .await
    .expect("policy");

    // Sanity: before the override the push shows the base rpu.
    let composed = sync::compose_domain(t.team.org_id, t.team.id, &domain);
    let before = sync::build_push(&h.pool).await.expect("build push");
    assert_eq!(
        find_entry(&before, &composed)
            .expect("entry present before override")
            .requests_per_unit,
        100
    );

    // Apply an override of 5 and re-build.
    rl::create_override(
        &h.pool,
        &t.granted,
        t.team,
        &domain,
        &policy,
        RateLimitTeamOverrideSpec {
            requests_per_unit: 5,
        },
        rid(),
    )
    .await
    .expect("create override");

    let after = sync::build_push(&h.pool).await.expect("build push");
    let entry = find_entry(&after, &composed).expect("entry present after override");
    assert_eq!(
        entry.requests_per_unit, 5,
        "the team override (5) is the effective rpu, not the base (100)"
    );
    assert_ne!(
        entry.requests_per_unit, 100,
        "base rpu must not survive an override"
    );
}

// ============================================================================================
// Acceptance 4 / acceptance #5: soft-deleted policies and domains are EXCLUDED from the push.
// ============================================================================================

#[tokio::test]
async fn deleted_policy_then_domain_disappear_from_the_push() {
    let Some(h) = harness().await else { return };
    let rid = RequestId::generate;
    let t = make_tenant(&h.pool).await;

    let domain = unique("checkout");
    let policy_a = unique("policy-a");
    let policy_b = unique("policy-b");
    rl::create_domain(&h.pool, &t.granted, t.team, &domain, rid())
        .await
        .expect("domain");
    rl::create_policy(
        &h.pool,
        &t.granted,
        t.team,
        &domain,
        &policy_a,
        policy_spec("aaa", 100),
        rid(),
    )
    .await
    .expect("policy a");
    rl::create_policy(
        &h.pool,
        &t.granted,
        t.team,
        &domain,
        &policy_b,
        policy_spec("bbb", 200),
        rid(),
    )
    .await
    .expect("policy b");

    let composed = sync::compose_domain(t.team.org_id, t.team.id, &domain);

    // Both policies under the composed domain are present.
    let present = sync::build_push(&h.pool).await.expect("build push");
    let domain_entries =
        |b: &sync::PushBody| b.policies.iter().filter(|p| p.domain == composed).count();
    assert_eq!(
        domain_entries(&present),
        2,
        "both freshly-created policies appear under the composed domain"
    );

    // Delete policy A (version 1) via the service. It must vanish; B must remain.
    rl::delete_policy(&h.pool, &t.granted, t.team, &domain, &policy_a, 1, rid())
        .await
        .expect("delete policy a");
    let after_policy_delete = sync::build_push(&h.pool).await.expect("build push");
    assert_eq!(
        domain_entries(&after_policy_delete),
        1,
        "soft-deleted policy must be excluded from the push (<=60s convergence is immediate here)"
    );
    // The surviving entry is B (rpu 200), proving we removed A and not B.
    let survivor = find_entry(&after_policy_delete, &composed).expect("B survives");
    assert_eq!(survivor.requests_per_unit, 200, "the survivor is policy B");

    // Delete policy B then the domain. After deleting the domain, NO entry for that composed
    // domain may remain.
    rl::delete_policy(&h.pool, &t.granted, t.team, &domain, &policy_b, 1, rid())
        .await
        .expect("delete policy b");
    rl::delete_domain(&h.pool, &t.granted, t.team, &domain, 1, rid())
        .await
        .expect("delete domain");

    let after_domain_delete = sync::build_push(&h.pool).await.expect("build push");
    assert_eq!(
        domain_entries(&after_domain_delete),
        0,
        "after delete_domain, none of that domain's entries remain in the push"
    );
    assert!(
        find_entry(&after_domain_delete, &composed).is_none(),
        "the composed domain is fully absent post-delete"
    );
}

// ============================================================================================
// Acceptance 5 (tenant isolation): two teams with identical domain + descriptors yield two
// distinct push entries — identical descriptors never collapse across teams.
// ============================================================================================

#[tokio::test]
async fn identical_domains_across_teams_do_not_collapse() {
    let Some(h) = harness().await else { return };
    let rid = RequestId::generate;

    let t1 = make_tenant(&h.pool).await;
    let t2 = make_tenant(&h.pool).await;

    // Same domain literal, same descriptor, in two different teams (different orgs).
    for t in [&t1, &t2] {
        rl::create_domain(&h.pool, &t.granted, t.team, "checkout", rid())
            .await
            .expect("domain");
        rl::create_policy(
            &h.pool,
            &t.granted,
            t.team,
            "checkout",
            &unique("p"),
            policy_spec("bob", 100),
            rid(),
        )
        .await
        .expect("policy");
    }

    let d1 = sync::compose_domain(t1.team.org_id, t1.team.id, "checkout");
    let d2 = sync::compose_domain(t2.team.org_id, t2.team.id, "checkout");
    assert_ne!(
        d1, d2,
        "the namespaced prefixes must differ even for the same raw domain name"
    );

    let body = sync::build_push(&h.pool).await.expect("build push");
    let e1 = find_entry(&body, &d1).expect("team 1 entry");
    let e2 = find_entry(&body, &d2).expect("team 2 entry");

    // Identical descriptors, but distinct composed domains — no cross-team merge.
    assert_eq!(
        e1.descriptors, e2.descriptors,
        "descriptors are identical..."
    );
    assert_ne!(
        e1.domain, e2.domain,
        "...yet the entries stay separate by domain"
    );
}

// ============================================================================================
// Acceptance 6: reconcile_once over real HTTP against a stub axum server.
// ============================================================================================

/// Shared buffer of every JSON body and Authorization header the stub received.
type Captured = Arc<Mutex<Vec<(serde_json::Value, Option<String>)>>>;

/// Spawn a stub RLS admin server on 127.0.0.1:0. `status` is returned for the policies route;
/// the JSON body of every POST is captured into the returned buffer. Returns (addr, captured).
async fn spawn_stub(status: axum::http::StatusCode) -> (SocketAddr, Captured) {
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));

    async fn handler(
        State((status, captured)): State<(axum::http::StatusCode, Captured)>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        let authorization = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        captured.lock().expect("lock").push((body, authorization));
        status
    }

    let app = Router::new()
        .route("/api/v1/admin/rls/policies", post(handler))
        .with_state((status, Arc::clone(&captured)));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });

    (addr, captured)
}

async fn spawn_auth_stub(expected_authorization: &'static str) -> (SocketAddr, Captured) {
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));

    async fn handler(
        State((expected, captured)): State<(&'static str, Captured)>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        let authorization = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let status = if authorization.as_deref() == Some(expected) {
            axum::http::StatusCode::NO_CONTENT
        } else {
            axum::http::StatusCode::UNAUTHORIZED
        };
        captured.lock().expect("lock").push((body, authorization));
        status
    }

    let app = Router::new()
        .route("/api/v1/admin/rls/policies", post(handler))
        .with_state((expected_authorization, Arc::clone(&captured)));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });

    (addr, captured)
}

fn sample_push_body() -> sync::PushBody {
    sync::PushBody {
        policies: vec![sync::PushPolicy {
            domain: "orgA|teamA|checkout".to_string(),
            descriptors: [("client_id".to_string(), "bob".to_string())]
                .into_iter()
                .collect(),
            requests_per_unit: 100,
            unit: RateLimitUnit::Minute,
        }],
    }
}

#[tokio::test]
async fn push_policy_set_sends_configured_admin_credential() {
    let (addr, captured) = spawn_auth_stub("Bearer expected-token").await;
    let client = reqwest::Client::new();
    let credential = sync::AdminCredential::new("expected-token".to_string()).expect("credential");

    let count = sync::push_policy_set(
        &sample_push_body(),
        &format!("http://{addr}"),
        Some(&credential),
        &client,
    )
    .await
    .expect("matching credential must succeed");

    assert_eq!(count, 1);
    let bodies = captured.lock().expect("lock");
    assert_eq!(
        bodies[0].1.as_deref(),
        Some("Bearer expected-token"),
        "push_policy_set sends the configured credential"
    );
}

#[tokio::test]
async fn push_policy_set_treats_rejected_admin_credential_as_sync_failure() {
    let (addr, captured) = spawn_auth_stub("Bearer expected-token").await;
    let client = reqwest::Client::new();
    let credential = sync::AdminCredential::new("wrong-token".to_string()).expect("credential");

    sync::push_policy_set(
        &sample_push_body(),
        &format!("http://{addr}"),
        Some(&credential),
        &client,
    )
    .await
    .expect_err("401 from RLS admin endpoint must fail the sync");

    let bodies = captured.lock().expect("lock");
    assert_eq!(
        bodies[0].1.as_deref(),
        Some("Bearer wrong-token"),
        "push_policy_set sent the configured credential before surfacing rejection"
    );
}

#[tokio::test]
async fn reconcile_once_posts_full_snapshot_and_succeeds_on_204() {
    let Some(h) = harness().await else { return };
    let rid = RequestId::generate;
    let t = make_tenant(&h.pool).await;

    let domain = unique("checkout");
    rl::create_domain(&h.pool, &t.granted, t.team, &domain, rid())
        .await
        .expect("domain");
    rl::create_policy(
        &h.pool,
        &t.granted,
        t.team,
        &domain,
        &unique("p"),
        policy_spec("bob", 100),
        rid(),
    )
    .await
    .expect("policy");
    let composed = sync::compose_domain(t.team.org_id, t.team.id, &domain);

    let (addr, captured) = spawn_stub(axum::http::StatusCode::NO_CONTENT).await;
    let client = reqwest::Client::new();

    let credential = sync::AdminCredential::new("sync-token".to_string()).expect("credential");
    let count = sync::reconcile_once(
        &h.pool,
        &format!("http://{addr}"),
        Some(&credential),
        &client,
    )
    .await
    .expect("reconcile_once over 204 must succeed");
    assert!(count >= 1, "returns the number of pushed policies");

    // The stub captured a body whose `policies` array includes OUR composed-domain entry.
    let bodies = captured.lock().expect("lock");
    assert_eq!(bodies.len(), 1, "exactly one POST per reconcile");
    assert_eq!(
        bodies[0].1.as_deref(),
        Some("Bearer sync-token"),
        "configured admin credential is sent as a bearer token"
    );
    let policies = bodies[0]
        .0
        .get("policies")
        .and_then(|v| v.as_array())
        .expect("body has a policies array");
    assert_eq!(
        policies.len(),
        count,
        "the returned count matches the pushed policy array length"
    );
    let ours = policies
        .iter()
        .find(|p| p.get("domain").and_then(|d| d.as_str()) == Some(composed.as_str()))
        .expect("the captured snapshot includes our composed-domain entry");
    assert_eq!(
        ours.get("requests_per_unit").and_then(|v| v.as_u64()),
        Some(100),
        "pushed JSON carries the rpu"
    );
}

#[tokio::test]
async fn reconcile_once_errors_on_non_2xx_response() {
    let Some(h) = harness().await else { return };
    // No data needed; even an empty snapshot must surface the 500.
    let (addr, _captured) = spawn_stub(axum::http::StatusCode::INTERNAL_SERVER_ERROR).await;
    let client = reqwest::Client::new();

    let credential = sync::AdminCredential::new("sync-token".to_string()).expect("credential");
    let err = sync::reconcile_once(
        &h.pool,
        &format!("http://{addr}"),
        Some(&credential),
        &client,
    )
    .await
    .expect_err("a 500 from the RLS admin endpoint must be an error");
    // Don't over-fit the exact code, but a transport/admin failure must not masquerade as Ok.
    let _ = err;
}

#[tokio::test]
async fn reconcile_once_errors_when_admin_credential_is_rejected() {
    let Some(h) = harness().await else { return };
    let (addr, captured) = spawn_auth_stub("Bearer expected-token").await;
    let client = reqwest::Client::new();
    let credential = sync::AdminCredential::new("wrong-token".to_string()).expect("credential");

    sync::reconcile_once(
        &h.pool,
        &format!("http://{addr}"),
        Some(&credential),
        &client,
    )
    .await
    .expect_err("401 from RLS admin credential rejection must fail sync");

    let bodies = captured.lock().expect("lock");
    assert_eq!(
        bodies[0].1.as_deref(),
        Some("Bearer wrong-token"),
        "sync sent the configured credential and surfaced rejection"
    );
}

#[tokio::test]
async fn reconcile_once_errors_on_unreachable_endpoint() {
    let Some(h) = harness().await else { return };
    let client = reqwest::Client::new();

    // Loopback port 1 is reserved and never listening: the connection must fail, surfacing Err.
    let credential = sync::AdminCredential::new("sync-token".to_string()).expect("credential");
    let err = sync::reconcile_once(&h.pool, "http://127.0.0.1:1", Some(&credential), &client)
        .await
        .expect_err("an unreachable RLS admin endpoint must be an error");
    let _ = err;
}

// ============================================================================================
// Acceptance 7: authorize_repush is platform-governance only.
// ============================================================================================

#[tokio::test]
async fn authorize_repush_denies_ordinary_user_and_allows_platform_admin() {
    let Some(h) = harness().await else { return };
    let t = make_tenant(&h.pool).await;

    // Deny case: an ordinary, fully-granted team member is NOT platform governance.
    let err = sync::authorize_repush(&t.granted)
        .expect_err("a non-platform principal must be denied the repush trigger");
    assert_eq!(
        err.code,
        fp_domain::ErrorCode::Forbidden,
        "repush denial is Forbidden"
    );

    // Allow case: a platform admin (admin:all) may trigger a repush.
    let platform_admin = PrincipalCtx::User {
        user_id: t.granted_user_id(),
        platform_admin: true,
        org_selector_required: false,
        org: None,
        grants: GrantSet::default(),
    };
    sync::authorize_repush(&platform_admin).expect("platform admin may force a repush");
}

impl Tenant {
    fn granted_user_id(&self) -> fp_domain::UserId {
        match &self.granted {
            PrincipalCtx::User { user_id, .. } => *user_id,
            _ => panic!("test tenant principal is always a User"),
        }
    }
}
