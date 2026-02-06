// NOTE: This file requires PostgreSQL (via Testcontainers)
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

use flowplane::auth::auth_service::AuthService;
use flowplane::auth::models::AuthError;
use flowplane::auth::token_service::TokenService;
use flowplane::auth::validation::CreateTokenRequest;
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository};
use std::sync::Arc;

#[allow(clippy::duplicate_mod)]
#[path = "../test_schema.rs"]
mod test_schema;
use test_schema::{create_test_pool, TestDatabase};

async fn setup_services() -> (TestDatabase, TokenService, AuthService) {
    let test_db = create_test_pool().await;
    let pool = test_db.pool.clone();
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool));

    let token_service = TokenService::new(repo.clone(), audit.clone());
    let auth_service = AuthService::new(repo, audit);

    (test_db, token_service, auth_service)
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
    let (_db, token_service, auth_service) = setup_services().await;
    let secret = token_service.create_token(sample_request(), None).await.unwrap();

    let auth_header = format!("Bearer {}", secret.token);
    let context = auth_service.authenticate(&auth_header, None, None).await.unwrap();
    assert!(context.has_scope("clusters:read"));
}

#[tokio::test]
async fn authenticate_rejects_invalid_secret() {
    let (_db, token_service, auth_service) = setup_services().await;
    let secret = token_service.create_token(sample_request(), None).await.unwrap();

    let bad_header = format!("Bearer fp_pat_{}.WRONG", secret.id);
    let err = auth_service.authenticate(&bad_header, None, None).await.unwrap_err();
    assert!(matches!(err, AuthError::TokenNotFound));
}

#[tokio::test]
async fn authenticate_requires_prefix() {
    let (_db, _, auth_service) = setup_services().await;
    let err = auth_service.authenticate("not-a-token", None, None).await.unwrap_err();
    assert!(matches!(err, AuthError::MalformedBearer | AuthError::MissingBearer));
}
