//! Integration tests for database schema constraints and referential integrity
//!
//! Tests FK constraints, cascading deletes, and source enum validation

use flowplane::auth::team::CreateTeamRequest;
use flowplane::config::DatabaseConfig;
use flowplane::storage::create_pool;
use flowplane::storage::repositories::team::{SqlxTeamRepository, TeamRepository};
use flowplane::storage::repository::{
    ClusterRepository, CreateClusterRequest, CreateListenerRequest, CreateRouteRequest,
    ListenerRepository, RouteRepository,
};

async fn create_test_pool() -> sqlx::Pool<sqlx::Sqlite> {
    let config = DatabaseConfig {
        url: "sqlite://:memory:".to_string(),
        auto_migrate: true,
        ..Default::default()
    };
    create_pool(&config).await.unwrap()
}

async fn create_test_team(pool: &sqlx::Pool<sqlx::Sqlite>, team_name: &str) {
    let team_repo = SqlxTeamRepository::new(pool.clone());
    let _ = team_repo
        .create_team(CreateTeamRequest {
            name: team_name.to_string(),
            display_name: format!("Test Team {}", team_name),
            description: Some("Test team for integration tests".to_string()),
            owner_user_id: None,
            settings: None,
        })
        .await;
}

#[tokio::test]
async fn test_source_enum_constraint_on_listeners() {
    let pool = create_test_pool().await;

    // Try to insert with invalid source value (should fail)
    let result = sqlx::query(
        "INSERT INTO listeners (id, name, address, port, protocol, configuration, version, source, created_at, updated_at)
         VALUES ('test-id', 'test', '0.0.0.0', 8080, 'HTTP', '{}', 1, 'invalid_source', datetime('now'), datetime('now'))"
    )
    .execute(&pool)
    .await;

    assert!(result.is_err(), "Should fail with invalid source value");
}

#[tokio::test]
async fn test_source_enum_constraint_on_routes() {
    let pool = create_test_pool().await;
    create_test_team(&pool, "test").await;

    // Create a cluster first (required by FK constraint)
    let cluster_repo = ClusterRepository::new(pool.clone());
    cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
            team: Some("test".into()),
        })
        .await
        .unwrap();

    // Try to insert route with invalid source value (should fail)
    let result = sqlx::query(
        "INSERT INTO routes (id, name, path_prefix, cluster_name, configuration, version, source, created_at, updated_at)
         VALUES ('test-id', 'test', '/', 'test-cluster', '{}', 1, 'bad_source', datetime('now'), datetime('now'))"
    )
    .execute(&pool)
    .await;

    assert!(result.is_err(), "Should fail with invalid source value");
}

#[tokio::test]
async fn test_source_enum_constraint_on_clusters() {
    let pool = create_test_pool().await;

    // Try to insert with invalid source value (should fail)
    let result = sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, source, created_at, updated_at)
         VALUES ('test-id', 'test', 'test-svc', '{}', 1, 'wrong_source', datetime('now'), datetime('now'))"
    )
    .execute(&pool)
    .await;

    assert!(result.is_err(), "Should fail with invalid source value");
}

#[tokio::test]
async fn test_valid_source_values_accepted() {
    let pool = create_test_pool().await;
    create_test_team(&pool, "test").await;

    let listener_repo = ListenerRepository::new(pool.clone());
    let cluster_repo = ClusterRepository::new(pool.clone());
    let route_repo = RouteRepository::new(pool.clone());

    // Create resources with valid 'native_api' source (default)
    let listener = listener_repo
        .create(CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "test"}),
            team: Some("test".into()),
        })
        .await
        .unwrap();

    assert_eq!(listener.source, "native_api");

    let cluster = cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
            team: Some("test".into()),
        })
        .await
        .unwrap();

    assert_eq!(cluster.source, "native_api");

    let route = route_repo
        .create(CreateRouteRequest {
            name: "test-route".to_string(),
            path_prefix: "/".to_string(),
            cluster_name: "test-cluster".to_string(),
            configuration: serde_json::json!({"name": "test"}),
            team: Some("test".into()),
            import_id: None,
            route_order: None,
            headers: None,
        })
        .await
        .unwrap();

    assert_eq!(route.source, "native_api");
}
