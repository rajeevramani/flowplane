/// Envoy Access Log Service gRPC implementation
///
/// This service implements the AccessLogService interface to receive HTTP access logs
/// from Envoy proxies. It is designed to filter logs for active learning sessions only
/// and queue them for background processing.
///
/// Key responsibilities:
/// - Receive StreamAccessLogsMessage from Envoy
/// - Parse HttpLogEntry for request/response details
/// - Filter logs based on active learning session route patterns
/// - Queue valid entries for background processing (in-memory only, no persistence)
/// - Return StreamAccessLogsResponse acknowledgments
use async_trait::async_trait;
use envoy_types::pb::envoy::data::accesslog::v3::{HttpAccessLogEntry, TcpAccessLogEntry};
use envoy_types::pb::envoy::service::accesslog::v3::{
    access_log_service_server::AccessLogService, StreamAccessLogsMessage, StreamAccessLogsResponse,
};
#[allow(unused_imports)] // Will be used for protobuf decoding in stream processing
use prost::Message;
use regex::Regex;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info};

use crate::observability::metrics;
use crate::storage::repositories::LearningSessionRepository;

#[allow(unused_imports)] // Will be used for logging unknown entry types
use tracing::warn;

/// Represents an active learning session with route patterns to match
#[derive(Debug, Clone)]
#[allow(dead_code)] // Will be fully utilized in subtask 1.4
pub struct LearningSession {
    /// Unique session ID
    pub id: String,
    /// Owning team for multi-tenancy
    pub team: String,
    /// Route patterns to match (regex patterns)
    pub route_patterns: Vec<Regex>,
    /// Optional method filter (GET, POST, etc.)
    pub methods: Option<Vec<String>>,
}

/// Processed HTTP access log entry ready for background processing
#[derive(Debug, Clone)]
#[allow(dead_code)] // Will be queued and processed in subtask 1.4
pub struct ProcessedLogEntry {
    /// Session ID this log belongs to
    pub session_id: String,
    /// Request ID for correlating with ExtProc body captures (x-request-id header)
    pub request_id: Option<String>,
    /// Team that owns the learning session
    pub team: String,
    /// HTTP method
    pub method: i32,
    /// Request path
    pub path: String,
    /// Request headers (limited subset)
    pub request_headers: Vec<(String, String)>,
    /// Request body (first 10KB) - for schema inference only, NOT persisted
    pub request_body: Option<Vec<u8>>,
    /// Request body size
    pub request_body_size: u64,
    /// Response status code
    pub response_status: u32,
    /// Response headers (limited subset)
    pub response_headers: Vec<(String, String)>,
    /// Response body (first 10KB) - for schema inference only, NOT persisted
    pub response_body: Option<Vec<u8>>,
    /// Response body size
    pub response_body_size: u64,
    /// Request start timestamp
    pub start_time_seconds: i64,
    /// Request duration in milliseconds
    pub duration_ms: i64,
}

impl LearningSession {
    /// Check if a request path matches any of the session's route patterns
    #[allow(dead_code)] // Used in tests and will be used in stream processing
    pub fn matches_path(&self, path: &str) -> bool {
        self.route_patterns.iter().any(|pattern| pattern.is_match(path))
    }

    /// Check if a request method matches the session's method filter (if any)
    #[allow(dead_code)] // Used in tests and will be used in stream processing
    pub fn matches_method(&self, method: &str) -> bool {
        match &self.methods {
            Some(methods) => methods.iter().any(|m| m.eq_ignore_ascii_case(method)),
            None => true, // No filter means all methods match
        }
    }
}

/// Implementation of the Envoy AccessLogService
///
/// This service receives access log streams from Envoy proxies and processes them
/// asynchronously. Filters logs based on active learning sessions and queues
/// valid entries for background processing.
#[derive(Clone)]
pub struct FlowplaneAccessLogService {
    /// Active learning sessions (shared across streams)
    learning_sessions: Arc<RwLock<Vec<LearningSession>>>,
    /// Channel sender for queuing processed log entries
    log_queue_tx: mpsc::UnboundedSender<ProcessedLogEntry>,
    /// Repository for incrementing sample counts
    session_repository: Option<LearningSessionRepository>,
}

// Manual Debug implementation since we removed derive(Debug)
impl std::fmt::Debug for FlowplaneAccessLogService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowplaneAccessLogService")
            .field("learning_sessions", &self.learning_sessions)
            .field("has_session_repository", &self.session_repository.is_some())
            .finish()
    }
}

impl FlowplaneAccessLogService {
    /// Create a new AccessLogService instance
    ///
    /// Returns both the service and a receiver for processing queued log entries
    pub fn new() -> (Self, mpsc::UnboundedReceiver<ProcessedLogEntry>) {
        info!("Initializing Flowplane AccessLogService with filtering and queuing");

        let (tx, rx) = mpsc::unbounded_channel();
        let service = Self {
            learning_sessions: Arc::new(RwLock::new(Vec::new())),
            log_queue_tx: tx,
            session_repository: None,
        };

        (service, rx)
    }

    /// Set the learning session repository for sample count tracking
    pub fn with_repository(mut self, repository: LearningSessionRepository) -> Self {
        self.session_repository = Some(repository);
        self
    }

    /// Add a learning session to track
    ///
    /// # Arguments
    /// * `session` - The learning session to add
    #[allow(dead_code)] // Will be used in subtask 1.4 for server integration
    pub async fn add_session(&self, session: LearningSession) {
        let mut sessions = self.learning_sessions.write().await;
        info!(session_id = %session.id, patterns = sessions.len(), "Adding learning session");
        sessions.push(session);
    }

    /// Remove a learning session
    ///
    /// # Arguments
    /// * `session_id` - The session ID to remove
    #[allow(dead_code)] // Will be used in subtask 1.4 for server integration
    pub async fn remove_session(&self, session_id: &str) {
        let mut sessions = self.learning_sessions.write().await;
        sessions.retain(|s| s.id != session_id);
        info!(session_id = %session_id, "Removed learning session");
    }

    /// Check if a log entry matches any active learning session
    ///
    /// # Arguments
    /// * `path` - The request path
    /// * `method` - The HTTP method
    ///
    /// # Returns
    /// The session ID if matched, None otherwise
    #[allow(dead_code)] // Will be used in stream processing with real Envoy
    async fn find_matching_session(&self, path: &str, method: &str) -> Option<String> {
        let sessions = self.learning_sessions.read().await;

        for session in sessions.iter() {
            if session.matches_path(path) && session.matches_method(method) {
                debug!(
                    session_id = %session.id,
                    path = %path,
                    method = %method,
                    "Access log matched learning session"
                );
                return Some(session.id.clone());
            }
        }

        None
    }

    /// Parse an `Any` protobuf message into an HttpAccessLogEntry
    ///
    /// # Arguments
    /// * `any` - The google.protobuf.Any message from log_entries
    ///
    /// # Returns
    /// Some(HttpAccessLogEntry) if successfully parsed, None otherwise
    #[allow(dead_code)] // Will be used once we determine exact wire format
    fn parse_http_log_entry(any: &prost_types::Any) -> Option<HttpAccessLogEntry> {
        // Type URL for HTTP access log entries
        const HTTP_LOG_TYPE_URL: &str =
            "type.googleapis.com/envoy.data.accesslog.v3.HTTPAccessLogEntry";

        if any.type_url == HTTP_LOG_TYPE_URL {
            match HttpAccessLogEntry::decode(&any.value[..]) {
                Ok(entry) => Some(entry),
                Err(e) => {
                    error!(error = %e, "Failed to decode HTTPAccessLogEntry");
                    None
                }
            }
        } else {
            None
        }
    }

    /// Parse an `Any` protobuf message into a TcpAccessLogEntry
    ///
    /// # Arguments
    /// * `any` - The google.protobuf.Any message from log_entries
    ///
    /// # Returns
    /// Some(TcpAccessLogEntry) if successfully parsed, None otherwise
    #[allow(dead_code)] // Will be used once we determine exact wire format
    fn parse_tcp_log_entry(any: &prost_types::Any) -> Option<TcpAccessLogEntry> {
        // Type URL for TCP access log entries
        const TCP_LOG_TYPE_URL: &str =
            "type.googleapis.com/envoy.data.accesslog.v3.TCPAccessLogEntry";

        if any.type_url == TCP_LOG_TYPE_URL {
            match TcpAccessLogEntry::decode(&any.value[..]) {
                Ok(entry) => Some(entry),
                Err(e) => {
                    error!(error = %e, "Failed to decode TCPAccessLogEntry");
                    None
                }
            }
        } else {
            None
        }
    }

    /// Process an HTTP access log entry and extract relevant details
    ///
    /// Extracts:
    /// - HTTP method, path, headers, body (first 10KB)
    /// - Response status, headers, body (first 10KB)
    ///
    /// # Arguments
    /// * `entry` - The parsed HttpAccessLogEntry
    /// * `session_id` - The learning session ID this entry belongs to
    ///
    /// # Returns
    /// A ProcessedLogEntry ready for queuing
    /// Extract x-request-id from request headers for correlation with ExtProc body captures
    fn extract_request_id_from_headers(
        request_headers: &std::collections::HashMap<String, String>,
    ) -> Option<String> {
        request_headers
            .get("x-request-id")
            .or_else(|| request_headers.get(":path").and(request_headers.get("x-request-id")))
            .cloned()
    }

    #[allow(dead_code)] // Will be used once we determine exact wire format
    fn process_http_log_entry(
        entry: &HttpAccessLogEntry,
        session_id: String,
        team: String,
    ) -> ProcessedLogEntry {
        // Extract request details
        let (method, path, request_headers, request_body, request_body_size, request_id) =
            if let Some(request) = &entry.request {
                let method = request.request_method;
                let path = request.path.clone();

                // Extract limited headers (first 20 headers to avoid excessive memory)
                // Note: Actual header structure will be validated with real Envoy
                let headers: Vec<(String, String)> = Vec::new(); // TODO: Parse headers once structure is confirmed

                // Extract x-request-id for correlation with ExtProc body captures
                let headers_map: std::collections::HashMap<String, String> =
                    request.request_headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let request_id = Self::extract_request_id_from_headers(&headers_map);

                // TODO: Extract request body (up to 10KB for schema inference)
                // Note: The HttpRequestProperties protobuf from Envoy does NOT include
                // the actual body content by default. To capture bodies, we need to:
                // 1. Enable body buffering in Envoy's HTTP connection manager
                // 2. Use the `body` field in access log format configuration
                // For now, bodies are not available in the protobuf message.
                let body = None;

                let body_size = request.request_body_bytes;

                debug!(
                    method = method,
                    path = %path,
                    headers_count = headers.len(),
                    body_bytes = body_size,
                    has_body = body.is_some(),
                    request_id = ?request_id,
                    "HTTP request extracted"
                );

                (method, path, headers, body, body_size, request_id)
            } else {
                (0, String::new(), Vec::new(), None, 0, None)
            };

        // Extract response details
        let (response_status, response_headers, response_body, response_body_size) =
            if let Some(response) = &entry.response {
                let status = response.response_code.as_ref().map(|c| c.value).unwrap_or(0);

                // Extract limited headers
                // Note: Actual header structure will be validated with real Envoy
                let headers: Vec<(String, String)> = Vec::new(); // TODO: Parse headers once structure is confirmed

                // TODO: Extract response body (up to 10KB for schema inference)
                // Note: The HttpResponseProperties protobuf from Envoy does NOT include
                // the actual body content by default. To capture bodies, we need to:
                // 1. Enable body buffering in Envoy's HTTP connection manager
                // 2. Use the `body` field in access log format configuration
                // For now, bodies are not available in the protobuf message.
                let body = None;

                let body_size = response.response_body_bytes;

                debug!(
                    status = status,
                    headers_count = headers.len(),
                    body_bytes = body_size,
                    has_body = body.is_some(),
                    "HTTP response extracted"
                );

                (status, headers, body, body_size)
            } else {
                (0, Vec::new(), None, 0)
            };

        // Extract timing information
        let (start_time_seconds, duration_ms) = if let Some(common) = &entry.common_properties {
            let start = common.start_time.as_ref().map(|t| t.seconds).unwrap_or(0);

            let duration = common
                .time_to_last_downstream_tx_byte
                .as_ref()
                .map(|d| (d.nanos / 1_000_000) as i64)
                .unwrap_or(0);

            (start, duration)
        } else {
            (0, 0)
        };

        ProcessedLogEntry {
            session_id,
            request_id,
            team,
            method,
            path,
            request_headers,
            request_body,
            request_body_size,
            response_status,
            response_headers,
            response_body,
            response_body_size,
            start_time_seconds,
            duration_ms,
        }
    }
}

impl Default for FlowplaneAccessLogService {
    fn default() -> Self {
        Self::new().0 // Return the service, discard the receiver
    }
}

#[async_trait]
impl AccessLogService for FlowplaneAccessLogService {
    /// StreamAccessLogs receives a stream of access log messages from Envoy
    ///
    /// According to Envoy documentation, this is a client-streaming RPC where:
    /// - Envoy sends StreamAccessLogsMessage messages continuously
    /// - The server should respond with StreamAccessLogsResponse (empty acknowledgment)
    /// - No response is strictly required as Envoy doesn't act on failures
    ///
    /// # Arguments
    /// * `request` - Tonic request containing stream of StreamAccessLogsMessage
    ///
    /// # Returns
    /// Empty StreamAccessLogsResponse as acknowledgment
    async fn stream_access_logs(
        &self,
        request: Request<tonic::Streaming<StreamAccessLogsMessage>>,
    ) -> Result<Response<StreamAccessLogsResponse>, Status> {
        let mut stream = request.into_inner();

        debug!("AccessLogService: New stream connection established");

        // Process incoming log messages
        let mut message_count = 0;
        while let Some(result) = stream.message().await? {
            let start = Instant::now();
            message_count += 1;

            // Extract identifier information
            if let Some(identifier) = &result.identifier {
                debug!(
                    node_id = %identifier.node.as_ref().map(|n| &n.id).unwrap_or(&String::new()),
                    log_name = %identifier.log_name,
                    "Received access log message"
                );
            }

            // Process log entries from the message
            // The log_entries field is an Option<LogEntries> enum with HttpLogs or TcpLogs
            if let Some(log_entries) = &result.log_entries {
                use envoy_types::pb::envoy::service::accesslog::v3::stream_access_logs_message::LogEntries;

                match log_entries {
                    LogEntries::HttpLogs(http_logs) => {
                        let entry_count = http_logs.log_entry.len();
                        debug!(
                            entry_count = entry_count,
                            "Access log message with HTTP entries received"
                        );

                        // Record message metrics
                        metrics::record_access_log_message(entry_count).await;

                        // Update active session count
                        let session_count = {
                            let sessions = self.learning_sessions.read().await;
                            sessions.len()
                        };
                        metrics::update_active_learning_sessions(session_count).await;

                        // Process each HTTP log entry
                        for http_entry in &http_logs.log_entry {
                            // Extract path and method for matching
                            let (path, method) = if let Some(request) = &http_entry.request {
                                let method_str = match request.request_method {
                                    1 => "GET",
                                    2 => "POST",
                                    3 => "PUT",
                                    4 => "DELETE",
                                    5 => "PATCH",
                                    6 => "HEAD",
                                    7 => "OPTIONS",
                                    8 => "TRACE",
                                    9 => "CONNECT",
                                    _ => "UNKNOWN",
                                };
                                (request.path.as_str(), method_str)
                            } else {
                                continue; // Skip entries without request data
                            };

                            // Check if this entry matches any active learning session
                            if let Some(session_id) = self.find_matching_session(path, method).await
                            {
                                debug!(
                                    session_id = %session_id,
                                    path = %path,
                                    method = %method,
                                    "Access log matched learning session, processing entry"
                                );

                                // Find the session to extract team for attribution
                                let team = {
                                    let sessions = self.learning_sessions.read().await;
                                    sessions
                                        .iter()
                                        .find(|s| s.id == session_id)
                                        .map(|s| s.team.clone())
                                        .unwrap_or_else(|| "".to_string())
                                };

                                // Process and queue the entry
                                let processed_entry = Self::process_http_log_entry(
                                    http_entry,
                                    session_id.clone(),
                                    team,
                                );

                                // Queue for background processing
                                match self.log_queue_tx.send(processed_entry) {
                                    Ok(_) => {
                                        // Increment sample count for the learning session
                                        if let Some(ref repository) = self.session_repository {
                                            match repository
                                                .increment_sample_count(&session_id)
                                                .await
                                            {
                                                Ok(new_count) => {
                                                    debug!(
                                                        session_id = %session_id,
                                                        sample_count = new_count,
                                                        "Incremented learning session sample count"
                                                    );
                                                }
                                                Err(e) => {
                                                    error!(
                                                        session_id = %session_id,
                                                        error = %e,
                                                        "Failed to increment sample count"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!(
                                            error = %e,
                                            "Failed to queue processed log entry for background processing"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    LogEntries::TcpLogs(_tcp_logs) => {
                        // We're focused on HTTP logs for learning sessions
                        debug!("Received TCP access logs, skipping (not supported for learning sessions)");
                    }
                }
            }

            // Record processing latency
            let duration = start.elapsed().as_secs_f64();
            metrics::record_access_log_latency(duration).await;
        }

        info!(total_messages = message_count, "Access log stream completed");

        // Return empty response as acknowledgment
        Ok(Response::new(StreamAccessLogsResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_creation() {
        let (_service, _rx) = FlowplaneAccessLogService::new();
        // Service and receiver created successfully
    }

    #[test]
    fn test_service_default() {
        let _service = FlowplaneAccessLogService::default();
        // Service created with default implementation
    }

    #[test]
    fn test_learning_session_path_matching() {
        let session = LearningSession {
            id: "session-1".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![
                Regex::new(r"^/api/users/.*").unwrap(),
                Regex::new(r"^/api/products/\d+").unwrap(),
            ],
            methods: None,
        };

        // Test matching paths
        assert!(session.matches_path("/api/users/123"));
        assert!(session.matches_path("/api/products/456"));

        // Test non-matching paths
        assert!(!session.matches_path("/api/orders/789"));
        assert!(!session.matches_path("/health"));
    }

    #[test]
    fn test_learning_session_method_matching() {
        let session = LearningSession {
            id: "session-2".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![Regex::new(r"^/api/.*").unwrap()],
            methods: Some(vec!["GET".to_string(), "POST".to_string()]),
        };

        // Test matching methods
        assert!(session.matches_method("GET"));
        assert!(session.matches_method("POST"));
        assert!(session.matches_method("get")); // Case insensitive

        // Test non-matching methods
        assert!(!session.matches_method("DELETE"));
        assert!(!session.matches_method("PUT"));
    }

    #[test]
    fn test_learning_session_no_method_filter() {
        let session = LearningSession {
            id: "session-3".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![Regex::new(r"^/api/.*").unwrap()],
            methods: None, // No method filter
        };

        // All methods should match when no filter is specified
        assert!(session.matches_method("GET"));
        assert!(session.matches_method("POST"));
        assert!(session.matches_method("DELETE"));
        assert!(session.matches_method("PATCH"));
    }

    #[tokio::test]
    async fn test_add_and_remove_sessions() {
        let (service, _rx) = FlowplaneAccessLogService::new();

        let session1 = LearningSession {
            id: "session-1".to_string(),
            team: "team-a".to_string(),
            route_patterns: vec![Regex::new(r"^/api/.*").unwrap()],
            methods: None,
        };

        let session2 = LearningSession {
            id: "session-2".to_string(),
            team: "team-b".to_string(),
            route_patterns: vec![Regex::new(r"^/admin/.*").unwrap()],
            methods: Some(vec!["GET".to_string()]),
        };

        // Add sessions
        service.add_session(session1).await;
        service.add_session(session2).await;

        // Verify sessions exist by checking matches
        assert!(service.find_matching_session("/api/users", "POST").await.is_some());
        assert!(service.find_matching_session("/admin/dashboard", "GET").await.is_some());

        // Remove one session
        service.remove_session("session-1").await;

        // Verify session-1 is gone
        assert!(service.find_matching_session("/api/users", "POST").await.is_none());

        // Verify session-2 still exists
        assert!(service.find_matching_session("/admin/dashboard", "GET").await.is_some());
    }

    #[tokio::test]
    async fn test_find_matching_session() {
        let (service, _rx) = FlowplaneAccessLogService::new();

        let session = LearningSession {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![
                Regex::new(r"^/api/v1/users/.*").unwrap(),
                Regex::new(r"^/api/v1/posts/\d+").unwrap(),
            ],
            methods: Some(vec!["GET".to_string(), "POST".to_string()]),
        };

        service.add_session(session).await;

        // Test matching cases
        let result = service.find_matching_session("/api/v1/users/123", "GET").await;
        assert_eq!(result, Some("test-session".to_string()));

        let result = service.find_matching_session("/api/v1/posts/456", "POST").await;
        assert_eq!(result, Some("test-session".to_string()));

        // Test non-matching cases
        let result = service.find_matching_session("/api/v1/users/123", "DELETE").await;
        assert_eq!(result, None);

        let result = service.find_matching_session("/api/v2/users/123", "GET").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_process_http_log_entry() {
        use envoy_types::pb::envoy::data::accesslog::v3::{
            AccessLogCommon, HttpRequestProperties, HttpResponseProperties,
        };
        use envoy_types::pb::google::protobuf::{Duration, Timestamp, UInt32Value};

        // Create a mock HTTP access log entry
        let entry = HttpAccessLogEntry {
            common_properties: Some(AccessLogCommon {
                start_time: Some(Timestamp { seconds: 1234567890, nanos: 0 }),
                time_to_last_downstream_tx_byte: Some(Duration {
                    seconds: 0,
                    nanos: 42_000_000, // 42ms
                }),
                ..Default::default()
            }),
            request: Some(HttpRequestProperties {
                request_method: 1, // GET
                path: "/api/users/123".to_string(),
                request_body_bytes: 0,
                ..Default::default()
            }),
            response: Some(HttpResponseProperties {
                response_code: Some(UInt32Value { value: 200 }),
                response_body_bytes: 1024,
                ..Default::default()
            }),
            ..Default::default()
        };

        let session_id = "test-session".to_string();
        let processed = FlowplaneAccessLogService::process_http_log_entry(
            &entry,
            session_id,
            "test-team".to_string(),
        );

        assert_eq!(processed.session_id, "test-session");
        assert_eq!(processed.method, 1); // GET
        assert_eq!(processed.path, "/api/users/123");
        assert_eq!(processed.response_status, 200);
        assert_eq!(processed.request_body_size, 0);
        assert_eq!(processed.response_body_size, 1024);
        assert_eq!(processed.start_time_seconds, 1234567890);
        assert_eq!(processed.duration_ms, 42);
    }

    #[tokio::test]
    async fn test_parse_http_log_entry() {
        use envoy_types::pb::envoy::data::accesslog::v3::HttpRequestProperties;

        // Create a protobuf Any message containing an HTTP log entry
        let http_entry = HttpAccessLogEntry {
            request: Some(HttpRequestProperties {
                request_method: 2, // POST
                path: "/api/posts".to_string(),
                request_body_bytes: 100,
                ..Default::default()
            }),
            ..Default::default()
        };

        // Encode to protobuf Any
        let any = prost_types::Any {
            type_url: "type.googleapis.com/envoy.data.accesslog.v3.HTTPAccessLogEntry".to_string(),
            value: {
                let mut buf = Vec::new();
                prost::Message::encode(&http_entry, &mut buf).unwrap();
                buf
            },
        };

        // Parse back
        let parsed = FlowplaneAccessLogService::parse_http_log_entry(&any);
        assert!(parsed.is_some());

        let parsed_entry = parsed.unwrap();
        assert_eq!(parsed_entry.request.as_ref().unwrap().request_method, 2);
        assert_eq!(parsed_entry.request.as_ref().unwrap().path, "/api/posts");
    }

    #[tokio::test]
    async fn test_parse_invalid_http_log_entry() {
        // Create an Any message with wrong type URL
        let any = prost_types::Any {
            type_url: "type.googleapis.com/some.other.Type".to_string(),
            value: vec![1, 2, 3, 4],
        };

        let parsed = FlowplaneAccessLogService::parse_http_log_entry(&any);
        assert!(parsed.is_none());
    }

    #[tokio::test]
    async fn test_queue_processing_with_matching_session() {
        let (service, mut rx) = FlowplaneAccessLogService::new();

        // Add a session
        let session = LearningSession {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![Regex::new(r"^/api/users/.*").unwrap()],
            methods: None,
        };
        service.add_session(session).await;

        // Simulate processing a matching entry
        use envoy_types::pb::envoy::data::accesslog::v3::{
            HttpRequestProperties, HttpResponseProperties,
        };
        use envoy_types::pb::google::protobuf::UInt32Value;

        let http_entry = HttpAccessLogEntry {
            request: Some(HttpRequestProperties {
                request_method: 1, // GET
                path: "/api/users/123".to_string(),
                request_body_bytes: 0,
                ..Default::default()
            }),
            response: Some(HttpResponseProperties {
                response_code: Some(UInt32Value { value: 200 }),
                response_body_bytes: 512,
                ..Default::default()
            }),
            ..Default::default()
        };

        // Check if it matches
        let session_id = service.find_matching_session("/api/users/123", "GET").await;
        assert_eq!(session_id, Some("test-session".to_string()));

        // Process and queue
        let processed = FlowplaneAccessLogService::process_http_log_entry(
            &http_entry,
            session_id.unwrap(),
            "test-team".to_string(),
        );
        service.log_queue_tx.send(processed).unwrap();

        // Verify it's in the queue
        let queued_entry = rx.try_recv();
        assert!(queued_entry.is_ok());

        let entry = queued_entry.unwrap();
        assert_eq!(entry.session_id, "test-session");
        assert_eq!(entry.path, "/api/users/123");
        assert_eq!(entry.response_status, 200);
    }

    #[tokio::test]
    async fn test_no_queue_for_non_matching_session() {
        let (service, mut rx) = FlowplaneAccessLogService::new();

        // Add a session for /api/users only
        let session = LearningSession {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![Regex::new(r"^/api/users/.*").unwrap()],
            methods: None,
        };
        service.add_session(session).await;

        // Try to match a different path
        let session_id = service.find_matching_session("/api/posts/123", "GET").await;
        assert_eq!(session_id, None);

        // Queue should be empty
        let queued_entry = rx.try_recv();
        assert!(queued_entry.is_err()); // No entry queued
    }

    #[tokio::test]
    async fn test_method_filtering() {
        let (service, _rx) = FlowplaneAccessLogService::new();

        // Add a session that only matches GET and POST
        let session = LearningSession {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_patterns: vec![Regex::new(r"^/api/.*").unwrap()],
            methods: Some(vec!["GET".to_string(), "POST".to_string()]),
        };
        service.add_session(session).await;

        // GET should match
        let result = service.find_matching_session("/api/users", "GET").await;
        assert_eq!(result, Some("test-session".to_string()));

        // POST should match
        let result = service.find_matching_session("/api/users", "POST").await;
        assert_eq!(result, Some("test-session".to_string()));

        // DELETE should not match
        let result = service.find_matching_session("/api/users", "DELETE").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_multiple_sessions_priority() {
        let (service, _rx) = FlowplaneAccessLogService::new();

        // Add multiple sessions with overlapping patterns
        let session1 = LearningSession {
            id: "session-1".to_string(),
            team: "team-a".to_string(),
            route_patterns: vec![Regex::new(r"^/api/.*").unwrap()],
            methods: None,
        };
        service.add_session(session1).await;

        let session2 = LearningSession {
            id: "session-2".to_string(),
            team: "team-b".to_string(),
            route_patterns: vec![Regex::new(r"^/api/users/.*").unwrap()],
            methods: None,
        };
        service.add_session(session2).await;

        // Should match the first one added (session-1)
        let result = service.find_matching_session("/api/users/123", "GET").await;
        assert_eq!(result, Some("session-1".to_string()));
    }
}
