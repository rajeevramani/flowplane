// NOTE: This file requires PostgreSQL (via Testcontainers)
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

//! Integration tests for setup token migration
//!
//! Verifies that migration 20251108000001_add_setup_token_fields.sql properly adds:
//! - is_setup_token column
//! - max_usage_count column
//! - usage_count column
//! - Indexes for setup tokens

mod common;

use common::db_metadata::{get_column_names, get_index_names};
use common::test_db::TestDatabase;
use sqlx::Row;

#[tokio::test]
async fn test_setup_token_migration_adds_columns() {
    let test_db = TestDatabase::new("setup_token_cols").await;
    let pool = &test_db.pool;

    // Verify new columns exist by querying the table schema
    let column_names =
        get_column_names(pool, "personal_access_tokens").await.expect("Failed to fetch table info");

    // Verify is_setup_token column exists
    assert!(
        column_names.contains(&"is_setup_token".to_string()),
        "is_setup_token column should exist"
    );

    // Verify max_usage_count column exists
    assert!(
        column_names.contains(&"max_usage_count".to_string()),
        "max_usage_count column should exist"
    );

    // Verify usage_count column exists
    assert!(column_names.contains(&"usage_count".to_string()), "usage_count column should exist");
}

#[tokio::test]
async fn test_setup_token_migration_creates_indexes() {
    let test_db = TestDatabase::new("setup_token_idx").await;
    let pool = &test_db.pool;

    // Verify indexes exist
    let index_names =
        get_index_names(pool, "personal_access_tokens").await.expect("Failed to fetch indexes");

    // Verify setup token index exists
    assert!(
        index_names.contains(&"idx_personal_access_tokens_is_setup_token".to_string()),
        "idx_personal_access_tokens_is_setup_token index should exist"
    );

    // Verify composite setup token index exists
    assert!(
        index_names.contains(&"idx_personal_access_tokens_setup_active".to_string()),
        "idx_personal_access_tokens_setup_active index should exist"
    );
}

#[tokio::test]
async fn test_setup_token_migration_preserves_existing_data() {
    let test_db = TestDatabase::new("setup_token_data").await;
    let pool = &test_db.pool;

    // Insert a test token
    sqlx::query(
        r#"
        INSERT INTO personal_access_tokens (id, name, token_hash, status, created_at, updated_at)
        VALUES ('test-token-1', 'Test Token', 'hash123', 'active', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        "#,
    )
    .execute(pool)
    .await
    .expect("Failed to insert test token");

    // Query the token and verify default values for new columns
    let row = sqlx::query("SELECT is_setup_token, max_usage_count, usage_count FROM personal_access_tokens WHERE id = 'test-token-1'")
        .fetch_one(pool)
        .await
        .expect("Failed to fetch test token");

    // Verify is_setup_token defaults to FALSE
    let is_setup_token: bool = row.get("is_setup_token");
    assert!(!is_setup_token, "is_setup_token should default to FALSE");

    // Verify max_usage_count can be NULL
    let max_usage_count: Option<i64> = row.get("max_usage_count");
    assert!(max_usage_count.is_none(), "max_usage_count should default to NULL");

    // Verify usage_count defaults to 0
    let usage_count: i64 = row.get("usage_count");
    assert_eq!(usage_count, 0, "usage_count should default to 0");
}

#[tokio::test]
async fn test_setup_token_can_insert_with_new_fields() {
    let test_db = TestDatabase::new("setup_token_insert").await;
    let pool = &test_db.pool;

    // Insert a setup token with new fields
    sqlx::query(
        r#"
        INSERT INTO personal_access_tokens
        (id, name, token_hash, status, is_setup_token, max_usage_count, usage_count, created_at, updated_at)
        VALUES ('setup-token-1', 'Setup Token', 'hash456', 'active', TRUE, 5, 2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        "#
    )
    .execute(pool)
    .await
    .expect("Failed to insert setup token");

    // Query the token
    let row = sqlx::query("SELECT is_setup_token, max_usage_count, usage_count FROM personal_access_tokens WHERE id = 'setup-token-1'")
        .fetch_one(pool)
        .await
        .expect("Failed to fetch setup token");

    // Verify values
    let is_setup_token: bool = row.get("is_setup_token");
    assert!(is_setup_token, "is_setup_token should be TRUE");

    let max_usage_count: Option<i64> = row.get("max_usage_count");
    assert_eq!(max_usage_count, Some(5), "max_usage_count should be 5");

    let usage_count: i64 = row.get("usage_count");
    assert_eq!(usage_count, 2, "usage_count should be 2");
}

#[tokio::test]
async fn test_setup_token_usage_count_can_be_incremented() {
    let test_db = TestDatabase::new("setup_token_incr").await;
    let pool = &test_db.pool;

    // Insert a setup token
    sqlx::query(
        r#"
        INSERT INTO personal_access_tokens
        (id, name, token_hash, status, is_setup_token, max_usage_count, usage_count, created_at, updated_at)
        VALUES ('setup-token-2', 'Setup Token', 'hash789', 'active', TRUE, 5, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        "#
    )
    .execute(pool)
    .await
    .expect("Failed to insert setup token");

    // Increment usage count
    sqlx::query("UPDATE personal_access_tokens SET usage_count = usage_count + 1 WHERE id = 'setup-token-2'")
        .execute(pool)
        .await
        .expect("Failed to increment usage count");

    // Verify the count was incremented
    let row =
        sqlx::query("SELECT usage_count FROM personal_access_tokens WHERE id = 'setup-token-2'")
            .fetch_one(pool)
            .await
            .expect("Failed to fetch setup token");

    let usage_count: i64 = row.get("usage_count");
    assert_eq!(usage_count, 1, "usage_count should be incremented to 1");
}

#[tokio::test]
async fn test_setup_token_index_filters_correctly() {
    let test_db = TestDatabase::new("setup_token_filter").await;
    let pool = &test_db.pool;

    // Insert test data
    for i in 0..10 {
        let is_setup = i % 3 == 0; // 4 setup tokens (0, 3, 6, 9)
        sqlx::query(
            r#"
            INSERT INTO personal_access_tokens
            (id, name, token_hash, status, is_setup_token, created_at, updated_at)
            VALUES ($1, $2, $3, 'active', $4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(format!("token-{}", i))
        .bind(format!("Token {}", i))
        .bind(format!("hash-{}", i))
        .bind(is_setup)
        .execute(pool)
        .await
        .expect("Failed to insert test token");
    }

    // Query for setup tokens
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM personal_access_tokens WHERE is_setup_token = TRUE",
    )
    .fetch_one(pool)
    .await
    .expect("Failed to count setup tokens");

    assert_eq!(count, 4, "Should have 4 setup tokens");

    // Query for non-setup tokens
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM personal_access_tokens WHERE is_setup_token = FALSE",
    )
    .fetch_one(pool)
    .await
    .expect("Failed to count non-setup tokens");

    assert_eq!(count, 6, "Should have 6 non-setup tokens");
}
