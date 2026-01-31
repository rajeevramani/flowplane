use chrono::{Duration, Utc};
use flowplane::auth::cleanup_service::CleanupService;
use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus};
use flowplane::domain::TokenId;
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use std::sync::Arc;

#[allow(clippy::duplicate_mod)]
#[path = "../test_schema.rs"]
mod test_schema;
use test_schema::create_test_pool;

#[tokio::test]
async fn run_once_marks_expired_tokens() {
    let pool = create_test_pool().await;
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
