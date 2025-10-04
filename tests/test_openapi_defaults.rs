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

    // Core config tables used by the default seeding logic.
    sqlx::query(
        r#"
        CREATE TABLE clusters (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            service_name TEXT NOT NULL,
            configuration TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
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
        CREATE TABLE routes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path_prefix TEXT NOT NULL,
            cluster_name TEXT NOT NULL,
            configuration TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
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
        CREATE TABLE listeners (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            address TEXT NOT NULL,
            port INTEGER,
            protocol TEXT,
            configuration TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
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
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
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
async fn ensure_default_gateway_resources_seeds_bootstrap_token() {
    let pool = setup_pool().await;
    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));

    ensure_default_gateway_resources(&state).await.expect("default resources");

    let token_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(token_count, 1);

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.seeded' AND resource_type = 'auth.token'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}
