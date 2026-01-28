//! Gateway Executor
//!
//! Executes HTTP requests through the Envoy gateway based on MCP tool definitions.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, ToolCallResult};
use crate::storage::repositories::mcp_tool::McpToolData;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use tracing::{debug, error};

/// Static HTTP client for gateway requests
static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Static regex for path parameter extraction
static PATH_PARAM_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\{([^}]+)\}").expect("Path parameter regex is valid at compile time")
});

/// Gateway executor for executing HTTP requests through Envoy
pub struct GatewayExecutor;

impl GatewayExecutor {
    /// Create a new gateway executor
    pub fn new() -> Self {
        Self
    }

    /// Execute a gateway tool call
    ///
    /// # Arguments
    /// * `tool` - The MCP tool definition containing HTTP method, path, and listener port
    /// * `arguments` - The arguments to pass to the tool (includes path params and body)
    /// * `gateway_host` - Optional gateway host to use instead of 127.0.0.1
    ///
    /// # Returns
    /// * `Ok(ToolCallResult)` - The result of the tool execution
    /// * `Err(McpError)` - An error if the tool execution failed
    pub async fn execute(
        &self,
        tool: &McpToolData,
        arguments: serde_json::Value,
        gateway_host: Option<&str>,
    ) -> Result<ToolCallResult, McpError> {
        // Extract required fields from tool
        let http_method = tool.http_method.as_ref().ok_or_else(|| {
            McpError::InternalError(format!("Tool '{}' missing http_method", tool.name))
        })?;

        let http_path = tool.http_path.as_ref().ok_or_else(|| {
            McpError::InternalError(format!("Tool '{}' missing http_path", tool.name))
        })?;

        let listener_port = tool.listener_port.ok_or_else(|| {
            McpError::InternalError(format!("Tool '{}' missing listener_port", tool.name))
        })?;

        debug!(
            tool_name = %tool.name,
            http_method = %http_method,
            http_path = %http_path,
            listener_port = listener_port,
            "Executing gateway tool"
        );

        // Build URL with path parameter substitution
        let url = self.build_url(http_path, listener_port, &arguments, gateway_host)?;

        // Extract request body (remove path params from arguments)
        let body = self.extract_request_body(&arguments, http_path)?;

        // Build the Host header value for upstream routing
        // Priority: tool.host_header (from virtual host) > gateway_host:port fallback
        let host_header = tool.host_header.clone().unwrap_or_else(|| {
            let host = gateway_host.unwrap_or("127.0.0.1");
            format!("{}:{}", host, listener_port)
        });

        debug!(
            url = %url,
            method = %http_method,
            host_header = %host_header,
            has_body = !body.is_null(),
            "Executing HTTP request"
        );

        // Execute request based on HTTP method
        // Include Host header for proper Envoy virtual host routing
        let response = match http_method.to_uppercase().as_str() {
            "GET" => {
                let mut req = HTTP_CLIENT.get(&url).header("Host", &host_header);
                // Add query parameters from body for GET requests
                if let Some(obj) = body.as_object() {
                    for (key, value) in obj {
                        if let Some(s) = value.as_str() {
                            req = req.query(&[(key, s)]);
                        } else {
                            req = req.query(&[(key, value.to_string())]);
                        }
                    }
                }
                req.send().await
            }
            "POST" => HTTP_CLIENT.post(&url).header("Host", &host_header).json(&body).send().await,
            "PUT" => HTTP_CLIENT.put(&url).header("Host", &host_header).json(&body).send().await,
            "PATCH" => {
                HTTP_CLIENT.patch(&url).header("Host", &host_header).json(&body).send().await
            }
            "DELETE" => HTTP_CLIENT.delete(&url).header("Host", &host_header).send().await,
            _ => {
                return Err(McpError::InvalidParams(format!(
                    "Unsupported HTTP method: {}",
                    http_method
                )));
            }
        }
        .map_err(|e| {
            error!(error = %e, url = %url, "HTTP request failed");
            McpError::GatewayError(format!("HTTP request failed: {}", e))
        })?;

        let status = response.status();
        let body_text = response.text().await.map_err(|e| {
            error!(error = %e, "Failed to read response body");
            McpError::GatewayError(format!("Failed to read response body: {}", e))
        })?;

        debug!(
            status = %status,
            body_length = body_text.len(),
            "HTTP request completed"
        );

        // Format response
        let content_text = format!(
            "HTTP {} {}\n\n{}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown"),
            body_text
        );

        Ok(ToolCallResult {
            content: vec![ContentBlock::Text { text: content_text }],
            is_error: if status.is_success() { None } else { Some(true) },
        })
    }

    /// Build URL with path parameter substitution
    ///
    /// # Arguments
    /// * `path_pattern` - The path pattern with parameters like "/users/{id}"
    /// * `listener_port` - The Envoy listener port
    /// * `arguments` - The arguments containing parameter values
    /// * `gateway_host` - Optional gateway host (defaults to 127.0.0.1)
    ///
    /// # Returns
    /// * `Ok(String)` - The complete URL with substituted parameters
    /// * `Err(McpError)` - An error if parameter substitution fails
    fn build_url(
        &self,
        path_pattern: &str,
        listener_port: i64,
        arguments: &serde_json::Value,
        gateway_host: Option<&str>,
    ) -> Result<String, McpError> {
        let mut path = path_pattern.to_string();

        let arguments_obj = arguments.as_object().ok_or_else(|| {
            McpError::InvalidParams("Arguments must be a JSON object".to_string())
        })?;

        // Substitute each path parameter using pre-compiled regex
        for captures in PATH_PARAM_REGEX.captures_iter(path_pattern) {
            let param_name = &captures[1];
            let param_value = arguments_obj.get(param_name).ok_or_else(|| {
                McpError::InvalidParams(format!("Missing required path parameter: {}", param_name))
            })?;

            // Convert parameter value to string
            let value_str = match param_value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => {
                    return Err(McpError::InvalidParams(format!(
                        "Path parameter '{}' must be a string, number, or boolean",
                        param_name
                    )));
                }
            };

            path = path.replace(&format!("{{{}}}", param_name), &value_str);
        }

        let host = gateway_host.unwrap_or("127.0.0.1");
        Ok(format!("http://{}:{}{}", host, listener_port, path))
    }

    /// Extract request body by removing path parameters from arguments
    ///
    /// # Arguments
    /// * `arguments` - The full arguments object
    /// * `path_pattern` - The path pattern to extract parameter names from
    ///
    /// # Returns
    /// * `Ok(serde_json::Value)` - The body without path parameters
    /// * `Err(McpError)` - An error if extraction fails
    fn extract_request_body(
        &self,
        arguments: &serde_json::Value,
        path_pattern: &str,
    ) -> Result<serde_json::Value, McpError> {
        // Extract path parameter names using pre-compiled regex
        let path_params: Vec<String> =
            PATH_PARAM_REGEX.captures_iter(path_pattern).map(|cap| cap[1].to_string()).collect();

        // If no arguments or empty object, return empty object
        let Some(arguments_obj) = arguments.as_object() else {
            return Ok(serde_json::json!({}));
        };

        // Create new object without path parameters
        let mut body_map: HashMap<String, serde_json::Value> = HashMap::new();
        for (key, value) in arguments_obj {
            if !path_params.contains(key) {
                body_map.insert(key.clone(), value.clone());
            }
        }

        serde_json::to_value(body_map)
            .map_err(|e| McpError::InternalError(format!("Failed to serialize body: {}", e)))
    }
}

impl Default for GatewayExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{McpToolCategory, McpToolId, McpToolSourceType};

    fn create_test_tool() -> McpToolData {
        McpToolData {
            id: McpToolId::from_string("tool-123".to_string()),
            team: "test-team".to_string(),
            name: "test_tool".to_string(),
            description: Some("Test tool".to_string()),
            category: McpToolCategory::GatewayApi,
            source_type: McpToolSourceType::Builtin,
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            learned_schema_id: None,
            schema_source: None,
            route_id: None,
            http_method: Some("GET".to_string()),
            http_path: Some("/users/{id}".to_string()),
            cluster_name: Some("test-cluster".to_string()),
            listener_port: Some(8080),
            host_header: None,
            enabled: true,
            confidence: Some(1.0),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_build_url_with_single_param() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "id": "123",
            "filter": "active"
        });

        let url =
            executor.build_url("/users/{id}", 8080, &arguments, None).expect("Failed to build URL");

        assert_eq!(url, "http://127.0.0.1:8080/users/123");
    }

    #[test]
    fn test_build_url_with_multiple_params() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "org_id": "org-456",
            "user_id": "user-789"
        });

        let url = executor
            .build_url("/orgs/{org_id}/users/{user_id}", 8080, &arguments, None)
            .expect("Failed to build URL");

        assert_eq!(url, "http://127.0.0.1:8080/orgs/org-456/users/user-789");
    }

    #[test]
    fn test_build_url_with_numeric_param() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "id": 123
        });

        let url =
            executor.build_url("/users/{id}", 8080, &arguments, None).expect("Failed to build URL");

        assert_eq!(url, "http://127.0.0.1:8080/users/123");
    }

    #[test]
    fn test_build_url_with_custom_gateway_host() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "id": "123"
        });

        let url = executor
            .build_url("/users/{id}", 8080, &arguments, Some("10.0.0.5"))
            .expect("Failed to build URL");

        assert_eq!(url, "http://10.0.0.5:8080/users/123");
    }

    #[test]
    fn test_build_url_missing_param() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "name": "john"
        });

        let result = executor.build_url("/users/{id}", 8080, &arguments, None);

        assert!(result.is_err());
        match result {
            Err(McpError::InvalidParams(msg)) => {
                assert!(msg.contains("Missing required path parameter: id"));
            }
            _ => panic!("Expected InvalidParams error"),
        }
    }

    #[test]
    fn test_extract_request_body_filters_path_params() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "id": "123",
            "name": "John Doe",
            "email": "john@example.com"
        });

        let body = executor
            .extract_request_body(&arguments, "/users/{id}")
            .expect("Failed to extract body");

        let body_obj = body.as_object().expect("Body should be an object");
        assert!(!body_obj.contains_key("id"));
        assert_eq!(body_obj.get("name").and_then(|v| v.as_str()), Some("John Doe"));
        assert_eq!(body_obj.get("email").and_then(|v| v.as_str()), Some("john@example.com"));
    }

    #[test]
    fn test_extract_request_body_multiple_path_params() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "org_id": "org-456",
            "user_id": "user-789",
            "role": "admin"
        });

        let body = executor
            .extract_request_body(&arguments, "/orgs/{org_id}/users/{user_id}")
            .expect("Failed to extract body");

        let body_obj = body.as_object().expect("Body should be an object");
        assert!(!body_obj.contains_key("org_id"));
        assert!(!body_obj.contains_key("user_id"));
        assert_eq!(body_obj.get("role").and_then(|v| v.as_str()), Some("admin"));
    }

    #[test]
    fn test_extract_request_body_no_path_params() {
        let executor = GatewayExecutor::new();
        let arguments = serde_json::json!({
            "name": "John Doe",
            "email": "john@example.com"
        });

        let body =
            executor.extract_request_body(&arguments, "/users").expect("Failed to extract body");

        let body_obj = body.as_object().expect("Body should be an object");
        assert_eq!(body_obj.len(), 2);
        assert_eq!(body_obj.get("name").and_then(|v| v.as_str()), Some("John Doe"));
    }

    #[test]
    fn test_execute_missing_http_method() {
        let executor = GatewayExecutor::new();
        let mut tool = create_test_tool();
        tool.http_method = None;

        let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let result = runtime.block_on(async {
            executor.execute(&tool, serde_json::json!({"id": "123"}), None).await
        });

        assert!(result.is_err());
        match result {
            Err(McpError::InternalError(msg)) => {
                assert!(msg.contains("missing http_method"));
            }
            _ => panic!("Expected InternalError"),
        }
    }

    #[test]
    fn test_execute_missing_http_path() {
        let executor = GatewayExecutor::new();
        let mut tool = create_test_tool();
        tool.http_path = None;

        let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let result = runtime.block_on(async {
            executor.execute(&tool, serde_json::json!({"id": "123"}), None).await
        });

        assert!(result.is_err());
        match result {
            Err(McpError::InternalError(msg)) => {
                assert!(msg.contains("missing http_path"));
            }
            _ => panic!("Expected InternalError"),
        }
    }

    #[test]
    fn test_execute_missing_listener_port() {
        let executor = GatewayExecutor::new();
        let mut tool = create_test_tool();
        tool.listener_port = None;

        let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let result = runtime.block_on(async {
            executor.execute(&tool, serde_json::json!({"id": "123"}), None).await
        });

        assert!(result.is_err());
        match result {
            Err(McpError::InternalError(msg)) => {
                assert!(msg.contains("missing listener_port"));
            }
            _ => panic!("Expected InternalError"),
        }
    }
}
