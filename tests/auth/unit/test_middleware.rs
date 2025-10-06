use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::get,
    Router,
};
use flowplane::auth::{
    auth_service::AuthService,
    middleware::{authenticate, ensure_scopes, ScopeState},
    token_service::TokenService,
};
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use tower::ServiceExt;

async fn setup_pool() -> flowplane::storage::DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

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
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

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
    .execute(&pool)
    .await
    .unwrap();

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
    .execute(&pool)
    .await
    .unwrap();

    pool
}

async fn setup_services() -> (TokenService, Arc<AuthService>) {
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool));

    let token_service = TokenService::new(repo.clone(), audit.clone());
    let auth_service = Arc::new(AuthService::new(repo, audit));

    (token_service, auth_service)
}

fn secured_router(auth_service: Arc<AuthService>, scopes: Vec<&str>) -> Router {
    let scope_layer = {
        let required: ScopeState = Arc::new(scopes.into_iter().map(|s| s.to_string()).collect());
        middleware::from_fn_with_state(required, ensure_scopes)
    };

    Router::new()
        .route("/secure", get(|| async { StatusCode::OK }))
        .route_layer(scope_layer)
        .layer(middleware::from_fn_with_state(auth_service, authenticate))
        .with_state(())
}

#[tokio::test]
async fn missing_bearer_returns_unauthorized() {
    let (_, auth_service) = setup_services().await;
    let app = secured_router(auth_service, vec!["clusters:read"]);

    let response =
        app.oneshot(Request::builder().uri("/secure").body(Body::empty()).unwrap()).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn insufficient_scope_returns_forbidden() {
    let (token_service, auth_service) = setup_services().await;
    let token = token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "scope-test".into(),
            description: None,
            expires_at: None,
            scopes: vec!["routes:read".into()],
            created_by: Some("tests".into()),
        })
        .await
        .unwrap();

    let app = secured_router(auth_service, vec!["clusters:write"]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/secure")
                .header("Authorization", format!("Bearer {}", token.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn valid_token_allows_request() {
    let (token_service, auth_service) = setup_services().await;
    let token = token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "valid-token".into(),
            description: None,
            expires_at: None,
            scopes: vec!["clusters:read".into()],
            created_by: Some("tests".into()),
        })
        .await
        .unwrap();

    let app = secured_router(auth_service, vec!["clusters:read"]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/secure")
                .header("Authorization", format!("Bearer {}", token.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
