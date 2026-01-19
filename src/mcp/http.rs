//! MCP HTTP Transport
//!
//! Provides HTTP endpoint for MCP protocol, enabling remote AI agents to call MCP over HTTP.

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, error};

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::connection::ConnectionId;
use crate::mcp::handler::McpHandler;
use crate::mcp::protocol::{
    error_codes, InitializeParams, JsonRpcError, JsonRpcRequest, JsonRpcResponse,
};
use crate::mcp::session::SessionId;
use crate::storage::DbPool;

/// Header name for MCP connection ID linking HTTP requests to SSE connections
const MCP_CONNECTION_ID_HEADER: &str = "mcp-connection-id";

/// Query parameters for MCP HTTP endpoint
#[derive(Debug, Deserialize)]
pub struct McpHttpQuery {
    pub team: Option<String>,
}

/// Extract team name from query parameters or auth context
///
/// Priority:
/// 1. Query parameter ?team=<name>
/// 2. Token scopes with pattern team:{name}:*
/// 3. For admin:all scope, team query parameter is required
fn extract_team(query: &McpHttpQuery, context: &AuthContext) -> Result<String, String> {
    // First check query parameter
    if let Some(team) = &query.team {
        debug!(team = %team, "Team extracted from query parameter");
        return Ok(team.clone());
    }

    // Check if user has admin:all scope - they MUST provide team via query param
    if context.has_scope("admin:all") {
        error!("Admin user must provide team via query parameter");
        return Err("Admin users must specify team via query parameter".to_string());
    }

    // Extract team from scopes (pattern: team:{name}:*)
    for scope in context.scopes() {
        if let Some(team_part) = scope.strip_prefix("team:") {
            if let Some(team_name) = team_part.split(':').next() {
                debug!(team = %team_name, scope = %scope, "Team extracted from token scope");
                return Ok(team_name.to_string());
            }
        }
    }

    error!("No team found in query parameter or token scopes");
    Err("Unable to determine team. Please provide team via query parameter".to_string())
}

/// Check if auth context has required scope for the given method
fn check_authorization(method: &str, context: &AuthContext) -> Result<(), String> {
    match method {
        // No scope required for initialization and ping
        "initialize" | "initialized" | "ping" => Ok(()),

        // Read operations
        "tools/list" | "resources/list" | "prompts/list" => {
            if context.has_scope("mcp:read") || context.has_scope("admin:all") {
                Ok(())
            } else {
                Err(format!("Missing required scope 'mcp:read' for method '{}'", method))
            }
        }

        // Execute operations
        "tools/call" | "prompts/get" => {
            if context.has_scope("mcp:execute") || context.has_scope("admin:all") {
                Ok(())
            } else {
                Err(format!("Missing required scope 'mcp:execute' for method '{}'", method))
            }
        }

        // Resource read operations (use control plane read scope)
        "resources/read" => {
            if context.has_scope("cp:read") || context.has_scope("admin:all") {
                Ok(())
            } else {
                Err(format!("Missing required scope 'cp:read' for method '{}'", method))
            }
        }

        // Logging operations
        "logging/setLevel" => {
            if context.has_scope("mcp:read") || context.has_scope("admin:all") {
                Ok(())
            } else {
                Err(format!("Missing required scope 'mcp:read' for method '{}'", method))
            }
        }

        _ => {
            // Unknown method - let handler deal with it
            Ok(())
        }
    }
}

/// Get database pool from API state
fn get_db_pool(state: &ApiState) -> Result<DbPool, String> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| "Database not available".to_string())?;

    Ok(cluster_repo.pool().clone())
}

/// POST /api/v1/mcp
///
/// HTTP endpoint for MCP protocol. Accepts JSON-RPC 2.0 requests and returns responses.
///
/// # Authentication
/// Requires a valid bearer token with appropriate scopes.
///
/// # Team Resolution
/// Team is resolved in the following priority:
/// 1. Query parameter ?team=<name>
/// 2. Token scopes (team:{name}:*)
/// 3. Admin users must provide team via query parameter
///
/// # Headers
/// - `Mcp-Connection-Id`: Optional. When provided, links this HTTP request to an SSE connection.
///   On successful `initialize`, client metadata will be stored for the connection.
///
/// # Authorization
/// - `initialize`, `initialized` - No scope required
/// - `tools/list`, `resources/list` - Require `mcp:read`
/// - `tools/call` - Require `mcp:execute`
/// - `resources/read` - Require `cp:read`
#[utoipa::path(
    post,
    path = "/api/v1/mcp",
    request_body = JsonRpcRequest,
    responses(
        (status = 200, description = "JSON-RPC response", body = JsonRpcResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal server error")
    ),
    tag = "MCP Protocol"
)]
pub async fn mcp_http_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
    Query(query): Query<McpHttpQuery>,
    Json(request): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    debug!(
        method = %request.method,
        id = ?request.id,
        token_name = %context.token_name,
        "Received MCP HTTP request"
    );

    // Extract optional connection ID from header
    let connection_id = headers
        .get(MCP_CONNECTION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| ConnectionId::new(s.to_string()));

    // Extract team from query or context
    let team = match extract_team(&query, &context) {
        Ok(team) => team,
        Err(e) => {
            error!(error = %e, "Failed to extract team");
            return Json(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: None,
                error: Some(JsonRpcError {
                    code: error_codes::INVALID_REQUEST,
                    message: e,
                    data: None,
                }),
            });
        }
    };

    debug!(team = %team, method = %request.method, "Processing MCP request");

    // Check authorization
    if let Err(e) = check_authorization(&request.method, &context) {
        error!(error = %e, method = %request.method, "Authorization failed");
        return Json(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: None,
            error: Some(JsonRpcError {
                code: error_codes::INVALID_REQUEST,
                message: e,
                data: None,
            }),
        });
    }

    // Get database pool
    let db_pool = match get_db_pool(&state) {
        Ok(pool) => Arc::new(pool),
        Err(e) => {
            error!(error = %e, "Failed to get database pool");
            return Json(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: None,
                error: Some(JsonRpcError {
                    code: error_codes::INTERNAL_ERROR,
                    message: format!("Service unavailable: {}", e),
                    data: None,
                }),
            });
        }
    };

    // Create session for this client (keyed by auth token)
    let session_id = SessionId::from_token(context.token_id.as_str());
    let _ = state.mcp_session_manager.get_or_create_for_team(&session_id, &team);

    // For initialize requests, capture client info
    let is_initialize = request.method == "initialize";
    let init_params = if is_initialize {
        // Parse initialize params to extract client info
        serde_json::from_value::<InitializeParams>(request.params.clone()).ok()
    } else {
        None
    };

    // Create MCP handler and process request
    let mut handler = McpHandler::new(db_pool, team.clone());
    let response = handler.handle_request(request).await;

    // On successful initialize, update session and SSE connection metadata
    if is_initialize && response.error.is_none() {
        if let Some(params) = &init_params {
            // Extract negotiated protocol version from response
            let protocol_version = response
                .result
                .as_ref()
                .and_then(|r| r.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(&params.protocol_version)
                .to_string();

            // Update HTTP session metadata
            state.mcp_session_manager.mark_initialized_with_team(
                &session_id,
                protocol_version.clone(),
                params.client_info.clone(),
                Some(team),
            );

            // Also update SSE connection if present
            if let Some(conn_id) = &connection_id {
                state
                    .mcp_connection_manager
                    .set_client_metadata(conn_id, params.client_info.clone(), protocol_version)
                    .await;
            }
        }
    }

    debug!(
        method = ?response.id,
        has_error = response.error.is_some(),
        "Completed MCP HTTP request"
    );

    Json(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TokenId;

    #[test]
    fn test_extract_team_from_query() {
        let query = McpHttpQuery { team: Some("test-team".to_string()) };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec![],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-team");
    }

    #[test]
    fn test_extract_team_from_scope() {
        let query = McpHttpQuery { team: None };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec!["team:my-team:mcp:read".to_string()],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my-team");
    }

    #[test]
    fn test_extract_team_admin_without_query() {
        let query = McpHttpQuery { team: None };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("admin-token-1"),
            "admin-token".to_string(),
            vec!["admin:all".to_string()],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Admin users must specify team"));
    }

    #[test]
    fn test_extract_team_admin_with_query() {
        let query = McpHttpQuery { team: Some("target-team".to_string()) };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("admin-token-1"),
            "admin-token".to_string(),
            vec!["admin:all".to_string()],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "target-team");
    }

    #[test]
    fn test_extract_team_no_team_found() {
        let query = McpHttpQuery { team: None };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec!["some:other:scope".to_string()],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unable to determine team"));
    }

    #[test]
    fn test_check_authorization_initialize() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec![],
        );

        assert!(check_authorization("initialize", &context).is_ok());
        assert!(check_authorization("initialized", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_tools_list() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec!["mcp:read".to_string()],
        );

        assert!(check_authorization("tools/list", &context).is_ok());
        assert!(check_authorization("resources/list", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_tools_list_forbidden() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec![],
        );

        assert!(check_authorization("tools/list", &context).is_err());
    }

    #[test]
    fn test_check_authorization_tools_call() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec!["mcp:execute".to_string()],
        );

        assert!(check_authorization("tools/call", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_tools_call_forbidden() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec!["mcp:read".to_string()],
        );

        assert!(check_authorization("tools/call", &context).is_err());
    }

    #[test]
    fn test_check_authorization_resources_read() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            vec!["cp:read".to_string()],
        );

        assert!(check_authorization("resources/read", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_admin_all() {
        let context = AuthContext::new(
            TokenId::from_str_unchecked("admin-token-1"),
            "admin-token".to_string(),
            vec!["admin:all".to_string()],
        );

        assert!(check_authorization("tools/list", &context).is_ok());
        assert!(check_authorization("tools/call", &context).is_ok());
        assert!(check_authorization("resources/read", &context).is_ok());
    }
}
