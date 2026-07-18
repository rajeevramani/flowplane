//! Black-box contract tests for
//! `GET /api/v1/teams/{team}/api-definitions/{name}/specs/{version}/events`
//! (per-spec-version review-event history read model).
//!
//! Written from acceptance criteria only — the endpoint is exercised strictly over HTTP
//! through the production router/middleware stack. Fixtures use pre-existing public
//! endpoints (POST api-definitions) plus direct sqlx INSERTs into
//! `spec_version_review_events` with explicit ids and created_at timestamps (fixtures
//! may touch the DB; the endpoint under test never is).
//!
//! Contract under test:
//! - 200 with the uniform Page envelope {"items", "total", "limit", "offset"}.
//! - Items are the FULL review-event history of one spec version, ordered OLDEST first
//!   (created_at ASC, id ASC tie-break) — every event appears, not just the latest.
//! - Item shape: id (uuid), decision, actor_type, optional actor_id (OMITTED when
//!   null), reason, metadata (object, echoed verbatim), created_at. Never any spec
//!   document content.
//! - limit/offset pagination; total = full event count for the version.
//! - Unknown API name -> 404; known API + nonexistent version -> 404.
//! - Version that exists but has zero events -> 200 empty page, NOT 404.
//! - Cross-org caller -> 404 (anti-enumeration), by team name AND team UUID.
//! - Same-org caller with no grant on the team -> 403 with NO existence oracle
//!   (existing version == absent version status; convention pinned in
//!   spec_versions_list.rs / rate_limit_api.rs).
//!
//! Parallel-safe: every org/team/user/api name is uuid-suffixed and unique per test;
//! event ids are random uuids; no global row-count assertions; in-process router via
//! `oneshot` (no TCP ports). Skipped (with a notice) when FLOWPLANE_TEST_DATABASE_URL
//! is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct Env {
    app: axum::Router,
    issuer: DevIssuer,
    pool: PgPool,
}

async fn env() -> Option<Env> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    Some(Env { app, issuer, pool })
}

/// Create a user with one org membership and mint a bearer token for them.
async fn user_with_org_role(env: &Env, org_id: OrgId, role: OrgRole) -> (UserId, String) {
    let subject = unique("sub");
    let email = format!("{}@test", unique("user"));
    let user = identity::upsert_user_by_subject(&env.pool, &subject, &email, "Test User")
        .await
        .expect("user");
    identity::add_org_membership(&env.pool, user, org_id, role)
        .await
        .expect("org membership");
    let token = env
        .issuer
        .mint(&subject, &email, "Test User", 600)
        .expect("mint");
    (user, token)
}

fn request(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

/// GET as `token`, returning (status, request-id header, JSON body).
async fn get_json(
    env: &Env,
    uri: &str,
    token: &str,
) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    let response = env
        .app
        .clone()
        .oneshot(request("GET", uri, token, None))
        .await
        .expect("response");
    let status = response.status();
    let rid = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok());
    (status, rid, json_of(response).await)
}

/// Assert a response body is the standard error envelope for `code`.
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not data: {body}"
    );
    assert_eq!(body["code"], code, "unexpected error code in {body}");
    assert!(
        body.get("items").is_none() && body.get("total").is_none(),
        "error envelope must not carry page fields: {body}"
    );
    let rid = rid.expect("x-request-id header present");
    assert_eq!(
        body["request_id"],
        rid.to_string(),
        "envelope and header request id agree"
    );
}

/// Assert the uniform Page envelope shape is present.
fn assert_page_envelope(body: &serde_json::Value) {
    assert!(body["items"].is_array(), "items array present: {body}");
    assert!(
        body["total"].is_i64() || body["total"].is_u64(),
        "total is a number: {body}"
    );
    assert!(
        body["limit"].is_i64() || body["limit"].is_u64(),
        "limit is a number: {body}"
    );
    assert!(
        body["offset"].is_i64() || body["offset"].is_u64(),
        "offset is a number: {body}"
    );
}

struct TeamFixture {
    org_id: OrgId,
    team_id: TeamId,
    team_name: String,
}

/// One uuid-unique org with one uuid-unique team.
async fn org_with_team(env: &Env) -> TeamFixture {
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&env.pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    TeamFixture {
        org_id: org.id,
        team_id: team.id,
        team_name: team.name,
    }
}

/// Create an API definition over HTTP with an inline OpenAPI document, producing the
/// imported spec version 1 (which starts with ZERO review events). `marker` is a
/// uuid-unique string planted inside the spec document so leak assertions can grep for
/// it in event responses.
async fn create_api_with_v1(env: &Env, token: &str, team_name: &str, marker: &str) -> String {
    let api_name = unique("api");
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/teams/{team_name}/api-definitions"),
            token,
            Some(serde_json::json!({
                "name": api_name,
                "display_name": "Review Events Fixture",
                "openapi": {
                    "openapi": "3.0.3",
                    "info": {"title": marker, "version": "1.0.0"},
                    "paths": {
                        "/items": {"get": {"operationId": "listItems"}}
                    }
                }
            })),
        ))
        .await
        .expect("create api");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create api definition fixture"
    );
    let body = json_of(response).await;
    assert_eq!(body["latest_spec"]["version"], 1, "import produced v1");
    api_name
}

/// Resolve the API definition's row id (fixture lookup, not the endpoint under test).
async fn api_definition_id(pool: &PgPool, team_id: TeamId, api_name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM api_definitions WHERE team_id = $1 AND name = $2")
        .bind(team_id.as_uuid())
        .bind(api_name)
        .fetch_one(pool)
        .await
        .expect("api definition id")
}

/// Resolve a spec version's row id by number (fixture lookup).
async fn spec_version_id(pool: &PgPool, api_id: Uuid, version: i64) -> Uuid {
    sqlx::query_scalar("SELECT id FROM spec_versions WHERE api_definition_id = $1 AND version = $2")
        .bind(api_id)
        .bind(version)
        .fetch_one(pool)
        .await
        .expect("spec version id")
}

/// A fully-specified event fixture: explicit id, actor, reason, metadata, and an
/// EXPLICIT created_at (RFC 3339 string, cast server-side) so ordering — including
/// exact created_at ties — is deterministic under parallel test runs.
#[allow(clippy::too_many_arguments)]
async fn seed_event(
    env: &Env,
    fx: &TeamFixture,
    api_id: Uuid,
    spec_version_id: Uuid,
    id: Uuid,
    decision: &str,
    actor_type: &str,
    actor_id: Option<Uuid>,
    reason: &str,
    metadata: serde_json::Value,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO spec_version_review_events \
         (id, team_id, org_id, api_definition_id, spec_version_id, decision, actor_type, \
          actor_id, reason, metadata, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::timestamptz)",
    )
    .bind(id)
    .bind(fx.team_id.as_uuid())
    .bind(fx.org_id.as_uuid())
    .bind(api_id)
    .bind(spec_version_id)
    .bind(decision)
    .bind(actor_type)
    .bind(actor_id)
    .bind(reason)
    .bind(&metadata)
    .bind(created_at)
    .execute(&env.pool)
    .await
    .expect("seed review event");
}

fn events_uri(team: &str, api: &str, version: i64) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/specs/{version}/events")
}

/// Standard four-event lifecycle history seeded onto one spec version at strictly
/// increasing timestamps. Returns the event ids in seeded (chronological) order.
async fn seed_lifecycle_history(
    env: &Env,
    fx: &TeamFixture,
    api_id: Uuid,
    v_id: Uuid,
    actor: Option<Uuid>,
) -> Vec<Uuid> {
    let decisions = ["submitted", "reviewed", "published", "unpublished"];
    let mut ids = Vec::new();
    for (n, decision) in decisions.iter().enumerate() {
        let id = Uuid::new_v4();
        seed_event(
            env,
            fx,
            api_id,
            v_id,
            id,
            decision,
            "user",
            actor,
            &format!("reason-{decision}"),
            serde_json::json!({"step": n, "decision": decision}),
            &format!("2026-01-01T00:00:{:02}Z", 10 + n * 10),
        )
        .await;
        ids.push(id);
    }
    ids
}

// --- Criterion 1 & 7: full history oldest-first, verbatim fields, actor_id optics ----

#[tokio::test]
async fn lists_full_event_history_oldest_first_with_verbatim_fields() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (user_id, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &marker).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let v1_id = spec_version_id(&env.pool, api_id, 1).await;

    // Full lifecycle: submitted -> reviewed -> published -> unpublished, strictly
    // increasing created_at. The first two carry a real actor_id; the last two are
    // NULL-actor system events (criterion 7: key must be ABSENT, not null).
    let e_submitted = Uuid::new_v4();
    let e_reviewed = Uuid::new_v4();
    let e_published = Uuid::new_v4();
    let e_unpublished = Uuid::new_v4();
    let meta_submitted = serde_json::json!({"source": "import", "note": unique("meta")});
    let meta_reviewed = serde_json::json!({"reviewer": "alice", "score": 7});
    let meta_published = serde_json::json!({});
    let meta_unpublished = serde_json::json!({"rollback": true});
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        e_submitted,
        "submitted",
        "user",
        Some(user_id.as_uuid()),
        "initial submission",
        meta_submitted.clone(),
        "2026-01-01T00:00:10Z",
    )
    .await;
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        e_reviewed,
        "reviewed",
        "user",
        Some(user_id.as_uuid()),
        "looks good",
        meta_reviewed.clone(),
        "2026-01-01T00:00:20Z",
    )
    .await;
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        e_published,
        "published",
        "system",
        None,
        "auto-publish",
        meta_published.clone(),
        "2026-01-01T00:00:30Z",
    )
    .await;
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        e_unpublished,
        "unpublished",
        "system",
        None,
        "rolled back",
        meta_unpublished.clone(),
        "2026-01-01T00:00:40Z",
    )
    .await;

    let (status, _, body) = get_json(&env, &events_uri(&fx.team_name, &api_name, 1), &token).await;
    assert_eq!(status, StatusCode::OK, "list review events: {body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 4, "full history counted: {body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(
        items.len(),
        4,
        "EVERY event appears, not just the latest: {body}"
    );

    // Oldest first (created_at ASC): submitted, reviewed, published, unpublished.
    let ids: Vec<&str> = items
        .iter()
        .map(|i| i["id"].as_str().expect("id string"))
        .collect();
    assert_eq!(
        ids,
        [
            e_submitted.to_string(),
            e_reviewed.to_string(),
            e_published.to_string(),
            e_unpublished.to_string()
        ]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>(),
        "events ordered OLDEST first by created_at: {body}"
    );
    let decisions: Vec<&str> = items
        .iter()
        .map(|i| i["decision"].as_str().expect("decision"))
        .collect();
    assert_eq!(
        decisions,
        vec!["submitted", "reviewed", "published", "unpublished"],
        "{body}"
    );

    // Verbatim field rendering.
    for item in items {
        Uuid::parse_str(item["id"].as_str().expect("id")).expect("id is a uuid");
        assert!(item["actor_type"].is_string(), "actor_type present: {body}");
        assert!(item["reason"].is_string(), "reason present: {body}");
        assert!(
            item["metadata"].is_object(),
            "metadata is an object: {body}"
        );
        assert!(item["created_at"].is_string(), "created_at present: {body}");
        assert!(
            item.get("spec").is_none(),
            "event items must never carry spec content: {body}"
        );
    }
    assert_eq!(items[0]["reason"], "initial submission", "{body}");
    assert_eq!(items[1]["reason"], "looks good", "{body}");
    assert_eq!(
        items[0]["metadata"], meta_submitted,
        "metadata echoed verbatim: {body}"
    );
    assert_eq!(
        items[1]["metadata"], meta_reviewed,
        "metadata echoed verbatim: {body}"
    );
    assert_eq!(items[2]["metadata"], meta_published, "{body}");
    assert_eq!(items[3]["metadata"], meta_unpublished, "{body}");
    assert_eq!(items[0]["actor_type"], "user", "{body}");
    assert_eq!(items[2]["actor_type"], "system", "{body}");

    // actor_id: present as a uuid when set...
    assert_eq!(
        items[0]["actor_id"],
        user_id.as_uuid().to_string(),
        "actor_id rendered when non-null: {body}"
    );
    // ...and the KEY is strictly ABSENT (not null-with-key) when NULL.
    assert!(
        items[2].get("actor_id").is_none() && items[3].get("actor_id").is_none(),
        "actor_id key must be OMITTED for null-actor events: {body}"
    );

    // Adversarial content-leak check: the marker planted in the spec document must not
    // appear anywhere in the events response, under any field name.
    assert!(
        !body.to_string().contains(&marker),
        "spec document content leaked into the event history: {body}"
    );
}

// --- Criterion 2: identical created_at -> ascending id tie-break ----------------------

#[tokio::test]
async fn identical_created_at_ties_break_by_ascending_id() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &unique("mark")).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let v1_id = spec_version_id(&env.pool, api_id, 1).await;

    // Two random uuids, sorted so `lo < hi` is known a priori. Seed the HIGHER id
    // first so insertion order cannot masquerade as the tie-break.
    let mut pair = [Uuid::new_v4(), Uuid::new_v4()];
    pair.sort();
    let [lo, hi] = pair;
    const TS: &str = "2026-01-01T00:00:30Z";
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        hi,
        "reviewed",
        "user",
        None,
        "second by id",
        serde_json::json!({}),
        TS,
    )
    .await;
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        lo,
        "submitted",
        "user",
        None,
        "first by id",
        serde_json::json!({}),
        TS,
    )
    .await;

    let (status, _, body) = get_json(&env, &events_uri(&fx.team_name, &api_name, 1), &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{body}");
    assert_eq!(
        items[0]["id"],
        lo.to_string(),
        "equal created_at must order by ascending id (lower id first): {body}"
    );
    assert_eq!(items[1]["id"], hi.to_string(), "{body}");
}

// --- Criterion 3: pagination ----------------------------------------------------------

#[tokio::test]
async fn paginates_events_with_limit_offset_keeping_total_stable() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &unique("mark")).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let v1_id = spec_version_id(&env.pool, api_id, 1).await;
    let ids = seed_lifecycle_history(&env, &fx, api_id, v1_id, None).await;

    // limit=2 offset=1 -> events 2 and 3 of 4 (chronological), total stays 4.
    let uri = format!(
        "{}?limit=2&offset=1",
        events_uri(&fx.team_name, &api_name, 1)
    );
    let (status, _, body) = get_json(&env, &uri, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 4, "total is page-independent: {body}");
    assert_eq!(body["limit"], 2, "limit echoed: {body}");
    assert_eq!(body["offset"], 1, "offset echoed: {body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{body}");
    assert_eq!(
        items[0]["id"],
        ids[1].to_string(),
        "offset=1 skips only the oldest event: {body}"
    );
    assert_eq!(items[1]["id"], ids[2].to_string(), "{body}");
    assert_eq!(items[0]["decision"], "reviewed", "{body}");
    assert_eq!(items[1]["decision"], "published", "{body}");
}

// --- Criterion 4: version exists, zero events -> 200 empty page -----------------------

#[tokio::test]
async fn version_with_no_events_returns_empty_page_not_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    // The imported v1 starts with zero review events — the version exists regardless.
    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &unique("mark")).await;

    let (status, _, body) = get_json(&env, &events_uri(&fx.team_name, &api_name, 1), &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an existing version with no events is 200, NOT 404: {body}"
    );
    assert_page_envelope(&body);
    assert_eq!(body["total"], 0, "{body}");
    assert_eq!(
        body["items"].as_array().expect("items").len(),
        0,
        "empty items for an event-less version: {body}"
    );
}

// --- Criterion 5: unknown version / unknown API -> 404 --------------------------------

#[tokio::test]
async fn unknown_version_and_unknown_api_return_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &unique("mark")).await;

    // Known API, nonexistent version 999.
    let (status, rid, body) =
        get_json(&env, &events_uri(&fx.team_name, &api_name, 999), &token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "nonexistent version must read as absent, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);

    // Unknown API name, any version.
    let ghost = unique("ghost");
    let (status, rid, body) = get_json(&env, &events_uri(&fx.team_name, &ghost, 1), &token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown api name must read as absent, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
}

// --- Criterion 6a: cross-org anti-enumeration -----------------------------------------

#[tokio::test]
async fn cross_org_caller_gets_404_for_name_and_uuid_team_refs() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, owner_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &owner_token, &fx.team_name, &marker).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let v1_id = spec_version_id(&env.pool, api_id, 1).await;
    let reason_marker = unique("reasonmark");
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        Uuid::new_v4(),
        "submitted",
        "user",
        None,
        &reason_marker,
        serde_json::json!({"m": reason_marker}),
        "2026-01-01T00:00:10Z",
    )
    .await;

    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, foreign_token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    for team_ref in [fx.team_name.clone(), fx.team_id.as_uuid().to_string()] {
        let (status, rid, body) =
            get_json(&env, &events_uri(&team_ref, &api_name, 1), &foreign_token).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "403 for {team_ref} confirms the team exists to an outsider: {body}"
        );
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "cross-org access to {team_ref} must read as absent, got {status}: {body}"
        );
        assert_error_envelope(&body, "not_found", rid);
        assert!(
            !body.to_string().contains(&reason_marker),
            "review event data leaked cross-org: {body}"
        );
    }
}

// --- Criterion 6b: same-org caller without a grant on the team ------------------------

#[tokio::test]
async fn same_org_caller_without_grant_is_denied_with_no_version_oracle() {
    let Some(env) = env().await else { return };
    // One org, two teams: the API lives in team A; the caller is granted on team B only.
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&env.pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&env.pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");
    let fx = TeamFixture {
        org_id: org.id,
        team_id: team_a.id,
        team_name: team_a.name.clone(),
    };
    let (_, admin_token) = user_with_org_role(&env, org.id, OrgRole::Admin).await;

    let api_name = create_api_with_v1(&env, &admin_token, &fx.team_name, &unique("mark")).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let v1_id = spec_version_id(&env.pool, api_id, 1).await;
    let reason_marker = unique("reasonmark");
    seed_event(
        &env,
        &fx,
        api_id,
        v1_id,
        Uuid::new_v4(),
        "submitted",
        "user",
        None,
        &reason_marker,
        serde_json::json!({}),
        "2026-01-01T00:00:10Z",
    )
    .await;

    // Sanity (auth contract): a member holding api-definitions:read ON TEAM A reads the
    // events — the denial below can never pass vacuously against a broken read path.
    let (granted, granted_token) = user_with_org_role(&env, org.id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        granted,
        org.id,
        team_a.id,
        Resource::ApiDefinitions,
        Action::Read,
        None,
    )
    .await
    .expect("api-definitions:read on team a");
    let (status, _, body) = get_json(
        &env,
        &events_uri(&fx.team_name, &api_name, 1),
        &granted_token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "api-definitions:read grant on the team must open the events, got {status}: {body}"
    );
    assert_eq!(body["total"], 1, "granted reader sees the event: {body}");

    // The no-grant caller: same org, api-definitions:read granted ONLY on team B.
    let (holder, no_grant_token) = user_with_org_role(&env, org.id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        holder,
        org.id,
        team_b.id,
        Resource::ApiDefinitions,
        Action::Read,
        None,
    )
    .await
    .expect("api-definitions:read on team b");

    // Existing version 1 vs never-existing version 999 under team A.
    let (status_existing, rid_existing, body_existing) = get_json(
        &env,
        &events_uri(&fx.team_name, &api_name, 1),
        &no_grant_token,
    )
    .await;
    let (status_absent, _, body_absent) = get_json(
        &env,
        &events_uri(&fx.team_name, &api_name, 999),
        &no_grant_token,
    )
    .await;

    // Load-bearing invariant: the denial must not vary with version existence, so the
    // status can never be used to probe which spec versions team A owns.
    assert_eq!(
        status_existing, status_absent,
        "same-org no-grant denial must be identical for existing vs absent versions \
         (no oracle): existing={body_existing} absent={body_absent}"
    );
    assert!(
        status_existing == StatusCode::FORBIDDEN || status_existing == StatusCode::NOT_FOUND,
        "no-grant caller must be denied, got {status_existing}: {body_existing}"
    );
    // Pin the codebase's current convention (team-level NoMatchingGrant -> 403, as
    // pinned for the previous slice in spec_versions_list.rs).
    assert_eq!(
        status_existing,
        StatusCode::FORBIDDEN,
        "pinned convention for same-org no-grant reads, body: {body_existing}"
    );
    assert_error_envelope(&body_existing, "forbidden", rid_existing);

    // No event data may ride along on the denial.
    assert!(
        !body_existing.to_string().contains(&reason_marker),
        "review event data leaked to a no-grant caller: {body_existing}"
    );
}
