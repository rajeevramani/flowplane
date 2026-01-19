//! MCP SSE (Server-Sent Events) Transport
//!
//! Provides HTTP+SSE streaming for MCP protocol, enabling real-time progress updates,
//! log streaming, and response delivery.

use axum::{
    extract::{Query, State},
    http::header::HeaderName,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Extension,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tracing::{error, info};

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::connection::{ConnectionId, ConnectionManager};
use crate::mcp::error::McpError;
use crate::mcp::notifications::NotificationMessage;
use crate::mcp::protocol::{error_codes, JsonRpcError, JsonRpcResponse};
use crate::mcp::SharedConnectionManager;

/// Stream wrapper that cleans up the connection when dropped
///
/// This ensures that when the SSE stream ends (client disconnects),
/// the connection is properly unregistered from the connection manager.
struct CleanupStream<S> {
    inner: S,
    connection_manager: SharedConnectionManager,
    connection_id: ConnectionId,
}

impl<S> CleanupStream<S> {
    fn new(
        inner: S,
        connection_manager: SharedConnectionManager,
        connection_id: ConnectionId,
    ) -> Self {
        Self { inner, connection_manager, connection_id }
    }
}

impl<S: Stream + Unpin> Stream for CleanupStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl<S> Drop for CleanupStream<S> {
    fn drop(&mut self) {
        info!(connection_id = %self.connection_id, "SSE connection closed, cleaning up");
        self.connection_manager.unregister(&self.connection_id);
    }
}

/// Query parameters for SSE endpoint
#[derive(Debug, Deserialize)]
pub struct SseQuery {
    pub team: Option<String>,
}

/// SSE heartbeat interval in seconds
/// Shorter interval allows faster detection of client disconnects
const HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// Extract team name from query or auth context
fn extract_team(query: &SseQuery, context: &AuthContext) -> Result<String, String> {
    // Check query parameter first
    if let Some(team) = &query.team {
        return Ok(team.clone());
    }

    // Check if admin (must provide team)
    if context.has_scope("admin:all") {
        return Err("Admin users must specify team via query parameter".to_string());
    }

    // Extract from scopes (pattern: team:{name}:*)
    for scope in context.scopes() {
        if let Some(team_part) = scope.strip_prefix("team:") {
            if let Some(team_name) = team_part.split(':').next() {
                return Ok(team_name.to_string());
            }
        }
    }

    Err("Unable to determine team. Please provide team via query parameter".to_string())
}

/// Check if context has required scope for SSE
fn check_sse_authorization(context: &AuthContext) -> Result<(), String> {
    if context.has_scope("mcp:read") || context.has_scope("admin:all") {
        Ok(())
    } else {
        Err("Missing required scope 'mcp:read' for SSE streaming".to_string())
    }
}

/// Format a notification message as an SSE event
fn format_sse_event(message: &NotificationMessage, event_id: u64) -> Result<Event, Infallible> {
    let event_type = message.event_type();
    let data = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());

    Ok(Event::default().id(event_id.to_string()).event(event_type).data(data))
}

/// Custom header name for MCP connection ID
const MCP_CONNECTION_ID_HEADER: &str = "mcp-connection-id";

/// GET /api/v1/mcp/sse
///
/// Establishes an SSE connection for streaming MCP notifications.
///
/// # Authentication
/// Requires a valid bearer token with `mcp:read` scope.
///
/// # Query Parameters
/// - `team`: Optional team name. Required for admin users.
///
/// # Response Headers
/// - `Mcp-Connection-Id`: The unique identifier for this SSE connection. Use this when
///   sending requests to `/api/v1/mcp` to associate them with this SSE session.
///
/// # Events
/// - `message`: JSON-RPC response messages
/// - `progress`: Progress notifications for long-running operations
/// - `log`: Log messages from the server
/// - `ping`: Heartbeat events (every 30 seconds)
#[utoipa::path(
    get,
    path = "/api/v1/mcp/sse",
    responses(
        (status = 200, description = "SSE stream established"),
        (status = 400, description = "Invalid request (missing team)"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 429, description = "Connection limit exceeded")
    ),
    tag = "MCP Protocol"
)]
pub async fn mcp_sse_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<SseQuery>,
) -> Result<impl IntoResponse, axum::response::Response> {
    let connection_manager = state.mcp_connection_manager.clone();
    // Extract team
    let team = match extract_team(&query, &context) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "Failed to extract team for SSE");
            return Err(error_response(error_codes::INVALID_REQUEST, e));
        }
    };

    // Check authorization
    if let Err(e) = check_sse_authorization(&context) {
        error!(error = %e, "SSE authorization failed");
        return Err(error_response(error_codes::INVALID_REQUEST, e));
    }

    // Register connection
    let (connection_id, receiver) = match connection_manager.register(team.clone()) {
        Ok(result) => result,
        Err(e) => {
            error!(error = %e, team = %team, "Failed to register SSE connection");
            let (code, msg) = match e {
                McpError::ConnectionLimitExceeded { team, limit } => (
                    429, // Too Many Requests
                    format!("Connection limit ({}) exceeded for team: {}", limit, team),
                ),
                _ => (error_codes::INTERNAL_ERROR, e.to_string()),
            };
            return Err(error_response(code, msg));
        }
    };

    info!(
        connection_id = %connection_id,
        team = %team,
        token_name = %context.token_name,
        "SSE connection established"
    );

    // Store connection ID for header before moving
    let connection_id_str = connection_id.to_string();

    // Create stream from receiver
    let receiver_stream = ReceiverStream::new(receiver);

    // Add event ID counter
    let mut event_id = 0u64;

    // Map messages to SSE events
    let event_stream = receiver_stream.map(move |message| {
        event_id += 1;
        format_sse_event(&message, event_id)
    });

    // Wrap with cleanup stream to unregister connection when client disconnects
    let cleanup_stream = CleanupStream::new(event_stream, connection_manager, connection_id);

    // Create SSE response with keepalive
    let sse = Sse::new(cleanup_stream).keep_alive(
        KeepAlive::new().interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS)).text("ping"),
    );

    // Return SSE response with Mcp-Connection-Id header
    let header_name = HeaderName::from_static(MCP_CONNECTION_ID_HEADER);
    Ok(([(header_name, connection_id_str)], sse))
}

/// Create an error response for SSE endpoint
fn error_response(code: i32, message: String) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;

    let status = match code {
        429 => StatusCode::TOO_MANY_REQUESTS,
        error_codes::INVALID_REQUEST => StatusCode::BAD_REQUEST,
        error_codes::INTERNAL_ERROR => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };

    let body = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: Some(JsonRpcError { code, message, data: None }),
    };

    (status, Json(body)).into_response()
}

/// Helper to broadcast a message to a team's connections
pub async fn broadcast_notification(
    connection_manager: &ConnectionManager,
    team: &str,
    message: NotificationMessage,
) {
    connection_manager.broadcast_to_team(team, message).await;
}

/// Helper to send a message to a specific connection
pub async fn send_notification(
    connection_manager: &ConnectionManager,
    connection_id: &ConnectionId,
    message: NotificationMessage,
) -> Result<(), McpError> {
    connection_manager.send_to_connection(connection_id, message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TokenId;

    #[test]
    fn test_extract_team_from_query() {
        let query = SseQuery { team: Some("test-team".to_string()) };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("token-1"),
            "test-token".to_string(),
            vec![],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-team");
    }

    #[test]
    fn test_extract_team_from_scope() {
        let query = SseQuery { team: None };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("token-1"),
            "test-token".to_string(),
            vec!["team:my-team:mcp:read".to_string()],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my-team");
    }

    #[test]
    fn test_extract_team_admin_requires_query() {
        let query = SseQuery { team: None };
        let context = AuthContext::new(
            TokenId::from_str_unchecked("admin-1"),
            "admin-token".to_string(),
            vec!["admin:all".to_string()],
        );

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Admin users must specify team"));
    }

    #[test]
    fn test_check_sse_authorization() {
        let context_with_scope = AuthContext::new(
            TokenId::from_str_unchecked("token-1"),
            "test-token".to_string(),
            vec!["mcp:read".to_string()],
        );
        assert!(check_sse_authorization(&context_with_scope).is_ok());

        let admin_context = AuthContext::new(
            TokenId::from_str_unchecked("admin-1"),
            "admin-token".to_string(),
            vec!["admin:all".to_string()],
        );
        assert!(check_sse_authorization(&admin_context).is_ok());

        let no_scope_context = AuthContext::new(
            TokenId::from_str_unchecked("token-1"),
            "test-token".to_string(),
            vec![],
        );
        assert!(check_sse_authorization(&no_scope_context).is_err());
    }

    #[test]
    fn test_format_sse_event() {
        let message = NotificationMessage::ping();
        let result = format_sse_event(&message, 1);
        assert!(result.is_ok());
    }
}
