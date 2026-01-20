//! MCP API Tools Handler
//!
//! Handles MCP JSON-RPC requests for API tools (gateway_api category).
//! Unlike CP tools which are hardcoded, API tools are dynamically loaded
//! from the database and executed via GatewayExecutor.

use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{debug, error};

use crate::domain::McpToolCategory;
use crate::mcp::error::McpError;
use crate::mcp::gateway::GatewayExecutor;
use crate::mcp::protocol::*;
use crate::storage::repositories::mcp_tool::McpToolRepository;

pub struct McpApiHandler {
    db_pool: Arc<SqlitePool>,
    team: String,
    gateway_executor: GatewayExecutor,
    #[allow(dead_code)]
    initialized: bool,
}

impl McpApiHandler {
    pub fn new(db_pool: Arc<SqlitePool>, team: String) -> Self {
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
                experimental: None,
            },
            server_info: ServerInfo {
                name: "flowplane-mcp-api".to_string(),
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
        let tools: Vec<Tool> = tools_data
            .into_iter()
            .map(|t| Tool {
                name: t.name.clone(),
                description: t.description.clone().unwrap_or_default(),
                input_schema: t.input_schema.clone(),
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

        // Find the tool by name and team
        let tool = match repo.get_by_name(&self.team, &params.name).await {
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

        let args = params.arguments.unwrap_or(serde_json::json!({}));

        // Execute via GatewayExecutor
        let result = self.gateway_executor.execute(&tool, args).await;

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
        let client_version = if client_version.is_empty() { "2024-11-05" } else { client_version };

        let negotiated = SUPPORTED_VERSIONS.iter().rev().find(|&&v| v <= client_version).copied();

        match negotiated {
            Some(v) => Ok(v.to_string()),
            None => Err(McpError::UnsupportedProtocolVersion {
                client: client_version.to_string(),
                supported: SUPPORTED_VERSIONS.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ping() {
        // Create in-memory database
        let pool =
            sqlx::SqlitePool::connect("sqlite::memory:").await.expect("Failed to create pool");

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
        let pool =
            sqlx::SqlitePool::connect("sqlite::memory:").await.expect("Failed to create pool");

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
        let pool =
            sqlx::SqlitePool::connect("sqlite::memory:").await.expect("Failed to create pool");

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
        let pool =
            sqlx::SqlitePool::connect("sqlite::memory:").await.expect("Failed to create pool");

        let handler = McpApiHandler::new(Arc::new(pool), "test-team".to_string());

        let result = handler.negotiate_version("2025-11-25");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2025-11-25");
    }

    #[tokio::test]
    async fn test_version_negotiation_unsupported() {
        let pool =
            sqlx::SqlitePool::connect("sqlite::memory:").await.expect("Failed to create pool");

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
