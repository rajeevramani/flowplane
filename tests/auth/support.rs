use std::sync::Arc;

use axum::{
    body::to_bytes,
    body::Body,
    http::{Method, Request},
    Router,
};
use flowplane::{
    auth::{
        token_service::{TokenSecretResponse, TokenService},
        validation::CreateTokenRequest,
    },
    config::SimpleXdsConfig,
    storage::{self, repository::AuditLogRepository, DbPool},
    xds::XdsState,
};
use hyper::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

pub struct TestApp {
    state: Arc<XdsState>,
    pub pool: DbPool,
    pub token_service: TokenService,
}

impl TestApp {
    pub fn router(&self) -> Router {
        flowplane::api::routes::build_router(self.state.clone())
    }

    pub async fn issue_token(&self, name: &str, scopes: &[&str]) -> TokenSecretResponse {
        self.token_service
            .create_token(CreateTokenRequest {
                name: name.to_string(),
                description: None,
                expires_at: None,
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                created_by: Some("tests".into()),
                user_id: None,
                user_email: None,
            })
            .await
            .expect("create token")
    }
}

pub async fn setup_test_app() -> TestApp {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    // Use the same migration system as production instead of manual table creation
    storage::run_migrations(&pool).await.expect("run migrations for tests");

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));

    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
    let token_service = TokenService::with_sqlx(pool.clone(), audit_repo);

    TestApp { state, pool, token_service }
}

pub async fn send_request(
    app: &TestApp,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }

    let request = if let Some(json) = body {
        let bytes = serde_json::to_vec(&json).expect("serialize body");
        builder
            .header("content-type", "application/json")
            .body(Body::from(bytes))
            .expect("build request")
    } else {
        builder.body(Body::empty()).expect("build request")
    };

    app.router().oneshot(request).await.expect("request")
}

pub async fn read_json<T: DeserializeOwned>(response: Response<Body>) -> T {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("read body");
    serde_json::from_slice(&bytes).expect("parse json")
}

pub async fn create_team(app: &TestApp, admin_token: &str, team_name: &str) {
    use axum::http::StatusCode;
    use serde_json::json;

    let response = send_request(
        app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(admin_token),
        Some(json!({
            "name": team_name,
            "displayName": format!("{} Team", team_name),
            "description": format!("Test team: {}", team_name)
        })),
    )
    .await;

    if response.status() != StatusCode::CREATED && response.status() != StatusCode::CONFLICT {
        panic!("Failed to create team {}: status {}", team_name, response.status());
    }
}
