//! MCP Request Handler
//!
//! Routes incoming JSON-RPC requests to the appropriate method handlers.

use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{debug, error};

use crate::mcp::error::McpError;
use crate::mcp::protocol::*;
use crate::mcp::tools;

pub struct McpHandler {
    #[allow(dead_code)]
    db_pool: Arc<SqlitePool>,
    #[allow(dead_code)]
    team: String,
    initialized: bool,
}

impl McpHandler {
    pub fn new(db_pool: Arc<SqlitePool>, team: String) -> Self {
        Self { db_pool, team, initialized: false }
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

        let response = match request.method.as_str() {
            "initialize" => self.handle_initialize(request.id.clone(), request.params).await,
            "initialized" => self.handle_initialized(request.id.clone()).await,
            "tools/list" => {
                if !self.initialized {
                    self.error_response(request.id.clone(), McpError::NotInitialized)
                } else {
                    self.handle_tools_list(request.id.clone()).await
                }
            }
            "tools/call" => {
                if !self.initialized {
                    self.error_response(request.id.clone(), McpError::NotInitialized)
                } else {
                    self.handle_tools_call(request.id.clone(), request.params).await
                }
            }
            "resources/list" => {
                if !self.initialized {
                    self.error_response(request.id.clone(), McpError::NotInitialized)
                } else {
                    self.handle_resources_list(request.id.clone()).await
                }
            }
            "resources/read" => {
                if !self.initialized {
                    self.error_response(request.id.clone(), McpError::NotInitialized)
                } else {
                    self.handle_resources_read(request.id.clone(), request.params).await
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

        self.initialized = true;

        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: Some(false) }),
                resources: Some(ResourcesCapability {
                    subscribe: Some(false),
                    list_changed: Some(false),
                }),
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
            tools::cp_list_clusters_tool(),
            tools::cp_get_cluster_tool(),
            tools::cp_list_listeners_tool(),
            tools::cp_get_listener_tool(),
            tools::cp_list_routes_tool(),
            tools::cp_list_filters_tool(),
            tools::cp_get_filter_tool(),
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

        let args = params.arguments.unwrap_or(serde_json::json!({}));

        let result = match params.name.as_str() {
            "cp_list_clusters" => {
                tools::execute_list_clusters(&self.db_pool, &self.team, args).await
            }
            "cp_get_cluster" => tools::execute_get_cluster(&self.db_pool, &self.team, args).await,
            "cp_list_listeners" => {
                tools::execute_list_listeners(&self.db_pool, &self.team, args).await
            }
            "cp_get_listener" => tools::execute_get_listener(&self.db_pool, &self.team, args).await,
            "cp_list_routes" => tools::execute_list_routes(&self.db_pool, &self.team, args).await,
            "cp_list_filters" => tools::execute_list_filters(&self.db_pool, &self.team, args).await,
            "cp_get_filter" => tools::execute_get_filter(&self.db_pool, &self.team, args).await,
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
        debug!("Listing available resources");

        let resources = vec![];

        let result = ResourcesListResult { resources, next_cursor: None };

        match serde_json::to_value(result) {
            Ok(value) => {
                JsonRpcResponse { jsonrpc: "2.0".to_string(), id, result: Some(value), error: None }
            }
            Err(e) => self.error_response(id, McpError::SerializationError(e)),
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

        self.error_response(id, McpError::ResourceNotFound(params.uri))
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
        McpHandler::new(Arc::new(pool), "test-team".to_string())
    }

    #[tokio::test]
    async fn test_initialize() {
        let mut handler = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2024-11-05",
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
    async fn test_not_initialized() {
        let mut handler = create_test_handler().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(1)),
            method: "tools/list".to_string(),
            params: serde_json::json!({}),
        };

        let response = handler.handle_request(request).await;

        assert!(response.result.is_none());
        assert!(response.error.is_some());
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
                "protocolVersion": "2024-11-05",
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
}
