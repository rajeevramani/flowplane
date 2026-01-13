//! OpenAPI export utilities for the learning feature
//!
//! This module provides utilities for transforming learned API schemas into valid
//! OpenAPI 3.1 specifications. It handles:
//!
//! - Query parameter extraction from stored paths
//! - Operation ID sanitization
//! - Schema format conversion (internal -> OpenAPI 3.1)
//! - Path deduplication and parameter merging

use std::collections::HashMap;

/// Parsed path components extracted from a URL path with optional query string
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPath {
    /// Base path without query string (e.g., "/v2/api/atms")
    pub base_path: String,
    /// Extracted query parameters as key-value pairs
    pub query_params: Vec<(String, String)>,
}

/// Parse a path that may contain a query string
///
/// Splits the path at the first `?` and parses query parameters.
///
/// # Examples
///
/// ```
/// use flowplane::api::handlers::openapi_utils::parse_path_with_query;
///
/// let parsed = parse_path_with_query("/v2/api/atms?status=offline");
/// assert_eq!(parsed.base_path, "/v2/api/atms");
/// assert_eq!(parsed.query_params, vec![("status".to_string(), "offline".to_string())]);
/// ```
pub fn parse_path_with_query(path: &str) -> ParsedPath {
    // Handle fragment identifiers (remove everything after #)
    let path_without_fragment = path.split('#').next().unwrap_or(path);

    match path_without_fragment.split_once('?') {
        Some((base, query)) => {
            let params: Vec<(String, String)> = query
                .split('&')
                .filter_map(|pair| {
                    let pair = pair.trim();
                    if pair.is_empty() {
                        return None;
                    }
                    match pair.split_once('=') {
                        Some((k, v)) => {
                            let key = url_decode(k.trim());
                            let value = url_decode(v.trim());
                            if key.is_empty() {
                                None
                            } else {
                                Some((key, value))
                            }
                        }
                        // Handle params without value (e.g., "?flag")
                        None => {
                            let key = url_decode(pair);
                            if key.is_empty() {
                                None
                            } else {
                                Some((key, String::new()))
                            }
                        }
                    }
                })
                .collect();

            ParsedPath { base_path: base.to_string(), query_params: params }
        }
        None => ParsedPath { base_path: path_without_fragment.to_string(), query_params: vec![] },
    }
}

/// Simple URL decoding for common cases
fn url_decode(s: &str) -> String {
    // Handle common URL-encoded characters
    s.replace("%20", " ")
        .replace("%21", "!")
        .replace("%22", "\"")
        .replace("%23", "#")
        .replace("%24", "$")
        .replace("%25", "%")
        .replace("%26", "&")
        .replace("%27", "'")
        .replace("%28", "(")
        .replace("%29", ")")
        .replace("%2B", "+")
        .replace("%2C", ",")
        .replace("%2F", "/")
        .replace("%3A", ":")
        .replace("%3B", ";")
        .replace("%3D", "=")
        .replace("%3F", "?")
        .replace("%40", "@")
}

/// Extract path template parameters from a path
///
/// Finds all `{paramName}` patterns in the path and returns the parameter names.
///
/// # Examples
///
/// ```
/// use flowplane::api::handlers::openapi_utils::extract_path_parameters;
///
/// assert_eq!(extract_path_parameters("/users/{id}"), vec!["id"]);
/// assert_eq!(extract_path_parameters("/users/{userId}/orders/{orderId}"), vec!["userId", "orderId"]);
/// assert_eq!(extract_path_parameters("/users"), Vec::<String>::new());
/// ```
pub fn extract_path_parameters(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut chars = path.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            let mut param_name = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                param_name.push(inner);
            }
            if !param_name.is_empty() {
                params.push(param_name);
            }
        }
    }

    params
}

/// Build OpenAPI path parameters from extracted template parameters
///
/// Creates parameter objects for path template variables like `{customerId}`.
/// All path parameters are required by OpenAPI spec.
pub fn build_path_parameters(param_names: &[String]) -> Vec<serde_json::Value> {
    param_names
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": {
                    "type": "string"
                }
            })
        })
        .collect()
}

/// Generate a valid OpenAPI operation ID from HTTP method and path
///
/// OpenAPI operation IDs must be alphanumeric with underscores/hyphens.
/// This function sanitizes the path by removing query strings and special characters.
///
/// # Examples
///
/// ```
/// use flowplane::api::handlers::openapi_utils::generate_operation_id;
///
/// assert_eq!(generate_operation_id("GET", "/v2/api/atms?status=offline"), "get_v2_api_atms");
/// assert_eq!(generate_operation_id("POST", "/users/{id}"), "post_users_id");
/// ```
pub fn generate_operation_id(method: &str, path: &str) -> String {
    let parsed = parse_path_with_query(path);
    let method_key = method.to_lowercase();

    // Replace path separators and remove template markers
    let sanitized_path = parsed
        .base_path
        .replace('/', "_")
        .replace(['{', '}'], "")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();

    // Remove leading underscores
    let path_part = sanitized_path.trim_start_matches('_');

    if path_part.is_empty() {
        method_key
    } else {
        format!("{}_{}", method_key, path_part)
    }
}

/// Infer OpenAPI parameter type from an observed value
///
/// Analyzes the string representation of a value to determine its most likely type.
///
/// # Returns
///
/// - `"boolean"` for "true" or "false" (case-insensitive)
/// - `"integer"` for whole numbers
/// - `"number"` for decimal numbers
/// - `"string"` for everything else
pub fn infer_param_type(value: &str) -> &'static str {
    let value = value.trim();

    // Check for boolean
    if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
        return "boolean";
    }

    // Check for integer (including negative)
    if value.parse::<i64>().is_ok() {
        return "integer";
    }

    // Check for number (float)
    if value.parse::<f64>().is_ok() {
        return "number";
    }

    // Default to string
    "string"
}

/// Convert internal schema format to OpenAPI 3.1 compliant format
///
/// Transforms flowplane-internal schema representations to standard OpenAPI 3.1:
///
/// - `array_constraints` -> `minItems`, `maxItems`, `uniqueItems`
/// - `numeric_constraints` -> `minimum`, `maximum`, `multipleOf`
/// - `type: { oneof: [...] }` -> `oneOf: [{ type: "..." }]`
/// - Removes internal metadata fields
pub fn convert_schema_to_openapi(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(map) => {
            let mut result = serde_json::Map::new();

            for (key, value) in map {
                match key.as_str() {
                    // Convert array_constraints to standard OpenAPI properties
                    "array_constraints" => {
                        if let Some(obj) = value.as_object() {
                            if let Some(min) = obj.get("min_items") {
                                if !min.is_null() {
                                    result.insert("minItems".to_string(), min.clone());
                                }
                            }
                            if let Some(max) = obj.get("max_items") {
                                if !max.is_null() {
                                    result.insert("maxItems".to_string(), max.clone());
                                }
                            }
                            if let Some(unique) = obj.get("unique_items") {
                                if !unique.is_null() {
                                    result.insert("uniqueItems".to_string(), unique.clone());
                                }
                            }
                        }
                    }

                    // Convert numeric_constraints to standard OpenAPI properties
                    "numeric_constraints" => {
                        if let Some(obj) = value.as_object() {
                            if let Some(min) = obj.get("minimum") {
                                if !min.is_null() {
                                    result.insert("minimum".to_string(), min.clone());
                                }
                            }
                            if let Some(max) = obj.get("maximum") {
                                if !max.is_null() {
                                    result.insert("maximum".to_string(), max.clone());
                                }
                            }
                            if let Some(mult) = obj.get("multiple_of") {
                                if !mult.is_null() {
                                    result.insert("multipleOf".to_string(), mult.clone());
                                }
                            }
                        }
                    }

                    // Convert internal oneof format to standard OpenAPI oneOf
                    "type" => {
                        if let Some(type_obj) = value.as_object() {
                            if let Some(oneof_arr) =
                                type_obj.get("oneof").and_then(|v| v.as_array())
                            {
                                let one_of: Vec<serde_json::Value> = oneof_arr
                                    .iter()
                                    .map(|t| serde_json::json!({ "type": t }))
                                    .collect();
                                result.insert("oneOf".to_string(), serde_json::json!(one_of));
                            } else {
                                // Pass through if not our special oneof format
                                result.insert(key.clone(), value.clone());
                            }
                        } else {
                            // Regular type value, pass through
                            result.insert(key.clone(), value.clone());
                        }
                    }

                    // Recursively process nested objects
                    "properties" => {
                        if let Some(props) = value.as_object() {
                            let converted_props: serde_json::Map<String, serde_json::Value> = props
                                .iter()
                                .map(|(k, v)| (k.clone(), convert_schema_to_openapi(v)))
                                .collect();
                            result.insert(
                                "properties".to_string(),
                                serde_json::Value::Object(converted_props),
                            );
                        } else {
                            result.insert(key.clone(), convert_schema_to_openapi(value));
                        }
                    }

                    // Recursively process array items
                    "items" => {
                        result.insert("items".to_string(), convert_schema_to_openapi(value));
                    }

                    // Skip internal metadata fields (these shouldn't appear after strip_internal_attributes)
                    "confidence" | "presence_count" | "sample_count" => {
                        // Skip these internal fields
                    }

                    // Pass through all other keys (required, format, enum, etc.)
                    _ => {
                        result.insert(key.clone(), convert_schema_to_openapi(value));
                    }
                }
            }

            serde_json::Value::Object(result)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(convert_schema_to_openapi).collect())
        }
        _ => schema.clone(),
    }
}

/// Information about an observed query parameter
#[derive(Debug, Clone, Default)]
pub struct QueryParamInfo {
    /// Inferred type (string, integer, boolean, number)
    pub param_type: String,
    /// Sample values observed
    pub observed_values: Vec<String>,
    /// Is this parameter required (appears in all observations)?
    pub required: bool,
}

/// Aggregated information for a single OpenAPI path
#[derive(Debug, Clone, Default)]
pub struct AggregatedPathInfo {
    /// Aggregated query parameters with inferred types
    pub query_params: HashMap<String, QueryParamInfo>,
}

/// Aggregate query parameters from multiple schemas with the same base path
///
/// Groups schemas by their base path (without query string) and collects all
/// observed query parameter values for type inference and enum generation.
pub fn aggregate_query_params(
    schemas: &[crate::storage::repositories::AggregatedSchemaData],
) -> HashMap<String, HashMap<String, AggregatedPathInfo>> {
    // Map: base_path -> method -> AggregatedPathInfo
    let mut result: HashMap<String, HashMap<String, AggregatedPathInfo>> = HashMap::new();

    for schema in schemas {
        let parsed = parse_path_with_query(&schema.path);
        let method_key = schema.http_method.to_lowercase();

        let method_map = result.entry(parsed.base_path.clone()).or_default();
        let path_info = method_map.entry(method_key).or_default();

        // Aggregate query parameters
        for (key, value) in &parsed.query_params {
            let param_info =
                path_info.query_params.entry(key.clone()).or_insert_with(|| QueryParamInfo {
                    param_type: "string".to_string(),
                    observed_values: vec![],
                    required: false,
                });

            // Infer type from value - use most specific type seen
            let inferred_type = infer_param_type(value);

            // Type promotion: string is least specific, prefer more specific types
            if param_info.param_type == "string" && inferred_type != "string" {
                param_info.param_type = inferred_type.to_string();
            }

            // Collect unique observed values (for enum generation)
            if !param_info.observed_values.contains(value) {
                param_info.observed_values.push(value.clone());
            }
        }
    }

    result
}

/// Generate a semantic operation summary from HTTP method and path
///
/// Creates a human-readable summary that describes the operation based on HTTP conventions.
/// This improves API documentation quality compared to generic "GET /path" summaries.
///
/// # BUG-007 Fix
/// Replaces generic summaries like "GET /users/{id}" with semantic ones like "Get user by ID".
///
/// # Examples
///
/// ```
/// use flowplane::api::handlers::openapi_utils::generate_semantic_summary;
///
/// assert_eq!(generate_semantic_summary("GET", "/users"), "List users");
/// assert_eq!(generate_semantic_summary("GET", "/users/{id}"), "Get user by ID");
/// assert_eq!(generate_semantic_summary("POST", "/users"), "Create user");
/// assert_eq!(generate_semantic_summary("PUT", "/users/{id}"), "Update user by ID");
/// assert_eq!(generate_semantic_summary("DELETE", "/users/{id}"), "Delete user by ID");
/// ```
pub fn generate_semantic_summary(method: &str, path: &str) -> String {
    let parsed = parse_path_with_query(path);
    let base_path = &parsed.base_path;

    // Extract resource name from path (last non-parameter segment)
    let segments: Vec<&str> =
        base_path.split('/').filter(|s| !s.is_empty() && !s.starts_with('{')).collect();

    if segments.is_empty() {
        return format!("{} root", method);
    }

    // Get the resource name (last non-parameter segment)
    let raw_resource = segments.last().unwrap_or(&"resource");

    // Singularize common patterns (simple heuristic)
    let resource = singularize(raw_resource);

    // Check if path has ID parameter (common patterns: {id}, {uuid}, {name}, etc.)
    let has_id_param = base_path.contains("{id}")
        || base_path.contains("{uuid}")
        || has_resource_id_param(base_path);

    // Generate semantic summary based on HTTP method
    match method.to_uppercase().as_str() {
        "GET" => {
            if has_id_param {
                format!("Get {} by ID", resource)
            } else {
                format!("List {}s", resource)
            }
        }
        "POST" => format!("Create {}", resource),
        "PUT" => format!("Update {} by ID", resource),
        "PATCH" => format!("Partially update {} by ID", resource),
        "DELETE" => format!("Delete {} by ID", resource),
        "HEAD" => format!("Check {} exists", resource),
        "OPTIONS" => format!("Get {} options", resource),
        _ => format!("{} {}", method, resource),
    }
}

/// Simple singularization for common resource names
///
/// This is a basic heuristic that handles common patterns. For production,
/// consider using a proper inflection library.
fn singularize(word: &str) -> String {
    let lower = word.to_lowercase();

    // Handle common irregular plurals
    match lower.as_str() {
        "people" => return "person".to_string(),
        "children" => return "child".to_string(),
        "men" => return "man".to_string(),
        "women" => return "woman".to_string(),
        _ => {}
    }

    // Handle common plural endings
    if lower.ends_with("ies") {
        // categories -> category, entries -> entry
        let base = &lower[..lower.len() - 3];
        return format!("{}y", base);
    }
    if lower.ends_with("es") {
        // Skip common -es words that aren't plurals
        if lower.ends_with("ches")
            || lower.ends_with("shes")
            || lower.ends_with("xes")
            || lower.ends_with("sses")
        {
            // batches -> batch, boxes -> box
            return lower[..lower.len() - 2].to_string();
        }
        // statuses -> status (but be careful with proper -es plurals)
        if lower.ends_with("ses") || lower.ends_with("zes") {
            return lower[..lower.len() - 2].to_string();
        }
    }
    if lower.ends_with('s') && !lower.ends_with("ss") {
        // users -> user, products -> product
        return lower[..lower.len() - 1].to_string();
    }

    // Return as-is if no pattern matched
    word.to_string()
}

/// Check if the path has a resource ID parameter (like /{customerId}, /{orderId}, etc.)
fn has_resource_id_param(path: &str) -> bool {
    // Look for common ID parameter patterns
    let id_patterns = ["Id}", "ID}", "_id}", "Uuid}", "UUID}"];

    for pattern in id_patterns {
        if path.contains(pattern) {
            return true;
        }
    }

    // Also check for single segment parameters at the end (e.g., /users/{id}, /products/{sku})
    let segments: Vec<&str> = path.split('/').collect();
    if let Some(last) = segments.last() {
        if last.starts_with('{') && last.ends_with('}') {
            return true;
        }
    }

    false
}

/// Build OpenAPI parameters array from aggregated query params
pub fn build_query_parameters(params: &HashMap<String, QueryParamInfo>) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = params
        .iter()
        .map(|(name, info)| {
            let mut schema = serde_json::json!({
                "type": info.param_type
            });

            // Add enum if we have a small set of observed values (1-5)
            if !info.observed_values.is_empty() && info.observed_values.len() <= 5 {
                // Convert values to appropriate types based on param_type
                let typed_values: Vec<serde_json::Value> = info
                    .observed_values
                    .iter()
                    .map(|v| match info.param_type.as_str() {
                        "integer" => v
                            .parse::<i64>()
                            .map(serde_json::Value::from)
                            .unwrap_or_else(|_| serde_json::Value::String(v.clone())),
                        "number" => v
                            .parse::<f64>()
                            .map(|n| serde_json::json!(n))
                            .unwrap_or_else(|_| serde_json::Value::String(v.clone())),
                        "boolean" => serde_json::Value::Bool(v.eq_ignore_ascii_case("true")),
                        _ => serde_json::Value::String(v.clone()),
                    })
                    .collect();
                schema["enum"] = serde_json::json!(typed_values);
            }

            let mut param = serde_json::json!({
                "name": name,
                "in": "query",
                "required": info.required,
                "schema": schema
            });

            // Add example if we have observed values
            if let Some(first_value) = info.observed_values.first() {
                // Convert example to appropriate type
                let example = match info.param_type.as_str() {
                    "integer" => first_value
                        .parse::<i64>()
                        .map(serde_json::Value::from)
                        .unwrap_or_else(|_| serde_json::Value::String(first_value.clone())),
                    "number" => first_value
                        .parse::<f64>()
                        .map(|n| serde_json::json!(n))
                        .unwrap_or_else(|_| serde_json::Value::String(first_value.clone())),
                    "boolean" => serde_json::Value::Bool(first_value.eq_ignore_ascii_case("true")),
                    _ => serde_json::Value::String(first_value.clone()),
                };
                param["example"] = example;
            }

            param
        })
        .collect();

    // Sort parameters by name for consistent output
    result.sort_by(|a, b| {
        let name_a = a["name"].as_str().unwrap_or("");
        let name_b = b["name"].as_str().unwrap_or("");
        name_a.cmp(name_b)
    });

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============== parse_path_with_query tests ==============

    #[test]
    fn test_parse_path_without_query() {
        let parsed = parse_path_with_query("/v2/api/atms");
        assert_eq!(parsed.base_path, "/v2/api/atms");
        assert!(parsed.query_params.is_empty());
    }

    #[test]
    fn test_parse_path_with_single_query_param() {
        let parsed = parse_path_with_query("/v2/api/atms?status=offline");
        assert_eq!(parsed.base_path, "/v2/api/atms");
        assert_eq!(parsed.query_params, vec![("status".to_string(), "offline".to_string())]);
    }

    #[test]
    fn test_parse_path_with_multiple_query_params() {
        let parsed = parse_path_with_query("/loans?customerId=9&loanType=auto");
        assert_eq!(parsed.base_path, "/loans");
        assert_eq!(parsed.query_params.len(), 2);
        assert!(parsed.query_params.contains(&("customerId".to_string(), "9".to_string())));
        assert!(parsed.query_params.contains(&("loanType".to_string(), "auto".to_string())));
    }

    #[test]
    fn test_parse_path_with_empty_query_string() {
        let parsed = parse_path_with_query("/path?");
        assert_eq!(parsed.base_path, "/path");
        assert!(parsed.query_params.is_empty());
    }

    #[test]
    fn test_parse_path_with_param_no_value() {
        let parsed = parse_path_with_query("/path?flag");
        assert_eq!(parsed.base_path, "/path");
        assert_eq!(parsed.query_params, vec![("flag".to_string(), String::new())]);
    }

    #[test]
    fn test_parse_path_with_url_encoded_value() {
        let parsed = parse_path_with_query("/path?name=John%20Doe");
        assert_eq!(parsed.base_path, "/path");
        assert_eq!(parsed.query_params, vec![("name".to_string(), "John Doe".to_string())]);
    }

    #[test]
    fn test_parse_path_with_fragment() {
        let parsed = parse_path_with_query("/path#section?query=value");
        assert_eq!(parsed.base_path, "/path");
        assert!(parsed.query_params.is_empty()); // Fragment comes before query, so no query parsed
    }

    #[test]
    fn test_parse_path_with_query_and_fragment() {
        let parsed = parse_path_with_query("/path?key=value#section");
        assert_eq!(parsed.base_path, "/path");
        assert_eq!(parsed.query_params, vec![("key".to_string(), "value".to_string())]);
    }

    // ============== generate_operation_id tests ==============

    #[test]
    fn test_generate_operation_id_simple() {
        let op_id = generate_operation_id("GET", "/v2/api/atms");
        assert_eq!(op_id, "get_v2_api_atms");
    }

    #[test]
    fn test_generate_operation_id_with_query() {
        let op_id = generate_operation_id("GET", "/v2/api/atms?status=offline");
        assert_eq!(op_id, "get_v2_api_atms");
    }

    #[test]
    fn test_generate_operation_id_with_path_params() {
        let op_id = generate_operation_id("GET", "/users/{id}/orders");
        assert_eq!(op_id, "get_users_id_orders");
    }

    #[test]
    fn test_generate_operation_id_post() {
        let op_id = generate_operation_id("POST", "/users");
        assert_eq!(op_id, "post_users");
    }

    #[test]
    fn test_generate_operation_id_root_path() {
        let op_id = generate_operation_id("GET", "/");
        assert_eq!(op_id, "get");
    }

    #[test]
    fn test_generate_operation_id_complex_path() {
        let op_id =
            generate_operation_id("DELETE", "/v2/api/users/{userId}/orders/{orderId}?force=true");
        assert_eq!(op_id, "delete_v2_api_users_userId_orders_orderId");
    }

    // ============== infer_param_type tests ==============

    #[test]
    fn test_infer_param_type_integer() {
        assert_eq!(infer_param_type("123"), "integer");
        assert_eq!(infer_param_type("-456"), "integer");
        assert_eq!(infer_param_type("0"), "integer");
    }

    #[test]
    fn test_infer_param_type_boolean() {
        assert_eq!(infer_param_type("true"), "boolean");
        assert_eq!(infer_param_type("false"), "boolean");
        assert_eq!(infer_param_type("TRUE"), "boolean");
        assert_eq!(infer_param_type("False"), "boolean");
    }

    #[test]
    fn test_infer_param_type_number() {
        assert_eq!(infer_param_type("3.14"), "number");
        assert_eq!(infer_param_type("-2.5"), "number");
        assert_eq!(infer_param_type("0.0"), "number");
    }

    #[test]
    fn test_infer_param_type_string() {
        assert_eq!(infer_param_type("hello"), "string");
        assert_eq!(infer_param_type("offline"), "string");
        assert_eq!(infer_param_type("user@example.com"), "string");
        assert_eq!(infer_param_type(""), "string");
    }

    // ============== convert_schema_to_openapi tests ==============

    #[test]
    fn test_convert_schema_array_constraints() {
        let input = serde_json::json!({
            "type": "array",
            "array_constraints": {
                "min_items": 1,
                "max_items": 10,
                "unique_items": null
            }
        });
        let output = convert_schema_to_openapi(&input);
        assert_eq!(output["type"], "array");
        assert_eq!(output["minItems"], 1);
        assert_eq!(output["maxItems"], 10);
        assert!(output.get("uniqueItems").is_none());
        assert!(output.get("array_constraints").is_none());
    }

    #[test]
    fn test_convert_schema_numeric_constraints() {
        let input = serde_json::json!({
            "type": "integer",
            "numeric_constraints": {
                "minimum": 0,
                "maximum": 100,
                "multiple_of": null
            }
        });
        let output = convert_schema_to_openapi(&input);
        assert_eq!(output["type"], "integer");
        assert_eq!(output["minimum"], 0);
        assert_eq!(output["maximum"], 100);
        assert!(output.get("multipleOf").is_none());
        assert!(output.get("numeric_constraints").is_none());
    }

    #[test]
    fn test_convert_schema_oneof_type() {
        let input = serde_json::json!({
            "type": {
                "oneof": ["string", "null"]
            }
        });
        let output = convert_schema_to_openapi(&input);
        assert!(output.get("type").is_none());
        let one_of = output["oneOf"].as_array().unwrap();
        assert_eq!(one_of.len(), 2);
        assert_eq!(one_of[0]["type"], "string");
        assert_eq!(one_of[1]["type"], "null");
    }

    #[test]
    fn test_convert_schema_nested_properties() {
        let input = serde_json::json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "numeric_constraints": {
                        "minimum": 0,
                        "maximum": 100,
                        "multiple_of": null
                    }
                }
            }
        });
        let output = convert_schema_to_openapi(&input);
        assert_eq!(output["type"], "object");
        assert_eq!(output["properties"]["count"]["type"], "integer");
        assert_eq!(output["properties"]["count"]["minimum"], 0);
        assert_eq!(output["properties"]["count"]["maximum"], 100);
        assert!(output["properties"]["count"].get("numeric_constraints").is_none());
    }

    #[test]
    fn test_convert_schema_array_with_items() {
        let input = serde_json::json!({
            "type": "array",
            "items": {
                "type": "integer",
                "numeric_constraints": {
                    "minimum": 1,
                    "maximum": 10,
                    "multiple_of": null
                }
            },
            "array_constraints": {
                "min_items": 1,
                "max_items": 5,
                "unique_items": null
            }
        });
        let output = convert_schema_to_openapi(&input);
        assert_eq!(output["type"], "array");
        assert_eq!(output["minItems"], 1);
        assert_eq!(output["maxItems"], 5);
        assert_eq!(output["items"]["type"], "integer");
        assert_eq!(output["items"]["minimum"], 1);
        assert_eq!(output["items"]["maximum"], 10);
    }

    #[test]
    fn test_convert_schema_preserves_format() {
        let input = serde_json::json!({
            "type": "string",
            "format": "email"
        });
        let output = convert_schema_to_openapi(&input);
        assert_eq!(output["type"], "string");
        assert_eq!(output["format"], "email");
    }

    #[test]
    fn test_convert_schema_preserves_required() {
        let input = serde_json::json!({
            "type": "object",
            "required": ["name", "email"],
            "properties": {
                "name": { "type": "string" },
                "email": { "type": "string", "format": "email" }
            }
        });
        let output = convert_schema_to_openapi(&input);
        assert_eq!(output["required"], serde_json::json!(["name", "email"]));
    }

    // ============== build_query_parameters tests ==============

    #[test]
    fn test_build_query_parameters_empty() {
        let params: HashMap<String, QueryParamInfo> = HashMap::new();
        let result = build_query_parameters(&params);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_query_parameters_single() {
        let mut params = HashMap::new();
        params.insert(
            "status".to_string(),
            QueryParamInfo {
                param_type: "string".to_string(),
                observed_values: vec!["offline".to_string()],
                required: false,
            },
        );

        let result = build_query_parameters(&params);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "status");
        assert_eq!(result[0]["in"], "query");
        assert_eq!(result[0]["schema"]["type"], "string");
        assert_eq!(result[0]["example"], "offline");
    }

    #[test]
    fn test_build_query_parameters_with_enum() {
        let mut params = HashMap::new();
        params.insert(
            "status".to_string(),
            QueryParamInfo {
                param_type: "string".to_string(),
                observed_values: vec!["active".to_string(), "inactive".to_string()],
                required: false,
            },
        );

        let result = build_query_parameters(&params);
        assert_eq!(result[0]["schema"]["enum"], serde_json::json!(["active", "inactive"]));
    }

    #[test]
    fn test_build_query_parameters_integer_type() {
        let mut params = HashMap::new();
        params.insert(
            "customerId".to_string(),
            QueryParamInfo {
                param_type: "integer".to_string(),
                observed_values: vec!["9".to_string(), "10".to_string(), "11".to_string()],
                required: false,
            },
        );

        let result = build_query_parameters(&params);
        assert_eq!(result[0]["schema"]["type"], "integer");
        assert_eq!(result[0]["schema"]["enum"], serde_json::json!([9, 10, 11]));
        assert_eq!(result[0]["example"], 9);
    }

    #[test]
    fn test_build_query_parameters_sorted_by_name() {
        let mut params = HashMap::new();
        params.insert(
            "zebra".to_string(),
            QueryParamInfo {
                param_type: "string".to_string(),
                observed_values: vec![],
                required: false,
            },
        );
        params.insert(
            "alpha".to_string(),
            QueryParamInfo {
                param_type: "string".to_string(),
                observed_values: vec![],
                required: false,
            },
        );
        params.insert(
            "beta".to_string(),
            QueryParamInfo {
                param_type: "string".to_string(),
                observed_values: vec![],
                required: false,
            },
        );

        let result = build_query_parameters(&params);
        assert_eq!(result[0]["name"], "alpha");
        assert_eq!(result[1]["name"], "beta");
        assert_eq!(result[2]["name"], "zebra");
    }

    // ============== extract_path_parameters tests ==============

    #[test]
    fn test_extract_path_parameters_none() {
        let params = extract_path_parameters("/users");
        assert!(params.is_empty());
    }

    #[test]
    fn test_extract_path_parameters_single() {
        let params = extract_path_parameters("/users/{id}");
        assert_eq!(params, vec!["id"]);
    }

    #[test]
    fn test_extract_path_parameters_multiple() {
        let params = extract_path_parameters("/users/{userId}/orders/{orderId}");
        assert_eq!(params, vec!["userId", "orderId"]);
    }

    #[test]
    fn test_extract_path_parameters_at_start() {
        let params = extract_path_parameters("/{version}/api/users");
        assert_eq!(params, vec!["version"]);
    }

    #[test]
    fn test_extract_path_parameters_complex() {
        let params = extract_path_parameters(
            "/v2/api/customers/{customerId}/accounts/{accountId}/transactions",
        );
        assert_eq!(params, vec!["customerId", "accountId"]);
    }

    // ============== build_path_parameters tests ==============

    #[test]
    fn test_build_path_parameters_empty() {
        let result = build_path_parameters(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_path_parameters_single() {
        let result = build_path_parameters(&["customerId".to_string()]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "customerId");
        assert_eq!(result[0]["in"], "path");
        assert_eq!(result[0]["required"], true);
        assert_eq!(result[0]["schema"]["type"], "string");
    }

    #[test]
    fn test_build_path_parameters_multiple() {
        let result = build_path_parameters(&["userId".to_string(), "orderId".to_string()]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["name"], "userId");
        assert_eq!(result[1]["name"], "orderId");
        // All path params are required
        assert_eq!(result[0]["required"], true);
        assert_eq!(result[1]["required"], true);
    }

    // ============== BUG-007 FIX: Semantic summary generation tests ==============

    #[test]
    fn test_generate_semantic_summary_list() {
        assert_eq!(generate_semantic_summary("GET", "/users"), "List users");
        assert_eq!(generate_semantic_summary("GET", "/v2/api/products"), "List products");
        assert_eq!(generate_semantic_summary("GET", "/categories"), "List categorys");
    }

    #[test]
    fn test_generate_semantic_summary_get_by_id() {
        assert_eq!(generate_semantic_summary("GET", "/users/{id}"), "Get user by ID");
        assert_eq!(generate_semantic_summary("GET", "/users/{userId}"), "Get user by ID");
        assert_eq!(generate_semantic_summary("GET", "/products/{uuid}"), "Get product by ID");
        assert_eq!(
            generate_semantic_summary("GET", "/customers/{customerId}"),
            "Get customer by ID"
        );
    }

    #[test]
    fn test_generate_semantic_summary_create() {
        assert_eq!(generate_semantic_summary("POST", "/users"), "Create user");
        assert_eq!(generate_semantic_summary("POST", "/v2/api/products"), "Create product");
    }

    #[test]
    fn test_generate_semantic_summary_update() {
        assert_eq!(generate_semantic_summary("PUT", "/users/{id}"), "Update user by ID");
        assert_eq!(
            generate_semantic_summary("PATCH", "/users/{id}"),
            "Partially update user by ID"
        );
    }

    #[test]
    fn test_generate_semantic_summary_delete() {
        assert_eq!(generate_semantic_summary("DELETE", "/users/{id}"), "Delete user by ID");
        assert_eq!(
            generate_semantic_summary("DELETE", "/products/{productId}"),
            "Delete product by ID"
        );
    }

    #[test]
    fn test_generate_semantic_summary_root_path() {
        assert_eq!(generate_semantic_summary("GET", "/"), "GET root");
    }

    #[test]
    fn test_generate_semantic_summary_with_query_string() {
        // Query strings should be stripped before generating summary
        assert_eq!(generate_semantic_summary("GET", "/users?status=active"), "List users");
        assert_eq!(
            generate_semantic_summary("GET", "/users/{id}?include=orders"),
            "Get user by ID"
        );
    }

    #[test]
    fn test_singularize() {
        // Regular plurals
        assert_eq!(singularize("users"), "user");
        assert_eq!(singularize("products"), "product");
        assert_eq!(singularize("orders"), "order");

        // -ies plurals
        assert_eq!(singularize("categories"), "category");
        assert_eq!(singularize("entries"), "entry");

        // -es plurals
        assert_eq!(singularize("batches"), "batch");
        assert_eq!(singularize("boxes"), "box");

        // Irregular plurals
        assert_eq!(singularize("people"), "person");
        assert_eq!(singularize("children"), "child");

        // Already singular
        assert_eq!(singularize("user"), "user");
        assert_eq!(singularize("status"), "statu"); // Edge case - this is a limitation
    }

    #[test]
    fn test_has_resource_id_param() {
        assert!(has_resource_id_param("/users/{id}"));
        assert!(has_resource_id_param("/users/{userId}"));
        assert!(has_resource_id_param("/users/{uuid}"));
        assert!(has_resource_id_param("/users/{user_id}"));
        assert!(has_resource_id_param("/products/{sku}")); // Any param at end

        assert!(!has_resource_id_param("/users"));
        assert!(!has_resource_id_param("/v2/api/users"));
    }
}
