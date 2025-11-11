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
    storage::{repository::AuditLogRepository, DbPool},
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

    initialize_schema(&pool).await;

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));

    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
    let token_service = TokenService::with_sqlx(pool.clone(), audit_repo);

    TestApp { state, pool, token_service }
}

async fn initialize_schema(pool: &DbPool) {
    sqlx::query(
        r#"
        CREATE TABLE personal_access_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL,
            expires_at DATETIME,
            last_used_at DATETIME,
            created_by TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            is_setup_token BOOLEAN NOT NULL DEFAULT FALSE,
            max_usage_count INTEGER,
            usage_count INTEGER NOT NULL DEFAULT 0,
            failed_attempts INTEGER NOT NULL DEFAULT 0,
            locked_until DATETIME,
            csrf_token TEXT
        );
        "#,
    )
    .execute(pool)
    .await
    .expect("create personal_access_tokens table");

    sqlx::query(
        r#"
        CREATE TABLE token_scopes (
            id TEXT PRIMARY KEY,
            token_id TEXT NOT NULL,
            scope TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (token_id) REFERENCES personal_access_tokens(id) ON DELETE CASCADE
        );
        "#,
    )
    .execute(pool)
    .await
    .expect("create token_scopes table");

    sqlx::query(
        r#"
        CREATE TABLE audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            resource_type TEXT NOT NULL,
            resource_id TEXT,
            resource_name TEXT,
            action TEXT NOT NULL,
            old_configuration TEXT,
            new_configuration TEXT,
            user_id TEXT,
            client_ip TEXT,
            user_agent TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .execute(pool)
    .await
    .expect("create audit_log table");
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
