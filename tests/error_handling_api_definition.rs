//! Tests for error handling in API definition repository
//!
//! These tests verify that:
//! 1. JSON deserialization errors are properly logged and handled
//! 2. Invalid data gracefully degrades instead of panicking
//! 3. Error paths propagate correctly through the system

use flowplane::storage::repositories::api_definition::{
    ApiDefinitionRepository, CreateApiDefinitionRequest, CreateApiRouteRequest,
};
use flowplane::storage::{self, DbPool};
use sqlx::sqlite::SqlitePoolOptions;

async fn create_test_pool() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:")
        .await
        .expect("create sqlite pool");

    storage::run_migrations(&pool).await.expect("run migrations");
    pool
}

#[tokio::test]
async fn test_deserialize_optional_handles_invalid_json() {
    let pool = create_test_pool().await;
    let repo = ApiDefinitionRepository::new(pool.clone());

    // Create a definition with valid JSON
    let request = CreateApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        target_listeners: Some(vec!["listener1".to_string(), "listener2".to_string()]),
        tls_config: Some(serde_json::json!({
            "server_name": "test.example.com",
            "verify_certificate": true
        })),
        metadata: Some(serde_json::json!({
            "env": "production",
            "version": "1.0.0"
        })),
    };

    let definition = repo.create_definition(request).await.expect("create definition");

    // Verify data was stored and deserialized correctly
    assert_eq!(
        definition.target_listeners,
        Some(vec!["listener1".to_string(), "listener2".to_string()])
    );
    assert!(definition.tls_config.is_some());
    assert!(definition.metadata.is_some());

    // Now manually corrupt the JSON in the database
    sqlx::query("UPDATE api_definitions SET tls_config = 'invalid json' WHERE id = $1")
        .bind(definition.id.as_str())
        .execute(&pool)
        .await
        .expect("corrupt tls_config");

    // Fetch the definition - it should handle the invalid JSON gracefully
    let fetched = repo.get_definition(&definition.id).await.expect("fetch definition");

    // Invalid JSON should result in None, not a panic
    assert!(fetched.tls_config.is_none(), "Invalid JSON should deserialize to None");
    assert_eq!(fetched.domain, "test.example.com");
    assert_eq!(fetched.team, "test-team");
}

#[tokio::test]
async fn test_deserialize_required_uses_fallback() {
    let pool = create_test_pool().await;
    let repo = ApiDefinitionRepository::new(pool.clone());

    // Create a definition first
    let def_request = CreateApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        target_listeners: None,
        tls_config: None,
        metadata: None,
    };

    let definition = repo.create_definition(def_request).await.expect("create definition");

    // Create a route with valid JSON
    let route_request = CreateApiRouteRequest {
        api_definition_id: definition.id.as_str().to_string(),
        match_type: "prefix".to_string(),
        match_value: "/api".to_string(),
        case_sensitive: true,
        headers: None,
        rewrite_prefix: None,
        rewrite_regex: None,
        rewrite_substitution: None,
        upstream_targets: serde_json::json!({
            "targets": [{"name": "backend", "endpoint": "backend.svc:8080", "weight": 100}]
        }),
        timeout_seconds: Some(30),
        override_config: None,
        deployment_note: None,
        route_order: 0,
    };

    let route = repo.create_route(route_request).await.expect("create route");

    // Verify valid upstream_targets
    assert!(route.upstream_targets.is_object());

    // Manually corrupt the upstream_targets (required field)
    sqlx::query(
        "UPDATE api_routes SET upstream_targets = 'totally invalid json {[}' WHERE id = $1",
    )
    .bind(route.id.as_str())
    .execute(&pool)
    .await
    .expect("corrupt upstream_targets");

    // Fetch the route - should use fallback value (Null) instead of panicking
    let fetched = repo.get_route(&route.id).await.expect("fetch route");

    // Should fall back to Null instead of panicking
    assert!(fetched.upstream_targets.is_null(), "Invalid required JSON should use fallback (Null)");
    assert_eq!(fetched.match_value, "/api");
}

#[tokio::test]
async fn test_multiple_corrupted_json_fields() {
    let pool = create_test_pool().await;
    let repo = ApiDefinitionRepository::new(pool.clone());

    // Create definition with all JSON fields populated
    let request = CreateApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        target_listeners: Some(vec!["listener1".to_string()]),
        tls_config: Some(serde_json::json!({"server_name": "test.example.com"})),
        metadata: Some(serde_json::json!({"key": "value"})),
    };

    let definition = repo.create_definition(request).await.expect("create definition");

    // Corrupt ALL JSON fields simultaneously
    sqlx::query(
        "UPDATE api_definitions SET
         tls_config = 'bad{json',
         metadata = '}invalid{',
         target_listeners = '[not,valid]'
         WHERE id = $1",
    )
    .bind(definition.id.as_str())
    .execute(&pool)
    .await
    .expect("corrupt all JSON fields");

    // Should still fetch without panicking
    let fetched = repo.get_definition(&definition.id).await.expect("fetch with corrupted JSON");

    // All JSON fields should gracefully degrade to None
    assert!(fetched.tls_config.is_none());
    assert!(fetched.metadata.is_none());
    assert!(fetched.target_listeners.is_none());

    // Non-JSON fields should still be correct
    assert_eq!(fetched.domain, "test.example.com");
    assert_eq!(fetched.team, "test-team");
}

#[tokio::test]
async fn test_list_routes_with_mixed_valid_invalid_json() {
    let pool = create_test_pool().await;
    let repo = ApiDefinitionRepository::new(pool.clone());

    // Create definition
    let def_request = CreateApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        target_listeners: None,
        tls_config: None,
        metadata: None,
    };

    let definition = repo.create_definition(def_request).await.expect("create definition");

    // Create multiple routes
    for i in 0..3 {
        let route_request = CreateApiRouteRequest {
            api_definition_id: definition.id.as_str().to_string(),
            match_type: "prefix".to_string(),
            match_value: format!("/api/v{}", i),
            case_sensitive: true,
            headers: Some(serde_json::json!({"x-version": format!("v{}", i)})),
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{"name": format!("backend-{}", i), "endpoint": "backend.svc:8080"}]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: i as i64,
        };

        repo.create_route(route_request).await.expect("create route");
    }

    // Corrupt the headers field for the second route only
    sqlx::query(
        "UPDATE api_routes SET headers = 'bad json {['
         WHERE match_value = '/api/v1'",
    )
    .execute(&pool)
    .await
    .expect("corrupt one route's headers");

    // List all routes - should work without panicking
    let routes = repo.list_routes(&definition.id).await.expect("list routes");

    assert_eq!(routes.len(), 3);

    // Route 0 should have valid headers
    assert!(routes[0].headers.is_some(), "Route 0 headers should be valid");

    // Route 1 should have None headers (corrupted)
    assert!(routes[1].headers.is_none(), "Route 1 headers should be None due to corruption");

    // Route 2 should have valid headers
    assert!(routes[2].headers.is_some(), "Route 2 headers should be valid");

    // All routes should have other fields intact
    for (i, route) in routes.iter().enumerate() {
        assert_eq!(route.match_value, format!("/api/v{}", i));
    }
}

#[tokio::test]
async fn test_empty_json_arrays_and_objects() {
    let pool = create_test_pool().await;
    let repo = ApiDefinitionRepository::new(pool.clone());

    // Create definition with empty arrays/objects
    let request = CreateApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        target_listeners: Some(vec![]),          // Empty array
        tls_config: Some(serde_json::json!({})), // Empty object
        metadata: Some(serde_json::json!({})),
    };

    let definition = repo.create_definition(request).await.expect("create with empty JSON");

    // Verify empty structures are preserved
    assert_eq!(definition.target_listeners, Some(vec![]));
    assert_eq!(definition.tls_config, Some(serde_json::json!({})));
    assert_eq!(definition.metadata, Some(serde_json::json!({})));

    // Fetch and verify persistence
    let fetched = repo.get_definition(&definition.id).await.expect("fetch");
    assert_eq!(fetched.target_listeners, Some(vec![]));
}

#[tokio::test]
async fn test_null_vs_missing_json_fields() {
    let pool = create_test_pool().await;
    let repo = ApiDefinitionRepository::new(pool.clone());

    // Create with explicit None values
    let request = CreateApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        target_listeners: None,
        tls_config: None,
        metadata: None,
    };

    let definition = repo.create_definition(request).await.expect("create with None");

    // All optional JSON fields should be None
    assert!(definition.target_listeners.is_none());
    assert!(definition.tls_config.is_none());
    assert!(definition.metadata.is_none());
}
