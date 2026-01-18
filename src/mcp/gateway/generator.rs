//! Gateway Tool Generator
//!
//! Generates MCP tools from routes with metadata for gateway API operations.

use crate::domain::{McpToolCategory, McpToolSourceType};
use crate::mcp::error::McpError;
use crate::storage::repositories::mcp_tool::CreateMcpToolRequest;
use crate::storage::repositories::route::RouteData;
use crate::storage::repositories::route_metadata::RouteMetadataData;
use once_cell::sync::Lazy;
use regex::Regex;

/// Static regex for path parameter extraction - compile-time constant
static PATH_PARAM_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\{([^}]+)\}").unwrap_or_else(|_| Regex::new("").unwrap()));

/// Gateway tool generator for creating MCP tools from routes with metadata
pub struct GatewayToolGenerator;

impl GatewayToolGenerator {
    /// Create a new gateway tool generator
    pub fn new() -> Self {
        Self
    }
}

impl Default for GatewayToolGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayToolGenerator {
    /// Generate an MCP tool from a route with its metadata
    pub fn generate_tool(
        &self,
        route: &RouteData,
        metadata: &RouteMetadataData,
        listener_port: i32,
        team: &str,
    ) -> Result<CreateMcpToolRequest, McpError> {
        let tool_name = self.generate_tool_name(metadata, route);
        let description = self.generate_description(metadata, route);
        let input_schema = self.generate_input_schema(metadata, route);

        Ok(CreateMcpToolRequest {
            team: team.to_string(),
            name: tool_name,
            description: Some(description),
            category: McpToolCategory::GatewayApi,
            source_type: match metadata.source_type {
                crate::domain::RouteMetadataSourceType::Openapi => McpToolSourceType::Openapi,
                crate::domain::RouteMetadataSourceType::Manual => McpToolSourceType::Manual,
                crate::domain::RouteMetadataSourceType::Learned => McpToolSourceType::Learned,
            },
            input_schema,
            output_schema: metadata.response_schemas.clone(),
            learned_schema_id: metadata.learning_schema_id,
            schema_source: None,
            route_id: Some(route.id.clone()),
            http_method: metadata.http_method.clone(),
            http_path: Some(route.path_pattern.clone()),
            cluster_name: None,
            listener_port: Some(listener_port as i64),
            enabled: true,
            confidence: metadata.confidence,
        })
    }

    /// Generate tool name from operation_id or path
    ///
    /// When an operation_id is available (from OpenAPI import), it's used directly.
    /// When no operation_id exists, we generate a name from path + method + route_id suffix
    /// to ensure uniqueness even when multiple routes have the same path pattern.
    fn generate_tool_name(&self, metadata: &RouteMetadataData, route: &RouteData) -> String {
        if let Some(ref operation_id) = metadata.operation_id {
            return format!("api_{}", operation_id);
        }

        // Generate from path + method
        let method = metadata
            .http_method
            .as_ref()
            .map(|m| m.to_lowercase())
            .unwrap_or_else(|| "get".to_string());

        let path_parts: Vec<String> = route
            .path_pattern
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                // Remove curly braces from path parameters
                if s.starts_with('{') && s.ends_with('}') {
                    s.trim_start_matches('{').trim_end_matches('}').replace('-', "_")
                } else {
                    s.replace('-', "_")
                }
            })
            .collect();

        // Include route_id suffix for uniqueness when no operation_id is available
        // This prevents UNIQUE constraint violations when multiple routes have same path
        let route_id_suffix = &route.id.as_str()[..8.min(route.id.as_str().len())];
        format!("api_{}_{}_{}", path_parts.join("_"), method, route_id_suffix)
    }

    /// Generate description from summary or path
    fn generate_description(&self, metadata: &RouteMetadataData, route: &RouteData) -> String {
        if let Some(ref summary) = metadata.summary {
            return summary.clone();
        }

        if let Some(ref description) = metadata.description {
            return description.clone();
        }

        // Generate from method and path
        let method = metadata
            .http_method
            .as_ref()
            .map(|m| m.to_uppercase())
            .unwrap_or_else(|| "GET".to_string());

        format!("{} {}", method, route.path_pattern)
    }

    /// Generate input schema from request body or path params
    fn generate_input_schema(
        &self,
        metadata: &RouteMetadataData,
        route: &RouteData,
    ) -> serde_json::Value {
        let path_params = self.extract_path_parameters(&route.path_pattern);

        // If we have a request body schema, merge it with path params
        if let Some(ref body_schema) = metadata.request_body_schema {
            return self.merge_path_params_with_body_schema(&path_params, body_schema);
        }

        // Fallback to just path parameters
        self.build_path_params_schema(&path_params)
    }

    /// Extract path parameters from path pattern (/users/{id} -> ["id"])
    fn extract_path_parameters(&self, path: &str) -> Vec<String> {
        PATH_PARAM_REGEX
            .captures_iter(path)
            .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
            .collect()
    }

    /// Build schema from path params only (fallback)
    fn build_path_params_schema(&self, params: &[String]) -> serde_json::Value {
        if params.is_empty() {
            return serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            });
        }

        let mut properties = serde_json::Map::new();
        for param in params {
            properties.insert(
                param.clone(),
                serde_json::json!({
                    "type": "string",
                    "description": format!("Path parameter: {}", param)
                }),
            );
        }

        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": params,
            "additionalProperties": false
        })
    }

    /// Merge path parameters with request body schema
    fn merge_path_params_with_body_schema(
        &self,
        path_params: &[String],
        body_schema: &serde_json::Value,
    ) -> serde_json::Value {
        if path_params.is_empty() {
            return body_schema.clone();
        }

        // Start with path params schema
        let mut merged_properties = serde_json::Map::new();
        let mut required = Vec::new();

        // Add path parameters
        for param in path_params {
            merged_properties.insert(
                param.clone(),
                serde_json::json!({
                    "type": "string",
                    "description": format!("Path parameter: {}", param)
                }),
            );
            required.push(param.clone());
        }

        // Merge with body schema properties if it's an object
        if let Some(body_props) = body_schema.get("properties").and_then(|p| p.as_object()) {
            for (key, value) in body_props {
                merged_properties.insert(key.clone(), value.clone());
            }
        }

        // Merge required fields from body schema
        if let Some(body_required) = body_schema.get("required").and_then(|r| r.as_array()) {
            for req in body_required {
                if let Some(field) = req.as_str() {
                    if !required.contains(&field.to_string()) {
                        required.push(field.to_string());
                    }
                }
            }
        }

        serde_json::json!({
            "type": "object",
            "properties": merged_properties,
            "required": required,
            "additionalProperties": false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VirtualHostId;
    use crate::domain::{RouteId, RouteMatchType, RouteMetadataId, RouteMetadataSourceType};
    use chrono::Utc;

    fn create_test_route(path_pattern: &str) -> RouteData {
        RouteData {
            id: RouteId::new(),
            virtual_host_id: VirtualHostId::new(),
            name: "test_route".to_string(),
            path_pattern: path_pattern.to_string(),
            match_type: RouteMatchType::Prefix,
            rule_order: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn create_test_metadata(
        operation_id: Option<String>,
        summary: Option<String>,
        http_method: Option<String>,
    ) -> RouteMetadataData {
        RouteMetadataData {
            id: RouteMetadataId::new(),
            route_id: RouteId::new(),
            operation_id,
            summary,
            description: None,
            tags: None,
            http_method,
            request_body_schema: None,
            response_schemas: None,
            learning_schema_id: None,
            enriched_from_learning: false,
            source_type: RouteMetadataSourceType::Openapi,
            confidence: Some(1.0),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_extract_path_parameters() {
        let generator = GatewayToolGenerator::new();

        assert_eq!(generator.extract_path_parameters("/users/{id}"), vec!["id"]);
        assert_eq!(
            generator.extract_path_parameters("/users/{user_id}/posts/{post_id}"),
            vec!["user_id", "post_id"]
        );
        assert_eq!(generator.extract_path_parameters("/users"), Vec::<String>::new());
    }

    #[test]
    fn test_generate_tool_name_with_operation_id() {
        let generator = GatewayToolGenerator::new();

        let route = create_test_route("/users/{id}");
        let metadata =
            create_test_metadata(Some("getUser".to_string()), None, Some("GET".to_string()));

        let name = generator.generate_tool_name(&metadata, &route);
        assert_eq!(name, "api_getUser");
    }

    #[test]
    fn test_generate_tool_name_without_operation_id() {
        let generator = GatewayToolGenerator::new();

        let route = create_test_route("/users/{id}");
        let metadata = create_test_metadata(None, None, Some("GET".to_string()));

        let name = generator.generate_tool_name(&metadata, &route);
        // Name now includes route_id suffix for uniqueness
        assert!(name.starts_with("api_users_id_get_"));
        assert_eq!(name.len(), "api_users_id_get_".len() + 8); // 8 chars from route_id
    }

    #[test]
    fn test_generate_description_with_summary() {
        let generator = GatewayToolGenerator::new();

        let route = create_test_route("/users/{id}");
        let metadata =
            create_test_metadata(None, Some("Get user by ID".to_string()), Some("GET".to_string()));

        let desc = generator.generate_description(&metadata, &route);
        assert_eq!(desc, "Get user by ID");
    }

    #[test]
    fn test_generate_description_fallback() {
        let generator = GatewayToolGenerator::new();

        let route = create_test_route("/users/{id}");
        let metadata = create_test_metadata(None, None, Some("GET".to_string()));

        let desc = generator.generate_description(&metadata, &route);
        assert_eq!(desc, "GET /users/{id}");
    }

    #[test]
    fn test_build_path_params_schema() {
        let generator = GatewayToolGenerator::new();

        let schema = generator.build_path_params_schema(&["id".to_string()]);
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());
        assert_eq!(schema["properties"]["id"]["type"], "string");
        assert_eq!(schema["required"], serde_json::json!(["id"]));
    }

    #[test]
    fn test_build_path_params_schema_empty() {
        let generator = GatewayToolGenerator::new();

        let schema = generator.build_path_params_schema(&[]);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"], serde_json::json!({}));
    }

    #[test]
    fn test_generate_tool_with_response_schemas() {
        let generator = GatewayToolGenerator::new();

        let route = create_test_route("/users/{id}");
        let mut metadata = create_test_metadata(
            Some("getUser".to_string()),
            Some("Get user by ID".to_string()),
            Some("GET".to_string()),
        );

        // Set response schemas
        let response_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "email": { "type": "string" }
            },
            "required": ["id", "name", "email"]
        });
        metadata.response_schemas = Some(response_schema.clone());

        let tool = generator
            .generate_tool(&route, &metadata, 8080, "test-team")
            .expect("Failed to generate tool");

        // Verify output_schema is populated with response_schemas
        assert!(tool.output_schema.is_some());
        assert_eq!(tool.output_schema.unwrap(), response_schema);
    }

    #[test]
    fn test_generate_tool_without_response_schemas() {
        let generator = GatewayToolGenerator::new();

        let route = create_test_route("/users/{id}");
        let metadata = create_test_metadata(
            Some("getUser".to_string()),
            Some("Get user by ID".to_string()),
            Some("GET".to_string()),
        );

        // Ensure response_schemas is None (already set by create_test_metadata)
        assert!(metadata.response_schemas.is_none());

        let tool = generator
            .generate_tool(&route, &metadata, 8080, "test-team")
            .expect("Failed to generate tool");

        // Verify output_schema is None when metadata has no response_schemas
        assert!(tool.output_schema.is_none());
    }
}
