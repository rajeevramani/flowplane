use chrono::Utc;
use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus, UpdatePersonalAccessToken};
use flowplane::domain::TokenId;
use flowplane::storage::repository::{SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

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
            ,
            is_setup_token BOOLEAN NOT NULL DEFAULT FALSE,
            max_usage_count INTEGER,
            usage_count INTEGER NOT NULL DEFAULT 0
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

    pool
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
    }
}

#[tokio::test]
async fn create_and_get_token_round_trip() {
    let pool = setup_pool().await;
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
    let pool = setup_pool().await;
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
    let pool = setup_pool().await;
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
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());

    for _ in 0..3 {
        repo.create_token(sample_token(&Uuid::new_v4().to_string())).await.unwrap();
    }

    let tokens = repo.list_tokens(10, 0).await.unwrap();
    assert_eq!(tokens.len(), 3);

    let count = repo.count_tokens().await.unwrap();
    assert_eq!(count, 3);

    let active = repo.count_active_tokens().await.unwrap();
    assert_eq!(active, 3);
}
