use flowplane::auth::auth_service::AuthService;
use flowplane::auth::models::AuthError;
use flowplane::auth::token_service::TokenService;
use flowplane::auth::validation::CreateTokenRequest;
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;

async fn setup_pool() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("in-memory sqlite");

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
            locked_until DATETIME
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

async fn setup_services() -> (TokenService, AuthService) {
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool));

    let token_service = TokenService::new(repo.clone(), audit.clone());
    let auth_service = AuthService::new(repo, audit);

    (token_service, auth_service)
}

fn sample_request() -> CreateTokenRequest {
    CreateTokenRequest {
        name: "auth".into(),
        description: None,
        expires_at: None,
        scopes: vec!["clusters:read".into()],
        created_by: Some("tests".into()),
        user_id: None,
        user_email: None,
    }
}

#[tokio::test]
async fn authenticate_valid_token() {
    let (token_service, auth_service) = setup_services().await;
    let secret = token_service.create_token(sample_request()).await.unwrap();

    let auth_header = format!("Bearer {}", secret.token);
    let context = auth_service.authenticate(&auth_header).await.unwrap();
    assert!(context.has_scope("clusters:read"));
}

#[tokio::test]
async fn authenticate_rejects_invalid_secret() {
    let (token_service, auth_service) = setup_services().await;
    let secret = token_service.create_token(sample_request()).await.unwrap();

    let bad_header = format!("Bearer fp_pat_{}.WRONG", secret.id);
    let err = auth_service.authenticate(&bad_header).await.unwrap_err();
    assert!(matches!(err, AuthError::TokenNotFound));
}

#[tokio::test]
async fn authenticate_requires_prefix() {
    let (_, auth_service) = setup_services().await;
    let err = auth_service.authenticate("not-a-token").await.unwrap_err();
    assert!(matches!(err, AuthError::MalformedBearer | AuthError::MissingBearer));
}
