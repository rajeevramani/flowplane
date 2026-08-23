//! fpv2-a09.2 black-box REST contract for deleting an unreferenced secret.
//!
//! The success path was independently authored from acceptance criteria under a production-source
//! firewall. Producer-owned assertions extend it with HTTP precondition failures.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn deleting_unreferenced_secret_removes_it_and_emits_redacted_evidence() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    std::env::set_var(
        "FLOWPLANE_SECRET_ENCRYPTION_KEY",
        "12345678901234567890123456789012",
    );
    let pool = fp_storage::connect(&url, 4)
        .await
        .expect("connect real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");

    let issuer = DevIssuer::generate().expect("OIDC issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("load test JWKS");
    let subject = unique("secret-delete-subject");
    let email = format!("{}@test.invalid", unique("secret-delete"));
    let token = issuer
        .mint(&subject, &email, "Secret Delete", 600)
        .expect("mint admin token");

    let org = identity::create_org(&pool, &unique("secret-delete-org"), "")
        .await
        .expect("create org");
    let team = identity::create_team(&pool, org.id, &unique("secret-delete-team"), "")
        .await
        .expect("create team");
    let user = identity::upsert_user_by_subject(&pool, &subject, &email, "Secret Delete")
        .await
        .expect("create user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("grant org admin membership");

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

    let secret_name = unique("delete-me");
    let collection = format!("/api/v1/teams/{}/secrets", team.name);
    let item = format!("{collection}/{secret_name}");
    let encoded_secret = "c2VjcmV0LWRlbGV0ZS1saWZlY3ljbGUtdmFsdWU=";
    let plaintext_secret = "secret-delete-lifecycle-value";

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&collection)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": secret_name,
                        "description": "unreferenced deletion fixture",
                        "spec": {
                            "type": "generic_secret",
                            "secret": encoded_secret
                        }
                    })
                    .to_string(),
                ))
                .expect("create request"),
        )
        .await
        .expect("create response");
    let create_status = created.status();
    let create_body = body_bytes(created).await;
    assert_eq!(
        create_status,
        StatusCode::CREATED,
        "secret fixture response: {}",
        String::from_utf8_lossy(&create_body)
    );
    let created: serde_json::Value =
        serde_json::from_slice(&create_body).expect("create response JSON");
    assert_eq!(created["revision"], 1, "new secret starts at revision 1");

    for if_match in [None, Some("not-a-revision")] {
        let mut request = Request::builder()
            .method("DELETE")
            .uri(&item)
            .header("authorization", format!("Bearer {token}"))
            .header("x-request-id", Uuid::now_v7().to_string());
        if let Some(value) = if_match {
            request = request.header("if-match", value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).expect("invalid delete request"))
            .await
            .expect("invalid delete response");
        let status = response.status();
        let body = body_bytes(response).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "missing/invalid If-Match must fail closed: {}",
            String::from_utf8_lossy(&body)
        );
        let still_present = fp_storage::repos::secrets::get_secret(&pool, team.id, &secret_name)
            .await
            .expect("get secret after invalid delete");
        assert!(still_present.is_some());
    }

    let request_id = Uuid::now_v7();
    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&item)
                .header("authorization", format!("Bearer {token}"))
                .header("if-match", "1")
                .header("x-request-id", request_id.to_string())
                .body(Body::empty())
                .expect("delete request"),
        )
        .await
        .expect("delete response");
    let delete_status = deleted.status();
    let delete_body = body_bytes(deleted).await;
    assert_eq!(
        delete_status,
        StatusCode::NO_CONTENT,
        "current If-Match and admin Secrets/Delete must remove an unreferenced secret; body: {}",
        String::from_utf8_lossy(&delete_body)
    );
    assert!(delete_body.is_empty(), "204 response body must be empty");

    let get = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&item)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("get request"),
        )
        .await
        .expect("get response");
    assert_eq!(
        get.status(),
        StatusCode::NOT_FOUND,
        "deleted secret must be absent from GET"
    );

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&collection)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("list request"),
        )
        .await
        .expect("list response");
    let list_status = list.status();
    let list_body = body_bytes(list).await;
    assert_eq!(
        list_status,
        StatusCode::OK,
        "list response: {}",
        String::from_utf8_lossy(&list_body)
    );
    let listed: serde_json::Value =
        serde_json::from_slice(&list_body).expect("secret list response JSON");
    assert!(
        listed["items"]
            .as_array()
            .expect("secret list items")
            .iter()
            .all(|secret| secret["name"] != secret_name),
        "deleted secret must be omitted from list: {listed}"
    );

    let audits: Vec<(String, String, Option<Uuid>, serde_json::Value)> = sqlx::query_as(
        "SELECT action, outcome, team_id, detail FROM audit_log \
         WHERE request_id = $1 AND action = 'secret.delete'",
    )
    .bind(request_id)
    .fetch_all(&pool)
    .await
    .expect("delete audit query");
    assert_eq!(audits.len(), 1, "DELETE must emit exactly one audit row");
    let (action, outcome, audit_team_id, detail) = &audits[0];
    assert_eq!(action, "secret.delete");
    assert_eq!(outcome, "success");
    assert_eq!(*audit_team_id, Some(team.id.as_uuid()));
    let audit_text = detail.to_string();
    assert!(
        !audit_text.contains(encoded_secret) && !audit_text.contains(plaintext_secret),
        "secret.delete audit detail must redact secret values: {detail}"
    );

    let events: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE team_id = $1 AND event_type = 'secret.deleted'",
    )
    .bind(team.id.as_uuid())
    .fetch_all(&pool)
    .await
    .expect("secret.deleted event query");
    assert_eq!(
        events.len(),
        1,
        "DELETE must emit exactly one team-scoped secret.deleted event"
    );
    let event_text = events[0].to_string();
    assert!(
        !event_text.contains(encoded_secret) && !event_text.contains(plaintext_secret),
        "secret.deleted event payload must not expose secret values: {}",
        events[0]
    );
}
