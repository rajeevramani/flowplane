//! MCP Streamable HTTP POST Handler
//!
//! Handles POST requests for JSON-RPC messages.
//! - Initialize: Creates new session with UUID v4 session ID
//! - Subsequent: Validates session and routes to handler

use axum::{
    extract::{Query, State},
    http::{header::HeaderName, HeaderMap, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::api_handler::McpApiHandler;
use crate::mcp::connection::ConnectionId;
use crate::mcp::handler::McpHandler;
use crate::mcp::notifications::NotificationMessage;
use crate::mcp::protocol::{error_codes, InitializeParams, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::security::{generate_secure_session_id, validate_session_id_format};
use crate::mcp::session::SessionId;
use crate::mcp::transport_common::{
    check_method_authorization, determine_response_mode, error_response_json, extract_mcp_headers,
    extract_team, get_db_pool, validate_protocol_version, ResponseMode,
};

use super::McpScope;

/// Query parameters for POST endpoint
#[derive(Debug, Deserialize)]
pub struct PostQuery {
    pub team: Option<String>,
    /// Session ID for SSE-linked requests (legacy compatibility)
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

/// Header name for session ID in responses
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Header name for protocol version in responses
const MCP_PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

/// POST /api/v1/mcp/cp
///
/// Handle Control Plane JSON-RPC requests.
///
/// # Headers
/// - `MCP-Protocol-Version`: Required - must be "2025-11-25"
/// - `MCP-Session-Id`: Required after initialize
/// - `Accept`: `application/json` or `text/event-stream`
///
/// # Response Headers
/// - `MCP-Session-Id`: Assigned on initialize, echoed on subsequent requests
#[utoipa::path(
    post,
    path = "/api/v1/mcp/cp",
    request_body = JsonRpcRequest,
    responses(
        (status = 200, description = "JSON-RPC response", body = JsonRpcResponse),
        (status = 202, description = "Response sent via SSE"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal server error")
    ),
    tag = "MCP Protocol"
)]
pub async fn post_handler_cp(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
    Query(query): Query<PostQuery>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    post_handler(McpScope::ControlPlane, state, context, headers, query, request).await
}

/// POST /api/v1/mcp/api
///
/// Handle Gateway API JSON-RPC requests.
///
/// # Headers
/// - `MCP-Protocol-Version`: Required - must be "2025-11-25"
/// - `MCP-Session-Id`: Required after initialize
/// - `Accept`: `application/json` or `text/event-stream`
///
/// # Response Headers
/// - `MCP-Session-Id`: Assigned on initialize, echoed on subsequent requests
#[utoipa::path(
    post,
    path = "/api/v1/mcp/api",
    request_body = JsonRpcRequest,
    responses(
        (status = 200, description = "JSON-RPC response", body = JsonRpcResponse),
        (status = 202, description = "Response sent via SSE"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal server error")
    ),
    tag = "MCP Protocol"
)]
pub async fn post_handler_api(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
    Query(query): Query<PostQuery>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    post_handler(McpScope::GatewayApi, state, context, headers, query, request).await
}

/// Generic POST handler for JSON-RPC requests
async fn post_handler(
    scope: McpScope,
    state: ApiState,
    context: AuthContext,
    headers: HeaderMap,
    query: PostQuery,
    request: JsonRpcRequest,
) -> impl IntoResponse {
    let scope_config = scope.scope_config();

    debug!(
        method = %request.method,
        id = ?request.id,
        token_name = %context.token_name,
        scope = ?scope,
        "Received MCP POST request"
    );

    // Extract MCP headers
    let mcp_headers = extract_mcp_headers(&headers);

    // Check protocol version (required for 2025-11-25)
    if let Some(version) = &mcp_headers.protocol_version {
        if let Err(e) = validate_protocol_version(version) {
            warn!(
                version = %version,
                "Unsupported MCP protocol version"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(error_response_json(error_codes::INVALID_REQUEST, e.to_string(), request.id)),
            )
                .into_response();
        }
    }

    // Determine if this is an initialize request
    let is_initialize = request.method == "initialize";

    // Session handling
    let (session_id, session_id_str, is_new_session) = if is_initialize {
        // For initialize: create new session with UUID v4
        let new_session_id = generate_secure_session_id();
        let sid = SessionId::from_header(&new_session_id);
        (sid, new_session_id, true)
    } else {
        // For subsequent requests: require valid session ID
        let session_id_str = match &mcp_headers.session_id {
            Some(id) => id.clone(),
            None => {
                // Check query param for backward compatibility
                match &query.session_id {
                    Some(id) => id.clone(),
                    None => {
                        warn!(
                            method = %request.method,
                            "POST request missing MCP-Session-Id header"
                        );
                        return Json(error_response_json(
                            error_codes::INVALID_REQUEST,
                            "MCP-Session-Id header required for non-initialize requests"
                                .to_string(),
                            request.id,
                        ))
                        .into_response();
                    }
                }
            }
        };

        // Validate session ID format
        if let Err(e) = validate_session_id_format(&session_id_str) {
            warn!(
                session_id = %session_id_str,
                error = %e,
                "Invalid session ID format"
            );
            return Json(error_response_json(
                error_codes::INVALID_REQUEST,
                e.to_string(),
                request.id,
            ))
            .into_response();
        }

        let sid = SessionId::from_header(&session_id_str);

        // Verify session exists
        if !state.mcp_session_manager.exists(&sid) {
            warn!(
                session_id = %session_id_str,
                method = %request.method,
                "Session not found or expired"
            );
            return Json(error_response_json(
                error_codes::INVALID_REQUEST,
                "Session not found or expired".to_string(),
                request.id,
            ))
            .into_response();
        }

        (sid, session_id_str, false)
    };

    // Extract team from query or context
    let team = match extract_team(query.team.as_deref(), &context) {
        Ok(team) => team,
        Err(e) => {
            error!(error = %e, "Failed to extract team");
            return Json(error_response_json(error_codes::INVALID_REQUEST, e, request.id))
                .into_response();
        }
    };

    // For new sessions, create the session in the manager
    if is_new_session {
        let _ = state.mcp_session_manager.get_or_create_for_team(&session_id, &team);
    } else {
        // Validate session ownership for existing sessions
        if let Err(_e) = state.mcp_session_manager.validate_session_ownership(&session_id, &team) {
            warn!(
                session_id = %session_id_str,
                team = %team,
                "Session ownership validation failed"
            );
            // Return generic error to avoid leaking info
            return Json(error_response_json(
                error_codes::INVALID_REQUEST,
                "Session not found or expired".to_string(),
                request.id,
            ))
            .into_response();
        }
    }

    debug!(team = %team, method = %request.method, "Processing MCP request");

    // Check authorization
    if let Err(e) = check_method_authorization(&request.method, &context, scope_config) {
        error!(error = %e, method = %request.method, "Authorization failed");
        return Json(error_response_json(error_codes::INVALID_REQUEST, e, request.id))
            .into_response();
    }

    // Get database pool
    let db_pool = match get_db_pool(&state) {
        Ok(pool) => Arc::new(pool),
        Err(e) => {
            error!(error = %e, "Failed to get database pool");
            return Json(error_response_json(
                error_codes::INTERNAL_ERROR,
                format!("Service unavailable: {}", e),
                request.id,
            ))
            .into_response();
        }
    };

    // Parse initialize params if applicable
    let init_params = if is_initialize {
        serde_json::from_value::<InitializeParams>(request.params.clone()).ok()
    } else {
        None
    };

    // Route to appropriate handler
    let response = match scope {
        McpScope::ControlPlane => {
            let scopes: Vec<String> = context.scopes().map(|s| s.to_string()).collect();
            let mut handler =
                McpHandler::with_xds_state(db_pool, state.xds_state.clone(), team.clone(), scopes);
            handler.handle_request(request.clone()).await
        }
        McpScope::GatewayApi => {
            let mut handler = McpApiHandler::new(db_pool, team.clone());
            handler.handle_request(request.clone()).await
        }
    };

    // On successful initialize, mark session as initialized
    if is_initialize && response.error.is_none() {
        if let Some(params) = &init_params {
            let protocol_version = response
                .result
                .as_ref()
                .and_then(|r| r.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(&params.protocol_version)
                .to_string();

            state.mcp_session_manager.mark_initialized_with_team(
                &session_id,
                protocol_version,
                params.client_info.clone(),
                Some(team.clone()),
            );

            info!(
                session_id = %session_id_str,
                client = %params.client_info.name,
                team = %team,
                scope = ?scope,
                "Session initialized"
            );
        }
    }

    // Determine response mode
    let accept_header = headers.get("accept").and_then(|v| v.to_str().ok());
    let response_mode = determine_response_mode(accept_header);

    // Check if we have an SSE connection to send through
    let connection_id = state.mcp_session_manager.get_connection_id(&session_id);

    debug!(
        method = ?response.id,
        has_error = response.error.is_some(),
        response_mode = ?response_mode,
        has_connection = connection_id.is_some(),
        "Completed MCP POST request"
    );

    // Handle response based on mode
    match response_mode {
        ResponseMode::Sse if connection_id.is_some() => {
            // Send response via SSE and return 202 Accepted
            let conn_id_str = connection_id.unwrap();
            let conn_id = ConnectionId::new(conn_id_str.clone());

            let message = NotificationMessage::message(response);
            if let Err(e) = state.mcp_connection_manager.send_to_connection(&conn_id, message).await
            {
                error!(
                    error = %e,
                    connection_id = %conn_id_str,
                    "Failed to send response via SSE"
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(error_response_json(
                        error_codes::INTERNAL_ERROR,
                        "Failed to send response via SSE".to_string(),
                        request.id,
                    )),
                )
                    .into_response();
            }

            info!(connection_id = %conn_id_str, "Response sent via SSE");

            // Return 202 Accepted with session ID header
            let session_header = HeaderName::from_static(MCP_SESSION_ID_HEADER);
            let version_header = HeaderName::from_static(MCP_PROTOCOL_VERSION_HEADER);
            (
                StatusCode::ACCEPTED,
                [(session_header, session_id_str), (version_header, "2025-11-25".to_string())],
            )
                .into_response()
        }
        _ => {
            // Return JSON response with session ID header
            let session_header = HeaderName::from_static(MCP_SESSION_ID_HEADER);
            let version_header = HeaderName::from_static(MCP_PROTOCOL_VERSION_HEADER);
            (
                [(session_header, session_id_str), (version_header, "2025-11-25".to_string())],
                Json(response),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_config() {
        let cp_config = McpScope::ControlPlane.scope_config();
        assert_eq!(cp_config.read_scope, "mcp:read");
        assert_eq!(cp_config.execute_scope, "mcp:execute");

        let api_config = McpScope::GatewayApi.scope_config();
        assert_eq!(api_config.read_scope, "api:read");
        assert_eq!(api_config.execute_scope, "api:execute");
    }
}
