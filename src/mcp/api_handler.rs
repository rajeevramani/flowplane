//! MCP API Tools Handler
//!
//! Handles MCP JSON-RPC requests for API tools (gateway_api category).
//! Unlike CP tools which are hardcoded, API tools are dynamically loaded
//! from the database and executed via GatewayExecutor.

use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, error};

use crate::domain::McpToolCategory;
use crate::mcp::error::McpError;
use crate::mcp::gateway::GatewayExecutor;
use crate::mcp::protocol::*;
use crate::storage::repositories::mcp_tool::McpToolRepository;
use crate::storage::DbPool;

pub struct McpApiHandler {
    db_pool: Arc<DbPool>,
    team: String,
    gateway_executor: GatewayExecutor,
    #[allow(dead_code)]
    initialized: bool,
}

impl McpApiHandler {
    pub fn new(db_pool: Arc<DbPool>, team: String) -> Self {
        Self { db_pool, team, gateway_executor: GatewayExecutor::new(), initialized: false }
    }

    /// Handle an incoming JSON-RPC request for API tools
    pub async fn handle_request(&mut self, request: JsonRpcRequest) -> JsonRpcResponse {
        let method = request.method.clone();
        let id = request.id.clone();

        debug!(
            method = %method,
            id = ?id,
            team = %self.team,
            "Handling MCP API request"
        );

        let response = match request.method.as_str() {
            "initialize" => self.handle_initialize(request.id.clone(), request.params).await,
            "initialized" => self.handle_initialized(request.id.clone()).await,
            "ping" => self.handle_ping(request.id.clone()).await,
            "tools/list" => self.handle_tools_list(request.id.clone()).await,
            "tools/call" => self.handle_tools_call(request.id.clone(), request.params).await,
            "notifications/initialized" => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: Some(serde_json::json!({})),
                error: None,
            },
            "notifications/cancelled" => {
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
            "Completed MCP API request"
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

        // Version negotiation
        let negotiated_version = match self.negotiate_version(&params.protocol_version) {
            Ok(v) => v,
            Err(e) => return self.error_response(id, e),
        };

        self.initialized = true;

        let result = InitializeResult {
            protocol_version: negotiated_version,
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: Some(false) }),
                resources: None, // API tools don't expose resources
                prompts: None,   // API tools don't expose prompts
                logging: None,   // Simplified for API tools
                completions: None,
                tasks: None,
                experimental: None,
                roots: None,       // Client-only capability
                sampling: None,    // Client-only capability
                elicitation: None, // Client-only capability
            },
            server_info: ServerInfo {
                name: "flowplane-mcp-api".to_string(),
                version: crate::VERSION.to_string(),
                title: Some("Flowplane Gateway MCP Server".to_string()),
                description: Some("Gateway API management via Model Context Protocol".to_string()),
                icons: None,
                website_url: None,
            },
            instructions: None,
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
        debug!(team = %self.team, "Listing API tools");

        let repo = McpToolRepository::new((*self.db_pool).clone());

        // Query enabled API tools for this team
        let tools_data = match repo.list_by_category(&self.team, McpToolCategory::GatewayApi).await
        {
            Ok(tools) => tools.into_iter().filter(|t| t.enabled).collect::<Vec<_>>(),
            Err(e) => {
                error!(error = %e, team = %self.team, "Failed to list API tools");
                return self.error_response(
                    id,
                    McpError::InternalError(format!("Failed to list API tools: {}", e)),
                );
            }
        };

        // Convert to MCP Tool format
        // MCP spec requires inputSchema to always be a valid JSON object
        let tools: Vec<Tool> = tools_data
            .into_iter()
            .map(|t| {
                let mut tool = Tool::new(
                    t.name.clone(),
                    t.description.clone().unwrap_or_default(),
                    if t.input_schema.is_null() || !t.input_schema.is_object() {
                        // Fallback to empty object schema if stored value is null or not an object
                        serde_json::json!({
                            "type": "object",
                            "properties": {},
                            "additionalProperties": false
                        })
                    } else {
                        t.input_schema.clone()
                    },
                );
                // Forward output_schema from learned/OpenAPI data to MCP protocol
                tool.output_schema = t.output_schema.clone();
                tool
            })
            .collect();

        debug!(count = tools.len(), team = %self.team, "Found API tools");

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

        debug!(tool_name = %params.name, team = %self.team, "Executing API tool call");

        let repo = McpToolRepository::new((*self.db_pool).clone());

        // Find the tool by name and team with gateway_host resolved from dataplane
        let tool = match repo.get_by_name_with_gateway(&self.team, &params.name).await {
            Ok(Some(t)) if t.enabled => t,
            Ok(Some(_)) => {
                return self.error_response(
                    id,
                    McpError::ToolNotFound(format!("Tool '{}' is disabled", params.name)),
                );
            }
            Ok(None) => {
                return self.error_response(id, McpError::ToolNotFound(params.name));
            }
            Err(e) => {
                error!(error = %e, tool_name = %params.name, "Failed to get API tool");
                return self.error_response(
                    id,
                    McpError::InternalError(format!("Failed to get tool: {}", e)),
                );
            }
        };

        // Verify it's a gateway API tool
        if tool.category != McpToolCategory::GatewayApi {
            error!(
                tool_name = %params.name,
                category = ?tool.category,
                "Tool is not a gateway API tool"
            );
            return self.error_response(
                id,
                McpError::ToolNotFound(format!("Tool '{}' is not an API tool", params.name)),
            );
        }

        // Require gateway_host for execution - fail explicitly if listener has no dataplane
        let gateway_host = match &tool.gateway_host {
            Some(host) if !host.is_empty() => host.clone(),
            _ => {
                error!(
                    tool_name = %params.name,
                    team = %self.team,
                    "Tool cannot execute: listener has no dataplane with gateway_host"
                );
                return self.error_response(
                    id,
                    McpError::Configuration(format!(
                        "Tool '{}' cannot execute: listener has no dataplane with gateway_host configured. \
                         Create a dataplane first, then assign the listener to it.",
                        params.name
                    )),
                );
            }
        };

        let args = params.arguments.unwrap_or(serde_json::json!({}));

        // Convert McpToolWithGateway to McpToolData for executor
        let tool_data = crate::storage::repositories::mcp_tool::McpToolData {
            id: tool.id,
            team: tool.team,
            name: tool.name.clone(),
            description: tool.description,
            category: tool.category,
            source_type: tool.source_type,
            input_schema: tool.input_schema,
            output_schema: tool.output_schema,
            learned_schema_id: tool.learned_schema_id,
            schema_source: tool.schema_source,
            route_id: tool.route_id,
            http_method: tool.http_method,
            http_path: tool.http_path,
            cluster_name: tool.cluster_name,
            listener_port: tool.listener_port,
            host_header: tool.host_header,
            enabled: tool.enabled,
            confidence: tool.confidence,
            created_at: tool.created_at,
            updated_at: tool.updated_at,
        };

        // Execute via GatewayExecutor with validated gateway_host from dataplane
        let result = self.gateway_executor.execute(&tool_data, args, Some(&gateway_host)).await;

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

    async fn handle_ping(&self, id: Option<JsonRpcId>) -> JsonRpcResponse {
        debug!("Received ping request");
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::json!({})),
            error: None,
        }
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
        error!(error = %error, "MCP API error");
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error.to_json_rpc_error()),
        }
    }

    fn negotiate_version(&self, client_version: &str) -> Result<String, McpError> {
        // Only support MCP 2025-11-25 per Phase 1.1 (Single Version Support)
        if client_version == PROTOCOL_VERSION {
            Ok(PROTOCOL_VERSION.to_string())
        } else {
            Err(McpError::UnsupportedProtocolVersion {
                client: client_version.to_string(),
                supported: vec![PROTOCOL_VERSION.to_string()],
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_ping() {
        let _db = TestDatabase::new("mcp_api_handler_ping").await;
        let pool = _db.pool.clone();

        let mut handler = McpApiHandler::new(Arc::new(pool), "test-team".to_string());

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "ping".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert_eq!(response.result.unwrap(), serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_initialize() {
        let _db = TestDatabase::new("mcp_api_handler_init").await;
        let pool = _db.pool.clone();

        let mut handler = McpApiHandler::new(Arc::new(pool), "test-team".to_string());

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

        let result = response.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "flowplane-mcp-api");
        // API tools endpoint doesn't expose resources or prompts
        assert!(result["capabilities"]["resources"].is_null());
        assert!(result["capabilities"]["prompts"].is_null());
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let _db = TestDatabase::new("mcp_api_handler_not_found").await;
        let pool = _db.pool.clone();

        let mut handler = McpApiHandler::new(Arc::new(pool), "test-team".to_string());

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "unknown/method".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, error_codes::METHOD_NOT_FOUND);
        assert!(error.message.contains("Method not found"));
    }

    #[tokio::test]
    async fn test_version_negotiation_supported() {
        let _db = TestDatabase::new("mcp_api_handler_version_ok").await;
        let pool = _db.pool.clone();

        let handler = McpApiHandler::new(Arc::new(pool), "test-team".to_string());

        let result = handler.negotiate_version("2025-11-25");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2025-11-25");
    }

    #[tokio::test]
    async fn test_version_negotiation_unsupported() {
        let _db = TestDatabase::new("mcp_api_handler_version_bad").await;
        let pool = _db.pool.clone();

        let handler = McpApiHandler::new(Arc::new(pool), "test-team".to_string());

        let result = handler.negotiate_version("2020-01-01");
        assert!(result.is_err());
        match result {
            Err(McpError::UnsupportedProtocolVersion { client, supported }) => {
                assert_eq!(client, "2020-01-01");
                assert!(!supported.is_empty());
            }
            _ => panic!("Expected UnsupportedProtocolVersion error"),
        }
    }
}
