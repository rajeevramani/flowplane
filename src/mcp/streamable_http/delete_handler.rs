//! MCP Streamable HTTP DELETE Handler
//!
//! Handles DELETE requests to terminate MCP sessions.
//! Returns 200 OK if session found and removed, 404 Not Found otherwise.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Extension,
};
use tracing::{debug, warn};

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::connection::ConnectionId;
use crate::mcp::session::SessionId;
use crate::mcp::transport_common::extract_mcp_headers;

use super::McpScope;

/// DELETE /api/v1/mcp/cp
///
/// Terminate a Control Plane MCP session.
///
/// # Headers
/// - `MCP-Session-Id`: Required - session ID to terminate
///
/// # Returns
/// - 200 OK: Session terminated successfully
/// - 400 Bad Request: Missing or invalid session ID header
/// - 404 Not Found: Session not found
#[utoipa::path(
    delete,
    path = "/api/v1/mcp/cp",
    responses(
        (status = 200, description = "Session terminated"),
        (status = 400, description = "Invalid session ID"),
        (status = 404, description = "Session not found")
    ),
    tag = "MCP Protocol"
)]
pub async fn delete_handler_cp(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
) -> impl IntoResponse {
    delete_handler(McpScope::ControlPlane, state, context, headers).await
}

/// DELETE /api/v1/mcp/api
///
/// Terminate a Gateway API MCP session.
///
/// # Headers
/// - `MCP-Session-Id`: Required - session ID to terminate
///
/// # Returns
/// - 200 OK: Session terminated successfully
/// - 400 Bad Request: Missing or invalid session ID header
/// - 404 Not Found: Session not found
#[utoipa::path(
    delete,
    path = "/api/v1/mcp/api",
    responses(
        (status = 200, description = "Session terminated"),
        (status = 400, description = "Invalid session ID"),
        (status = 404, description = "Session not found")
    ),
    tag = "MCP Protocol"
)]
pub async fn delete_handler_api(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
) -> impl IntoResponse {
    delete_handler(McpScope::GatewayApi, state, context, headers).await
}

/// Generic DELETE handler for session termination
async fn delete_handler(
    scope: McpScope,
    state: ApiState,
    context: AuthContext,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Extract MCP headers
    let mcp_headers = extract_mcp_headers(&headers);

    // Session ID is required for DELETE
    let session_id_str = match &mcp_headers.session_id {
        Some(id) => id,
        None => {
            warn!(
                scope = ?scope,
                token = %context.token_name,
                "DELETE request missing MCP-Session-Id header"
            );
            return StatusCode::BAD_REQUEST;
        }
    };

    // Validate session ID format
    if let Err(e) = crate::mcp::security::validate_session_id_format(session_id_str) {
        warn!(
            session_id = %session_id_str,
            error = %e,
            "Invalid session ID format in DELETE request"
        );
        return StatusCode::BAD_REQUEST;
    }

    // Create session ID from header value
    let session_id = SessionId::from_header(session_id_str);

    // Check if session exists
    if !state.mcp_session_manager.exists(&session_id) {
        debug!(
            session_id = %session_id_str,
            scope = ?scope,
            "DELETE request for non-existent session"
        );
        return StatusCode::NOT_FOUND;
    }

    // Get connection ID before removing session (if any)
    let connection_id = state.mcp_session_manager.get_connection_id(&session_id);

    // Remove session
    let removed = state.mcp_session_manager.remove(&session_id);
    if !removed {
        debug!(
            session_id = %session_id_str,
            scope = ?scope,
            "Session already removed during DELETE"
        );
        return StatusCode::NOT_FOUND;
    }

    // Close associated SSE connection if exists
    if let Some(conn_id_str) = connection_id {
        let conn_id = ConnectionId::new(conn_id_str.clone());
        state.mcp_connection_manager.unregister(&conn_id);
        debug!(
            session_id = %session_id_str,
            connection_id = %conn_id_str,
            "Closed associated SSE connection"
        );
    }

    debug!(
        session_id = %session_id_str,
        scope = ?scope,
        token = %context.token_name,
        "Session terminated via DELETE"
    );

    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_endpoint_paths() {
        assert_eq!(McpScope::ControlPlane.endpoint_path(), "/api/v1/mcp/cp");
        assert_eq!(McpScope::GatewayApi.endpoint_path(), "/api/v1/mcp/api");
    }
}
