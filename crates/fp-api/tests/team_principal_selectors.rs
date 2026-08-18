//! Independent black-box acceptance RED for fpv2-ppo.1.
//!
//! Authored from the approved immutable-team-principal-selector design and existing
//! integration-test conventions without reading production source. This exercises raw REST JSON
//! through the real Axum router, OIDC validation path, and PostgreSQL database. Fixtures and
//! assertions are scoped by UUID-derived org, team, subject, and user identifiers.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::{OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7().simple())
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
        xds_degraded: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    Some(Env { app, issuer, pool })
}

struct Fixture {
    org_id: OrgId,
    team_id: TeamId,
    team_name: String,
    token: String,
}

async fn fixture(env: &Env) -> Fixture {
    let org = identity::create_org(&env.pool, &unique("selector-org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&env.pool, org.id, &unique("selector-team"), "")
        .await
        .expect("team");
    let admin_subject = unique("oidc-admin-subject");
    let admin_email = format!("{}@example.test", unique("admin"));
    let admin =
        identity::upsert_user_by_subject(&env.pool, &admin_subject, &admin_email, "Selector Admin")
            .await
            .expect("admin");
    identity::add_org_membership(&env.pool, admin, org.id, OrgRole::Admin)
        .await
        .expect("admin org membership");
    let token = env
        .issuer
        .mint(&admin_subject, &admin_email, "Selector Admin", 600)
        .expect("admin token");
    Fixture {
        org_id: org.id,
        team_id: team.id,
        team_name: team.name,
        token,
    }
}

struct Candidate {
    id: UserId,
    subject: String,
}

async fn empty_email_candidate(env: &Env, org_id: OrgId) -> Candidate {
    let subject = unique("provider-neutral-subject");
    let id = identity::upsert_user_by_subject(&env.pool, &subject, "", "Candidate")
        .await
        .expect("candidate");
    identity::add_org_membership(&env.pool, id, org_id, OrgRole::Member)
        .await
        .expect("candidate org membership");
    Candidate { id, subject }
}

struct HttpResult {
    status: StatusCode,
    request_id: Uuid,
    body: String,
}

async fn post_member(env: &Env, fx: &Fixture, body: Value) -> HttpResult {
    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/members", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.token))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .expect("x-request-id")
        .to_str()
        .expect("request id text")
        .parse()
        .expect("request id UUID");
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    HttpResult {
        status,
        request_id,
        body: String::from_utf8(bytes.to_vec()).expect("UTF-8 response"),
    }
}

async fn post_grant(env: &Env, fx: &Fixture, mut body: Value) -> HttpResult {
    body["resource"] = json!("clusters");
    body["action"] = json!("read");
    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/grants", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.token))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .expect("x-request-id")
        .to_str()
        .expect("request id text")
        .parse()
        .expect("request id UUID");
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    HttpResult {
        status,
        request_id,
        body: String::from_utf8(bytes.to_vec()).expect("UTF-8 response"),
    }
}

async fn membership_count(env: &Env, fx: &Fixture, user: UserId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM team_memberships WHERE team_id = $1 AND user_id = $2")
        .bind(fx.team_id.as_uuid())
        .bind(user.as_uuid())
        .fetch_one(&env.pool)
        .await
        .expect("membership count")
}

async fn event_count(env: &Env, fx: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE team_id = $1")
        .bind(fx.team_id.as_uuid())
        .fetch_one(&env.pool)
        .await
        .expect("event count")
}

async fn grant_count(env: &Env, fx: &Fixture, user: UserId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM user_grants \
         WHERE org_id = $1 AND team_id = $2 AND user_id = $3 \
         AND resource = 'clusters' AND action = 'read'",
    )
    .bind(fx.org_id.as_uuid())
    .bind(fx.team_id.as_uuid())
    .bind(user.as_uuid())
    .fetch_one(&env.pool)
    .await
    .expect("grant count")
}

async fn successful_audit_resources(env: &Env, request_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT resource FROM audit_log WHERE request_id = $1 AND outcome = 'success'",
    )
    .bind(request_id)
    .fetch_all(&env.pool)
    .await
    .expect("successful audit resources")
}

async fn assert_member_added_with_uuid_audit(
    env: &Env,
    fx: &Fixture,
    candidate: &Candidate,
    result: &HttpResult,
) {
    assert_eq!(
        result.status,
        StatusCode::NO_CONTENT,
        "immutable selector must add same-org active user with empty email; response: {}",
        result.body
    );
    assert_eq!(
        membership_count(env, fx, candidate.id).await,
        1,
        "selected user must become a member of the target team"
    );
    let resources = successful_audit_resources(env, result.request_id).await;
    assert_eq!(
        resources,
        vec![format!("users/{}", candidate.id)],
        "success audit target must use the resolved Flowplane user UUID"
    );
    assert!(
        !resources.join("\n").contains(&candidate.subject),
        "success audit target must not contain the raw stored subject"
    );
}

#[tokio::test]
async fn subject_adds_same_org_user_with_empty_email_to_team() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let candidate = empty_email_candidate(&env, fx.org_id).await;
    let events_before = event_count(&env, &fx).await;

    let result = post_member(&env, &fx, json!({"subject": candidate.subject})).await;

    assert_member_added_with_uuid_audit(&env, &fx, &candidate, &result).await;
    assert_eq!(
        event_count(&env, &fx).await,
        events_before,
        "team membership mutation must not emit an outbox event"
    );
}

#[tokio::test]
async fn user_id_adds_same_org_user_with_empty_email_to_team() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let candidate = empty_email_candidate(&env, fx.org_id).await;
    let events_before = event_count(&env, &fx).await;

    let result = post_member(&env, &fx, json!({"user_id": candidate.id.as_uuid()})).await;

    assert_member_added_with_uuid_audit(&env, &fx, &candidate, &result).await;
    assert_eq!(
        event_count(&env, &fx).await,
        events_before,
        "team membership mutation must not emit an outbox event"
    );
}

async fn assert_grant_added_with_uuid_audit(
    env: &Env,
    fx: &Fixture,
    candidate: &Candidate,
    result: &HttpResult,
) {
    assert_eq!(
        result.status,
        StatusCode::NO_CONTENT,
        "immutable selector must grant same-org active user with empty email; response: {}",
        result.body
    );
    assert_eq!(grant_count(env, fx, candidate.id).await, 1);
    let resources = successful_audit_resources(env, result.request_id).await;
    assert_eq!(
        resources,
        vec![format!("users/{}:clusters:read", candidate.id)]
    );
    assert!(!resources.join("\n").contains(&candidate.subject));
}

#[tokio::test]
async fn subject_grants_same_org_user_with_empty_email() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let candidate = empty_email_candidate(&env, fx.org_id).await;
    let events_before = event_count(&env, &fx).await;
    let result = post_grant(&env, &fx, json!({"subject": candidate.subject})).await;
    assert_grant_added_with_uuid_audit(&env, &fx, &candidate, &result).await;
    assert_eq!(event_count(&env, &fx).await, events_before);
}

#[tokio::test]
async fn user_id_grants_same_org_user_with_empty_email() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let candidate = empty_email_candidate(&env, fx.org_id).await;
    let events_before = event_count(&env, &fx).await;
    let result = post_grant(&env, &fx, json!({"user_id": candidate.id.as_uuid()})).await;
    assert_grant_added_with_uuid_audit(&env, &fx, &candidate, &result).await;
    assert_eq!(event_count(&env, &fx).await, events_before);
}

#[tokio::test]
async fn selector_shape_and_empty_values_fail_without_effect() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let candidate = empty_email_candidate(&env, fx.org_id).await;
    let events_before = event_count(&env, &fx).await;
    for body in [
        json!({}),
        json!({"subject": candidate.subject, "user_id": candidate.id.as_uuid()}),
        json!({"subject": "   "}),
        json!({"email": ""}),
    ] {
        let result = post_member(&env, &fx, body).await;
        assert_eq!(result.status, StatusCode::BAD_REQUEST, "{}", result.body);
        assert!(successful_audit_resources(&env, result.request_id)
            .await
            .is_empty());
    }
    assert_eq!(membership_count(&env, &fx, candidate.id).await, 0);
    assert_eq!(event_count(&env, &fx).await, events_before);
}

#[tokio::test]
async fn foreign_org_subject_is_not_found_with_no_effect() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let foreign_org = identity::create_org(&env.pool, &unique("foreign-org"), "")
        .await
        .expect("foreign org");
    let candidate = empty_email_candidate(&env, foreign_org.id).await;
    let events_before = event_count(&env, &fx).await;
    let result = post_member(&env, &fx, json!({"subject": candidate.subject})).await;
    assert_eq!(result.status, StatusCode::NOT_FOUND, "{}", result.body);
    assert!(!result.body.contains(&candidate.subject));
    assert_eq!(membership_count(&env, &fx, candidate.id).await, 0);
    assert!(successful_audit_resources(&env, result.request_id)
        .await
        .is_empty());
    assert_eq!(event_count(&env, &fx).await, events_before);
}

#[tokio::test]
async fn positional_email_contract_remains_supported_by_rest_body() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let subject = unique("email-compat-subject");
    let email = format!("{}@example.test", unique("email-compat"));
    let id = identity::upsert_user_by_subject(&env.pool, &subject, &email, "Email User")
        .await
        .expect("email user");
    identity::add_org_membership(&env.pool, id, fx.org_id, OrgRole::Member)
        .await
        .expect("email user org membership");
    let candidate = Candidate { id, subject };
    let result = post_member(&env, &fx, json!({"email": email})).await;
    assert_member_added_with_uuid_audit(&env, &fx, &candidate, &result).await;
}

#[tokio::test]
async fn inactive_subject_is_not_found_with_no_effect() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let candidate = empty_email_candidate(&env, fx.org_id).await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(candidate.id.as_uuid())
        .execute(&env.pool)
        .await
        .expect("suspend candidate");
    let events_before = event_count(&env, &fx).await;
    let result = post_member(&env, &fx, json!({"subject": candidate.subject})).await;
    assert_eq!(result.status, StatusCode::NOT_FOUND, "{}", result.body);
    assert!(!result.body.contains(&candidate.subject));
    assert_eq!(membership_count(&env, &fx, candidate.id).await, 0);
    assert!(successful_audit_resources(&env, result.request_id)
        .await
        .is_empty());
    assert_eq!(event_count(&env, &fx).await, events_before);
}

#[tokio::test]
async fn ambiguous_email_remains_a_conflict_without_effect() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let email = format!("{}@example.test", unique("ambiguous"));
    let first = identity::upsert_user_by_subject(&env.pool, &unique("ambiguous-a"), &email, "A")
        .await
        .expect("first user");
    let second = identity::upsert_user_by_subject(&env.pool, &unique("ambiguous-b"), &email, "B")
        .await
        .expect("second user");
    for user in [first, second] {
        identity::add_org_membership(&env.pool, user, fx.org_id, OrgRole::Member)
            .await
            .expect("ambiguous user org membership");
    }
    let result = post_member(&env, &fx, json!({"email": email})).await;
    assert_eq!(result.status, StatusCode::CONFLICT, "{}", result.body);
    assert_eq!(membership_count(&env, &fx, first).await, 0);
    assert_eq!(membership_count(&env, &fx, second).await, 0);
    assert!(successful_audit_resources(&env, result.request_id)
        .await
        .is_empty());
}

#[tokio::test]
async fn malformed_user_id_is_rejected_before_mutation() {
    let Some(env) = env().await else { return };
    let fx = fixture(&env).await;
    let result = post_member(&env, &fx, json!({"user_id": "not-a-uuid"})).await;
    assert_eq!(result.status, StatusCode::BAD_REQUEST, "{}", result.body);
    assert!(successful_audit_resources(&env, result.request_id)
        .await
        .is_empty());
}

#[test]
fn openapi_documents_all_optional_exact_one_selector_fields() {
    let document = serde_json::to_value(fp_api::routes::openapi_document()).expect("OpenAPI JSON");
    for schema_name in ["AddMemberBody", "AddGrantBody"] {
        let schema = &document["components"]["schemas"][schema_name];
        let properties = schema["properties"].as_object().expect("schema properties");
        for selector in ["email", "subject", "user_id"] {
            assert!(
                properties.contains_key(selector),
                "{schema_name} must expose {selector}"
            );
        }
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        for selector in ["email", "subject", "user_id"] {
            assert!(
                !required.iter().any(|field| field == selector),
                "{schema_name}.{selector} must be optional so exact-one validation can select another field"
            );
        }
        assert!(
            properties["email"]["description"]
                .as_str()
                .is_some_and(|text| text.contains("Exactly one selector is required")),
            "{schema_name} must document exact-one selector semantics"
        );
    }
}
