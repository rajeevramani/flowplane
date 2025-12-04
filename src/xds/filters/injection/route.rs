//! Route-level filter injection (JSON-based).
//!
//! Injects filter configurations into route JSON by modifying the
//! `typed_per_filter_config` field of virtual host routes.

use crate::domain::FilterConfig;
use crate::storage::{FilterRepository, RouteData};
use crate::xds::filters::http::header_mutation::{
    HeaderMutationEntry, HeaderMutationPerRouteConfig,
};
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::HttpScopedConfig;
use crate::Result;
use tracing::{info, warn};

/// Inject attached filters into route configurations.
///
/// This modifies the route's JSON configuration to include any filters
/// attached via the route_filters junction table.
///
/// # Arguments
///
/// * `routes` - Mutable slice of route data to inject filters into
/// * `filter_repo` - Repository for loading filter configurations
///
/// # Returns
///
/// Ok(()) on success, or an error if filter injection fails.
pub async fn inject_route_filters(
    routes: &mut [RouteData],
    filter_repo: &FilterRepository,
) -> Result<()> {
    for route in routes.iter_mut() {
        // Get filters attached to this route
        let filters = filter_repo.list_route_filters(&route.id).await?;

        if filters.is_empty() {
            continue;
        }

        // Parse the route configuration
        let mut config: serde_json::Value =
            serde_json::from_str(&route.configuration).map_err(|e| {
                crate::Error::internal(format!(
                    "Failed to parse route configuration for '{}': {}",
                    route.name, e
                ))
            })?;

        // Process each filter and add to typed_per_filter_config
        for filter_data in filters {
            // Parse the filter configuration
            let filter_config: FilterConfig = match serde_json::from_str(&filter_data.configuration)
            {
                Ok(cfg) => cfg,
                Err(e) => {
                    warn!(
                        filter_id = %filter_data.id,
                        filter_name = %filter_data.name,
                        error = %e,
                        "Failed to parse filter configuration, skipping"
                    );
                    continue;
                }
            };

            // Convert to per-route config based on filter type
            let (filter_name, scoped_config) = match filter_config {
                FilterConfig::HeaderMutation(hm_config) => {
                    let per_route = HeaderMutationPerRouteConfig {
                        request_headers_to_add: hm_config
                            .request_headers_to_add
                            .into_iter()
                            .map(|e| HeaderMutationEntry {
                                key: e.key,
                                value: e.value,
                                append: e.append,
                            })
                            .collect(),
                        request_headers_to_remove: hm_config.request_headers_to_remove,
                        response_headers_to_add: hm_config
                            .response_headers_to_add
                            .into_iter()
                            .map(|e| HeaderMutationEntry {
                                key: e.key,
                                value: e.value,
                                append: e.append,
                            })
                            .collect(),
                        response_headers_to_remove: hm_config.response_headers_to_remove,
                    };

                    (
                        "envoy.filters.http.header_mutation".to_string(),
                        HttpScopedConfig::HeaderMutation(per_route),
                    )
                }
                FilterConfig::JwtAuth(jwt_config) => {
                    // Per-route JWT uses requirement_name to reference listener-level config
                    // Use the first provider name as the default requirement
                    let per_route = jwt_config
                        .providers
                        .keys()
                        .next()
                        .map(|name| JwtPerRouteConfig::RequirementName {
                            requirement_name: name.clone(),
                        })
                        .unwrap_or(JwtPerRouteConfig::Disabled { disabled: true });

                    (
                        "envoy.filters.http.jwt_authn".to_string(),
                        HttpScopedConfig::JwtAuthn(per_route),
                    )
                }
                FilterConfig::LocalRateLimit(config) => {
                    // LocalRateLimit can be used as per-route config
                    (
                        "envoy.filters.http.local_ratelimit".to_string(),
                        HttpScopedConfig::LocalRateLimit(config),
                    )
                }
            };

            // Inject into the route's virtual hosts
            inject_into_virtual_hosts(&mut config, &filter_name, &scoped_config);

            info!(
                route_name = %route.name,
                filter_name = %filter_data.name,
                filter_type = %filter_data.filter_type,
                "Injected filter into route configuration"
            );
        }

        // Update the route configuration with the modified JSON
        route.configuration = serde_json::to_string(&config).map_err(|e| {
            crate::Error::internal(format!(
                "Failed to serialize modified route configuration: {}",
                e
            ))
        })?;
    }

    Ok(())
}

/// Inject a scoped filter config into the virtual hosts of a route configuration.
fn inject_into_virtual_hosts(
    config: &mut serde_json::Value,
    filter_name: &str,
    scoped_config: &HttpScopedConfig,
) {
    if let Some(virtual_hosts) = config.get_mut("virtual_hosts") {
        if let Some(vhosts_arr) = virtual_hosts.as_array_mut() {
            for vhost in vhosts_arr {
                // Add to each route within the virtual host
                if let Some(routes_arr) = vhost.get_mut("routes") {
                    if let Some(routes) = routes_arr.as_array_mut() {
                        for route_entry in routes {
                            // Add typed_per_filter_config to the route
                            let tpfc = route_entry.as_object_mut().and_then(|obj| {
                                obj.entry("typed_per_filter_config")
                                    .or_insert_with(|| serde_json::json!({}))
                                    .as_object_mut()
                            });

                            if let Some(tpfc_obj) = tpfc {
                                // Serialize the scoped config
                                if let Ok(config_value) = serde_json::to_value(scoped_config) {
                                    tpfc_obj.insert(filter_name.to_string(), config_value);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_inject_into_virtual_hosts() {
        let mut config = json!({
            "virtual_hosts": [{
                "name": "test",
                "routes": [{
                    "match": { "prefix": "/" },
                    "route": { "cluster": "test" }
                }]
            }]
        });

        let scoped_config = HttpScopedConfig::HeaderMutation(HeaderMutationPerRouteConfig {
            request_headers_to_add: vec![HeaderMutationEntry {
                key: "X-Test".to_string(),
                value: "value".to_string(),
                append: false,
            }],
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        });

        inject_into_virtual_hosts(
            &mut config,
            "envoy.filters.http.header_mutation",
            &scoped_config,
        );

        // Verify the filter was injected
        let route = &config["virtual_hosts"][0]["routes"][0];
        assert!(route["typed_per_filter_config"]["envoy.filters.http.header_mutation"].is_object());
    }

    #[test]
    fn test_inject_preserves_existing_tpfc() {
        let mut config = json!({
            "virtual_hosts": [{
                "name": "test",
                "routes": [{
                    "match": { "prefix": "/" },
                    "route": { "cluster": "test" },
                    "typed_per_filter_config": {
                        "existing.filter": { "some": "config" }
                    }
                }]
            }]
        });

        let scoped_config =
            HttpScopedConfig::JwtAuthn(JwtPerRouteConfig::Disabled { disabled: true });

        inject_into_virtual_hosts(&mut config, "envoy.filters.http.jwt_authn", &scoped_config);

        // Verify both filters are present
        let tpfc = &config["virtual_hosts"][0]["routes"][0]["typed_per_filter_config"];
        assert!(tpfc["existing.filter"].is_object());
        assert!(tpfc["envoy.filters.http.jwt_authn"].is_object());
    }
}
