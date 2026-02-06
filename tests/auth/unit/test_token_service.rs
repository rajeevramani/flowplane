// NOTE: This file requires PostgreSQL (via Testcontainers)
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

use chrono::Utc;
use flowplane::auth::models::TokenStatus;
use flowplane::auth::token_service::{TokenSecretResponse, TokenService};
use flowplane::auth::validation::{CreateTokenRequest, UpdateTokenRequest};
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use std::sync::Arc;
use validator::Validate;

#[allow(clippy::duplicate_mod)]
#[path = "../test_schema.rs"]
mod test_schema;
use test_schema::{create_test_pool, TestDatabase};

async fn setup_service(
) -> (TestDatabase, TokenService, Arc<SqlxTokenRepository>, Arc<AuditLogRepository>, DbPool) {
    let test_db = create_test_pool().await;
    let pool = test_db.pool.clone();
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::new(repo.clone(), audit.clone());
    (test_db, service, repo, audit, pool)
}

fn sample_create_request() -> CreateTokenRequest {
    CreateTokenRequest {
        name: "example".into(),
        description: Some("demo".into()),
        expires_at: None,
        scopes: vec!["clusters:read".into()],
        created_by: Some("unit".into()),
        user_id: None,
        user_email: None,
    }
}

#[tokio::test]
async fn create_token_returns_secret_and_persists() {
    let (_db, service, repo, _, _) = setup_service().await;
    let request = sample_create_request();

    let TokenSecretResponse { id, token } =
        service.create_token(request.clone(), None).await.unwrap();
    assert!(token.starts_with("fp_pat_"));

    let stored =
        repo.get_token(&flowplane::domain::TokenId::from_str_unchecked(&id)).await.unwrap();
    assert_eq!(stored.name, request.name);
    assert!(stored.has_scope("clusters:read"));
}

#[tokio::test]
async fn create_token_without_expiry_defaults_to_30_days() {
    let (_db, service, repo, _, _) = setup_service().await;
    let request = CreateTokenRequest {
        name: "no-expiry-test".into(),
        description: Some("Test default expiry".into()),
        expires_at: None, // Explicitly no expiry provided
        scopes: vec!["clusters:read".into()],
        created_by: Some("unit".into()),
        user_id: None,
        user_email: None,
    };

    let TokenSecretResponse { id, .. } = service.create_token(request, None).await.unwrap();
    let stored =
        repo.get_token(&flowplane::domain::TokenId::from_str_unchecked(&id)).await.unwrap();

    // Verify that expires_at was set to ~30 days from now
    assert!(stored.expires_at.is_some(), "Expected expires_at to be set with default value");
    let expires_at = stored.expires_at.unwrap();
    let now = Utc::now();
    let expected_expiry = now + chrono::Duration::days(30);

    // Allow 5 second tolerance for test execution time
    let diff = (expires_at - expected_expiry).num_seconds().abs();
    assert!(
        diff < 5,
        "Expected expiry to be ~30 days from now, but difference was {} seconds",
        diff
    );
}

#[tokio::test]
async fn update_and_revoke_token() {
    let (_db, service, repo, _, _) = setup_service().await;
    let secret = service.create_token(sample_create_request(), None).await.unwrap();

    let update_payload = UpdateTokenRequest {
        name: Some("renamed".into()),
        description: Some("desc".into()),
        status: Some("active".into()),
        expires_at: Some(Some(Utc::now())),
        scopes: Some(vec!["routes:read".into()]),
    };
    update_payload.validate().unwrap();

    let updated = service.update_token(&secret.id, update_payload, None).await.unwrap();
    assert_eq!(updated.name, "renamed");
    assert!(updated.has_scope("routes:read"));

    let revoked = service.revoke_token(&secret.id, None).await.unwrap();
    assert_eq!(revoked.status, TokenStatus::Revoked);
    assert!(!revoked.has_scope("routes:read"));

    let stored =
        repo.get_token(&flowplane::domain::TokenId::from_str_unchecked(&secret.id)).await.unwrap();
    assert_eq!(stored.status, TokenStatus::Revoked);
    assert!(stored.scopes.is_empty());
}

#[tokio::test]
async fn rotate_generates_new_secret() {
    let (_db, service, repo, _, _) = setup_service().await;
    let created = service.create_token(sample_create_request(), None).await.unwrap();

    let rotated = service.rotate_token(&created.id, None).await.unwrap();
    assert_ne!(created.token, rotated.token);

    let parts: Vec<&str> = rotated.token.split('.').collect();
    assert_eq!(parts.len(), 2);
    let secret_part = parts[1];

    let (_, hashed) = repo
        .find_active_for_auth(&flowplane::domain::TokenId::from_str_unchecked(&created.id))
        .await
        .unwrap()
        .unwrap();
    assert!(service.verify_secret(&hashed, secret_part).unwrap());
}

#[tokio::test]
async fn ensure_bootstrap_token_creates_when_empty() {
    let (_db, service, _, _, pool) = setup_service().await;
    let bootstrap_secret = "test-bootstrap-token-min-32-characters-long";
    // Pass None for secrets client - dev mode without Vault
    let maybe_token = service
        .ensure_bootstrap_token(bootstrap_secret, None::<&flowplane::secrets::EnvVarSecretsClient>)
        .await
        .unwrap();
    assert!(maybe_token.is_some());

    let token = maybe_token.unwrap();
    assert!(token.starts_with("fp_pat_"));
    assert!(token.contains(bootstrap_secret));

    // Subsequent call is a no-op.
    assert!(service
        .ensure_bootstrap_token(bootstrap_secret, None::<&flowplane::secrets::EnvVarSecretsClient>)
        .await
        .unwrap()
        .is_none());

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
