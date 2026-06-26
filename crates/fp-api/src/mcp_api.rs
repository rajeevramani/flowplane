use crate::state::AppState;
use crate::{error::ApiError, resources::resolve_team};
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{body::Body, Json};
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
    connection_id: uuid::Uuid,
    created_at: Instant,
    last_seen: Instant,
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
            Some(response) => response,
            None => tools_list(&state, &ctx, req.id, req.params, rid).await,
        },
        "tools/call" => match validate_session(&headers, &principal, id, rid) {
            Some(response) => response,
            None => tools_call(&state, &ctx, req.id, req.params, rid).await,
        },
        _ => rpc_error(req.id, -32601, "method not found", rid, "method").into_response(),
    };
    if let Ok(value) = HeaderValue::from_str(&version) {
        response.headers_mut().insert("mcp-protocol-version", value);
    }
    response
}

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
        let active_sessions = visible_sessions(&ctx).len();
        Ok::<_, DomainError>(McpStatusView {
            transport: "streamable_http_post".into(),
            preferred_protocol_version: PREFERRED_VERSION.into(),
            supported_protocol_versions: SUPPORTED_VERSIONS
                .iter()
                .map(|version| (*version).to_string())
                .collect(),
            session_ttl_seconds: SESSION_TTL.as_secs(),
            active_sessions,
            static_tool_count: STATIC_TOOLS.len(),
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
        Ok::<_, DomainError>(visible_sessions(&ctx))
    };
    run.await.map(Json).map_err(|e| ApiError::new(e, rid))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolRisk {
    Read,
    Mutate,
    Delete,
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

#[derive(Clone, Copy, Debug)]
struct StaticTool {
    name: &'static str,
    description: &'static str,
    resource: Resource,
    action: Action,
    risk: ToolRisk,
    input_schema: fn() -> Value,
    executor: ToolExecutor,
}

const STATIC_TOOLS: &[StaticTool] = &[
    StaticTool {
        name: "cp_clusters_list",
        description: "List upstream clusters for one team.",
        resource: Resource::Clusters,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::ClusterList,
    },
    StaticTool {
        name: "cp_clusters_get",
        description: "Read one upstream cluster by name.",
        resource: Resource::Clusters,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::ClusterGet,
    },
    StaticTool {
        name: "cp_clusters_create",
        description: "Create an upstream cluster.",
        resource: Resource::Clusters,
        action: Action::Create,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec,
        executor: ToolExecutor::ClusterCreate,
    },
    StaticTool {
        name: "cp_clusters_update",
        description: "Update an upstream cluster using an expected revision.",
        resource: Resource::Clusters,
        action: Action::Update,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec_revision,
        executor: ToolExecutor::ClusterUpdate,
    },
    StaticTool {
        name: "cp_clusters_delete",
        description: "Delete an upstream cluster using an expected revision.",
        resource: Resource::Clusters,
        action: Action::Delete,
        risk: ToolRisk::Delete,
        input_schema: schema_named_revision,
        executor: ToolExecutor::ClusterDelete,
    },
    StaticTool {
        name: "cp_route_configs_list",
        description: "List route configs for one team.",
        resource: Resource::RouteConfigs,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::RouteConfigList,
    },
    StaticTool {
        name: "cp_route_configs_get",
        description: "Read one route config by name.",
        resource: Resource::RouteConfigs,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::RouteConfigGet,
    },
    StaticTool {
        name: "cp_route_configs_create",
        description: "Create a route config.",
        resource: Resource::RouteConfigs,
        action: Action::Create,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec,
        executor: ToolExecutor::RouteConfigCreate,
    },
    StaticTool {
        name: "cp_route_configs_update",
        description: "Update a route config using an expected revision.",
        resource: Resource::RouteConfigs,
        action: Action::Update,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec_revision,
        executor: ToolExecutor::RouteConfigUpdate,
    },
    StaticTool {
        name: "cp_route_configs_delete",
        description: "Delete a route config using an expected revision.",
        resource: Resource::RouteConfigs,
        action: Action::Delete,
        risk: ToolRisk::Delete,
        input_schema: schema_named_revision,
        executor: ToolExecutor::RouteConfigDelete,
    },
    StaticTool {
        name: "cp_listeners_list",
        description: "List listeners for one team.",
        resource: Resource::Listeners,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::ListenerList,
    },
    StaticTool {
        name: "cp_listeners_get",
        description: "Read one listener by name.",
        resource: Resource::Listeners,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::ListenerGet,
    },
    StaticTool {
        name: "cp_listeners_create",
        description: "Create a listener.",
        resource: Resource::Listeners,
        action: Action::Create,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec,
        executor: ToolExecutor::ListenerCreate,
    },
    StaticTool {
        name: "cp_listeners_update",
        description: "Update a listener using an expected revision.",
        resource: Resource::Listeners,
        action: Action::Update,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec_revision,
        executor: ToolExecutor::ListenerUpdate,
    },
    StaticTool {
        name: "cp_listeners_delete",
        description: "Delete a listener using an expected revision.",
        resource: Resource::Listeners,
        action: Action::Delete,
        risk: ToolRisk::Delete,
        input_schema: schema_named_revision,
        executor: ToolExecutor::ListenerDelete,
    },
    StaticTool {
        name: "cp_apis_list",
        description: "List API definitions for one team.",
        resource: Resource::ApiDefinitions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::ApiList,
    },
    StaticTool {
        name: "cp_apis_get",
        description: "Read one API definition by name.",
        resource: Resource::ApiDefinitions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::ApiGet,
    },
    StaticTool {
        name: "cp_apis_status",
        description: "Read publish/spec/tool status for one API definition.",
        resource: Resource::ApiDefinitions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::ApiStatus,
    },
    StaticTool {
        name: "cp_learning_sessions_list",
        description: "List learning capture sessions for one team.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::LearningList,
    },
    StaticTool {
        name: "cp_learning_sessions_get",
        description: "Read one learning capture session by name or UUID.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::LearningGet,
    },
    StaticTool {
        name: "cp_discovery_sessions_list",
        description: "List passive discovery sessions for one team.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::DiscoveryList,
    },
    StaticTool {
        name: "cp_discovery_sessions_get",
        description: "Read one passive discovery session by name or UUID.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::DiscoveryGet,
    },
    StaticTool {
        name: "ops_xds_status",
        description: "Summarize xDS dataplane and recent NACK status for one team.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_team,
        executor: ToolExecutor::OpsXdsStatus,
    },
    StaticTool {
        name: "ops_xds_nacks",
        description: "List recent xDS NACK events for one team.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::OpsXdsNacks,
    },
    StaticTool {
        name: "ops_xds_trace",
        description: "Trace audit/outbox rows by request id, trace id, or path fragment.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_trace,
        executor: ToolExecutor::OpsXdsTrace,
    },
    StaticTool {
        name: "ops_stats_overview",
        description: "Summarize dataplane request/error telemetry for one team.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_team,
        executor: ToolExecutor::OpsStatsOverview,
    },
    StaticTool {
        name: "cp_secrets_list",
        description: "List secret metadata for one team. Secret values are never returned.",
        resource: Resource::Secrets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::SecretsList,
    },
    StaticTool {
        name: "cp_secrets_get",
        description: "Read one secret metadata record. Secret values are never returned.",
        resource: Resource::Secrets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::SecretsGet,
    },
    StaticTool {
        name: "cp_ai_providers_list",
        description: "List AI providers for one team.",
        resource: Resource::AiProviders,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::AiProvidersList,
    },
    StaticTool {
        name: "cp_ai_providers_get",
        description: "Read one AI provider by name.",
        resource: Resource::AiProviders,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::AiProvidersGet,
    },
    StaticTool {
        name: "cp_ai_routes_list",
        description: "List AI routes for one team.",
        resource: Resource::AiRoutes,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::AiRoutesList,
    },
    StaticTool {
        name: "cp_ai_routes_get",
        description: "Read one AI route by name.",
        resource: Resource::AiRoutes,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::AiRoutesGet,
    },
    StaticTool {
        name: "cp_ai_budgets_list",
        description: "List AI budgets for one team.",
        resource: Resource::AiBudgets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::AiBudgetsList,
    },
    StaticTool {
        name: "cp_ai_budgets_get",
        description: "Read one AI budget by name.",
        resource: Resource::AiBudgets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
        executor: ToolExecutor::AiBudgetsGet,
    },
    StaticTool {
        name: "cp_ai_usage",
        description: "Read AI usage summary rows for one team.",
        resource: Resource::AiUsage,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
        executor: ToolExecutor::AiUsage,
    },
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
            connection_id: uuid::Uuid::new_v4(),
            created_at: now,
            last_seen: now,
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
) -> Response {
    let team = match resolve_tool_team(state, ctx, &params).await {
        Ok(team) => team,
        Err(e) => return rpc_error(id, -32600, e.message, rid, "validation").into_response(),
    };
    let tools = STATIC_TOOLS
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
    let mut tools = tools;
    if dynamic_tool_allowed(ctx, team, Action::Execute) {
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
        return match execute_dynamic_tool(state, ctx, api_tool_name, arguments, rid).await {
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
    tool: &StaticTool,
    team: TeamRef,
    arguments: Value,
    rid: RequestId,
) -> DomainResult<Value> {
    match tool.executor {
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
            let items = fp_core::services::ai::usage_summary(
                &state.pool,
                ctx,
                team,
                fp_storage::repos::ai::AiUsageQuery {
                    route_config_id: None,
                    provider_id: None,
                    limit: integer_arg(&arguments, "limit").unwrap_or(50),
                    offset: integer_arg(&arguments, "offset").unwrap_or(0),
                },
                rid,
            )
            .await?;
            serde_json::to_value(items).map_err(json_err)
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
    let mut sessions = sessions();
    let Some(session) = sessions.get_mut(session_id) else {
        return rpc_error(id, -32600, "unknown MCP session", rid, "session").into_response();
    };
    if session.principal != principal {
        return rpc_error(id, -32600, "MCP session principal mismatch", rid, "authz")
            .into_response();
    }
    session.last_seen = Instant::now();
    rpc_result(id, result()).into_response()
}

fn validate_session(
    headers: &HeaderMap,
    principal: &str,
    id: Option<Value>,
    rid: RequestId,
) -> Option<Response> {
    let Some(session_id) = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| valid_session_id(s))
    else {
        return Some(
            rpc_error(
                id,
                -32600,
                "missing or invalid MCP-Session-Id",
                rid,
                "session",
            )
            .into_response(),
        );
    };
    let mut sessions = sessions();
    let Some(session) = sessions.get_mut(session_id) else {
        return Some(rpc_error(id, -32600, "unknown MCP session", rid, "session").into_response());
    };
    if session.principal != principal {
        return Some(
            rpc_error(id, -32600, "MCP session principal mismatch", rid, "authz").into_response(),
        );
    }
    session.last_seen = Instant::now();
    None
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

fn visible_sessions(ctx: &PrincipalCtx) -> Vec<McpConnectionView> {
    let meta = principal_metadata(ctx);
    let now = Instant::now();
    sessions()
        .values()
        .filter(|session| session.org_id.is_some() && session.org_id == meta.org_id)
        .map(|session| McpConnectionView {
            connection_id: session.connection_id,
            principal_kind: session.principal_kind.into(),
            transport: "streamable_http_post".into(),
            sse: false,
            age_seconds: now.duration_since(session.created_at).as_secs(),
            idle_seconds: now.duration_since(session.last_seen).as_secs(),
        })
        .collect()
}

fn dynamic_tool_view(tool: ApiTool) -> Value {
    json!({
        "name": format!("api_{}", tool.name),
        "description": format!("{} {}", tool.method.as_str(), tool.path),
        "inputSchema": dynamic_input_schema(&tool.input_schema),
        "annotations": {
            "resource": Resource::McpTools.as_str(),
            "action": Action::Execute.as_str(),
            "risk": "mutate",
            "apiToolId": tool.id.as_uuid(),
            "apiDefinitionId": tool.api_definition_id.as_uuid(),
            "specVersionId": tool.spec_version_id.as_uuid(),
            "operationId": tool.operation_id,
            "method": tool.method.as_str(),
            "path": tool.path,
        }
    })
}

fn dynamic_input_schema(schema: &Value) -> Value {
    let mut schema = schema.as_object().cloned().unwrap_or_default();
    schema.insert("type".into(), json!("object"));
    let mut properties = schema
        .remove("properties")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    properties.insert(
        "team".into(),
        json!({ "type": "string", "description": "Team name or UUID" }),
    );
    properties
        .entry("pathParams")
        .or_insert_with(|| json!({ "type": "object" }));
    properties
        .entry("query")
        .or_insert_with(|| json!({ "type": "object" }));
    properties
        .entry("headers")
        .or_insert_with(|| json!({ "type": "object" }));
    properties.entry("body").or_insert_with(|| json!({}));
    schema.insert("properties".into(), Value::Object(properties));
    let mut required = schema
        .remove("required")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if !required.iter().any(|v| v.as_str() == Some("team")) {
        required.push(json!("team"));
    }
    schema.insert("required".into(), Value::Array(required));
    Value::Object(schema)
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

fn static_tool(name: &str) -> Option<&'static StaticTool> {
    STATIC_TOOLS.iter().find(|tool| tool.name == name)
}

fn tool_allowed(ctx: &PrincipalCtx, tool: &StaticTool, team: TeamRef) -> bool {
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

impl ToolRisk {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mutate => "mutate",
            Self::Delete => "delete",
        }
    }
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

fn schema_team() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" }
        },
        "required": ["team"],
        "additionalProperties": false
    })
}

fn schema_list() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 },
            "offset": { "type": "integer", "minimum": 0, "default": 0 }
        },
        "required": ["team"],
        "additionalProperties": false
    })
}

fn schema_named() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" }
        },
        "required": ["team", "name"],
        "additionalProperties": false
    })
}

fn schema_named_spec() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" },
            "spec": { "type": "object" }
        },
        "required": ["team", "name", "spec"],
        "additionalProperties": false
    })
}

fn schema_named_revision() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" },
            "revision": { "type": "integer" }
        },
        "required": ["team", "name", "revision"],
        "additionalProperties": false
    })
}

fn schema_named_spec_revision() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" },
            "spec": { "type": "object" },
            "revision": { "type": "integer" }
        },
        "required": ["team", "name", "spec", "revision"],
        "additionalProperties": false
    })
}

fn schema_trace() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "requestId": { "type": "string" },
            "traceId": { "type": "string" },
            "path": { "type": "string" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
        },
        "required": ["team"],
        "additionalProperties": false
    })
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
    let now = Instant::now();
    sessions().retain(|_, session| now.duration_since(session.last_seen) <= SESSION_TTL);
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
        let schema = dynamic_input_schema(&json!({
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
    fn static_registry_has_unique_names_and_matching_executor_authz() {
        let mut names = HashSet::new();
        for tool in STATIC_TOOLS {
            assert!(names.insert(tool.name), "duplicate tool {}", tool.name);
            assert!(!tool.description.is_empty(), "missing description");
            assert!(matches!(
                (tool.risk, tool.action),
                (ToolRisk::Read, Action::Read)
                    | (ToolRisk::Mutate, Action::Create | Action::Update)
                    | (ToolRisk::Delete, Action::Delete)
            ));
            assert_eq!(
                (tool.resource, tool.action),
                executor_authz(tool.executor),
                "{} authz metadata drifted from executor",
                tool.name
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
