// NOTE: This file requires PostgreSQL - disabled until Phase 4 of PostgreSQL migration
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

use chrono::Utc;
use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus, UpdatePersonalAccessToken};
use flowplane::domain::TokenId;
use flowplane::storage::repository::{SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use uuid::Uuid;

#[allow(clippy::duplicate_mod)]
#[path = "../../common/mod.rs"]
mod common;
use common::test_db::TestDatabase;

async fn setup_test_db() -> (TestDatabase, DbPool) {
    let test_db = TestDatabase::new("auth_repository").await;
    let pool = test_db.pool.clone();
    (test_db, pool)
}

fn sample_token(id: &str) -> NewPersonalAccessToken {
    NewPersonalAccessToken {
        id: TokenId::from_str_unchecked(id),
        name: "sample".into(),
        description: Some("demo token".into()),
        hashed_secret: "hashed".into(),
        status: TokenStatus::Active,
        expires_at: None,
        created_by: Some("admin".into()),
        scopes: vec!["clusters:read".into(), "clusters:write".into()],
        is_setup_token: false,
        max_usage_count: None,
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
        user_id: None,
        user_email: None,
    }
}

#[tokio::test]
async fn create_and_get_token_round_trip() {
    let (_test_db, pool) = setup_test_db().await;
    let repo = SqlxTokenRepository::new(pool.clone());

    let token = sample_token(&Uuid::new_v4().to_string());
    let created = repo.create_token(token.clone()).await.unwrap();
    assert_eq!(created.name, token.name);
    assert!(created.has_scope("clusters:read"));

    let active_count = repo.count_active_tokens().await.unwrap();
    assert_eq!(active_count, 1);

    let fetched = repo.get_token(&created.id).await.unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.scopes.len(), 2);
}

#[tokio::test]
async fn update_metadata_replaces_scopes() {
    let (_test_db, pool) = setup_test_db().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    let token_id = TokenId::from_string(Uuid::new_v4().to_string());
    repo.create_token(sample_token(token_id.as_str())).await.unwrap();

    let update = UpdatePersonalAccessToken {
        name: Some("updated".into()),
        description: None,
        status: Some(TokenStatus::Revoked),
        expires_at: Some(Some(Utc::now())),
        scopes: Some(vec!["routes:read".into()]),
    };

    let updated = repo.update_metadata(&token_id, update).await.unwrap();
    assert_eq!(updated.name, "updated");
    assert_eq!(updated.status, TokenStatus::Revoked);
    assert!(updated.has_scope("routes:read"));
    assert!(!updated.has_scope("clusters:read"));

    let active = repo.count_active_tokens().await.unwrap();
    assert_eq!(active, 0);
}

#[tokio::test]
async fn rotate_and_auth_lookup() {
    let (_test_db, pool) = setup_test_db().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    let token_id = TokenId::from_string(Uuid::new_v4().to_string());
    repo.create_token(sample_token(token_id.as_str())).await.unwrap();

    repo.rotate_secret(&token_id, "new-hash".into()).await.unwrap();
    repo.update_last_used(&token_id, Utc::now()).await.unwrap();

    let (token, hash) = repo.find_active_for_auth(&token_id).await.unwrap().expect("token present");
    assert_eq!(hash, "new-hash");
    assert_eq!(token.status, TokenStatus::Active);
}

#[tokio::test]
async fn list_and_count_tokens() {
    let (_test_db, pool) = setup_test_db().await;
    let repo = SqlxTokenRepository::new(pool.clone());

    for _ in 0..3 {
        repo.create_token(sample_token(&Uuid::new_v4().to_string())).await.unwrap();
    }

    let tokens = repo.list_tokens(10, 0, None).await.unwrap();
    assert_eq!(tokens.len(), 3);

    let count = repo.count_tokens().await.unwrap();
    assert_eq!(count, 3);

    let active = repo.count_active_tokens().await.unwrap();
    assert_eq!(active, 3);
}
