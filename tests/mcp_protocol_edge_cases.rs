//! MCP Protocol Edge Case Tests
//!
//! Comprehensive tests for MCP protocol edge cases including:
//! - Invalid JSON-RPC requests
//! - Protocol version negotiation edge cases
//! - Session management edge cases
//! - Error handling scenarios

use flowplane::config::DatabaseConfig;
use flowplane::mcp::error::McpError;
use flowplane::mcp::handler::McpHandler;
use flowplane::mcp::protocol::*;
use flowplane::mcp::session::{create_session_manager_with_ttl, SessionId, SessionManager};
use flowplane::storage::create_pool;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

// -----------------------------------------------------------------------------
// Test Helpers
// -----------------------------------------------------------------------------

async fn create_test_handler() -> McpHandler {
    let config = DatabaseConfig {
        url: "sqlite://:memory:".to_string(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_seconds: 5,
        idle_timeout_seconds: 0,
        auto_migrate: false,
    };
    let pool = create_pool(&config).await.expect("Failed to create pool");
    McpHandler::new(Arc::new(pool), "test-team".to_string())
}

fn create_test_session_manager() -> SessionManager {
    SessionManager::new(Duration::from_secs(60))
}

// -----------------------------------------------------------------------------
// Invalid JSON-RPC Request Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_jsonrpc_request_with_null_id() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: None,
        method: "ping".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;

    // Null ID should be preserved in response
    assert!(response.id.is_none());
    assert!(response.result.is_some());
}

#[tokio::test]
async fn test_jsonrpc_request_missing_method() {
    let mut handler = create_test_handler().await;

    // Create request with empty method
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    assert_eq!(response.error.unwrap().code, error_codes::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn test_jsonrpc_request_invalid_method_type() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::String("test".to_string())),
        method: "not/a/valid/method".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    let error = response.error.unwrap();
    assert_eq!(error.code, error_codes::METHOD_NOT_FOUND);
    assert!(error.message.contains("not/a/valid/method"));
}

#[tokio::test]
async fn test_initialize_missing_required_fields() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    let error = response.error.unwrap();
    assert_eq!(error.code, error_codes::INVALID_PARAMS);
    assert!(error.message.contains("Failed to parse initialize params"));
}

#[tokio::test]
async fn test_initialize_malformed_client_info() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": "not-an-object"
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    assert_eq!(response.error.unwrap().code, error_codes::INVALID_PARAMS);
}

#[tokio::test]
async fn test_tool_call_missing_name() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "tools/call".to_string(),
        params: json!({
            "arguments": {}
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    assert_eq!(response.error.unwrap().code, error_codes::INVALID_PARAMS);
}

// -----------------------------------------------------------------------------
// Protocol Version Negotiation Edge Cases
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_version_negotiation_empty_string() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }),
    };

    let response = handler.handle_request(request).await;

    // Empty version should default to oldest supported version
    assert!(response.error.is_none());
    assert!(response.result.is_some());
    let result = response.result.unwrap();
    assert_eq!(result["protocolVersion"], "2024-11-05");
}

#[tokio::test]
async fn test_version_negotiation_very_old_version() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "2020-01-01",
            "capabilities": {},
            "clientInfo": {
                "name": "old-client",
                "version": "0.1.0"
            }
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    let error = response.error.unwrap();
    assert_eq!(error.code, error_codes::INVALID_REQUEST);
    assert!(error.message.contains("Unsupported protocol version"));
    assert!(error.data.is_some());
    let data = error.data.unwrap();
    assert!(data["supportedVersions"].is_array());
}

#[tokio::test]
async fn test_version_negotiation_malformed_version() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "not-a-date",
            "capabilities": {},
            "clientInfo": {
                "name": "bad-client",
                "version": "1.0.0"
            }
        }),
    };

    let response = handler.handle_request(request).await;

    // Malformed version: "not-a-date" > "2024-11-05" in lexicographic comparison
    // so it gets negotiated down to latest supported version
    assert!(response.error.is_none());
    let result = response.result.unwrap();
    assert_eq!(result["protocolVersion"], "2025-11-25");
}

#[tokio::test]
async fn test_version_negotiation_future_version() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "2099-12-31",
            "capabilities": {},
            "clientInfo": {
                "name": "future-client",
                "version": "99.0.0"
            }
        }),
    };

    let response = handler.handle_request(request).await;

    // Should negotiate down to our latest supported version
    assert!(response.error.is_none());
    let result = response.result.unwrap();
    assert_eq!(result["protocolVersion"], "2025-11-25");
}

#[tokio::test]
async fn test_version_negotiation_exact_boundary() {
    let mut handler = create_test_handler().await;

    // Test exact match with oldest supported version
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "boundary-client",
                "version": "1.0.0"
            }
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_none());
    let result = response.result.unwrap();
    assert_eq!(result["protocolVersion"], "2024-11-05");
}

// -----------------------------------------------------------------------------
// Session Management Edge Cases
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_session_id_collision() {
    let manager = create_test_session_manager();
    let id = SessionId::from_token("collision-test");

    // Create first session
    let session1 = manager.get_or_create(&id);
    assert!(!session1.initialized);

    // Mark as initialized
    let client_info = ClientInfo { name: "client-1".to_string(), version: "1.0.0".to_string() };
    manager.mark_initialized(&id, "2025-11-25".to_string(), client_info);

    // Try to create/get again with same ID
    let session2 = manager.get_or_create(&id);

    // Should be the same session, still initialized
    assert!(session2.initialized);
    assert_eq!(manager.total_sessions(), 1);
}

#[tokio::test]
async fn test_session_expiration_at_boundary() {
    let manager = SessionManager::new(Duration::from_millis(50));
    let id = SessionId::from_token("expiry-test");

    // Create session
    let _ = manager.get_or_create(&id);
    assert!(manager.exists(&id));

    // Wait exactly at the boundary
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Session should be expired
    let removed = manager.cleanup_expired();
    assert_eq!(removed, 1);
    assert!(!manager.exists(&id));
}

#[tokio::test]
async fn test_session_touch_extends_lifetime() {
    let manager = SessionManager::new(Duration::from_millis(100));
    let id = SessionId::from_token("touch-test");

    // Create session
    let _ = manager.get_or_create(&id);

    // Wait 60ms
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Touch session
    let _ = manager.get_or_create(&id);

    // Wait another 60ms (total 120ms from creation, but only 60ms from touch)
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Session should still exist due to touch
    assert!(manager.exists(&id));

    // Wait another 50ms (110ms from touch)
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Now it should be expired
    let removed = manager.cleanup_expired();
    assert_eq!(removed, 1);
}

#[tokio::test]
async fn test_concurrent_session_initialization() {
    let manager = Arc::new(create_test_session_manager());
    let id = SessionId::from_token("concurrent-init");

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let manager = Arc::clone(&manager);
            let id = id.clone();
            tokio::spawn(async move {
                let client_info =
                    ClientInfo { name: format!("client-{}", i), version: "1.0.0".to_string() };
                manager.mark_initialized(&id, "2025-11-25".to_string(), client_info);
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task failed");
    }

    // Should have exactly one session
    assert_eq!(manager.total_sessions(), 1);
    assert!(manager.is_initialized(&id));
}

#[tokio::test]
async fn test_session_multiple_teams() {
    let manager = create_test_session_manager();

    let id1 = SessionId::from_token("team1-token");
    let id2 = SessionId::from_token("team2-token");

    let _ = manager.get_or_create_for_team(&id1, "team1");
    let _ = manager.get_or_create_for_team(&id2, "team2");

    let team1_sessions = manager.list_sessions_by_team("team1");
    let team2_sessions = manager.list_sessions_by_team("team2");

    assert_eq!(team1_sessions.len(), 1);
    assert_eq!(team2_sessions.len(), 1);
    assert_eq!(manager.total_sessions(), 2);
}

// -----------------------------------------------------------------------------
// Error Handling Edge Cases
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_tool_not_found_error() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "nonexistent_tool",
            "arguments": {}
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    let error = response.error.unwrap();
    assert_eq!(error.code, error_codes::METHOD_NOT_FOUND);
    assert!(error.message.contains("nonexistent_tool"));
}

#[tokio::test]
async fn test_resource_read_invalid_uri() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "resources/read".to_string(),
        params: json!({
            "uri": "invalid://not-a-valid-uri"
        }),
    };

    let response = handler.handle_request(request).await;

    // Error should be returned for invalid URI
    assert!(response.error.is_some());
}

#[tokio::test]
async fn test_prompt_get_nonexistent() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "prompts/get".to_string(),
        params: json!({
            "name": "nonexistent_prompt"
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    // PromptNotFound error maps to METHOD_NOT_FOUND, but invalid params could also be returned
    let error = response.error.unwrap();
    assert!(
        error.code == error_codes::METHOD_NOT_FOUND || error.code == error_codes::INVALID_PARAMS
    );
}

#[tokio::test]
async fn test_mcp_error_to_json_rpc_error_conversion() {
    let error = McpError::ToolNotFound("test_tool".to_string());
    let json_error = error.to_json_rpc_error();

    assert_eq!(json_error.code, error_codes::METHOD_NOT_FOUND);
    assert!(json_error.message.contains("test_tool"));
}

#[tokio::test]
async fn test_mcp_error_validation() {
    let error = McpError::ValidationError("Invalid input".to_string());
    let json_error = error.to_json_rpc_error();

    assert_eq!(json_error.code, error_codes::INVALID_PARAMS);
    assert!(json_error.message.contains("Validation"));
}

#[tokio::test]
async fn test_mcp_error_forbidden() {
    let error = McpError::Forbidden("Access denied".to_string());
    let json_error = error.to_json_rpc_error();

    assert_eq!(json_error.code, error_codes::INVALID_REQUEST);
    assert!(json_error.message.contains("Forbidden"));
}

// -----------------------------------------------------------------------------
// Session Cleanup and Lifecycle Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_session_cleanup_with_no_expired() {
    let manager = create_test_session_manager();
    let id = SessionId::from_token("active-session");

    let _ = manager.get_or_create(&id);

    let removed = manager.cleanup_expired();
    assert_eq!(removed, 0);
    assert!(manager.exists(&id));
}

#[tokio::test]
async fn test_session_remove_nonexistent() {
    let manager = create_test_session_manager();
    let id = SessionId::from_token("nonexistent");

    let removed = manager.remove(&id);
    assert!(!removed);
}

#[tokio::test]
async fn test_session_get_protocol_version_nonexistent() {
    let manager = create_test_session_manager();
    let id = SessionId::from_token("nonexistent");

    let version = manager.get_protocol_version(&id);
    assert!(version.is_none());
}

#[tokio::test]
async fn test_session_manager_with_custom_ttl() {
    let ttl = Duration::from_secs(300);
    let manager = create_session_manager_with_ttl(ttl);

    assert_eq!(manager.ttl(), ttl);
}

// -----------------------------------------------------------------------------
// JSON-RPC ID Edge Cases
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_json_rpc_id_string_variant() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::String("custom-string-id".to_string())),
        method: "ping".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_none());
    assert_eq!(response.id, Some(JsonRpcId::String("custom-string-id".to_string())));
}

#[tokio::test]
async fn test_json_rpc_id_number_variant() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(42)),
        method: "ping".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_none());
    assert_eq!(response.id, Some(JsonRpcId::Number(42)));
}

// -----------------------------------------------------------------------------
// Multiple Requests with Same Handler
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_handler_multiple_requests_sequence() {
    let mut handler = create_test_handler().await;

    // First request: initialize
    let init_request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "initialize".to_string(),
        params: json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }),
    };

    let init_response = handler.handle_request(init_request).await;
    assert!(init_response.error.is_none());

    // Second request: ping
    let ping_request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(2)),
        method: "ping".to_string(),
        params: json!({}),
    };

    let ping_response = handler.handle_request(ping_request).await;
    assert!(ping_response.error.is_none());

    // Third request: tools/list
    let tools_request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(3)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let tools_response = handler.handle_request(tools_request).await;
    assert!(tools_response.error.is_none());
}

// -----------------------------------------------------------------------------
// Additional Error Cases
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_logging_set_level_with_invalid_params() {
    let mut handler = create_test_handler().await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "logging/setLevel".to_string(),
        params: json!({
            "invalid_field": "value"
        }),
    };

    let response = handler.handle_request(request).await;

    assert!(response.error.is_some());
    assert_eq!(response.error.unwrap().code, error_codes::INVALID_PARAMS);
}

#[tokio::test]
async fn test_notification_methods() {
    let mut handler = create_test_handler().await;

    // Test notifications/initialized
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(1)),
        method: "notifications/initialized".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;
    assert!(response.error.is_none());

    // Test notifications/cancelled
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(JsonRpcId::Number(2)),
        method: "notifications/cancelled".to_string(),
        params: json!({}),
    };

    let response = handler.handle_request(request).await;
    assert!(response.error.is_none());
}

#[tokio::test]
async fn test_session_isolation_between_teams() {
    let manager = create_test_session_manager();

    // Create sessions for different teams
    let id1 = SessionId::from_token("token1");
    let id2 = SessionId::from_token("token2");
    let id3 = SessionId::from_token("token3");

    let _ = manager.get_or_create_for_team(&id1, "team-a");
    let _ = manager.get_or_create_for_team(&id2, "team-a");
    let _ = manager.get_or_create_for_team(&id3, "team-b");

    let team_a_sessions = manager.list_sessions_by_team("team-a");
    let team_b_sessions = manager.list_sessions_by_team("team-b");
    let team_c_sessions = manager.list_sessions_by_team("team-c");

    assert_eq!(team_a_sessions.len(), 2);
    assert_eq!(team_b_sessions.len(), 1);
    assert_eq!(team_c_sessions.len(), 0);
}
