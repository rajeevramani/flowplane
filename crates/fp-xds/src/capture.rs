//! Envoy ALS/ExtProc learning capture services.
//!
//! xDS injects these services only for active capture scopes. This module is intentionally
//! thin: it parses Envoy protobufs into the domain `ObservationIngest` shape and delegates all
//! tenancy, quota, merge, TTL, and counter rules to storage.

use crate::ads::TeamResolver;
use envoy_types::pb::envoy::config::core::v3::{HeaderMap, RequestMethod};
use envoy_types::pb::envoy::data::accesslog::v3::HttpAccessLogEntry;
use envoy_types::pb::envoy::service::accesslog::v3::{
    access_log_service_server::{AccessLogService, AccessLogServiceServer},
    stream_access_logs_message, StreamAccessLogsMessage, StreamAccessLogsResponse,
};
use envoy_types::pb::envoy::service::ext_proc::v3::{
    common_response,
    external_processor_server::{ExternalProcessor, ExternalProcessorServer},
    processing_request, processing_response, BodyResponse, CommonResponse, HeadersResponse,
    ProcessingRequest, ProcessingResponse, TrailersResponse,
};
use fp_domain::api_lifecycle::ObservationIngest;
use fp_domain::{
    ApiDefinitionId, CaptureSessionId, DomainError, ListenerId, RouteConfigId, TeamId,
};
use fp_storage::repos::{api_lifecycle, identity};
use serde_json::{Map, Value};
use sqlx::types::chrono::Utc;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

const MAX_CAPTURE_BODY_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct LearningCaptureService {
    pool: sqlx::PgPool,
    resolver: Arc<dyn TeamResolver>,
}

#[derive(Debug, Clone, Copy)]
struct CaptureContext {
    team_id: TeamId,
    session_id: CaptureSessionId,
    api_definition_id: Option<ApiDefinitionId>,
    route_config_id: RouteConfigId,
    listener_id: Option<ListenerId>,
}

#[derive(Debug, Clone, Default)]
struct ExtProcState {
    request_id: Option<String>,
    method: Option<String>,
    path: Option<String>,
    request_headers: Map<String, Value>,
    response_headers: Map<String, Value>,
    response_status: Option<i32>,
}

impl LearningCaptureService {
    pub fn new(pool: sqlx::PgPool, resolver: Arc<dyn TeamResolver>) -> Self {
        Self { pool, resolver }
    }

    pub fn access_log_server(self) -> AccessLogServiceServer<Self> {
        AccessLogServiceServer::new(self)
    }

    pub fn ext_proc_server(self) -> ExternalProcessorServer<Self> {
        ExternalProcessorServer::new(self)
    }

    async fn ingest(&self, ctx: CaptureContext, input: ObservationIngest) -> Result<(), Status> {
        let team = identity::resolve_team_ref(&self.pool, ctx.team_id)
            .await
            .map_err(status_from_domain)?
            .ok_or_else(|| Status::not_found("capture team not found"))?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Status::unavailable(format!("begin capture ingest: {e}")))?;
        match api_lifecycle::ingest_raw_observation(
            &mut tx,
            team,
            ctx.session_id,
            ctx.api_definition_id,
            ctx.route_config_id,
            ctx.listener_id,
            &input,
        )
        .await
        {
            Ok(_) => tx
                .commit()
                .await
                .map_err(|e| Status::unavailable(format!("commit capture ingest: {e}"))),
            Err(err) => {
                tx.commit()
                    .await
                    .map_err(|e| Status::unavailable(format!("commit capture drop: {e}")))?;
                Err(status_from_domain(err))
            }
        }
    }
}

#[tonic::async_trait]
impl AccessLogService for LearningCaptureService {
    async fn stream_access_logs(
        &self,
        request: Request<Streaming<StreamAccessLogsMessage>>,
    ) -> Result<Response<StreamAccessLogsResponse>, Status> {
        let ctx = capture_context(&request, &self.resolver).await?;
        let mut stream = request.into_inner();
        while let Some(message) = stream.message().await? {
            if let Some(stream_access_logs_message::LogEntries::HttpLogs(entries)) =
                message.log_entries
            {
                for entry in entries.log_entry {
                    match observation_from_access_log(&entry) {
                        Some(input) => {
                            if let Err(status) = self.ingest(ctx, input).await {
                                tracing::warn!(code = ?status.code(), message = %status.message(), "dropped ALS learning observation");
                            }
                        }
                        None => tracing::debug!("skipping incomplete ALS learning entry"),
                    }
                }
            }
        }
        Ok(Response::new(StreamAccessLogsResponse {}))
    }
}

type ExtProcResponseStream = ReceiverStream<Result<ProcessingResponse, Status>>;

#[tonic::async_trait]
impl ExternalProcessor for LearningCaptureService {
    type ProcessStream = ExtProcResponseStream;

    async fn process(
        &self,
        request: Request<Streaming<ProcessingRequest>>,
    ) -> Result<Response<Self::ProcessStream>, Status> {
        let ctx = capture_context(&request, &self.resolver).await?;
        let mut stream = request.into_inner();
        let service = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProcessingResponse, Status>>(16);
        tokio::spawn(async move {
            let mut state = ExtProcState::default();
            loop {
                match stream.message().await {
                    Ok(Some(message)) => {
                        if tx.send(Ok(continue_response(&message))).await.is_err() {
                            break;
                        }
                        if let Some(input) = observation_from_ext_proc(&mut state, message) {
                            if let Err(status) = service.ingest(ctx, input).await {
                                tracing::warn!(code = ?status.code(), message = %status.message(), "dropped ExtProc learning observation");
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(status) => {
                        tracing::debug!(code = ?status.code(), message = %status.message(), "ExtProc learning stream ended with error");
                        break;
                    }
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn capture_context<T>(
    request: &Request<T>,
    resolver: &Arc<dyn TeamResolver>,
) -> Result<CaptureContext, Status> {
    let metadata = request.metadata();
    let claimed_team_id = TeamId::from(metadata_uuid(metadata, "x-flowplane-team-id")?);
    let peer_spiffe = request
        .peer_certs()
        .and_then(|certs| {
            certs
                .first()
                .and_then(|der| crate::server::spiffe_uri_from_der(der.as_ref()))
        })
        .or_else(|| {
            request
                .extensions()
                .get::<crate::server::PeerSpiffe>()
                .map(|p| p.0.clone())
        });
    let identity = resolver
        .resolve(&format!("team={claimed_team_id}"), peer_spiffe.as_deref())
        .await?;
    if identity.team_id != claimed_team_id {
        return Err(Status::permission_denied(
            "capture team_id does not match the client certificate",
        ));
    }
    Ok(CaptureContext {
        team_id: identity.team_id,
        session_id: CaptureSessionId::from(metadata_uuid(
            metadata,
            "x-flowplane-capture-session-id",
        )?),
        api_definition_id: optional_metadata_uuid(metadata, "x-flowplane-api-definition-id")?
            .map(ApiDefinitionId::from),
        route_config_id: RouteConfigId::from(metadata_uuid(
            metadata,
            "x-flowplane-route-config-id",
        )?),
        listener_id: optional_metadata_uuid(metadata, "x-flowplane-listener-id")?
            .map(ListenerId::from),
    })
}

fn metadata_uuid(metadata: &MetadataMap, key: &'static str) -> Result<Uuid, Status> {
    optional_metadata_uuid(metadata, key)?
        .ok_or_else(|| Status::invalid_argument(format!("missing capture metadata {key}")))
}

fn optional_metadata_uuid(
    metadata: &MetadataMap,
    key: &'static str,
) -> Result<Option<Uuid>, Status> {
    metadata
        .get(key)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| Status::invalid_argument(format!("invalid capture metadata {key}")))
                .and_then(|raw| {
                    Uuid::parse_str(raw).map_err(|_| {
                        Status::invalid_argument(format!("invalid capture metadata {key}"))
                    })
                })
        })
        .transpose()
}

fn observation_from_access_log(entry: &HttpAccessLogEntry) -> Option<ObservationIngest> {
    let request = entry.request.as_ref()?;
    let method = RequestMethod::try_from(request.request_method)
        .ok()?
        .as_str_name()
        .to_string();
    if method == "METHOD_UNSPECIFIED" {
        return None;
    }
    let request_id = if request.request_id.is_empty() {
        request.request_headers.get("x-request-id")?.clone()
    } else {
        request.request_id.clone()
    };
    let response = entry.response.as_ref();
    Some(ObservationIngest {
        request_id,
        method,
        path: if request.path.is_empty() {
            "/".to_string()
        } else {
            request.path.clone()
        },
        response_status: response
            .and_then(|r| r.response_code.as_ref())
            .and_then(|code| i32::try_from(code.value).ok()),
        request_headers: request
            .request_headers
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
        response_headers: response
            .map(|r| {
                r.response_headers
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect()
            })
            .unwrap_or_default(),
        request_body: None,
        response_body: None,
        request_body_truncated: false,
        response_body_truncated: false,
        request_body_bytes: Some(u64_to_i64(request.request_body_bytes)),
        response_body_bytes: response.map(|r| u64_to_i64(r.response_body_bytes)),
        metadata_seen: true,
        body_seen: false,
        observed_at: Utc::now(),
    })
}

fn observation_from_ext_proc(
    state: &mut ExtProcState,
    message: ProcessingRequest,
) -> Option<ObservationIngest> {
    match message.request? {
        processing_request::Request::RequestHeaders(headers) => {
            let map = headers_from_header_map(headers.headers.as_ref());
            state.method = header_value(&map, ":method");
            state.path = header_value(&map, ":path");
            state.request_id = header_value(&map, "x-request-id");
            state.request_headers = strip_pseudo_headers(map);
            base_ext_proc_observation(state, true, false)
        }
        processing_request::Request::ResponseHeaders(headers) => {
            let map = headers_from_header_map(headers.headers.as_ref());
            state.response_status = header_value(&map, ":status").and_then(|v| v.parse().ok());
            state.response_headers = strip_pseudo_headers(map);
            base_ext_proc_observation(state, true, false)
        }
        processing_request::Request::RequestBody(body) => {
            let (body, truncated) = capture_body(body.body, !body.end_of_stream);
            let mut input = base_ext_proc_observation(state, false, true)?;
            input.request_body = Some(body);
            input.request_body_truncated = truncated;
            Some(input)
        }
        processing_request::Request::ResponseBody(body) => {
            let (body, truncated) = capture_body(body.body, !body.end_of_stream);
            let mut input = base_ext_proc_observation(state, false, true)?;
            input.response_body = Some(body);
            input.response_body_truncated = truncated;
            Some(input)
        }
        processing_request::Request::RequestTrailers(_)
        | processing_request::Request::ResponseTrailers(_) => None,
    }
}

fn base_ext_proc_observation(
    state: &ExtProcState,
    metadata_seen: bool,
    body_seen: bool,
) -> Option<ObservationIngest> {
    Some(ObservationIngest {
        request_id: state.request_id.clone()?,
        method: state.method.clone()?,
        path: state.path.clone().unwrap_or_else(|| "/".to_string()),
        response_status: state.response_status,
        request_headers: if metadata_seen {
            state.request_headers.clone()
        } else {
            Map::new()
        },
        response_headers: if metadata_seen {
            state.response_headers.clone()
        } else {
            Map::new()
        },
        request_body: None,
        response_body: None,
        request_body_truncated: false,
        response_body_truncated: false,
        request_body_bytes: None,
        response_body_bytes: None,
        metadata_seen,
        body_seen,
        observed_at: Utc::now(),
    })
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn headers_from_header_map(headers: Option<&HeaderMap>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(headers) = headers {
        for header in &headers.headers {
            let value = if header.raw_value.is_empty() {
                header.value.clone()
            } else {
                String::from_utf8_lossy(&header.raw_value).to_string()
            };
            out.insert(header.key.clone(), Value::String(value));
        }
    }
    out
}

fn strip_pseudo_headers(mut headers: Map<String, Value>) -> Map<String, Value> {
    headers.retain(|key, _| !key.starts_with(':'));
    headers
}

fn header_value(headers: &Map<String, Value>, key: &str) -> Option<String> {
    headers.get(key).and_then(Value::as_str).map(str::to_string)
}

fn capture_body(bytes: Vec<u8>, partial: bool) -> (String, bool) {
    let truncated = partial || bytes.len() > MAX_CAPTURE_BODY_BYTES;
    let bytes = if bytes.len() > MAX_CAPTURE_BODY_BYTES {
        &bytes[..MAX_CAPTURE_BODY_BYTES]
    } else {
        &bytes
    };
    (String::from_utf8_lossy(bytes).to_string(), truncated)
}

fn continue_response(request: &ProcessingRequest) -> ProcessingResponse {
    let common = || CommonResponse {
        status: common_response::ResponseStatus::Continue as i32,
        ..Default::default()
    };
    let response = match request.request.as_ref() {
        Some(processing_request::Request::RequestHeaders(_)) => {
            processing_response::Response::RequestHeaders(HeadersResponse {
                response: Some(common()),
            })
        }
        Some(processing_request::Request::ResponseHeaders(_)) => {
            processing_response::Response::ResponseHeaders(HeadersResponse {
                response: Some(common()),
            })
        }
        Some(processing_request::Request::RequestBody(_)) => {
            processing_response::Response::RequestBody(BodyResponse {
                response: Some(common()),
            })
        }
        Some(processing_request::Request::ResponseBody(_)) => {
            processing_response::Response::ResponseBody(BodyResponse {
                response: Some(common()),
            })
        }
        Some(processing_request::Request::RequestTrailers(_)) => {
            processing_response::Response::RequestTrailers(TrailersResponse {
                header_mutation: None,
            })
        }
        Some(processing_request::Request::ResponseTrailers(_)) => {
            processing_response::Response::ResponseTrailers(TrailersResponse {
                header_mutation: None,
            })
        }
        None => processing_response::Response::RequestHeaders(HeadersResponse {
            response: Some(common()),
        }),
    };
    ProcessingResponse {
        response: Some(response),
        ..Default::default()
    }
}

fn status_from_domain(err: DomainError) -> Status {
    use fp_domain::ErrorCode;
    match err.code {
        ErrorCode::NotFound => Status::not_found(err.message),
        ErrorCode::ValidationFailed => Status::invalid_argument(err.message),
        ErrorCode::Conflict | ErrorCode::QuotaExceeded => Status::failed_precondition(err.message),
        ErrorCode::Unavailable | ErrorCode::RateLimited => Status::unavailable(err.message),
        ErrorCode::Unauthorized => Status::unauthenticated(err.message),
        ErrorCode::Forbidden => Status::permission_denied(err.message),
        _ => Status::internal(err.message),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ads::PeerIdentity;
    use envoy_types::pb::envoy::config::core::v3::{HeaderValue, RequestMethod};
    use envoy_types::pb::envoy::data::accesslog::v3::{
        HttpRequestProperties, HttpResponseProperties,
    };
    use envoy_types::pb::google::protobuf::UInt32Value;
    use std::collections::HashMap;
    use tonic::Code;

    struct FixedTeamResolver {
        team_id: TeamId,
    }

    #[tonic::async_trait]
    impl TeamResolver for FixedTeamResolver {
        async fn resolve(
            &self,
            _node_id: &str,
            _peer_spiffe: Option<&str>,
        ) -> Result<PeerIdentity, Status> {
            Ok(PeerIdentity {
                team_id: self.team_id,
                dataplane_id: None,
                certificate_id: None,
            })
        }
    }

    fn capture_request(team_id: TeamId) -> Request<()> {
        let mut request = Request::new(());
        let metadata = request.metadata_mut();
        metadata.insert(
            "x-flowplane-team-id",
            team_id.to_string().parse().expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-capture-session-id",
            Uuid::now_v7().to_string().parse().expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-route-config-id",
            Uuid::now_v7().to_string().parse().expect("metadata value"),
        );
        request
    }

    #[tokio::test]
    async fn capture_context_accepts_cert_bound_team_match() {
        let team_id = TeamId::from(Uuid::now_v7());
        let resolver = Arc::new(FixedTeamResolver { team_id }) as Arc<dyn TeamResolver>;
        let request = capture_request(team_id);

        let ctx = capture_context(&request, &resolver).await.expect("context");

        assert_eq!(ctx.team_id, team_id);
    }

    #[tokio::test]
    async fn capture_context_rejects_cert_bound_team_mismatch() {
        let claimed_team_id = TeamId::from(Uuid::now_v7());
        let bound_team_id = TeamId::from(Uuid::now_v7());
        let resolver = Arc::new(FixedTeamResolver {
            team_id: bound_team_id,
        }) as Arc<dyn TeamResolver>;
        let request = capture_request(claimed_team_id);

        let err = capture_context(&request, &resolver)
            .await
            .expect_err("mismatch should be rejected");

        assert_eq!(err.code(), Code::PermissionDenied);
        assert_eq!(
            err.message(),
            "capture team_id does not match the client certificate"
        );
    }

    #[test]
    fn als_entry_maps_request_response_metadata() {
        let entry = HttpAccessLogEntry {
            request: Some(HttpRequestProperties {
                request_method: RequestMethod::Post as i32,
                path: "/orders".into(),
                request_id: "req-1".into(),
                request_body_bytes: 123,
                request_headers: HashMap::from([("authorization".into(), "Bearer secret".into())]),
                ..Default::default()
            }),
            response: Some(HttpResponseProperties {
                response_code: Some(UInt32Value { value: 201 }),
                response_body_bytes: 456,
                response_headers: HashMap::from([(
                    "content-type".into(),
                    "application/json".into(),
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let input = observation_from_access_log(&entry).expect("observation");
        assert_eq!(input.request_id, "req-1");
        assert_eq!(input.method, "POST");
        assert_eq!(input.path, "/orders");
        assert_eq!(input.response_status, Some(201));
        assert_eq!(input.request_body_bytes, Some(123));
        assert_eq!(input.response_body_bytes, Some(456));
        assert_eq!(input.request_headers["authorization"], "Bearer secret");
        assert_eq!(input.response_headers["content-type"], "application/json");
        assert!(input.metadata_seen);
        assert!(!input.body_seen);
    }

    #[test]
    fn ext_proc_headers_and_body_merge_key_are_extracted() {
        let headers = HeaderMap {
            headers: vec![
                HeaderValue {
                    key: ":method".into(),
                    value: "PATCH".into(),
                    raw_value: Vec::new(),
                },
                HeaderValue {
                    key: ":path".into(),
                    value: "/items/1".into(),
                    raw_value: Vec::new(),
                },
                HeaderValue {
                    key: "x-request-id".into(),
                    value: "req-2".into(),
                    raw_value: Vec::new(),
                },
                HeaderValue {
                    key: "content-type".into(),
                    value: "application/json".into(),
                    raw_value: Vec::new(),
                },
            ],
        };
        let mut state = ExtProcState::default();
        let input = observation_from_ext_proc(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders {
                        headers: Some(headers),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        )
        .expect("headers observation");
        assert_eq!(input.request_id, "req-2");
        assert_eq!(input.method, "PATCH");
        assert_eq!(input.path, "/items/1");
        assert!(input.request_headers.get(":method").is_none());
        assert_eq!(input.request_headers["content-type"], "application/json");

        let input = observation_from_ext_proc(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: b"{\"a\":1}".to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        )
        .expect("body observation");
        assert_eq!(input.request_id, "req-2");
        assert_eq!(input.request_body.as_deref(), Some("{\"a\":1}"));
        assert!(!input.request_body_truncated);
        assert!(input.body_seen);
        assert!(!input.metadata_seen);
    }

    #[test]
    fn capture_body_caps_large_payloads() {
        let (body, truncated) = capture_body(vec![b'a'; MAX_CAPTURE_BODY_BYTES + 1], false);
        assert_eq!(body.len(), MAX_CAPTURE_BODY_BYTES);
        assert!(truncated);
    }
}
