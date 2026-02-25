//! MCP Request Handler
//!
//! Routes incoming JSON-RPC requests to the appropriate method handlers.

use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, error, warn};

use crate::domain::OrgId;
use crate::mcp::error::McpError;
use crate::mcp::logging::SetLogLevelParams;
use crate::mcp::protocol::*;
use crate::mcp::resources;
use crate::mcp::tool_registry::{check_scope_grants_authorization, get_tool_authorization};
use crate::mcp::tools;
use crate::storage::DbPool;
use crate::xds::XdsState;

/// Server instructions provided to LLM clients during initialization.
///
/// This shapes how every AI agent interacts with Flowplane — explaining
/// resource creation order, xDS delivery semantics, and diagnostic workflows.
/// Keep this slim (~30 lines). Per-filter and per-resource guidance is
/// delivered contextually in tool responses and `cp_get_filter_type`.
const SERVER_INSTRUCTIONS: &str = r#"# Flowplane API Gateway Control Plane

## Resource Creation Order (ALWAYS follow)
1. Clusters (backends) — must exist before routes reference them
2. Route Configs (routing rules with virtual hosts)
3. Filters — create then attach to listeners or routes
4. Listeners (entry points) — bind to route configs

## Before Creating Resources
- cp_query_port: check if port is available
- cp_query_path: check if path is already routed
- dev_preflight_check: comprehensive pre-creation validation

## xDS Delivery (How Config Reaches Envoy)
- Flowplane pushes config to Envoy via xDS protocol
- If Envoy rejects a config, it NACKs and keeps the PREVIOUS valid config
- A NACK on one resource blocks ALL resources of that type in that batch
- Example: A bad cluster NACKs the entire CDS update, blocking unrelated clusters
- Use ops_xds_delivery_status to check if Envoy accepted or rejected config

## Route Matching: First-Match Wins
- Routes in a virtual host are evaluated top-to-bottom in array order
- The FIRST matching route handles the request
- More specific paths must come BEFORE broader prefixes
- Example: /api/health must come before /api in the routes array

## Route Config Updates Are Full Replacement
- cp_update_route_config replaces the entire virtualHosts array
- Always fetch existing config with cp_get_route_config first
- Include ALL existing routes plus any new ones

## Diagnostic Workflow
1. ops_topology: understand the full gateway layout
2. ops_trace_request: trace a specific request path
3. ops_config_validate: find configuration problems
4. ops_xds_delivery_status: check if Envoy accepted config
5. ops_nack_history: see recent config rejections

## Error Recovery
- ALREADY_EXISTS: query existing resource, reuse or choose different name
- NOT_FOUND: create the prerequisite first
- CONFLICT: check existing resource, resolve before retrying"#;

/// Validate MCP protocol version against the supported versions list.
///
/// We support all versions listed in SUPPORTED_VERSIONS. Versions not in this
/// list are rejected with an error message listing the supported versions.
fn validate_protocol_version(client_version: &str) -> Result<(), McpError> {
    if SUPPORTED_VERSIONS.contains(&client_version) {
        Ok(())
    } else {
        Err(McpError::UnsupportedProtocolVersion {
            client: client_version.to_string(),
            supported: SUPPORTED_VERSIONS.iter().map(|v| v.to_string()).collect(),
        })
    }
}

pub struct McpHandler {
    db_pool: Arc<DbPool>,
    xds_state: Option<Arc<XdsState>>,
    team: String,
    /// Scopes from the authenticated token for tool-level authorization
    scopes: Vec<String>,
    /// Organization ID from the authenticated user's context (None for CLI/system)
    org_id: Option<OrgId>,
    initialized: bool,
}

impl McpHandler {
    /// Create a new MCP handler with read-only capabilities
    ///
    /// # Arguments
    /// * `db_pool` - Database connection pool
    /// * `team` - Team context for multi-tenancy
    /// * `scopes` - Authorization scopes from the authenticated token
    // org_id is None: CLI MCP has direct machine access with admin:all scopes.
    // Org isolation is only enforced on the HTTP MCP path via `with_xds_state`.
    pub fn new(db_pool: Arc<DbPool>, team: String, scopes: Vec<String>) -> Self {
        Self { db_pool, xds_state: None, team, scopes, org_id: None, initialized: false }
    }

    /// Create a new MCP handler with full read/write capabilities
    ///
    /// # Arguments
    /// * `db_pool` - Database connection pool
    /// * `xds_state` - XDS state for control plane operations
    /// * `team` - Team context for multi-tenancy
    /// * `scopes` - Authorization scopes from the authenticated token
    pub fn with_xds_state(
        db_pool: Arc<DbPool>,
        xds_state: Arc<XdsState>,
        team: String,
        scopes: Vec<String>,
        org_id: Option<OrgId>,
    ) -> Self {
        Self { db_pool, xds_state: Some(xds_state), team, scopes, org_id, initialized: false }
    }

    /// Resolve team name to UUID for database queries.
    ///
    /// Resource tables store team UUIDs (not names), so any direct SQL query
    /// must use the resolved UUID. Internal API tools handle this themselves,
    /// but direct-DB tools and the reporting repository need the UUID passed in.
    ///
    /// Returns the team name unchanged if it's already a UUID or is empty.
    async fn resolve_team_uuid(&self) -> Result<String, McpError> {
        if self.team.is_empty() {
            return Ok(self.team.clone());
        }
        if uuid::Uuid::parse_str(&self.team).is_ok() {
            return Ok(self.team.clone());
        }
        let row: Option<(String,)> = sqlx::query_as("SELECT id FROM teams WHERE name = $1")
            .bind(&self.team)
            .fetch_optional(&*self.db_pool)
            .await
            .map_err(|e| {
                McpError::InternalError(format!("Failed to resolve team '{}': {}", self.team, e))
            })?;
        row.map(|r| r.0)
            .ok_or_else(|| McpError::InternalError(format!("Team '{}' not found", self.team)))
    }

    /// Handle an incoming JSON-RPC request
    pub async fn handle_request(&mut self, request: JsonRpcRequest) -> JsonRpcResponse {
        let method = request.method.clone();
        let id = request.id.clone();

        debug!(
            method = %method,
            id = ?id,
            "Handling MCP request"
        );

        // Note: For stateless HTTP transport, we don't enforce initialization checks.
        // Each HTTP request creates a new handler, so session state isn't preserved.
        // Authentication via bearer token provides the security boundary instead.
        let response = match request.method.as_str() {
            "initialize" => self.handle_initialize(request.id.clone(), request.params).await,
            "initialized" => self.handle_initialized(request.id.clone()).await,
            "ping" => self.handle_ping(request.id.clone()).await,
            "tools/list" => self.handle_tools_list(request.id.clone()).await,
            "tools/call" => self.handle_tools_call(request.id.clone(), request.params).await,
            "resources/list" => self.handle_resources_list(request.id.clone()).await,
            "resources/read" => {
                self.handle_resources_read(request.id.clone(), request.params).await
            }
            "prompts/list" => self.handle_prompts_list(request.id.clone()).await,
            "prompts/get" => self.handle_prompts_get(request.id.clone(), request.params).await,
            "logging/setLevel" => {
                self.handle_logging_set_level(request.id.clone(), request.params).await
            }
            "notifications/initialized" => {
                // Client acknowledgment - just return empty success
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: Some(serde_json::json!({})),
                    error: None,
                }
            }
            "notifications/cancelled" => {
                // Client cancelled a request - acknowledge it
                debug!("Received cancellation notification");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: Some(serde_json::json!({})),
                    error: None,
                }
            }
            _ => self.method_not_found(request.id.clone(), &request.method),
        };

        debug!(
            method = %method,
            id = ?id,
            has_error = response.error.is_some(),
            "Completed MCP request"
        );

        response
    }

    async fn handle_initialize(&mut self, id: Option<JsonRpcId>, params: Value) -> JsonRpcResponse {
        let params: InitializeParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to parse initialize params");
                return self.error_response(
                    id,
                    McpError::InvalidParams(format!("Failed to parse initialize params: {}", e)),
                );
            }
        };

        debug!(
            protocol_version = %params.protocol_version,
            client_name = %params.client_info.name,
            "Received initialize request"
        );

        // Validate protocol version against supported versions
        if let Err(e) = validate_protocol_version(&params.protocol_version) {
            error!(
                client_version = %params.protocol_version,
                supported_versions = ?SUPPORTED_VERSIONS,
                "Unsupported protocol version"
            );
            return self.error_response(id, e);
        }

        debug!(
            protocol_version = %params.protocol_version,
            "Protocol version validated"
        );

        self.initialized = true;

        let result = InitializeResult {
            protocol_version: params.protocol_version.clone(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: Some(false) }),
                resources: Some(ResourcesCapability {
                    subscribe: Some(false),
                    list_changed: Some(false),
                }),
                prompts: Some(PromptsCapability { list_changed: Some(false) }),
                logging: Some(LoggingCapability {}),
                completions: None,
                tasks: None,
                experimental: None,
                roots: None,       // Client-only capability
                sampling: None,    // Client-only capability
                elicitation: None, // Client-only capability
            },
            server_info: ServerInfo {
                name: "flowplane-mcp".to_string(),
                version: crate::VERSION.to_string(),
                title: Some("Flowplane MCP Server".to_string()),
                description: Some(
                    "Envoy control plane management via Model Context Protocol".to_string(),
                ),
                icons: None,
                website_url: None,
            },
            instructions: Some(SERVER_INSTRUCTIONS.to_string()),
        };

        match serde_json::to_value(result) {
            Ok(value) => {
                JsonRpcResponse { jsonrpc: "2.0".to_string(), id, result: Some(value), error: None }
            }
            Err(e) => self.error_response(id, McpError::SerializationError(e)),
        }
    }

    async fn handle_initialized(&mut self, id: Option<JsonRpcId>) -> JsonRpcResponse {
        debug!("Received initialized notification");

        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::json!({})),
            error: None,
        }
    }

    async fn handle_tools_list(&self, id: Option<JsonRpcId>) -> JsonRpcResponse {
        debug!("Listing available tools");

        // Use get_all_tools() as single source of truth (63 tools)
        let mut tools = tools::get_all_tools();

        // Enrich tool descriptions with risk level hints from the registry
        for tool in &mut tools {
            if let Some(auth) = get_tool_authorization(&tool.name) {
                if let Some(ref mut desc) = tool.description {
                    *desc = format!("[Risk: {}] {}", auth.risk_level, desc);
                }
            }
        }

        let result = ToolsListResult { tools, next_cursor: None };

        match serde_json::to_value(result) {
            Ok(value) => {
                JsonRpcResponse { jsonrpc: "2.0".to_string(), id, result: Some(value), error: None }
            }
            Err(e) => self.error_response(id, McpError::SerializationError(e)),
        }
    }

    async fn handle_tools_call(&self, id: Option<JsonRpcId>, params: Value) -> JsonRpcResponse {
        let params: ToolCallParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to parse tool call params");
                return self.error_response(
                    id,
                    McpError::InvalidParams(format!("Failed to parse tool call params: {}", e)),
                );
            }
        };

        debug!(tool_name = %params.name, "Executing tool call");

        // Check tool-level authorization
        if let Err(auth_error) = self.check_tool_authorization(&params.name) {
            warn!(
                tool_name = %params.name,
                team = %self.team,
                error = %auth_error,
                "Tool authorization failed"
            );
            return self.error_response(id, auth_error);
        }

        let args = params.arguments.unwrap_or(serde_json::json!({}));

        // Resolve team name → UUID once for all tool dispatches.
        // Resource tables store UUIDs, not names.
        let team = match self.resolve_team_uuid().await {
            Ok(t) => t,
            Err(e) => {
                warn!(team = %self.team, error = %e, "Failed to resolve team UUID");
                return self.error_response(id, e);
            }
        };

        let result = match params.name.as_str() {
            // Read operations that only need db_pool (direct table query for efficiency)
            "cp_list_routes" => {
                tools::execute_list_routes(&self.db_pool, &team, self.org_id.as_ref(), args).await
            }
            // Query-first tools (direct db_pool access for token efficiency)
            "cp_query_port" => {
                tools::execute_query_port(&self.db_pool, &team, self.org_id.as_ref(), args).await
            }
            "cp_query_path" => {
                tools::execute_query_path(&self.db_pool, &team, self.org_id.as_ref(), args).await
            }
            // Ops tools that only need db_pool (diagnostic/reporting queries)
            "ops_trace_request" => {
                tools::execute_ops_trace_request(&self.db_pool, &team, self.org_id.as_ref(), args)
                    .await
            }
            "ops_topology" => {
                tools::execute_ops_topology(&self.db_pool, &team, self.org_id.as_ref(), args).await
            }
            "ops_config_validate" => {
                tools::execute_ops_config_validate(&self.db_pool, &team, self.org_id.as_ref(), args)
                    .await
            }
            "ops_audit_query" => {
                tools::execute_ops_audit_query(&self.db_pool, &team, self.org_id.as_ref(), args)
                    .await
            }
            "ops_xds_delivery_status" => {
                tools::execute_ops_xds_delivery_status(
                    &self.db_pool,
                    &team,
                    self.org_id.as_ref(),
                    args,
                )
                .await
            }
            "ops_nack_history" => {
                tools::execute_ops_nack_history(&self.db_pool, &team, self.org_id.as_ref(), args)
                    .await
            }
            // Dev agent tools (db_pool only)
            "dev_preflight_check" => {
                tools::execute_dev_preflight_check(&self.db_pool, &team, self.org_id.as_ref(), args)
                    .await
            }
            "cp_query_service" => {
                tools::execute_query_service(&self.db_pool, &team, self.org_id.as_ref(), args).await
            }

            // Operations that require xds_state (internal API layer)
            "cp_list_clusters"
            | "cp_get_cluster"
            | "cp_get_cluster_health"
            | "cp_list_listeners"
            | "cp_get_listener"
            | "cp_get_listener_status"
            | "cp_list_filters"
            | "cp_get_filter"
            | "cp_list_virtual_hosts"
            | "cp_get_virtual_host"
            | "cp_list_aggregated_schemas"
            | "cp_get_aggregated_schema"
            | "cp_list_learning_sessions"
            | "cp_get_learning_session"
            | "cp_create_learning_session"
            | "cp_activate_learning_session"
            | "cp_delete_learning_session"
            | "ops_learning_session_health"
            | "cp_create_cluster"
            | "cp_update_cluster"
            | "cp_delete_cluster"
            | "cp_create_listener"
            | "cp_update_listener"
            | "cp_delete_listener"
            | "cp_list_route_configs"
            | "cp_get_route_config"
            | "cp_create_route_config"
            | "cp_update_route_config"
            | "cp_delete_route_config"
            | "cp_get_route"
            | "cp_create_route"
            | "cp_update_route"
            | "cp_delete_route"
            | "cp_create_virtual_host"
            | "cp_update_virtual_host"
            | "cp_delete_virtual_host"
            | "cp_create_filter"
            | "cp_update_filter"
            | "cp_delete_filter"
            | "cp_attach_filter"
            | "cp_detach_filter"
            | "cp_list_filter_attachments"
            | "cp_list_openapi_imports"
            | "cp_get_openapi_import"
            | "cp_list_dataplanes"
            | "cp_get_dataplane"
            | "cp_create_dataplane"
            | "cp_update_dataplane"
            | "cp_delete_dataplane"
            | "cp_list_filter_types"
            | "cp_get_filter_type"
            | "devops_get_deployment_status"
            | "cp_export_schema_openapi" => {
                let xds_state = match &self.xds_state {
                    Some(state) => state,
                    None => {
                        return self.error_response(
                            id,
                            McpError::InternalError(
                                "Operation not available: xds_state not configured".to_string(),
                            ),
                        );
                    }
                };
                match params.name.as_str() {
                    // Cluster operations (use internal API layer)
                    "cp_list_clusters" => {
                        tools::execute_list_clusters(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_get_cluster" => {
                        tools::execute_get_cluster(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_get_cluster_health" => {
                        tools::execute_get_cluster_health(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_create_cluster" => {
                        tools::execute_create_cluster(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_update_cluster" => {
                        tools::execute_update_cluster(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_delete_cluster" => {
                        tools::execute_delete_cluster(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    // Listener operations (use internal API layer)
                    "cp_list_listeners" => {
                        tools::execute_list_listeners(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_get_listener" => {
                        tools::execute_get_listener(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_get_listener_status" => {
                        tools::execute_get_listener_status(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_create_listener" => {
                        tools::execute_create_listener(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_update_listener" => {
                        tools::execute_update_listener(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_delete_listener" => {
                        tools::execute_delete_listener(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    // Route config operations (use internal API layer)
                    "cp_list_route_configs" => {
                        tools::execute_list_route_configs(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_get_route_config" => {
                        tools::execute_get_route_config(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_create_route_config" => {
                        tools::execute_create_route_config(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_update_route_config" => {
                        tools::execute_update_route_config(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_delete_route_config" => {
                        tools::execute_delete_route_config(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // Individual route CRUD (use internal API layer)
                    "cp_get_route" => {
                        tools::execute_get_route(xds_state, &team, self.org_id.as_ref(), args).await
                    }
                    "cp_create_route" => {
                        tools::execute_create_route(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_update_route" => {
                        tools::execute_update_route(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_delete_route" => {
                        tools::execute_delete_route(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    // Filter operations (use internal API layer)
                    "cp_list_filters" => {
                        tools::execute_list_filters(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_get_filter" => {
                        tools::execute_get_filter(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_create_filter" => {
                        tools::execute_create_filter(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_update_filter" => {
                        tools::execute_update_filter(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_delete_filter" => {
                        tools::execute_delete_filter(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    // Filter attachment operations
                    "cp_attach_filter" => {
                        tools::execute_attach_filter(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_detach_filter" => {
                        tools::execute_detach_filter(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_list_filter_attachments" => {
                        tools::execute_list_filter_attachments(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // Virtual host operations (use internal API layer)
                    "cp_list_virtual_hosts" => {
                        tools::execute_list_virtual_hosts(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_get_virtual_host" => {
                        tools::execute_get_virtual_host(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_create_virtual_host" => {
                        tools::execute_create_virtual_host(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_update_virtual_host" => {
                        tools::execute_update_virtual_host(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_delete_virtual_host" => {
                        tools::execute_delete_virtual_host(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // Aggregated schema operations (use internal API layer)
                    "cp_list_aggregated_schemas" => {
                        tools::execute_list_aggregated_schemas(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_get_aggregated_schema" => {
                        tools::execute_get_aggregated_schema(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // Learning session operations (use internal API layer)
                    "cp_list_learning_sessions" => {
                        tools::execute_list_learning_sessions(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_get_learning_session" => {
                        tools::execute_get_learning_session(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_create_learning_session" => {
                        tools::execute_create_learning_session(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_activate_learning_session" => {
                        tools::execute_activate_learning_session(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_delete_learning_session" => {
                        tools::execute_delete_learning_session(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "ops_learning_session_health" => {
                        tools::execute_ops_learning_session_health(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // OpenAPI import operations
                    "cp_list_openapi_imports" => {
                        tools::execute_list_openapi_imports(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_get_openapi_import" => {
                        tools::execute_get_openapi_import(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // Dataplane operations
                    "cp_list_dataplanes" => {
                        tools::execute_list_dataplanes(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_get_dataplane" => {
                        tools::execute_get_dataplane(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    "cp_create_dataplane" => {
                        tools::execute_create_dataplane(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_update_dataplane" => {
                        tools::execute_update_dataplane(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_delete_dataplane" => {
                        tools::execute_delete_dataplane(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // Filter type operations
                    "cp_list_filter_types" => {
                        tools::execute_list_filter_types(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    "cp_get_filter_type" => {
                        tools::execute_get_filter_type(xds_state, &team, self.org_id.as_ref(), args)
                            .await
                    }
                    // Schema export operations
                    "cp_export_schema_openapi" => {
                        tools::execute_export_schema_openapi(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    // DevOps agent workflow operations
                    "devops_get_deployment_status" => {
                        tools::execute_devops_get_deployment_status(
                            xds_state,
                            &team,
                            self.org_id.as_ref(),
                            args,
                        )
                        .await
                    }
                    _ => unreachable!(),
                }
            }
            _ => {
                return self.error_response(id, McpError::ToolNotFound(params.name));
            }
        };

        match result {
            Ok(tool_result) => match serde_json::to_value(tool_result) {
                Ok(value) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(value),
                    error: None,
                },
                Err(e) => self.error_response(id, McpError::SerializationError(e)),
            },
            Err(e) => self.error_response(id, e),
        }
    }

    async fn handle_resources_list(&self, id: Option<JsonRpcId>) -> JsonRpcResponse {
        debug!(team = %self.team, "Listing available resources");

        let team = match self.resolve_team_uuid().await {
            Ok(t) => t,
            Err(e) => return self.error_response(id, e),
        };

        match resources::list_resources(&self.db_pool, &team).await {
            Ok(result) => match serde_json::to_value(result) {
                Ok(value) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(value),
                    error: None,
                },
                Err(e) => self.error_response(id, McpError::SerializationError(e)),
            },
            Err(e) => {
                error!(error = %e, "Failed to list resources");
                self.error_response(id, e)
            }
        }
    }

    async fn handle_resources_read(&self, id: Option<JsonRpcId>, params: Value) -> JsonRpcResponse {
        let params: ResourceReadParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to parse resource read params");
                return self.error_response(
                    id,
                    McpError::InvalidParams(format!("Failed to parse resource read params: {}", e)),
                );
            }
        };

        debug!(uri = %params.uri, "Reading resource");

        match resources::read_resource(&self.db_pool, &params.uri).await {
            Ok(contents) => {
                let result = ResourceReadResult { contents: vec![contents] };
                match serde_json::to_value(result) {
                    Ok(value) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: Some(value),
                        error: None,
                    },
                    Err(e) => self.error_response(id, McpError::SerializationError(e)),
                }
            }
            Err(e) => {
                error!(error = %e, uri = %params.uri, "Failed to read resource");
                self.error_response(id, e)
            }
        }
    }

    async fn handle_ping(&self, id: Option<JsonRpcId>) -> JsonRpcResponse {
        debug!("Received ping request");

        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::json!({})),
            error: None,
        }
    }

    async fn handle_prompts_list(&self, id: Option<JsonRpcId>) -> JsonRpcResponse {
        debug!("Listing available prompts");

        let prompts = crate::mcp::prompts::get_all_prompts();
        let result = PromptsListResult { prompts, next_cursor: None };

        match serde_json::to_value(result) {
            Ok(value) => {
                JsonRpcResponse { jsonrpc: "2.0".to_string(), id, result: Some(value), error: None }
            }
            Err(e) => self.error_response(id, McpError::SerializationError(e)),
        }
    }

    async fn handle_prompts_get(&self, id: Option<JsonRpcId>, params: Value) -> JsonRpcResponse {
        let params: PromptGetParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to parse prompt get params");
                return self.error_response(
                    id,
                    McpError::InvalidParams(format!("Failed to parse prompt get params: {}", e)),
                );
            }
        };

        debug!(prompt_name = %params.name, "Getting prompt");

        match crate::mcp::prompts::get_prompt(&params.name, params.arguments) {
            Ok(result) => match serde_json::to_value(result) {
                Ok(value) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(value),
                    error: None,
                },
                Err(e) => self.error_response(id, McpError::SerializationError(e)),
            },
            Err(e) => self.error_response(id, e),
        }
    }

    async fn handle_logging_set_level(
        &self,
        id: Option<JsonRpcId>,
        params: Value,
    ) -> JsonRpcResponse {
        let params: SetLogLevelParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to parse logging set level params");
                return self.error_response(
                    id,
                    McpError::InvalidParams(format!("Failed to parse logging params: {}", e)),
                );
            }
        };

        debug!(level = ?params.level, "Setting log level");

        // Note: Actual log level filtering is done at the connection manager level
        // This handler acknowledges the request; SSE integration applies the level

        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::json!({})),
            error: None,
        }
    }

    /// Check if the current token has authorization to execute the given tool
    ///
    /// Uses the tool registry to lookup required scopes and checks against
    /// the token's scopes. Implements hierarchical scope matching:
    /// - `admin:all` grants all access
    /// - `cp:read` grants all CP read operations
    /// - `cp:write` grants all CP write/delete operations
    /// - Specific scopes like `clusters:read` for granular control
    fn check_tool_authorization(&self, tool_name: &str) -> Result<(), McpError> {
        // Lookup tool authorization requirements
        let auth = get_tool_authorization(tool_name).ok_or_else(|| {
            McpError::ToolNotFound(format!(
                "Tool '{}' is not registered in the authorization registry",
                tool_name
            ))
        })?;

        // Check if any scope grants the required authorization
        if check_scope_grants_authorization(self.scopes.iter().map(|s| s.as_str()), auth) {
            debug!(
                tool_name = %tool_name,
                resource = %auth.resource,
                action = %auth.action,
                "Tool authorization granted"
            );
            return Ok(());
        }

        // Build helpful error message
        let required_scope = format!("{}:{}", auth.resource, auth.action);
        let fallback_info =
            if ["clusters", "listeners", "routes", "filters"].contains(&auth.resource) {
                format!(" Alternatively, 'cp:{}' grants access to all core resources.", auth.action)
            } else {
                String::new()
            };

        Err(McpError::Forbidden(format!(
            "Access denied: Tool '{}' requires scope '{}' or 'admin:all'.{} Your token has scopes: {:?}",
            tool_name, required_scope, fallback_info, self.scopes
        )))
    }

    fn method_not_found(&self, id: Option<JsonRpcId>, method: &str) -> JsonRpcResponse {
        error!(method = %method, "Method not found");

        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code: error_codes::METHOD_NOT_FOUND,
                message: format!("Method not found: {}", method),
                data: None,
            }),
        }
    }

    fn error_response(&self, id: Option<JsonRpcId>, error: McpError) -> JsonRpcResponse {
        error!(error = %error, "MCP error");

        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error.to_json_rpc_error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    async fn create_test_handler() -> (TestDatabase, McpHandler) {
        let test_db = TestDatabase::new("mcp_handler").await;
        let pool = test_db.pool.clone();
        // Use admin:all scope for tests to bypass authorization
        let handler =
            McpHandler::new(Arc::new(pool), "test-team".to_string(), vec!["admin:all".to_string()]);
        (test_db, handler)
    }

    #[tokio::test]
    async fn test_initialize() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert!(handler.initialized);
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::String("test".to_string())),
            method: "unknown/method".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;

        assert!(response.result.is_none());
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_tools_list_without_initialize() {
        // For stateless HTTP transport, tools/list should work without initialize
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "tools/list".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;

        // Should succeed without initialization for HTTP transport
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_tools_list() {
        let (_db, mut handler) = create_test_handler().await;

        // Initialize first
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };
        handler.handle_request(init_request).await;

        // Now list tools
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(2)),
            method: "tools/list".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[test]
    fn test_version_validation_exact_match() {
        // Only 2025-11-25 is accepted
        let result = validate_protocol_version("2025-11-25");
        assert!(result.is_ok());
    }

    #[test]
    fn test_version_validation_rejects_older_version() {
        // Truly unsupported older versions are rejected
        let result = validate_protocol_version("2024-11-05");
        assert!(result.is_err());

        match result.unwrap_err() {
            McpError::UnsupportedProtocolVersion { client, supported } => {
                assert_eq!(client, "2024-11-05");
                assert!(supported.contains(&"2025-11-25".to_string()));
                assert!(supported.contains(&"2025-03-26".to_string()));
            }
            _ => panic!("Expected UnsupportedProtocolVersion error"),
        }
    }

    #[test]
    fn test_version_validation_rejects_newer_version() {
        // Newer unknown versions are rejected
        let result = validate_protocol_version("2026-01-01");
        assert!(result.is_err());

        match result.unwrap_err() {
            McpError::UnsupportedProtocolVersion { client, supported } => {
                assert_eq!(client, "2026-01-01");
                assert!(supported.contains(&"2025-11-25".to_string()));
                assert!(supported.contains(&"2025-03-26".to_string()));
            }
            _ => panic!("Expected UnsupportedProtocolVersion error"),
        }
    }

    #[test]
    fn test_version_validation_rejects_ancient_version() {
        // Very old versions are rejected
        let result = validate_protocol_version("2024-11-05");
        assert!(result.is_err());

        match result.unwrap_err() {
            McpError::UnsupportedProtocolVersion { client, supported } => {
                assert_eq!(client, "2024-11-05");
                assert_eq!(supported.len(), 2);
                assert!(supported.contains(&"2025-11-25".to_string()));
                assert!(supported.contains(&"2025-03-26".to_string()));
            }
            _ => panic!("Expected UnsupportedProtocolVersion error"),
        }
    }

    #[test]
    fn test_unsupported_version_error_message() {
        let result = validate_protocol_version("2023-12-31");
        assert!(result.is_err());

        let error = result.unwrap_err();
        let message = error.to_string();

        assert!(message.contains("2023-12-31"));
        assert!(message.contains("2025-11-25"));
        assert!(message.contains("2025-03-26"));
    }

    #[test]
    fn test_unsupported_version_json_rpc_error() {
        let result = validate_protocol_version("2020-01-01");
        assert!(result.is_err());

        let error = result.unwrap_err();
        let json_rpc_error = error.to_json_rpc_error();

        assert_eq!(json_rpc_error.code, error_codes::INVALID_REQUEST);
        assert!(json_rpc_error.message.contains("Unsupported protocol version"));

        // Verify the data field contains supportedVersions
        assert!(json_rpc_error.data.is_some());
        let data = json_rpc_error.data.unwrap();
        assert!(data.get("supportedVersions").is_some());

        let supported = data["supportedVersions"].as_array().unwrap();
        assert_eq!(supported.len(), 2);
        assert!(supported.iter().any(|v| v == "2025-11-25"));
        assert!(supported.iter().any(|v| v == "2025-03-26"));
    }

    #[tokio::test]
    async fn test_initialize_with_unsupported_version() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2023-01-01",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, error_codes::INVALID_REQUEST);
        assert!(error.message.contains("Unsupported protocol version"));
        assert!(error.data.is_some());

        // Handler should not be initialized after failed negotiation
        assert!(!handler.initialized);
    }

    #[tokio::test]
    async fn test_initialize_with_2025_03_26_succeeds() {
        let (_db, mut handler) = create_test_handler().await;

        // 2025-03-26 is now a supported version
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert!(handler.initialized);
    }

    #[tokio::test]
    async fn test_initialize_with_newer_version_rejected() {
        let (_db, mut handler) = create_test_handler().await;

        // Future unknown versions are rejected
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2026-12-31",
                "capabilities": {},
                "clientInfo": {
                    "name": "future-client",
                    "version": "2.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, error_codes::INVALID_REQUEST);
        assert!(error.message.contains("Unsupported protocol version"));
        assert!(!handler.initialized);
    }

    #[tokio::test]
    async fn test_initialize_with_2025_03_26_returns_matching_version() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-03-26");
    }

    #[tokio::test]
    async fn test_initialize_with_2025_11_25_returns_matching_version() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-11-25");
    }

    #[tokio::test]
    async fn test_initialize_returns_instructions() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }),
        };

        let response = handler.handle_request(request).await;
        assert!(response.error.is_none());

        let result = response.result.unwrap();
        let instructions = result.get("instructions").and_then(|v| v.as_str());
        assert!(instructions.is_some(), "instructions must be set in InitializeResult");

        let text = instructions.unwrap();
        assert!(text.contains("Flowplane API Gateway Control Plane"));
        assert!(text.contains("Resource Creation Order"));
        assert!(text.contains("Diagnostic Workflow"));
        assert!(text.contains("ops_topology"));
    }

    #[tokio::test]
    async fn test_tools_list_synced_with_get_all_tools() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "tools/list".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;
        assert!(response.error.is_none());

        let result = response.result.unwrap();
        let tools_array = result.get("tools").and_then(|v| v.as_array()).unwrap();

        // handle_tools_list must return exactly get_all_tools() count
        let all_tools = tools::get_all_tools();
        assert_eq!(
            tools_array.len(),
            all_tools.len(),
            "handle_tools_list() ({}) must be in sync with get_all_tools() ({})",
            tools_array.len(),
            all_tools.len()
        );
    }

    #[tokio::test]
    async fn test_tools_list_has_risk_level_hints() {
        let (_db, mut handler) = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "tools/list".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;
        assert!(response.error.is_none());

        let result = response.result.unwrap();
        let tools_array = result.get("tools").and_then(|v| v.as_array()).unwrap();

        // Check that tool descriptions include risk level hints
        for tool in tools_array {
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap();
            if let Some(desc) = tool.get("description").and_then(|v| v.as_str()) {
                assert!(
                    desc.starts_with("[Risk: "),
                    "Tool '{}' description should start with [Risk: ...] hint, got: {}",
                    name,
                    &desc[..std::cmp::min(50, desc.len())]
                );
            }
        }
    }
}
