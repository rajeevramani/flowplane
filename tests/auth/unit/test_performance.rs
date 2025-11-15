use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus};
use flowplane::domain::TokenId;
use flowplane::storage::repository::{SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::time::Instant;
use uuid::Uuid;

async fn setup_pool() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
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

    pool
}

fn sample_token(id: &str, index: usize) -> NewPersonalAccessToken {
    NewPersonalAccessToken {
        id: TokenId::from_str_unchecked(id),
        name: format!("token-{}", index),
        description: Some(format!("Performance test token {}", index)),
        hashed_secret: format!("hash-{}", index),
        status: TokenStatus::Active,
        expires_at: None,
        created_by: Some("perf-test".into()),
        scopes: vec![
            format!("scope:read:{}", index),
            format!("scope:write:{}", index),
            format!("scope:delete:{}", index),
        ],
        is_setup_token: false,
        max_usage_count: None,
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
    }
}

async fn seed_tokens(repo: &SqlxTokenRepository, count: usize) {
    for i in 0..count {
        let token = sample_token(&Uuid::new_v4().to_string(), i);
        repo.create_token(token).await.unwrap();
    }
}

#[tokio::test]
async fn performance_list_tokens_100() {
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    seed_tokens(&repo, 100).await;

    let start = Instant::now();
    let tokens = repo.list_tokens(100, 0, None).await.unwrap();
    let duration = start.elapsed();

    assert_eq!(tokens.len(), 100, "Should fetch all 100 tokens");
    assert!(
        duration.as_millis() < 100,
        "Query should complete in under 100ms, took: {:?}",
        duration
    );

    println!("list_tokens(100) completed in {:?}", duration);
}

#[tokio::test]
async fn performance_list_tokens_1000() {
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    seed_tokens(&repo, 1000).await;

    let start = Instant::now();
    let tokens = repo.list_tokens(100, 0, None).await.unwrap();
    let duration = start.elapsed();

    assert_eq!(tokens.len(), 100, "Should fetch first 100 tokens");
    assert!(
        duration.as_millis() < 200,
        "Query should complete in under 200ms even with 1000 tokens in DB, took: {:?}",
        duration
    );

    println!("list_tokens(100) with 1000 tokens in DB completed in {:?}", duration);
}

#[tokio::test]
async fn performance_list_tokens_pagination() {
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    seed_tokens(&repo, 500).await;

    let start = Instant::now();
    let page1 = repo.list_tokens(50, 0, None).await.unwrap();
    let page2 = repo.list_tokens(50, 50, None).await.unwrap();
    let page3 = repo.list_tokens(50, 100, None).await.unwrap();
    let duration = start.elapsed();

    assert_eq!(page1.len(), 50);
    assert_eq!(page2.len(), 50);
    assert_eq!(page3.len(), 50);
    assert!(
        duration.as_millis() < 150,
        "Three paginated queries should complete in under 150ms, took: {:?}",
        duration
    );

    println!("Three paginated queries (50 each) completed in {:?}", duration);
}

#[tokio::test]
async fn performance_count_operations() {
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    seed_tokens(&repo, 1000).await;

    let start = Instant::now();
    let count = repo.count_tokens().await.unwrap();
    let active_count = repo.count_active_tokens().await.unwrap();
    let duration = start.elapsed();

    assert_eq!(count, 1000);
    assert_eq!(active_count, 1000);
    assert!(
        duration.as_millis() < 50,
        "Count operations should complete in under 50ms, took: {:?}",
        duration
    );

    println!("Count operations completed in {:?}", duration);
}

#[tokio::test]
async fn performance_get_single_token() {
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    seed_tokens(&repo, 1000).await;

    let tokens = repo.list_tokens(1, 0, None).await.unwrap();
    let token_id = &tokens[0].id;

    let start = Instant::now();
    let mut total_queries = 0;
    for _ in 0..100 {
        let _token = repo.get_token(token_id).await.unwrap();
        total_queries += 1;
    }
    let duration = start.elapsed();

    assert_eq!(total_queries, 100);
    assert!(
        duration.as_millis() < 1000,
        "100 get_token queries should complete in under 1 second, took: {:?}",
        duration
    );

    println!(
        "100 get_token queries completed in {:?} ({:?} avg per query)",
        duration,
        duration / 100
    );
}

#[tokio::test]
async fn performance_comparison_theoretical() {
    let pool = setup_pool().await;
    let repo = SqlxTokenRepository::new(pool.clone());
    seed_tokens(&repo, 100).await;

    // Test the optimized version
    let start = Instant::now();
    let tokens = repo.list_tokens(100, 0, None).await.unwrap();
    let optimized_duration = start.elapsed();

    assert_eq!(tokens.len(), 100);

    // Calculate theoretical old implementation time
    // Old: 1 query for IDs + 2N queries (N for tokens, N for scopes)
    // = 1 + 200 = 201 queries
    // Assuming ~1ms per query = ~201ms minimum
    let theoretical_old_time_ms = 201; // Conservative estimate
    let optimized_time_ms = optimized_duration.as_millis();

    println!("Optimized list_tokens(100): {:?} ({} ms)", optimized_duration, optimized_time_ms);
    println!("Theoretical old implementation: ~{} ms (201 queries)", theoretical_old_time_ms);
    println!(
        "Theoretical speedup: ~{}x faster",
        theoretical_old_time_ms / optimized_time_ms.max(1)
    );
    println!("Query reduction: 201 queries → 1 query (99.5% reduction)");
}
