//! MCP API Tools HTTP Transport
//!
//! Provides HTTP endpoint for API tools (gateway_api category).
//! Separate from CP tools endpoint to enforce different authorization scopes.

use axum::{extract::Query, extract::State, http::HeaderMap, Extension, Json};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, error};

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::api_handler::McpApiHandler;
use crate::mcp::connection::ConnectionId;
use crate::mcp::protocol::{
    error_codes, InitializeParams, JsonRpcError, JsonRpcRequest, JsonRpcResponse,
};
use crate::mcp::session::SessionId;
use crate::storage::DbPool;

const MCP_CONNECTION_ID_HEADER: &str = "mcp-connection-id";

#[derive(Debug, Deserialize)]
pub struct McpApiHttpQuery {
    pub team: Option<String>,
}

/// Extract team name from query parameters or auth context
fn extract_team(query: &McpApiHttpQuery, context: &AuthContext) -> Result<String, String> {
    if let Some(team) = &query.team {
        debug!(team = %team, "Team extracted from query parameter");
        return Ok(team.clone());
    }

    if context.has_scope("admin:all") {
        error!("Admin user must provide team via query parameter");
        return Err("Admin users must specify team via query parameter".to_string());
    }

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
        "initialize"
        | "initialized"
        | "ping"
        | "notifications/initialized"
        | "notifications/cancelled" => Ok(()),

        // Read operations - require api:read
        "tools/list" => {
            if context.has_scope("api:read") || context.has_scope("admin:all") {
                Ok(())
            } else {
                Err(format!("Missing required scope 'api:read' for method '{}'", method))
            }
        }

        // Execute operations - require api:execute
        "tools/call" => {
            if context.has_scope("api:execute") || context.has_scope("admin:all") {
                Ok(())
            } else {
                Err(format!("Missing required scope 'api:execute' for method '{}'", method))
            }
        }

        _ => {
            // Unknown method - let handler deal with it
            Ok(())
        }
    }
}

fn get_db_pool(state: &ApiState) -> Result<DbPool, String> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| "Database not available".to_string())?;

    Ok(cluster_repo.pool().clone())
}

/// POST /api/v1/mcp/api
///
/// HTTP endpoint for API tools. Accepts JSON-RPC 2.0 requests and returns responses.
///
/// # Authentication
/// Requires a valid bearer token with appropriate scopes.
///
/// # Authorization
/// - `initialize`, `initialized`, `ping` - No scope required
/// - `tools/list` - Require `api:read`
/// - `tools/call` - Require `api:execute`
#[utoipa::path(
    post,
    path = "/api/v1/mcp/api",
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
pub async fn mcp_api_http_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
    Query(query): Query<McpApiHttpQuery>,
    Json(request): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    debug!(
        method = %request.method,
        id = ?request.id,
        token_name = %context.token_name,
        "Received MCP API HTTP request"
    );

    let connection_id = headers
        .get(MCP_CONNECTION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| ConnectionId::new(s.to_string()));

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

    debug!(team = %team, method = %request.method, "Processing MCP API request");

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

    let has_valid_sse_connection =
        connection_id.as_ref().map(|id| state.mcp_connection_manager.exists(id)).unwrap_or(false);

    let session_id = if !has_valid_sse_connection {
        let sid = SessionId::from_token(context.token_id.as_str());
        let _ = state.mcp_session_manager.get_or_create_for_team(&sid, &team);
        Some(sid)
    } else {
        debug!(
            connection_id = ?connection_id,
            "Skipping HTTP session creation - using SSE connection"
        );
        None
    };

    let is_initialize = request.method == "initialize";
    let init_params = if is_initialize {
        serde_json::from_value::<InitializeParams>(request.params.clone()).ok()
    } else {
        None
    };

    let mut handler = McpApiHandler::new(db_pool, team.clone());
    let response = handler.handle_request(request).await;

    if is_initialize && response.error.is_none() {
        if let Some(params) = &init_params {
            let protocol_version = response
                .result
                .as_ref()
                .and_then(|r| r.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(&params.protocol_version)
                .to_string();

            if let Some(conn_id) = &connection_id {
                if has_valid_sse_connection {
                    state
                        .mcp_connection_manager
                        .set_client_metadata(
                            conn_id,
                            params.client_info.clone(),
                            protocol_version.clone(),
                        )
                        .await;
                }
            }

            if let Some(sid) = &session_id {
                state.mcp_session_manager.mark_initialized_with_team(
                    sid,
                    protocol_version,
                    params.client_info.clone(),
                    Some(team),
                );
            }
        }
    }

    debug!(
        method = ?response.id,
        has_error = response.error.is_some(),
        "Completed MCP API HTTP request"
    );

    Json(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TokenId;

    fn create_test_context_with_scopes(scopes: Vec<String>) -> AuthContext {
        AuthContext::new(TokenId::from_str_unchecked("token-1"), "test-token".to_string(), scopes)
    }

    #[test]
    fn test_check_authorization_tools_list() {
        let context = create_test_context_with_scopes(vec!["api:read".to_string()]);
        assert!(check_authorization("tools/list", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_tools_list_forbidden() {
        let context = create_test_context_with_scopes(vec![]);
        assert!(check_authorization("tools/list", &context).is_err());
    }

    #[test]
    fn test_check_authorization_tools_call() {
        let context = create_test_context_with_scopes(vec!["api:execute".to_string()]);
        assert!(check_authorization("tools/call", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_tools_call_forbidden() {
        let context = create_test_context_with_scopes(vec!["api:read".to_string()]);
        assert!(check_authorization("tools/call", &context).is_err());
    }

    #[test]
    fn test_check_authorization_admin_all() {
        let context = create_test_context_with_scopes(vec!["admin:all".to_string()]);

        assert!(check_authorization("tools/list", &context).is_ok());
        assert!(check_authorization("tools/call", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_ping_no_scope() {
        let context = create_test_context_with_scopes(vec![]);
        assert!(check_authorization("ping", &context).is_ok());
    }

    #[test]
    fn test_check_authorization_initialize_no_scope() {
        let context = create_test_context_with_scopes(vec![]);
        assert!(check_authorization("initialize", &context).is_ok());
    }

    #[test]
    fn test_extract_team_from_query() {
        let context = create_test_context_with_scopes(vec![]);
        let query = McpApiHttpQuery { team: Some("my-team".to_string()) };

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my-team");
    }

    #[test]
    fn test_extract_team_from_scope() {
        let context = create_test_context_with_scopes(vec!["team:my-team:api:read".to_string()]);
        let query = McpApiHttpQuery { team: None };

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my-team");
    }

    #[test]
    fn test_extract_team_admin_requires_query() {
        let context = create_test_context_with_scopes(vec!["admin:all".to_string()]);
        let query = McpApiHttpQuery { team: None };

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Admin users must specify team"));
    }

    #[test]
    fn test_extract_team_no_team_no_scope() {
        let context = create_test_context_with_scopes(vec!["api:read".to_string()]);
        let query = McpApiHttpQuery { team: None };

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unable to determine team"));
    }
}
