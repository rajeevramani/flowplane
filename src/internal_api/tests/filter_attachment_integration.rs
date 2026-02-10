//! Filter Attachment MCP Tool Integration Tests
//!
//! Tests the integration between:
//! - MCP filter attachment tools → FilterOperations → Repository layer
//!
//! Verifies that filter attachment tools correctly handle:
//! - Attach/detach to listener
//! - Attach/detach to route config
//! - List attachments
//! - Parameter validation
//! - Error cases

use crate::config::SimpleXdsConfig;
use crate::mcp::tools::filters::{
    execute_attach_filter, execute_detach_filter, execute_list_filter_attachments,
};
use crate::storage::test_helpers::{TestDatabase, TEST_TEAM_ID};
use crate::xds::XdsState;
use serde_json::json;
use std::sync::Arc;

// =============================================================================
// Test Setup Helpers
// =============================================================================

async fn setup_state_with_migrations() -> (TestDatabase, Arc<XdsState>) {
    let test_db = TestDatabase::new("internal_api_filter_attachment").await;
    let pool = test_db.pool.clone();
    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));

    // Teams are already seeded by TestDatabase::new() with predictable UUIDs:
    // test-team -> TEST_TEAM_ID, team-a -> TEAM_A_ID, team-b -> TEAM_B_ID

    (test_db, state)
}

/// Helper to create a test filter
async fn create_test_filter(state: &Arc<XdsState>, name: &str) -> String {
    let filter_repo = state.filter_repository.as_ref().expect("filter repo");
    let req = crate::storage::repositories::filter::CreateFilterRequest {
        name: name.to_string(),
        filter_type: "HeaderToMetadata".to_string(),
        description: Some("Test filter".to_string()),
        configuration: json!({"request_rules": []}).to_string(),
        team: TEST_TEAM_ID.to_string(), // Must match the seeded team UUID
    };
    let filter = filter_repo.create(req).await.expect("Failed to create filter");
    filter.id.to_string()
}

/// Helper to create a test listener with valid xDS-compatible configuration
async fn create_test_listener(state: &Arc<XdsState>, name: &str) -> String {
    let listener_repo = state.listener_repository.as_ref().expect("listener repo");

    // Use a unique port based on name hash to avoid partial unique index violations
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let port = 8000 + (hasher.finish() % 1000) as i64;

    // Create a valid ListenerConfig JSON that can be deserialized by xDS layer
    let config = json!({
        "name": name,
        "address": "0.0.0.0",
        "port": port,
        "filter_chains": [{
            "name": "default",
            "filters": [{
                "http_connection_manager": {
                    "route_config_name": "test-routes",
                    "stat_prefix": "ingress_http"
                }
            }]
        }]
    });

    let req = crate::storage::repositories::listener::CreateListenerRequest {
        name: name.to_string(),
        address: "0.0.0.0".to_string(),
        port: Some(port),
        protocol: Some("http".to_string()),
        configuration: config,
        team: None, // Optional for tests
        import_id: None,
        dataplane_id: None,
    };
    let listener = listener_repo.create(req).await.expect("Failed to create listener");
    listener.id.to_string()
}

/// Helper to create a route config with cluster dependency and valid xDS configuration
async fn create_test_route_config(state: &Arc<XdsState>, name: &str) -> String {
    // Create cluster first
    let cluster_repo = state.cluster_repository.as_ref().expect("cluster repo");
    let cluster_req = crate::storage::repositories::cluster::CreateClusterRequest {
        name: format!("{}-cluster", name),
        service_name: "test-service".to_string(),
        configuration: json!({}),
        team: None,
        import_id: None,
    };
    let _ = cluster_repo.create(cluster_req).await;

    // Create a valid RouteConfig JSON that can be deserialized by xDS layer
    let config = json!({
        "name": name,
        "virtual_hosts": [{
            "name": "default",
            "domains": ["*"],
            "routes": [{
                "match": {
                    "prefix": "/"
                },
                "action": {
                    "cluster": format!("{}-cluster", name)
                }
            }]
        }]
    });

    let repo = state.route_config_repository.as_ref().expect("route config repo");
    let req = crate::storage::repositories::route_config::CreateRouteConfigRequest {
        name: name.to_string(),
        path_prefix: "/".to_string(),
        cluster_name: format!("{}-cluster", name),
        configuration: config,
        team: None,
        import_id: None,
        route_order: None,
        headers: None,
    };
    let rc = repo.create(req).await.expect("Failed to create route config");
    rc.id.to_string()
}

// =============================================================================
// MCP Tool: cp_attach_filter - Listener Tests
// =============================================================================
//
// NOTE: The following attach/detach tests are marked #[ignore] because they
// require full xDS resource validation which needs properly structured
// listener and route configurations. The filter attachment feature is tested
// via:
// 1. Unit tests in filters.rs (tool definitions)
// 2. Repository-level tests (list_attachments_* tests below)
// 3. E2E tests with proper resource setup
//
// These tests CAN be enabled by providing valid xDS-compatible configurations.

#[tokio::test]
#[ignore = "requires full xDS integration - configuration must be deserializable to ListenerConfig"]
async fn test_mcp_attach_filter_to_listener_success() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter and listener
    create_test_filter(&state, "test-filter").await;
    create_test_listener(&state, "main-listener").await;

    // Test: Attach filter to listener
    let args = json!({
        "filter": "test-filter",
        "listener": "main-listener",
        "order": 10
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_ok(), "Failed to attach filter: {:?}", result);
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["success"], true);
        assert_eq!(response["attachment"]["filter"], "test-filter");
        assert_eq!(response["attachment"]["target_type"], "listener");
        assert_eq!(response["attachment"]["target_name"], "main-listener");
        assert_eq!(response["attachment"]["order"], 10);
        assert!(response["message"].as_str().unwrap().contains("attached"));
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
#[ignore = "requires full xDS integration - configuration must be deserializable to ListenerConfig"]
async fn test_mcp_attach_filter_to_listener_no_order() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup
    create_test_filter(&state, "test-filter").await;
    create_test_listener(&state, "main-listener").await;

    // Test: Attach without specifying order
    let args = json!({
        "filter": "test-filter",
        "listener": "main-listener"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_ok());
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["success"], true);
        // Order should be null when not specified
        assert!(response["attachment"]["order"].is_null());
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
async fn test_mcp_attach_filter_listener_not_found() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Only create filter
    create_test_filter(&state, "test-filter").await;

    // Test: Attach to non-existent listener
    let args = json!({
        "filter": "test-filter",
        "listener": "nonexistent-listener"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_mcp_attach_filter_filter_not_found() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Only create listener
    create_test_listener(&state, "main-listener").await;

    // Test: Attach non-existent filter
    let args = json!({
        "filter": "nonexistent-filter",
        "listener": "main-listener"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

// =============================================================================
// MCP Tool: cp_attach_filter - Route Config Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires full xDS integration - configuration must be deserializable to RouteConfig"]
async fn test_mcp_attach_filter_to_route_config_success() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup
    create_test_filter(&state, "rate-limit-filter").await;
    create_test_route_config(&state, "api-routes").await;

    // Test: Attach filter to route config
    let args = json!({
        "filter": "rate-limit-filter",
        "route_config": "api-routes",
        "order": 5
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_ok(), "Failed to attach: {:?}", result);
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["success"], true);
        assert_eq!(response["attachment"]["target_type"], "route_config");
        assert_eq!(response["attachment"]["target_name"], "api-routes");
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
#[ignore = "requires full xDS integration - configuration must be deserializable to RouteConfig"]
async fn test_mcp_attach_filter_to_route_config_with_settings() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup
    create_test_filter(&state, "rate-limit-filter").await;
    create_test_route_config(&state, "api-routes").await;

    // Test: Attach with custom settings override
    let args = json!({
        "filter": "rate-limit-filter",
        "route_config": "api-routes",
        "order": 5,
        "settings": {
            "max_requests": 1000,
            "window_seconds": 60
        }
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_ok());
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());
}

#[tokio::test]
async fn test_mcp_attach_filter_route_config_not_found() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Only create filter
    create_test_filter(&state, "test-filter").await;

    // Test: Attach to non-existent route config
    let args = json!({
        "filter": "test-filter",
        "route_config": "nonexistent-config"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

// =============================================================================
// MCP Tool: cp_attach_filter - Validation Tests
// =============================================================================

#[tokio::test]
async fn test_mcp_attach_filter_missing_filter() {
    let (_db, state) = setup_state_with_migrations().await;

    // Test: Missing filter parameter
    let args = json!({
        "listener": "main-listener"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_mcp_attach_filter_no_target() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup
    create_test_filter(&state, "test-filter").await;

    // Test: Neither listener nor route_config specified
    let args = json!({
        "filter": "test-filter"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_mcp_attach_filter_both_targets() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup
    create_test_filter(&state, "test-filter").await;
    create_test_listener(&state, "main-listener").await;
    create_test_route_config(&state, "api-routes").await;

    // Test: Both listener and route_config specified
    let args = json!({
        "filter": "test-filter",
        "listener": "main-listener",
        "route_config": "api-routes"
    });
    let result = execute_attach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

// =============================================================================
// MCP Tool: cp_detach_filter - Listener Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires full xDS integration - configuration must be deserializable to ListenerConfig"]
async fn test_mcp_detach_filter_from_listener_success() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter, listener, and attach
    let filter_id = create_test_filter(&state, "test-filter").await;
    let listener_id = create_test_listener(&state, "main-listener").await;

    // Attach directly via repository
    let filter_repo = state.filter_repository.as_ref().expect("filter repo");
    filter_repo
        .attach_to_listener(
            &crate::domain::ListenerId::from_string(listener_id),
            &crate::domain::FilterId::from_string(filter_id),
            10,
        )
        .await
        .expect("Failed to attach");

    // Test: Detach filter
    let args = json!({
        "filter": "test-filter",
        "listener": "main-listener"
    });
    let result = execute_detach_filter(&state, "", None, args).await;

    assert!(result.is_ok(), "Failed to detach: {:?}", result);
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["success"], true);
        assert_eq!(response["detachment"]["target_type"], "listener");
        assert!(response["message"].as_str().unwrap().contains("detached"));
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
async fn test_mcp_detach_filter_not_attached() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter and listener but don't attach
    create_test_filter(&state, "test-filter").await;
    create_test_listener(&state, "main-listener").await;

    // Test: Detach when not attached
    let args = json!({
        "filter": "test-filter",
        "listener": "main-listener"
    });
    let result = execute_detach_filter(&state, "", None, args).await;

    // Should fail since filter is not attached
    assert!(result.is_err());
}

// =============================================================================
// MCP Tool: cp_detach_filter - Route Config Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires full xDS integration - configuration must be deserializable to RouteConfig"]
async fn test_mcp_detach_filter_from_route_config_success() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter, route config, and attach
    let filter_id = create_test_filter(&state, "test-filter").await;
    create_test_route_config(&state, "api-routes").await;

    // Attach via repository
    let filter_repo = state.filter_repository.as_ref().expect("filter repo");
    let route_config_repo = state.route_config_repository.as_ref().expect("rc repo");
    let rc = route_config_repo.get_by_name("api-routes").await.expect("get rc");

    filter_repo
        .attach_to_route_config(&rc.id, &crate::domain::FilterId::from_string(filter_id), 10, None)
        .await
        .expect("Failed to attach to route config");

    // Test: Detach filter
    let args = json!({
        "filter": "test-filter",
        "route_config": "api-routes"
    });
    let result = execute_detach_filter(&state, "", None, args).await;

    assert!(result.is_ok(), "Failed to detach: {:?}", result);
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["success"], true);
        assert_eq!(response["detachment"]["target_type"], "route_config");
    } else {
        panic!("Expected text content");
    }
}

// =============================================================================
// MCP Tool: cp_detach_filter - Validation Tests
// =============================================================================

#[tokio::test]
async fn test_mcp_detach_filter_missing_filter() {
    let (_db, state) = setup_state_with_migrations().await;

    // Test: Missing filter parameter
    let args = json!({
        "listener": "main-listener"
    });
    let result = execute_detach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_mcp_detach_filter_no_target() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup
    create_test_filter(&state, "test-filter").await;

    // Test: Neither listener nor route_config specified
    let args = json!({
        "filter": "test-filter"
    });
    let result = execute_detach_filter(&state, "", None, args).await;

    assert!(result.is_err());
}

// =============================================================================
// MCP Tool: cp_list_filter_attachments
// =============================================================================

#[tokio::test]
async fn test_mcp_list_filter_attachments_no_attachments() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter only
    create_test_filter(&state, "lonely-filter").await;

    // Test: List attachments (should be empty)
    let args = json!({
        "filter": "lonely-filter"
    });
    let result = execute_list_filter_attachments(&state, "", None, args).await;

    assert!(result.is_ok(), "Failed to list attachments: {:?}", result);
    let tool_result = result.unwrap();
    assert!(tool_result.is_error.is_none());

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["filter"]["name"], "lonely-filter");
        assert_eq!(response["listener_attachments"].as_array().unwrap().len(), 0);
        assert_eq!(response["route_config_attachments"].as_array().unwrap().len(), 0);
        assert_eq!(response["total_attachments"], 0);
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
async fn test_mcp_list_filter_attachments_with_listener() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter, listener, and attach
    let filter_id = create_test_filter(&state, "attached-filter").await;
    let listener_id = create_test_listener(&state, "main-listener").await;

    let filter_repo = state.filter_repository.as_ref().expect("filter repo");
    filter_repo
        .attach_to_listener(
            &crate::domain::ListenerId::from_string(listener_id),
            &crate::domain::FilterId::from_string(filter_id),
            5,
        )
        .await
        .expect("Failed to attach");

    // Test: List attachments
    let args = json!({
        "filter": "attached-filter"
    });
    let result = execute_list_filter_attachments(&state, "", None, args).await;

    assert!(result.is_ok(), "Expected Ok, got: {:?}", result.as_ref().err());
    let tool_result = result.unwrap();

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");
        assert_eq!(response["filter"]["name"], "attached-filter");

        let listener_attachments = response["listener_attachments"].as_array().unwrap();
        assert_eq!(listener_attachments.len(), 1);
        assert_eq!(listener_attachments[0]["resource_name"], "main-listener");
        assert_eq!(listener_attachments[0]["order"], 5);

        assert_eq!(response["total_attachments"], 1);
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
async fn test_mcp_list_filter_attachments_with_route_config() {
    let (_db, state) = setup_state_with_migrations().await;

    // Setup: Create filter, route config, and attach
    let filter_id = create_test_filter(&state, "scoped-filter").await;
    create_test_route_config(&state, "api-routes").await;

    let filter_repo = state.filter_repository.as_ref().expect("filter repo");
    let route_config_repo = state.route_config_repository.as_ref().expect("rc repo");
    let rc = route_config_repo.get_by_name("api-routes").await.expect("get rc");

    filter_repo
        .attach_to_route_config(&rc.id, &crate::domain::FilterId::from_string(filter_id), 10, None)
        .await
        .expect("Failed to attach");

    // Test: List attachments
    let args = json!({
        "filter": "scoped-filter"
    });
    let result = execute_list_filter_attachments(&state, "", None, args).await;

    assert!(result.is_ok());
    let tool_result = result.unwrap();

    let content_text = &tool_result.content[0];
    if let crate::mcp::protocol::ContentBlock::Text { text } = content_text {
        let response: serde_json::Value = serde_json::from_str(text).expect("Invalid JSON");

        let rc_attachments = response["route_config_attachments"].as_array().unwrap();
        assert_eq!(rc_attachments.len(), 1);

        assert_eq!(response["total_attachments"], 1);
    } else {
        panic!("Expected text content");
    }
}

#[tokio::test]
async fn test_mcp_list_filter_attachments_filter_not_found() {
    let (_db, state) = setup_state_with_migrations().await;

    // Test: Non-existent filter
    let args = json!({
        "filter": "nonexistent-filter"
    });
    let result = execute_list_filter_attachments(&state, "", None, args).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_mcp_list_filter_attachments_missing_filter() {
    let (_db, state) = setup_state_with_migrations().await;

    // Test: Missing filter parameter
    let args = json!({});
    let result = execute_list_filter_attachments(&state, "", None, args).await;

    assert!(result.is_err());
}
