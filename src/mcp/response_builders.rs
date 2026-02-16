//! MCP Response Builders
//!
//! Type-safe response construction utilities for MCP tools.
//! Implements the token-efficient response format from the design template.
//!
//! # Token Budget Guidelines
//!
//! - Query responses: 50-80 tokens max
//! - Action responses: 30-50 tokens max
//! - Error responses: 50-80 tokens with actionable fix suggestions
//!
//! # Response Formats
//!
//! Query success: `{"found": true, "ref": {type, name, id}, "data": {...}}`
//! Query miss: `{"found": false}`
//! Action success: `{"ok": true, "ref": {type, name, id}}`
//! Action error: `{"ok": false, "error": "ERROR_CODE", "existing": {...}, "fix": "..."}`

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Resource reference for minimal responses (15-20 tokens)
///
/// Used in both query and action responses to identify resources
/// without returning full object details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceRef {
    /// Resource type (e.g., "listener", "route", "cluster")
    #[serde(rename = "type")]
    pub resource_type: String,
    /// Resource name (human-readable identifier)
    pub name: String,
    /// Internal resource ID
    pub id: String,
}

impl ResourceRef {
    /// Create a new resource reference
    pub fn new(
        resource_type: impl Into<String>,
        name: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self { resource_type: resource_type.into(), name: name.into(), id: id.into() }
    }

    /// Create a listener resource reference
    pub fn listener(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("listener", name, id)
    }

    /// Create a route resource reference
    pub fn route(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("route", name, id)
    }

    /// Create a cluster resource reference
    pub fn cluster(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("cluster", name, id)
    }

    /// Create a route_config resource reference
    pub fn route_config(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("route_config", name, id)
    }

    /// Create a filter resource reference
    pub fn filter(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("filter", name, id)
    }

    /// Create a virtual_host resource reference
    pub fn virtual_host(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("virtual_host", name, id)
    }

    /// Create a dataplane resource reference
    pub fn dataplane(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("dataplane", name, id)
    }

    /// Create a learning_session resource reference
    pub fn learning_session(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("learning_session", name, id)
    }

    /// Create an openapi_import resource reference
    pub fn openapi_import(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("openapi_import", name, id)
    }

    /// Create an aggregated_schema resource reference
    pub fn aggregated_schema(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("aggregated_schema", name, id)
    }

    /// Create a filter_attachment resource reference
    pub fn filter_attachment(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new("filter_attachment", name, id)
    }
}

/// Build a query response for found/not found scenarios
///
/// # Token Budget: 50-80 tokens max
///
/// # Returns
/// - Not found: `{"found": false}` (~15 tokens)
/// - Found: `{"found": true, "ref": {...}, "data": {...}}` (~50-80 tokens)
///
/// # Example
///
/// ```rust
/// use flowplane::mcp::response_builders::{build_query_response, ResourceRef};
/// use serde_json::json;
///
/// // Not found
/// let response = build_query_response(false, None, None);
/// assert_eq!(response, json!({"found": false}));
///
/// // Found with data
/// let ref_ = ResourceRef::listener("api-gateway", "l-123");
/// let data = json!({"address": "0.0.0.0", "port": 8080});
/// let response = build_query_response(true, Some(ref_), Some(data));
/// ```
pub fn build_query_response(found: bool, ref_: Option<ResourceRef>, data: Option<Value>) -> Value {
    if !found {
        return json!({"found": false});
    }

    let mut response = json!({"found": true});

    if let Some(r) = ref_ {
        response["ref"] = json!({
            "type": r.resource_type,
            "name": r.name,
            "id": r.id
        });
    }

    if let Some(d) = data {
        response["data"] = d;
    }

    response
}

/// Build an action response for success/failure scenarios
///
/// # Token Budget: 30-50 tokens max
///
/// # Returns
/// - Success: `{"ok": true, "ref": {...}}` (~30-40 tokens)
/// - Failure: `{"ok": false}` (~15 tokens)
///
/// # Example
///
/// ```rust
/// use flowplane::mcp::response_builders::{build_action_response, ResourceRef};
///
/// let ref_ = ResourceRef::cluster("user-svc", "c-456");
/// let response = build_action_response(true, Some(ref_));
/// ```
pub fn build_action_response(ok: bool, ref_: Option<ResourceRef>) -> Value {
    if ok {
        let mut response = json!({"ok": true});
        if let Some(r) = ref_ {
            response["ref"] = json!({
                "type": r.resource_type,
                "name": r.name,
                "id": r.id
            });
        }
        response
    } else {
        json!({"ok": false})
    }
}

/// Build an error response with actionable fix suggestion
///
/// # Token Budget: 50-80 tokens max
///
/// # Returns
/// `{"ok": false, "error": "ERROR_CODE", "existing": {...}, "fix": "..."}`
///
/// # Example
///
/// ```rust
/// use flowplane::mcp::response_builders::{build_error_response, error_codes};
/// use serde_json::json;
///
/// let existing = json!({"name": "api-gateway", "port": 8080});
/// let response = build_error_response(
///     error_codes::ALREADY_EXISTS,
///     Some(existing),
///     "use query_port first to check availability"
/// );
/// ```
pub fn build_error_response(error_code: &str, existing: Option<Value>, fix: &str) -> Value {
    let mut response = json!({
        "ok": false,
        "error": error_code,
        "fix": fix
    });

    if let Some(e) = existing {
        response["existing"] = e;
    }

    response
}

/// Build a list response for cp_list_* tools
///
/// # Token Budget: 20-30 tokens per item + 15 tokens pagination
///
/// # Returns
/// `{"items": [{type, name, id}, ...], "count": N}`
///
/// # Example
///
/// ```rust
/// use flowplane::mcp::response_builders::{build_list_response, ResourceRef};
///
/// let refs = vec![
///     ResourceRef::cluster("svc1", "c-1"),
///     ResourceRef::cluster("svc2", "c-2"),
/// ];
/// let response = build_list_response(refs, 2);
/// ```
pub fn build_list_response(items: Vec<ResourceRef>, count: i64) -> Value {
    json!({
        "items": items.iter().map(|r| json!({
            "type": r.resource_type,
            "name": r.name,
            "id": r.id
        })).collect::<Vec<_>>(),
        "count": count
    })
}

/// Build a get response for cp_get_* tools
///
/// # Token Budget: 50-80 tokens (ref + essential data fields)
///
/// # Returns
/// `{"ok": true, "ref": {type, name, id}, "data": {...}}`
///
/// # Example
///
/// ```rust
/// use flowplane::mcp::response_builders::{build_get_response, ResourceRef};
/// use serde_json::json;
///
/// let ref_ = ResourceRef::cluster("user-svc", "c-123");
/// let data = json!({"configuration": {"lb_policy": "round_robin"}, "version": 1});
/// let response = build_get_response(ref_, data);
/// ```
pub fn build_get_response(ref_: ResourceRef, data: Value) -> Value {
    json!({
        "ok": true,
        "ref": {
            "type": ref_.resource_type,
            "name": ref_.name,
            "id": ref_.id
        },
        "data": data
    })
}

/// Build a create response with automatic ref construction
///
/// # Token Budget: 30-50 tokens
///
/// Convenience wrapper around `build_action_response` for create operations.
pub fn build_create_response(resource_type: &str, name: &str, id: &str) -> Value {
    build_action_response(true, Some(ResourceRef::new(resource_type, name, id)))
}

/// Build a rich create response with summary and next-step guidance.
///
/// # Token Budget: 50-80 tokens
///
/// Extends `build_create_response` with agent-friendly context:
/// - `details`: key parameter echo (endpoint count, lb_policy, etc.)
/// - `created`: counts of child resources created (virtual_hosts, routes)
/// - `next_step`: what tool to call next in the workflow
///
/// Agents that receive `{"ok":true}` tend to make verification calls.
/// Agents that receive `{"ok":true, "created":{...}, "next_step":"..."}` proceed with confidence.
pub fn build_rich_create_response(
    resource_type: &str,
    name: &str,
    id: &str,
    details: Option<Value>,
    created: Option<Value>,
    next_step: Option<&str>,
) -> Value {
    let mut response = json!({
        "ok": true,
        "ref": {
            "type": resource_type,
            "name": name,
            "id": id
        }
    });
    if let Some(d) = details {
        response["details"] = d;
    }
    if let Some(c) = created {
        response["created"] = c;
    }
    if let Some(ns) = next_step {
        response["next_step"] = json!(ns);
    }
    response
}

/// Build a rich delete response with confirmation of what was removed.
///
/// # Token Budget: 30-50 tokens
///
/// Extends `build_delete_response` with agent-friendly context:
/// - `deleted`: identifies what was removed (type + name)
/// - `cascade`: optional counts of child resources also removed
pub fn build_rich_delete_response(
    resource_type: &str,
    name: &str,
    cascade: Option<Value>,
) -> Value {
    let mut response = json!({
        "ok": true,
        "deleted": {
            "type": resource_type,
            "name": name
        }
    });
    if let Some(c) = cascade {
        response["cascade"] = c;
    }
    response
}

/// Build an update response with automatic ref construction
///
/// # Token Budget: 30-50 tokens
///
/// Convenience wrapper around `build_action_response` for update operations.
pub fn build_update_response(resource_type: &str, name: &str, id: &str) -> Value {
    build_action_response(true, Some(ResourceRef::new(resource_type, name, id)))
}

/// Build a delete response (minimal `{"ok": true}`)
///
/// # Token Budget: 15 tokens
///
/// Delete operations don't return a ref since the resource no longer exists.
pub fn build_delete_response() -> Value {
    json!({"ok": true})
}

/// Standard error codes for structured error responses
pub mod error_codes {
    /// Resource with the same name/identifier already exists
    pub const ALREADY_EXISTS: &str = "ALREADY_EXISTS";

    /// Requested resource was not found
    pub const NOT_FOUND: &str = "NOT_FOUND";

    /// Configuration validation failed
    pub const INVALID_CONFIG: &str = "INVALID_CONFIG";

    /// Resource has dependents that must be removed first
    pub const DEPENDENCY: &str = "DEPENDENCY";

    /// State conflict (e.g., port already in use)
    pub const CONFLICT: &str = "CONFLICT";

    /// Missing required parameter
    pub const MISSING_PARAM: &str = "MISSING_PARAM";

    /// Invalid parameter value
    pub const INVALID_PARAM: &str = "INVALID_PARAM";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_ref_new() {
        let ref_ = ResourceRef::new("listener", "api-gateway", "l-123");
        assert_eq!(ref_.resource_type, "listener");
        assert_eq!(ref_.name, "api-gateway");
        assert_eq!(ref_.id, "l-123");
    }

    #[test]
    fn test_resource_ref_convenience_methods() {
        let listener = ResourceRef::listener("my-listener", "l-1");
        assert_eq!(listener.resource_type, "listener");

        let route = ResourceRef::route("my-route", "r-1");
        assert_eq!(route.resource_type, "route");

        let cluster = ResourceRef::cluster("my-cluster", "c-1");
        assert_eq!(cluster.resource_type, "cluster");

        let route_config = ResourceRef::route_config("my-config", "rc-1");
        assert_eq!(route_config.resource_type, "route_config");

        let filter = ResourceRef::filter("my-filter", "f-1");
        assert_eq!(filter.resource_type, "filter");
    }

    #[test]
    fn test_query_response_not_found() {
        let response = build_query_response(false, None, None);
        assert_eq!(response, json!({"found": false}));

        // Verify token budget: {"found": false} is ~15 characters
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.len() < 20);
    }

    #[test]
    fn test_query_response_found_minimal() {
        let ref_ = ResourceRef::listener("api-gateway", "l-123");
        let response = build_query_response(true, Some(ref_), None);

        assert_eq!(response["found"], true);
        assert_eq!(response["ref"]["type"], "listener");
        assert_eq!(response["ref"]["name"], "api-gateway");
        assert_eq!(response["ref"]["id"], "l-123");
        assert!(response.get("data").is_none());
    }

    #[test]
    fn test_query_response_found_with_data() {
        let ref_ = ResourceRef::listener("api-gateway", "l-123");
        let data = json!({"address": "0.0.0.0", "port": 8080, "route_config": "api-routes"});
        let response = build_query_response(true, Some(ref_), Some(data));

        assert_eq!(response["found"], true);
        assert_eq!(response["ref"]["type"], "listener");
        assert_eq!(response["data"]["port"], 8080);
        assert_eq!(response["data"]["address"], "0.0.0.0");
        assert_eq!(response["data"]["route_config"], "api-routes");

        // Verify token budget: should be < 200 characters (roughly ~50-80 tokens)
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.len() < 200);
    }

    #[test]
    fn test_action_response_success() {
        let ref_ = ResourceRef::cluster("user-svc", "c-456");
        let response = build_action_response(true, Some(ref_));

        assert_eq!(response["ok"], true);
        assert_eq!(response["ref"]["type"], "cluster");
        assert_eq!(response["ref"]["name"], "user-svc");
        assert_eq!(response["ref"]["id"], "c-456");

        // Verify token budget: should be < 100 characters (roughly ~30-40 tokens)
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.len() < 100);
    }

    #[test]
    fn test_action_response_success_no_ref() {
        let response = build_action_response(true, None);
        assert_eq!(response, json!({"ok": true}));
    }

    #[test]
    fn test_action_response_failure() {
        let response = build_action_response(false, None);
        assert_eq!(response, json!({"ok": false}));
    }

    #[test]
    fn test_error_response_basic() {
        let response =
            build_error_response(error_codes::NOT_FOUND, None, "check resource name and try again");

        assert_eq!(response["ok"], false);
        assert_eq!(response["error"], "NOT_FOUND");
        assert!(response["fix"].as_str().unwrap().contains("check resource name"));
        assert!(response.get("existing").is_none());
    }

    #[test]
    fn test_error_response_with_existing() {
        let existing = json!({"name": "api-gateway", "port": 8080});
        let response = build_error_response(
            error_codes::ALREADY_EXISTS,
            Some(existing),
            "use query_port first to check availability",
        );

        assert_eq!(response["ok"], false);
        assert_eq!(response["error"], "ALREADY_EXISTS");
        assert!(response["fix"].as_str().unwrap().contains("query_port"));
        assert_eq!(response["existing"]["name"], "api-gateway");
        assert_eq!(response["existing"]["port"], 8080);

        // Verify token budget: should be < 250 characters (roughly ~60-80 tokens)
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.len() < 250);
    }

    #[test]
    fn test_error_codes_are_uppercase() {
        // Error codes should be SCREAMING_SNAKE_CASE for consistency
        assert_eq!(error_codes::ALREADY_EXISTS, "ALREADY_EXISTS");
        assert_eq!(error_codes::NOT_FOUND, "NOT_FOUND");
        assert_eq!(error_codes::INVALID_CONFIG, "INVALID_CONFIG");
        assert_eq!(error_codes::DEPENDENCY, "DEPENDENCY");
        assert_eq!(error_codes::CONFLICT, "CONFLICT");
        assert_eq!(error_codes::MISSING_PARAM, "MISSING_PARAM");
        assert_eq!(error_codes::INVALID_PARAM, "INVALID_PARAM");
    }

    #[test]
    fn test_resource_ref_serialization() {
        let ref_ = ResourceRef::new("listener", "my-listener", "l-abc");
        let json = serde_json::to_value(&ref_).unwrap();

        // "type" key should be serialized correctly (renamed from resource_type)
        assert_eq!(json["type"], "listener");
        assert_eq!(json["name"], "my-listener");
        assert_eq!(json["id"], "l-abc");
    }

    #[test]
    fn test_resource_ref_deserialization() {
        let json = json!({
            "type": "cluster",
            "name": "test-cluster",
            "id": "c-xyz"
        });

        let ref_: ResourceRef = serde_json::from_value(json).unwrap();
        assert_eq!(ref_.resource_type, "cluster");
        assert_eq!(ref_.name, "test-cluster");
        assert_eq!(ref_.id, "c-xyz");
    }

    // Tests for new ResourceRef constructors
    #[test]
    fn test_resource_ref_new_constructors() {
        let vh = ResourceRef::virtual_host("my-vh", "vh-1");
        assert_eq!(vh.resource_type, "virtual_host");

        let dp = ResourceRef::dataplane("my-dp", "dp-1");
        assert_eq!(dp.resource_type, "dataplane");

        let ls = ResourceRef::learning_session("my-session", "ls-1");
        assert_eq!(ls.resource_type, "learning_session");

        let oi = ResourceRef::openapi_import("my-import", "oi-1");
        assert_eq!(oi.resource_type, "openapi_import");

        let as_ = ResourceRef::aggregated_schema("GET /api", "as-1");
        assert_eq!(as_.resource_type, "aggregated_schema");

        let fa = ResourceRef::filter_attachment("rate-limit-listener", "fa-1");
        assert_eq!(fa.resource_type, "filter_attachment");
    }

    // Tests for build_list_response
    #[test]
    fn test_list_response_empty() {
        let response = build_list_response(vec![], 0);
        assert_eq!(response["items"].as_array().unwrap().len(), 0);
        assert_eq!(response["count"], 0);
    }

    #[test]
    fn test_list_response_with_items() {
        let refs = vec![
            ResourceRef::cluster("svc1", "c-1"),
            ResourceRef::cluster("svc2", "c-2"),
            ResourceRef::cluster("svc3", "c-3"),
        ];
        let response = build_list_response(refs, 3);

        let items = response["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["type"], "cluster");
        assert_eq!(items[0]["name"], "svc1");
        assert_eq!(items[0]["id"], "c-1");
        assert_eq!(items[1]["name"], "svc2");
        assert_eq!(items[2]["name"], "svc3");
        assert_eq!(response["count"], 3);
    }

    #[test]
    fn test_list_response_token_budget() {
        let refs = vec![
            ResourceRef::cluster("svc1", "c-1"),
            ResourceRef::cluster("svc2", "c-2"),
            ResourceRef::cluster("svc3", "c-3"),
        ];
        let response = build_list_response(refs, 3);
        let serialized = serde_json::to_string(&response).unwrap();

        // Budget: 3 items * ~40 chars + pagination ~20 = ~140 chars
        assert!(serialized.len() < 200);
    }

    // Tests for build_get_response
    #[test]
    fn test_get_response() {
        let ref_ = ResourceRef::cluster("user-svc", "c-123");
        let data = json!({"configuration": {"lb_policy": "round_robin"}, "version": 1});
        let response = build_get_response(ref_, data);

        assert_eq!(response["ok"], true);
        assert_eq!(response["ref"]["type"], "cluster");
        assert_eq!(response["ref"]["name"], "user-svc");
        assert_eq!(response["ref"]["id"], "c-123");
        assert_eq!(response["data"]["version"], 1);
    }

    #[test]
    fn test_get_response_token_budget() {
        let ref_ = ResourceRef::cluster("user-svc", "c-123");
        let data = json!({"config": "minimal", "version": 1});
        let response = build_get_response(ref_, data);
        let serialized = serde_json::to_string(&response).unwrap();

        // Budget: ~50-80 tokens = ~150-240 chars
        assert!(serialized.len() < 250);
    }

    // Tests for convenience action builders
    #[test]
    fn test_create_response() {
        let response = build_create_response("cluster", "test-svc", "c-123");
        assert_eq!(response["ok"], true);
        assert_eq!(response["ref"]["type"], "cluster");
        assert_eq!(response["ref"]["name"], "test-svc");
        assert_eq!(response["ref"]["id"], "c-123");
    }

    #[test]
    fn test_update_response() {
        let response = build_update_response("listener", "api-gw", "l-456");
        assert_eq!(response["ok"], true);
        assert_eq!(response["ref"]["type"], "listener");
        assert_eq!(response["ref"]["name"], "api-gw");
        assert_eq!(response["ref"]["id"], "l-456");
    }

    #[test]
    fn test_delete_response() {
        let response = build_delete_response();
        assert_eq!(response, json!({"ok": true}));

        // Verify minimal token budget
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.len() < 15);
    }

    #[test]
    fn test_create_response_token_budget() {
        let response = build_create_response("cluster", "my-service", "c-12345");
        let serialized = serde_json::to_string(&response).unwrap();

        // Budget: 30-50 tokens = ~90-150 chars
        assert!(serialized.len() < 100);
    }

    // Tests for build_rich_create_response
    #[test]
    fn test_rich_create_response_minimal() {
        let response = build_rich_create_response("cluster", "svc1", "c-1", None, None, None);
        assert_eq!(response["ok"], true);
        assert_eq!(response["ref"]["type"], "cluster");
        assert_eq!(response["ref"]["name"], "svc1");
        assert!(response.get("details").is_none());
        assert!(response.get("created").is_none());
        assert!(response.get("next_step").is_none());
    }

    #[test]
    fn test_rich_create_response_full() {
        let response = build_rich_create_response(
            "route_config",
            "api-routes",
            "rc-123",
            None,
            Some(json!({"virtual_hosts": 1, "routes": 3})),
            Some("Create a listener with cp_create_listener"),
        );
        assert_eq!(response["ok"], true);
        assert_eq!(response["ref"]["name"], "api-routes");
        assert_eq!(response["created"]["virtual_hosts"], 1);
        assert_eq!(response["created"]["routes"], 3);
        assert_eq!(response["next_step"], "Create a listener with cp_create_listener");
    }

    #[test]
    fn test_rich_create_response_with_details() {
        let response = build_rich_create_response(
            "cluster",
            "orders-svc",
            "c-456",
            Some(json!({"endpoints": 2, "lb_policy": "ROUND_ROBIN"})),
            None,
            Some("Create route_config referencing cluster 'orders-svc'"),
        );
        assert_eq!(response["details"]["endpoints"], 2);
        assert_eq!(response["details"]["lb_policy"], "ROUND_ROBIN");
        assert!(response.get("created").is_none());
    }

    #[test]
    fn test_rich_create_response_token_budget() {
        let response = build_rich_create_response(
            "route_config",
            "api-routes",
            "rc-123",
            None,
            Some(json!({"virtual_hosts": 1, "routes": 3})),
            Some("Create a listener with cp_create_listener using routeConfigName: 'api-routes'"),
        );
        let serialized = serde_json::to_string(&response).unwrap();
        // Budget: 50-80 tokens = ~150-250 chars
        assert!(serialized.len() < 300);
    }

    // Tests for build_rich_delete_response
    #[test]
    fn test_rich_delete_response_basic() {
        let response = build_rich_delete_response("cluster", "orders-svc", None);
        assert_eq!(response["ok"], true);
        assert_eq!(response["deleted"]["type"], "cluster");
        assert_eq!(response["deleted"]["name"], "orders-svc");
        assert!(response.get("cascade").is_none());
    }

    #[test]
    fn test_rich_delete_response_with_cascade() {
        let response = build_rich_delete_response(
            "route_config",
            "api-routes",
            Some(json!({"virtual_hosts": 2, "routes": 5})),
        );
        assert_eq!(response["deleted"]["name"], "api-routes");
        assert_eq!(response["cascade"]["virtual_hosts"], 2);
        assert_eq!(response["cascade"]["routes"], 5);
    }
}
