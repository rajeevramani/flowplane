use openapiv3::{OpenAPI, PathItem, ReferenceOr};
use serde_json::json;

use crate::openapi::GatewayError;

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
) -> Result<ApiDefinitionSpec, GatewayError> {
    // Extract domain from first server URL
    let servers = if openapi.servers.is_empty() {
        return Err(GatewayError::MissingServers);
    } else {
        &openapi.servers
    };

    let primary_server = &servers[0];
    let url = url::Url::parse(&primary_server.url).map_err(|err| {
        GatewayError::UnsupportedServer(format!("{} ({})", primary_server.url, err))
    })?;

    let domain = url
        .host_str()
        .ok_or_else(|| {
            GatewayError::UnsupportedServer(format!(
                "Server URL '{}' does not contain a host",
                primary_server.url
            ))
        })?
        .to_string();

    let port = url.port_or_known_default().ok_or_else(|| {
        GatewayError::UnsupportedServer(format!(
            "Server URL '{}' does not include a usable port",
            primary_server.url
        ))
    })?;

    let use_tls = matches!(url.scheme(), "https" | "grpcs");

    // Parse global x-flowplane-filters from OpenAPI extensions
    let global_filters = crate::openapi::parse_global_filters(&openapi)?;

    // Convert OpenAPI paths to RouteSpec
    let mut routes = Vec::new();

    for (path_template, item) in openapi.paths.paths.iter() {
        let path_item = match item {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { reference } => {
                return Err(GatewayError::UnsupportedServer(reference.clone()))
            }
        };

        let effective_path = combine_base_path(primary_server, path_template);

        // Create upstream target configuration
        let upstream_targets = json!({
            "targets": [{
                "name": format!("{}-upstream", domain),
                "endpoint": format!("{}:{}", domain, port),
            }]
        });

        // Parse route-level x-flowplane-filters from path item operations
        // Store the raw JSON value in override_config for typed_per_filter_config processing
        let override_config = parse_route_level_filters(path_item)?;

        let route_spec = RouteSpec {
            match_type: "prefix".to_string(),
            match_value: effective_path,
            case_sensitive: true,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets,
            timeout_seconds: Some(30),
            override_config,
            deployment_note: Some(format!("Generated from OpenAPI path: {}", path_template)),
            route_order: Some(routes.len() as i64),
        };

        routes.push(route_spec);
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
        Some(ListenerInput {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 10000, // Default port for isolated listener
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

/// Parse route-level x-flowplane-filters from path item operations.
///
/// Checks operations in priority order (GET, POST, PUT, DELETE, PATCH, etc.)
/// and returns the raw filter extension value from the first operation that has it.
/// Returns None if no operations have x-flowplane-filters extensions.
///
/// Note: The returned JSON value is stored in RouteSpec.override_config and passed to
/// the materializer's typed_per_filter_config function for processing. The x-flowplane-filters
/// format (array of HttpFilterConfigEntry) may need conversion to work with typed_per_filter_config's
/// expected format (object with filter aliases as keys). This will be addressed in subtask 19.4.
fn parse_route_level_filters(
    path_item: &PathItem,
) -> Result<Option<serde_json::Value>, GatewayError> {
    const EXTENSION_ROUTE_FILTERS: &str = "x-flowplane-filters";

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

    for operation_opt in operations.iter() {
        if let Some(operation) = operation_opt {
            if let Some(value) = operation.extensions.get(EXTENSION_ROUTE_FILTERS) {
                // Return the raw JSON value - it will be processed by typed_per_filter_config
                return Ok(Some(value.clone()));
            }
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

        let spec = openapi_to_api_definition_spec(doc, "platform-team".to_string(), false)
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

        let spec = openapi_to_api_definition_spec(doc, "isolated-team".to_string(), true)
            .expect("convert spec");

        assert!(spec.listener_isolation);
        assert!(spec.isolation_listener.is_some());
        assert!(spec.tls_config.is_none()); // HTTP not HTTPS

        let listener = spec.isolation_listener.unwrap();
        assert_eq!(listener.port, 10000);
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

        let result = openapi_to_api_definition_spec(doc, "test-team".to_string(), false);
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
                            "x-flowplane-filters": [
                                {
                                    "filter": {
                                        "type": "header_mutation",
                                        "request_headers_to_add": [
                                            {"key": "x-route-header", "value": "users", "append": false}
                                        ]
                                    }
                                }
                            ],
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let spec = openapi_to_api_definition_spec(doc, "test-team".to_string(), false)
            .expect("convert spec");

        assert_eq!(spec.routes.len(), 1);
        let route = &spec.routes[0];
        assert!(route.override_config.is_some());

        // Verify the override_config contains the filter
        let override_config = route.override_config.as_ref().unwrap();
        assert!(override_config.is_array());
        let filters = override_config.as_array().unwrap();
        assert_eq!(filters.len(), 1);
    }
}
