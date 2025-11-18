/// Integration tests for team foreign key constraints
///
/// Tests that verify:
/// 1. FK constraints prevent invalid team references
/// 2. CASCADE delete behavior for ephemeral data (memberships, learning sessions, schemas)
/// 3. RESTRICT delete behavior for core resources (api_definitions, clusters, routes, listeners)
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;

async fn setup_test_db() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:")
        .await
        .expect("Failed to create test database");

    // Run migrations
    flowplane::storage::migrations::run_migrations(&pool).await.expect("Failed to run migrations");

    pool
}

#[tokio::test]
async fn test_fk_prevents_invalid_team_in_user_team_memberships() {
    let pool = setup_test_db().await;

    // Try to create a membership with non-existent team
    let result = sqlx::query(
        "INSERT INTO user_team_memberships (id, user_id, team, scopes)
         VALUES ('mem-001', 'user-001', 'nonexistent-team', '[]')",
    )
    .execute(&pool)
    .await;

    // Should fail with FK constraint error
    assert!(result.is_err(), "FK constraint should prevent invalid team reference");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("FOREIGN KEY constraint failed"),
        "Expected FK constraint error, got: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_fk_prevents_invalid_team_in_api_definitions() {
    let pool = setup_test_db().await;

    // Try to create an API definition with non-existent team
    let result = sqlx::query(
        "INSERT INTO api_definitions (id, team, domain)
         VALUES ('api-001', 'nonexistent-team', 'api.example.com')",
    )
    .execute(&pool)
    .await;

    // Should fail with FK constraint error
    assert!(result.is_err(), "FK constraint should prevent invalid team reference");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("FOREIGN KEY constraint failed"),
        "Expected FK constraint error, got: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_cascade_delete_user_team_memberships() {
    let pool = setup_test_db().await;

    // Create a team
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, status)
         VALUES ('team-001', 'engineering', 'Engineering Team', 'active')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create team");

    // Create a user
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, name)
         VALUES ('user-001', 'test@example.com', 'hash', 'Test User')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create user");

    // Create a membership
    sqlx::query(
        "INSERT INTO user_team_memberships (id, user_id, team, scopes)
         VALUES ('mem-001', 'user-001', 'engineering', '[\"team:engineering:*:*\"]')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create membership");

    // Verify membership exists
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_team_memberships WHERE team = 'engineering'")
            .fetch_one(&pool)
            .await
            .expect("Failed to count memberships");
    assert_eq!(count, 1, "Membership should exist");

    // Delete the team
    sqlx::query("DELETE FROM teams WHERE name = 'engineering'")
        .execute(&pool)
        .await
        .expect("Failed to delete team");

    // Verify membership was CASCADE deleted
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_team_memberships WHERE team = 'engineering'")
            .fetch_one(&pool)
            .await
            .expect("Failed to count memberships");
    assert_eq!(count, 0, "Membership should be CASCADE deleted with team");
}

#[tokio::test]
async fn test_cascade_delete_learning_sessions() {
    let pool = setup_test_db().await;

    // Create a team
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, status)
         VALUES ('team-001', 'platform', 'Platform Team', 'active')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create team");

    // Create a learning session
    sqlx::query(
        "INSERT INTO learning_sessions (id, team, route_pattern, target_sample_count)
         VALUES ('session-001', 'platform', '/api/users', 100)",
    )
    .execute(&pool)
    .await
    .expect("Failed to create learning session");

    // Verify session exists
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM learning_sessions WHERE team = 'platform'")
            .fetch_one(&pool)
            .await
            .expect("Failed to count sessions");
    assert_eq!(count, 1, "Learning session should exist");

    // Delete the team
    sqlx::query("DELETE FROM teams WHERE name = 'platform'")
        .execute(&pool)
        .await
        .expect("Failed to delete team");

    // Verify session was CASCADE deleted
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM learning_sessions WHERE team = 'platform'")
            .fetch_one(&pool)
            .await
            .expect("Failed to count sessions");
    assert_eq!(count, 0, "Learning session should be CASCADE deleted with team");
}

#[tokio::test]
async fn test_restrict_delete_api_definitions() {
    let pool = setup_test_db().await;

    // Create a team
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, status)
         VALUES ('team-001', 'products', 'Products Team', 'active')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create team");

    // Create an API definition
    sqlx::query(
        "INSERT INTO api_definitions (id, team, domain)
         VALUES ('api-001', 'products', 'api.products.com')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create API definition");

    // Try to delete the team (should fail due to RESTRICT)
    let result = sqlx::query("DELETE FROM teams WHERE name = 'products'").execute(&pool).await;

    // Should fail with FK constraint error
    assert!(result.is_err(), "RESTRICT constraint should prevent team deletion");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("FOREIGN KEY constraint failed"),
        "Expected FK constraint error, got: {}",
        err_msg
    );

    // Verify team still exists
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM teams WHERE name = 'products'")
        .fetch_one(&pool)
        .await
        .expect("Failed to count teams");
    assert_eq!(count, 1, "Team should still exist after failed delete");

    // Verify API definition still exists
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_definitions WHERE team = 'products'")
            .fetch_one(&pool)
            .await
            .expect("Failed to count API definitions");
    assert_eq!(count, 1, "API definition should still exist");
}

#[tokio::test]
async fn test_restrict_delete_clusters() {
    let pool = setup_test_db().await;

    // Create a team
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, status)
         VALUES ('team-001', 'services', 'Services Team', 'active')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create team");

    // Create a cluster with team
    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, team)
         VALUES ('cluster-001', 'web-cluster', 'web-service', '{}', 'services')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create cluster");

    // Try to delete the team (should fail due to RESTRICT)
    let result = sqlx::query("DELETE FROM teams WHERE name = 'services'").execute(&pool).await;

    // Should fail with FK constraint error
    assert!(result.is_err(), "RESTRICT constraint should prevent team deletion");

    // Verify team and cluster still exist
    let team_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM teams WHERE name = 'services'")
        .fetch_one(&pool)
        .await
        .expect("Failed to count teams");
    assert_eq!(team_count, 1, "Team should still exist");

    let cluster_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM clusters WHERE team = 'services'")
            .fetch_one(&pool)
            .await
            .expect("Failed to count clusters");
    assert_eq!(cluster_count, 1, "Cluster should still exist");
}

#[tokio::test]
async fn test_null_team_in_clusters_allowed() {
    let pool = setup_test_db().await;

    // Create a cluster with NULL team (global resource)
    let result = sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, team)
         VALUES ('cluster-001', 'global-cluster', 'global-service', '{}', NULL)",
    )
    .execute(&pool)
    .await;

    // Should succeed - NULL teams are allowed for global resources
    assert!(result.is_ok(), "NULL team should be allowed for global resources");

    // Verify cluster exists with NULL team
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM clusters WHERE name = 'global-cluster' AND team IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count clusters");
    assert_eq!(count, 1, "Global cluster with NULL team should exist");
}

#[tokio::test]
async fn test_valid_team_reference_succeeds() {
    let pool = setup_test_db().await;

    // Create a team
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, status)
         VALUES ('team-001', 'payments', 'Payments Team', 'active')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create team");

    // Create a user
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, name)
         VALUES ('user-001', 'test@example.com', 'hash', 'Test User')",
    )
    .execute(&pool)
    .await
    .expect("Failed to create user");

    // Create resources with valid team reference - all should succeed
    let membership_result = sqlx::query(
        "INSERT INTO user_team_memberships (id, user_id, team, scopes)
         VALUES ('mem-001', 'user-001', 'payments', '[\"team:payments:*:*\"]')",
    )
    .execute(&pool)
    .await;
    assert!(membership_result.is_ok(), "Valid team reference should succeed");

    let api_def_result = sqlx::query(
        "INSERT INTO api_definitions (id, team, domain)
         VALUES ('api-001', 'payments', 'api.payments.com')",
    )
    .execute(&pool)
    .await;
    assert!(api_def_result.is_ok(), "Valid team reference should succeed");

    let cluster_result = sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, team)
         VALUES ('cluster-001', 'payments-cluster', 'payments-svc', '{}', 'payments')",
    )
    .execute(&pool)
    .await;
    assert!(cluster_result.is_ok(), "Valid team reference should succeed");

    let learning_result = sqlx::query(
        "INSERT INTO learning_sessions (id, team, route_pattern, target_sample_count)
         VALUES ('session-001', 'payments', '/api/payments', 50)",
    )
    .execute(&pool)
    .await;
    assert!(learning_result.is_ok(), "Valid team reference should succeed");
}
