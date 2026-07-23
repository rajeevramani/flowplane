use crate::state::AppState;
use crate::{error::ApiError, resources::resolve_team};
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{body::Body, Json};
use fp_core::mcp_declarations::{StaticToolDecl, STATIC_TOOL_DECLS};
use fp_core::services::{deny_to_error, record_authz_denial};
use fp_core::{check_resource_access, Decision, PrincipalCtx};
use fp_domain::api_lifecycle::{ApiRouteBinding, ApiTool};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::gateway::cluster::ClusterSpec;
use fp_domain::gateway::listener::ListenerSpec;
use fp_domain::gateway::route_config::RouteConfigSpec;
use fp_domain::{AgentKind, DomainError, DomainResult, ErrorCode, RequestId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const PREFERRED_VERSION: &str = "2025-11-25";
const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-03-26"];
const SESSION_TTL: Duration = Duration::from_secs(3600);

static SESSIONS: LazyLock<Mutex<HashMap<String, McpSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct McpSession {
    principal: String,
    principal_kind: &'static str,
    org_id: Option<uuid::Uuid>,
    last_seen: Instant,
    // Requests currently executing against this session (validated, not yet finished).
    // Cleanup never reaps a session with in-flight requests: a request validated at the
    // TTL boundary must still find its session when its authorization succeeds and it
    // stamps. This is presence, not lifetime — it does not move last_seen, so denied
    // traffic still cannot extend the session beyond its in-flight window.
    in_flight: u32,
    // Display metadata for the team-scoped status/connections endpoints only.
    // Authorization is re-evaluated per request and never reads this map.
    team_activity: HashMap<uuid::Uuid, TeamActivity>,
}

/// Decrements a session's in-flight counter on drop (cancellation-safe: fires even if the
/// request future is dropped mid-await). Created by `validate_session`.
struct InFlightGuard {
    session_id: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Some(session) = sessions().get_mut(&self.session_id) {
            session.in_flight = session.in_flight.saturating_sub(1);
        }
    }
}

#[derive(Clone)]
struct TeamActivity {
    // Random per (session, team): teams must never receive a shared identifier
    // for one session, or the endpoint becomes a cross-team correlation channel.
    team_connection_id: uuid::Uuid,
    first_authorized_at: Instant,
    last_authorized_at: Instant,
}

fn team_activity_stale(activity: &TeamActivity, now: Instant, ttl: Duration) -> bool {
    now.duration_since(activity.last_authorized_at) > ttl
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct McpStatusView {
    pub transport: String,
    pub preferred_protocol_version: String,
    pub supported_protocol_versions: Vec<String>,
    pub session_ttl_seconds: u64,
    pub active_sessions: usize,
    pub static_tool_count: usize,
    pub dynamic_enabled_tool_count: usize,
    pub tools_list_changed: bool,
    pub sse_enabled: bool,
    pub resources_enabled: bool,
    pub prompts_enabled: bool,
    pub api_invocation_mode: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct McpConnectionView {
    pub connection_id: uuid::Uuid,
    pub principal_kind: String,
    pub transport: String,
    pub sse: bool,
    pub age_seconds: u64,
    pub idle_seconds: u64,
}

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    jsonrpc: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    id: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Value,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    // JSON-RPC transport: a malformed body is a Parse error (-32700), not the
    // REST error envelope and not axum's bare 422.
    let req: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => return rpc_error(None, -32700, "Parse error", rid, "validation").into_response(),
    };
    if let Err(error) = check_origin(&headers) {
        return rpc_error(req.id, -32600, error, rid, "origin").into_response();
    }
    if req.jsonrpc.as_deref() != Some("2.0") {
        return rpc_error(req.id, -32600, "jsonrpc must be \"2.0\"", rid, "validation")
            .into_response();
    }
    let version = match requested_version(&headers, &req) {
        Ok(version) => version,
        Err(error) => {
            return rpc_error(req.id, -32600, error, rid, "protocol").into_response();
        }
    };
    let principal = principal_key(&ctx);
    let metadata = principal_metadata(&ctx);
    cleanup_sessions();
    let id = req.id.clone();
    let mut response: Response = match req.method.as_str() {
        "initialize"
            if matches!(
                ctx,
                PrincipalCtx::Agent {
                    kind: AgentKind::ApiConsumer,
                    ..
                }
            ) =>
        {
            rpc_error(
                req.id,
                -32600,
                "api-consumer agents do not use the MCP endpoint",
                rid,
                "authz",
            )
            .into_response()
        }
        "initialize" => initialize(req.id, &principal, metadata, version.clone()),
        "notifications/initialized" | "initialized" => notification(req.id),
        "ping" => with_session(&headers, &principal, req.id, rid, || json!({})),
        "tools/list" => match validate_session(&headers, &principal, id, rid) {
            Err(response) => *response,
            Ok(guard) => {
                // guard held for the whole handler: keeps the session unreapable until
                // dispatch (and its post-authz stamp) completes, then decrements on drop.
                let response =
                    tools_list(&state, &ctx, req.id, req.params, rid, &guard.session_id).await;
                drop(guard);
                response
            }
        },
        "tools/call" => match validate_session(&headers, &principal, id, rid) {
            Err(response) => *response,
            Ok(guard) => {
                let response =
                    tools_call(&state, &ctx, req.id, req.params, rid, &guard.session_id).await;
                drop(guard);
                response
            }
        },
        _ => rpc_error(req.id, -32601, "method not found", rid, "method").into_response(),
    };
    if let Ok(value) = HeaderValue::from_str(&version) {
        response.headers_mut().insert("mcp-protocol-version", value);
    }
    response
}

/// MCP transport status for one team. `active_sessions` counts live sessions with recent,
/// successfully authorized MCP activity for this team on this control-plane node.
#[utoipa::path(get, path = "/api/v1/teams/{team}/mcp/status",
    tag = "McpTools",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses(
        (status = 200, body = McpStatusView),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn status(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<McpStatusView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        authorize_mcp_read(&state, &ctx, team, rid).await?;
        let dynamic_enabled_tool_count =
            fp_storage::repos::api_lifecycle::list_enabled_published_api_tools(
                &state.pool,
                team.id,
            )
            .await?
            .len();
        cleanup_sessions();
        let active_sessions = visible_sessions(&ctx, team).len();
        Ok::<_, DomainError>(McpStatusView {
            transport: "streamable_http_post".into(),
            preferred_protocol_version: PREFERRED_VERSION.into(),
            supported_protocol_versions: SUPPORTED_VERSIONS
                .iter()
                .map(|version| (*version).to_string())
                .collect(),
            session_ttl_seconds: SESSION_TTL.as_secs(),
            active_sessions,
            static_tool_count: STATIC_TOOL_DECLS.len(),
            dynamic_enabled_tool_count,
            tools_list_changed: false,
            sse_enabled: false,
            resources_enabled: false,
            prompts_enabled: false,
            api_invocation_mode: "gateway_invocation_descriptor".into(),
        })
    };
    run.await.map(Json).map_err(|e| ApiError::new(e, rid))
}

/// This team's attributed MCP connection records on this control-plane node: sessions with
/// recent, successfully authorized activity for the team. `connection_id` is a per-team
/// identifier — the same underlying session presents a different id to each team.
#[utoipa::path(get, path = "/api/v1/teams/{team}/mcp/connections",
    tag = "McpTools",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses(
        (status = 200, body = [McpConnectionView]),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn connections(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<McpConnectionView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        authorize_mcp_read(&state, &ctx, team, rid).await?;
        cleanup_sessions();
        Ok::<_, DomainError>(visible_sessions(&ctx, team))
    };
    run.await.map(Json).map_err(|e| ApiError::new(e, rid))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct McpToolCatalogRowView {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub resource: String,
    pub action: String,
    pub risk: String,
    /// "static" (cp_*/ops_* registry declaration) or "dynamic" (generated api_* tool).
    pub kind: String,
    pub enabled: bool,
    /// Authorization AND every serving gate execution enforces for this caller on the path
    /// team; a disabled dynamic tool is never executable regardless of grants.
    pub executable_by_caller: bool,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ToolCatalogQuery {
    /// Include disabled dynamic tools. Requires `mcp-tools:update` (the pair gating tool
    /// enable/disable); fails closed with 403 without it — never a silent downgrade.
    #[serde(default)]
    pub include_disabled: bool,
}

/// The team's declared MCP tool catalog: every static registry entry plus dynamic `api_*`
/// tools of currently published spec versions, each annotated with per-caller
/// executability. A catalog row is a declaration, not MCP exposure.
#[utoipa::path(get, path = "/api/v1/teams/{team}/mcp/tools",
    tag = "McpTools",
    params(("team" = String, Path, description = "Team name or UUID"), ToolCatalogQuery),
    responses(
        (status = 200, body = [McpToolCatalogRowView]),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn tool_catalog(
    State(state): State<AppState>,
    Path(team): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ToolCatalogQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<McpToolCatalogRowView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        let rows = fp_core::services::mcp::tool_catalog(
            &state.pool,
            &ctx,
            team,
            query.include_disabled,
            rid,
        )
        .await?;
        Ok::<_, DomainError>(
            rows.into_iter()
                .map(|row| McpToolCatalogRowView {
                    name: row.name,
                    description: row.description,
                    input_schema: row.input_schema,
                    resource: row.resource.to_string(),
                    action: row.action.to_string(),
                    risk: row.risk.to_string(),
                    kind: row.kind.as_str().to_string(),
                    enabled: row.enabled,
                    executable_by_caller: row.executable_by_caller,
                })
                .collect::<Vec<_>>(),
        )
    };
    run.await.map(Json).map_err(|e| ApiError::new(e, rid))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolExecutor {
    ClusterList,
    ClusterGet,
    ClusterCreate,
    ClusterUpdate,
    ClusterDelete,
    RouteConfigList,
    RouteConfigGet,
    RouteConfigCreate,
    RouteConfigUpdate,
    RouteConfigDelete,
    ListenerList,
    ListenerGet,
    ListenerCreate,
    ListenerUpdate,
    ListenerDelete,
    ApiList,
    ApiGet,
    ApiStatus,
    LearningList,
    LearningGet,
    DiscoveryList,
    DiscoveryGet,
    OpsXdsStatus,
    OpsXdsNacks,
    OpsXdsTrace,
    OpsStatsOverview,
    SecretsList,
    SecretsGet,
    AiProvidersList,
    AiProvidersGet,
    AiRoutesList,
    AiRoutesGet,
    AiBudgetsList,
    AiBudgetsGet,
    AiUsage,
}

/// Executor bindings for the shared static-tool declarations (fp-core
/// `mcp_declarations::STATIC_TOOL_DECLS`), keyed by declaration name. Dispatch-only:
/// authz/risk/schema metadata lives on the declaration. The bijection test below pins
/// declaration<->binding completeness in both directions.
const EXECUTOR_BINDINGS: &[(&str, ToolExecutor)] = &[
    ("cp_clusters_list", ToolExecutor::ClusterList),
    ("cp_clusters_get", ToolExecutor::ClusterGet),
    ("cp_clusters_create", ToolExecutor::ClusterCreate),
    ("cp_clusters_update", ToolExecutor::ClusterUpdate),
    ("cp_clusters_delete", ToolExecutor::ClusterDelete),
    ("cp_route_configs_list", ToolExecutor::RouteConfigList),
    ("cp_route_configs_get", ToolExecutor::RouteConfigGet),
    ("cp_route_configs_create", ToolExecutor::RouteConfigCreate),
    ("cp_route_configs_update", ToolExecutor::RouteConfigUpdate),
    ("cp_route_configs_delete", ToolExecutor::RouteConfigDelete),
    ("cp_listeners_list", ToolExecutor::ListenerList),
    ("cp_listeners_get", ToolExecutor::ListenerGet),
    ("cp_listeners_create", ToolExecutor::ListenerCreate),
    ("cp_listeners_update", ToolExecutor::ListenerUpdate),
    ("cp_listeners_delete", ToolExecutor::ListenerDelete),
    ("cp_apis_list", ToolExecutor::ApiList),
    ("cp_apis_get", ToolExecutor::ApiGet),
    ("cp_apis_status", ToolExecutor::ApiStatus),
    ("cp_learning_sessions_list", ToolExecutor::LearningList),
    ("cp_learning_sessions_get", ToolExecutor::LearningGet),
    ("cp_discovery_sessions_list", ToolExecutor::DiscoveryList),
    ("cp_discovery_sessions_get", ToolExecutor::DiscoveryGet),
    ("ops_xds_status", ToolExecutor::OpsXdsStatus),
    ("ops_xds_nacks", ToolExecutor::OpsXdsNacks),
    ("ops_xds_trace", ToolExecutor::OpsXdsTrace),
    ("ops_stats_overview", ToolExecutor::OpsStatsOverview),
    ("cp_secrets_list", ToolExecutor::SecretsList),
    ("cp_secrets_get", ToolExecutor::SecretsGet),
    ("cp_ai_providers_list", ToolExecutor::AiProvidersList),
    ("cp_ai_providers_get", ToolExecutor::AiProvidersGet),
    ("cp_ai_routes_list", ToolExecutor::AiRoutesList),
    ("cp_ai_routes_get", ToolExecutor::AiRoutesGet),
    ("cp_ai_budgets_list", ToolExecutor::AiBudgetsList),
    ("cp_ai_budgets_get", ToolExecutor::AiBudgetsGet),
    ("cp_ai_usage", ToolExecutor::AiUsage),
];

fn initialize(
    id: Option<Value>,
    principal: &str,
    metadata: PrincipalMetadata,
    protocol_version: String,
) -> Response {
    let session_id = format!("mcp-{}", uuid::Uuid::new_v4());
    let now = Instant::now();
    sessions().insert(
        session_id.clone(),
        McpSession {
            principal: principal.to_string(),
            principal_kind: metadata.kind,
            org_id: metadata.org_id,
            last_seen: now,
            in_flight: 0,
            team_activity: HashMap::new(),
        },
    );
    let mut response = rpc_result(
        id,
        json!({
            "protocolVersion": protocol_version,
            "serverInfo": {
                "name": "flowplane-mcp",
                "version": env!("CARGO_PKG_VERSION"),
                "title": "Flowplane MCP"
            },
            "capabilities": {
                "tools": { "listChanged": false }
            }
        }),
    )
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&session_id) {
        response.headers_mut().insert("mcp-session-id", value);
    }
    response
}

fn notification(id: Option<Value>) -> Response {
    match id {
        Some(id) => rpc_result(Some(id), json!({})).into_response(),
        None => axum::http::Response::builder()
            .status(axum::http::StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap_or_else(|_| Body::empty().into_response()),
    }
}

async fn tools_list(
    state: &AppState,
    ctx: &PrincipalCtx,
    id: Option<Value>,
    params: Value,
    rid: RequestId,
    session_id: &str,
) -> Response {
    let team = match resolve_tool_team(state, ctx, &params).await {
        Ok(team) => team,
        Err(e) => return rpc_error(id, -32600, e.message, rid, "validation").into_response(),
    };
    let tools = STATIC_TOOL_DECLS
        .iter()
        .filter(|tool| tool_allowed(ctx, tool, team))
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.input_schema)(),
                "annotations": {
                    "resource": tool.resource.as_str(),
                    "action": tool.action.as_str(),
                    "risk": tool.risk.as_str(),
                }
            })
        })
        .collect::<Vec<_>>();
    let dynamic_allowed = dynamic_tool_allowed(ctx, team, Action::Execute);
    // Attribution is authorization-based, not result-count-based: a passing grant check
    // stamps even when the dynamic-tool query below returns zero rows.
    if !tools.is_empty() || dynamic_allowed {
        stamp_team_activity(session_id, team.id.as_uuid());
    }
    let mut tools = tools;
    if dynamic_allowed {
        match fp_storage::repos::api_lifecycle::list_enabled_published_api_tools(
            &state.pool,
            team.id,
        )
        .await
        {
            Ok(api_tools) => tools.extend(api_tools.into_iter().map(dynamic_tool_view)),
            Err(e) => return tool_result_error(id, e).into_response(),
        }
    }
    rpc_result(id, json!({ "tools": tools })).into_response()
}

async fn tools_call(
    state: &AppState,
    ctx: &PrincipalCtx,
    id: Option<Value>,
    params: Value,
    rid: RequestId,
    session_id: &str,
) -> Response {
    let name = match params.get("name").and_then(Value::as_str) {
        Some(name) => name,
        None => {
            return rpc_error(
                id,
                -32600,
                "tools/call requires params.name",
                rid,
                "validation",
            )
            .into_response();
        }
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(api_tool_name) = name.strip_prefix("api_") {
        return match execute_dynamic_tool(state, ctx, api_tool_name, arguments, rid, session_id)
            .await
        {
            Ok(value) => tool_result_ok(id, value).into_response(),
            Err(e) => tool_result_error(id, e).into_response(),
        };
    }
    let Some(tool) = static_tool(name) else {
        return rpc_error(id, -32602, format!("unknown tool: {name}"), rid, "tool").into_response();
    };
    let team = match resolve_tool_team(state, ctx, &arguments).await {
        Ok(team) => team,
        Err(e) => return tool_result_error(id, e).into_response(),
    };
    if let Decision::Deny(reason) =
        check_resource_access(ctx, tool.resource, tool.action, Some(team))
    {
        record_authz_denial(
            &state.pool,
            ctx,
            rid,
            tool.resource,
            tool.action,
            Some(team),
            reason,
        )
        .await;
        return rpc_error(
            id,
            -32600,
            format!(
                "missing permission: {}:{}",
                tool.resource.as_str(),
                tool.action.as_str()
            ),
            rid,
            "authz",
        )
        .into_response();
    }
    stamp_team_activity(session_id, team.id.as_uuid());

    match execute_static_tool(state, ctx, tool, team, arguments, rid).await {
        Ok(value) => tool_result_ok(id, value).into_response(),
        Err(e) => tool_result_error(id, e).into_response(),
    }
}

async fn execute_dynamic_tool(
    state: &AppState,
    ctx: &PrincipalCtx,
    api_tool_name: &str,
    arguments: Value,
    rid: RequestId,
    session_id: &str,
) -> DomainResult<Value> {
    let team = resolve_tool_team(state, ctx, &arguments).await?;
    if let Decision::Deny(reason) =
        check_resource_access(ctx, Resource::McpTools, Action::Execute, Some(team))
    {
        fp_core::services::record_authz_denial(
            &state.pool,
            ctx,
            rid,
            Resource::McpTools,
            Action::Execute,
            Some(team),
            reason,
        )
        .await;
        return Err(fp_core::services::deny_to_error(
            Resource::McpTools,
            Action::Execute,
            reason,
        ));
    }
    stamp_team_activity(session_id, team.id.as_uuid());
    let tool = fp_storage::repos::api_lifecycle::get_enabled_published_api_tool(
        &state.pool,
        team.id,
        api_tool_name,
    )
    .await?
    .ok_or_else(|| DomainError::not_found("api tool", api_tool_name))?;
    let bindings = fp_storage::repos::api_lifecycle::list_route_bindings_for_api(
        &state.pool,
        team.id,
        tool.api_definition_id,
    )
    .await?;
    // ponytail: first listener binding wins; add explicit binding selection if callers need it.
    let binding = match bindings
        .into_iter()
        .find(|binding| binding.listener_id.is_some())
    {
        Some(binding) => binding,
        None => {
            record_dynamic_tool_audit(
                state,
                ctx,
                rid,
                team,
                &tool,
                fp_storage::repos::audit::Outcome::Failure,
                json!({ "error": "unbound_route" }),
            )
            .await;
            return Err(DomainError::new(
                ErrorCode::Conflict,
                format!("api tool \"{}\" has no listener/dataplane route", tool.name),
            )
            .with_hint("bind the API definition to a listener before calling this tool"));
        }
    };
    let listener_id = binding.listener_id.ok_or_else(|| {
        DomainError::internal("listener binding disappeared while resolving api tool")
    })?;
    let listener =
        fp_storage::repos::gateway::get_listener_by_id(&state.pool, team.id, listener_id)
            .await?
            .ok_or_else(|| {
                DomainError::not_found("listener", &listener_id.as_uuid().to_string())
            })?;
    let descriptor = match dynamic_tool_descriptor(&tool, &arguments, &listener.spec, &binding, rid)
    {
        Ok(descriptor) => descriptor,
        Err(e) => {
            record_dynamic_tool_audit(
                state,
                ctx,
                rid,
                team,
                &tool,
                fp_storage::repos::audit::Outcome::Failure,
                json!({ "error": "descriptor_unavailable" }),
            )
            .await;
            return Err(e);
        }
    };
    record_dynamic_tool_audit(
        state,
        ctx,
        rid,
        team,
        &tool,
        fp_storage::repos::audit::Outcome::Success,
        json!({ "descriptor": true }),
    )
    .await;
    Ok(descriptor)
}

async fn record_dynamic_tool_audit(
    state: &AppState,
    ctx: &PrincipalCtx,
    rid: RequestId,
    team: TeamRef,
    tool: &ApiTool,
    outcome: fp_storage::repos::audit::Outcome,
    detail: Value,
) {
    let (actor_type, actor_id) = fp_core::services::actor_of(ctx);
    fp_storage::repos::audit::record_best_effort(
        &state.pool,
        &fp_storage::repos::audit::AuditEntry {
            request_id: Some(rid),
            actor_type,
            actor_id,
            actor_label: String::new(),
            surface: fp_storage::repos::audit::Surface::Mcp,
            action: "api_tool.execute".into(),
            resource: format!("api-tools/{}", tool.name),
            org_id: Some(team.org_id),
            team_id: Some(team.id),
            outcome,
            detail,
        },
    )
    .await;
}

async fn execute_static_tool(
    state: &AppState,
    ctx: &PrincipalCtx,
    tool: &StaticToolDecl,
    team: TeamRef,
    arguments: Value,
    rid: RequestId,
) -> DomainResult<Value> {
    // Fail closed if a declaration has no dispatch binding; the bijection test makes this
    // unreachable, but a missing binding must never fall through to another tool.
    let executor = executor_binding(tool.name).ok_or_else(|| {
        DomainError::internal(format!("no executor binding for tool {}", tool.name))
    })?;
    match executor {
        ToolExecutor::ClusterList => {
            let (items, total) = fp_core::services::clusters::list_clusters(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::ClusterGet => {
            let item = fp_core::services::clusters::get_cluster(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ClusterCreate => {
            let spec =
                serde_json::from_value::<ClusterSpec>(required_value(&arguments, "spec")?)
                    .map_err(|e| DomainError::validation(format!("invalid cluster spec: {e}")))?;
            let item = fp_core::services::clusters::create_cluster(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                spec,
                rid,
                state.egress_advisory.clone(),
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ClusterUpdate => {
            let spec =
                serde_json::from_value::<ClusterSpec>(required_value(&arguments, "spec")?)
                    .map_err(|e| DomainError::validation(format!("invalid cluster spec: {e}")))?;
            let item = fp_core::services::clusters::update_cluster(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                spec,
                integer_arg(&arguments, "revision")
                    .ok_or_else(|| DomainError::validation("revision is required"))?,
                rid,
                state.egress_advisory.clone(),
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ClusterDelete => {
            fp_core::services::clusters::delete_cluster(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                integer_arg(&arguments, "revision")
                    .ok_or_else(|| DomainError::validation("revision is required"))?,
                rid,
            )
            .await?;
            Ok(json!({ "deleted": true }))
        }
        ToolExecutor::RouteConfigList => {
            let (items, total) = fp_core::services::gateway::list_route_configs(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::RouteConfigGet => {
            let item = fp_core::services::gateway::get_route_config(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::RouteConfigCreate => {
            let spec =
                serde_json::from_value::<RouteConfigSpec>(required_value(&arguments, "spec")?)
                    .map_err(|e| {
                        DomainError::validation(format!("invalid route config spec: {e}"))
                    })?;
            let item = fp_core::services::gateway::create_route_config(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                spec,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::RouteConfigUpdate => {
            let spec =
                serde_json::from_value::<RouteConfigSpec>(required_value(&arguments, "spec")?)
                    .map_err(|e| {
                        DomainError::validation(format!("invalid route config spec: {e}"))
                    })?;
            let item = fp_core::services::gateway::update_route_config(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                spec,
                required_revision(&arguments)?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::RouteConfigDelete => {
            fp_core::services::gateway::delete_route_config(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                required_revision(&arguments)?,
                rid,
            )
            .await?;
            Ok(json!({ "deleted": true }))
        }
        ToolExecutor::ListenerList => {
            let (items, total) = fp_core::services::gateway::list_listeners(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::ListenerGet => {
            let item = fp_core::services::gateway::get_listener(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ListenerCreate => {
            let spec = serde_json::from_value::<ListenerSpec>(required_value(&arguments, "spec")?)
                .map_err(|e| DomainError::validation(format!("invalid listener spec: {e}")))?;
            let item = fp_core::services::gateway::create_listener(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                spec,
                rid,
                state.rls_grpc_configured,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ListenerUpdate => {
            let spec = serde_json::from_value::<ListenerSpec>(required_value(&arguments, "spec")?)
                .map_err(|e| DomainError::validation(format!("invalid listener spec: {e}")))?;
            let item = fp_core::services::gateway::update_listener(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                spec,
                required_revision(&arguments)?,
                rid,
                state.rls_grpc_configured,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ListenerDelete => {
            fp_core::services::gateway::delete_listener(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                required_revision(&arguments)?,
                rid,
            )
            .await?;
            Ok(json!({ "deleted": true }))
        }
        ToolExecutor::ApiList => {
            let (items, total) = fp_core::services::api_lifecycle::list_apis(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::ApiGet => {
            let item = fp_core::services::api_lifecycle::get_api(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::ApiStatus => {
            let item = fp_core::services::api_lifecycle::api_status(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            Ok(json!({
                "api": item.api,
                "latest_spec": item.latest_spec,
                "tool_count": item.tool_count,
                "route_binding_count": item.route_binding_count,
            }))
        }
        ToolExecutor::LearningList => {
            let (items, total) = fp_core::services::learning::list_sessions(
                &state.pool,
                ctx,
                team,
                None,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::LearningGet => {
            let item = fp_core::services::learning::get_session(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::DiscoveryList => {
            let (items, total) = fp_core::services::discovery::list_sessions(
                &state.pool,
                ctx,
                team,
                None,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::DiscoveryGet => {
            let item = fp_core::services::discovery::get_session(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::OpsXdsStatus => {
            let status = fp_core::services::xds_status::status(&state.pool, ctx, team, rid).await?;
            let latest_nack = status.latest_nack.map(|nack| {
                json!({
                    "id": nack.id,
                    "node_id": nack.node_id,
                    "type_url": nack.type_url,
                    "version_rejected": nack.version_rejected,
                    "error_message": nack.error_message,
                    "quarantined_resources": nack.quarantined_resources,
                    "created_at": nack.created_at,
                })
            });
            Ok(json!({
                "total_dataplanes": status.total_dataplanes,
                "live_dataplanes": status.live_dataplanes,
                "stale_dataplanes": status.stale_dataplanes,
                "config_verified_dataplanes": status.config_verified_dataplanes,
                "total_requests": status.total_requests,
                "total_errors": status.total_errors,
                "warming_failures": status.warming_failures,
                "recent_nack_count": status.recent_nack_count,
                "latest_nack": latest_nack,
            }))
        }
        ToolExecutor::OpsXdsNacks => {
            let nacks = fp_core::services::xds_status::list_nack_events(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                rid,
            )
            .await?;
            let items = nacks
                .into_iter()
                .map(|nack| {
                    json!({
                        "id": nack.id,
                        "node_id": nack.node_id,
                        "type_url": nack.type_url,
                        "version_rejected": nack.version_rejected,
                        "error_message": nack.error_message,
                        "quarantined_resources": nack.quarantined_resources,
                        "created_at": nack.created_at,
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({ "items": items }))
        }
        ToolExecutor::OpsXdsTrace => {
            let query = fp_core::services::xds_status::TraceQuery {
                request_id: optional_request_id(&arguments)?,
                trace_id: arguments
                    .get("traceId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                path: arguments
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                limit: integer_arg(&arguments, "limit").unwrap_or(50),
            };
            let trace =
                fp_core::services::xds_status::trace(&state.pool, ctx, team, query, rid).await?;
            let audit = trace
                .audit
                .into_iter()
                .map(|row| {
                    json!({
                        "id": row.id,
                        "request_id": row.request_id,
                        "actor_label": row.actor_label,
                        "surface": row.surface,
                        "action": row.action,
                        "resource": row.resource,
                        "outcome": row.outcome,
                        "detail": row.detail,
                        "occurred_at": row.occurred_at,
                    })
                })
                .collect::<Vec<_>>();
            let events = trace
                .events
                .into_iter()
                .map(|row| {
                    json!({
                        "seq": row.seq,
                        "event_type": row.event_type,
                        "payload": row.payload,
                        "trace_context": row.trace_context,
                        "occurred_at": row.occurred_at,
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "audit": audit,
                "events": events,
            }))
        }
        ToolExecutor::OpsStatsOverview => {
            let overview =
                fp_core::services::dataplanes::stats_overview(&state.pool, ctx, team, rid).await?;
            serde_json::to_value(overview).map_err(json_err)
        }
        ToolExecutor::SecretsList => {
            let (items, total) = fp_core::services::secrets::list_secrets(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::SecretsGet => {
            let item = fp_core::services::secrets::get_secret(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::AiProvidersList => {
            let (items, total) = fp_core::services::ai::list_providers(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::AiProvidersGet => {
            let item = fp_core::services::ai::get_provider(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::AiRoutesList => {
            let (items, total) = fp_core::services::ai::list_routes(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::AiRoutesGet => {
            let item = fp_core::services::ai::get_route(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::AiBudgetsList => {
            let (items, total) = fp_core::services::ai::list_budgets(
                &state.pool,
                ctx,
                team,
                integer_arg(&arguments, "limit").unwrap_or(50),
                integer_arg(&arguments, "offset").unwrap_or(0),
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
        ToolExecutor::AiBudgetsGet => {
            let item = fp_core::services::ai::get_budget(
                &state.pool,
                ctx,
                team,
                string_arg(&arguments, "name")?,
                rid,
            )
            .await?;
            serde_json::to_value(item).map_err(json_err)
        }
        ToolExecutor::AiUsage => {
            let (items, total) = fp_core::services::ai::usage_summary(
                &state.pool,
                ctx,
                team,
                fp_storage::repos::ai::AiUsageQuery {
                    route_config_id: None,
                    provider_id: None,
                    since: optional_timestamp_arg(&arguments, "since")?,
                    until: optional_timestamp_arg(&arguments, "until")?,
                    limit: integer_arg(&arguments, "limit").unwrap_or(50),
                    offset: integer_arg(&arguments, "offset").unwrap_or(0),
                },
                rid,
            )
            .await?;
            Ok(json!({ "items": items, "total": total }))
        }
    }
}

fn with_session(
    headers: &HeaderMap,
    principal: &str,
    id: Option<Value>,
    rid: RequestId,
    result: impl FnOnce() -> Value,
) -> Response {
    let Some(session_id) = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| valid_session_id(s))
    else {
        return rpc_error(
            id,
            -32600,
            "missing or invalid MCP-Session-Id",
            rid,
            "session",
        )
        .into_response();
    };
    let sessions = sessions();
    let Some(session) = sessions.get(session_id) else {
        return rpc_error(id, -32600, "unknown MCP session", rid, "session").into_response();
    };
    if session.principal != principal {
        return rpc_error(id, -32600, "MCP session principal mismatch", rid, "authz")
            .into_response();
    }
    // Deliberately no last_seen refresh: pings validate a session but never extend its
    // lifetime — only successfully authorized team operations do (stamp_team_activity).
    rpc_result(id, result()).into_response()
}

fn validate_session(
    headers: &HeaderMap,
    principal: &str,
    id: Option<Value>,
    rid: RequestId,
) -> Result<InFlightGuard, Box<Response>> {
    let Some(session_id) = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| valid_session_id(s))
    else {
        return Err(Box::new(
            rpc_error(
                id,
                -32600,
                "missing or invalid MCP-Session-Id",
                rid,
                "session",
            )
            .into_response(),
        ));
    };
    let mut sessions = sessions();
    let Some(session) = sessions.get_mut(session_id) else {
        return Err(Box::new(
            rpc_error(id, -32600, "unknown MCP session", rid, "session").into_response(),
        ));
    };
    if session.principal != principal {
        return Err(Box::new(
            rpc_error(id, -32600, "MCP session principal mismatch", rid, "authz").into_response(),
        ));
    }
    // Mark the session in-flight so concurrent TTL cleanup cannot reap it while this
    // request's authorization crosses `.await` points — otherwise an authorized call at
    // the TTL boundary could lose its session before its post-authz stamp runs (§4).
    // Deliberately no last_seen refresh here (pre-authorization): denied or malformed
    // tool requests must not extend the session. The refresh happens only in
    // stamp_team_activity, after the operation's team authorization allowed it.
    session.in_flight = session.in_flight.saturating_add(1);
    Ok(InFlightGuard {
        session_id: session_id.to_string(),
    })
}

/// Records a successfully authorized team operation on the session: refreshes `last_seen`
/// and inserts-or-refreshes the team's activity entry. A stale entry (older than `ttl`) is
/// replaced rather than resumed, minting a fresh `team_connection_id`. Call strictly AFTER
/// the operation's authorization allowed it — denied or malformed requests must never stamp.
fn stamp_team_activity(session_id: &str, team_id: uuid::Uuid) {
    let now = Instant::now();
    let mut sessions = sessions();
    let Some(session) = sessions.get_mut(session_id) else {
        return;
    };
    session.last_seen = now;
    match session.team_activity.get_mut(&team_id) {
        Some(activity) if !team_activity_stale(activity, now, SESSION_TTL) => {
            activity.last_authorized_at = now;
        }
        _ => {
            session.team_activity.insert(
                team_id,
                TeamActivity {
                    team_connection_id: uuid::Uuid::new_v4(),
                    first_authorized_at: now,
                    last_authorized_at: now,
                },
            );
        }
    }
}

async fn resolve_tool_team(
    state: &AppState,
    ctx: &PrincipalCtx,
    params: &Value,
) -> DomainResult<TeamRef> {
    let team = params.get("team").and_then(Value::as_str).ok_or_else(|| {
        DomainError::new(
            ErrorCode::ValidationFailed,
            "MCP static tools require arguments.team",
        )
        .with_hint("pass the team name or UUID in tools/list params or tools/call arguments")
    })?;
    crate::resources::resolve_team(state, ctx, team).await
}

fn dynamic_tool_allowed(ctx: &PrincipalCtx, team: TeamRef, action: Action) -> bool {
    matches!(
        check_resource_access(ctx, Resource::McpTools, action, Some(team)),
        Decision::Allow(_)
    )
}

async fn authorize_mcp_read(
    state: &AppState,
    ctx: &PrincipalCtx,
    team: TeamRef,
    rid: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::McpTools, Action::Read, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                &state.pool,
                ctx,
                rid,
                Resource::McpTools,
                Action::Read,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::McpTools, Action::Read, reason))
        }
    }
}

// Team-scoped visibility (security-scan 742e6d3 finding 12): a session is listed for a
// team only with non-stale, successfully authorized activity on that team, and each team
// sees its own per-team connection id — never the session-global one.
fn visible_sessions(ctx: &PrincipalCtx, team: TeamRef) -> Vec<McpConnectionView> {
    let meta = principal_metadata(ctx);
    let now = Instant::now();
    sessions()
        .values()
        .filter(|session| {
            session.org_id.is_some()
                && session.org_id == meta.org_id
                && session.org_id == Some(team.org_id.as_uuid())
        })
        .filter_map(|session| {
            let activity = session.team_activity.get(&team.id.as_uuid())?;
            if team_activity_stale(activity, now, SESSION_TTL) {
                return None;
            }
            Some(McpConnectionView {
                connection_id: activity.team_connection_id,
                principal_kind: session.principal_kind.into(),
                transport: "streamable_http_post".into(),
                sse: false,
                age_seconds: now.duration_since(activity.first_authorized_at).as_secs(),
                idle_seconds: now.duration_since(activity.last_authorized_at).as_secs(),
            })
        })
        .collect()
}

fn dynamic_tool_view(tool: ApiTool) -> Value {
    json!({
        "name": fp_core::mcp_declarations::dynamic_tool_name(&tool.name),
        "description": fp_core::mcp_declarations::dynamic_tool_description(
            tool.method.as_str(),
            &tool.path,
        ),
        "inputSchema": fp_core::mcp_declarations::dynamic_input_schema(&tool.input_schema),
        "annotations": {
            "resource": Resource::McpTools.as_str(),
            "action": Action::Execute.as_str(),
            "risk": fp_core::mcp_declarations::DYNAMIC_TOOL_RISK,
            "apiToolId": tool.id.as_uuid(),
            "apiDefinitionId": tool.api_definition_id.as_uuid(),
            "specVersionId": tool.spec_version_id.as_uuid(),
            "operationId": tool.operation_id,
            "method": tool.method.as_str(),
            "path": tool.path,
        }
    })
}

fn dynamic_tool_descriptor(
    tool: &ApiTool,
    arguments: &Value,
    listener: &ListenerSpec,
    binding: &ApiRouteBinding,
    rid: RequestId,
) -> DomainResult<Value> {
    let base_url = listener.public_base_url.as_deref().ok_or_else(|| {
        DomainError::new(
            ErrorCode::Conflict,
            format!(
                "listener for api tool \"{}\" has no public_base_url",
                tool.name
            ),
        )
        .with_hint("set listener.spec.public_base_url before invoking api tools")
    })?;
    let url = dynamic_tool_url(tool, arguments, base_url)?;
    let headers = dynamic_descriptor_headers(arguments, binding)?;
    Ok(json!({
        "type": "gateway_invocation",
        "version": 1,
        "tool": format!("api_{}", tool.name),
        "apiToolId": tool.id.as_uuid(),
        "apiDefinitionId": tool.api_definition_id.as_uuid(),
        "specVersionId": tool.spec_version_id.as_uuid(),
        "operationId": tool.operation_id,
        "method": tool.method.as_str(),
        "url": url.as_str(),
        "headers": headers,
        "body": arguments.get("body").cloned().unwrap_or(Value::Null),
        "auth": { "mode": "caller_gateway_credentials" },
        "expiresAt": (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
        "correlationId": rid.to_string(),
    }))
}

fn dynamic_tool_url(
    tool: &ApiTool,
    arguments: &Value,
    base_url: &str,
) -> DomainResult<reqwest::Url> {
    let mut path = tool.path.clone();
    if let Some(params) = arguments.get("pathParams").and_then(Value::as_object) {
        for (key, value) in params {
            let Some(value) = value.as_str() else {
                return Err(DomainError::validation(format!(
                    "pathParams.{key} must be a string"
                )));
            };
            if is_dot_path_segment(value) {
                return Err(DomainError::validation(format!(
                    "pathParams.{key} must not be a dot segment"
                )));
            }
            path = path.replace(&format!("{{{key}}}"), &encode_path_segment(value));
        }
    }
    let mut url = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .map_err(|e| DomainError::validation(format!("invalid listener public_base_url: {e}")))?;
    url.set_path(&path);
    if let Some(query) = arguments.get("query").and_then(Value::as_object) {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            match value {
                Value::String(value) => {
                    pairs.append_pair(key, value);
                }
                Value::Number(value) => {
                    pairs.append_pair(key, &value.to_string());
                }
                Value::Bool(value) => {
                    pairs.append_pair(key, &value.to_string());
                }
                _ => {
                    return Err(DomainError::validation(format!(
                        "query.{key} must be scalar"
                    )))
                }
            }
        }
    }
    Ok(url)
}

fn dynamic_descriptor_headers(arguments: &Value, binding: &ApiRouteBinding) -> DomainResult<Value> {
    let mut headers = serde_json::Map::new();
    if let Some(input) = arguments.get("headers").and_then(Value::as_object) {
        for (key, value) in input {
            if key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case(":authority") {
                return Err(DomainError::validation(format!(
                    "headers.{key} is controlled by the api route binding"
                )));
            }
            let Some(value) = value.as_str() else {
                return Err(DomainError::validation(format!(
                    "headers.{key} must be a string"
                )));
            };
            headers.insert(key.clone(), Value::String(value.to_string()));
        }
    }
    if let Some(host) = binding.virtual_host.as_deref().filter(|host| *host != "*") {
        headers.insert("host".into(), Value::String(host.to_string()));
    }
    Ok(Value::Object(headers))
}

fn is_dot_path_segment(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
    )
}

fn encode_path_segment(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn static_tool(name: &str) -> Option<&'static StaticToolDecl> {
    STATIC_TOOL_DECLS.iter().find(|tool| tool.name == name)
}

fn executor_binding(name: &str) -> Option<ToolExecutor> {
    EXECUTOR_BINDINGS
        .iter()
        .find(|(bound_name, _)| *bound_name == name)
        .map(|(_, executor)| *executor)
}

fn tool_allowed(ctx: &PrincipalCtx, tool: &StaticToolDecl, team: TeamRef) -> bool {
    matches!(
        check_resource_access(ctx, tool.resource, tool.action, Some(team)),
        Decision::Allow(_)
    )
}

#[cfg(test)]
fn executor_authz(executor: ToolExecutor) -> (Resource, Action) {
    match executor {
        ToolExecutor::ClusterList | ToolExecutor::ClusterGet => (Resource::Clusters, Action::Read),
        ToolExecutor::ClusterCreate => (Resource::Clusters, Action::Create),
        ToolExecutor::ClusterUpdate => (Resource::Clusters, Action::Update),
        ToolExecutor::ClusterDelete => (Resource::Clusters, Action::Delete),
        ToolExecutor::RouteConfigList | ToolExecutor::RouteConfigGet => {
            (Resource::RouteConfigs, Action::Read)
        }
        ToolExecutor::RouteConfigCreate => (Resource::RouteConfigs, Action::Create),
        ToolExecutor::RouteConfigUpdate => (Resource::RouteConfigs, Action::Update),
        ToolExecutor::RouteConfigDelete => (Resource::RouteConfigs, Action::Delete),
        ToolExecutor::ListenerList | ToolExecutor::ListenerGet => {
            (Resource::Listeners, Action::Read)
        }
        ToolExecutor::ListenerCreate => (Resource::Listeners, Action::Create),
        ToolExecutor::ListenerUpdate => (Resource::Listeners, Action::Update),
        ToolExecutor::ListenerDelete => (Resource::Listeners, Action::Delete),
        ToolExecutor::ApiList | ToolExecutor::ApiGet | ToolExecutor::ApiStatus => {
            (Resource::ApiDefinitions, Action::Read)
        }
        ToolExecutor::LearningList
        | ToolExecutor::LearningGet
        | ToolExecutor::DiscoveryList
        | ToolExecutor::DiscoveryGet => (Resource::LearningSessions, Action::Read),
        ToolExecutor::OpsXdsStatus | ToolExecutor::OpsStatsOverview => {
            (Resource::Stats, Action::Read)
        }
        ToolExecutor::OpsXdsNacks | ToolExecutor::OpsXdsTrace => (Resource::Stats, Action::Read),
        ToolExecutor::SecretsList | ToolExecutor::SecretsGet => (Resource::Secrets, Action::Read),
        ToolExecutor::AiProvidersList | ToolExecutor::AiProvidersGet => {
            (Resource::AiProviders, Action::Read)
        }
        ToolExecutor::AiRoutesList | ToolExecutor::AiRoutesGet => {
            (Resource::AiRoutes, Action::Read)
        }
        ToolExecutor::AiBudgetsList | ToolExecutor::AiBudgetsGet => {
            (Resource::AiBudgets, Action::Read)
        }
        ToolExecutor::AiUsage => (Resource::AiUsage, Action::Read),
    }
}

fn requested_version(headers: &HeaderMap, req: &JsonRpcRequest) -> Result<String, &'static str> {
    let header_version = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok());
    let param_version = req.params.get("protocolVersion").and_then(|v| v.as_str());
    let requested = header_version
        .or(param_version)
        .unwrap_or(PREFERRED_VERSION);
    if SUPPORTED_VERSIONS.contains(&requested) {
        Ok(requested.to_string())
    } else {
        Err("unsupported MCP protocol version")
    }
}

fn check_origin(headers: &HeaderMap) -> Result<(), &'static str> {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    let allowed = std::env::var("FLOWPLANE_MCP_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost,http://127.0.0.1,http://[::1]".to_string());
    if allowed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|allowed| origin_matches(allowed, origin))
    {
        Ok(())
    } else {
        Err("origin is not allowed")
    }
}

fn origin_matches(allowed: &str, origin: &str) -> bool {
    fn scheme_host(value: &str) -> Option<(&str, &str)> {
        let (scheme, rest) = value.split_once("://")?;
        let host = rest
            .strip_prefix('[')
            .and_then(|v| v.split_once(']').map(|(host, _)| host))
            .or_else(|| rest.split(':').next())
            .unwrap_or(rest);
        Some((scheme, host))
    }
    scheme_host(allowed) == scheme_host(origin)
}

fn required_value(args: &Value, key: &'static str) -> DomainResult<Value> {
    args.get(key)
        .cloned()
        .ok_or_else(|| DomainError::validation(format!("{key} is required")))
}

fn string_arg<'a>(args: &'a Value, key: &'static str) -> DomainResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| DomainError::validation(format!("{key} is required")))
}

fn integer_arg(args: &Value, key: &'static str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

/// Optional RFC 3339 timestamp argument; any PRESENT value that is not an RFC 3339
/// string — wrong JSON type included — is a validation error, never silently ignored.
fn optional_timestamp_arg(
    args: &Value,
    key: &'static str,
) -> DomainResult<Option<chrono::DateTime<chrono::Utc>>> {
    match args.get(key) {
        None => Ok(None),
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| {
                DomainError::validation(format!("{key} must be an RFC 3339 string"))
            })?;
            chrono::DateTime::parse_from_rfc3339(raw)
                .map(|dt| Some(dt.with_timezone(&chrono::Utc)))
                .map_err(|e| DomainError::validation(format!("{key} is not RFC 3339: {e}")))
        }
    }
}

fn required_revision(args: &Value) -> DomainResult<i64> {
    integer_arg(args, "revision").ok_or_else(|| DomainError::validation("revision is required"))
}

fn optional_request_id(args: &Value) -> DomainResult<Option<RequestId>> {
    args.get("requestId")
        .and_then(Value::as_str)
        .map(RequestId::from_str)
        .transpose()
}

fn json_err(e: serde_json::Error) -> DomainError {
    DomainError::internal(format!("serialize MCP tool result: {e}"))
}

fn tool_result_ok(id: Option<Value>, value: Value) -> Json<JsonRpcResponse> {
    rpc_result(
        id,
        json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            }],
            "structuredContent": value,
            "isError": false,
        }),
    )
}

fn tool_result_error(id: Option<Value>, error: DomainError) -> Json<JsonRpcResponse> {
    rpc_result(
        id,
        json!({
            "content": [{
                "type": "text",
                "text": error.hint.as_ref().map_or_else(
                    || error.message.clone(),
                    |hint| format!("{} Hint: {hint}", error.message),
                ),
            }],
            "isError": true,
            "error": {
                "code": error.code.as_str(),
                "message": error.message,
                "hint": error.hint,
                "details": error.details,
            }
        }),
    )
}

fn principal_key(ctx: &PrincipalCtx) -> String {
    match ctx {
        PrincipalCtx::User { user_id, org, .. } => {
            format!(
                "user:{user_id}:org:{:?}",
                org.map(|(id, role)| (id, role.as_str()))
            )
        }
        PrincipalCtx::Agent {
            agent_id, org_id, ..
        } => format!("agent:{agent_id}:org:{org_id}"),
    }
}

#[derive(Clone, Copy)]
struct PrincipalMetadata {
    kind: &'static str,
    org_id: Option<uuid::Uuid>,
}

fn principal_metadata(ctx: &PrincipalCtx) -> PrincipalMetadata {
    match ctx {
        PrincipalCtx::User { org, .. } => PrincipalMetadata {
            kind: "user",
            org_id: org.map(|(org_id, _)| org_id.as_uuid()),
        },
        PrincipalCtx::Agent { org_id, .. } => PrincipalMetadata {
            kind: "agent",
            org_id: Some(org_id.as_uuid()),
        },
    }
}

fn cleanup_sessions() {
    cleanup_sessions_at(Instant::now(), SESSION_TTL);
}

fn cleanup_sessions_at(now: Instant, ttl: Duration) {
    let mut sessions = sessions();
    // Never reap a session with in-flight requests: one may have validated just before
    // its last_seen crossed the TTL and still be awaiting authorization, after which it
    // will stamp and refresh. Idle sessions have in_flight == 0 and expire normally.
    sessions
        .retain(|_, session| session.in_flight > 0 || now.duration_since(session.last_seen) <= ttl);
    for session in sessions.values_mut() {
        session
            .team_activity
            .retain(|_, activity| !team_activity_stale(activity, now, ttl));
    }
}

fn sessions() -> MutexGuard<'static, HashMap<String, McpSession>> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn valid_session_id(session_id: &str) -> bool {
    session_id
        .strip_prefix("mcp-")
        .and_then(|uuid| uuid::Uuid::parse_str(uuid).ok())
        .is_some()
}

fn rpc_result(id: Option<Value>, result: Value) -> Json<JsonRpcResponse> {
    Json(JsonRpcResponse {
        jsonrpc: "2.0",
        result: Some(result),
        error: None,
        id,
    })
}

fn rpc_error(
    id: Option<Value>,
    code: i64,
    message: impl Into<String>,
    rid: RequestId,
    kind: &'static str,
) -> Json<JsonRpcResponse> {
    Json(JsonRpcResponse {
        jsonrpc: "2.0",
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: json!({
                "kind": kind,
                "requestId": rid.to_string(),
                "fix": "check the MCP request, session, protocol version, and bearer token",
            }),
        }),
        id,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::Body;
    use fp_core::GrantSet;
    use fp_domain::api_lifecycle::HttpMethod;
    use fp_domain::{OrgId, OrgRole, UserId};
    use http_body_util::BodyExt;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::HashSet;

    fn test_user_ctx(org_id: OrgId) -> PrincipalCtx {
        PrincipalCtx::User {
            user_id: UserId::generate(),
            platform_admin: false,
            org_selector_required: false,
            org: Some((org_id, OrgRole::Member)),
            grants: GrantSet::new([]),
        }
    }

    // The SESSIONS registry is process-global (shared across parallel tests): every test
    // uses unique session ids / org ids / team ids and asserts only on its own entries.
    fn insert_test_session(session_id: &str, org_id: Option<uuid::Uuid>) {
        let now = Instant::now();
        sessions().insert(
            session_id.to_string(),
            McpSession {
                principal: format!("test:{session_id}"),
                principal_kind: "user",
                org_id,
                last_seen: now,
                in_flight: 0,
                team_activity: HashMap::new(),
            },
        );
    }

    fn remove_test_sessions(ids: &[&str]) {
        let mut sessions = sessions();
        for id in ids {
            sessions.remove(*id);
        }
    }

    #[test]
    fn stamp_creates_refreshes_and_isolates_per_team() {
        let sid = format!("mcp-{}", uuid::Uuid::new_v4());
        let team_a = uuid::Uuid::new_v4();
        let team_b = uuid::Uuid::new_v4();
        insert_test_session(&sid, Some(uuid::Uuid::new_v4()));

        stamp_team_activity(&sid, team_a);
        let (id_a1, first_a1) = {
            let sessions = sessions();
            let activity = &sessions[&sid].team_activity[&team_a];
            (activity.team_connection_id, activity.first_authorized_at)
        };

        stamp_team_activity(&sid, team_a);
        stamp_team_activity(&sid, team_b);
        {
            let sessions = sessions();
            let session = &sessions[&sid];
            let activity_a = &session.team_activity[&team_a];
            let activity_b = &session.team_activity[&team_b];
            // Re-stamp on a fresh entry keeps the id and first-authorized instant.
            assert_eq!(activity_a.team_connection_id, id_a1);
            assert_eq!(activity_a.first_authorized_at, first_a1);
            assert!(activity_a.last_authorized_at >= first_a1);
            // Per-team ids are distinct across teams.
            assert_ne!(activity_b.team_connection_id, id_a1);
        }
        remove_test_sessions(&[&sid]);
    }

    #[test]
    fn stamp_after_stale_entry_mints_new_team_connection_id() {
        let sid = format!("mcp-{}", uuid::Uuid::new_v4());
        let team = uuid::Uuid::new_v4();
        insert_test_session(&sid, Some(uuid::Uuid::new_v4()));
        let stale_instant = Instant::now() - (SESSION_TTL + Duration::from_secs(60));
        let old_id = uuid::Uuid::new_v4();
        sessions().get_mut(&sid).unwrap().team_activity.insert(
            team,
            TeamActivity {
                team_connection_id: old_id,
                first_authorized_at: stale_instant,
                last_authorized_at: stale_instant,
            },
        );

        stamp_team_activity(&sid, team);
        {
            let sessions = sessions();
            let activity = &sessions[&sid].team_activity[&team];
            assert_ne!(activity.team_connection_id, old_id);
            assert!(activity.first_authorized_at > stale_instant);
        }
        remove_test_sessions(&[&sid]);
    }

    #[test]
    fn stamp_on_unknown_session_is_a_noop() {
        let sid = format!("mcp-{}", uuid::Uuid::new_v4());
        stamp_team_activity(&sid, uuid::Uuid::new_v4());
        assert!(!sessions().contains_key(&sid));
    }

    #[test]
    fn visible_sessions_requires_nonstale_activity_on_the_requested_team() {
        let org = OrgId::generate();
        let other_org = OrgId::generate();
        let team = fp_domain::TeamId::generate();
        let team_ref = TeamRef {
            id: team,
            org_id: org,
        };
        let ctx = test_user_ctx(org);

        let sid_active = format!("mcp-{}", uuid::Uuid::new_v4());
        let sid_other_team = format!("mcp-{}", uuid::Uuid::new_v4());
        let sid_stale = format!("mcp-{}", uuid::Uuid::new_v4());
        let sid_other_org = format!("mcp-{}", uuid::Uuid::new_v4());
        let sid_orgless = format!("mcp-{}", uuid::Uuid::new_v4());
        insert_test_session(&sid_active, Some(org.as_uuid()));
        insert_test_session(&sid_other_team, Some(org.as_uuid()));
        insert_test_session(&sid_stale, Some(org.as_uuid()));
        insert_test_session(&sid_other_org, Some(other_org.as_uuid()));
        insert_test_session(&sid_orgless, None);

        stamp_team_activity(&sid_active, team.as_uuid());
        stamp_team_activity(&sid_other_team, uuid::Uuid::new_v4());
        stamp_team_activity(&sid_other_org, team.as_uuid());
        stamp_team_activity(&sid_orgless, team.as_uuid());
        let stale_instant = Instant::now() - (SESSION_TTL + Duration::from_secs(60));
        sessions()
            .get_mut(&sid_stale)
            .unwrap()
            .team_activity
            .insert(
                team.as_uuid(),
                TeamActivity {
                    team_connection_id: uuid::Uuid::new_v4(),
                    first_authorized_at: stale_instant,
                    last_authorized_at: stale_instant,
                },
            );

        let views = visible_sessions(&ctx, team_ref);
        let expected_id = sessions()[&sid_active].team_activity[&team.as_uuid()].team_connection_id;
        // Other parallel tests may add their own sessions, but none can produce this
        // team's id: assert our session is present exactly once and the others absent.
        assert_eq!(
            views
                .iter()
                .filter(|view| view.connection_id == expected_id)
                .count(),
            1
        );
        // None of the other sessions' per-team ids may appear in this team's view.
        let foreign_ids: Vec<uuid::Uuid> =
            [&sid_other_team, &sid_stale, &sid_other_org, &sid_orgless]
                .iter()
                .flat_map(|sid| {
                    sessions()[sid.as_str()]
                        .team_activity
                        .values()
                        .map(|activity| activity.team_connection_id)
                        .collect::<Vec<_>>()
                })
                .collect();
        for view in &views {
            assert!(!foreign_ids.contains(&view.connection_id));
        }
        remove_test_sessions(&[
            &sid_active,
            &sid_other_team,
            &sid_stale,
            &sid_other_org,
            &sid_orgless,
        ]);
    }

    #[test]
    fn session_validation_and_ping_do_not_extend_lifetime_but_stamping_does() {
        let sid = format!("mcp-{}", uuid::Uuid::new_v4());
        insert_test_session(&sid, Some(uuid::Uuid::new_v4()));
        let t_old = Instant::now() - Duration::from_secs(100);
        sessions().get_mut(&sid).unwrap().last_seen = t_old;
        let principal = format!("test:{sid}");
        let mut headers = HeaderMap::new();
        headers.insert("mcp-session-id", HeaderValue::from_str(&sid).unwrap());
        let rid = RequestId::generate();

        // validate_session succeeds but must not refresh last_seen (pre-authz); it does
        // mark the session in-flight so concurrent cleanup can't reap it mid-request.
        let guard = validate_session(&headers, &principal, None, rid).expect("must validate");
        assert_eq!(
            sessions()[&sid].last_seen,
            t_old,
            "validate must not refresh"
        );
        assert_eq!(sessions()[&sid].in_flight, 1, "validate marks in-flight");
        drop(guard);
        assert_eq!(sessions()[&sid].in_flight, 0, "guard drop clears in-flight");

        // ping (with_session) succeeds but must not refresh last_seen.
        let response = with_session(&headers, &principal, None, rid, || json!({}));
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(sessions()[&sid].last_seen, t_old, "ping must not refresh");

        // A successfully authorized team operation (the stamp) is what refreshes.
        stamp_team_activity(&sid, uuid::Uuid::new_v4());
        assert!(sessions()[&sid].last_seen > t_old, "stamp must refresh");
        remove_test_sessions(&[&sid]);
    }

    #[test]
    fn unstamped_stale_session_is_reaped_while_stamped_one_survives() {
        let sid_stale = format!("mcp-{}", uuid::Uuid::new_v4());
        let sid_stamped = format!("mcp-{}", uuid::Uuid::new_v4());
        insert_test_session(&sid_stale, Some(uuid::Uuid::new_v4()));
        insert_test_session(&sid_stamped, Some(uuid::Uuid::new_v4()));
        let t_old = Instant::now() - (SESSION_TTL + Duration::from_secs(60));
        {
            let mut sessions = sessions();
            sessions.get_mut(&sid_stale).unwrap().last_seen = t_old;
            sessions.get_mut(&sid_stamped).unwrap().last_seen = t_old;
        }
        // Only the session with an authorized operation gets its lifetime extended.
        stamp_team_activity(&sid_stamped, uuid::Uuid::new_v4());

        cleanup_sessions_at(Instant::now(), SESSION_TTL);
        {
            let sessions = sessions();
            assert!(
                !sessions.contains_key(&sid_stale),
                "ping-/denied-only session expires at TTL"
            );
            assert!(
                sessions.contains_key(&sid_stamped),
                "authorized activity keeps the session alive"
            );
        }
        remove_test_sessions(&[&sid_stamped]);
    }

    #[test]
    fn in_flight_session_survives_cleanup_at_ttl_boundary() {
        // Regression for the TTL-boundary race: a request that validated just before its
        // last_seen crossed the TTL must not be reaped by a concurrent cleanup while its
        // authorization is still in flight (before it can stamp).
        let sid = format!("mcp-{}", uuid::Uuid::new_v4());
        insert_test_session(&sid, Some(uuid::Uuid::new_v4()));
        let principal = format!("test:{sid}");
        let mut headers = HeaderMap::new();
        headers.insert("mcp-session-id", HeaderValue::from_str(&sid).unwrap());

        // Session's last_seen is already past the TTL; a concurrent request validates it
        // (guard alive = in flight) before cleanup runs.
        sessions().get_mut(&sid).unwrap().last_seen =
            Instant::now() - (SESSION_TTL + Duration::from_secs(60));
        let guard = validate_session(&headers, &principal, None, RequestId::generate())
            .expect("must validate");

        cleanup_sessions_at(Instant::now(), SESSION_TTL);
        assert!(
            sessions().contains_key(&sid),
            "in-flight session must survive cleanup even past its TTL"
        );

        // Once the request completes (guard dropped) and no authorized stamp refreshed it,
        // a later cleanup reaps it normally.
        drop(guard);
        cleanup_sessions_at(Instant::now(), SESSION_TTL);
        assert!(
            !sessions().contains_key(&sid),
            "after the request finishes with no stamp, the stale session is reaped"
        );
    }

    #[test]
    fn visible_sessions_renders_team_relative_age_and_idle() {
        // Deterministic pin of team-relative clock rendering (no sleeps, crafted
        // Instants): a session-global idle/age source would fail these assertions.
        let org = OrgId::generate();
        let team_a = fp_domain::TeamId::generate();
        let team_b = fp_domain::TeamId::generate();
        let ctx = test_user_ctx(org);
        let sid = format!("mcp-{}", uuid::Uuid::new_v4());
        insert_test_session(&sid, Some(org.as_uuid()));

        let now = Instant::now();
        {
            let mut sessions = sessions();
            let session = sessions.get_mut(&sid).unwrap();
            // Team A: first authorized 300s ago, last authorized 100s ago.
            session.team_activity.insert(
                team_a.as_uuid(),
                TeamActivity {
                    team_connection_id: uuid::Uuid::new_v4(),
                    first_authorized_at: now - Duration::from_secs(300),
                    last_authorized_at: now - Duration::from_secs(100),
                },
            );
            // Team B: authorized just now.
            session.team_activity.insert(
                team_b.as_uuid(),
                TeamActivity {
                    team_connection_id: uuid::Uuid::new_v4(),
                    first_authorized_at: now,
                    last_authorized_at: now,
                },
            );
            // A session-global source would read this and report ~0 idle everywhere.
            session.last_seen = now;
        }

        let view_a = visible_sessions(
            &ctx,
            TeamRef {
                id: team_a,
                org_id: org,
            },
        );
        let view_b = visible_sessions(
            &ctx,
            TeamRef {
                id: team_b,
                org_id: org,
            },
        );
        let expected_a = sessions()[&sid].team_activity[&team_a.as_uuid()].team_connection_id;
        let entry_a = view_a
            .iter()
            .find(|view| view.connection_id == expected_a)
            .expect("team A entry");
        // ±1s tolerance for elapsed test time around the crafted instants.
        assert!(
            (99..=101).contains(&entry_a.idle_seconds),
            "A idle from A's last_authorized_at"
        );
        assert!(
            (299..=301).contains(&entry_a.age_seconds),
            "A age from A's first_authorized_at"
        );
        let expected_b = sessions()[&sid].team_activity[&team_b.as_uuid()].team_connection_id;
        let entry_b = view_b
            .iter()
            .find(|view| view.connection_id == expected_b)
            .expect("team B entry");
        assert!(entry_b.idle_seconds <= 1, "B idle from B's own clock");
        assert!(entry_b.age_seconds <= 1, "B age from B's own clock");
        remove_test_sessions(&[&sid]);
    }

    #[test]
    fn cleanup_prunes_stale_sessions_and_stale_team_entries() {
        let sid_live = format!("mcp-{}", uuid::Uuid::new_v4());
        let sid_dead = format!("mcp-{}", uuid::Uuid::new_v4());
        let team_fresh = uuid::Uuid::new_v4();
        let team_stale = uuid::Uuid::new_v4();
        insert_test_session(&sid_live, Some(uuid::Uuid::new_v4()));
        insert_test_session(&sid_dead, Some(uuid::Uuid::new_v4()));

        let now = Instant::now();
        let stale_instant = now - (SESSION_TTL + Duration::from_secs(60));
        {
            let mut sessions = sessions();
            let live = sessions.get_mut(&sid_live).unwrap();
            live.team_activity.insert(
                team_fresh,
                TeamActivity {
                    team_connection_id: uuid::Uuid::new_v4(),
                    first_authorized_at: now,
                    last_authorized_at: now,
                },
            );
            live.team_activity.insert(
                team_stale,
                TeamActivity {
                    team_connection_id: uuid::Uuid::new_v4(),
                    first_authorized_at: stale_instant,
                    last_authorized_at: stale_instant,
                },
            );
            sessions.get_mut(&sid_dead).unwrap().last_seen = stale_instant;
        }

        cleanup_sessions_at(now, SESSION_TTL);
        {
            let sessions = sessions();
            assert!(!sessions.contains_key(&sid_dead));
            let live = &sessions[&sid_live];
            assert!(live.team_activity.contains_key(&team_fresh));
            assert!(!live.team_activity.contains_key(&team_stale));
        }
        remove_test_sessions(&[&sid_live]);
    }

    #[test]
    fn origin_match_ignores_port() {
        assert!(origin_matches("http://localhost", "http://localhost:3000"));
        assert!(origin_matches("http://[::1]", "http://[::1]:3000"));
        assert!(!origin_matches("https://localhost", "http://localhost"));
        assert!(!origin_matches("http://localhost", "http://example.com"));
    }

    #[test]
    fn dynamic_tool_url_encodes_path_params_as_segments() {
        let now = chrono::Utc::now();
        let tool = ApiTool {
            id: fp_domain::ApiToolId::generate(),
            team_id: fp_domain::TeamId::generate(),
            api_definition_id: fp_domain::ApiDefinitionId::generate(),
            spec_version_id: fp_domain::SpecVersionId::generate(),
            name: "catalog-get".into(),
            operation_id: "getItem".into(),
            method: HttpMethod::Get,
            path: "/items/{id}".into(),
            input_schema: json!({}),
            output_schema: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        };

        let url = dynamic_tool_url(
            &tool,
            &json!({ "pathParams": { "id": "a/b ?#%" }, "query": { "q": "../admin" } }),
            "https://gateway.example",
        )
        .expect("url");
        assert_eq!(
            url.as_str(),
            "https://gateway.example/items/a%2Fb%20%3F%23%25?q=..%2Fadmin"
        );

        let url = dynamic_tool_url(
            &tool,
            &json!({ "pathParams": { "id": "../admin" } }),
            "https://gateway.example",
        )
        .expect("url");
        assert_eq!(url.path(), "/items/..%2Fadmin");

        for id in [".", "..", "%2e", "%2E%2e", ".%2e", "%2e."] {
            let err = dynamic_tool_url(
                &tool,
                &json!({ "pathParams": { "id": id } }),
                "https://gateway.example",
            )
            .expect_err("dot segment must be rejected");
            assert!(
                err.to_string().contains("must not be a dot segment"),
                "{err}"
            );
        }
    }

    #[test]
    fn dynamic_descriptor_uses_public_base_url_and_binding_host() {
        let now = chrono::Utc::now();
        let tool = ApiTool {
            id: fp_domain::ApiToolId::generate(),
            team_id: fp_domain::TeamId::generate(),
            api_definition_id: fp_domain::ApiDefinitionId::generate(),
            spec_version_id: fp_domain::SpecVersionId::generate(),
            name: "catalog-get".into(),
            operation_id: "getItem".into(),
            method: HttpMethod::Get,
            path: "/items/{id}".into(),
            input_schema: json!({}),
            output_schema: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        let listener = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 18080,
            public_base_url: Some("https://gateway.example".into()),
            protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
            route_config: Some("routes".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };
        let binding = ApiRouteBinding {
            id: fp_domain::ApiRouteBindingId::generate(),
            team_id: fp_domain::TeamId::generate(),
            api_definition_id: tool.api_definition_id,
            route_config_id: fp_domain::RouteConfigId::generate(),
            listener_id: Some(fp_domain::ListenerId::generate()),
            name: "binding".into(),
            virtual_host: Some("api.example".into()),
            route: None,
            created_at: now,
        };

        let descriptor = dynamic_tool_descriptor(
            &tool,
            &json!({
                "pathParams": { "id": "123" },
                "query": { "debug": true },
                "headers": { "x-requested-by": "mcp" },
                "body": { "ignoredForGet": true }
            }),
            &listener,
            &binding,
            RequestId::generate(),
        )
        .expect("descriptor");

        assert_eq!(descriptor["type"], "gateway_invocation");
        assert_eq!(descriptor["tool"], "api_catalog-get");
        assert_eq!(descriptor["method"], "GET");
        assert_eq!(
            descriptor["url"],
            "https://gateway.example/items/123?debug=true"
        );
        assert_eq!(descriptor["headers"]["host"], "api.example");
        assert_eq!(descriptor["headers"]["x-requested-by"], "mcp");
        assert_eq!(descriptor["auth"]["mode"], "caller_gateway_credentials");
        assert!(descriptor["expiresAt"].as_str().is_some());
        assert!(descriptor["correlationId"].as_str().is_some());
    }

    #[test]
    fn dynamic_descriptor_rejects_missing_endpoint_and_host_override() {
        let now = chrono::Utc::now();
        let tool = ApiTool {
            id: fp_domain::ApiToolId::generate(),
            team_id: fp_domain::TeamId::generate(),
            api_definition_id: fp_domain::ApiDefinitionId::generate(),
            spec_version_id: fp_domain::SpecVersionId::generate(),
            name: "catalog-get".into(),
            operation_id: "getItem".into(),
            method: HttpMethod::Get,
            path: "/items/{id}".into(),
            input_schema: json!({}),
            output_schema: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        let mut listener = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 18080,
            public_base_url: None,
            protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
            route_config: Some("routes".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };
        let binding = ApiRouteBinding {
            id: fp_domain::ApiRouteBindingId::generate(),
            team_id: fp_domain::TeamId::generate(),
            api_definition_id: tool.api_definition_id,
            route_config_id: fp_domain::RouteConfigId::generate(),
            listener_id: Some(fp_domain::ListenerId::generate()),
            name: "binding".into(),
            virtual_host: Some("api.example".into()),
            route: None,
            created_at: now,
        };

        let err = dynamic_tool_descriptor(
            &tool,
            &json!({ "pathParams": { "id": "123" } }),
            &listener,
            &binding,
            RequestId::generate(),
        )
        .expect_err("missing public endpoint");
        assert_eq!(err.code, ErrorCode::Conflict);

        listener.public_base_url = Some("https://gateway.example".into());
        let err = dynamic_tool_descriptor(
            &tool,
            &json!({ "headers": { "Host": "other.example" } }),
            &listener,
            &binding,
            RequestId::generate(),
        )
        .expect_err("host override rejected");
        assert!(err
            .to_string()
            .contains("controlled by the api route binding"));

        let err = dynamic_tool_descriptor(
            &tool,
            &json!({ "headers": { ":authority": "other.example" } }),
            &listener,
            &binding,
            RequestId::generate(),
        )
        .expect_err("authority override rejected");
        assert!(err
            .to_string()
            .contains("controlled by the api route binding"));
    }

    #[test]
    fn dynamic_input_schema_keeps_spec_required_fields() {
        let schema = fp_core::mcp_declarations::dynamic_input_schema(&json!({
            "properties": { "pathParams": { "type": "object" } },
            "required": ["pathParams"]
        }));

        assert_eq!(schema["required"], json!(["pathParams", "team"]));
    }

    fn user(user_id: UserId, org_id: OrgId) -> PrincipalCtx {
        PrincipalCtx::User {
            user_id,
            platform_admin: false,
            org: Some((org_id, OrgRole::Admin)),
            org_selector_required: false,
            grants: GrantSet::default(),
        }
    }

    fn state() -> AppState {
        AppState {
            pool: PgPoolOptions::new()
                .connect_lazy("postgres://postgres:postgres@localhost/unused")
                .expect("lazy pool"),
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            version: "test",
            validator: None,
            write_throttle: std::sync::Arc::new(crate::throttle::WriteThrottle::new(1000)),
            xds_readiness: None,
            discovery_forwarding_policy: Default::default(),
            egress_advisory: Default::default(),
            rls_repush: None,
            rls_grpc_configured: false,
        }
    }

    fn initialize_request() -> axum::body::Bytes {
        serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": { "protocolVersion": PREFERRED_VERSION },
            "id": 1,
        }))
        .unwrap()
        .into()
    }

    fn ping_request() -> axum::body::Bytes {
        serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": "ping",
            "params": {},
            "id": 2,
        }))
        .unwrap()
        .into()
    }

    async fn json_body(response: Response<Body>) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    #[test]
    fn declarations_and_executor_bindings_are_a_bijection_with_matching_authz() {
        // Completeness/uniqueness both directions: a declaration without a binding would
        // be listed/catalogued but undispatchable; a binding without a declaration would
        // be dispatchable but invisible. Either is contract drift (invariant 13).
        let mut bound_names = HashSet::new();
        for (name, _) in EXECUTOR_BINDINGS {
            assert!(
                bound_names.insert(*name),
                "duplicate executor binding {name}"
            );
        }
        assert_eq!(
            STATIC_TOOL_DECLS.len(),
            EXECUTOR_BINDINGS.len(),
            "declaration/binding count mismatch"
        );
        for tool in STATIC_TOOL_DECLS {
            let executor = executor_binding(tool.name)
                .unwrap_or_else(|| panic!("{} has no executor binding", tool.name));
            assert_eq!(
                (tool.resource, tool.action),
                executor_authz(executor),
                "{} authz metadata drifted from executor",
                tool.name
            );
        }
        for (name, _) in EXECUTOR_BINDINGS {
            assert!(
                static_tool(name).is_some(),
                "executor binding {name} names no declaration"
            );
        }
    }

    #[tokio::test]
    async fn initialize_and_ping_work_without_origin_header() {
        let ctx = user(UserId::generate(), OrgId::generate());
        let response = post(
            State(state()),
            HeaderMap::new(),
            Extension(ctx.clone()),
            Extension(RequestId::generate()),
            initialize_request(),
        )
        .await;
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .expect("session")
            .to_string();
        let body = json_body(response).await;
        assert_eq!(body["result"]["protocolVersion"], PREFERRED_VERSION);

        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&session).expect("session header"),
        );
        let response = post(
            State(state()),
            headers,
            Extension(ctx),
            Extension(RequestId::generate()),
            ping_request(),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["result"], json!({}));
    }

    #[tokio::test]
    async fn session_rejects_different_reauthenticated_principal() {
        let org_id = OrgId::generate();
        let response = post(
            State(state()),
            HeaderMap::new(),
            Extension(user(UserId::generate(), org_id)),
            Extension(RequestId::generate()),
            initialize_request(),
        )
        .await;
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .expect("session")
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&session).expect("session header"),
        );
        let response = post(
            State(state()),
            headers,
            Extension(user(UserId::generate(), org_id)),
            Extension(RequestId::generate()),
            ping_request(),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["error"]["data"]["kind"], "authz");
    }

    #[tokio::test]
    async fn bad_origin_and_protocol_fail_closed() {
        let ctx = user(UserId::generate(), OrgId::generate());
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://denied.example"));
        let response = post(
            State(state()),
            headers,
            Extension(ctx.clone()),
            Extension(RequestId::generate()),
            initialize_request(),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["error"]["data"]["kind"], "origin");

        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static("1999-01-01"),
        );
        let response = post(
            State(state()),
            headers,
            Extension(ctx),
            Extension(RequestId::generate()),
            initialize_request(),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["error"]["data"]["kind"], "protocol");
    }
}
