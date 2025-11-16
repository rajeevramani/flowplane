use openapiv3::{OpenAPI, PathItem, ReferenceOr};
use serde_json::json;

use crate::openapi::GatewayError;
use crate::xds::route::HeaderMatchConfig;

use super::materializer::{ApiDefinitionSpec, ListenerInput, RouteSpec};

/// Convert OpenAPI 3.x specification to Platform API definition spec.
///
/// This adapter bridges the OpenAPI import functionality with the Platform API
/// materializer, enabling OpenAPI specs to benefit from:
/// - Foreign key relationship tracking
/// - Source tagging (source='platform_api')
/// - Bootstrap config generation
/// - Unified data model
pub fn openapi_to_api_definition_spec(
    openapi: OpenAPI,
    team: String,
    listener_isolation: bool,
    port: Option<u32>,
) -> Result<ApiDefinitionSpec, GatewayError> {
    // Validate servers array is not empty
    if openapi.servers.is_empty() {
        return Err(GatewayError::MissingServers);
    }

    // Extract domain from first server URL (used as the primary domain for routing)
    let primary_server = &openapi.servers[0];
    let primary_url = url::Url::parse(&primary_server.url).map_err(|err| {
        GatewayError::UnsupportedServer(format!("{} ({})", primary_server.url, err))
    })?;

    let domain = primary_url
        .host_str()
        .ok_or_else(|| {
            GatewayError::UnsupportedServer(format!(
                "Server URL '{}' does not contain a host",
                primary_server.url
            ))
        })?
        .to_string();

    let primary_scheme = primary_url.scheme();
    let use_tls = matches!(primary_scheme, "https" | "grpcs");

    // Parse global x-flowplane-filters from OpenAPI extensions
    let global_filters = crate::openapi::parse_global_filters(&openapi)?;

    // Parse all servers into upstream targets for load balancing
    // Multiple servers will be load-balanced across using Envoy's default round-robin policy
    let mut targets = Vec::new();

    for (idx, server) in openapi.servers.iter().enumerate() {
        let server_url = url::Url::parse(&server.url)
            .map_err(|err| GatewayError::UnsupportedServer(format!("{} ({})", server.url, err)))?;

        // Validate all servers use the same scheme (http vs https)
        if server_url.scheme() != primary_scheme {
            return Err(GatewayError::UnsupportedServer(format!(
                "All servers must use the same scheme. Primary server uses '{}' but server {} uses '{}'",
                primary_scheme, idx, server_url.scheme()
            )));
        }

        let host = server_url
            .host_str()
            .ok_or_else(|| {
                GatewayError::UnsupportedServer(format!(
                    "Server URL '{}' does not contain a host",
                    server.url
                ))
            })?
            .to_string();

        let port = server_url.port_or_known_default().ok_or_else(|| {
            GatewayError::UnsupportedServer(format!(
                "Server URL '{}' does not include a usable port",
                server.url
            ))
        })?;

        // Extract optional weight from server variables (x-flowplane-weight)
        // If not specified, Envoy will use equal weights for round-robin
        let weight = server
            .variables
            .as_ref()
            .and_then(|vars| vars.get("x-flowplane-weight"))
            .and_then(|var| var.default.parse::<u32>().ok());

        let mut target = json!({
            "name": format!("{}-upstream-{}", host, idx),
            "endpoint": format!("{}:{}", host, port),
        });

        // Add weight if specified
        if let Some(w) = weight {
            target.as_object_mut().unwrap().insert("weight".to_string(), json!(w));
        }

        targets.push(target);
    }

    // Create upstream target configuration with all servers for load balancing
    let upstream_targets = json!({
        "targets": targets
    });

    // Convert OpenAPI paths and operations to RouteSpec (one route per operation)
    let mut routes = Vec::new();

    for (path_template, item) in openapi.paths.paths.iter() {
        let path_item = match item {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { reference } => {
                return Err(GatewayError::UnsupportedServer(reference.clone()))
            }
        };

        let effective_path = combine_base_path(primary_server, path_template);

        // Determine match type: use "template" for paths with parameters, otherwise "prefix"
        let match_type =
            if path_template.contains('{') { "template".to_string() } else { "prefix".to_string() };

        // Parse route-level x-flowplane-route-overrides from path item operations
        // Store the raw JSON value in override_config for typed_per_filter_config processing
        let override_config = parse_route_level_filters(path_item)?;

        // Create a route for each HTTP method (operation) present in the path
        let operations = [
            ("GET", &path_item.get),
            ("POST", &path_item.post),
            ("PUT", &path_item.put),
            ("DELETE", &path_item.delete),
            ("PATCH", &path_item.patch),
            ("HEAD", &path_item.head),
            ("OPTIONS", &path_item.options),
            ("TRACE", &path_item.trace),
        ];

        for (method, operation) in operations.iter() {
            if let Some(_op) = operation {
                // Create :method pseudo-header matcher for HTTP method matching
                let headers = Some(vec![HeaderMatchConfig {
                    name: ":method".to_string(),
                    value: Some(method.to_string()),
                    regex: None,
                    present: None,
                }]);

                let route_spec = RouteSpec {
                    match_type: match_type.clone(),
                    match_value: effective_path.clone(),
                    case_sensitive: true,
                    headers,
                    rewrite_prefix: None,
                    rewrite_regex: None,
                    rewrite_substitution: None,
                    upstream_targets: upstream_targets.clone(),
                    timeout_seconds: Some(30),
                    override_config: override_config.clone(),
                    deployment_note: Some(format!(
                        "Generated from OpenAPI {} {}",
                        method, path_template
                    )),
                    route_order: Some(routes.len() as i64),
                };

                routes.push(route_spec);
            }
        }
    }

    if routes.is_empty() {
        return Err(GatewayError::NoRoutes);
    }

    // Configure TLS if needed
    let tls_config = if use_tls {
        Some(json!({
            "enabled": true,
            "server_name": domain
        }))
    } else {
        None
    };

    // Configure listener isolation if requested
    let isolation_listener = if listener_isolation {
        // Use provided port if available, otherwise generate deterministic port
        let listener_port = if let Some(p) = port {
            p
        } else {
            // Use a deterministic port based on the domain to avoid conflicts
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            domain.hash(&mut hasher);
            // Port range 20000-29999 for isolated listeners
            20000 + (hasher.finish() % 10000) as u32
        };

        Some(ListenerInput {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: listener_port,
            protocol: if use_tls { "HTTPS".to_string() } else { "HTTP".to_string() },
            tls_config: tls_config.clone(),
            http_filters: if global_filters.is_empty() {
                None
            } else {
                Some(global_filters.clone())
            },
        })
    } else {
        None
    };

    Ok(ApiDefinitionSpec {
        team,
        domain,
        listener_isolation,
        isolation_listener,
        target_listeners: None,
        tls_config,
        routes,
    })
}

/// Parse route-level x-flowplane-route-overrides from path item operations.
///
/// Checks operations in priority order (GET, POST, PUT, DELETE, PATCH, etc.)
/// and returns the raw filter override object from the first operation that has it.
/// Returns None if no operations have x-flowplane-route-overrides extensions.
///
/// The returned JSON value is stored in RouteSpec.override_config and passed to
/// the materializer's typed_per_filter_config function for processing. It should be
/// an object with filter aliases as keys (e.g., {"authn": "disabled", "cors": {...}}).
fn parse_route_level_filters(
    path_item: &PathItem,
) -> Result<Option<serde_json::Value>, GatewayError> {
    const EXTENSION_ROUTE_FILTERS: &str = "x-flowplane-route-overrides";

    // Check operations in priority order
    let operations = [
        &path_item.get,
        &path_item.post,
        &path_item.put,
        &path_item.delete,
        &path_item.patch,
        &path_item.head,
        &path_item.options,
        &path_item.trace,
    ];

    for operation in operations.iter().copied().flatten() {
        if let Some(value) = operation.extensions.get(EXTENSION_ROUTE_FILTERS) {
            // Return the raw JSON value - it will be processed by typed_per_filter_config
            return Ok(Some(value.clone()));
        }
    }

    Ok(None)
}

fn combine_base_path(server: &openapiv3::Server, template: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_basic_openapi_to_api_definition_spec() {
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
                    },
                    "/posts": {
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

        let spec = openapi_to_api_definition_spec(doc, "platform-team".to_string(), false, None)
            .expect("convert spec");

        assert_eq!(spec.team, "platform-team");
        assert_eq!(spec.domain, "api.example.com");
        assert!(!spec.listener_isolation);
        assert!(spec.isolation_listener.is_none());
        assert!(spec.tls_config.is_some());
        assert_eq!(spec.routes.len(), 2);

        let first_route = &spec.routes[0];
        assert_eq!(first_route.match_type, "prefix");
        assert!(first_route.match_value == "/users" || first_route.match_value == "/posts");
        assert!(first_route.upstream_targets.get("targets").is_some());
    }

    #[test]
    fn converts_openapi_with_listener_isolation() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "http://api.example.com:8080"}],
                "paths": {
                    "/health": {
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

        let spec = openapi_to_api_definition_spec(doc, "isolated-team".to_string(), true, None)
            .expect("convert spec");

        assert!(spec.listener_isolation);
        assert!(spec.isolation_listener.is_some());
        assert!(spec.tls_config.is_none()); // HTTP not HTTPS

        let listener = spec.isolation_listener.unwrap();
        assert!(
            listener.port >= 20000 && listener.port < 30000,
            "Port should be in 20000-29999 range"
        );
        assert_eq!(listener.bind_address, "0.0.0.0");
        assert_eq!(listener.protocol, "HTTP");
        assert!(listener.tls_config.is_none());
    }

    #[test]
    fn rejects_openapi_without_servers() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [],
                "paths": {
                    "/test": {
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

        let result = openapi_to_api_definition_spec(doc, "test-team".to_string(), false, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GatewayError::MissingServers));
    }

    #[test]
    fn extracts_route_level_xflowplane_filters() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {
                            "x-flowplane-route-overrides": {
                                "authn": "disabled"
                            },
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), false, None)
            .expect("convert spec");

        assert_eq!(spec.routes.len(), 1);
        let route = &spec.routes[0];
        assert!(route.override_config.is_some());

        // Verify the override_config contains the filter overrides
        let override_config = route.override_config.as_ref().unwrap();
        assert!(override_config.is_object());
        let overrides = override_config.as_object().unwrap();
        assert!(overrides.contains_key("authn"));
    }

    #[test]
    fn extracts_global_xflowplane_filters_with_listener_isolation() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "x-flowplane-filters": [
                    {
                        "filter": {
                            "type": "cors",
                            "policy": {
                                "allow_origin": [{"type": "exact", "value": "*"}],
                                "allow_methods": ["GET", "POST"]
                            }
                        }
                    }
                ],
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

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), true, None)
            .expect("convert spec");

        assert!(spec.listener_isolation);
        assert!(spec.isolation_listener.is_some());

        let listener = spec.isolation_listener.unwrap();
        assert!(listener.http_filters.is_some());

        let filters = listener.http_filters.unwrap();
        assert_eq!(filters.len(), 1);
    }

    #[test]
    fn no_global_filters_when_extension_missing() {
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

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), true, None)
            .expect("convert spec");

        assert!(spec.listener_isolation);
        let listener = spec.isolation_listener.unwrap();
        assert!(listener.http_filters.is_none());
    }

    #[test]
    fn no_route_filters_when_extension_missing() {
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

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), false, None)
            .expect("convert spec");

        assert_eq!(spec.routes.len(), 1);
        let route = &spec.routes[0];
        assert!(route.override_config.is_none());
    }

    #[test]
    fn rejects_invalid_global_filter_format() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "x-flowplane-filters": "invalid-not-array",
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

        let result = openapi_to_api_definition_spec(doc, "test-team".to_string(), true, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GatewayError::InvalidFilters(_)));
    }

    #[test]
    fn route_filters_from_post_when_get_missing() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "post": {
                            "x-flowplane-route-overrides": {
                                "cors": "allow-authenticated"
                            },
                            "responses": {
                                "201": {"description": "Created"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), false, None)
            .expect("convert spec");

        assert_eq!(spec.routes.len(), 1);
        let route = &spec.routes[0];
        assert!(route.override_config.is_some());
    }

    #[test]
    fn combines_global_and_route_level_filters() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "x-flowplane-filters": [
                    {
                        "filter": {
                            "type": "cors",
                            "policy": {
                                "allow_origin": [{"type": "exact", "value": "*"}],
                                "allow_methods": ["GET", "POST"]
                            }
                        }
                    }
                ],
                "paths": {
                    "/users": {
                        "get": {
                            "x-flowplane-route-overrides": {
                                "authn": "oidc-default"
                            },
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), true, None)
            .expect("convert spec");

        // Verify global filters
        let listener = spec.isolation_listener.as_ref().unwrap();
        assert!(listener.http_filters.is_some());
        let global_filters = listener.http_filters.as_ref().unwrap();
        assert_eq!(global_filters.len(), 1);

        // Verify route-level filters
        assert_eq!(spec.routes.len(), 1);
        let route = &spec.routes[0];
        assert!(route.override_config.is_some());
    }

    #[test]
    fn converts_openapi_with_multiple_servers_for_load_balancing() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Multi-Server Example", "version": "1.0.0"},
                "servers": [
                    {"url": "https://api-1.example.com:443"},
                    {"url": "https://api-2.example.com:443"},
                    {"url": "https://api-3.example.com:443"}
                ],
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

        let spec = openapi_to_api_definition_spec(doc, "lb-team".to_string(), true, None)
            .expect("convert spec");

        assert_eq!(spec.team, "lb-team");
        assert_eq!(spec.domain, "api-1.example.com");
        assert_eq!(spec.routes.len(), 1);

        // Verify upstream targets include all 3 servers
        let route = &spec.routes[0];
        let targets = route.upstream_targets.get("targets").unwrap().as_array().unwrap();
        assert_eq!(targets.len(), 3);

        // Verify each target has correct endpoint
        assert_eq!(targets[0]["endpoint"], "api-1.example.com:443");
        assert_eq!(targets[1]["endpoint"], "api-2.example.com:443");
        assert_eq!(targets[2]["endpoint"], "api-3.example.com:443");

        // Verify names are unique
        assert_eq!(targets[0]["name"], "api-1.example.com-upstream-0");
        assert_eq!(targets[1]["name"], "api-2.example.com-upstream-1");
        assert_eq!(targets[2]["name"], "api-3.example.com-upstream-2");
    }

    #[test]
    fn rejects_openapi_with_mixed_http_and_https_servers() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Mixed Scheme Example", "version": "1.0.0"},
                "servers": [
                    {"url": "https://api-1.example.com:443"},
                    {"url": "http://api-2.example.com:80"}
                ],
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

        let result = openapi_to_api_definition_spec(doc, "test-team".to_string(), true, None);

        assert!(result.is_err());
        match result {
            Err(GatewayError::UnsupportedServer(msg)) => {
                assert!(msg.contains("same scheme"));
                assert!(msg.contains("https"));
                assert!(msg.contains("http"));
            }
            _ => panic!("Expected UnsupportedServer error for mixed schemes"),
        }
    }

    #[test]
    fn supports_server_variables_for_load_balancing_weights() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Weighted LB Example", "version": "1.0.0"},
                "servers": [
                    {
                        "url": "https://api-1.example.com:443",
                        "variables": {
                            "x-flowplane-weight": {
                                "default": "100"
                            }
                        }
                    },
                    {
                        "url": "https://api-2.example.com:443",
                        "variables": {
                            "x-flowplane-weight": {
                                "default": "50"
                            }
                        }
                    }
                ],
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

        let spec = openapi_to_api_definition_spec(doc, "weighted-team".to_string(), true, None)
            .expect("convert spec");

        let route = &spec.routes[0];
        let targets = route.upstream_targets.get("targets").unwrap().as_array().unwrap();
        assert_eq!(targets.len(), 2);

        // Verify weights are set
        assert_eq!(targets[0]["weight"], 100);
        assert_eq!(targets[1]["weight"], 50);
    }
}
