//! Integration tests for database schema constraints and referential integrity
//!
//! Tests FK constraints, cascading deletes, and source enum validation

use flowplane::config::DatabaseConfig;
use flowplane::storage::create_pool;
use flowplane::storage::repository::{
    ApiDefinitionRepository, ClusterRepository, CreateApiDefinitionRequest, CreateApiRouteRequest,
    CreateClusterRequest, CreateListenerRequest, CreateRouteRequest, ListenerRepository,
    RouteRepository,
};

async fn create_test_pool() -> sqlx::Pool<sqlx::Sqlite> {
    let config = DatabaseConfig {
        url: "sqlite://:memory:".to_string(),
        auto_migrate: true,
        ..Default::default()
    };
    create_pool(&config).await.unwrap()
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

    // Create a cluster first (required by FK constraint)
    let cluster_repo = ClusterRepository::new(pool.clone());
    cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
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
        })
        .await
        .unwrap();

    assert_eq!(listener.source, "native_api");

    let cluster = cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
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
        })
        .await
        .unwrap();

    assert_eq!(route.source, "native_api");
}

#[tokio::test]
async fn test_api_definition_listener_fk_on_delete_set_null() {
    let pool = create_test_pool().await;
    let api_repo = ApiDefinitionRepository::new(pool.clone());
    let listener_repo = ListenerRepository::new(pool.clone());

    // Create a listener
    let listener = listener_repo
        .create(CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "test"}),
        })
        .await
        .unwrap();

    // Create an API definition
    let api_def = api_repo
        .create_definition(CreateApiDefinitionRequest {
            team: "test-team".to_string(),
            domain: "test.example.com".to_string(),
            listener_isolation: false,
            target_listeners: None,
            tls_config: None,
            metadata: None,
        })
        .await
        .unwrap();

    // Manually update the API definition to reference the listener
    sqlx::query("UPDATE api_definitions SET generated_listener_id = $1 WHERE id = $2")
        .bind(&listener.id)
        .bind(&api_def.id)
        .execute(&pool)
        .await
        .unwrap();

    // Verify the FK is set
    let updated_def = api_repo.get_definition(&api_def.id).await.unwrap();
    assert_eq!(updated_def.generated_listener_id, Some(listener.id.clone()));

    // Delete the listener - should set generated_listener_id to NULL
    listener_repo.delete(&listener.id).await.unwrap();

    // Verify the FK is now NULL
    let final_def = api_repo.get_definition(&api_def.id).await.unwrap();
    assert_eq!(final_def.generated_listener_id, None);
}

#[tokio::test]
async fn test_api_route_fk_on_delete_set_null() {
    let pool = create_test_pool().await;
    let api_repo = ApiDefinitionRepository::new(pool.clone());
    let route_repo = RouteRepository::new(pool.clone());
    let cluster_repo = ClusterRepository::new(pool.clone());

    // Create a cluster first
    let cluster = cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
        })
        .await
        .unwrap();

    // Create a route
    let route = route_repo
        .create(CreateRouteRequest {
            name: "test-route".to_string(),
            path_prefix: "/api".to_string(),
            cluster_name: "test-cluster".to_string(),
            configuration: serde_json::json!({"name": "test"}),
        })
        .await
        .unwrap();

    // Create an API definition
    let api_def = api_repo
        .create_definition(CreateApiDefinitionRequest {
            team: "test-team".to_string(),
            domain: "test.example.com".to_string(),
            listener_isolation: false,
            target_listeners: None,
            tls_config: None,
            metadata: None,
        })
        .await
        .unwrap();

    // Create an API route
    let api_route = api_repo
        .create_route(CreateApiRouteRequest {
            api_definition_id: api_def.id.clone(),
            match_type: "prefix".to_string(),
            match_value: "/api".to_string(),
            case_sensitive: true,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({"targets": []}),
            timeout_seconds: None,
            override_config: None,
            deployment_note: None,
            route_order: 0,
        })
        .await
        .unwrap();

    // Manually update the API route to reference the route and cluster
    sqlx::query(
        "UPDATE api_routes SET generated_route_id = $1, generated_cluster_id = $2 WHERE id = $3",
    )
    .bind(&route.id)
    .bind(&cluster.id)
    .bind(&api_route.id)
    .execute(&pool)
    .await
    .unwrap();

    // Verify the FKs are set
    let updated_route = api_repo.get_route(&api_route.id).await.unwrap();
    assert_eq!(updated_route.generated_route_id, Some(route.id.clone()));
    assert_eq!(updated_route.generated_cluster_id, Some(cluster.id.clone()));

    // Delete the route - should set generated_route_id to NULL
    route_repo.delete(&route.id).await.unwrap();

    // Verify generated_route_id is now NULL
    let after_route_delete = api_repo.get_route(&api_route.id).await.unwrap();
    assert_eq!(after_route_delete.generated_route_id, None);
    assert_eq!(after_route_delete.generated_cluster_id, Some(cluster.id.clone()));

    // Delete the cluster - should set generated_cluster_id to NULL
    cluster_repo.delete(&cluster.id).await.unwrap();

    // Verify generated_cluster_id is now NULL
    let after_cluster_delete = api_repo.get_route(&api_route.id).await.unwrap();
    assert_eq!(after_cluster_delete.generated_route_id, None);
    assert_eq!(after_cluster_delete.generated_cluster_id, None);
}

#[tokio::test]
async fn test_api_definition_cascading_delete_to_routes() {
    let pool = create_test_pool().await;
    let api_repo = ApiDefinitionRepository::new(pool.clone());

    // Create an API definition
    let api_def = api_repo
        .create_definition(CreateApiDefinitionRequest {
            team: "test-team".to_string(),
            domain: "test.example.com".to_string(),
            listener_isolation: false,
            target_listeners: None,
            tls_config: None,
            metadata: None,
        })
        .await
        .unwrap();

    // Create multiple API routes
    for i in 0..3 {
        api_repo
            .create_route(CreateApiRouteRequest {
                api_definition_id: api_def.id.clone(),
                match_type: "prefix".to_string(),
                match_value: format!("/api{}", i),
                case_sensitive: true,
                rewrite_prefix: None,
                rewrite_regex: None,
                rewrite_substitution: None,
                upstream_targets: serde_json::json!({"targets": []}),
                timeout_seconds: None,
                override_config: None,
                deployment_note: None,
                route_order: i,
            })
            .await
            .unwrap();
    }

    // Verify routes exist
    let routes = api_repo.list_routes(&api_def.id).await.unwrap();
    assert_eq!(routes.len(), 3);

    // Delete the API definition - should cascade to routes
    api_repo.delete_definition(&api_def.id).await.unwrap();

    // Verify API definition is deleted
    let def_result = api_repo.get_definition(&api_def.id).await;
    assert!(def_result.is_err());

    // Verify routes are also deleted (cascading delete)
    let routes_after = api_repo.list_routes(&api_def.id).await.unwrap();
    assert_eq!(routes_after.len(), 0);
}

#[tokio::test]
async fn test_referential_integrity_prevents_orphaned_records() {
    let pool = create_test_pool().await;

    // Try to create an API route without a valid API definition (should fail)
    let result = sqlx::query(
        "INSERT INTO api_routes (id, api_definition_id, match_type, match_value, case_sensitive, upstream_targets, route_order, created_at, updated_at)
         VALUES ('route-id', 'nonexistent-def-id', 'prefix', '/', 1, '{}', 0, datetime('now'), datetime('now'))"
    )
    .execute(&pool)
    .await;

    assert!(result.is_err(), "Should fail due to FK constraint violation");
}
