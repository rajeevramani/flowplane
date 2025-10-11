use chrono::Utc;
use flowplane::auth::models::TokenStatus;
use flowplane::auth::token_service::{TokenSecretResponse, TokenService};
use flowplane::auth::validation::{CreateTokenRequest, UpdateTokenRequest};
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use validator::Validate;

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

async fn setup_service() -> (TokenService, Arc<SqlxTokenRepository>, Arc<AuditLogRepository>, DbPool)
{
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::new(repo.clone(), audit.clone());
    (service, repo, audit, pool)
}

fn sample_create_request() -> CreateTokenRequest {
    CreateTokenRequest {
        name: "example".into(),
        description: Some("demo".into()),
        expires_at: None,
        scopes: vec!["clusters:read".into()],
        created_by: Some("unit".into()),
    }
}

#[tokio::test]
async fn create_token_returns_secret_and_persists() {
    let (service, repo, _, _) = setup_service().await;
    let request = sample_create_request();

    let TokenSecretResponse { id, token } = service.create_token(request.clone()).await.unwrap();
    assert!(token.starts_with("fp_pat_"));

    let stored = repo.get_token(&id).await.unwrap();
    assert_eq!(stored.name, request.name);
    assert!(stored.has_scope("clusters:read"));
}

#[tokio::test]
async fn create_token_without_expiry_defaults_to_30_days() {
    let (service, repo, _, _) = setup_service().await;
    let request = CreateTokenRequest {
        name: "no-expiry-test".into(),
        description: Some("Test default expiry".into()),
        expires_at: None, // Explicitly no expiry provided
        scopes: vec!["clusters:read".into()],
        created_by: Some("unit".into()),
    };

    let TokenSecretResponse { id, .. } = service.create_token(request).await.unwrap();
    let stored = repo.get_token(&id).await.unwrap();

    // Verify that expires_at was set to ~30 days from now
    assert!(stored.expires_at.is_some(), "Expected expires_at to be set with default value");
    let expires_at = stored.expires_at.unwrap();
    let now = Utc::now();
    let expected_expiry = now + chrono::Duration::days(30);

    // Allow 5 second tolerance for test execution time
    let diff = (expires_at - expected_expiry).num_seconds().abs();
    assert!(diff < 5, "Expected expiry to be ~30 days from now, but difference was {} seconds", diff);
}

#[tokio::test]
async fn update_and_revoke_token() {
    let (service, repo, _, _) = setup_service().await;
    let secret = service.create_token(sample_create_request()).await.unwrap();

    let update_payload = UpdateTokenRequest {
        name: Some("renamed".into()),
        description: Some("desc".into()),
        status: Some("active".into()),
        expires_at: Some(Some(Utc::now())),
        scopes: Some(vec!["routes:read".into()]),
    };
    update_payload.validate().unwrap();

    let updated = service.update_token(&secret.id, update_payload).await.unwrap();
    assert_eq!(updated.name, "renamed");
    assert!(updated.has_scope("routes:read"));

    let revoked = service.revoke_token(&secret.id).await.unwrap();
    assert_eq!(revoked.status, TokenStatus::Revoked);
    assert!(!revoked.has_scope("routes:read"));

    let stored = repo.get_token(&secret.id).await.unwrap();
    assert_eq!(stored.status, TokenStatus::Revoked);
    assert!(stored.scopes.is_empty());
}

#[tokio::test]
async fn rotate_generates_new_secret() {
    let (service, repo, _, _) = setup_service().await;
    let created = service.create_token(sample_create_request()).await.unwrap();

    let rotated = service.rotate_token(&created.id).await.unwrap();
    assert_ne!(created.token, rotated.token);

    let parts: Vec<&str> = rotated.token.split('.').collect();
    assert_eq!(parts.len(), 2);
    let secret_part = parts[1];

    let (_, hashed) = repo.find_active_for_auth(&created.id).await.unwrap().unwrap();
    assert!(service.verify_secret(&hashed, secret_part).unwrap());
}

#[tokio::test]
async fn ensure_bootstrap_token_creates_when_empty() {
    let (service, _, _, pool) = setup_service().await;
    let bootstrap_secret = "test-bootstrap-token-min-32-characters-long";
    let maybe_token = service.ensure_bootstrap_token(bootstrap_secret).await.unwrap();
    assert!(maybe_token.is_some());

    let token = maybe_token.unwrap();
    assert!(token.starts_with("fp_pat_"));
    assert!(token.contains(bootstrap_secret));

    // Subsequent call is a no-op.
    assert!(service.ensure_bootstrap_token(bootstrap_secret).await.unwrap().is_none());

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let seeded_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.bootstrap_seeded'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seeded_events, 1);
}
