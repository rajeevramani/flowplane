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
//!
//! ## Dynamic Filter Support
//!
//! This module supports both static (hardcoded) and dynamic filter types:
//! - Static filters use `FilterConfig` enum for strongly-typed conversion
//! - Dynamic filters use `DynamicFilterConverter` with schema-driven conversion
//!
//! For dynamic filters, the configuration is converted to a `google.protobuf.Struct`
//! wrapped as `TypedConfig`, which Envoy can interpret at runtime.

use crate::domain::filter_schema::FilterSchemaRegistry;
use crate::domain::FilterConfig;
use crate::storage::{
    FilterData, FilterRepository, RouteConfigData, RouteFilterRepository, RouteRepository,
    VirtualHostFilterRepository, VirtualHostRepository,
};
use crate::xds::filters::dynamic_conversion::DynamicFilterConverter;
use crate::xds::filters::http::HttpScopedConfig;
use crate::xds::filters::{Base64Bytes, TypedConfig};
use crate::Result;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Context for hierarchical filter injection.
pub struct HierarchicalFilterContext<'a> {
    pub filter_repo: FilterRepository,
    pub vhost_repo: VirtualHostRepository,
    pub route_repo: RouteRepository,
    pub vhost_filter_repo: VirtualHostFilterRepository,
    pub route_filter_repo: RouteFilterRepository,
    /// Schema registry for dynamic filter type conversion
    pub schema_registry: &'a FilterSchemaRegistry,
}

/// Inject attached filters into route configurations.
///
/// This modifies the route config's JSON configuration to include any filters
/// attached via the route_config_filters junction table.
///
/// Supports both static (hardcoded) and dynamic filter types:
/// - Static filters use `FilterConfig` enum for strongly-typed conversion
/// - Dynamic filters use `DynamicFilterConverter` with schema-driven conversion
///
/// # Arguments
///
/// * `route_configs` - Mutable slice of route config data to inject filters into
/// * `filter_repo` - Repository for loading filter configurations
/// * `schema_registry` - Registry for dynamic filter type schemas
///
/// # Returns
///
/// Ok(()) on success, or an error if filter injection fails.
pub async fn inject_route_config_filters(
    route_configs: &mut [RouteConfigData],
    filter_repo: &FilterRepository,
    schema_registry: &FilterSchemaRegistry,
) -> Result<()> {
    let converter = DynamicFilterConverter::new(schema_registry);

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
            // Try to convert using static or dynamic approach
            let (filter_name, scoped_config) =
                match convert_filter_to_scoped_config(&filter_data, &converter) {
                    Some(result) => result,
                    None => {
                        debug!(
                            filter_id = %filter_data.id,
                            filter_name = %filter_data.name,
                            filter_type = %filter_data.filter_type,
                            "Filter doesn't support per-route config, skipping"
                        );
                        continue;
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
/// Supports both static (hardcoded) and dynamic filter types using the
/// schema registry for dynamic conversion.
///
/// Hierarchy (most specific wins):
/// 1. RouteConfig-level filters → applied to ALL routes
/// 2. VirtualHost-level filters → applied to routes in that vhost (overrides #1)
/// 3. Route-level filters → applied to specific routes only (overrides #2)
pub async fn inject_route_filters_hierarchical(
    route_configs: &mut [RouteConfigData],
    ctx: &HierarchicalFilterContext<'_>,
) -> Result<()> {
    let converter = DynamicFilterConverter::new(ctx.schema_registry);

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
            &converter,
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
    converter: &DynamicFilterConverter,
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
                                    convert_filter_to_scoped_config(filter_data, converter)
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
///
/// This function uses a hybrid approach:
/// 1. First tries to parse as a known `FilterConfig` enum variant (strongly typed)
/// 2. Falls back to dynamic conversion using `DynamicFilterConverter` (schema-driven)
///
/// For dynamic filters, the configuration is wrapped as `HttpScopedConfig::Typed`
/// with the protobuf bytes base64-encoded.
fn convert_filter_to_scoped_config(
    filter_data: &FilterData,
    converter: &DynamicFilterConverter,
) -> Option<(String, HttpScopedConfig)> {
    // Try static conversion first (for known filter types)
    if let Ok(filter_config) = serde_json::from_str::<FilterConfig>(&filter_data.configuration) {
        match filter_config.to_per_route_config() {
            Ok(Some((name, config))) => return Some((name, config)),
            Ok(None) => {
                debug!(
                    filter_id = %filter_data.id,
                    filter_name = %filter_data.name,
                    filter_type = %filter_data.filter_type,
                    "Static filter doesn't support per-route config"
                );
                return None;
            }
            Err(e) => {
                warn!(
                    filter_id = %filter_data.id,
                    filter_name = %filter_data.name,
                    error = %e,
                    "Failed static conversion, trying dynamic"
                );
                // Fall through to dynamic conversion
            }
        }
    }

    // Fall back to dynamic conversion for unknown/custom filter types
    let config_json: serde_json::Value = match serde_json::from_str(&filter_data.configuration) {
        Ok(json) => json,
        Err(e) => {
            warn!(
                filter_id = %filter_data.id,
                filter_name = %filter_data.name,
                error = %e,
                "Failed to parse filter configuration as JSON"
            );
            return None;
        }
    };

    // Use dynamic converter to get per-route config
    match converter.to_per_route_any(&filter_data.filter_type, &config_json) {
        Ok(Some((filter_name, any))) => {
            // Wrap the EnvoyAny as TypedConfig for JSON storage
            let typed_config =
                TypedConfig { type_url: any.type_url, value: Base64Bytes(any.value) };
            Some((filter_name, HttpScopedConfig::Typed(typed_config)))
        }
        Ok(None) => {
            debug!(
                filter_id = %filter_data.id,
                filter_name = %filter_data.name,
                filter_type = %filter_data.filter_type,
                "Dynamic filter doesn't support per-route config"
            );
            None
        }
        Err(e) => {
            warn!(
                filter_id = %filter_data.id,
                filter_name = %filter_data.name,
                filter_type = %filter_data.filter_type,
                error = %e,
                "Failed to convert filter using dynamic converter"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::filters::http::header_mutation::{
        HeaderMutationEntry, HeaderMutationPerRouteConfig,
    };
    use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
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
