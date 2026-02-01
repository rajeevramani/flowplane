//! MCP Request Handler
//!
//! Routes incoming JSON-RPC requests to the appropriate method handlers.

use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{debug, error, warn};

use crate::mcp::error::McpError;
use crate::mcp::logging::SetLogLevelParams;
use crate::mcp::protocol::*;
use crate::mcp::resources;
use crate::mcp::tool_registry::{check_scope_grants_authorization, get_tool_authorization};
use crate::mcp::tools;
use crate::xds::XdsState;

/// Negotiate MCP protocol version
///
/// Finds the highest version we support that is <= client's version.
/// This ensures backward compatibility while preventing clients from
/// requesting features we don't support.
fn negotiate_version(client_version: &str) -> Result<String, McpError> {
    // Find highest version we support that's <= client's version
    let negotiated = SUPPORTED_VERSIONS.iter().rev().find(|&&v| v <= client_version).copied();

    match negotiated {
        Some(v) => Ok(v.to_string()),
        None => Err(McpError::UnsupportedProtocolVersion {
            client: client_version.to_string(),
            supported: SUPPORTED_VERSIONS.iter().map(|s| s.to_string()).collect(),
        }),
    }
}

pub struct McpHandler {
    db_pool: Arc<SqlitePool>,
    xds_state: Option<Arc<XdsState>>,
    team: String,
    /// Scopes from the authenticated token for tool-level authorization
    scopes: Vec<String>,
    initialized: bool,
}

impl McpHandler {
    /// Create a new MCP handler with read-only capabilities
    ///
    /// # Arguments
    /// * `db_pool` - Database connection pool
    /// * `team` - Team context for multi-tenancy
    /// * `scopes` - Authorization scopes from the authenticated token
    pub fn new(db_pool: Arc<SqlitePool>, team: String, scopes: Vec<String>) -> Self {
        Self { db_pool, xds_state: None, team, scopes, initialized: false }
    }

    /// Create a new MCP handler with full read/write capabilities
    ///
    /// # Arguments
    /// * `db_pool` - Database connection pool
    /// * `xds_state` - XDS state for control plane operations
    /// * `team` - Team context for multi-tenancy
    /// * `scopes` - Authorization scopes from the authenticated token
    pub fn with_xds_state(
        db_pool: Arc<SqlitePool>,
        xds_state: Arc<XdsState>,
        team: String,
        scopes: Vec<String>,
    ) -> Self {
        Self { db_pool, xds_state: Some(xds_state), team, scopes, initialized: false }
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

        // Negotiate protocol version with strict validation
        let client_version = if params.protocol_version.is_empty() {
            "2024-11-05" // Default to oldest supported version for backwards compatibility
        } else {
            &params.protocol_version
        };

        let negotiated_version = match negotiate_version(client_version) {
            Ok(v) => v,
            Err(e) => {
                error!(
                    client_version = %client_version,
                    error = %e,
                    "Protocol version negotiation failed"
                );
                return self.error_response(id, e);
            }
        };

        debug!(
            client_version = %client_version,
            negotiated_version = %negotiated_version,
            "Protocol version negotiated"
        );

        self.initialized = true;

        let result = InitializeResult {
            protocol_version: negotiated_version,
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: Some(false) }),
                resources: Some(ResourcesCapability {
                    subscribe: Some(false),
                    list_changed: Some(false),
                }),
                prompts: Some(PromptsCapability { list_changed: Some(false) }),
                logging: Some(LoggingCapability {}),
                experimental: None,
            },
            server_info: ServerInfo {
                name: "flowplane-mcp".to_string(),
                version: crate::VERSION.to_string(),
            },
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

        let tools = vec![
            // Read operations
            tools::cp_list_clusters_tool(),
            tools::cp_get_cluster_tool(),
            tools::cp_list_listeners_tool(),
            tools::cp_get_listener_tool(),
            tools::cp_list_routes_tool(),
            tools::cp_get_route_tool(),
            tools::cp_list_filters_tool(),
            tools::cp_get_filter_tool(),
            tools::cp_list_virtual_hosts_tool(),
            tools::cp_get_virtual_host_tool(),
            // Write operations (requires xds_state)
            // Cluster CRUD
            tools::cp_create_cluster_tool(),
            tools::cp_update_cluster_tool(),
            tools::cp_delete_cluster_tool(),
            // Listener CRUD
            tools::cp_create_listener_tool(),
            tools::cp_update_listener_tool(),
            tools::cp_delete_listener_tool(),
            // Route config CRUD
            tools::cp_create_route_config_tool(),
            tools::cp_update_route_config_tool(),
            tools::cp_delete_route_config_tool(),
            // Individual route CRUD
            tools::cp_create_route_tool(),
            tools::cp_update_route_tool(),
            tools::cp_delete_route_tool(),
            // Virtual host CRUD
            tools::cp_create_virtual_host_tool(),
            tools::cp_update_virtual_host_tool(),
            tools::cp_delete_virtual_host_tool(),
            // Filter CRUD
            tools::cp_create_filter_tool(),
            tools::cp_update_filter_tool(),
            tools::cp_delete_filter_tool(),
            // Filter attachment tools
            tools::cp_attach_filter_tool(),
            tools::cp_detach_filter_tool(),
            tools::cp_list_filter_attachments_tool(),
            // Learning session tools
            tools::cp_list_learning_sessions_tool(),
            tools::cp_get_learning_session_tool(),
            tools::cp_create_learning_session_tool(),
            tools::cp_delete_learning_session_tool(),
            // OpenAPI import tools
            tools::cp_list_openapi_imports_tool(),
            tools::cp_get_openapi_import_tool(),
            // Dataplane CRUD tools
            tools::cp_list_dataplanes_tool(),
            tools::cp_get_dataplane_tool(),
            tools::cp_create_dataplane_tool(),
            tools::cp_update_dataplane_tool(),
            tools::cp_delete_dataplane_tool(),
            // Filter type tools
            tools::cp_list_filter_types_tool(),
            tools::cp_get_filter_type_tool(),
            // DevOps agent workflow tools
            tools::devops_deploy_api_tool(),
            tools::devops_configure_rate_limiting_tool(),
            tools::devops_enable_jwt_auth_tool(),
            tools::devops_configure_cors_tool(),
            tools::devops_create_canary_deployment_tool(),
            tools::devops_get_deployment_status_tool(),
        ];

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

        let result = match params.name.as_str() {
            // Read operations that only need db_pool (route table query)
            "cp_list_routes" => tools::execute_list_routes(&self.db_pool, &self.team, args).await,
            // Operations that require xds_state (internal API layer)
            "cp_list_clusters"
            | "cp_get_cluster"
            | "cp_list_listeners"
            | "cp_get_listener"
            | "cp_list_filters"
            | "cp_get_filter"
            | "cp_list_virtual_hosts"
            | "cp_get_virtual_host"
            | "cp_list_aggregated_schemas"
            | "cp_get_aggregated_schema"
            | "cp_list_learning_sessions"
            | "cp_get_learning_session"
            | "cp_create_learning_session"
            | "cp_delete_learning_session"
            | "cp_create_cluster"
            | "cp_update_cluster"
            | "cp_delete_cluster"
            | "cp_create_listener"
            | "cp_update_listener"
            | "cp_delete_listener"
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
            | "devops_deploy_api"
            | "devops_configure_rate_limiting"
            | "devops_enable_jwt_auth"
            | "devops_configure_cors"
            | "devops_create_canary_deployment"
            | "devops_get_deployment_status" => {
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
                        tools::execute_list_clusters(xds_state, &self.team, args).await
                    }
                    "cp_get_cluster" => {
                        tools::execute_get_cluster(xds_state, &self.team, args).await
                    }
                    "cp_create_cluster" => {
                        tools::execute_create_cluster(xds_state, &self.team, args).await
                    }
                    "cp_update_cluster" => {
                        tools::execute_update_cluster(xds_state, &self.team, args).await
                    }
                    "cp_delete_cluster" => {
                        tools::execute_delete_cluster(xds_state, &self.team, args).await
                    }
                    // Listener operations (use internal API layer)
                    "cp_list_listeners" => {
                        tools::execute_list_listeners(xds_state, &self.team, args).await
                    }
                    "cp_get_listener" => {
                        tools::execute_get_listener(xds_state, &self.team, args).await
                    }
                    "cp_create_listener" => {
                        tools::execute_create_listener(xds_state, &self.team, args).await
                    }
                    "cp_update_listener" => {
                        tools::execute_update_listener(xds_state, &self.team, args).await
                    }
                    "cp_delete_listener" => {
                        tools::execute_delete_listener(xds_state, &self.team, args).await
                    }
                    // Route config CRUD (use internal API layer)
                    "cp_create_route_config" => {
                        tools::execute_create_route_config(xds_state, &self.team, args).await
                    }
                    "cp_update_route_config" => {
                        tools::execute_update_route_config(xds_state, &self.team, args).await
                    }
                    "cp_delete_route_config" => {
                        tools::execute_delete_route_config(xds_state, &self.team, args).await
                    }
                    // Individual route CRUD (use internal API layer)
                    "cp_get_route" => tools::execute_get_route(xds_state, &self.team, args).await,
                    "cp_create_route" => {
                        tools::execute_create_route(xds_state, &self.team, args).await
                    }
                    "cp_update_route" => {
                        tools::execute_update_route(xds_state, &self.team, args).await
                    }
                    "cp_delete_route" => {
                        tools::execute_delete_route(xds_state, &self.team, args).await
                    }
                    // Filter operations (use internal API layer)
                    "cp_list_filters" => {
                        tools::execute_list_filters(xds_state, &self.team, args).await
                    }
                    "cp_get_filter" => tools::execute_get_filter(xds_state, &self.team, args).await,
                    "cp_create_filter" => {
                        tools::execute_create_filter(xds_state, &self.team, args).await
                    }
                    "cp_update_filter" => {
                        tools::execute_update_filter(xds_state, &self.team, args).await
                    }
                    "cp_delete_filter" => {
                        tools::execute_delete_filter(xds_state, &self.team, args).await
                    }
                    // Filter attachment operations
                    "cp_attach_filter" => {
                        tools::execute_attach_filter(xds_state, &self.team, args).await
                    }
                    "cp_detach_filter" => {
                        tools::execute_detach_filter(xds_state, &self.team, args).await
                    }
                    "cp_list_filter_attachments" => {
                        tools::execute_list_filter_attachments(xds_state, &self.team, args).await
                    }
                    // Virtual host operations (use internal API layer)
                    "cp_list_virtual_hosts" => {
                        tools::execute_list_virtual_hosts(xds_state, &self.team, args).await
                    }
                    "cp_get_virtual_host" => {
                        tools::execute_get_virtual_host(xds_state, &self.team, args).await
                    }
                    "cp_create_virtual_host" => {
                        tools::execute_create_virtual_host(xds_state, &self.team, args).await
                    }
                    "cp_update_virtual_host" => {
                        tools::execute_update_virtual_host(xds_state, &self.team, args).await
                    }
                    "cp_delete_virtual_host" => {
                        tools::execute_delete_virtual_host(xds_state, &self.team, args).await
                    }
                    // Aggregated schema operations (use internal API layer)
                    "cp_list_aggregated_schemas" => {
                        tools::execute_list_aggregated_schemas(xds_state, &self.team, args).await
                    }
                    "cp_get_aggregated_schema" => {
                        tools::execute_get_aggregated_schema(xds_state, &self.team, args).await
                    }
                    // Learning session operations (use internal API layer)
                    "cp_list_learning_sessions" => {
                        tools::execute_list_learning_sessions(xds_state, &self.team, args).await
                    }
                    "cp_get_learning_session" => {
                        tools::execute_get_learning_session(xds_state, &self.team, args).await
                    }
                    "cp_create_learning_session" => {
                        tools::execute_create_learning_session(xds_state, &self.team, args).await
                    }
                    "cp_delete_learning_session" => {
                        tools::execute_delete_learning_session(xds_state, &self.team, args).await
                    }
                    // OpenAPI import operations
                    "cp_list_openapi_imports" => {
                        tools::execute_list_openapi_imports(xds_state, &self.team, args).await
                    }
                    "cp_get_openapi_import" => {
                        tools::execute_get_openapi_import(xds_state, &self.team, args).await
                    }
                    // Dataplane operations
                    "cp_list_dataplanes" => {
                        tools::execute_list_dataplanes(xds_state, &self.team, args).await
                    }
                    "cp_get_dataplane" => {
                        tools::execute_get_dataplane(xds_state, &self.team, args).await
                    }
                    "cp_create_dataplane" => {
                        tools::execute_create_dataplane(xds_state, &self.team, args).await
                    }
                    "cp_update_dataplane" => {
                        tools::execute_update_dataplane(xds_state, &self.team, args).await
                    }
                    "cp_delete_dataplane" => {
                        tools::execute_delete_dataplane(xds_state, &self.team, args).await
                    }
                    // Filter type operations
                    "cp_list_filter_types" => {
                        tools::execute_list_filter_types(xds_state, &self.team, args).await
                    }
                    "cp_get_filter_type" => {
                        tools::execute_get_filter_type(xds_state, &self.team, args).await
                    }
                    // DevOps agent workflow operations
                    "devops_deploy_api" => {
                        tools::execute_devops_deploy_api(xds_state, &self.team, args).await
                    }
                    "devops_configure_rate_limiting" => {
                        tools::execute_devops_configure_rate_limiting(xds_state, &self.team, args)
                            .await
                    }
                    "devops_enable_jwt_auth" => {
                        tools::execute_devops_enable_jwt_auth(xds_state, &self.team, args).await
                    }
                    "devops_configure_cors" => {
                        tools::execute_devops_configure_cors(xds_state, &self.team, args).await
                    }
                    "devops_create_canary_deployment" => {
                        tools::execute_devops_create_canary_deployment(xds_state, &self.team, args)
                            .await
                    }
                    "devops_get_deployment_status" => {
                        tools::execute_devops_get_deployment_status(xds_state, &self.team, args)
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

        match resources::list_resources(&self.db_pool, &self.team).await {
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
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;

    async fn create_test_handler() -> McpHandler {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 5,
            min_connections: 1,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 0,
            auto_migrate: false,
        };
        let pool = create_pool(&config).await.expect("Failed to create pool");
        // Use admin:all scope for tests to bypass authorization
        McpHandler::new(Arc::new(pool), "test-team".to_string(), vec!["admin:all".to_string()])
    }

    #[tokio::test]
    async fn test_initialize() {
        let mut handler = create_test_handler().await;

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
        let mut handler = create_test_handler().await;

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
        let mut handler = create_test_handler().await;

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
        let mut handler = create_test_handler().await;

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
    fn test_version_negotiation_exact_match() {
        // Test exact match with a supported version
        let result = negotiate_version("2025-11-25");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2025-11-25");
    }

    #[test]
    fn test_version_negotiation_newer_client() {
        // Client has newer version than we support - should get our newest
        let result = negotiate_version("2026-01-01");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2025-11-25");
    }

    #[test]
    fn test_version_negotiation_older_client() {
        // Client has older supported version - should get their version
        let result = negotiate_version("2025-03-26");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2025-03-26");
    }

    #[test]
    fn test_version_negotiation_oldest_supported() {
        // Client requests oldest supported version
        let result = negotiate_version("2024-11-05");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2024-11-05");
    }

    #[test]
    fn test_version_negotiation_failure() {
        // Client has version older than any we support
        let result = negotiate_version("2024-01-01");
        assert!(result.is_err());

        match result.unwrap_err() {
            McpError::UnsupportedProtocolVersion { client, supported } => {
                assert_eq!(client, "2024-01-01");
                assert_eq!(supported.len(), 4);
                assert!(supported.contains(&"2024-11-05".to_string()));
                assert!(supported.contains(&"2025-03-26".to_string()));
                assert!(supported.contains(&"2025-06-18".to_string()));
                assert!(supported.contains(&"2025-11-25".to_string()));
            }
            _ => panic!("Expected UnsupportedProtocolVersion error"),
        }
    }

    #[test]
    fn test_unsupported_version_error_message() {
        let result = negotiate_version("2023-12-31");
        assert!(result.is_err());

        let error = result.unwrap_err();
        let message = error.to_string();

        assert!(message.contains("2023-12-31"));
        assert!(message.contains("2024-11-05"));
        assert!(message.contains("2025-11-25"));
    }

    #[test]
    fn test_unsupported_version_json_rpc_error() {
        let result = negotiate_version("2020-01-01");
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
        assert_eq!(supported.len(), 4);
    }

    #[tokio::test]
    async fn test_initialize_with_unsupported_version() {
        let mut handler = create_test_handler().await;

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
    async fn test_initialize_with_supported_version() {
        let mut handler = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2025-06-18",
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

        let result = response.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert!(handler.initialized);
    }

    #[tokio::test]
    async fn test_initialize_with_newer_version() {
        let mut handler = create_test_handler().await;

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

        assert!(response.error.is_none());
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        // Should negotiate down to our newest supported version
        assert_eq!(result["protocolVersion"], "2025-11-25");
        assert!(handler.initialized);
    }
}
