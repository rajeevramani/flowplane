use chrono::{Duration, Utc};
use flowplane::auth::cleanup_service::CleanupService;
use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus};
use flowplane::domain::TokenId;
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;

async fn setup_pool() -> DbPool {
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
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

            is_setup_token BOOLEAN NOT NULL DEFAULT FALSE,
            max_usage_count INTEGER,
            usage_count INTEGER NOT NULL DEFAULT 0,
            failed_attempts INTEGER NOT NULL DEFAULT 0,
            locked_until DATETIME,
            csrf_token TEXT,
            user_id TEXT,
            user_email TEXT
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

#[tokio::test]
async fn run_once_marks_expired_tokens() {
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
    let cleanup = CleanupService::new(repo.clone(), audit_repo.clone());

    let token_id = TokenId::from_string(uuid::Uuid::new_v4().to_string());
    let token = NewPersonalAccessToken {
        id: token_id.clone(),
        name: "cleanup".into(),
        description: None,
        hashed_secret: "hash".into(),
        status: TokenStatus::Active,
        expires_at: Some(Utc::now() - Duration::hours(1)),
        created_by: Some("tests".into()),
        scopes: vec!["clusters:read".into()],
        is_setup_token: false,
        max_usage_count: None,
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
        user_id: None,
        user_email: None,
    };
    repo.create_token(token).await.unwrap();

    cleanup.run_once().await.unwrap();

    let updated = repo.get_token(&token_id).await.unwrap();
    assert_eq!(updated.status, TokenStatus::Expired);

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.expired' AND resource_id = $1",
    )
    .bind(token_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}
