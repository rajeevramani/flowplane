use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus};
use flowplane::domain::TokenId;
use flowplane::storage::repository::{SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::time::Duration;
use tokio::runtime::Runtime;
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

    pool
}

async fn seed_tokens(repo: &SqlxTokenRepository, count: usize) {
    for i in 0..count {
        let token = NewPersonalAccessToken {
            id: TokenId::from_str_unchecked(&Uuid::new_v4().to_string()),
            name: format!("token-{}", i),
            description: Some(format!("Benchmark token {}", i)),
            hashed_secret: format!("hash-{}", i),
            status: TokenStatus::Active,
            expires_at: None,
            created_by: Some("benchmark".into()),
            scopes: vec![
                format!("scope:read:{}", i),
                format!("scope:write:{}", i),
                format!("scope:delete:{}", i),
            ],
        };
        repo.create_token(token).await.unwrap();
    }
}

fn bench_list_tokens(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("token_repository");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    // Benchmark with different token counts
    for count in [100, 500, 1000, 2000].iter() {
        let pool = rt.block_on(setup_pool());
        let repo = SqlxTokenRepository::new(pool);
        rt.block_on(seed_tokens(&repo, *count));

        group.bench_with_input(BenchmarkId::new("list_tokens", count), count, |b, &_count| {
            b.to_async(&rt).iter(|| async {
                let tokens = repo.list_tokens(black_box(100), black_box(0)).await.unwrap();
                black_box(tokens)
            });
        });
    }

    group.finish();
}

fn bench_list_tokens_pagination(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("token_repository_pagination");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    // Set up once with 1000 tokens
    let pool = rt.block_on(setup_pool());
    let repo = SqlxTokenRepository::new(pool);
    rt.block_on(seed_tokens(&repo, 1000));

    // Benchmark different page sizes
    for page_size in [10, 50, 100, 500].iter() {
        group.bench_with_input(
            BenchmarkId::new("list_tokens_page_size", page_size),
            page_size,
            |b, &size| {
                b.to_async(&rt).iter(|| async {
                    let tokens = repo.list_tokens(black_box(size), black_box(0)).await.unwrap();
                    black_box(tokens)
                });
            },
        );
    }

    group.finish();
}

fn bench_get_single_token(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("token_repository_get");
    group.measurement_time(Duration::from_secs(10));

    let pool = rt.block_on(setup_pool());
    let repo = SqlxTokenRepository::new(pool);
    rt.block_on(seed_tokens(&repo, 100));

    // Get the first token ID for benchmarking
    let tokens = rt.block_on(repo.list_tokens(1, 0)).unwrap();
    let token_id = &tokens[0].id;

    group.bench_function("get_token", |b| {
        b.to_async(&rt).iter(|| async {
            let token = repo.get_token(black_box(token_id)).await.unwrap();
            black_box(token)
        });
    });

    group.finish();
}

fn bench_count_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("token_repository_count");
    group.measurement_time(Duration::from_secs(10));

    let pool = rt.block_on(setup_pool());
    let repo = SqlxTokenRepository::new(pool);
    rt.block_on(seed_tokens(&repo, 1000));

    group.bench_function("count_tokens", |b| {
        b.to_async(&rt).iter(|| async {
            let count = repo.count_tokens().await.unwrap();
            black_box(count)
        });
    });

    group.bench_function("count_active_tokens", |b| {
        b.to_async(&rt).iter(|| async {
            let count = repo.count_active_tokens().await.unwrap();
            black_box(count)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_list_tokens,
    bench_list_tokens_pagination,
    bench_get_single_token,
    bench_count_operations
);
criterion_main!(benches);
