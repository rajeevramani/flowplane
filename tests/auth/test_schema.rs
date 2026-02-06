// NOTE: This file requires PostgreSQL (via Testcontainers)
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]
#![allow(dead_code)]
//! Shared test database setup for auth-related tests.
//!
//! Provides PostgreSQL test databases via Testcontainers with all migrations applied.
//! The migration system creates all required auth tables (personal_access_tokens,
//! token_scopes, audit_log, etc.).

#[allow(clippy::duplicate_mod)]
#[path = "../common/mod.rs"]
mod common;
pub use common::test_db::TestDatabase;
use flowplane::storage::DbPool;

/// Create a test database with all migrations applied.
///
/// Returns a TestDatabase that MUST be kept alive for the duration of the test.
/// The PostgreSQL container is stopped when the TestDatabase is dropped.
pub async fn create_test_pool() -> TestDatabase {
    TestDatabase::new("auth_test").await
}

/// Create a test database with all migrations applied.
///
/// The `_max_connections` parameter is accepted for API compatibility but
/// the pool size is configured by TestDatabase (5 connections by default).
pub async fn create_test_pool_with_connections(_max_connections: u32) -> TestDatabase {
    TestDatabase::new("auth_test").await
}

/// Create a test database with all migrations applied (minimal variant).
///
/// With PostgreSQL migrations, there is no distinction between full and minimal
/// schemas - all tables are always created by migrations.
pub async fn create_test_pool_minimal() -> TestDatabase {
    TestDatabase::new("auth_test_minimal").await
}

/// Create a test database with all migrations applied (minimal variant).
pub async fn create_test_pool_minimal_with_connections(_max_connections: u32) -> TestDatabase {
    TestDatabase::new("auth_test_minimal").await
}

/// No-op: PostgreSQL migrations handle all schema creation.
///
/// Retained for API compatibility with tests that call setup_auth_schema after
/// creating a pool. Since TestDatabase already runs all migrations, this is a no-op.
pub async fn setup_auth_schema(_pool: &DbPool) -> Result<(), sqlx::Error> {
    Ok(())
}

/// No-op: PostgreSQL migrations handle all schema creation.
pub async fn setup_auth_schema_minimal(_pool: &DbPool) -> Result<(), sqlx::Error> {
    Ok(())
}
