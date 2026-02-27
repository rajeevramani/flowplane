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
use crate::mcp::protocol::{
    error_codes, InitializeParams, JsonRpcRequest, JsonRpcResponse, PROTOCOL_VERSION,
};
use crate::mcp::security::{generate_secure_session_id, validate_session_id_format};
use crate::mcp::session::SessionId;
use crate::mcp::transport_common::{
    determine_response_mode, error_response_json, extract_mcp_headers, extract_teams, get_db_pool,
    validate_protocol_version, ResponseMode,
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
/// - `MCP-Protocol-Version`: Optional - supported versions: 2025-11-25, 2025-03-26
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
/// - `MCP-Protocol-Version`: Optional - supported versions: 2025-11-25, 2025-03-26
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

    // Extract all authorized teams from token scopes.
    // ?team= query param is accepted but ignored — multi-team sessions eliminate
    // the need to specify a team at connection time.
    let teams = extract_teams(&context);

    // For new sessions, create the session bound to the token's authorized teams.
    // For existing sessions, the session ID is the security boundary — any valid
    // token that presents a known session ID may use it.
    if is_new_session {
        let _ = state.mcp_session_manager.get_or_create_for_teams(&session_id, &teams);
    }

    debug!(teams = ?teams, method = %request.method, "Processing MCP request");

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
            let mut handler = McpHandler::with_xds_state(
                db_pool,
                state.xds_state.clone(),
                teams.clone(),
                context.clone(),
            );
            handler.handle_request(request.clone()).await
        }
        McpScope::GatewayApi => {
            let mut handler = McpApiHandler::new(db_pool, teams.clone(), context.clone());
            handler.handle_request(request.clone()).await
        }
    };

    // Detect if the original request was a notification (no id per JSON-RPC 2.0)
    let is_notification = request.id.is_none();

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

            state.mcp_session_manager.mark_initialized_with_teams(
                &session_id,
                protocol_version,
                params.client_info.clone(),
                teams.clone(),
            );

            info!(
                session_id = %session_id_str,
                client = %params.client_info.name,
                teams = ?teams,
                scope = ?scope,
                "Session initialized"
            );
        }
    }

    // JSON-RPC notifications (no id) get 202 Accepted with no body per spec
    if is_notification {
        let session_header = HeaderName::from_static(MCP_SESSION_ID_HEADER);
        let version_header = HeaderName::from_static(MCP_PROTOCOL_VERSION_HEADER);
        let negotiated_version = state
            .mcp_session_manager
            .get_protocol_version(&session_id)
            .unwrap_or_else(|| PROTOCOL_VERSION.to_string());
        return (
            StatusCode::ACCEPTED,
            [(session_header, session_id_str), (version_header, negotiated_version)],
        )
            .into_response();
    }

    // Compute negotiated protocol version for response headers
    let negotiated_version = if is_initialize {
        response
            .result
            .as_ref()
            .and_then(|r| r.get("protocolVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or(PROTOCOL_VERSION)
            .to_string()
    } else {
        state
            .mcp_session_manager
            .get_protocol_version(&session_id)
            .unwrap_or_else(|| PROTOCOL_VERSION.to_string())
    };

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
                [(session_header, session_id_str), (version_header, negotiated_version.clone())],
            )
                .into_response()
        }
        _ => {
            // Return JSON response with session ID header
            let session_header = HeaderName::from_static(MCP_SESSION_ID_HEADER);
            let version_header = HeaderName::from_static(MCP_PROTOCOL_VERSION_HEADER);
            (
                [(session_header, session_id_str), (version_header, negotiated_version)],
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
    fn test_notification_detection() {
        // Notifications have no id
        let notification = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: serde_json::Value::Null,
        };
        assert!(notification.id.is_none());

        // Requests have an id
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(crate::mcp::protocol::JsonRpcId::Number(1)),
            method: "tools/list".to_string(),
            params: serde_json::Value::Null,
        };
        assert!(request.id.is_some());
    }
}
