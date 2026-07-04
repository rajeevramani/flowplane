//! Envoy ALS/ExtProc learning capture services.
//!
//! xDS injects these services only for active capture scopes. This module is intentionally
//! thin: it parses Envoy protobufs into the domain `ObservationIngest` shape and delegates all
//! tenancy, quota, merge, TTL, and counter rules to storage.

use crate::ads::TeamResolver;
use chrono::{DateTime, SecondsFormat, Utc};
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
use fp_storage::repos::{ai, ai_trace, api_lifecycle, discovery, identity, secrets};
use serde_json::{json, Map, Value};
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
    request_id: Option<String>,
    trace_id: Option<String>,
    model: Option<String>,
    request_headers_at: Option<DateTime<Utc>>,
    hops: Vec<TraceHop>,
}

/// One entry of the per-request hop timeline persisted into `ai_trace_events.hops`.
/// `origin` and `failed` are merge/derivation mechanics for the storage upsert: origin
/// decides the winner when both ExtProc streams record the same hop name, and `failed`
/// feeds the order-independent `failure_hop` derivation. Never carries request/response
/// bodies or credential values — `detail` holds ids, header *names*, and counters only.
#[derive(Debug, Clone)]
struct TraceHop {
    hop: &'static str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    outcome: &'static str,
    origin: &'static str,
    failed: bool,
    detail: Value,
}

impl TraceHop {
    fn to_json(&self) -> Value {
        json!({
            "hop": self.hop,
            "started_at": self.started_at.to_rfc3339_opts(SecondsFormat::Micros, true),
            "ended_at": self.ended_at.to_rfc3339_opts(SecondsFormat::Micros, true),
            "outcome": self.outcome,
            "origin": self.origin,
            "failed": self.failed,
            "detail": self.detail,
        })
    }
}

impl AiExtProcState {
    /// Which side of the request this ExtProc stream is: the listener-side router stream
    /// carries a listener id, the upstream-side provider stream carries provider metadata
    /// (`ai_context` admits no other shapes).
    fn trace_origin(&self) -> &'static str {
        match &self.context {
            Some(context)
                if context.provider_id.is_some() || !context.failover_chain.is_empty() =>
            {
                "upstream"
            }
            _ => "listener",
        }
    }

    fn push_hop(
        &mut self,
        hop: &'static str,
        started_at: DateTime<Utc>,
        outcome: &'static str,
        failed: bool,
        detail: Value,
    ) {
        let ended_at = Utc::now().max(started_at);
        let origin = self.trace_origin();
        self.hops.push(TraceHop {
            hop,
            started_at,
            ended_at,
            outcome,
            origin,
            failed,
            detail,
        });
    }
}

/// Extract the 32-hex trace-id field from a W3C `traceparent` header value.
fn traceparent_trace_id(value: &str) -> Option<String> {
    let mut parts = value.split('-');
    let _version = parts.next()?;
    let trace_id = parts.next()?;
    (trace_id.len() == 32
        && trace_id.bytes().all(|b| b.is_ascii_hexdigit())
        && trace_id.bytes().any(|b| b != b'0'))
    .then(|| trace_id.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AiExtProcContext {
    team_id: TeamId,
    listener_id: Option<ListenerId>,
    route_config_id: RouteConfigId,
    provider_id: Option<AiProviderId>,
    backend_position: Option<i32>,
    failover_chain: Vec<(AiProviderId, i32)>,
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
                                metrics::counter!(
                                    "fp_capture_dropped_total",
                                    "source" => "als",
                                    "reason" => status.code().to_string()
                                )
                                .increment(1);
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
                                metrics::counter!(
                                    "fp_capture_dropped_total",
                                    "source" => "ext_proc",
                                    "reason" => status.code().to_string()
                                )
                                .increment(1);
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
        let settlement = persist_ai_usage(&pool, &state).await;
        persist_ai_trace(&pool, &state, settlement).await;
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
    let provider_chain = optional_metadata_string(metadata, "x-flowplane-ai-provider-chain")?;
    let backend_position_chain =
        optional_metadata_string(metadata, "x-flowplane-ai-backend-position-chain")?;
    match (
        team_id,
        listener_id,
        route_config_id,
        provider_id,
        backend_position,
        provider_chain,
        backend_position_chain,
    ) {
        (Some(team_id), Some(listener_id), Some(route_config_id), None, None, None, None) => {
            Ok(Some(AiExtProcContext {
                team_id: TeamId::from(team_id),
                listener_id: Some(ListenerId::from(listener_id)),
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: None,
                backend_position: None,
                failover_chain: Vec::new(),
            }))
        }
        (
            Some(team_id),
            None,
            Some(route_config_id),
            Some(provider_id),
            Some(backend_position),
            None,
            None,
        ) => Ok(Some(AiExtProcContext {
            team_id: TeamId::from(team_id),
            listener_id: None,
            route_config_id: RouteConfigId::from(route_config_id),
            provider_id: Some(AiProviderId::from(provider_id)),
            backend_position: Some(backend_position),
            failover_chain: Vec::new(),
        })),
        (
            Some(team_id),
            None,
            Some(route_config_id),
            None,
            None,
            Some(provider_chain),
            Some(backend_position_chain),
        ) => Ok(Some(AiExtProcContext {
            team_id: TeamId::from(team_id),
            listener_id: None,
            route_config_id: RouteConfigId::from(route_config_id),
            provider_id: None,
            backend_position: None,
            failover_chain: parse_ai_failover_chain(&provider_chain, &backend_position_chain)?,
        })),
        (None, None, None, None, None, None, None) => Ok(None),
        _ => Err(Status::invalid_argument(
            "AI processor metadata must include either router or upstream context",
        )),
    }
}

fn parse_ai_failover_chain(
    provider_chain: &str,
    backend_position_chain: &str,
) -> Result<Vec<(AiProviderId, i32)>, Status> {
    let providers = provider_chain
        .split(',')
        .map(|raw| {
            Uuid::parse_str(raw)
                .map(AiProviderId::from)
                .map_err(|_| Status::invalid_argument("invalid AI provider chain metadata"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let positions = backend_position_chain
        .split(',')
        .map(|raw| {
            raw.parse::<i32>()
                .map_err(|_| Status::invalid_argument("invalid AI backend chain metadata"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if providers.is_empty() || providers.len() != positions.len() {
        return Err(Status::invalid_argument(
            "AI failover chain metadata must include matching providers and positions",
        ));
    }
    Ok(providers.into_iter().zip(positions).collect())
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

fn optional_metadata_string(
    metadata: &MetadataMap,
    key: &'static str,
) -> Result<Option<String>, Status> {
    metadata
        .get(key)
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|_| Status::invalid_argument(format!("invalid capture metadata {key}")))
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
        note_ai_request_identity(state, &map);
        return ai_request_headers_response(pool, state, request).await;
    }
    if matches!(
        request.request,
        Some(processing_request::Request::RequestBody(_))
    ) && state
        .context
        .as_ref()
        .and_then(|context| context.provider_id)
        .is_some()
    {
        return ai_upstream_request_body_response(state, request);
    }
    ai_response(state, request)
}

/// Record the request-identity facts the trace row is keyed and correlated by: the
/// server-owned `x-request-id` (post-HCM-mutation, so both streams observe the same value),
/// the inbound `traceparent` trace-id when present, and the model routing hint.
fn note_ai_request_identity(state: &mut AiExtProcState, map: &Map<String, Value>) {
    state.request_headers_at = Some(Utc::now());
    state.request_id = header_value(map, "x-request-id");
    if state.trace_id.is_none() {
        state.trace_id =
            header_value(map, "traceparent").and_then(|value| traceparent_trace_id(&value));
    }
    if state.model.is_none() {
        state.model = header_value(map, AI_MODEL_HEADER);
    }
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
        Ok(body) => request_body_replacement_response(body),
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
            note_ai_upstream_response(state);
            response_headers_response(CommonResponse {
                status: common_response::ResponseStatus::Continue as i32,
                ..Default::default()
            })
        }
        processing_request::Request::RequestBody(body) => {
            match prepare_openai_chat_request(&body.body) {
                Ok(prepared) => {
                    state.include_usage_injected = prepared.include_usage_injected;
                    state.model = Some(prepared.model.clone());
                    let mut common = CommonResponse {
                        status: common_response::ResponseStatus::Continue as i32,
                        header_mutation: Some(HeaderMutation {
                            set_headers: vec![mutation_header_value(
                                AI_MODEL_HEADER.into(),
                                prepared.model,
                            )],
                            remove_headers: Vec::new(),
                        }),
                        clear_route_cache: true,
                        ..Default::default()
                    };
                    if prepared.include_usage_injected {
                        common = add_request_body_replacement(common, prepared.body);
                    }
                    request_body_response(common)
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
    let Some(context) = state.context.clone() else {
        return request_headers_response(remove_internal_model_header());
    };
    let Some((provider_id, backend_position)) = selected_upstream_backend(&context, &request)
    else {
        return ai_listener_request_headers_response(pool, state, request, context).await;
    };
    let budget_started = Utc::now();
    match ai::exhausted_enforcing_budget(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
    )
    .await
    {
        Ok(Some(name)) => {
            state.push_hop(
                "budget",
                budget_started,
                "rejected",
                true,
                json!({"mode": "enforcing", "budget": name}),
            );
            metrics::counter!(
                "fp_ai_budget_threshold_crossings_total",
                "mode" => "enforcing",
                "result" => "exhausted"
            )
            .increment(1);
            return immediate_response_with_details(
                429,
                format!("AI budget \"{name}\" exceeded"),
                "flowplane_ai_budget_exceeded",
            );
        }
        Ok(None) => {
            state.push_hop(
                "budget",
                budget_started,
                "allowed",
                false,
                json!({"mode": "enforcing"}),
            );
        }
        Err(err) => {
            state.push_hop(
                "budget",
                budget_started,
                "check_failed",
                true,
                json!({"mode": "enforcing"}),
            );
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
    let credential_started = Utc::now();
    match selected_backend_runtime(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
        Some(backend_position),
        request_path.as_deref(),
    )
    .await
    {
        Ok(runtime) => {
            if let Some(state_context) = state.context.as_mut() {
                state_context.provider_id = Some(provider_id);
                state_context.backend_position = Some(backend_position);
            }
            state.push_hop(
                "credential_injection",
                credential_started,
                "injected",
                false,
                json!({
                    "provider_id": provider_id,
                    "backend_position": backend_position,
                    "auth_header": runtime.auth_header.clone(),
                }),
            );
            state.upstream_model_override = runtime.model_override;
            let mut set_headers = vec![mutation_header_value(
                runtime.auth_header,
                runtime.auth_value,
            )];
            if let Some(path) = runtime.path_rewrite {
                set_headers.push(mutation_header_value(":path".into(), path));
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
        Err(_) => {
            state.push_hop(
                "credential_injection",
                credential_started,
                "unavailable",
                true,
                json!({"provider_id": provider_id, "backend_position": backend_position}),
            );
            immediate_response(500, "AI provider credential unavailable".into())
        }
    }
}

fn selected_upstream_backend(
    context: &AiExtProcContext,
    request: &ProcessingRequest,
) -> Option<(AiProviderId, i32)> {
    if let (Some(provider_id), Some(backend_position)) =
        (context.provider_id, context.backend_position)
    {
        return Some((provider_id, backend_position));
    }
    if context.failover_chain.is_empty() {
        return None;
    }
    let attempt = match &request.request {
        Some(processing_request::Request::RequestHeaders(headers)) => {
            let map = headers_from_header_map(headers.headers.as_ref());
            header_value(&map, "x-envoy-attempt-count")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1)
        }
        _ => 1,
    };
    let idx = attempt
        .saturating_sub(1)
        .min(context.failover_chain.len().saturating_sub(1));
    context.failover_chain.get(idx).copied()
}

async fn ai_listener_request_headers_response(
    pool: &sqlx::PgPool,
    state: &mut AiExtProcState,
    request: ProcessingRequest,
    context: AiExtProcContext,
) -> ProcessingResponse {
    // Listener-side hops: the request reached the AI listener and its route/team identity
    // was reconstructed from CP-attached gRPC metadata; no per-request authn filter runs on
    // AI listeners today, so the auth hop truthfully records `not_configured`.
    let route_started = state.request_headers_at.unwrap_or_else(Utc::now);
    let route_detail = json!({
        "listener_id": context.listener_id,
        "route_config_id": context.route_config_id,
        "model": state.model.clone(),
    });
    state.push_hop("route_match", route_started, "matched", false, route_detail);
    let auth_at = Utc::now();
    state.push_hop("auth", auth_at, "not_configured", false, json!({}));
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
    let budget_started = Utc::now();
    match ai::exhausted_enforcing_budget(
        pool,
        context.team_id,
        context.route_config_id,
        provider_id,
    )
    .await
    {
        Ok(Some(name)) => {
            state.push_hop(
                "budget",
                budget_started,
                "rejected",
                true,
                json!({"mode": "enforcing", "budget": name}),
            );
            metrics::counter!(
                "fp_ai_budget_threshold_crossings_total",
                "mode" => "enforcing",
                "result" => "exhausted"
            )
            .increment(1);
            return immediate_response_with_details(
                429,
                format!("AI budget \"{name}\" exceeded"),
                "flowplane_ai_budget_exceeded",
            );
        }
        Ok(None) => {
            state.push_hop(
                "budget",
                budget_started,
                "allowed",
                false,
                json!({"mode": "enforcing"}),
            );
        }
        Err(err) => {
            state.push_hop(
                "budget",
                budget_started,
                "check_failed",
                true,
                json!({"mode": "enforcing"}),
            );
            tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to check AI budget: {}", err.message);
            return immediate_response(500, "AI budget check unavailable".into());
        }
    }
    let credential_started = Utc::now();
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
            state.push_hop(
                "credential_injection",
                credential_started,
                "injected",
                false,
                json!({
                    "provider_id": provider_id,
                    "backend_position": backend_position,
                    "auth_header": runtime.auth_header.clone(),
                }),
            );
            state.upstream_model_override = runtime.model_override;
            let mut set_headers = vec![mutation_header_value(
                runtime.auth_header,
                runtime.auth_value,
            )];
            if let Some(path) = runtime.path_rewrite {
                set_headers.push(mutation_header_value(":path".into(), path));
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
        Err(_) => {
            state.push_hop(
                "credential_injection",
                credential_started,
                "unavailable",
                true,
                json!({"provider_id": provider_id, "backend_position": backend_position}),
            );
            immediate_response(500, "AI provider credential unavailable".into())
        }
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

        if state.include_usage_injected && !complete.is_empty() {
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
    if let Some(context) = &state.context {
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

/// Persist token usage and settle budgets. Returns `Some(true)` when the usage event was
/// recorded and settled, `Some(false)` when persistence failed, and `None` when this stream
/// had no attributable usage to persist — feeds the trace `usage` hop's settlement outcome.
async fn persist_ai_usage(pool: &sqlx::PgPool, state: &AiExtProcState) -> Option<bool> {
    let (Some(context), Some(usage)) = (&state.context, state.last_usage) else {
        return None;
    };
    let provider_id = context.provider_id?;
    match ai::record_usage_event_and_settle_budgets(
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
        Ok(()) => Some(true),
        Err(err) => {
            tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to persist AI usage: {}", err.message);
            Some(false)
        }
    }
}

/// Close the `upstream` hop when the upstream-side stream observes response headers. The
/// listener-side stream records the final status in the row's top-level `status_code`
/// column instead, so the hop stays owned by exactly one origin.
fn note_ai_upstream_response(state: &mut AiExtProcState) {
    if state.trace_origin() != "upstream" {
        return;
    }
    let started = state.request_headers_at.unwrap_or_else(Utc::now);
    let status = state.response_status;
    let failed = status.is_none_or(|status| status >= 400);
    let detail = json!({
        "provider_id": state.context.as_ref().and_then(|context| context.provider_id),
        "backend_position": state.context.as_ref().and_then(|context| context.backend_position),
        "status": status,
        "latency_ms": (Utc::now() - started).num_milliseconds(),
    });
    let outcome = if failed { "error" } else { "ok" };
    state.push_hop("upstream", started, outcome, failed, detail);
}

/// Persist this stream's trace contribution — strictly best-effort: runs after the HTTP
/// exchange has already completed (the ExtProc stream is closed), and any error is logged
/// and counted, never surfaced. The listener stream owns the row identity columns; the
/// upstream stream merges provider-side hops into the same `(team_id, request_id)` row.
async fn persist_ai_trace(pool: &sqlx::PgPool, state: &AiExtProcState, settlement: Option<bool>) {
    let Some(context) = &state.context else {
        return;
    };
    let Some(request_id) = state.request_id.clone() else {
        return;
    };
    let is_upstream = state.trace_origin() == "upstream";
    let mut hops: Vec<Value> = state.hops.iter().map(TraceHop::to_json).collect();
    if is_upstream {
        if let Some(usage) = state.last_usage {
            let now = Utc::now();
            hops.push(
                TraceHop {
                    hop: "usage",
                    started_at: now,
                    ended_at: now,
                    outcome: match settlement {
                        Some(true) => "settled",
                        Some(false) => "settle_failed",
                        None => "extracted",
                    },
                    origin: "upstream",
                    failed: false,
                    detail: json!({
                        "prompt_tokens": usage.prompt_tokens,
                        "completion_tokens": usage.completion_tokens,
                        "total_tokens": usage.total_tokens,
                    }),
                }
                .to_json(),
            );
        }
    } else if let Some(model) = &state.model {
        // The model routing header may only be known after the request body was parsed;
        // backfill the route_match hop detail recorded at header time.
        for hop in &mut hops {
            if hop.get("hop").and_then(Value::as_str) == Some("route_match") {
                if let Some(detail) = hop.get_mut("detail") {
                    if detail.get("model").is_none_or(Value::is_null) {
                        detail["model"] = Value::String(model.clone());
                    }
                }
            }
        }
    }
    let event = ai_trace::AiTraceEventUpsert {
        team_id: context.team_id,
        request_id,
        trace_id: if is_upstream {
            None
        } else {
            state.trace_id.clone()
        },
        route_config_id: context.route_config_id,
        listener_id: context.listener_id,
        provider_id: context.provider_id,
        model: if is_upstream {
            None
        } else {
            state.model.clone()
        },
        status_code: if is_upstream {
            None
        } else {
            state.response_status
        },
        hops: Value::Array(hops),
    };
    if let Err(err) = ai_trace::upsert_trace_event(pool, &event).await {
        metrics::counter!("fp_ai_trace_dropped_total", "reason" => "db").increment(1);
        tracing::debug!(team = %context.team_id, route_config = %context.route_config_id, "failed to persist AI trace event: {}", err.message);
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

fn request_body_replacement_response(body: Vec<u8>) -> ProcessingResponse {
    request_body_response(add_request_body_replacement(
        CommonResponse {
            status: common_response::ResponseStatus::Continue as i32,
            ..Default::default()
        },
        body,
    ))
}

fn add_request_body_replacement(mut common: CommonResponse, body: Vec<u8>) -> CommonResponse {
    common.body_mutation = Some(BodyMutation {
        mutation: Some(body_mutation::Mutation::Body(body)),
    });
    let mutation = common
        .header_mutation
        .get_or_insert_with(HeaderMutation::default);
    if !mutation
        .remove_headers
        .iter()
        .any(|header| header.eq_ignore_ascii_case("content-length"))
    {
        mutation.remove_headers.push("content-length".into());
    }
    common
}

fn mutation_header_value(key: String, value: String) -> HeaderValueOption {
    HeaderValueOption {
        header: Some(HeaderValue {
            key,
            value: String::new(),
            raw_value: value.into_bytes(),
        }),
        append_action: header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
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
        assert!(context.failover_chain.is_empty());
    }

    #[test]
    fn ai_context_reads_upstream_failover_chain_metadata() {
        let team_id = Uuid::now_v7();
        let route_config_id = Uuid::now_v7();
        let provider_a = Uuid::now_v7();
        let provider_b = Uuid::now_v7();
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
            "x-flowplane-ai-provider-chain",
            format!("{provider_a},{provider_b}")
                .parse()
                .expect("metadata value"),
        );
        metadata.insert(
            "x-flowplane-ai-backend-position-chain",
            "0,1".parse().expect("metadata value"),
        );

        let context = ai_context(request.metadata())
            .expect("context parse")
            .expect("context present");

        assert_eq!(context.team_id, TeamId::from(team_id));
        assert_eq!(
            context.route_config_id,
            RouteConfigId::from(route_config_id)
        );
        assert_eq!(context.provider_id, None);
        assert_eq!(
            context.failover_chain,
            vec![
                (AiProviderId::from(provider_a), 0),
                (AiProviderId::from(provider_b), 1)
            ]
        );
    }

    #[tokio::test]
    async fn ai_upstream_auth_injection_is_team_and_route_scoped() {
        let _guard = crate::snapshot::ENV_LOCK.lock().await;
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        use base64::Engine as _;

        let key = *b"12345678901234567890123456789012";
        std::env::set_var("FLOWPLANE_SECRET_ENCRYPTION_KEY_ID", "default");
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
                failover_chain: Vec::new(),
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
                failover_chain: Vec::new(),
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
                .raw_value,
            b"gpt-5"
        );
        assert_eq!(mutation.remove_headers, vec!["content-length"]);
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
        let common = body.response.expect("common");
        assert_eq!(
            common.header_mutation.expect("headers").remove_headers,
            vec!["content-length"]
        );
        let mutation = common.body_mutation.expect("body mutation");
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
    fn ai_ext_proc_forwards_complete_sse_event_when_usage_injected() {
        let mut state = AiExtProcState {
            include_usage_injected: true,
            response_content_type: Some("text/event-stream".into()),
            ..Default::default()
        };
        let response = ai_response(
            &mut state,
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                        body: b"data: {\"choices\":[{\"delta\":{\"content\":\"partial-stream\"}}]}\n\n".to_vec(),
                        end_of_stream: false,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let body = response_body_mutation(response).expect("body mutation");
        assert!(String::from_utf8(body)
            .expect("utf8")
            .contains("partial-stream"));
        assert!(state.last_usage.is_none());
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

    fn header_map(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect()
    }

    fn listener_trace_context(team_id: TeamId) -> AiExtProcContext {
        AiExtProcContext {
            team_id,
            listener_id: Some(ListenerId::from(Uuid::now_v7())),
            route_config_id: RouteConfigId::from(Uuid::now_v7()),
            provider_id: None,
            backend_position: None,
            failover_chain: Vec::new(),
        }
    }

    fn upstream_trace_context(team_id: TeamId) -> AiExtProcContext {
        AiExtProcContext {
            team_id,
            listener_id: None,
            route_config_id: RouteConfigId::from(Uuid::now_v7()),
            provider_id: Some(AiProviderId::from(Uuid::now_v7())),
            backend_position: Some(0),
            failover_chain: Vec::new(),
        }
    }

    #[test]
    fn traceparent_trace_id_extracts_valid_ids_only() {
        assert_eq!(
            traceparent_trace_id("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
                .as_deref(),
            Some("0af7651916cd43dd8448eb211c80319c")
        );
        assert_eq!(
            traceparent_trace_id("00-0AF7651916CD43DD8448EB211C80319C-b7ad6b7169203331-01")
                .as_deref(),
            Some("0af7651916cd43dd8448eb211c80319c"),
            "trace id is normalized to lowercase"
        );
        assert_eq!(traceparent_trace_id("not-a-traceparent"), None);
        assert_eq!(
            traceparent_trace_id("00-00000000000000000000000000000000-b7ad6b7169203331-01"),
            None,
            "all-zero trace id is invalid per W3C"
        );
        assert_eq!(traceparent_trace_id("00-shorttrace-b7ad-01"), None);
    }

    #[test]
    fn note_ai_request_identity_captures_request_id_trace_id_and_model() {
        let mut state = AiExtProcState::default();
        note_ai_request_identity(
            &mut state,
            &header_map(&[
                ("x-request-id", "req-abc"),
                (
                    "traceparent",
                    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
                ),
                (AI_MODEL_HEADER, "gpt-5"),
            ]),
        );
        assert_eq!(state.request_id.as_deref(), Some("req-abc"));
        assert_eq!(
            state.trace_id.as_deref(),
            Some("0af7651916cd43dd8448eb211c80319c")
        );
        assert_eq!(state.model.as_deref(), Some("gpt-5"));
        assert!(state.request_headers_at.is_some());

        let mut bare = AiExtProcState::default();
        note_ai_request_identity(&mut bare, &header_map(&[("x-request-id", "req-xyz")]));
        assert_eq!(
            bare.trace_id, None,
            "absent traceparent leaves trace_id unset"
        );
        assert_eq!(bare.model, None);
    }

    #[test]
    fn push_hop_stamps_origin_and_monotonic_hop_window() {
        let team_id = TeamId::from(Uuid::now_v7());
        let mut listener = AiExtProcState {
            context: Some(listener_trace_context(team_id)),
            ..Default::default()
        };
        let started = Utc::now();
        listener.push_hop("auth", started, "not_configured", false, json!({}));
        assert_eq!(listener.hops.len(), 1);
        let hop = &listener.hops[0];
        assert_eq!(hop.hop, "auth");
        assert_eq!(hop.origin, "listener");
        assert!(!hop.failed);
        assert!(hop.started_at <= hop.ended_at);

        let mut upstream = AiExtProcState {
            context: Some(upstream_trace_context(team_id)),
            ..Default::default()
        };
        upstream.push_hop("budget", Utc::now(), "rejected", true, json!({}));
        assert_eq!(upstream.hops[0].origin, "upstream");
        assert!(upstream.hops[0].failed);

        let serialized = upstream.hops[0].to_json();
        assert_eq!(serialized["hop"], "budget");
        assert_eq!(serialized["outcome"], "rejected");
        assert_eq!(serialized["origin"], "upstream");
        assert_eq!(serialized["failed"], true);
        assert!(
            serialized["started_at"].as_str().unwrap() <= serialized["ended_at"].as_str().unwrap()
        );
    }

    #[test]
    fn upstream_response_hop_maps_status_to_outcome_and_skips_listener_stream() {
        let team_id = TeamId::from(Uuid::now_v7());
        let mut ok = AiExtProcState {
            context: Some(upstream_trace_context(team_id)),
            response_status: Some(200),
            request_headers_at: Some(Utc::now()),
            ..Default::default()
        };
        note_ai_upstream_response(&mut ok);
        assert_eq!(ok.hops.len(), 1);
        assert_eq!(ok.hops[0].hop, "upstream");
        assert_eq!(ok.hops[0].outcome, "ok");
        assert!(!ok.hops[0].failed);
        assert_eq!(ok.hops[0].detail["status"], 200);

        let mut error = AiExtProcState {
            context: Some(upstream_trace_context(team_id)),
            response_status: Some(500),
            ..Default::default()
        };
        note_ai_upstream_response(&mut error);
        assert_eq!(error.hops[0].outcome, "error");
        assert!(error.hops[0].failed);

        let mut listener = AiExtProcState {
            context: Some(listener_trace_context(team_id)),
            response_status: Some(200),
            ..Default::default()
        };
        note_ai_upstream_response(&mut listener);
        assert!(
            listener.hops.is_empty(),
            "the listener stream owns status_code, not the upstream hop"
        );
    }

    #[tokio::test]
    async fn ai_trace_two_stream_capture_persists_merged_redacted_row() {
        let _guard = crate::snapshot::ENV_LOCK.lock().await;
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        use base64::Engine as _;

        let key = *b"12345678901234567890123456789012";
        std::env::set_var("FLOWPLANE_SECRET_ENCRYPTION_KEY_ID", "default");
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

        let secret_value = "Bearer fp-trace-secret-value";
        let spec = SecretSpec::GenericSecret {
            secret: base64::engine::general_purpose::STANDARD.encode(secret_value),
        };
        let plaintext = serde_json::to_vec(&spec).expect("secret json");
        let nonce = [9_u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&key).expect("cipher");
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
            .expect("encrypt");

        let secret_id = Uuid::now_v7();
        let provider_id = Uuid::now_v7();
        let route_id = Uuid::now_v7();
        let route_config_id = Uuid::now_v7();
        let listener_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO secrets \
             (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
             VALUES ($1, $2, $3, 'ai-trace-key', '', 'generic_secret', $4, $5, 'default')",
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
             VALUES ($1, $2, $3, 'openai-trace', 'openai', 'https://api.openai.com', NULL, $4, 'authorization')",
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
             VALUES ($1, $2, $3, 'ai-trace-routes', '{}'::jsonb)",
        )
        .bind(route_config_id)
        .bind(team.id.as_uuid())
        .bind(org.id.as_uuid())
        .execute(&pool)
        .await
        .expect("route config");
        let route_spec = serde_json::json!({
            "listener_port": 19100,
            "path": "/v1/chat/completions",
            "backends": [{
                "provider_id": provider_id,
                "models": [],
                "weight": 1,
                "priority": 0
            }]
        });
        sqlx::query(
            "INSERT INTO ai_routes \
             (id, team_id, org_id, name, spec, cluster_names, route_config_name, listener_name) \
             VALUES ($1, $2, $3, 'ai-trace-route', $4, ARRAY['ai-trace-b1'], 'ai-trace-routes', 'ai-trace-listener')",
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

        let request_id = Uuid::now_v7().to_string();
        let traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let prompt_marker = format!("fp-trace-prompt-{}", Uuid::now_v7());
        let request_headers = |extra: &[(&str, &str)]| {
            let mut headers = vec![
                (":path", "/v1/chat/completions"),
                ("x-request-id", request_id.as_str()),
                (AI_MODEL_HEADER, "gpt-5"),
            ];
            headers.extend_from_slice(extra);
            ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(
                    envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders {
                        headers: Some(HeaderMap {
                            headers: headers
                                .into_iter()
                                .map(|(key, value)| HeaderValue {
                                    key: key.into(),
                                    value: value.into(),
                                    raw_value: Vec::new(),
                                })
                                .collect(),
                        }),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
        };
        let response_headers = ProcessingRequest {
            request: Some(processing_request::Request::ResponseHeaders(
                envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders {
                    headers: Some(HeaderMap {
                        headers: vec![
                            HeaderValue {
                                key: ":status".into(),
                                value: "200".into(),
                                raw_value: Vec::new(),
                            },
                            HeaderValue {
                                key: "content-type".into(),
                                value: "application/json".into(),
                                raw_value: Vec::new(),
                            },
                        ],
                    }),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let response_body = ProcessingRequest {
            request: Some(processing_request::Request::ResponseBody(
                envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                    body: br#"{"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}"#.to_vec(),
                    end_of_stream: true,
                    ..Default::default()
                },
            )),
            ..Default::default()
        };

        // Listener-side stream: route_match/auth (+ single-backend budget/credential) hops.
        let mut listener_state = AiExtProcState {
            context: Some(AiExtProcContext {
                team_id: team.id,
                listener_id: Some(ListenerId::from(listener_id)),
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: None,
                backend_position: None,
                failover_chain: Vec::new(),
            }),
            ..Default::default()
        };
        ai_response_with_pool(
            &pool,
            &mut listener_state,
            request_headers(&[("traceparent", traceparent)]),
        )
        .await;
        let request_body = ProcessingRequest {
            request: Some(processing_request::Request::RequestBody(
                envoy_types::pb::envoy::service::ext_proc::v3::HttpBody {
                    body: format!(
                        r#"{{"model":"gpt-5","messages":[{{"role":"user","content":"{prompt_marker}"}}]}}"#
                    )
                    .into_bytes(),
                    end_of_stream: true,
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        ai_response_with_pool(&pool, &mut listener_state, request_body).await;
        ai_response_with_pool(&pool, &mut listener_state, response_headers.clone()).await;
        ai_response_with_pool(&pool, &mut listener_state, response_body.clone()).await;
        let settlement = persist_ai_usage(&pool, &listener_state).await;
        assert_eq!(
            settlement, None,
            "the listener stream has no attributed usage"
        );
        persist_ai_trace(&pool, &listener_state, settlement).await;

        // Upstream-side stream: budget/credential/upstream/usage hops merged into the row.
        let mut upstream_state = AiExtProcState {
            context: Some(AiExtProcContext {
                team_id: team.id,
                listener_id: None,
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: Some(AiProviderId::from(provider_id)),
                backend_position: Some(0),
                failover_chain: Vec::new(),
            }),
            ..Default::default()
        };
        ai_response_with_pool(
            &pool,
            &mut upstream_state,
            request_headers(&[("traceparent", traceparent)]),
        )
        .await;
        ai_response_with_pool(&pool, &mut upstream_state, response_headers).await;
        ai_response_with_pool(&pool, &mut upstream_state, response_body).await;
        let settlement = persist_ai_usage(&pool, &upstream_state).await;
        assert_eq!(
            settlement,
            Some(true),
            "usage settles on the upstream stream"
        );
        persist_ai_trace(&pool, &upstream_state, settlement).await;

        // Exactly one merged row with the full hop timeline and no sensitive strings.
        let rows = fp_storage::repos::ai_trace::list_trace_events(
            &pool,
            team.id,
            fp_storage::repos::ai_trace::AiTraceQuery {
                request_id: Some(&request_id),
                trace_id: None,
                limit: 10,
            },
        )
        .await
        .expect("list trace events");
        assert_eq!(rows.len(), 1, "both streams merged into one trace row");
        let row = &rows[0];
        assert_eq!(row.status_code, Some(200));
        assert_eq!(row.failure_hop, None);
        assert_eq!(row.model.as_deref(), Some("gpt-5"));
        assert_eq!(row.listener_id, Some(ListenerId::from(listener_id)));
        assert_eq!(row.provider_id, Some(AiProviderId::from(provider_id)));
        assert_eq!(
            row.trace_id.as_deref(),
            Some("0af7651916cd43dd8448eb211c80319c")
        );
        let hops = row.hops.as_array().expect("hops array");
        let names: Vec<&str> = hops
            .iter()
            .map(|hop| hop["hop"].as_str().expect("hop name"))
            .collect();
        for expected in [
            "route_match",
            "auth",
            "budget",
            "credential_injection",
            "upstream",
            "usage",
        ] {
            assert_eq!(
                names.iter().filter(|name| **name == expected).count(),
                1,
                "expected exactly one {expected} hop, got {names:?}"
            );
        }
        for hop in hops {
            let started = hop["started_at"].as_str().expect("started_at");
            let ended = hop["ended_at"].as_str().expect("ended_at");
            assert!(started <= ended, "hop {} window inverted", hop["hop"]);
        }
        let usage_hop = hops
            .iter()
            .find(|hop| hop["hop"] == "usage")
            .expect("usage hop");
        assert_eq!(usage_hop["outcome"], "settled");
        assert_eq!(usage_hop["detail"]["total_tokens"], 5);
        let auth_hop = hops
            .iter()
            .find(|hop| hop["hop"] == "auth")
            .expect("auth hop");
        assert_eq!(auth_hop["outcome"], "not_configured");
        let upstream_hop = hops
            .iter()
            .find(|hop| hop["hop"] == "upstream")
            .expect("upstream hop");
        assert_eq!(upstream_hop["detail"]["status"], 200);

        // Column-level sensitive scan: neither the prompt nor the secret value anywhere.
        let row_text: String = sqlx::query_scalar(
            "SELECT row_to_json(t)::text FROM \
             (SELECT * FROM ai_trace_events WHERE team_id = $1 AND request_id = $2) t",
        )
        .bind(team.id.as_uuid())
        .bind(&request_id)
        .fetch_one(&pool)
        .await
        .expect("row json");
        assert!(
            !row_text.contains(secret_value),
            "credential value must never appear in the trace row"
        );
        assert!(
            !row_text.contains(&prompt_marker),
            "prompt content must never appear in the trace row"
        );

        // Cross-team scoping: the other team sees nothing for this request id.
        let foreign = fp_storage::repos::ai_trace::list_trace_events(
            &pool,
            other_team.id,
            fp_storage::repos::ai_trace::AiTraceQuery {
                request_id: Some(&request_id),
                trace_id: None,
                limit: 10,
            },
        )
        .await
        .expect("foreign list");
        assert!(foreign.is_empty());

        // Absent traceparent leaves trace_id null (AC 9, negative half).
        let bare_request_id = Uuid::now_v7().to_string();
        let mut bare_state = AiExtProcState {
            context: Some(AiExtProcContext {
                team_id: team.id,
                listener_id: Some(ListenerId::from(listener_id)),
                route_config_id: RouteConfigId::from(route_config_id),
                provider_id: None,
                backend_position: None,
                failover_chain: Vec::new(),
            }),
            ..Default::default()
        };
        let bare_headers = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(
                envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders {
                    headers: Some(HeaderMap {
                        headers: vec![
                            HeaderValue {
                                key: ":path".into(),
                                value: "/v1/chat/completions".into(),
                                raw_value: Vec::new(),
                            },
                            HeaderValue {
                                key: "x-request-id".into(),
                                value: bare_request_id.clone(),
                                raw_value: Vec::new(),
                            },
                            HeaderValue {
                                key: AI_MODEL_HEADER.into(),
                                value: "gpt-5".into(),
                                raw_value: Vec::new(),
                            },
                        ],
                    }),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        ai_response_with_pool(&pool, &mut bare_state, bare_headers).await;
        persist_ai_trace(&pool, &bare_state, None).await;
        let bare_rows = fp_storage::repos::ai_trace::list_trace_events(
            &pool,
            team.id,
            fp_storage::repos::ai_trace::AiTraceQuery {
                request_id: Some(&bare_request_id),
                trace_id: None,
                limit: 10,
            },
        )
        .await
        .expect("bare list");
        assert_eq!(bare_rows.len(), 1);
        assert_eq!(bare_rows[0].trace_id, None);
    }

    #[tokio::test]
    async fn ai_trace_persistence_failure_is_swallowed_best_effort() {
        let _guard = crate::snapshot::ENV_LOCK.lock().await;
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fp_storage::connect(&url, 4).await.expect("connect");
        fp_storage::migrate(&pool).await.expect("migrate");
        // A team that does not exist violates the FK — the write fails, the call must not.
        let mut state = AiExtProcState {
            context: Some(AiExtProcContext {
                team_id: TeamId::from(Uuid::now_v7()),
                listener_id: Some(ListenerId::from(Uuid::now_v7())),
                route_config_id: RouteConfigId::from(Uuid::now_v7()),
                provider_id: None,
                backend_position: None,
                failover_chain: Vec::new(),
            }),
            request_id: Some(Uuid::now_v7().to_string()),
            ..Default::default()
        };
        state.push_hop("auth", Utc::now(), "not_configured", false, json!({}));
        persist_ai_trace(&pool, &state, None).await;
    }
}
