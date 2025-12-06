//! Route-level filter injection (JSON-based).
//!
//! Injects filter configurations into route JSON by modifying the
//! `typed_per_filter_config` field of virtual host routes.
//!
//! Supports 3-level hierarchical filter attachment:
//! 1. RouteConfig-level filters - applied to ALL routes
//! 2. VirtualHost-level filters - applied to routes in that vhost
//! 3. RouteRule-level filters - applied to specific routes only
//!
//! More specific filters override less specific ones.

use crate::domain::FilterConfig;
use crate::storage::{
    FilterData, FilterRepository, RouteConfigData, RouteFilterRepository, RouteRepository,
    VirtualHostFilterRepository, VirtualHostRepository,
};
use crate::xds::filters::http::header_mutation::{
    HeaderMutationEntry, HeaderMutationPerRouteConfig,
};
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::HttpScopedConfig;
use crate::Result;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Context for hierarchical filter injection.
pub struct HierarchicalFilterContext {
    pub filter_repo: FilterRepository,
    pub vhost_repo: VirtualHostRepository,
    pub route_repo: RouteRepository,
    pub vhost_filter_repo: VirtualHostFilterRepository,
    pub route_filter_repo: RouteFilterRepository,
}

/// Inject attached filters into route configurations.
///
/// This modifies the route config's JSON configuration to include any filters
/// attached via the route_config_filters junction table.
///
/// # Arguments
///
/// * `route_configs` - Mutable slice of route config data to inject filters into
/// * `filter_repo` - Repository for loading filter configurations
///
/// # Returns
///
/// Ok(()) on success, or an error if filter injection fails.
pub async fn inject_route_config_filters(
    route_configs: &mut [RouteConfigData],
    filter_repo: &FilterRepository,
) -> Result<()> {
    for route_config in route_configs.iter_mut() {
        // Get filters attached to this route config (RouteConfig level)
        let filters = filter_repo.list_route_config_filters(&route_config.id).await?;

        if filters.is_empty() {
            continue;
        }

        // Parse the route configuration
        let mut config: serde_json::Value = serde_json::from_str(&route_config.configuration)
            .map_err(|e| {
                crate::Error::internal(format!(
                    "Failed to parse route configuration for '{}': {}",
                    route_config.name, e
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
                route_config_name = %route_config.name,
                filter_name = %filter_data.name,
                filter_type = %filter_data.filter_type,
                "Injected filter into route configuration"
            );
        }

        // Update the route configuration with the modified JSON
        route_config.configuration = serde_json::to_string(&config).map_err(|e| {
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

/// Inject filters into route configurations with 3-level hierarchy support.
///
/// This applies filters at RouteConfig, VirtualHost, and Route levels,
/// with more specific filters overriding less specific ones.
///
/// Hierarchy (most specific wins):
/// 1. RouteConfig-level filters → applied to ALL routes
/// 2. VirtualHost-level filters → applied to routes in that vhost (overrides #1)
/// 3. Route-level filters → applied to specific routes only (overrides #2)
pub async fn inject_route_filters_hierarchical(
    route_configs: &mut [RouteConfigData],
    ctx: &HierarchicalFilterContext,
) -> Result<()> {
    for route_config in route_configs.iter_mut() {
        // Parse the route configuration
        let mut config: serde_json::Value = serde_json::from_str(&route_config.configuration)
            .map_err(|e| {
                crate::Error::internal(format!(
                    "Failed to parse route configuration for '{}': {}",
                    route_config.name, e
                ))
            })?;

        // 1. Get RouteConfig-level filters
        let route_config_filters =
            ctx.filter_repo.list_route_config_filters(&route_config.id).await?;

        // 2. Get VirtualHosts for this route config and their filters
        let virtual_hosts = ctx.vhost_repo.list_by_route_config(&route_config.id).await?;
        let mut vhost_filter_map: HashMap<String, Vec<FilterData>> = HashMap::new();

        for vhost in &virtual_hosts {
            let vh_filter_attachments =
                ctx.vhost_filter_repo.list_by_virtual_host(&vhost.id).await?;
            let mut vh_filters = Vec::new();

            for attachment in vh_filter_attachments {
                if let Ok(filter) = ctx.filter_repo.get_by_id(&attachment.filter_id).await {
                    vh_filters.push(filter);
                }
            }

            if !vh_filters.is_empty() {
                vhost_filter_map.insert(vhost.name.clone(), vh_filters);
            }
        }

        // 3. Get Routes and their filters
        let mut route_filter_map: HashMap<(String, String), Vec<FilterData>> = HashMap::new();

        for vhost in &virtual_hosts {
            let routes = ctx.route_repo.list_by_virtual_host(&vhost.id).await?;

            for route in routes {
                let route_filter_attachments =
                    ctx.route_filter_repo.list_by_route(&route.id).await?;
                let mut route_filters = Vec::new();

                for attachment in route_filter_attachments {
                    if let Ok(filter) = ctx.filter_repo.get_by_id(&attachment.filter_id).await {
                        route_filters.push(filter);
                    }
                }

                if !route_filters.is_empty() {
                    route_filter_map
                        .insert((vhost.name.clone(), route.name.clone()), route_filters);
                }
            }
        }

        // Apply hierarchical injection
        let config_modified = inject_hierarchical(
            &mut config,
            &route_config_filters,
            &vhost_filter_map,
            &route_filter_map,
        )?;

        if config_modified {
            route_config.configuration = serde_json::to_string(&config).map_err(|e| {
                crate::Error::internal(format!(
                    "Failed to serialize modified route configuration: {}",
                    e
                ))
            })?;

            debug!(
                route_config_name = %route_config.name,
                route_config_level = route_config_filters.len(),
                vhost_level = vhost_filter_map.len(),
                route_level = route_filter_map.len(),
                "Applied hierarchical filter injection"
            );
        }
    }

    Ok(())
}

/// Apply hierarchical filter injection to a route configuration.
///
/// Returns true if any modifications were made.
fn inject_hierarchical(
    config: &mut serde_json::Value,
    route_filters: &[FilterData],
    vhost_filter_map: &HashMap<String, Vec<FilterData>>,
    rule_filter_map: &HashMap<(String, String), Vec<FilterData>>,
) -> Result<bool> {
    let mut modified = false;

    if let Some(virtual_hosts) = config.get_mut("virtual_hosts") {
        if let Some(vhosts_arr) = virtual_hosts.as_array_mut() {
            for vhost in vhosts_arr {
                let vhost_name =
                    vhost.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();

                // Get VH-level filters for this vhost
                let vh_filters = vhost_filter_map.get(&vhost_name);

                if let Some(routes_arr) = vhost.get_mut("routes") {
                    if let Some(routes) = routes_arr.as_array_mut() {
                        for route_entry in routes {
                            let rule_name = route_entry
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();

                            // Get rule-level filters for this specific rule
                            let rule_key = (vhost_name.clone(), rule_name.clone());
                            let rule_filters = rule_filter_map.get(&rule_key);

                            // Determine effective filters with hierarchy
                            // More specific overrides less specific (by filter type)
                            let mut effective_filters: HashMap<String, FilterData> = HashMap::new();

                            // Layer 1: Route-level (least specific)
                            for filter in route_filters {
                                effective_filters
                                    .insert(filter.filter_type.clone(), filter.clone());
                            }

                            // Layer 2: VHost-level (overrides route-level)
                            if let Some(vh_f) = vh_filters {
                                for filter in vh_f {
                                    effective_filters
                                        .insert(filter.filter_type.clone(), filter.clone());
                                }
                            }

                            // Layer 3: Rule-level (most specific, overrides all)
                            if let Some(rule_f) = rule_filters {
                                for filter in rule_f {
                                    effective_filters
                                        .insert(filter.filter_type.clone(), filter.clone());
                                }
                            }

                            // Apply effective filters to route
                            for filter_data in effective_filters.values() {
                                if let Some((filter_name, scoped_config)) =
                                    convert_to_scoped_config(filter_data)
                                {
                                    let tpfc = route_entry.as_object_mut().and_then(|obj| {
                                        obj.entry("typed_per_filter_config")
                                            .or_insert_with(|| serde_json::json!({}))
                                            .as_object_mut()
                                    });

                                    if let Some(tpfc_obj) = tpfc {
                                        if let Ok(config_value) =
                                            serde_json::to_value(&scoped_config)
                                        {
                                            tpfc_obj.insert(filter_name, config_value);
                                            modified = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(modified)
}

/// Convert a FilterData to its scoped config representation.
fn convert_to_scoped_config(filter_data: &FilterData) -> Option<(String, HttpScopedConfig)> {
    let filter_config: FilterConfig = match serde_json::from_str(&filter_data.configuration) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(
                filter_id = %filter_data.id,
                filter_name = %filter_data.name,
                error = %e,
                "Failed to parse filter configuration"
            );
            return None;
        }
    };

    match filter_config {
        FilterConfig::HeaderMutation(hm_config) => {
            let per_route = HeaderMutationPerRouteConfig {
                request_headers_to_add: hm_config
                    .request_headers_to_add
                    .into_iter()
                    .map(|e| HeaderMutationEntry { key: e.key, value: e.value, append: e.append })
                    .collect(),
                request_headers_to_remove: hm_config.request_headers_to_remove,
                response_headers_to_add: hm_config
                    .response_headers_to_add
                    .into_iter()
                    .map(|e| HeaderMutationEntry { key: e.key, value: e.value, append: e.append })
                    .collect(),
                response_headers_to_remove: hm_config.response_headers_to_remove,
            };

            Some((
                "envoy.filters.http.header_mutation".to_string(),
                HttpScopedConfig::HeaderMutation(per_route),
            ))
        }
        FilterConfig::JwtAuth(jwt_config) => {
            let per_route = jwt_config
                .providers
                .keys()
                .next()
                .map(|name| JwtPerRouteConfig::RequirementName { requirement_name: name.clone() })
                .unwrap_or(JwtPerRouteConfig::Disabled { disabled: true });

            Some((
                "envoy.filters.http.jwt_authn".to_string(),
                HttpScopedConfig::JwtAuthn(per_route),
            ))
        }
        FilterConfig::LocalRateLimit(config) => Some((
            "envoy.filters.http.local_ratelimit".to_string(),
            HttpScopedConfig::LocalRateLimit(config),
        )),
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
