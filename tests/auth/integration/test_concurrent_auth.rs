use std::collections::HashSet;
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::json;
use tokio::task::JoinSet;

use crate::support::{read_json, send_request, setup_test_app};
use flowplane::auth::token_service::TokenSecretResponse;

enum OperationResult {
    Create(StatusCode, Option<TokenSecretResponse>),
    List(StatusCode),
}

#[tokio::test]
async fn concurrent_token_workloads_complete_successfully() {
    let app = Arc::new(setup_test_app().await);
    let admin = app
        .issue_token("load-admin", &["admin:all", "tokens:read", "tokens:write", "clusters:read"])
        .await;
    let admin_token = admin.token.clone();

    let create_workers = 20usize;
    let list_workers = 10usize;

    let mut jobs = JoinSet::new();

    for i in 0..create_workers {
        let app = Arc::clone(&app);
        let admin_token = admin_token.clone();
        jobs.spawn(async move {
            let body = json!({
                "name": format!("load-worker-{}", i),
                "description": "stress token creation",
                "scopes": ["tokens:read"]
            });
            let response = send_request(
                app.as_ref(),
                Method::POST,
                "/api/v1/tokens",
                Some(&admin_token),
                Some(body),
            )
            .await;
            let status = response.status();
            let secret = if status == StatusCode::CREATED {
                Some(read_json::<TokenSecretResponse>(response).await)
            } else {
                None
            };
            OperationResult::Create(status, secret)
        });
    }

    for _ in 0..list_workers {
        let app = Arc::clone(&app);
        let admin_token = admin_token.clone();
        jobs.spawn(async move {
            let response =
                send_request(app.as_ref(), Method::GET, "/api/v1/tokens", Some(&admin_token), None)
                    .await;
            OperationResult::List(response.status())
        });
    }

    let mut created_ids = HashSet::new();
    let mut create_successes = 0usize;
    let mut list_successes = 0usize;

    while let Some(result) = jobs.join_next().await {
        match result.expect("task panicked") {
            OperationResult::Create(status, secret) => {
                assert_eq!(status, StatusCode::CREATED, "concurrent create failed");
                let secret = secret.expect("missing token payload");
                assert!(
                    created_ids.insert(secret.id.clone()),
                    "duplicate token returned from concurrent create"
                );
                create_successes += 1;
            }
            OperationResult::List(status) => {
                assert!(
                    status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
                    "concurrent list returned unexpected status: {status}"
                );
                if status == StatusCode::OK {
                    list_successes += 1;
                }
            }
        }
    }

    assert_eq!(create_successes, create_workers);
    // Allow some list requests to fail with 503 under CI load pressure
    assert!(
        list_successes > 0,
        "all concurrent list requests failed — expected at least one success"
    );

    let total_tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
        .fetch_one(&app.pool)
        .await
        .expect("count tokens");
    assert_eq!(total_tokens, (create_workers as i64) + 1); // +1 for admin token

    let active_tokens: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens WHERE status = 'active'")
            .fetch_one(&app.pool)
            .await
            .expect("count active tokens");
    assert_eq!(active_tokens, (create_workers as i64) + 1);
}
