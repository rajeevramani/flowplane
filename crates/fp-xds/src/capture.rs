//! Envoy ALS/ExtProc learning capture services.
//!
//! xDS injects these services only for active capture scopes. This module is intentionally
//! thin: it parses Envoy protobufs into the domain `ObservationIngest` shape and delegates all
//! tenancy, quota, merge, TTL, and counter rules to storage.

use crate::ads::TeamResolver;
use envoy_types::pb::envoy::config::core::v3::{
    header_value_option, HeaderMap, HeaderValue, HeaderValueOption, RequestMethod,
};
use envoy_types::pb::envoy::data::accesslog::v3::HttpAccessLogEntry;
use envoy_types::pb::envoy::r#type::v3::{HttpStatus, StatusCode};
use envoy_types::pb::envoy::service::accesslog::v3::{
    access_log_service_server::{AccessLogService, AccessLogServiceServer},
    stream_access_logs_message, StreamAccessLogsMessage, StreamAccessLogsResponse,
};
use envoy_types::pb::envoy::service::ext_proc::v3::{
    body_mutation, common_response,
    external_processor_server::{ExternalProcessor, ExternalProcessorServer},
    processing_request, processing_response, BodyMutation, BodyResponse, CommonResponse,
    HeaderMutation, HeadersResponse, ImmediateResponse, ProcessingRequest, ProcessingResponse,
    TrailersResponse,
};
use fp_domain::api_lifecycle::ObservationIngest;
use fp_domain::discovery::DiscoveryObservationProvenance;
use fp_domain::{
    openai_usage_from_json, prepare_openai_chat_request, rewrite_openai_chat_request_model,
    strip_synthetic_openai_usage_sse, AiProviderId, AiRouteSpec, ApiDefinitionId, CaptureSessionId,
    DiscoverySessionId, DomainError, ListenerId, OpenAiTokenUsage, RouteConfigId, SecretSpec,
    TeamId, AI_MODEL_HEADER,
};
use fp_storage::repos::{ai, api_lifecycle, discovery, identity, secrets};
use serde_json::{Map, Value};
use sqlx::types::chrono::Utc;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

const MAX_CAPTURE_BODY_BYTES: usize = 64 * 1024;
const MAX_AI_USAGE_JSON_BYTES: usize = 1024 * 1024;
const MAX_AI_SSE_REMAINDER_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct LearningCaptureService {
    pool: sqlx::PgPool,
    resolver: Arc<dyn TeamResolver>,
}

#[derive(Debug, Clone, Copy)]
struct ConfigCaptureContext {
    team_id: TeamId,
    session_id: CaptureSessionId,
    api_definition_id: Option<ApiDefinitionId>,
    route_config_id: RouteConfigId,
    listener_id: Option<ListenerId>,
}

#[derive(Debug, Clone)]
struct DiscoveryCaptureContext {
    team_id: TeamId,
    session_id: DiscoverySessionId,
    listener_id: ListenerId,
    forwarded_upstream_host: String,
    forwarded_upstream_port: i32,
    forwarded_upstream_ip: String,
    forwarded_upstream_tls: bool,
}

#[derive(Debug, Clone)]
enum CaptureContext {
    Config(ConfigCaptureContext),
    Discovery(DiscoveryCaptureContext),
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

#[derive(Debug, Clone, Default)]
struct AiExtProcState {
    context: Option<AiExtProcContext>,
    include_usage_injected: bool,
    response_status: Option<i32>,
    response_content_type: Option<String>,
    response_sse_remainder: String,
    response_json_body: Vec<u8>,
    last_usage: Option<OpenAiTokenUsage>,
    upstream_model_override: Option<String>,
    request_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AiExtProcContext {
    team_id: TeamId,
    listener_id: Option<ListenerId>,
    route_config_id: RouteConfigId,
    provider_id: Option<AiProviderId>,
    backend_position: Option<i32>,
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
        let team_id = match &ctx {
            CaptureContext::Config(ctx) => ctx.team_id,
            CaptureContext::Discovery(ctx) => ctx.team_id,
        };
        let team = identity::resolve_team_ref(&self.pool, team_id)
            .await
            .map_err(status_from_domain)?
            .ok_or_else(|| Status::not_found("capture team not found"))?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Status::unavailable(format!("begin capture ingest: {e}")))?;
        let result = match &ctx {
            CaptureContext::Config(ctx) => api_lifecycle::ingest_raw_observation(
                &mut tx,
                team,
                ctx.session_id,
                ctx.api_definition_id,
                ctx.route_config_id,
                ctx.listener_id,
                &input,
            )
            .await
            .map(|_| ()),
            CaptureContext::Discovery(ctx) => {
                let provenance = DiscoveryObservationProvenance {
                    discovery_session_id: ctx.session_id,
                    discovery_listener_id: ctx.listener_id,
                    observed_host: observed_host(&input)
                        .unwrap_or_else(|| ctx.forwarded_upstream_host.clone()),
                    observed_sni: None,
                    route_matched: false,
                    forwarded_upstream_host: ctx.forwarded_upstream_host.clone(),
                    forwarded_upstream_port: ctx.forwarded_upstream_port,
                    forwarded_upstream_ip: ctx.forwarded_upstream_ip.clone(),
                    forwarded_upstream_tls: ctx.forwarded_upstream_tls,
                };
                discovery::ingest_raw_observation(&mut tx, team, &input, &provenance)
                    .await
                    .map(|_| ())
            }
        };
        match result {
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
                            if let Err(status) = self.ingest(ctx.clone(), input).await {
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
        if is_ai_processor(request.metadata()) {
            let context = ai_context(request.metadata())?;
            return Ok(Response::new(ReceiverStream::new(ai_process_stream(
                self.pool.clone(),
                request.into_inner(),
                context,
            ))));
        }
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
                            if let Err(status) = service.ingest(ctx.clone(), input).await {
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

fn ai_process_stream(
    pool: sqlx::PgPool,
    mut stream: Streaming<ProcessingRequest>,
    context: Option<AiExtProcContext>,
) -> tokio::sync::mpsc::Receiver<Result<ProcessingResponse, Status>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProcessingResponse, Status>>(16);
    tokio::spawn(async move {
        let mut state = AiExtProcState {
            context,
            ..Default::default()
        };
        loop {
            match stream.message().await {
                Ok(Some(message)) => {
                    let response = ai_response_with_pool(&pool, &mut state, message).await;
                    if tx.send(Ok(response)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(status) => {
                    tracing::debug!(code = ?status.code(), message = %status.message(), "AI ExtProc stream ended with error");
                    break;
                }
            }
        }
        persist_ai_usage(&pool, &state).await;
    });
    rx
}

fn is_ai_processor(metadata: &MetadataMap) -> bool {
    metadata
        .get("x-flowplane-ai-processor")
        .and_then(|value| value.to_str().ok())
        == Some("true")
}

fn ai_context(metadata: &MetadataMap) -> Result<Option<AiExtProcContext>, Status> {
    let team_id = optional_metadata_uuid(metadata, "x-flowplane-team-id")?;
    let listener_id = optional_metadata_uuid(metadata, "x-flowplane-listener-id")?;
    let route_config_id = optional_metadata_uuid(metadata, "x-flowplane-route-config-id")?;
    let provider_id = optional_metadata_uuid(metadata, "x-flowplane-ai-provider-id")?;
    let backend_position = optional_metadata_i32(metadata, "x-flowplane-ai-backend-position")?;
    match (
        team_id,
        listener_id,
        route_config_id,
        provider_id,
        backend_position,
    ) {
        (Some(team_id), Some(listener_id), Some(route_config_id), None, None) => {
            Ok(Some(AiExtProcContext {
                team_id: TeamId::from(team_id),
                listener_id: Some(ListenerId::from(listener_id)),
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: None,
                backend_position: None,
            }))
        }
        (Some(team_id), None, Some(route_config_id), Some(provider_id), Some(backend_position)) => {
            Ok(Some(AiExtProcContext {
                team_id: TeamId::from(team_id),
                listener_id: None,
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: Some(AiProviderId::from(provider_id)),
                backend_position: Some(backend_position),
            }))
        }
        (None, None, None, None, None) => Ok(None),
        _ => Err(Status::invalid_argument(
            "AI processor metadata must include either router or upstream context",
        )),
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
    if let Some(session_id) = optional_metadata_uuid(metadata, "x-flowplane-discovery-session-id")?
    {
        return Ok(CaptureContext::Discovery(DiscoveryCaptureContext {
            team_id: identity.team_id,
            session_id: DiscoverySessionId::from(session_id),
            listener_id: ListenerId::from(metadata_uuid(
                metadata,
                "x-flowplane-discovery-listener-id",
            )?),
            forwarded_upstream_host: metadata_string(
                metadata,
                "x-flowplane-forwarded-upstream-host",
            )?,
            forwarded_upstream_port: metadata_i32(metadata, "x-flowplane-forwarded-upstream-port")?,
            forwarded_upstream_ip: metadata_string(metadata, "x-flowplane-forwarded-upstream-ip")?,
            forwarded_upstream_tls: metadata_bool(metadata, "x-flowplane-forwarded-upstream-tls")?,
        }));
    }
    Ok(CaptureContext::Config(ConfigCaptureContext {
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
    }))
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

fn optional_metadata_i32(metadata: &MetadataMap, key: &'static str) -> Result<Option<i32>, Status> {
    metadata
        .get(key)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| Status::invalid_argument(format!("invalid capture metadata {key}")))
                .and_then(|raw| {
                    raw.parse::<i32>().map_err(|_| {
                        Status::invalid_argument(format!("invalid capture metadata {key}"))
                    })
                })
        })
        .transpose()
}

fn metadata_string(metadata: &MetadataMap, key: &'static str) -> Result<String, Status> {
    metadata
        .get(key)
        .ok_or_else(|| Status::invalid_argument(format!("missing capture metadata {key}")))?
        .to_str()
        .map(str::to_string)
        .map_err(|_| Status::invalid_argument(format!("invalid capture metadata {key}")))
}

fn metadata_i32(metadata: &MetadataMap, key: &'static str) -> Result<i32, Status> {
    metadata_string(metadata, key)?
        .parse()
        .map_err(|_| Status::invalid_argument(format!("invalid capture metadata {key}")))
}

fn metadata_bool(metadata: &MetadataMap, key: &'static str) -> Result<bool, Status> {
    metadata_string(metadata, key)?
        .parse()
        .map_err(|_| Status::invalid_argument(format!("invalid capture metadata {key}")))
}

fn observed_host(input: &ObservationIngest) -> Option<String> {
    header_value(&input.request_headers, "host")
        .or_else(|| header_value(&input.request_headers, ":authority"))
        .map(|host| {
            host.split_once(':')
                .map(|(h, _)| h)
                .unwrap_or(&host)
                .to_string()
        })
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

async fn ai_response_with_pool(
    pool: &sqlx::PgPool,
    state: &mut AiExtProcState,
    request: ProcessingRequest,
) -> ProcessingResponse {
    if let Some(processing_request::Request::RequestHeaders(headers)) = request.request.as_ref() {
        let map = headers_from_header_map(headers.headers.as_ref());
        state.request_path = header_value(&map, ":path");
        return ai_request_headers_response(pool, state, request).await;
    }
    if matches!(
        request.request,
        Some(processing_request::Request::RequestBody(_))
    ) && state
        .context
        .and_then(|context| context.provider_id)
        .is_some()
    {
        return ai_upstream_request_body_response(state, request);
    }
    ai_response(state, request)
}

fn ai_upstream_request_body_response(
    state: &AiExtProcState,
    request: ProcessingRequest,
) -> ProcessingResponse {
    let Some(processing_request::Request::RequestBody(body)) = request.request else {
        return request_body_response(CommonResponse {
            status: common_response::ResponseStatus::Continue as i32,
            ..Default::default()
        });
    };
    let Some(model) = state.upstream_model_override.as_deref() else {
        return request_body_response(CommonResponse {
            status: common_response::ResponseStatus::Continue as i32,
            ..Default::default()
        });
    };
    match rewrite_openai_chat_request_model(&body.body, model) {
        Ok(body) => request_body_response(CommonResponse {
            status: common_response::ResponseStatus::Continue as i32,
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(body)),
            }),
            ..Default::default()
        }),
        Err(err) => immediate_response(400, err.message),
    }
}

fn ai_response(state: &mut AiExtProcState, request: ProcessingRequest) -> ProcessingResponse {
    let Some(request) = request.request else {
        return request_headers_response(CommonResponse {
            status: common_response::ResponseStatus::Continue as i32,
            ..Default::default()
        });
    };
    match request {
        processing_request::Request::RequestHeaders(_) => {
            request_headers_response(remove_internal_model_header())
        }
        processing_request::Request::ResponseHeaders(headers) => {
            let map = headers_from_header_map(headers.headers.as_ref());
            state.response_status = header_value(&map, ":status").and_then(|v| v.parse().ok());
            state.response_content_type = header_value(&map, "content-type")
                .or_else(|| header_value(&map, "Content-Type"))
                .map(|value| value.to_ascii_lowercase());
            response_headers_response(CommonResponse {
                status: common_response::ResponseStatus::Continue as i32,
                ..Default::default()
            })
        }
        processing_request::Request::RequestBody(body) => {
            match prepare_openai_chat_request(&body.body) {
                Ok(prepared) => {
                    state.include_usage_injected = prepared.include_usage_injected;
                    request_body_response(CommonResponse {
                        status: common_response::ResponseStatus::Continue as i32,
                        header_mutation: Some(HeaderMutation {
                            set_headers: vec![HeaderValueOption {
                                header: Some(HeaderValue {
                                    key: AI_MODEL_HEADER.into(),
                                    value: prepared.model,
                                    raw_value: Vec::new(),
                                }),
                                append_action:
                                    header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd
                                        as i32,
                                ..Default::default()
                            }],
                            remove_headers: Vec::new(),
                        }),
                        body_mutation: prepared.include_usage_injected.then_some(BodyMutation {
                            mutation: Some(body_mutation::Mutation::Body(prepared.body)),
                        }),
                        clear_route_cache: true,
                        ..Default::default()
                    })
                }
                Err(err) => immediate_response(400, err.message),
            }
        }
        processing_request::Request::ResponseBody(body) => {
            let mutation = ai_response_body_mutation(state, body.body, body.end_of_stream);
            if let Some(body) = mutation {
                response_body_response(CommonResponse {
                    status: common_response::ResponseStatus::Continue as i32,
                    body_mutation: Some(BodyMutation {
                        mutation: Some(body_mutation::Mutation::Body(body)),
                    }),
                    ..Default::default()
                })
            } else {
                response_body_response(CommonResponse {
                    status: common_response::ResponseStatus::Continue as i32,
                    ..Default::default()
                })
            }
        }
        other => continue_for_request(other),
    }
}

async fn single_eligible_backend(
    pool: &sqlx::PgPool,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    model: &str,
) -> Result<Option<(AiProviderId, i32)>, DomainError> {
    let row = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT r.spec \
         FROM ai_routes r \
         JOIN route_configs rc ON rc.team_id = r.team_id AND rc.name = r.route_config_name \
         WHERE rc.team_id = $1 AND rc.id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(route_config_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|err| DomainError::internal(format!("load AI route for backend selection: {err}")))?;
    let Some(spec) = row else {
        return Ok(None);
    };
    let spec: AiRouteSpec = serde_json::from_value(spec).map_err(|err| {
        DomainError::internal(format!("AI route spec in DB does not parse: {err}"))
    })?;
    let mut eligible = spec.backends.iter().enumerate().filter(|(_, backend)| {
        backend.models.is_empty() || backend.models.iter().any(|m| m == model)
    });
    let Some((idx, backend)) = eligible.next() else {
        return Ok(None);
    };
    if eligible.next().is_some() {
        return Ok(None);
    }
    Ok(Some((backend.provider_id, idx as i32)))
}

async fn ai_request_headers_response(
    pool: &sqlx::PgPool,
    state: &mut AiExtProcState,
    request: ProcessingRequest,
) -> ProcessingResponse {
    let Some(context) = state.context else {
        return request_headers_response(remove_internal_model_header());
    };
    let Some(provider_id) = context.provider_id else {
        return ai_listener_request_headers_response(pool, state, request, context).await;
    };
    match ai::exhausted_enforcing_budget(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
    )
    .await
    {
        Ok(Some(name)) => {
            return immediate_response_with_details(
                429,
                format!("AI budget \"{name}\" exceeded"),
                "flowplane_ai_budget_exceeded",
            );
        }
        Ok(None) => {}
        Err(err) => {
            tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to check AI budget: {}", err.message);
            return immediate_response(500, "AI budget check unavailable".into());
        }
    }
    let request_path = match request.request {
        Some(processing_request::Request::RequestHeaders(headers)) => {
            let map = headers_from_header_map(headers.headers.as_ref());
            header_value(&map, ":path")
        }
        _ => None,
    };
    match selected_backend_runtime(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
        context.backend_position,
        request_path.as_deref(),
    )
    .await
    {
        Ok(runtime) => {
            state.upstream_model_override = runtime.model_override;
            let mut set_headers = vec![HeaderValueOption {
                header: Some(HeaderValue {
                    key: runtime.auth_header,
                    value: runtime.auth_value,
                    raw_value: Vec::new(),
                }),
                append_action: header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd
                    as i32,
                ..Default::default()
            }];
            if let Some(path) = runtime.path_rewrite {
                set_headers.push(HeaderValueOption {
                    header: Some(HeaderValue {
                        key: ":path".into(),
                        value: path,
                        raw_value: Vec::new(),
                    }),
                    append_action: header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd
                        as i32,
                    ..Default::default()
                });
            }
            request_headers_response(CommonResponse {
                status: common_response::ResponseStatus::Continue as i32,
                header_mutation: Some(HeaderMutation {
                    set_headers,
                    remove_headers: vec![AI_MODEL_HEADER.into()],
                }),
                ..Default::default()
            })
        }
        Err(_) => immediate_response(500, "AI provider credential unavailable".into()),
    }
}

async fn ai_listener_request_headers_response(
    pool: &sqlx::PgPool,
    state: &mut AiExtProcState,
    request: ProcessingRequest,
    context: AiExtProcContext,
) -> ProcessingResponse {
    let request_model = match request.request {
        Some(processing_request::Request::RequestHeaders(headers)) => {
            let map = headers_from_header_map(headers.headers.as_ref());
            header_value(&map, AI_MODEL_HEADER)
        }
        _ => None,
    };
    let Some(model) = request_model else {
        return request_headers_response(remove_internal_model_header());
    };
    let selected = match single_eligible_backend(
        pool,
        context.team_id,
        context.route_config_id,
        &model,
    )
    .await
    {
        Ok(Some(selected)) => selected,
        Ok(None) => {
            return request_headers_response(remove_internal_model_header());
        }
        Err(err) => {
            tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to select AI backend: {}", err.message);
            return request_headers_response(remove_internal_model_header());
        }
    };
    let (provider_id, backend_position) = selected;
    match ai::exhausted_enforcing_budget(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
    )
    .await
    {
        Ok(Some(name)) => {
            return immediate_response_with_details(
                429,
                format!("AI budget \"{name}\" exceeded"),
                "flowplane_ai_budget_exceeded",
            );
        }
        Ok(None) => {}
        Err(err) => {
            tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to check AI budget: {}", err.message);
            return immediate_response(500, "AI budget check unavailable".into());
        }
    }
    match selected_backend_runtime(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
        Some(backend_position),
        state.request_path.as_deref(),
    )
    .await
    {
        Ok(runtime) => {
            state.upstream_model_override = runtime.model_override;
            let mut set_headers = vec![HeaderValueOption {
                header: Some(HeaderValue {
                    key: runtime.auth_header,
                    value: runtime.auth_value,
                    raw_value: Vec::new(),
                }),
                append_action: header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd
                    as i32,
                ..Default::default()
            }];
            if let Some(path) = runtime.path_rewrite {
                set_headers.push(HeaderValueOption {
                    header: Some(HeaderValue {
                        key: ":path".into(),
                        value: path,
                        raw_value: Vec::new(),
                    }),
                    append_action: header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd
                        as i32,
                    ..Default::default()
                });
            }
            request_headers_response(CommonResponse {
                status: common_response::ResponseStatus::Continue as i32,
                header_mutation: Some(HeaderMutation {
                    set_headers,
                    remove_headers: vec![AI_MODEL_HEADER.into()],
                }),
                ..Default::default()
            })
        }
        Err(_) => immediate_response(500, "AI provider credential unavailable".into()),
    }
}

#[derive(Debug)]
struct SelectedBackendRuntime {
    auth_header: String,
    auth_value: String,
    path_rewrite: Option<String>,
    model_override: Option<String>,
}

async fn selected_backend_runtime(
    pool: &sqlx::PgPool,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
    backend_position: Option<i32>,
    request_path: Option<&str>,
) -> Result<SelectedBackendRuntime, Status> {
    let selected = ai::get_backend_for_route_config(
        pool,
        team_id,
        route_config_id,
        provider_id,
        backend_position,
    )
    .await
    .map_err(status_from_domain)?
    .ok_or_else(|| Status::not_found("AI provider not found for route"))?;
    let provider = selected.provider;
    let encrypted =
        secrets::get_encrypted_secret_by_id(pool, team_id, provider.spec.credential_secret_id)
            .await
            .map_err(status_from_domain)?
            .ok_or_else(|| Status::not_found("AI provider credential not found"))?;
    let spec = crate::snapshot::decrypt_secret_spec(
        &encrypted.ciphertext,
        &encrypted.nonce,
        &encrypted.metadata.encryption_key_id,
    )
    .map_err(status_from_domain)?;
    let SecretSpec::GenericSecret { secret } = spec else {
        return Err(Status::failed_precondition(
            "AI provider credential is not a generic secret",
        ));
    };
    let value = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, secret)
        .map_err(|_| Status::failed_precondition("AI provider credential is invalid"))?;
    let value = String::from_utf8(value)
        .map_err(|_| Status::failed_precondition("AI provider credential is not UTF-8"))?;
    Ok(SelectedBackendRuntime {
        auth_header: provider.spec.auth_header,
        auth_value: value,
        path_rewrite: provider_path_rewrite(provider.spec.path_prefix.as_deref(), request_path),
        model_override: selected.backend.model_override,
    })
}

fn provider_path_rewrite(path_prefix: Option<&str>, request_path: Option<&str>) -> Option<String> {
    let prefix = path_prefix?;
    let request_path = request_path.unwrap_or("/v1/chat/completions");
    Some(join_prefix_path(prefix, request_path))
}

fn join_prefix_path(prefix: &str, path: &str) -> String {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let joined = format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    if query.is_empty() {
        joined
    } else {
        format!("{joined}?{query}")
    }
}

fn ai_response_body_mutation(
    state: &mut AiExtProcState,
    body: Vec<u8>,
    end_of_stream: bool,
) -> Option<Vec<u8>> {
    if state.response_status.is_some_and(|status| status >= 400) {
        return None;
    }
    let parse_sse = response_content_type_matches(state, "text/event-stream");
    let parse_json = response_content_type_matches(state, "application/json");

    if parse_json {
        collect_unary_usage_body(state, &body, end_of_stream);
    }
    if parse_sse {
        let body_text = String::from_utf8_lossy(&body);
        state.response_sse_remainder.push_str(&body_text);
        if state.response_sse_remainder.len() > MAX_AI_SSE_REMAINDER_BYTES {
            state.response_sse_remainder.clear();
            return None;
        }

        let (complete, remainder) =
            complete_sse_prefix(&state.response_sse_remainder, end_of_stream);
        let (stripped, usage) =
            strip_synthetic_openai_usage_sse(&complete, state.include_usage_injected);
        if let Some(usage) = usage {
            remember_ai_usage(state, usage);
        }
        state.response_sse_remainder = remainder;

        if state.include_usage_injected && stripped.as_bytes() != body.as_slice() {
            return Some(stripped.into_bytes());
        }
    }
    None
}

fn response_content_type_matches(state: &AiExtProcState, expected: &str) -> bool {
    state
        .response_content_type
        .as_deref()
        .map(|content_type| content_type.split(';').any(|part| part.trim() == expected))
        .unwrap_or(true)
}

fn complete_sse_prefix(buffer: &str, end_of_stream: bool) -> (String, String) {
    if end_of_stream {
        return (buffer.to_string(), String::new());
    }
    let Some(index) = buffer.rfind("\n\n") else {
        return (String::new(), buffer.to_string());
    };
    let split = index + 2;
    (buffer[..split].to_string(), buffer[split..].to_string())
}

fn collect_unary_usage_body(state: &mut AiExtProcState, body: &[u8], end_of_stream: bool) {
    if state.response_json_body.len() < MAX_AI_USAGE_JSON_BYTES {
        let remaining = MAX_AI_USAGE_JSON_BYTES - state.response_json_body.len();
        state
            .response_json_body
            .extend_from_slice(&body[..body.len().min(remaining)]);
    }
    if end_of_stream {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&state.response_json_body) {
            if let Some(usage) = openai_usage_from_json(&value) {
                remember_ai_usage(state, usage);
            }
        }
        state.response_json_body.clear();
    }
}

fn remember_ai_usage(state: &mut AiExtProcState, usage: OpenAiTokenUsage) {
    if let Some(context) = state.context {
        tracing::debug!(
            team = %context.team_id,
            listener = ?context.listener_id,
            route_config = %context.route_config_id,
            total_tokens = usage.total_tokens,
            "captured AI usage for future persistence"
        );
    }
    state.last_usage = Some(usage);
}

async fn persist_ai_usage(pool: &sqlx::PgPool, state: &AiExtProcState) {
    let (Some(context), Some(usage)) = (state.context, state.last_usage) else {
        return;
    };
    let Some(provider_id) = context.provider_id else {
        return;
    };
    if let Err(err) = ai::record_usage_event_and_settle_budgets(
        pool,
        ai::AiUsageEventInsert {
            team_id: context.team_id,
            route_config_id: context.route_config_id,
            provider_id,
            backend_position: context.backend_position,
            usage,
        },
    )
    .await
    {
        tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to persist AI usage: {}", err.message);
    }
}

fn continue_for_request(request: processing_request::Request) -> ProcessingResponse {
    let common = Some(CommonResponse {
        status: common_response::ResponseStatus::Continue as i32,
        ..Default::default()
    });
    let response = match request {
        processing_request::Request::RequestHeaders(_) => {
            processing_response::Response::RequestHeaders(HeadersResponse { response: common })
        }
        processing_request::Request::ResponseHeaders(_) => {
            processing_response::Response::ResponseHeaders(HeadersResponse { response: common })
        }
        processing_request::Request::RequestBody(_) => {
            processing_response::Response::RequestBody(BodyResponse { response: common })
        }
        processing_request::Request::ResponseBody(_) => {
            processing_response::Response::ResponseBody(BodyResponse { response: common })
        }
        processing_request::Request::RequestTrailers(_) => {
            processing_response::Response::RequestTrailers(TrailersResponse {
                header_mutation: None,
            })
        }
        processing_request::Request::ResponseTrailers(_) => {
            processing_response::Response::ResponseTrailers(TrailersResponse {
                header_mutation: None,
            })
        }
    };
    ProcessingResponse {
        response: Some(response),
        ..Default::default()
    }
}

fn remove_internal_model_header() -> CommonResponse {
    CommonResponse {
        status: common_response::ResponseStatus::Continue as i32,
        header_mutation: Some(HeaderMutation {
            set_headers: Vec::new(),
            remove_headers: vec![AI_MODEL_HEADER.into()],
        }),
        ..Default::default()
    }
}

fn request_headers_response(common: CommonResponse) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(processing_response::Response::RequestHeaders(
            HeadersResponse {
                response: Some(common),
            },
        )),
        ..Default::default()
    }
}

fn request_body_response(common: CommonResponse) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(processing_response::Response::RequestBody(BodyResponse {
            response: Some(common),
        })),
        ..Default::default()
    }
}

fn response_headers_response(common: CommonResponse) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(processing_response::Response::ResponseHeaders(
            HeadersResponse {
                response: Some(common),
            },
        )),
        ..Default::default()
    }
}

fn response_body_response(common: CommonResponse) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(processing_response::Response::ResponseBody(BodyResponse {
            response: Some(common),
        })),
        ..Default::default()
    }
}

fn immediate_response(status: u32, message: String) -> ProcessingResponse {
    immediate_response_with_details(status, message, "flowplane_ai_request_invalid")
}

fn immediate_response_with_details(
    status: u32,
    message: String,
    details: &str,
) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(
            ImmediateResponse {
                status: Some(HttpStatus {
                    code: if status == 400 {
                        StatusCode::BadRequest as i32
                    } else {
                        status as i32
                    },
                }),
                body: message.into_bytes(),
                details: details.into(),
                ..Default::default()
            },
        )),
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

        match ctx {
            CaptureContext::Config(ctx) => assert_eq!(ctx.team_id, team_id),
            CaptureContext::Discovery(_) => panic!("expected config capture context"),
        }
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
    fn ai_context_requires_complete_identity_metadata() {
        let mut request = Request::new(());
        request.metadata_mut().insert(
            "x-flowplane-team-id",
            Uuid::now_v7().to_string().parse().expect("metadata value"),
        );

        let err = ai_context(request.metadata()).expect_err("partial metadata");

        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.message().contains("router or upstream context"));
    }

    #[test]
    fn ai_context_reads_complete_identity_metadata() {
        let team_id = Uuid::now_v7();
        let listener_id = Uuid::now_v7();
        let route_config_id = Uuid::now_v7();
        let mut request = Request::new(());
        let metadata = request.metadata_mut();
        metadata.insert(
            "x-flowplane-team-id",
            team_id.to_string().parse().expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-listener-id",
            listener_id.to_string().parse().expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-route-config-id",
            route_config_id.to_string().parse().expect("metadata value"),
        );

        let context = ai_context(request.metadata())
            .expect("context parse")
            .expect("context present");

        assert_eq!(context.team_id, TeamId::from(team_id));
        assert_eq!(context.listener_id, Some(ListenerId::from(listener_id)));
        assert_eq!(
            context.route_config_id,
            RouteConfigId::from(route_config_id)
        );
        assert_eq!(context.provider_id, None);
        assert_eq!(context.backend_position, None);
    }

    #[test]
    fn ai_context_reads_upstream_provider_metadata() {
        let team_id = Uuid::now_v7();
        let route_config_id = Uuid::now_v7();
        let provider_id = Uuid::now_v7();
        let mut request = Request::new(());
        let metadata = request.metadata_mut();
        metadata.insert(
            "x-flowplane-team-id",
            team_id.to_string().parse().expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-route-config-id",
            route_config_id.to_string().parse().expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-ai-provider-id",
            provider_id.to_string().parse().expect("metadata value"),
        );
        metadata.insert("x-flowplane-ai-backend-position", "0".parse().unwrap());

        let context = ai_context(request.metadata())
            .expect("context parse")
            .expect("context present");

        assert_eq!(context.team_id, TeamId::from(team_id));
        assert_eq!(context.listener_id, None);
        assert_eq!(
            context.route_config_id,
            RouteConfigId::from(route_config_id)
        );
        assert_eq!(context.provider_id, Some(AiProviderId::from(provider_id)));
        assert_eq!(context.backend_position, Some(0));
    }

    #[tokio::test]
    async fn ai_upstream_auth_injection_is_team_and_route_scoped() {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        use base64::Engine as _;

        let key = *b"12345678901234567890123456789012";
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            String::from_utf8_lossy(&key).to_string(),
        );
        let pool = fp_storage::connect(&url, 4).await.expect("connect");
        fp_storage::migrate(&pool).await.expect("migrate");
        let org = identity::create_org(&pool, &format!("org-{}", Uuid::now_v7()), "")
            .await
            .expect("org");
        let team = identity::create_team(&pool, org.id, &format!("team-{}", Uuid::now_v7()), "")
            .await
            .expect("team");
        let other_team =
            identity::create_team(&pool, org.id, &format!("team-{}", Uuid::now_v7()), "")
                .await
                .expect("other team");

        let secret_value = "Bearer selected";
        let spec = SecretSpec::GenericSecret {
            secret: base64::engine::general_purpose::STANDARD.encode(secret_value),
        };
        let plaintext = serde_json::to_vec(&spec).expect("secret json");
        let nonce = [7_u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&key).expect("cipher");
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
            .expect("encrypt");

        let secret_id = Uuid::now_v7();
        let provider_id = Uuid::now_v7();
        let route_id = Uuid::now_v7();
        let route_config_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO secrets \
             (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
             VALUES ($1, $2, $3, 'ai-key', '', 'generic_secret', $4, $5, 'default')",
        )
        .bind(secret_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .bind(ciphertext)
        .bind(nonce.to_vec())
        .execute(&pool)
        .await
        .expect("secret");
        sqlx::query(
            "INSERT INTO ai_providers \
             (id, team_id, org_id, name, kind, base_url, path_prefix, credential_secret_id, auth_header) \
             VALUES ($1, $2, $3, 'openai', 'openai', 'https://api.openai.com', '/openai', $4, 'authorization')",
        )
        .bind(provider_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .bind(secret_id)
        .execute(&pool)
        .await
        .expect("provider");
        sqlx::query(
            "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
             VALUES ($1, $2, $3, 'ai-route-routes', '{}'::jsonb)",
        )
        .bind(route_config_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .execute(&pool)
        .await
        .expect("route config");
        let route_spec = serde_json::json!({
            "listener_port": 19000,
            "path": "/v1/chat/completions",
            "backends": [{
                "provider_id": provider_id,
                "models": [],
                "model_override": "upstream-model",
                "weight": 1,
                "priority": 0
            }]
        });
        sqlx::query(
            "INSERT INTO ai_routes \
             (id, team_id, org_id, name, spec, cluster_names, route_config_name, listener_name) \
             VALUES ($1, $2, $3, 'ai-route', $4, ARRAY['ai-route-b1'], 'ai-route-routes', 'ai-route-listener')",
        )
        .bind(route_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .bind(route_spec)
        .execute(&pool)
        .await
        .expect("ai route");
        sqlx::query(
            "INSERT INTO ai_route_backends (ai_route_id, team_id, provider_id, position) \
             VALUES ($1, $2, $3, 0)",
        )
        .bind(route_id)
        .bind(team.id.as_uuid())
        .bind(provider_id)
        .execute(&pool)
        .await
        .expect("backend");

        let runtime = selected_backend_runtime(
            &pool,
            team.id,
            RouteConfigId::from(route_config_id),
            AiProviderId::from(provider_id),
            Some(0),
            Some("/v1/chat/completions?stream=true"),
        )
        .await
        .expect("runtime");
        assert_eq!(runtime.auth_header, "authorization");
        assert_eq!(runtime.auth_value, secret_value);
        assert_eq!(
            runtime.path_rewrite.as_deref(),
            Some("/openai/v1/chat/completions?stream=true")
        );
        assert_eq!(runtime.model_override.as_deref(), Some("upstream-model"));
        assert!(
            selected_backend_runtime(
                &pool,
                other_team.id,
                RouteConfigId::from(route_config_id),
                AiProviderId::from(provider_id),
                Some(0),
                Some("/v1/chat/completions"),
            )
            .await
            .is_err(),
            "provider lookup is team scoped"
        );

        let state = AiExtProcState {
            context: Some(AiExtProcContext {
                team_id: team.id,
                listener_id: None,
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: Some(AiProviderId::from(provider_id)),
                backend_position: Some(0),
            }),
            last_usage: Some(OpenAiTokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            }),
            ..Default::default()
        };
        persist_ai_usage(&pool, &state).await;
        let total_tokens: i64 = sqlx::query_scalar(
            "SELECT total_tokens FROM ai_usage_events \
             WHERE team_id = $1 AND route_config_id = $2 AND provider_id = $3",
        )
        .bind(team.id.as_uuid())
        .bind(route_config_id)
        .bind(provider_id)
        .fetch_one(&pool)
        .await
        .expect("usage event");
        assert_eq!(total_tokens, 5);

        let shadow_budget_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO ai_budgets \
             (id, team_id, org_id, name, mode, limit_units, window_seconds, provider_id, route_config_id, prompt_token_weight, completion_token_weight) \
             VALUES ($1, $2, $3, 'shadow-only', 'shadow', 1, 3600, $4, $5, 1, 1)",
        )
        .bind(shadow_budget_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .bind(provider_id)
        .bind(route_config_id)
        .execute(&pool)
        .await
        .expect("shadow budget");
        sqlx::query(
            "INSERT INTO ai_budget_counters (budget_id, team_id, window_start, used_units) \
             VALUES ($1, $2, to_timestamp(floor(extract(epoch FROM now()) / 3600) * 3600), 5)",
        )
        .bind(shadow_budget_id)
        .bind(team.id.as_uuid())
        .execute(&pool)
        .await
        .expect("shadow counter");
        assert_eq!(
            ai::exhausted_enforcing_budget(
                &pool,
                team.id,
                RouteConfigId::from(route_config_id),
                AiProviderId::from(provider_id),
            )
            .await
            .expect("shadow budget check"),
            None,
            "shadow budgets do not block requests"
        );

        let budget_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO ai_budgets \
             (id, team_id, org_id, name, mode, limit_units, window_seconds, provider_id, route_config_id, prompt_token_weight, completion_token_weight) \
             VALUES ($1, $2, $3, 'hard-stop', 'enforcing', 5, 3600, $4, $5, 1, 1)",
        )
        .bind(budget_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .bind(provider_id)
        .bind(route_config_id)
        .execute(&pool)
        .await
        .expect("budget");
        sqlx::query(
            "INSERT INTO ai_budget_counters (budget_id, team_id, window_start, used_units) \
             VALUES ($1, $2, to_timestamp(floor(extract(epoch FROM now()) / 3600) * 3600), 5)",
        )
        .bind(budget_id)
        .bind(team.id.as_uuid())
        .execute(&pool)
        .await
        .expect("counter");

        let mut blocked_state = AiExtProcState {
            context: Some(AiExtProcContext {
                team_id: team.id,
                listener_id: None,
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: Some(AiProviderId::from(provider_id)),
                backend_position: Some(0),
            }),
            ..Default::default()
        };
        let blocked = ai_request_headers_response(
            &pool,
            &mut blocked_state,
            ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders {
                        headers: Some(HeaderMap {
                            headers: vec![HeaderValue {
                                key: ":path".into(),
                                value: "/v1/chat/completions".into(),
                                raw_value: Vec::new(),
                            }],
                        }),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        )
        .await;
        let processing_response::Response::ImmediateResponse(response) =
            blocked.response.expect("blocked response")
        else {
            panic!("expected budget immediate response");
        };
        assert_eq!(response.status.expect("status").code, 429);
        assert_eq!(response.details, "flowplane_ai_budget_exceeded");
        assert_eq!(
            String::from_utf8(response.body).expect("body"),
            "AI budget \"hard-stop\" exceeded"
        );

        assert_eq!(
            ai::exhausted_enforcing_budget(
                &pool,
                other_team.id,
                RouteConfigId::from(route_config_id),
                AiProviderId::from(provider_id),
            )
            .await
            .expect("other team budget check"),
            None,
            "budget checks are team scoped"
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

    #[test]
    fn ai_ext_proc_sets_model_header_and_replaces_forced_usage_body() {
        let mut state = AiExtProcState::default();
        let response = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: br#"{"model":"gpt-5","stream":true,"messages":[]}"#.to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::RequestBody(body) = response.response.expect("response")
        else {
            panic!("expected request body response");
        };
        let common = body.response.expect("common");
        assert!(common.clear_route_cache);
        let mutation = common.header_mutation.expect("headers");
        assert_eq!(
            mutation.set_headers[0].header.as_ref().expect("header").key,
            AI_MODEL_HEADER
        );
        assert_eq!(
            mutation.set_headers[0]
                .header
                .as_ref()
                .expect("header")
                .value,
            "gpt-5"
        );
        let body_mutation = common.body_mutation.expect("body mutation");
        let Some(body_mutation::Mutation::Body(body)) = body_mutation.mutation else {
            panic!("expected replacement body");
        };
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["stream_options"]["include_usage"], true);
        assert!(state.include_usage_injected);
    }

    #[test]
    fn ai_upstream_request_body_rewrites_model_override() {
        let state = AiExtProcState {
            upstream_model_override: Some("upstream-model".into()),
            ..Default::default()
        };
        let response = ai_upstream_request_body_response(
            &state,
            ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: br#"{"model":"client-model","messages":[]}"#.to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::RequestBody(body) = response.response.expect("response")
        else {
            panic!("expected request body response");
        };
        let mutation = body
            .response
            .expect("common")
            .body_mutation
            .expect("body mutation");
        let Some(body_mutation::Mutation::Body(body)) = mutation.mutation else {
            panic!("expected replacement body");
        };
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["model"], "upstream-model");
    }

    #[test]
    fn provider_path_rewrite_uses_only_path_prefix() {
        assert_eq!(
            provider_path_rewrite(Some("/openai"), Some("/v1/chat/completions?stream=true"))
                .as_deref(),
            Some("/openai/v1/chat/completions?stream=true")
        );
        assert_eq!(
            provider_path_rewrite(None, Some("/v1/chat/completions")),
            None
        );
    }

    #[test]
    fn ai_ext_proc_rejects_malformed_request_body() {
        let mut state = AiExtProcState::default();
        let response = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: br#"{"messages":[]}"#.to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::ImmediateResponse(response) =
            response.response.expect("response")
        else {
            panic!("expected immediate response");
        };
        assert_eq!(
            response.status.expect("status").code,
            StatusCode::BadRequest as i32
        );
    }

    #[test]
    fn ai_ext_proc_strips_synthetic_usage_response_body() {
        let mut state = AiExtProcState {
            include_usage_injected: true,
            ..Default::default()
        };
        let response = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: concat!(
                            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
                            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n"
                        )
                        .as_bytes()
                        .to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::ResponseBody(body) =
            response.response.expect("response")
        else {
            panic!("expected response body response");
        };
        let body_mutation = body
            .response
            .expect("common")
            .body_mutation
            .expect("body mutation");
        let Some(body_mutation::Mutation::Body(body)) = body_mutation.mutation else {
            panic!("expected replacement body");
        };
        let body = String::from_utf8(body).expect("utf8");
        assert!(body.contains("\"content\":\"hi\""));
        assert!(!body.contains("\"usage\""));
        assert_eq!(state.last_usage.expect("usage").total_tokens, 3);
    }

    #[test]
    fn ai_ext_proc_uses_response_headers_to_skip_error_body_mutation() {
        let mut state = AiExtProcState {
            include_usage_injected: true,
            ..Default::default()
        };
        let headers = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseHeaders(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders {
                        headers: Some(HeaderMap {
                            headers: vec![
                                HeaderValue {
                                    key: ":status".into(),
                                    value: "429".into(),
                                    raw_value: Vec::new(),
                                },
                                HeaderValue {
                                    key: "content-type".into(),
                                    value: "text/event-stream".into(),
                                    raw_value: Vec::new(),
                                },
                            ],
                        }),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );
        let processing_response::Response::ResponseHeaders(_) =
            headers.response.expect("headers response")
        else {
            panic!("expected response headers response");
        };

        let body = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n".to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::ResponseBody(body) =
            body.response.expect("body response")
        else {
            panic!("expected response body response");
        };
        assert!(body.response.expect("common").body_mutation.is_none());
        assert!(state.last_usage.is_none());
    }

    #[test]
    fn ai_ext_proc_strips_split_synthetic_usage_sse() {
        let mut state = AiExtProcState {
            include_usage_injected: true,
            ..Default::default()
        };

        let first = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[],\"usage\"".to_vec(),
                        end_of_stream: false,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );
        let second = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: b":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n".to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let first = response_body_mutation(first).expect("first mutation");
        let second = response_body_mutation(second).expect("second mutation");
        assert!(String::from_utf8(first)
            .expect("utf8")
            .contains("\"content\":\"hi\""));
        assert_eq!(String::from_utf8(second).expect("utf8"), "");
        assert_eq!(state.last_usage.expect("usage").total_tokens, 3);
    }

    #[test]
    fn ai_ext_proc_caps_unfinished_sse_remainder() {
        let mut state = AiExtProcState {
            include_usage_injected: true,
            ..Default::default()
        };
        let response = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: vec![b'a'; MAX_AI_SSE_REMAINDER_BYTES + 1],
                        end_of_stream: false,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::ResponseBody(body) =
            response.response.expect("response")
        else {
            panic!("expected response body response");
        };
        assert!(body.response.expect("common").body_mutation.is_none());
        assert!(state.response_sse_remainder.is_empty());
    }

    #[test]
    fn ai_ext_proc_extracts_unary_json_usage() {
        let mut state = AiExtProcState::default();
        let response = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: br#"{"choices":[],"usage":{"prompt_tokens":4,"completion_tokens":5,"total_tokens":9}}"#.to_vec(),
                        end_of_stream: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let processing_response::Response::ResponseBody(body) =
            response.response.expect("response")
        else {
            panic!("expected response body response");
        };
        assert!(body.response.expect("common").body_mutation.is_none());
        assert_eq!(state.last_usage.expect("usage").total_tokens, 9);
    }

    fn response_body_mutation(response: ProcessingResponse) -> Option<Vec<u8>> {
        let processing_response::Response::ResponseBody(body) = response.response? else {
            return None;
        };
        let mutation = body.response?.body_mutation?.mutation?;
        match mutation {
            body_mutation::Mutation::Body(body) => Some(body),
            _ => None,
        }
    }
}
