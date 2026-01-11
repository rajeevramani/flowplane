use std::collections::{HashMap, HashSet};

use openapiv3::{OpenAPI, Operation, ReferenceOr, Server};
use serde::{Deserialize, Serialize};
use serde_json::{self, Value as JsonValue};
use utoipa::ToSchema;

use crate::{
    errors::Error,
    storage::{CreateClusterRequest, CreateListenerRequest, CreateRouteConfigRepositoryRequest},
    utils::VALID_NAME_REGEX,
    xds::{
        filters::http::HttpFilterConfigEntry,
        listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig},
        route::{
            HeaderMatchConfig, PathMatch, RouteActionConfig, RouteConfig as XdsRouteConfig,
            RouteMatchConfig, RouteRule, VirtualHostConfig,
        },
    },
};

const EXTENSION_GLOBAL_FILTERS: &str = "x-flowplane-filters";

pub mod defaults;

/// Listener mode for import operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ListenerMode {
    /// Use an existing listener by name
    Existing { name: String },
    /// Create a new listener with the given configuration
    New { name: String, address: String, port: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayOptions {
    pub name: String,
    pub protocol: String,
    pub listener_mode: ListenerMode,
}

/// Metadata extracted from an OpenAPI operation for a route
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteMetadataEntry {
    /// Route name (used to link metadata to routes after sync)
    pub route_name: String,
    /// OpenAPI operationId
    pub operation_id: Option<String>,
    /// Short summary of the operation
    pub summary: Option<String>,
    /// Full description of the operation
    pub description: Option<String>,
    /// Tags from OpenAPI spec
    pub tags: Option<Vec<String>>,
    /// HTTP method (GET, POST, etc.)
    pub http_method: String,
    /// Request body JSON Schema
    pub request_body_schema: Option<serde_json::Value>,
    /// Response schemas keyed by status code
    pub response_schemas: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct GatewayPlan {
    pub cluster_requests: Vec<CreateClusterRequest>,
    pub route_request: Option<CreateRouteConfigRepositoryRequest>,
    pub listener_request: Option<CreateListenerRequest>,
    pub default_virtual_host: Option<VirtualHostConfig>,
    pub summary: GatewaySummary,
    /// Metadata entries for each route, keyed by route name
    pub route_metadata: HashMap<String, RouteMetadataEntry>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GatewaySummary {
    pub gateway: String,
    pub route_config: String,
    pub listener: String,
    pub clusters: Vec<String>,
    pub shared_listener: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("Invalid gateway name '{0}'")]
    InvalidGatewayName(String),
    #[error("OpenAPI document has no servers defined")]
    MissingServers,
    #[error("Failed to parse OpenAPI document: {0}")]
    InvalidSpec(String),
    #[error("Unsupported server URL '{0}'")]
    UnsupportedServer(String),
    #[error("No routes could be generated from the OpenAPI document")]
    NoRoutes,
    #[error("Failed to parse Flowplane filters: {0}")]
    InvalidFilters(String),
}

pub fn build_gateway_plan(
    openapi: OpenAPI,
    options: GatewayOptions,
) -> Result<GatewayPlan, GatewayError> {
    if !VALID_NAME_REGEX.is_match(&options.name) {
        return Err(GatewayError::InvalidGatewayName(options.name));
    }

    // Determine if we're using an existing listener or creating a new one
    let is_existing_listener = matches!(options.listener_mode, ListenerMode::Existing { .. });

    let (listener_name, bind_address, port) = match &options.listener_mode {
        ListenerMode::Existing { name } => (name.clone(), "0.0.0.0".to_string(), 10000u16),
        ListenerMode::New { name, address, port } => (name.clone(), address.clone(), *port),
    };

    let route_name = format!("{}-routes", &options.name);
    let virtual_host_name = format!("{}-vh", &options.name);

    let servers = if openapi.servers.is_empty() { Vec::new() } else { openapi.servers.clone() };

    if servers.is_empty() {
        return Err(GatewayError::MissingServers);
    }

    let primary_server = &servers[0];
    let cluster_info = cluster_from_server(primary_server, &options.name, &options.name)?;
    let cluster_requests = vec![cluster_info.request.clone()];
    let mut domains: HashSet<String> = HashSet::new();
    if !is_existing_listener {
        domains.insert("*".to_string());
    }
    if !cluster_info.domain.is_empty() {
        domains.insert(cluster_info.domain.clone());
    }

    let global_filters = parse_global_filters(&openapi)?;

    let mut route_rules: Vec<RouteRule> = Vec::new();
    let mut route_metadata: HashMap<String, RouteMetadataEntry> = HashMap::new();

    for (path_template, item) in openapi.paths.paths.iter() {
        let path_item = match item {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { reference } => {
                return Err(GatewayError::UnsupportedServer(reference.clone()))
            }
        };

        let effective_path = combine_base_path(primary_server, path_template);

        // Extract HTTP operations from the path item with their Operation objects
        let operations: [(&str, Option<&Operation>); 8] = [
            ("GET", path_item.get.as_ref()),
            ("POST", path_item.post.as_ref()),
            ("PUT", path_item.put.as_ref()),
            ("DELETE", path_item.delete.as_ref()),
            ("PATCH", path_item.patch.as_ref()),
            ("HEAD", path_item.head.as_ref()),
            ("OPTIONS", path_item.options.as_ref()),
            ("TRACE", path_item.trace.as_ref()),
        ];

        // Create a route for each HTTP method defined in the OpenAPI spec
        for (method, maybe_operation) in operations {
            if let Some(operation) = maybe_operation {
                // Include HTTP method in route name for uniqueness
                let route_name = route_name_for_path_method(&options.name, &effective_path, method);

                // Extract metadata from the OpenAPI operation
                let metadata_entry = extract_operation_metadata(
                    &route_name,
                    &effective_path,
                    method,
                    operation,
                    &openapi,
                );
                route_metadata.insert(route_name.clone(), metadata_entry);

                // Create :method header matcher for HTTP method matching
                let headers = Some(vec![HeaderMatchConfig {
                    name: ":method".to_string(),
                    value: Some(method.to_string()),
                    regex: None,
                    present: None,
                }]);

                let route_rule = RouteRule {
                    name: Some(route_name),
                    r#match: RouteMatchConfig {
                        path: PathMatch::Template(effective_path.clone()),
                        headers,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: cluster_info.request.name.clone(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                        retry_policy: None,
                    },
                    typed_per_filter_config: Default::default(),
                };

                route_rules.push(route_rule);
            }
        }
    }

    if route_rules.is_empty() {
        return Err(GatewayError::NoRoutes);
    }

    let mut domains_vec: Vec<String> =
        if domains.is_empty() { vec!["*".to_string()] } else { domains.into_iter().collect() };

    if is_existing_listener {
        domains_vec.retain(|domain| domain != "*");
        if domains_vec.is_empty() {
            domains_vec.push(cluster_info.domain.clone());
        }
    }

    let virtual_host = VirtualHostConfig {
        name: virtual_host_name,
        domains: domains_vec,
        routes: route_rules,
        typed_per_filter_config: Default::default(),
    };

    let summary = GatewaySummary {
        gateway: options.name.clone(),
        listener: listener_name.clone(),
        route_config: route_name.clone(),
        clusters: cluster_requests.iter().map(|request| request.name.clone()).collect(),
        shared_listener: is_existing_listener,
    };

    let default_cluster_name =
        summary.clusters.first().cloned().unwrap_or_else(|| cluster_info.request.name.clone());

    if is_existing_listener {
        // When using an existing listener, we return the virtual host to be merged
        // into the listener's route config
        Ok(GatewayPlan {
            cluster_requests,
            route_request: None,
            listener_request: None,
            default_virtual_host: Some(virtual_host),
            summary,
            route_metadata,
        })
    } else {
        // When creating a new listener, we create both the route config and listener
        let route_config =
            XdsRouteConfig { name: route_name.clone(), virtual_hosts: vec![virtual_host] };

        let mut route_config_value = serde_json::to_value(&route_config)
            .map_err(|err| GatewayError::InvalidSpec(err.to_string()))?;
        attach_gateway_tag(&mut route_config_value, &options.name);

        let route_request = CreateRouteConfigRepositoryRequest {
            name: route_name,
            path_prefix: "/".to_string(),
            cluster_name: default_cluster_name,
            configuration: route_config_value,
            team: None, // OpenAPI Gateway routes are not team-scoped by default
            import_id: None,
            route_order: None,
            headers: None,
        };

        let listener_config = ListenerConfig {
            name: listener_name.clone(),
            address: bind_address.clone(),
            port: port as u32,
            filter_chains: vec![FilterChainConfig {
                name: Some(format!("{}-chain", options.name)),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: Some(route_request.name.clone()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: global_filters,
                    },
                }],
                tls_context: None,
            }],
        };

        let mut listener_config_value = serde_json::to_value(&listener_config)
            .map_err(|err| GatewayError::InvalidSpec(err.to_string()))?;
        attach_gateway_tag(&mut listener_config_value, &options.name);

        let listener_request = CreateListenerRequest {
            name: listener_name,
            address: bind_address,
            port: Some(port as i64),
            protocol: Some(options.protocol),
            configuration: listener_config_value,
            team: None, // OpenAPI Gateway listeners are not team-scoped by default
            import_id: None,
        };

        Ok(GatewayPlan {
            cluster_requests,
            route_request: Some(route_request),
            listener_request: Some(listener_request),
            default_virtual_host: None,
            summary,
            route_metadata,
        })
    }
}

pub fn parse_global_filters(openapi: &OpenAPI) -> Result<Vec<HttpFilterConfigEntry>, GatewayError> {
    if let Some(value) = openapi.extensions.get(EXTENSION_GLOBAL_FILTERS) {
        serde_json::from_value::<Vec<HttpFilterConfigEntry>>(value.clone()).map_err(|err| {
            GatewayError::InvalidFilters(format!(
                "Failed to parse {}: {}",
                EXTENSION_GLOBAL_FILTERS, err
            ))
        })
    } else {
        Ok(Vec::new())
    }
}

/// Extract metadata from an OpenAPI operation for MCP tool generation
pub fn extract_operation_metadata(
    route_name: &str,
    _path: &str,
    method: &str,
    operation: &Operation,
    openapi: &OpenAPI,
) -> RouteMetadataEntry {
    RouteMetadataEntry {
        route_name: route_name.to_string(),
        operation_id: operation.operation_id.clone(),
        summary: operation.summary.clone(),
        description: operation.description.clone(),
        tags: if operation.tags.is_empty() { None } else { Some(operation.tags.clone()) },
        http_method: method.to_uppercase(),
        request_body_schema: extract_request_body_schema(operation, openapi),
        response_schemas: extract_response_schemas(operation, openapi),
    }
}

/// Extract request body JSON Schema from OpenAPI requestBody
fn extract_request_body_schema(
    operation: &Operation,
    openapi: &OpenAPI,
) -> Option<serde_json::Value> {
    let request_body = operation.request_body.as_ref()?;

    // Resolve reference if needed
    let resolved_body = match request_body {
        ReferenceOr::Reference { reference } => {
            // Try to resolve from components
            let ref_name = reference.strip_prefix("#/components/requestBodies/")?;
            openapi.components.as_ref()?.request_bodies.get(ref_name)?.as_item()
        }
        ReferenceOr::Item(item) => Some(item),
    }?;

    // Get JSON content type schema
    let media_type = resolved_body
        .content
        .get("application/json")
        .or_else(|| resolved_body.content.get("*/*"))
        .or_else(|| resolved_body.content.values().next())?;

    let schema = media_type.schema.as_ref()?;

    // Convert OpenAPI schema to JSON value
    resolve_schema_to_json(schema, openapi)
}

/// Extract response schemas keyed by status code
fn extract_response_schemas(operation: &Operation, openapi: &OpenAPI) -> Option<serde_json::Value> {
    let mut schemas: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

    for (status_code, response_ref) in operation.responses.responses.iter() {
        let status_key = match status_code {
            openapiv3::StatusCode::Code(code) => code.to_string(),
            openapiv3::StatusCode::Range(range) => format!("{}XX", range),
        };

        let response = match response_ref {
            ReferenceOr::Reference { reference } => {
                // Try to resolve from components
                if let Some(ref_name) = reference.strip_prefix("#/components/responses/") {
                    openapi
                        .components
                        .as_ref()
                        .and_then(|c| c.responses.get(ref_name))
                        .and_then(|r| r.as_item())
                } else {
                    None
                }
            }
            ReferenceOr::Item(item) => Some(item),
        };

        if let Some(resp) = response {
            // Get JSON content type schema
            if let Some(media_type) = resp
                .content
                .get("application/json")
                .or_else(|| resp.content.get("*/*"))
                .or_else(|| resp.content.values().next())
            {
                if let Some(schema) = media_type.schema.as_ref() {
                    if let Some(json_schema) = resolve_schema_to_json(schema, openapi) {
                        schemas.insert(status_key, json_schema);
                    }
                }
            }
        }
    }

    // Also check default response
    if let Some(default_ref) = &operation.responses.default {
        let response = match default_ref {
            ReferenceOr::Reference { reference } => {
                if let Some(ref_name) = reference.strip_prefix("#/components/responses/") {
                    openapi
                        .components
                        .as_ref()
                        .and_then(|c| c.responses.get(ref_name))
                        .and_then(|r| r.as_item())
                } else {
                    None
                }
            }
            ReferenceOr::Item(item) => Some(item),
        };

        if let Some(resp) = response {
            if let Some(media_type) = resp.content.get("application/json") {
                if let Some(schema) = media_type.schema.as_ref() {
                    if let Some(json_schema) = resolve_schema_to_json(schema, openapi) {
                        schemas.insert("default".to_string(), json_schema);
                    }
                }
            }
        }
    }

    if schemas.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(schemas))
    }
}

/// Resolve an OpenAPI schema reference to a JSON value
fn resolve_schema_to_json(
    schema_ref: &ReferenceOr<openapiv3::Schema>,
    _openapi: &OpenAPI, // Kept for future full resolution support
) -> Option<serde_json::Value> {
    match schema_ref {
        ReferenceOr::Reference { reference } => {
            // For references, just store the reference string and resolve later if needed
            // This avoids infinite recursion for circular references
            Some(serde_json::json!({
                "$ref": reference
            }))
        }
        ReferenceOr::Item(schema) => {
            // Convert the schema to JSON
            serde_json::to_value(schema).ok()
        }
    }
}

struct ClusterInfo {
    request: CreateClusterRequest,
    domain: String,
}

fn cluster_from_server(
    server: &Server,
    prefix: &str,
    gateway_tag: &str,
) -> Result<ClusterInfo, GatewayError> {
    let url = url::Url::parse(&server.url)
        .map_err(|err| GatewayError::UnsupportedServer(format!("{} ({})", server.url, err)))?;

    let host = url.host_str().ok_or_else(|| {
        GatewayError::UnsupportedServer(format!(
            "Server URL '{}' does not contain a host",
            server.url
        ))
    })?;

    let port = url.port_or_known_default().ok_or_else(|| {
        GatewayError::UnsupportedServer(format!(
            "Server URL '{}' does not include a usable port",
            server.url
        ))
    })?;

    let use_tls = matches!(url.scheme(), "https" | "grpcs");

    let cluster_name = sanitize_name(&format!("{}-{}", prefix, host));

    let spec = crate::xds::ClusterSpec {
        connect_timeout_seconds: Some(5),
        endpoints: vec![crate::xds::EndpointSpec::Address { host: host.to_string(), port }],
        use_tls: Some(use_tls),
        tls_server_name: if use_tls { Some(host.to_string()) } else { None },
        dns_lookup_family: None,
        lb_policy: None,
        least_request: None,
        ring_hash: None,
        maglev: None,
        circuit_breakers: None,
        health_checks: Vec::new(),
        outlier_detection: None,
        protocol_type: None,
    };

    let mut configuration =
        spec.to_value().map_err(|err| GatewayError::InvalidSpec(err.to_string()))?;
    attach_gateway_tag(&mut configuration, gateway_tag);

    let request = CreateClusterRequest {
        name: cluster_name.clone(),
        service_name: cluster_name.clone(),
        configuration,
        team: None, // OpenAPI Gateway clusters are not team-scoped by default
        import_id: None,
    };

    let domain = host.to_string();

    Ok(ClusterInfo { request, domain })
}

fn combine_base_path(server: &Server, template: &str) -> String {
    let base_path = url::Url::parse(&server.url)
        .map(|url| url.path().trim_matches('/').to_string())
        .unwrap_or_else(|_| String::new());

    let template_path = template.trim_matches('/');

    let mut segments: Vec<String> = Vec::new();

    if !base_path.is_empty() {
        segments.extend(
            base_path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string()),
        );
    }

    if !template_path.is_empty() {
        segments.extend(
            template_path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string()),
        );
    }

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

/// Generate a route name including the HTTP method for uniqueness
fn route_name_for_path_method(prefix: &str, path: &str, method: &str) -> String {
    let slug = path.trim_matches('/').replace('/', "_").replace(['{', '}'], "");
    let slug = if slug.is_empty() { "root".to_string() } else { slug };
    sanitize_name(&format!("{}-{}-{}", prefix, slug, method))
}

fn sanitize_name(raw: &str) -> String {
    let mut name: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();

    if name.is_empty() {
        name.push_str("resource");
    }

    // Ensure first character is alphabetic or underscore
    if let Some(first_char) = name.chars().next() {
        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            name.insert(0, '_');
        }
    } else {
        // Defensive: should not happen since we handle empty strings above
        name.push_str("resource");
    }

    if name.len() > 48 {
        name.truncate(48);
    }

    name
}

impl From<GatewayError> for Error {
    fn from(value: GatewayError) -> Self {
        Error::validation(value.to_string())
    }
}

pub(crate) fn attach_gateway_tag(value: &mut JsonValue, gateway: &str) {
    match value {
        JsonValue::Object(map) => {
            if !is_enum_wrapper(map) {
                map.entry("flowplaneGateway".to_string())
                    .or_insert_with(|| JsonValue::String(gateway.to_string()));
            }

            for (key, child) in map.iter_mut() {
                if key == "flowplaneGateway" || key == "typed_per_filter_config" {
                    continue;
                }
                attach_gateway_tag(child, gateway);
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                attach_gateway_tag(item, gateway);
            }
        }
        _ => {}
    }
}

pub(crate) fn strip_gateway_tags(value: &mut JsonValue) {
    match value {
        JsonValue::Object(map) => {
            map.remove("flowplaneGateway");
            for child in map.values_mut() {
                strip_gateway_tags(child);
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                strip_gateway_tags(item);
            }
        }
        _ => {}
    }
}

fn is_enum_wrapper(map: &serde_json::Map<String, JsonValue>) -> bool {
    if map.len() != 1 {
        return false;
    }

    map.keys().all(|key| key.chars().next().map(|c| c.is_uppercase()).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_gateway_plan_from_basic_openapi() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            protocol: "HTTP".to_string(),
            listener_mode: ListenerMode::New {
                name: "example-listener".to_string(),
                address: "0.0.0.0".to_string(),
                port: 10000,
            },
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        assert_eq!(plan.summary.gateway, "example");
        assert_eq!(plan.summary.route_config, "example-routes");
        assert_eq!(plan.summary.listener, "example-listener");
        assert!(!plan.summary.shared_listener);
        assert_eq!(plan.summary.clusters.len(), 1);
        assert_eq!(plan.cluster_requests.len(), 1);
        let route_request = plan.route_request.as_ref().expect("route request should exist");
        assert_eq!(route_request.name, "example-routes");
        let listener_request =
            plan.listener_request.as_ref().expect("listener request should exist");
        assert_eq!(listener_request.name, "example-listener");
        assert!(plan.default_virtual_host.is_none());

        fn expect_tag(value: &JsonValue, gateway: &str) {
            if let Some(map) = value.as_object() {
                if let Some(tag) = map.get("flowplaneGateway").and_then(|value| value.as_str()) {
                    assert_eq!(tag, gateway);
                    return;
                }

                if super::is_enum_wrapper(map) {
                    if let Some(child) = map.values().next() {
                        expect_tag(child, gateway);
                        return;
                    }
                }
            }

            panic!(
                "Expected flowplaneGateway tag '{}' but it was missing in value: {}",
                gateway, value
            );
        }

        let cluster_config = &plan.cluster_requests[0].configuration;
        expect_tag(cluster_config, "example");
        if let Some(endpoint) = cluster_config
            .get("endpoints")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
        {
            expect_tag(endpoint, "example");
        }

        let route_config = &route_request.configuration;
        expect_tag(route_config, "example");
        if let Some(virtual_host) = route_config
            .get("virtual_hosts")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
        {
            expect_tag(virtual_host, "example");
            if let Some(route) = virtual_host
                .get("routes")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
            {
                expect_tag(route, "example");
            }
        }

        let listener_config = &listener_request.configuration;
        expect_tag(listener_config, "example");
        if let Some(filter_chain) = listener_config
            .get("filter_chains")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
        {
            expect_tag(filter_chain, "example");
            if let Some(filter) = filter_chain
                .get("filters")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
            {
                expect_tag(filter, "example");
                if let Some(filter_type) = filter.get("filter_type") {
                    expect_tag(filter_type, "example");
                }
            }
        }
    }

    #[test]
    fn builds_shared_gateway_plan_from_basic_openapi() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            protocol: "HTTP".to_string(),
            listener_mode: ListenerMode::Existing {
                name: defaults::DEFAULT_GATEWAY_LISTENER.to_string(),
            },
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        assert!(plan.route_request.is_none());
        assert!(plan.listener_request.is_none());
        assert!(plan.default_virtual_host.is_some());
        assert!(plan.summary.shared_listener); // true when using existing listener
        assert_eq!(plan.summary.listener, defaults::DEFAULT_GATEWAY_LISTENER);
        assert_eq!(plan.summary.route_config, "example-routes"); // Now uses gateway-specific route name
    }

    #[test]
    fn extracts_multiple_http_methods_per_path() {
        // OpenAPI spec with multiple methods on same path
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {"responses": {"200": {"description": "List users"}}},
                        "post": {"responses": {"201": {"description": "Create user"}}}
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            protocol: "HTTP".to_string(),
            listener_mode: ListenerMode::New {
                name: "example-listener".to_string(),
                address: "0.0.0.0".to_string(),
                port: 10000,
            },
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        // Should create 2 routes: one for GET, one for POST
        let route_request = plan.route_request.as_ref().expect("route request");
        let route_config: XdsRouteConfig =
            serde_json::from_value(route_request.configuration.clone())
                .expect("parse route config");

        assert_eq!(route_config.virtual_hosts.len(), 1);
        let virtual_host = &route_config.virtual_hosts[0];

        // 2 routes for 2 HTTP methods
        assert_eq!(virtual_host.routes.len(), 2, "Expected 2 routes (GET, POST)");

        // Verify route names include HTTP method
        let route_names: Vec<&str> =
            virtual_host.routes.iter().filter_map(|r| r.name.as_deref()).collect();
        assert!(
            route_names.iter().any(|name| name.contains("GET")),
            "Expected a route with GET in name"
        );
        assert!(
            route_names.iter().any(|name| name.contains("POST")),
            "Expected a route with POST in name"
        );

        // Verify each route has :method header matcher
        for route in &virtual_host.routes {
            let headers = route.r#match.headers.as_ref().expect("route should have headers");
            assert_eq!(headers.len(), 1, "Expected exactly 1 header matcher");
            assert_eq!(headers[0].name, ":method", "Expected :method header");
            assert!(headers[0].value.is_some(), "Expected header value for method");
        }
    }

    #[test]
    fn extracts_all_http_method_types() {
        // OpenAPI spec with all supported HTTP methods
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/resource": {
                        "get": {"responses": {"200": {"description": "Get"}}},
                        "post": {"responses": {"201": {"description": "Post"}}},
                        "put": {"responses": {"200": {"description": "Put"}}},
                        "delete": {"responses": {"204": {"description": "Delete"}}},
                        "patch": {"responses": {"200": {"description": "Patch"}}},
                        "head": {"responses": {"200": {"description": "Head"}}},
                        "options": {"responses": {"200": {"description": "Options"}}}
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            protocol: "HTTP".to_string(),
            listener_mode: ListenerMode::New {
                name: "example-listener".to_string(),
                address: "0.0.0.0".to_string(),
                port: 10000,
            },
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        let route_request = plan.route_request.as_ref().expect("route request");
        let route_config: XdsRouteConfig =
            serde_json::from_value(route_request.configuration.clone())
                .expect("parse route config");

        let virtual_host = &route_config.virtual_hosts[0];

        // 7 routes for 7 HTTP methods (TRACE not in spec)
        assert_eq!(virtual_host.routes.len(), 7, "Expected 7 routes for all methods");

        // Collect all methods from header matchers
        let methods: Vec<String> = virtual_host
            .routes
            .iter()
            .filter_map(|r| {
                r.r#match
                    .headers
                    .as_ref()
                    .and_then(|h| h.first())
                    .and_then(|header| header.value.clone())
            })
            .collect();

        assert!(methods.contains(&"GET".to_string()), "Missing GET method");
        assert!(methods.contains(&"POST".to_string()), "Missing POST method");
        assert!(methods.contains(&"PUT".to_string()), "Missing PUT method");
        assert!(methods.contains(&"DELETE".to_string()), "Missing DELETE method");
        assert!(methods.contains(&"PATCH".to_string()), "Missing PATCH method");
        assert!(methods.contains(&"HEAD".to_string()), "Missing HEAD method");
        assert!(methods.contains(&"OPTIONS".to_string()), "Missing OPTIONS method");
    }

    #[test]
    fn handles_multiple_paths_with_multiple_methods() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {"responses": {"200": {"description": "List"}}},
                        "post": {"responses": {"201": {"description": "Create"}}}
                    },
                    "/users/{id}": {
                        "get": {"responses": {"200": {"description": "Get one"}}},
                        "put": {"responses": {"200": {"description": "Update"}}},
                        "delete": {"responses": {"204": {"description": "Delete"}}}
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            protocol: "HTTP".to_string(),
            listener_mode: ListenerMode::New {
                name: "example-listener".to_string(),
                address: "0.0.0.0".to_string(),
                port: 10000,
            },
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        let route_request = plan.route_request.as_ref().expect("route request");
        let route_config: XdsRouteConfig =
            serde_json::from_value(route_request.configuration.clone())
                .expect("parse route config");

        let virtual_host = &route_config.virtual_hosts[0];

        // 5 total routes: 2 for /users (GET, POST) + 3 for /users/{id} (GET, PUT, DELETE)
        assert_eq!(virtual_host.routes.len(), 5, "Expected 5 routes total");

        // Verify all routes have unique names
        let route_names: Vec<&str> =
            virtual_host.routes.iter().filter_map(|r| r.name.as_deref()).collect();
        let unique_names: std::collections::HashSet<&str> = route_names.iter().copied().collect();
        assert_eq!(route_names.len(), unique_names.len(), "Route names should be unique");
    }

    #[test]
    fn method_header_matcher_uses_correct_format() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/test": {
                        "get": {"responses": {"200": {"description": "OK"}}}
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            protocol: "HTTP".to_string(),
            listener_mode: ListenerMode::New {
                name: "example-listener".to_string(),
                address: "0.0.0.0".to_string(),
                port: 10000,
            },
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        let route_request = plan.route_request.as_ref().expect("route request");
        let route_config: XdsRouteConfig =
            serde_json::from_value(route_request.configuration.clone())
                .expect("parse route config");

        let route = &route_config.virtual_hosts[0].routes[0];
        let headers = route.r#match.headers.as_ref().expect("headers");

        // Verify the header matcher format
        assert_eq!(headers.len(), 1);
        let header = &headers[0];
        assert_eq!(header.name, ":method", "Should use :method pseudo-header");
        assert_eq!(header.value, Some("GET".to_string()), "Should have exact method value");
        assert!(header.regex.is_none(), "Should not use regex");
        assert!(header.present.is_none(), "Should not use present match");
    }
}
