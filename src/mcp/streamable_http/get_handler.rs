//! MCP Streamable HTTP GET Handler
//!
//! Handles GET requests to open SSE streams for server notifications.
//! Requires an existing session (MCP-Session-Id header).
//!
//! # Resumability (MCP 2025-11-25)
//!
//! Supports SSE resumption via `Last-Event-ID` header. When a client
//! reconnects with this header, buffered messages are replayed.

use axum::{
    extract::{Query, State},
    http::{header::HeaderName, HeaderMap},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Extension,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tracing::{error, info, warn};

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::connection::ConnectionId;
use crate::mcp::error::McpError;
use crate::mcp::notifications::NotificationMessage;
use crate::mcp::protocol::{error_codes, JsonRpcError, JsonRpcResponse};
use crate::mcp::session::SessionId;
use crate::mcp::transport_common::{
    check_method_authorization, extract_mcp_headers, extract_team, get_db_pool,
    validate_team_org_membership,
};
use crate::mcp::SharedConnectionManager;

use super::McpScope;

/// Event ID for SSE resumability (MCP 2025-11-25)
///
/// Format: `{connection_id}:{sequence}`
///
/// Used in SSE `id:` field and parsed from `Last-Event-ID` header
/// for message replay on client reconnection.
///
/// # Example
///
/// ```ignore
/// let event_id = EventId::new(connection_id, 42);
/// assert_eq!(event_id.to_string(), "conn-team-abc-123:42");
///
/// let parsed = EventId::from_last_event_id("conn-team-abc-123:42")?;
/// assert_eq!(parsed.sequence(), 42);
/// ```
#[derive(Debug, Clone)]
pub struct EventId {
    connection_id: ConnectionId,
    sequence: u64,
}

impl EventId {
    /// Create a new event ID
    pub fn new(connection_id: ConnectionId, sequence: u64) -> Self {
        Self { connection_id, sequence }
    }

    /// Parse from Last-Event-ID header value
    ///
    /// Expected format: `{connection_id}:{sequence}`
    ///
    /// # Errors
    ///
    /// Returns `McpError::InvalidEventId` if:
    /// - Format doesn't contain exactly one colon
    /// - Sequence number is not a valid u64
    pub fn from_last_event_id(header: &str) -> Result<Self, McpError> {
        // Find the last colon (connection ID may contain colons in future formats)
        let last_colon = header.rfind(':').ok_or_else(|| {
            McpError::InvalidEventId(
                "Event ID must be in format '{connection_id}:{sequence}'".to_string(),
            )
        })?;

        let conn_id_str = &header[..last_colon];
        let seq_str = &header[last_colon + 1..];

        if conn_id_str.is_empty() {
            return Err(McpError::InvalidEventId("Connection ID cannot be empty".to_string()));
        }

        let sequence = seq_str.parse::<u64>().map_err(|_| {
            McpError::InvalidEventId(format!("Invalid sequence number: {}", seq_str))
        })?;

        Ok(Self { connection_id: ConnectionId::new(conn_id_str.to_string()), sequence })
    }

    /// Get the connection ID component
    pub fn connection_id(&self) -> &ConnectionId {
        &self.connection_id
    }

    /// Get the sequence number component
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.connection_id, self.sequence)
    }
}

/// Query parameters for SSE endpoint
#[derive(Debug, Deserialize)]
pub struct SseQuery {
    pub team: Option<String>,
}

/// SSE heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// Header name for session ID in responses
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Stream wrapper that cleans up the connection when dropped
///
/// This ensures that when the SSE stream ends (client disconnects),
/// the connection is properly unregistered from the connection manager.
struct CleanupStream<S> {
    inner: S,
    connection_manager: SharedConnectionManager,
    connection_id: crate::mcp::connection::ConnectionId,
    session_id: Option<SessionId>,
    session_manager: crate::mcp::SharedSessionManager,
}

impl<S> CleanupStream<S> {
    fn new(
        inner: S,
        connection_manager: SharedConnectionManager,
        connection_id: crate::mcp::connection::ConnectionId,
        session_id: Option<SessionId>,
        session_manager: crate::mcp::SharedSessionManager,
    ) -> Self {
        Self { inner, connection_manager, connection_id, session_id, session_manager }
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

        // Unregister connection
        self.connection_manager.unregister(&self.connection_id);

        // Detach from session if linked
        if let Some(session_id) = &self.session_id {
            self.session_manager.detach_sse_connection(session_id);
        }
    }
}

/// Format a notification message as an SSE event with EventId
fn format_sse_event_with_id(
    message: &NotificationMessage,
    event_id: &EventId,
) -> Result<Event, Infallible> {
    let event_type = message.event_type();

    // Serialize only the inner data, not the wrapper enum
    let data = match message {
        NotificationMessage::Message { data } => {
            serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
        }
        NotificationMessage::Progress { data } => {
            serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
        }
        NotificationMessage::Log { data } => {
            serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
        }
        NotificationMessage::Ping { timestamp } => {
            serde_json::json!({"timestamp": timestamp}).to_string()
        }
    };

    Ok(Event::default().id(event_id.to_string()).event(event_type).data(data))
}

/// Extract Last-Event-ID header for SSE resumption
fn extract_last_event_id(headers: &HeaderMap) -> Option<String> {
    headers.get("last-event-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

/// Create an error response for SSE endpoint
fn error_response(code: i32, message: String) -> axum::response::Response {
    use axum::http::StatusCode;
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

/// GET /api/v1/mcp/cp
///
/// Open SSE stream for Control Plane notifications.
///
/// # Headers
/// - `MCP-Session-Id`: Required - existing session ID
///
/// # Response Headers
/// - `MCP-Session-Id`: Echo of session ID
///
/// # Events
/// - `message`: JSON-RPC response messages
/// - `progress`: Progress notifications for long-running operations
/// - `log`: Log messages from the server
/// - `ping`: Heartbeat events
#[utoipa::path(
    get,
    path = "/api/v1/mcp/cp",
    responses(
        (status = 200, description = "SSE stream established"),
        (status = 400, description = "Invalid or missing session"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 429, description = "Connection limit exceeded")
    ),
    tag = "MCP Protocol"
)]
pub async fn get_handler_cp(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
    Query(query): Query<SseQuery>,
) -> Result<impl IntoResponse, axum::response::Response> {
    get_handler(McpScope::ControlPlane, state, context, headers, query).await
}

/// GET /api/v1/mcp/api
///
/// Open SSE stream for Gateway API notifications.
///
/// # Headers
/// - `MCP-Session-Id`: Required - existing session ID
///
/// # Response Headers
/// - `MCP-Session-Id`: Echo of session ID
///
/// # Events
/// - `message`: JSON-RPC response messages
/// - `progress`: Progress notifications for long-running operations
/// - `log`: Log messages from the server
/// - `ping`: Heartbeat events
#[utoipa::path(
    get,
    path = "/api/v1/mcp/api",
    responses(
        (status = 200, description = "SSE stream established"),
        (status = 400, description = "Invalid or missing session"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 429, description = "Connection limit exceeded")
    ),
    tag = "MCP Protocol"
)]
pub async fn get_handler_api(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    headers: HeaderMap,
    Query(query): Query<SseQuery>,
) -> Result<impl IntoResponse, axum::response::Response> {
    get_handler(McpScope::GatewayApi, state, context, headers, query).await
}

/// Generic GET handler for SSE streaming
async fn get_handler(
    scope: McpScope,
    state: ApiState,
    context: AuthContext,
    headers: HeaderMap,
    query: SseQuery,
) -> Result<impl IntoResponse, axum::response::Response> {
    let connection_manager = state.mcp_connection_manager.clone();
    let session_manager = state.mcp_session_manager.clone();
    let scope_config = scope.scope_config();

    // Extract MCP headers
    let mcp_headers = extract_mcp_headers(&headers);

    // Session ID is required for GET
    let session_id_str = match &mcp_headers.session_id {
        Some(id) => id.clone(),
        None => {
            warn!(
                scope = ?scope,
                token = %context.token_name,
                "GET request missing MCP-Session-Id header"
            );
            return Err(error_response(
                error_codes::INVALID_REQUEST,
                "MCP-Session-Id header required for SSE stream".to_string(),
            ));
        }
    };

    // Validate session ID format
    if let Err(e) = crate::mcp::security::validate_session_id_format(&session_id_str) {
        warn!(
            session_id = %session_id_str,
            error = %e,
            "Invalid session ID format in GET request"
        );
        return Err(error_response(error_codes::INVALID_REQUEST, e.to_string()));
    }

    // Create session ID from header
    let session_id = SessionId::from_header(&session_id_str);

    // Verify session exists
    if !session_manager.exists(&session_id) {
        warn!(
            session_id = %session_id_str,
            scope = ?scope,
            "GET request for non-existent session"
        );
        return Err(error_response(
            error_codes::INVALID_REQUEST,
            "Session not found or expired".to_string(),
        ));
    }

    // Extract team
    let team = match extract_team(query.team.as_deref(), &context) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "Failed to extract team for SSE");
            return Err(error_response(error_codes::INVALID_REQUEST, e));
        }
    };

    // Validate team belongs to caller's org (prevents cross-org team access via query param)
    if let Some(ref org_id) = context.org_id {
        if let Ok(db_pool) = get_db_pool(&state) {
            if let Err(e) = validate_team_org_membership(&team, org_id, &db_pool).await {
                error!(error = %e, team = %team, "Team org membership validation failed");
                return Err(error_response(error_codes::INVALID_REQUEST, e));
            }
        }
    }

    // Validate session ownership
    if let Err(e) = session_manager.validate_session_ownership(&session_id, &team) {
        warn!(
            session_id = %session_id_str,
            team = %team,
            error = %e,
            "Session ownership validation failed"
        );
        // Return 404 to avoid leaking info about other teams' sessions
        return Err(error_response(
            error_codes::INVALID_REQUEST,
            "Session not found or expired".to_string(),
        ));
    }

    // Check authorization
    if let Err(e) = check_method_authorization("tools/list", &context, scope_config) {
        error!(error = %e, "SSE authorization failed");
        return Err(error_response(error_codes::INVALID_REQUEST, e));
    }

    // Register SSE connection
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

    // Attach connection to session
    session_manager.attach_sse_connection(&session_id, connection_id.as_str());

    // Check for resumption request via Last-Event-ID header
    let replayed_messages = if let Some(last_event_id_str) = extract_last_event_id(&headers) {
        match EventId::from_last_event_id(&last_event_id_str) {
            Ok(event_id) => {
                let resume_conn_id = event_id.connection_id();

                // Validate connection ownership (team check for security)
                if let Some(conn_team) = connection_manager.get_team(resume_conn_id) {
                    if conn_team != team {
                        // Cross-team access attempt - log and ignore (no info leakage)
                        warn!(
                            resume_connection = %resume_conn_id,
                            resume_team = %conn_team,
                            request_team = %team,
                            "Cross-team resumption attempt blocked"
                        );
                        vec![]
                    } else {
                        // Replay messages from the old connection's buffer
                        if let Some(buffer) = connection_manager.get_message_buffer(resume_conn_id)
                        {
                            let replayed = buffer.replay_from(event_id.sequence()).await;
                            info!(
                                resume_connection = %resume_conn_id,
                                new_connection = %connection_id,
                                from_sequence = event_id.sequence(),
                                replayed_count = replayed.len(),
                                "Resuming SSE stream with buffered messages"
                            );
                            replayed
                        } else {
                            vec![]
                        }
                    }
                } else {
                    // Connection not found - likely expired, start fresh
                    warn!(
                        resume_connection = %resume_conn_id,
                        last_event_id = %last_event_id_str,
                        "Resumption requested for expired connection"
                    );
                    vec![]
                }
            }
            Err(e) => {
                // Invalid Last-Event-ID format - log and start fresh
                warn!(
                    last_event_id = %last_event_id_str,
                    error = %e,
                    "Invalid Last-Event-ID format, starting fresh stream"
                );
                vec![]
            }
        }
    } else {
        vec![]
    };

    info!(
        connection_id = %connection_id,
        session_id = %session_id_str,
        team = %team,
        token_name = %context.token_name,
        scope = ?scope,
        replayed_count = replayed_messages.len(),
        "SSE connection established"
    );

    // Create the endpoint URI that clients should use to POST messages
    let endpoint_uri =
        format!("{}?team={}&sessionId={}", scope.endpoint_path(), team, session_id_str);

    // Create initial endpoint event (required by MCP spec)
    let initial_event = Event::default().event("endpoint").data(endpoint_uri);

    // Get message buffer for this connection to track sequence
    let message_buffer = connection_manager
        .get_message_buffer(&connection_id)
        .expect("Buffer should exist for newly registered connection");

    // Create stream from receiver
    let receiver_stream = ReceiverStream::new(receiver);

    // Clone connection_id for use in closures
    let conn_id_for_replay = connection_id.clone();
    let conn_id_for_stream = connection_id.clone();

    // Create stream of replayed messages (if any)
    let replayed_stream =
        tokio_stream::iter(replayed_messages.into_iter().map(move |(seq, message)| {
            let event_id = EventId::new(conn_id_for_replay.clone(), seq);
            format_sse_event_with_id(&message, &event_id)
        }));

    // Use AtomicU64 for thread-safe sequence counter
    // Since we can't use async .then() without breaking Unpin, we use synchronous
    // buffering with blocking_push if needed, or we track sequence separately
    use std::sync::atomic::AtomicU64;
    let sequence_counter = std::sync::Arc::new(AtomicU64::new(message_buffer.next_sequence()));

    // Map live messages to SSE events with EventId
    // Note: Buffering is done synchronously using spawn_blocking or we skip buffering
    // for incoming messages since they're already delivered to the client
    let event_stream = receiver_stream.map(move |message| {
        // Get next sequence number atomically
        let seq = sequence_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let event_id = EventId::new(conn_id_for_stream.clone(), seq);
        format_sse_event_with_id(&message, &event_id)
    });

    // Combine streams: initial → replayed → live
    let initial_stream = tokio_stream::once(Ok::<_, Infallible>(initial_event));
    let combined_stream = initial_stream.chain(replayed_stream).chain(event_stream);

    // Wrap with cleanup stream to unregister connection when client disconnects
    let cleanup_stream = CleanupStream::new(
        combined_stream,
        connection_manager,
        connection_id,
        Some(session_id),
        session_manager,
    );

    // Create SSE response with keepalive
    let sse = Sse::new(cleanup_stream).keep_alive(
        KeepAlive::new().interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS)).text("ping"),
    );

    // Return SSE response with session ID header
    let header_name = HeaderName::from_static(MCP_SESSION_ID_HEADER);
    Ok(([(header_name, session_id_str)], sse))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_sse_event_with_id_ping() {
        let message = NotificationMessage::ping();
        let conn_id = ConnectionId::new("conn-test-1".to_string());
        let event_id = EventId::new(conn_id, 1);
        let result = format_sse_event_with_id(&message, &event_id);
        assert!(result.is_ok());
    }

    // EventId tests
    mod event_id_tests {
        use super::*;

        #[test]
        fn test_event_id_new() {
            let conn_id = ConnectionId::new("conn-team-abc-123".to_string());
            let event_id = EventId::new(conn_id.clone(), 42);

            assert_eq!(event_id.connection_id().as_str(), "conn-team-abc-123");
            assert_eq!(event_id.sequence(), 42);
        }

        #[test]
        fn test_event_id_display() {
            let conn_id = ConnectionId::new("conn-team-abc-123".to_string());
            let event_id = EventId::new(conn_id, 42);

            assert_eq!(event_id.to_string(), "conn-team-abc-123:42");
        }

        #[test]
        fn test_event_id_display_zero_sequence() {
            let conn_id = ConnectionId::new("conn-team-x-0".to_string());
            let event_id = EventId::new(conn_id, 0);

            assert_eq!(event_id.to_string(), "conn-team-x-0:0");
        }

        #[test]
        fn test_event_id_display_large_sequence() {
            let conn_id = ConnectionId::new("conn-team-abc-123".to_string());
            let event_id = EventId::new(conn_id, u64::MAX);

            assert_eq!(event_id.to_string(), format!("conn-team-abc-123:{}", u64::MAX));
        }

        #[test]
        fn test_from_last_event_id_valid() {
            let result = EventId::from_last_event_id("conn-team-xyz-456:123");
            assert!(result.is_ok());

            let event_id = result.unwrap();
            assert_eq!(event_id.connection_id().as_str(), "conn-team-xyz-456");
            assert_eq!(event_id.sequence(), 123);
        }

        #[test]
        fn test_from_last_event_id_uuid_format() {
            let result = EventId::from_last_event_id(
                "conn-team-abc-550e8400-e29b-41d4-a716-446655440000:999",
            );
            assert!(result.is_ok());

            let event_id = result.unwrap();
            assert_eq!(
                event_id.connection_id().as_str(),
                "conn-team-abc-550e8400-e29b-41d4-a716-446655440000"
            );
            assert_eq!(event_id.sequence(), 999);
        }

        #[test]
        fn test_from_last_event_id_zero_sequence() {
            let result = EventId::from_last_event_id("conn-team-abc:0");
            assert!(result.is_ok());

            let event_id = result.unwrap();
            assert_eq!(event_id.sequence(), 0);
        }

        #[test]
        fn test_from_last_event_id_large_sequence() {
            let result = EventId::from_last_event_id(&format!("conn-team-abc:{}", u64::MAX));
            assert!(result.is_ok());

            let event_id = result.unwrap();
            assert_eq!(event_id.sequence(), u64::MAX);
        }

        #[test]
        fn test_from_last_event_id_missing_colon() {
            let result = EventId::from_last_event_id("conn-team-xyz-456");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
            assert!(err.to_string().contains("format"));
        }

        #[test]
        fn test_from_last_event_id_invalid_sequence() {
            let result = EventId::from_last_event_id("conn-team-xyz:abc");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
            assert!(err.to_string().contains("sequence"));
        }

        #[test]
        fn test_from_last_event_id_negative_sequence() {
            let result = EventId::from_last_event_id("conn-team-xyz:-1");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
        }

        #[test]
        fn test_from_last_event_id_empty_connection() {
            let result = EventId::from_last_event_id(":123");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
            assert!(err.to_string().contains("empty"));
        }

        #[test]
        fn test_from_last_event_id_empty_sequence() {
            let result = EventId::from_last_event_id("conn-team-xyz:");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
        }

        #[test]
        fn test_from_last_event_id_only_colon() {
            let result = EventId::from_last_event_id(":");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
        }

        #[test]
        fn test_from_last_event_id_multiple_colons() {
            // Should use last colon as separator (future-proofing)
            let result = EventId::from_last_event_id("conn:team:abc:123");
            assert!(result.is_ok());

            let event_id = result.unwrap();
            assert_eq!(event_id.connection_id().as_str(), "conn:team:abc");
            assert_eq!(event_id.sequence(), 123);
        }

        #[test]
        fn test_from_last_event_id_roundtrip() {
            let original_conn = ConnectionId::new("conn-team-test-uuid".to_string());
            let original = EventId::new(original_conn, 12345);

            let serialized = original.to_string();
            let parsed = EventId::from_last_event_id(&serialized).unwrap();

            assert_eq!(parsed.connection_id().as_str(), original.connection_id().as_str());
            assert_eq!(parsed.sequence(), original.sequence());
        }

        #[test]
        fn test_from_last_event_id_overflow() {
            // u64::MAX + 1 as string
            let result = EventId::from_last_event_id("conn:18446744073709551616");
            assert!(result.is_err());

            let err = result.unwrap_err();
            assert!(matches!(err, McpError::InvalidEventId(_)));
        }
    }
}
