//! API definition domain types
//!
//! This module contains pure domain entities for API definitions.
//! These types are independent of any infrastructure concerns and
//! can be used across different API layers.

use serde_json::Value;

/// High-level specification for creating a Platform API definition.
///
/// This type represents the complete configuration for an API definition
/// including team ownership, domain routing, isolation settings, and routes.
#[derive(Debug, Clone)]
pub struct ApiDefinitionSpec {
    /// Team or organization owning this API
    pub team: String,

    /// Domain name for routing (e.g., "api.example.com")
    pub domain: String,

    /// Whether this API requires listener isolation
    pub listener_isolation: bool,

    /// Optional isolated listener configuration
    pub isolation_listener: Option<ListenerConfig>,

    /// Target listener names (only used when listener_isolation is false)
    /// If None, defaults to "default-gateway-listener"
    pub target_listeners: Option<Vec<String>>,

    /// Optional TLS configuration
    pub tls_config: Option<Value>,

    /// Routes associated with this API definition
    pub routes: Vec<RouteConfig>,
}

/// Listener configuration for domain entities.
///
/// This represents the network binding configuration for a listener,
/// independent of any specific protocol or infrastructure implementation.
#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// Optional listener name (auto-generated if not provided)
    pub name: Option<String>,

    /// Bind address (e.g., "0.0.0.0", "::")
    pub bind_address: String,

    /// Port number
    pub port: u32,

    /// Protocol (e.g., "HTTP", "HTTPS")
    pub protocol: String,

    /// Optional TLS configuration
    pub tls_config: Option<Value>,
}

/// Route configuration for domain entities.
///
/// This represents a single route's matching and transformation logic,
/// independent of any specific routing implementation.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    /// Type of match (e.g., "prefix", "exact", "regex")
    pub match_type: String,

    /// Value to match against
    pub match_value: String,

    /// Whether matching is case-sensitive
    pub case_sensitive: bool,

    /// Optional prefix rewrite
    pub rewrite_prefix: Option<String>,

    /// Optional regex pattern for rewriting
    pub rewrite_regex: Option<String>,

    /// Optional regex substitution
    pub rewrite_substitution: Option<String>,

    /// Upstream targets configuration
    pub upstream_targets: Value,

    /// Optional timeout in seconds
    pub timeout_seconds: Option<i64>,

    /// Optional override configuration
    pub override_config: Option<Value>,

    /// Optional deployment note
    pub deployment_note: Option<String>,

    /// Optional route ordering
    pub route_order: Option<i64>,
}

/// Outcome of creating an API definition.
///
/// This captures all the resources generated during API definition creation.
#[derive(Debug)]
pub struct CreateDefinitionOutcome<TDefinition, TRoute> {
    /// The created API definition
    pub definition: TDefinition,

    /// Created routes
    pub routes: Vec<TRoute>,

    /// Bootstrap URI for Envoy configuration
    pub bootstrap_uri: String,

    /// Generated listener ID (if isolation enabled)
    pub generated_listener_id: Option<String>,

    /// Generated route IDs
    pub generated_route_ids: Vec<String>,

    /// Generated cluster IDs
    pub generated_cluster_ids: Vec<String>,
}

/// Outcome of appending a route to an existing API definition.
///
/// This captures the updated definition and the newly created route.
#[derive(Debug)]
pub struct AppendRouteOutcome<TDefinition, TRoute> {
    /// The updated API definition
    pub definition: TDefinition,

    /// The newly created route
    pub route: TRoute,

    /// Bootstrap URI for Envoy configuration
    pub bootstrap_uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_definition_spec_construction() {
        let spec = ApiDefinitionSpec {
            team: "platform".to_string(),
            domain: "api.example.com".to_string(),
            listener_isolation: false,
            isolation_listener: None,
            target_listeners: None,
            tls_config: None,
            routes: vec![],
        };

        assert_eq!(spec.team, "platform");
        assert_eq!(spec.domain, "api.example.com");
        assert!(!spec.listener_isolation);
        assert!(spec.routes.is_empty());
    }

    #[test]
    fn listener_config_with_tls() {
        let listener = ListenerConfig {
            name: Some("api-listener".to_string()),
            bind_address: "0.0.0.0".to_string(),
            port: 8443,
            protocol: "HTTPS".to_string(),
            tls_config: Some(serde_json::json!({
                "certificate": "cert.pem",
                "private_key": "key.pem"
            })),
        };

        assert_eq!(listener.port, 8443);
        assert_eq!(listener.protocol, "HTTPS");
        assert!(listener.tls_config.is_some());
    }

    #[test]
    fn route_config_with_prefix_match() {
        let route = RouteConfig {
            match_type: "prefix".to_string(),
            match_value: "/api/v1".to_string(),
            case_sensitive: true,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!([{
                "host": "backend.svc.local",
                "port": 8080
            }]),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(10),
        };

        assert_eq!(route.match_type, "prefix");
        assert_eq!(route.match_value, "/api/v1");
        assert!(route.case_sensitive);
        assert_eq!(route.timeout_seconds, Some(30));
    }

    #[test]
    fn route_config_with_rewrite() {
        let route = RouteConfig {
            match_type: "prefix".to_string(),
            match_value: "/old-api".to_string(),
            case_sensitive: false,
            rewrite_prefix: Some("/new-api".to_string()),
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!([]),
            timeout_seconds: None,
            override_config: None,
            deployment_note: Some("Migration to new API".to_string()),
            route_order: None,
        };

        assert_eq!(route.rewrite_prefix, Some("/new-api".to_string()));
        assert_eq!(route.deployment_note, Some("Migration to new API".to_string()));
    }
}
