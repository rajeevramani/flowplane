//! External Processor service for capturing request/response bodies during learning sessions
//!
//! This service implements Envoy's External Processor (ExtProc) protocol specifically for
//! capturing HTTP request and response bodies to support schema inference in learning sessions.
//!
//! **Design Decisions:**
//! - **Separation from ALS**: Access Log Service (ALS) handles metadata + sample counting;
//!   ExtProc handles ONLY body capture to avoid double-counting
//! - **Body Size Limits**: Maximum 10KB per request/response body with truncation and warnings
//! - **Session Matching**: Only captures bodies for requests matching active learning session patterns
//! - **Fail-Open**: Requests continue even if ExtProc processing fails
//! - **Integration**: Bodies sent to channel for merging with ALS metadata, matched by session_id + x-request-id

use envoy_types::pb::envoy::config::core::v3::HeaderValue;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessor;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    processing_request, processing_response, BodyResponse, CommonResponse, HeadersResponse,
    ProcessingRequest, ProcessingResponse,
};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

/// Maximum body size to capture (10KB)
const MAX_BODY_SIZE: usize = 10 * 1024;

/// Captured request and response bodies for a single request
#[derive(Debug, Clone)]
pub struct CapturedBody {
    /// Learning session ID this capture belongs to
    pub session_id: String,
    /// Unique request identifier (from x-request-id header)
    pub request_id: String,
    /// Captured request body (up to 10KB)
    pub request_body: Option<Vec<u8>>,
    /// Captured response body (up to 10KB)
    pub response_body: Option<Vec<u8>>,
    /// Whether request body was truncated
    pub request_truncated: bool,
    /// Whether response body was truncated
    pub response_truncated: bool,
}

/// Learning session information for path matching
#[derive(Debug, Clone)]
struct LearningSessionInfo {
    /// Regex pattern for matching request paths
    route_pattern: Regex,
}

/// External Processor service for body capture
#[derive(Clone)]
pub struct FlowplaneExtProcService {
    /// Active learning sessions indexed by session ID
    learning_sessions: Arc<RwLock<HashMap<String, LearningSessionInfo>>>,
    /// Channel for sending captured bodies to access log processor
    body_queue_tx: mpsc::UnboundedSender<CapturedBody>,
}

impl std::fmt::Debug for FlowplaneExtProcService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowplaneExtProcService")
            .field("learning_sessions", &"Arc<RwLock<HashMap<...>>>")
            .field("body_queue_tx", &"mpsc::UnboundedSender<CapturedBody>")
            .finish()
    }
}

impl FlowplaneExtProcService {
    /// Create a new ExtProc service instance
    pub fn new() -> (Self, mpsc::UnboundedReceiver<CapturedBody>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let service =
            Self { learning_sessions: Arc::new(RwLock::new(HashMap::new())), body_queue_tx: tx };
        (service, rx)
    }

    /// Add a learning session to monitor
    pub async fn add_session(
        &self,
        session_id: String,
        route_pattern: String,
    ) -> Result<(), String> {
        let regex = Regex::new(&route_pattern).map_err(|e| format!("Invalid regex: {}", e))?;
        let info = LearningSessionInfo { route_pattern: regex };
        self.learning_sessions.write().await.insert(session_id, info);
        Ok(())
    }

    /// Remove a learning session
    pub async fn remove_session(&self, session_id: &str) {
        self.learning_sessions.write().await.remove(session_id);
    }

    /// Extract request ID from headers
    fn extract_request_id(headers: &[HeaderValue]) -> Option<String> {
        headers
            .iter()
            .find(|h| h.key.eq_ignore_ascii_case("x-request-id"))
            .map(|h| String::from_utf8_lossy(&h.raw_value).to_string())
    }

    /// Extract path from headers
    fn extract_path(headers: &[HeaderValue]) -> Option<String> {
        headers
            .iter()
            .find(|h| h.key.eq_ignore_ascii_case(":path"))
            .map(|h| String::from_utf8_lossy(&h.raw_value).to_string())
    }

    /// Truncate body to MAX_BODY_SIZE if needed
    fn truncate_body(body: Vec<u8>) -> (Vec<u8>, bool) {
        if body.len() > MAX_BODY_SIZE {
            warn!(
                original_size = body.len(),
                truncated_size = MAX_BODY_SIZE,
                "Body exceeds 10KB limit, truncating"
            );
            (body[..MAX_BODY_SIZE].to_vec(), true)
        } else {
            (body, false)
        }
    }
}

impl Default for FlowplaneExtProcService {
    fn default() -> Self {
        Self::new().0
    }
}

#[tonic::async_trait]
impl ExternalProcessor for FlowplaneExtProcService {
    type ProcessStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<ProcessingResponse, Status>> + Send + 'static>,
    >;

    async fn process(
        &self,
        request: Request<Streaming<ProcessingRequest>>,
    ) -> Result<Response<Self::ProcessStream>, Status> {
        let active_session_count = self.learning_sessions.read().await.len();
        info!(active_session_count, "ExtProc: New processing stream opened from Envoy");

        let mut stream = request.into_inner();
        let body_queue_tx = self.body_queue_tx.clone();
        let sessions = self.learning_sessions.clone();

        let output_stream = async_stream::try_stream! {
            // State for tracking request context
            let mut session_id: Option<String> = None;
            let mut request_id: Option<String> = None;
            let mut request_body: Option<Vec<u8>> = None;
            let mut request_truncated = false;
            let mut response_body: Option<Vec<u8>> = None;
            let mut response_truncated = false;
            while let Some(req) = stream.message().await? {
                if let Some(request) = req.request {
                    match request {
                        // Request headers phase
                        processing_request::Request::RequestHeaders(headers) => {
                            debug!("ExtProc: Received request headers");

                            // Extract path and check for matching session
                            if let Some(path) = Self::extract_path(headers.headers.as_ref().map(|h| h.headers.as_slice()).unwrap_or(&[])) {
                                session_id = find_matching_session(&sessions, &path).await;
                            }

                            // Extract request ID
                            request_id = Self::extract_request_id(
                                headers.headers.as_ref().map(|h| h.headers.as_slice()).unwrap_or(&[])
                            );

                            // If we have a matching session, request body buffering
                            if session_id.is_some() {
                                info!(session_id = ?session_id, "ExtProc: Requesting request body buffering");
                                yield ProcessingResponse {
                                    response: Some(processing_response::Response::RequestHeaders(
                                        HeadersResponse {
                                            response: Some(CommonResponse {
                                                status: 0, // CONTINUE
                                                header_mutation: None,
                                                body_mutation: None,
                                                trailers: None,
                                                clear_route_cache: false,
                                            }),
                                        },
                                    )),
                                    mode_override: None,
                                    dynamic_metadata: None,
                                    override_message_timeout: None,
                                };
                            } else {
                                // No matching session, skip processing
                                let session_count = sessions.read().await.len();
                                info!(active_sessions = session_count, "ExtProc: No matching session, skipping body capture");
                                yield ProcessingResponse {
                                    response: Some(processing_response::Response::RequestHeaders(
                                        HeadersResponse {
                                            response: Some(CommonResponse {
                                                status: 0, // CONTINUE
                                                header_mutation: None,
                                                body_mutation: None,
                                                trailers: None,
                                                clear_route_cache: false,
                                            }),
                                        },
                                    )),
                                    mode_override: None,
                                    dynamic_metadata: None,
                                    override_message_timeout: None,
                                };
                            }
                        }

                        // Request body phase
                        processing_request::Request::RequestBody(body) => {
                            debug!(
                                session_match = session_id.is_some(),
                                end_of_stream = body.end_of_stream,
                                "ExtProc: Received request body chunk"
                            );

                            if session_id.is_some() {
                                // Accumulate body data
                                if request_body.is_none() {
                                    request_body = Some(Vec::new());
                                }
                                if let Some(ref mut buf) = request_body {
                                    buf.extend_from_slice(&body.body);
                                }

                                // If end of stream, truncate if needed
                                if body.end_of_stream {
                                    if let Some(body_data) = request_body.take() {
                                        let (truncated_body, truncated) = Self::truncate_body(body_data);
                                        request_body = Some(truncated_body);
                                        request_truncated = truncated;
                                    }
                                }
                            }

                            // Continue processing
                            yield ProcessingResponse {
                                response: Some(processing_response::Response::RequestBody(
                                    BodyResponse {
                                        response: Some(CommonResponse {
                                            status: 0, // CONTINUE
                                            header_mutation: None,
                                            body_mutation: None,
                                            trailers: None,
                                            clear_route_cache: false,
                                        }),
                                    },
                                )),
                                mode_override: None,
                                dynamic_metadata: None,
                                override_message_timeout: None,
                            };
                        }

                        // Response headers phase
                        processing_request::Request::ResponseHeaders(_headers) => {
                            debug!("ExtProc: Received response headers");

                            // Continue processing
                            yield ProcessingResponse {
                                response: Some(processing_response::Response::ResponseHeaders(
                                    HeadersResponse {
                                        response: Some(CommonResponse {
                                            status: 0, // CONTINUE
                                            header_mutation: None,
                                            body_mutation: None,
                                            trailers: None,
                                            clear_route_cache: false,
                                        }),
                                    },
                                )),
                                mode_override: None,
                                dynamic_metadata: None,
                                override_message_timeout: None,
                            };
                        }

                        // Response body phase
                        processing_request::Request::ResponseBody(body) => {
                            debug!(
                                session_match = session_id.is_some(),
                                end_of_stream = body.end_of_stream,
                                "ExtProc: Received response body chunk"
                            );

                            if session_id.is_some() {
                                // Accumulate body data
                                if response_body.is_none() {
                                    response_body = Some(Vec::new());
                                }
                                if let Some(ref mut buf) = response_body {
                                    buf.extend_from_slice(&body.body);
                                }

                                // If end of stream, truncate if needed and send captured data
                                if body.end_of_stream {
                                    if let Some(body_data) = response_body.take() {
                                        let (truncated_body, truncated) = Self::truncate_body(body_data);
                                        response_body = Some(truncated_body);
                                        response_truncated = truncated;
                                    }

                                    // Send captured bodies to processor
                                    if let (Some(sid), Some(rid)) = (&session_id, &request_id) {
                                        let captured = CapturedBody {
                                            session_id: sid.clone(),
                                            request_id: rid.clone(),
                                            request_body: request_body.clone(),
                                            response_body: response_body.clone(),
                                            request_truncated,
                                            response_truncated,
                                        };

                                        match body_queue_tx.send(captured) {
                                            Ok(_) => {
                                                debug!(
                                                    session_id = %sid,
                                                    request_id = %rid,
                                                    request_body_size = request_body.as_ref().map(|b| b.len()).unwrap_or(0),
                                                    response_body_size = response_body.as_ref().map(|b| b.len()).unwrap_or(0),
                                                    "ExtProc: Sent captured bodies to processor"
                                                );
                                            }
                                            Err(e) => {
                                                error!(error = %e, "ExtProc: Failed to send captured bodies");
                                            }
                                        }
                                    }

                                    // Reset state for next request
                                    session_id = None;
                                    request_id = None;
                                    request_body = None;
                                    request_truncated = false;
                                    response_body = None;
                                    response_truncated = false;
                                }
                            }

                            // Continue processing
                            yield ProcessingResponse {
                                response: Some(processing_response::Response::ResponseBody(
                                    BodyResponse {
                                        response: Some(CommonResponse {
                                            status: 0, // CONTINUE
                                            header_mutation: None,
                                            body_mutation: None,
                                            trailers: None,
                                            clear_route_cache: false,
                                        }),
                                    },
                                )),
                                mode_override: None,
                                dynamic_metadata: None,
                                override_message_timeout: None,
                            };
                        }

                        _ => {
                            debug!("ExtProc: Received other request type, skipping");
                        }
                    }
                }
            }
        };

        Ok(Response::new(Box::pin(output_stream)))
    }
}

// Helper function for async stream
async fn find_matching_session(
    sessions: &Arc<RwLock<HashMap<String, LearningSessionInfo>>>,
    path: &str,
) -> Option<String> {
    let sessions = sessions.read().await;
    for (session_id, info) in sessions.iter() {
        if info.route_pattern.is_match(path) {
            debug!(session_id = %session_id, path = %path, "Found matching learning session for ExtProc body capture");
            return Some(session_id.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_body_within_limit() {
        let body = vec![0u8; 5 * 1024]; // 5KB
        let (truncated, was_truncated) = FlowplaneExtProcService::truncate_body(body.clone());
        assert_eq!(truncated.len(), 5 * 1024);
        assert!(!was_truncated);
        assert_eq!(truncated, body);
    }

    #[test]
    fn test_truncate_body_exceeds_limit() {
        let body = vec![0u8; 15 * 1024]; // 15KB
        let (truncated, was_truncated) = FlowplaneExtProcService::truncate_body(body);
        assert_eq!(truncated.len(), MAX_BODY_SIZE);
        assert!(was_truncated);
    }

    #[test]
    fn test_truncate_body_at_limit() {
        let body = vec![0u8; MAX_BODY_SIZE];
        let (truncated, was_truncated) = FlowplaneExtProcService::truncate_body(body.clone());
        assert_eq!(truncated.len(), MAX_BODY_SIZE);
        assert!(!was_truncated);
        assert_eq!(truncated, body);
    }

    #[tokio::test]
    async fn test_add_and_remove_session() {
        let (service, _rx) = FlowplaneExtProcService::new();

        // Add session
        service.add_session("session-1".to_string(), r"^/users".to_string()).await.unwrap();

        // Verify session exists
        let sessions = service.learning_sessions.read().await;
        assert!(sessions.contains_key("session-1"));
        drop(sessions);

        // Remove session
        service.remove_session("session-1").await;

        // Verify session removed
        let sessions = service.learning_sessions.read().await;
        assert!(!sessions.contains_key("session-1"));
    }

    #[tokio::test]
    async fn test_find_matching_session() {
        let (service, _rx) = FlowplaneExtProcService::new();

        service.add_session("session-1".to_string(), r"^/users".to_string()).await.unwrap();
        service.add_session("session-2".to_string(), r"^/posts".to_string()).await.unwrap();

        let sessions = service.learning_sessions.clone();

        // Test matching paths using the helper function
        assert_eq!(
            find_matching_session(&sessions, "/users/123").await,
            Some("session-1".to_string())
        );
        assert_eq!(
            find_matching_session(&sessions, "/posts/456").await,
            Some("session-2".to_string())
        );

        // Test non-matching path
        assert_eq!(find_matching_session(&sessions, "/comments/789").await, None);
    }

    #[test]
    fn test_extract_request_id() {
        let headers = vec![
            HeaderValue {
                key: "x-request-id".to_string(),
                raw_value: b"test-request-123".to_vec(),
                value: String::new(),
            },
            HeaderValue {
                key: "content-type".to_string(),
                raw_value: b"application/json".to_vec(),
                value: String::new(),
            },
        ];

        let request_id = FlowplaneExtProcService::extract_request_id(&headers);
        assert_eq!(request_id, Some("test-request-123".to_string()));
    }

    #[test]
    fn test_extract_path() {
        let headers = vec![
            HeaderValue {
                key: ":path".to_string(),
                raw_value: b"/users/123".to_vec(),
                value: String::new(),
            },
            HeaderValue {
                key: ":method".to_string(),
                raw_value: b"GET".to_vec(),
                value: String::new(),
            },
        ];

        let path = FlowplaneExtProcService::extract_path(&headers);
        assert_eq!(path, Some("/users/123".to_string()));
    }

    #[tokio::test]
    async fn test_captured_body_structure() {
        let captured = CapturedBody {
            session_id: "session-1".to_string(),
            request_id: "req-123".to_string(),
            request_body: Some(b"{\"test\":\"data\"}".to_vec()),
            response_body: Some(b"{\"result\":\"ok\"}".to_vec()),
            request_truncated: false,
            response_truncated: true,
        };

        assert_eq!(captured.session_id, "session-1");
        assert_eq!(captured.request_id, "req-123");
        assert!(captured.request_body.is_some());
        assert!(captured.response_body.is_some());
        assert!(!captured.request_truncated);
        assert!(captured.response_truncated);
    }
}
