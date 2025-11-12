//! Integration tests for user and team membership migrations
//!
//! Verifies that the following migrations work correctly:
//! - 20251112000001_create_users_table.sql
//! - 20251112000002_create_user_team_memberships_table.sql
//! - 20251112000003_add_user_columns_to_tokens.sql

use flowplane::config::DatabaseConfig;
use flowplane::storage::create_pool;
use sqlx::Row;

async fn create_test_pool() -> sqlx::Pool<sqlx::Sqlite> {
    let config = DatabaseConfig {
        url: "sqlite://:memory:".to_string(),
        auto_migrate: true,
        ..Default::default()
    };
    create_pool(&config).await.unwrap()
}

// Tests for users table migration (20251112000001)

#[tokio::test]
async fn test_users_table_created() {
    let pool = create_test_pool().await;

    // Verify users table exists by querying its schema
    let schema = sqlx::query("PRAGMA table_info(users)")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch users table info");

    // Extract column names
    let column_names: Vec<String> = schema.iter().map(|row| row.get("name")).collect();

    // Verify all required columns exist
    assert!(column_names.contains(&"id".to_string()), "id column should exist");
    assert!(column_names.contains(&"email".to_string()), "email column should exist");
    assert!(
        column_names.contains(&"password_hash".to_string()),
        "password_hash column should exist"
    );
    assert!(column_names.contains(&"name".to_string()), "name column should exist");
    assert!(column_names.contains(&"status".to_string()), "status column should exist");
    assert!(column_names.contains(&"is_admin".to_string()), "is_admin column should exist");
    assert!(column_names.contains(&"created_at".to_string()), "created_at column should exist");
    assert!(column_names.contains(&"updated_at".to_string()), "updated_at column should exist");
}

#[tokio::test]
async fn test_users_table_indexes() {
    let pool = create_test_pool().await;

    // Verify indexes exist
    let indexes =
        sqlx::query("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='users'")
            .fetch_all(&pool)
            .await
            .expect("Failed to fetch indexes");

    let index_names: Vec<String> = indexes.iter().map(|row| row.get("name")).collect();

    // Verify required indexes exist
    assert!(
        index_names.contains(&"idx_users_email".to_string()),
        "idx_users_email index should exist"
    );
    assert!(
        index_names.contains(&"idx_users_status".to_string()),
        "idx_users_status index should exist"
    );
    assert!(
        index_names.contains(&"idx_users_is_admin".to_string()),
        "idx_users_is_admin index should exist"
    );
    assert!(
        index_names.contains(&"idx_users_status_admin".to_string()),
        "idx_users_status_admin index should exist"
    );
}

#[tokio::test]
async fn test_users_email_unique_constraint() {
    let pool = create_test_pool().await;

    // Insert a user
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name, status, is_admin, created_at, updated_at)
        VALUES ('user-1', 'test@example.com', 'hash123', 'Test User', 'active', FALSE, datetime('now'), datetime('now'))
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert first user");

    // Try to insert another user with the same email
    let result = sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name, status, is_admin, created_at, updated_at)
        VALUES ('user-2', 'test@example.com', 'hash456', 'Test User 2', 'active', FALSE, datetime('now'), datetime('now'))
        "#,
    )
    .execute(&pool)
    .await;

    // Should fail due to unique constraint on email
    assert!(result.is_err(), "Duplicate email should be rejected");
}

#[tokio::test]
async fn test_users_default_values() {
    let pool = create_test_pool().await;

    // Insert a user with minimal fields
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name)
        VALUES ('user-1', 'test@example.com', 'hash123', 'Test User')
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert user");

    // Query the user
    let row = sqlx::query("SELECT status, is_admin FROM users WHERE id = 'user-1'")
        .fetch_one(&pool)
        .await
        .expect("Failed to fetch user");

    // Verify default values
    let status: String = row.get("status");
    assert_eq!(status, "active", "status should default to 'active'");

    let is_admin: bool = row.get("is_admin");
    assert!(!is_admin, "is_admin should default to FALSE");
}

// Tests for user_team_memberships table migration (20251112000002)

#[tokio::test]
async fn test_user_team_memberships_table_created() {
    let pool = create_test_pool().await;

    // Verify user_team_memberships table exists
    let schema = sqlx::query("PRAGMA table_info(user_team_memberships)")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch user_team_memberships table info");

    // Extract column names
    let column_names: Vec<String> = schema.iter().map(|row| row.get("name")).collect();

    // Verify all required columns exist
    assert!(column_names.contains(&"id".to_string()), "id column should exist");
    assert!(column_names.contains(&"user_id".to_string()), "user_id column should exist");
    assert!(column_names.contains(&"team".to_string()), "team column should exist");
    assert!(column_names.contains(&"scopes".to_string()), "scopes column should exist");
    assert!(column_names.contains(&"created_at".to_string()), "created_at column should exist");
}

#[tokio::test]
async fn test_user_team_memberships_indexes() {
    let pool = create_test_pool().await;

    // Verify indexes exist
    let indexes = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='user_team_memberships'",
    )
    .fetch_all(&pool)
    .await
    .expect("Failed to fetch indexes");

    let index_names: Vec<String> = indexes.iter().map(|row| row.get("name")).collect();

    // Verify required indexes exist
    assert!(
        index_names.contains(&"idx_user_team_memberships_user_team".to_string()),
        "idx_user_team_memberships_user_team index should exist"
    );
    assert!(
        index_names.contains(&"idx_user_team_memberships_user_id".to_string()),
        "idx_user_team_memberships_user_id index should exist"
    );
    assert!(
        index_names.contains(&"idx_user_team_memberships_team".to_string()),
        "idx_user_team_memberships_team index should exist"
    );
}

#[tokio::test]
async fn test_user_team_memberships_unique_constraint() {
    let pool = create_test_pool().await;

    // First create a user
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name)
        VALUES ('user-1', 'test@example.com', 'hash123', 'Test User')
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert user");

    // Insert a membership
    sqlx::query(
        r#"
        INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at)
        VALUES ('membership-1', 'user-1', 'team-a', '["read", "write"]', datetime('now'))
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert first membership");

    // Try to insert duplicate membership (same user_id and team)
    let result = sqlx::query(
        r#"
        INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at)
        VALUES ('membership-2', 'user-1', 'team-a', '["admin"]', datetime('now'))
        "#,
    )
    .execute(&pool)
    .await;

    // Should fail due to unique constraint on (user_id, team)
    assert!(result.is_err(), "Duplicate user-team membership should be rejected");
}

#[tokio::test]
async fn test_user_team_memberships_foreign_key_cascade() {
    let pool = create_test_pool().await;

    // Create a user
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name)
        VALUES ('user-1', 'test@example.com', 'hash123', 'Test User')
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert user");

    // Create memberships for the user
    for i in 1..=3 {
        sqlx::query(
            r#"
            INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at)
            VALUES (?, 'user-1', ?, '["read"]', datetime('now'))
            "#,
        )
        .bind(format!("membership-{}", i))
        .bind(format!("team-{}", i))
        .execute(&pool)
        .await
        .expect("Failed to insert membership");
    }

    // Verify memberships exist
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM user_team_memberships WHERE user_id = 'user-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count memberships");
    assert_eq!(count, 3, "Should have 3 memberships");

    // Delete the user
    sqlx::query("DELETE FROM users WHERE id = 'user-1'")
        .execute(&pool)
        .await
        .expect("Failed to delete user");

    // Verify memberships were cascade deleted
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM user_team_memberships WHERE user_id = 'user-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count memberships after user deletion");
    assert_eq!(count, 0, "Memberships should be cascade deleted");
}

// Tests for personal_access_tokens user columns migration (20251112000003)

#[tokio::test]
async fn test_personal_access_tokens_user_columns_added() {
    let pool = create_test_pool().await;

    // Verify new columns exist
    let schema = sqlx::query("PRAGMA table_info(personal_access_tokens)")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch table info");

    let column_names: Vec<String> = schema.iter().map(|row| row.get("name")).collect();

    assert!(column_names.contains(&"user_id".to_string()), "user_id column should exist");
    assert!(column_names.contains(&"user_email".to_string()), "user_email column should exist");
}

#[tokio::test]
async fn test_personal_access_tokens_user_indexes() {
    let pool = create_test_pool().await;

    // Verify indexes exist
    let indexes = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='personal_access_tokens'",
    )
    .fetch_all(&pool)
    .await
    .expect("Failed to fetch indexes");

    let index_names: Vec<String> = indexes.iter().map(|row| row.get("name")).collect();

    assert!(
        index_names.contains(&"idx_personal_access_tokens_user_id".to_string()),
        "idx_personal_access_tokens_user_id index should exist"
    );
    assert!(
        index_names.contains(&"idx_personal_access_tokens_user_email".to_string()),
        "idx_personal_access_tokens_user_email index should exist"
    );
    assert!(
        index_names.contains(&"idx_personal_access_tokens_user_status".to_string()),
        "idx_personal_access_tokens_user_status index should exist"
    );
}

#[tokio::test]
async fn test_personal_access_tokens_backward_compatible() {
    let pool = create_test_pool().await;

    // Insert a token without user_id and user_email (backward compatibility)
    sqlx::query(
        r#"
        INSERT INTO personal_access_tokens (id, name, token_hash, status, created_at, updated_at)
        VALUES ('token-1', 'Legacy Token', 'hash123', 'active', datetime('now'), datetime('now'))
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert legacy token");

    // Query the token
    let row =
        sqlx::query("SELECT user_id, user_email FROM personal_access_tokens WHERE id = 'token-1'")
            .fetch_one(&pool)
            .await
            .expect("Failed to fetch token");

    // Verify user columns are NULL for legacy tokens
    let user_id: Option<String> = row.get("user_id");
    let user_email: Option<String> = row.get("user_email");

    assert!(user_id.is_none(), "user_id should be NULL for legacy tokens");
    assert!(user_email.is_none(), "user_email should be NULL for legacy tokens");
}

#[tokio::test]
async fn test_personal_access_tokens_with_user_data() {
    let pool = create_test_pool().await;

    // Create a user
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name)
        VALUES ('user-1', 'test@example.com', 'hash123', 'Test User')
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert user");

    // Insert a token with user data
    sqlx::query(
        r#"
        INSERT INTO personal_access_tokens (id, name, token_hash, status, user_id, user_email, created_at, updated_at)
        VALUES ('token-1', 'User Token', 'hash456', 'active', 'user-1', 'test@example.com', datetime('now'), datetime('now'))
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert token with user data");

    // Query the token
    let row =
        sqlx::query("SELECT user_id, user_email FROM personal_access_tokens WHERE id = 'token-1'")
            .fetch_one(&pool)
            .await
            .expect("Failed to fetch token");

    // Verify user data is stored correctly
    let user_id: Option<String> = row.get("user_id");
    let user_email: Option<String> = row.get("user_email");

    assert_eq!(user_id, Some("user-1".to_string()), "user_id should match");
    assert_eq!(user_email, Some("test@example.com".to_string()), "user_email should match");
}

#[tokio::test]
async fn test_query_tokens_by_user() {
    let pool = create_test_pool().await;

    // Create a user
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name)
        VALUES ('user-1', 'test@example.com', 'hash123', 'Test User')
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to insert user");

    // Insert multiple tokens for the user
    for i in 1..=5 {
        sqlx::query(
            r#"
            INSERT INTO personal_access_tokens (id, name, token_hash, status, user_id, user_email, created_at, updated_at)
            VALUES (?, ?, ?, 'active', 'user-1', 'test@example.com', datetime('now'), datetime('now'))
            "#,
        )
        .bind(format!("token-{}", i))
        .bind(format!("Token {}", i))
        .bind(format!("hash-{}", i))
        .execute(&pool)
        .await
        .expect("Failed to insert token");
    }

    // Query tokens by user_id
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM personal_access_tokens WHERE user_id = 'user-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count user tokens");

    assert_eq!(count, 5, "Should have 5 tokens for the user");

    // Query active tokens by user_id
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM personal_access_tokens WHERE user_id = 'user-1' AND status = 'active'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count active user tokens");

    assert_eq!(count, 5, "Should have 5 active tokens for the user");
}

// Integration test: Complete user workflow

#[tokio::test]
async fn test_complete_user_workflow() {
    let pool = create_test_pool().await;

    // 1. Create a user
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, name, is_admin)
        VALUES ('user-1', 'admin@example.com', 'hash123', 'Admin User', TRUE)
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create user");

    // 2. Add team memberships
    for team in &["team-a", "team-b", "team-c"] {
        sqlx::query(
            r#"
            INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at)
            VALUES (?, 'user-1', ?, '["read", "write", "admin"]', datetime('now'))
            "#,
        )
        .bind(format!("membership-{}", team))
        .bind(team)
        .execute(&pool)
        .await
        .expect("Failed to create membership");
    }

    // 3. Create personal access tokens for the user
    for i in 1..=3 {
        sqlx::query(
            r#"
            INSERT INTO personal_access_tokens (id, name, token_hash, status, user_id, user_email, created_at, updated_at)
            VALUES (?, ?, ?, 'active', 'user-1', 'admin@example.com', datetime('now'), datetime('now'))
            "#,
        )
        .bind(format!("token-{}", i))
        .bind(format!("Token {}", i))
        .bind(format!("hash-{}", i))
        .execute(&pool)
        .await
        .expect("Failed to create token");
    }

    // 4. Query user with all relationships
    let user = sqlx::query("SELECT * FROM users WHERE id = 'user-1'")
        .fetch_one(&pool)
        .await
        .expect("Failed to fetch user");

    let email: String = user.get("email");
    let is_admin: bool = user.get("is_admin");
    assert_eq!(email, "admin@example.com");
    assert!(is_admin);

    // 5. Verify team memberships
    let membership_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM user_team_memberships WHERE user_id = 'user-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count memberships");
    assert_eq!(membership_count, 3);

    // 6. Verify tokens
    let token_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM personal_access_tokens WHERE user_id = 'user-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count tokens");
    assert_eq!(token_count, 3);
}
