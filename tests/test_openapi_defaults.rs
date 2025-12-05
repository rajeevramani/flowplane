use std::sync::Arc;

use flowplane::config::SimpleXdsConfig;
use flowplane::openapi::defaults::ensure_default_gateway_resources;
use flowplane::storage::DbPool;
use flowplane::xds::XdsState;
use sqlx::sqlite::SqlitePoolOptions;

async fn setup_pool() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    // Use actual migrations instead of manual schema to avoid drift
    flowplane::storage::run_migrations(&pool).await.expect("run migrations");

    pool
}

#[tokio::test]
async fn ensure_default_gateway_resources_creates_default_resources() {
    let pool = setup_pool().await;
    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));

    ensure_default_gateway_resources(&state).await.expect("default resources");

    // Verify that default gateway resources were created (cluster, route, listener)
    // Note: Bootstrap token creation is now handled in src/startup.rs, not here

    let cluster_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM clusters WHERE name = 'default-gateway-cluster'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cluster_count, 1, "Expected default gateway cluster to be created");

    let route_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM route_configs WHERE name = 'default-gateway-routes'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(route_count, 1, "Expected default gateway routes to be created");

    let listener_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listeners WHERE name = 'default-gateway-listener'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(listener_count, 1, "Expected default gateway listener to be created");

    // Verify NO bootstrap tokens were created (that's now handled in startup module)
    let token_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(token_count, 0, "ensure_default_gateway_resources should NOT create tokens");
}
