#![allow(dead_code)]
//! Shared test schema setup for auth-related tables.
//!
//! Provides centralized schema creation functions to eliminate duplication
//! across test files and benchmarks. This module is the single source of
//! truth for auth test schemas.
//!
//! # Usage
//!
//! For tests that need full auth functionality (including audit log):
//! ```ignore
//! use crate::test_schema::setup_auth_schema;
//! setup_auth_schema(&pool).await.unwrap();
//! ```
//!
//! For unit tests and benchmarks that only need token storage:
//! ```ignore
//! use crate::test_schema::setup_auth_schema_minimal;
//! setup_auth_schema_minimal(&pool).await.unwrap();
//! ```

use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;

/// Setup complete auth schema including audit_log.
///
/// Creates:
/// - personal_access_tokens table
/// - token_scopes table
/// - audit_log table
///
/// Use this for integration tests that need full auth functionality.
pub async fn setup_auth_schema(pool: &DbPool) -> Result<(), sqlx::Error> {
    setup_auth_schema_minimal(pool).await?;
    create_audit_log_table(pool).await?;
    Ok(())
}

/// Setup minimal auth schema without audit_log.
///
/// Creates:
/// - personal_access_tokens table
/// - token_scopes table
///
/// Use this for unit tests and benchmarks that only need token storage.
pub async fn setup_auth_schema_minimal(pool: &DbPool) -> Result<(), sqlx::Error> {
    create_personal_access_tokens_table(pool).await?;
    create_token_scopes_table(pool).await?;
    Ok(())
}

async fn create_personal_access_tokens_table(pool: &DbPool) -> Result<(), sqlx::Error> {
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
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_token_scopes_table(pool: &DbPool) -> Result<(), sqlx::Error> {
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
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_audit_log_table(pool: &DbPool) -> Result<(), sqlx::Error> {
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
    .execute(pool)
    .await?;
    Ok(())
}

/// Create in-memory test pool with full auth schema.
///
/// Creates a SQLite in-memory database with:
/// - personal_access_tokens table
/// - token_scopes table
/// - audit_log table
/// - 5 max connections (standard for unit/integration tests)
///
/// # Example
/// ```ignore
/// let pool = create_test_pool().await;
/// let repo = SqlxTokenRepository::new(pool);
/// ```
pub async fn create_test_pool() -> DbPool {
    create_test_pool_with_connections(5).await
}

/// Create in-memory test pool with minimal auth schema (no audit_log).
///
/// Creates a SQLite in-memory database with:
/// - personal_access_tokens table
/// - token_scopes table
/// - 5 max connections (standard for unit/integration tests)
///
/// Use this for unit tests that only need token storage without audit logging.
pub async fn create_test_pool_minimal() -> DbPool {
    create_test_pool_minimal_with_connections(5).await
}

/// Create in-memory test pool with full auth schema and custom connection count.
///
/// Use this for performance tests or benchmarks that need higher concurrency.
/// Standard tests should use `create_test_pool()` instead.
///
/// # Arguments
/// * `max_connections` - Maximum number of concurrent connections (use 10+ for perf tests)
pub async fn create_test_pool_with_connections(max_connections: u32) -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create in-memory sqlite pool");

    setup_auth_schema(&pool).await.expect("setup auth schema");
    pool
}

/// Create in-memory test pool with minimal schema and custom connection count.
///
/// Use this for performance tests or benchmarks that need higher concurrency
/// but don't require audit logging.
///
/// # Arguments
/// * `max_connections` - Maximum number of concurrent connections (use 10+ for perf tests)
pub async fn create_test_pool_minimal_with_connections(max_connections: u32) -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create in-memory sqlite pool");

    setup_auth_schema_minimal(&pool).await.expect("setup auth schema");
    pool
}
