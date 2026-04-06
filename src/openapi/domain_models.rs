//! Domain model deduplication for OpenAPI export
//!
//! When multiple endpoints return the same object structure (e.g., `User` in both
//! `GET /users/{id}` and `GET /teams/{id}/members`), the exported OpenAPI spec
//! should use `$ref` references to a shared schema in `components/schemas/` instead
//! of duplicating the schema inline for each endpoint.
//!
//! This module implements:
//! - **Fingerprinting**: Structural hashing of JSON schemas, ignoring stats/format/required/enum
//! - **Discovery**: Finding schemas that appear in 2+ distinct endpoints with 2+ properties
//! - **Name derivation**: Generating human-readable model names from API paths
//! - **Ref replacement**: Replacing inline schemas with `$ref` pointers

use std::collections::{HashMap, HashSet};

use crate::services::singularize;

/// Represents where a schema was found in the API
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct SchemaLocation {
    /// The API path (e.g., "/v2/api/customers/{customerId}")
    pub path: String,
    /// The HTTP method (e.g., "GET", "POST")
    pub method: String,
    /// Where in the request/response (e.g., "request", "response.200")
    pub position: String,
}

/// A discovered domain model that should be promoted to `components/schemas/`
#[derive(Debug, Clone)]
pub struct DomainModel {
    /// The derived model name (e.g., "Customer")
    pub name: String,
    /// The structural fingerprint
    pub fingerprint: String,
    /// The canonical schema value (first occurrence)
    pub schema: serde_json::Value,
    /// All locations where this schema appears
    pub locations: Vec<SchemaLocation>,
}

/// Intermediate tracking of a fingerprinted schema during discovery
#[derive(Debug, Clone)]
struct FingerprintedSchema {
    fingerprint: String,
    schema: serde_json::Value,
    location: SchemaLocation,
}

/// Compute a structural fingerprint for a JSON schema.
///
/// The fingerprint captures the shape of the schema (property names, types, nesting)
/// while ignoring runtime metadata like stats, format, required, enum values, etc.
/// Two schemas with the same fingerprint are structurally identical.
pub fn fingerprint(schema: &serde_json::Value) -> String {
    match schema {
        serde_json::Value::Object(map) => {
            // Check for oneOf (our internal format uses lowercase "oneof" inside "type")
            if let Some(type_val) = map.get("type") {
                if let Some(type_obj) = type_val.as_object() {
                    if let Some(oneof_arr) = type_obj.get("oneof").and_then(|v| v.as_array()) {
                        let mut variant_fps: Vec<String> =
                            oneof_arr.iter().map(fingerprint).collect();
                        variant_fps.sort();
                        return format!("oneOf({})", variant_fps.join("|"));
                    }
                }

                // Check for standard OpenAPI oneOf at top level
                if let Some(oneof_arr) = map.get("oneOf").and_then(|v| v.as_array()) {
                    let mut variant_fps: Vec<String> = oneof_arr.iter().map(fingerprint).collect();
                    variant_fps.sort();
                    return format!("oneOf({})", variant_fps.join("|"));
                }

                let type_str = type_val.as_str().unwrap_or("unknown");

                // Object with properties
                if type_str == "object" {
                    if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
                        let mut prop_fps: Vec<String> = props
                            .iter()
                            .map(|(k, v)| format!("{}:{}", k, fingerprint(v)))
                            .collect();
                        prop_fps.sort();
                        return format!("{{{}}}", prop_fps.join(","));
                    }
                    // Object without properties
                    return "object".to_string();
                }

                // Array with items
                if type_str == "array" {
                    if let Some(items) = map.get("items") {
                        return format!("[{}]", fingerprint(items));
                    }
                    return "array".to_string();
                }

                // Primitive type
                return type_str.to_string();
            }

            // No type field — check for oneOf at top level (standard OpenAPI)
            if let Some(oneof_arr) = map.get("oneOf").and_then(|v| v.as_array()) {
                let mut variant_fps: Vec<String> = oneof_arr.iter().map(fingerprint).collect();
                variant_fps.sort();
                return format!("oneOf({})", variant_fps.join("|"));
            }

            // Unknown object — treat as opaque
            "object".to_string()
        }
        serde_json::Value::String(s) => {
            // Bare type string (e.g., in oneOf variants: ["string", "null"])
            s.clone()
        }
        _ => "unknown".to_string(),
    }
}

/// Extract all object schemas from a JSON schema recursively.
///
/// Returns a list of (schema, json_path) tuples for every object-typed schema
/// found at any nesting level (including the root if it's an object).
fn extract_object_schemas(
    schema: &serde_json::Value,
    location: &SchemaLocation,
) -> Vec<FingerprintedSchema> {
    let mut results = Vec::new();
    extract_objects_recursive(schema, location, &mut results);
    results
}

fn extract_objects_recursive(
    schema: &serde_json::Value,
    location: &SchemaLocation,
    results: &mut Vec<FingerprintedSchema>,
) {
    let map = match schema.as_object() {
        Some(m) => m,
        None => return,
    };

    let type_str = map.get("type").and_then(|t| t.as_str()).unwrap_or("");

    if type_str == "object" {
        if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
            // This is an object with properties — fingerprint it
            let fp = fingerprint(schema);
            results.push(FingerprintedSchema {
                fingerprint: fp,
                schema: schema.clone(),
                location: location.clone(),
            });

            // Recurse into each property to find nested objects
            for (_key, prop_schema) in props {
                extract_objects_recursive(prop_schema, location, results);
            }
        }
    } else if type_str == "array" {
        // Recurse into array items
        if let Some(items) = map.get("items") {
            extract_objects_recursive(items, location, results);
        }
    }
}

/// Derive a model name from an API path.
///
/// Uses the last non-parameter segment of the path, singularized and title-cased.
/// For example:
/// - `/v2/api/customers/{customerId}` -> `Customer`
/// - `/v2/api/customers` -> `Customer`
/// - `/users/{id}/orders` -> `Order`
fn derive_model_name(path: &str, _method: &str) -> String {
    let segments: Vec<&str> =
        path.split('/').filter(|s| !s.is_empty() && !s.starts_with('{')).collect();

    let resource_name = match segments.last() {
        Some(name) => name,
        None => return "Model".to_string(),
    };

    let singular = singularize(resource_name);
    title_case(&singular)
}

/// Derive a model name with priority rules:
/// 1. Single-resource GET (`GET /users/{id}`) -> "User"
/// 2. Collection GET array items (`GET /users`) -> "User"
/// 3. Fallback to first usage path
fn derive_name_from_locations(locations: &[SchemaLocation]) -> String {
    // Priority 1: Look for single-resource GET (path ends with parameter)
    for loc in locations {
        if loc.method.eq_ignore_ascii_case("GET") && loc.path.ends_with('}') {
            return derive_model_name(&loc.path, &loc.method);
        }
    }

    // Priority 2: Look for collection GET
    for loc in locations {
        if loc.method.eq_ignore_ascii_case("GET") && !loc.path.ends_with('}') {
            return derive_model_name(&loc.path, &loc.method);
        }
    }

    // Fallback: use first location
    if let Some(loc) = locations.first() {
        return derive_model_name(&loc.path, &loc.method);
    }

    "Model".to_string()
}

/// Convert a string to TitleCase (first letter uppercase, rest lowercase)
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            let rest: String = chars.collect();
            format!("{}{}", upper, rest)
        }
    }
}

/// Input schema data for domain model discovery
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    /// The API path
    pub path: String,
    /// The HTTP method
    pub method: String,
    /// Request body schema (if any)
    pub request_schema: Option<serde_json::Value>,
    /// Response schemas keyed by status code (if any)
    pub response_schemas: Option<serde_json::Value>,
}

/// Result of domain model discovery
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    /// Domain models to place in `components/schemas/`
    pub models: Vec<DomainModel>,
    /// Map from fingerprint to model name for quick lookup during ref replacement
    pub fingerprint_to_name: HashMap<String, String>,
}

/// Discover domain models from a set of aggregated schemas.
///
/// Scans all request and response schemas, fingerprints object types,
/// groups by fingerprint, and promotes to domain model if used in 2+ distinct
/// endpoints with 2+ properties.
pub fn discover_domain_models(entries: &[SchemaEntry]) -> DiscoveryResult {
    // Step 1: Extract all object schemas from all entries
    let mut all_fingerprinted: Vec<FingerprintedSchema> = Vec::new();

    for entry in entries {
        // Extract from request schema
        if let Some(ref req_schema) = entry.request_schema {
            let location = SchemaLocation {
                path: entry.path.clone(),
                method: entry.method.clone(),
                position: "request".to_string(),
            };
            all_fingerprinted.extend(extract_object_schemas(req_schema, &location));
        }

        // Extract from response schemas
        if let Some(ref resp_schemas) = entry.response_schemas {
            if let Some(resp_map) = resp_schemas.as_object() {
                for (status_code, resp_schema) in resp_map {
                    if resp_schema.is_null() {
                        continue;
                    }
                    let location = SchemaLocation {
                        path: entry.path.clone(),
                        method: entry.method.clone(),
                        position: format!("response.{}", status_code),
                    };
                    all_fingerprinted.extend(extract_object_schemas(resp_schema, &location));
                }
            }
        }
    }

    // Step 2: Group by fingerprint
    let mut groups: HashMap<String, Vec<FingerprintedSchema>> = HashMap::new();
    for fps in all_fingerprinted {
        groups.entry(fps.fingerprint.clone()).or_default().push(fps);
    }

    // Step 3: Filter and promote
    let mut models: Vec<DomainModel> = Vec::new();
    let mut used_names: HashMap<String, usize> = HashMap::new();

    // Sort fingerprints for deterministic output
    let mut sorted_fps: Vec<String> = groups.keys().cloned().collect();
    sorted_fps.sort();

    for fp in sorted_fps {
        let group = match groups.get(&fp) {
            Some(g) => g,
            None => continue,
        };

        // Count distinct endpoints (path + method pairs)
        let distinct_endpoints: HashSet<String> =
            group.iter().map(|s| format!("{} {}", s.location.method, s.location.path)).collect();

        if distinct_endpoints.len() < 2 {
            continue;
        }

        // Check property count (2+ properties required)
        let representative = &group[0];
        let prop_count = representative
            .schema
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|p| p.len())
            .unwrap_or(0);

        if prop_count < 2 {
            continue;
        }

        // Derive name
        let locations: Vec<SchemaLocation> = group.iter().map(|s| s.location.clone()).collect();
        let base_name = derive_name_from_locations(&locations);

        // Handle collisions
        let name = if let Some(count) = used_names.get_mut(&base_name) {
            *count += 1;
            format!("{}{}", base_name, count)
        } else {
            used_names.insert(base_name.clone(), 1);
            base_name
        };

        models.push(DomainModel {
            name,
            fingerprint: fp,
            schema: representative.schema.clone(),
            locations,
        });
    }

    // Build lookup map
    let fingerprint_to_name: HashMap<String, String> =
        models.iter().map(|m| (m.fingerprint.clone(), m.name.clone())).collect();

    DiscoveryResult { models, fingerprint_to_name }
}

/// Replace inline schemas with `$ref` references to discovered domain models.
///
/// Walks the OpenAPI paths object and replaces any inline schema whose fingerprint
/// matches a domain model with `{"$ref": "#/components/schemas/ModelName"}`.
pub fn replace_with_refs(
    paths: &mut serde_json::Value,
    fingerprint_to_name: &HashMap<String, String>,
) {
    if let Some(paths_obj) = paths.as_object_mut() {
        for (_path, methods) in paths_obj.iter_mut() {
            if let Some(methods_obj) = methods.as_object_mut() {
                for (_method, operation) in methods_obj.iter_mut() {
                    // Replace in request body
                    replace_in_request_body(operation, fingerprint_to_name);

                    // Replace in responses
                    replace_in_responses(operation, fingerprint_to_name);
                }
            }
        }
    }
}

fn replace_in_request_body(
    operation: &mut serde_json::Value,
    fingerprint_to_name: &HashMap<String, String>,
) {
    let schema_ptr = &["requestBody", "content", "application/json", "schema"];
    if let Some(schema) = get_nested_mut(operation, schema_ptr) {
        replace_schema_recursive(schema, fingerprint_to_name);
    }
}

fn replace_in_responses(
    operation: &mut serde_json::Value,
    fingerprint_to_name: &HashMap<String, String>,
) {
    if let Some(responses) = operation.get_mut("responses").and_then(|r| r.as_object_mut()) {
        for (_status, response) in responses.iter_mut() {
            let schema_ptr = &["content", "application/json", "schema"];
            if let Some(schema) = get_nested_mut(response, schema_ptr) {
                replace_schema_recursive(schema, fingerprint_to_name);
            }
        }
    }
}

/// Recursively replace schemas with $ref if they match a domain model fingerprint.
fn replace_schema_recursive(
    schema: &mut serde_json::Value,
    fingerprint_to_name: &HashMap<String, String>,
) {
    let fp = fingerprint(schema);

    // If this schema matches a domain model, replace with $ref
    if let Some(model_name) = fingerprint_to_name.get(&fp) {
        // Only replace object schemas (not primitives/arrays that happen to match)
        let is_object = schema
            .as_object()
            .and_then(|m| m.get("type"))
            .and_then(|t| t.as_str())
            .map(|t| t == "object")
            .unwrap_or(false);

        if is_object {
            *schema = serde_json::json!({
                "$ref": format!("#/components/schemas/{}", model_name)
            });
            return;
        }
    }

    // Recurse into nested schemas
    if let Some(map) = schema.as_object_mut() {
        // Recurse into properties
        if let Some(props) = map.get_mut("properties") {
            if let Some(props_obj) = props.as_object_mut() {
                for (_key, prop_schema) in props_obj.iter_mut() {
                    replace_schema_recursive(prop_schema, fingerprint_to_name);
                }
            }
        }

        // Recurse into array items
        if let Some(items) = map.get_mut("items") {
            replace_schema_recursive(items, fingerprint_to_name);
        }

        // Recurse into oneOf variants
        if let Some(oneof) = map.get_mut("oneOf") {
            if let Some(arr) = oneof.as_array_mut() {
                for variant in arr.iter_mut() {
                    replace_schema_recursive(variant, fingerprint_to_name);
                }
            }
        }
    }
}

/// Navigate into nested JSON by a sequence of keys, returning a mutable reference.
fn get_nested_mut<'a>(
    value: &'a mut serde_json::Value,
    keys: &[&str],
) -> Option<&'a mut serde_json::Value> {
    let mut current = value;
    for key in keys {
        current = current.get_mut(*key)?;
    }
    Some(current)
}

/// Build the `components.schemas` object from discovered domain models.
///
/// Each model's canonical schema (with internal attributes already stripped)
/// is placed under its derived name.
pub fn build_components_schemas(models: &[DomainModel]) -> serde_json::Value {
    let mut schemas = serde_json::Map::new();
    for model in models {
        schemas.insert(model.name.clone(), model.schema.clone());
    }
    serde_json::Value::Object(schemas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // === Fingerprinting tests ===

    #[test]
    fn test_fingerprint_primitive_string() {
        let schema = json!({"type": "string"});
        assert_eq!(fingerprint(&schema), "string");
    }

    #[test]
    fn test_fingerprint_primitive_integer() {
        let schema = json!({"type": "integer"});
        assert_eq!(fingerprint(&schema), "integer");
    }

    #[test]
    fn test_fingerprint_primitive_number() {
        let schema = json!({"type": "number"});
        assert_eq!(fingerprint(&schema), "number");
    }

    #[test]
    fn test_fingerprint_primitive_boolean() {
        let schema = json!({"type": "boolean"});
        assert_eq!(fingerprint(&schema), "boolean");
    }

    #[test]
    fn test_fingerprint_object_with_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            }
        });
        // Properties should be sorted alphabetically
        assert_eq!(fingerprint(&schema), "{age:integer,name:string}");
    }

    #[test]
    fn test_fingerprint_object_ignores_metadata() {
        // Two schemas with same structure but different metadata should match
        let schema1 = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "format": "email"},
                "id": {"type": "integer"}
            },
            "required": ["name", "id"],
            "confidence": 0.95,
            "sample_count": 100
        });

        let schema2 = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "id": {"type": "integer"}
            }
        });

        assert_eq!(fingerprint(&schema1), fingerprint(&schema2));
    }

    #[test]
    fn test_fingerprint_array_with_items() {
        let schema = json!({
            "type": "array",
            "items": {"type": "string"}
        });
        assert_eq!(fingerprint(&schema), "[string]");
    }

    #[test]
    fn test_fingerprint_nested_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "object",
                    "properties": {
                        "street": {"type": "string"},
                        "city": {"type": "string"}
                    }
                },
                "name": {"type": "string"}
            }
        });
        assert_eq!(fingerprint(&schema), "{address:{city:string,street:string},name:string}");
    }

    #[test]
    fn test_fingerprint_oneof_internal_format() {
        // Internal flowplane format: type is an object with "oneof" key
        let schema = json!({
            "type": {"oneof": ["string", "null"]}
        });
        assert_eq!(fingerprint(&schema), "oneOf(null|string)");
    }

    #[test]
    fn test_fingerprint_oneof_standard_format() {
        let schema = json!({
            "oneOf": [
                {"type": "string"},
                {"type": "integer"}
            ]
        });
        assert_eq!(fingerprint(&schema), "oneOf(integer|string)");
    }

    #[test]
    fn test_fingerprint_array_of_objects() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "name": {"type": "string"}
                }
            }
        });
        assert_eq!(fingerprint(&schema), "[{id:integer,name:string}]");
    }

    #[test]
    fn test_fingerprint_property_order_independent() {
        let schema1 = json!({
            "type": "object",
            "properties": {
                "z_field": {"type": "string"},
                "a_field": {"type": "integer"}
            }
        });
        let schema2 = json!({
            "type": "object",
            "properties": {
                "a_field": {"type": "integer"},
                "z_field": {"type": "string"}
            }
        });
        assert_eq!(fingerprint(&schema1), fingerprint(&schema2));
    }

    // === Discovery tests ===

    #[test]
    fn test_discover_shared_response_schema() {
        let user_schema = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "email": {"type": "string"}
            }
        });

        let entries = vec![
            SchemaEntry {
                path: "/users/{userId}".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": user_schema.clone()})),
            },
            SchemaEntry {
                path: "/teams/{teamId}/members".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({
                    "200": {
                        "type": "array",
                        "items": user_schema.clone()
                    }
                })),
            },
        ];

        let result = discover_domain_models(&entries);

        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].name, "User");
        assert_eq!(result.models[0].locations.len(), 2);
    }

    #[test]
    fn test_discover_no_models_when_unique() {
        let entries = vec![
            SchemaEntry {
                path: "/users".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({
                    "200": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "integer"},
                            "name": {"type": "string"}
                        }
                    }
                })),
            },
            SchemaEntry {
                path: "/orders".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({
                    "200": {
                        "type": "object",
                        "properties": {
                            "orderId": {"type": "integer"},
                            "total": {"type": "number"}
                        }
                    }
                })),
            },
        ];

        let result = discover_domain_models(&entries);
        assert_eq!(result.models.len(), 0);
    }

    #[test]
    fn test_discover_not_promoted_single_property() {
        // Schema with only 1 property should not be promoted even if shared
        let schema = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            }
        });

        let entries = vec![
            SchemaEntry {
                path: "/users".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": schema.clone()})),
            },
            SchemaEntry {
                path: "/orders".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": schema.clone()})),
            },
        ];

        let result = discover_domain_models(&entries);
        assert_eq!(result.models.len(), 0);
    }

    #[test]
    fn test_discover_not_promoted_single_endpoint() {
        // Schema used in only 1 endpoint should not be promoted
        let schema = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            }
        });

        let entries = vec![SchemaEntry {
            path: "/users".to_string(),
            method: "GET".to_string(),
            request_schema: None,
            response_schemas: Some(json!({"200": schema})),
        }];

        let result = discover_domain_models(&entries);
        assert_eq!(result.models.len(), 0);
    }

    // === Name derivation tests ===

    #[test]
    fn test_name_from_parameterized_path() {
        assert_eq!(derive_model_name("/users/{userId}", "GET"), "User");
    }

    #[test]
    fn test_name_from_collection_path() {
        assert_eq!(derive_model_name("/users", "GET"), "User");
    }

    #[test]
    fn test_name_from_nested_path() {
        assert_eq!(derive_model_name("/teams/{teamId}/members", "GET"), "Member");
    }

    #[test]
    fn test_name_from_versioned_path() {
        assert_eq!(derive_model_name("/v2/api/customers/{customerId}", "GET"), "Customer");
    }

    #[test]
    fn test_name_priority_single_resource_get() {
        let locations = vec![
            SchemaLocation {
                path: "/users".to_string(),
                method: "POST".to_string(),
                position: "request".to_string(),
            },
            SchemaLocation {
                path: "/users/{userId}".to_string(),
                method: "GET".to_string(),
                position: "response.200".to_string(),
            },
        ];
        assert_eq!(derive_name_from_locations(&locations), "User");
    }

    #[test]
    fn test_name_priority_collection_get() {
        let locations = vec![
            SchemaLocation {
                path: "/users".to_string(),
                method: "POST".to_string(),
                position: "request".to_string(),
            },
            SchemaLocation {
                path: "/users".to_string(),
                method: "GET".to_string(),
                position: "response.200".to_string(),
            },
        ];
        assert_eq!(derive_name_from_locations(&locations), "User");
    }

    #[test]
    fn test_name_collision_handling() {
        // Two different schemas that both derive to "User"
        let user_schema_v1 = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            }
        });

        let user_schema_v2 = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "email": {"type": "string"}
            }
        });

        let entries = vec![
            // First "User" schema — appears in /users and /teams endpoints
            SchemaEntry {
                path: "/users/{userId}".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": user_schema_v1.clone()})),
            },
            SchemaEntry {
                path: "/teams/{teamId}/users".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({
                    "200": {
                        "type": "array",
                        "items": user_schema_v1.clone()
                    }
                })),
            },
            // Second "User" schema (different structure) — appears in /admins and /managers
            SchemaEntry {
                path: "/admin/users/{userId}".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": user_schema_v2.clone()})),
            },
            SchemaEntry {
                path: "/managers/users".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({
                    "200": {
                        "type": "array",
                        "items": user_schema_v2.clone()
                    }
                })),
            },
        ];

        let result = discover_domain_models(&entries);

        assert_eq!(result.models.len(), 2);

        let names: Vec<&str> = result.models.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"User2"));
    }

    // === Ref replacement tests ===

    #[test]
    fn test_replace_with_refs_in_response() {
        let user_fp = "{email:string,id:integer,name:string}";
        let mut fp_map = HashMap::new();
        fp_map.insert(user_fp.to_string(), "User".to_string());

        let mut paths = json!({
            "/users/{userId}": {
                "get": {
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "integer"},
                                            "name": {"type": "string"},
                                            "email": {"type": "string"}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        replace_with_refs(&mut paths, &fp_map);

        let schema = &paths["/users/{userId}"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"];
        assert_eq!(schema, &json!({"$ref": "#/components/schemas/User"}));
    }

    #[test]
    fn test_replace_with_refs_in_request_body() {
        let user_fp = "{email:string,id:integer,name:string}";
        let mut fp_map = HashMap::new();
        fp_map.insert(user_fp.to_string(), "User".to_string());

        let mut paths = json!({
            "/users": {
                "post": {
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "id": {"type": "integer"},
                                        "name": {"type": "string"},
                                        "email": {"type": "string"}
                                    }
                                }
                            }
                        }
                    },
                    "responses": {}
                }
            }
        });

        replace_with_refs(&mut paths, &fp_map);

        let schema =
            &paths["/users"]["post"]["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(schema, &json!({"$ref": "#/components/schemas/User"}));
    }

    #[test]
    fn test_replace_with_refs_in_array_items() {
        let user_fp = "{email:string,id:integer,name:string}";
        let mut fp_map = HashMap::new();
        fp_map.insert(user_fp.to_string(), "User".to_string());

        let mut paths = json!({
            "/users": {
                "get": {
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "id": {"type": "integer"},
                                                "name": {"type": "string"},
                                                "email": {"type": "string"}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        replace_with_refs(&mut paths, &fp_map);

        let items = &paths["/users"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["items"];
        assert_eq!(items, &json!({"$ref": "#/components/schemas/User"}));
    }

    #[test]
    fn test_build_components_schemas() {
        let models = vec![
            DomainModel {
                name: "User".to_string(),
                fingerprint: "fp1".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "name": {"type": "string"}
                    }
                }),
                locations: vec![],
            },
            DomainModel {
                name: "Order".to_string(),
                fingerprint: "fp2".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "orderId": {"type": "integer"},
                        "total": {"type": "number"}
                    }
                }),
                locations: vec![],
            },
        ];

        let components = build_components_schemas(&models);
        assert!(components.get("User").is_some());
        assert!(components.get("Order").is_some());
    }

    // === End-to-end integration test ===

    #[test]
    fn test_full_discovery_and_replacement() {
        let customer_schema = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "email": {"type": "string"}
            }
        });

        let entries = vec![
            SchemaEntry {
                path: "/v2/api/customers".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({
                    "200": {
                        "type": "array",
                        "items": customer_schema.clone()
                    }
                })),
            },
            SchemaEntry {
                path: "/v2/api/customers/{customerId}".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": customer_schema.clone()})),
            },
            SchemaEntry {
                path: "/v2/api/customers".to_string(),
                method: "POST".to_string(),
                request_schema: Some(customer_schema.clone()),
                response_schemas: Some(json!({"201": customer_schema.clone()})),
            },
        ];

        let result = discover_domain_models(&entries);

        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].name, "Customer");

        // Build components
        let components = build_components_schemas(&result.models);
        assert!(components.get("Customer").is_some());

        // Build a mock paths object and replace
        let mut paths = json!({
            "/v2/api/customers": {
                "get": {
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": customer_schema.clone()
                                    }
                                }
                            }
                        }
                    }
                },
                "post": {
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": customer_schema.clone()
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "content": {
                                "application/json": {
                                    "schema": customer_schema.clone()
                                }
                            }
                        }
                    }
                }
            },
            "/v2/api/customers/{customerId}": {
                "get": {
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": customer_schema.clone()
                                }
                            }
                        }
                    }
                }
            }
        });

        replace_with_refs(&mut paths, &result.fingerprint_to_name);

        // Verify all inline customer schemas are replaced with $ref
        let ref_value = json!({"$ref": "#/components/schemas/Customer"});

        // GET /customers — array items should be ref
        let items = &paths["/v2/api/customers"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["items"];
        assert_eq!(items, &ref_value);

        // POST /customers — request body should be ref
        let req = &paths["/v2/api/customers"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"];
        assert_eq!(req, &ref_value);

        // POST /customers — 201 response should be ref
        let resp_201 = &paths["/v2/api/customers"]["post"]["responses"]["201"]["content"]
            ["application/json"]["schema"];
        assert_eq!(resp_201, &ref_value);

        // GET /customers/{customerId} — response should be ref
        let detail = &paths["/v2/api/customers/{customerId}"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"];
        assert_eq!(detail, &ref_value);
    }

    #[test]
    fn test_discover_from_request_and_response() {
        // Same schema in request body of one endpoint and response of another
        let schema = json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "description": {"type": "string"},
                "status": {"type": "string"}
            }
        });

        let entries = vec![
            SchemaEntry {
                path: "/tasks".to_string(),
                method: "POST".to_string(),
                request_schema: Some(schema.clone()),
                response_schemas: None,
            },
            SchemaEntry {
                path: "/tasks/{taskId}".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": schema.clone()})),
            },
        ];

        let result = discover_domain_models(&entries);

        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].name, "Task");
    }

    #[test]
    fn test_fingerprint_object_without_properties() {
        let schema = json!({"type": "object"});
        assert_eq!(fingerprint(&schema), "object");
    }

    #[test]
    fn test_fingerprint_array_without_items() {
        let schema = json!({"type": "array"});
        assert_eq!(fingerprint(&schema), "array");
    }

    #[test]
    fn test_derive_name_empty_path() {
        assert_eq!(derive_model_name("/", "GET"), "Model");
        assert_eq!(derive_model_name("", "GET"), "Model");
    }

    #[test]
    fn test_nested_object_discovery() {
        // A nested "address" object that appears in both customer and supplier schemas
        let address = json!({
            "type": "object",
            "properties": {
                "street": {"type": "string"},
                "city": {"type": "string"},
                "zip": {"type": "string"}
            }
        });

        let customer_schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "address": address.clone()
            }
        });

        let supplier_schema = json!({
            "type": "object",
            "properties": {
                "company": {"type": "string"},
                "address": address.clone()
            }
        });

        let entries = vec![
            SchemaEntry {
                path: "/customers".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": customer_schema})),
            },
            SchemaEntry {
                path: "/suppliers".to_string(),
                method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(json!({"200": supplier_schema})),
            },
        ];

        let result = discover_domain_models(&entries);

        // The nested address object should be discovered as a domain model
        // (it appears in 2 distinct endpoints and has 3 properties)
        let address_models: Vec<&DomainModel> = result
            .models
            .iter()
            .filter(|m| {
                m.schema
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|p| p.contains_key("street"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(address_models.len(), 1);
    }
}
