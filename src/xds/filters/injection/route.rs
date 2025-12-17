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
//!
//! ## Per-Scope Settings
//!
//! Filters can have per-scope settings that control behavior at specific scopes:
//! - `use_base`: Use the filter's base configuration (default)
//! - `disable`: Disable the filter for this scope
//! - `override`: Use custom configuration for this scope

use crate::domain::filter_schema::FilterSchemaRegistry;
use crate::domain::{FilterConfig, FilterType, PerRouteBehavior};
use crate::storage::{
    FilterData, FilterRepository, RouteConfigData, RouteFilterRepository, RouteRepository,
    VirtualHostFilterRepository, VirtualHostRepository,
};
use crate::xds::filters::dynamic_conversion::DynamicFilterConverter;
use crate::xds::filters::http::compressor::CompressorPerRouteConfig;
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::mcp::McpPerRouteConfig;
use crate::xds::filters::http::rbac::RbacPerRouteConfig;
use crate::xds::filters::http::HttpScopedConfig;
use crate::xds::filters::{Base64Bytes, TypedConfig};
use crate::Result;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Per-scope settings for filter behavior.
#[derive(Debug, Clone, Deserialize)]
pub struct PerScopeSettings {
    /// Behavior type: "use_base", "disable", or "override"
    pub behavior: String,
    /// Override configuration (for behavior = "override")
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    /// Requirement name for JWT-style filters (for behavior = "override" with reference_only)
    #[serde(default, rename = "requirementName")]
    #[allow(dead_code)] // Will be used when JWT per-route settings are fully implemented
    pub requirement_name: Option<String>,
}

/// Filter data paired with per-scope settings.
#[derive(Debug, Clone)]
pub struct FilterWithSettings {
    pub filter: FilterData,
    pub settings: Option<PerScopeSettings>,
}

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
            // Note: This simple injection doesn't support per-scope settings (pass None)
            let (filter_name, scoped_config) =
                match convert_filter_to_scoped_config(&filter_data, &None, &converter) {
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

        // 1. Get RouteConfig-level filters (these don't have per-scope settings - they ARE the base)
        let route_config_filters_raw =
            ctx.filter_repo.list_route_config_filters(&route_config.id).await?;
        let route_config_filters: Vec<FilterWithSettings> = route_config_filters_raw
            .into_iter()
            .map(|filter| FilterWithSettings { filter, settings: None })
            .collect();

        // 2. Get VirtualHosts for this route config and their filters (with settings)
        let virtual_hosts = ctx.vhost_repo.list_by_route_config(&route_config.id).await?;
        let mut vhost_filter_map: HashMap<String, Vec<FilterWithSettings>> = HashMap::new();

        for vhost in &virtual_hosts {
            let vh_filter_attachments =
                ctx.vhost_filter_repo.list_by_virtual_host(&vhost.id).await?;
            let mut vh_filters = Vec::new();

            for attachment in vh_filter_attachments {
                if let Ok(filter) = ctx.filter_repo.get_by_id(&attachment.filter_id).await {
                    // Parse settings from attachment
                    let settings = attachment
                        .settings
                        .and_then(|s| serde_json::from_value::<PerScopeSettings>(s).ok());
                    vh_filters.push(FilterWithSettings { filter, settings });
                }
            }

            if !vh_filters.is_empty() {
                vhost_filter_map.insert(vhost.name.clone(), vh_filters);
            }
        }

        // 3. Get Routes and their filters (with settings)
        let mut route_filter_map: HashMap<(String, String), Vec<FilterWithSettings>> =
            HashMap::new();

        for vhost in &virtual_hosts {
            let routes = ctx.route_repo.list_by_virtual_host(&vhost.id).await?;

            for route in routes {
                let route_filter_attachments =
                    ctx.route_filter_repo.list_by_route(&route.id).await?;
                let mut route_filters = Vec::new();

                for attachment in route_filter_attachments {
                    if let Ok(filter) = ctx.filter_repo.get_by_id(&attachment.filter_id).await {
                        // Parse settings from attachment
                        let settings = attachment
                            .settings
                            .and_then(|s| serde_json::from_value::<PerScopeSettings>(s).ok());
                        route_filters.push(FilterWithSettings { filter, settings });
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
///
/// Per-scope settings are applied as follows:
/// - `behavior: "use_base"` or no settings: Use base filter config
/// - `behavior: "disable"`: Skip this filter for this scope
/// - `behavior: "override"`: Use the settings.config instead of base config
fn inject_hierarchical(
    config: &mut serde_json::Value,
    route_filters: &[FilterWithSettings],
    vhost_filter_map: &HashMap<String, Vec<FilterWithSettings>>,
    rule_filter_map: &HashMap<(String, String), Vec<FilterWithSettings>>,
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
                            let mut effective_filters: HashMap<String, FilterWithSettings> =
                                HashMap::new();

                            // Layer 1: RouteConfig-level (least specific, base config)
                            for fws in route_filters {
                                effective_filters
                                    .insert(fws.filter.filter_type.clone(), fws.clone());
                            }

                            // Layer 2: VHost-level (overrides route-config level)
                            if let Some(vh_f) = vh_filters {
                                for fws in vh_f {
                                    effective_filters
                                        .insert(fws.filter.filter_type.clone(), fws.clone());
                                }
                            }

                            // Layer 3: Rule-level (most specific, overrides all)
                            if let Some(rule_f) = rule_filters {
                                for fws in rule_f {
                                    effective_filters
                                        .insert(fws.filter.filter_type.clone(), fws.clone());
                                }
                            }

                            // Apply effective filters to route
                            for fws in effective_filters.values() {
                                // Determine the scoped config to inject
                                let scoped_result: Option<(String, HttpScopedConfig)> =
                                    if let Some(ref settings) = fws.settings {
                                        if settings.behavior == "disable" {
                                            // Generate a disable config instead of skipping
                                            debug!(
                                                filter_name = %fws.filter.name,
                                                filter_type = %fws.filter.filter_type,
                                                route = %rule_name,
                                                "Generating disable config for filter at this scope"
                                            );
                                            generate_disable_scoped_config(&fws.filter)
                                        } else {
                                            // Use normal conversion (use_base or override)
                                            convert_filter_to_scoped_config(
                                                &fws.filter,
                                                &fws.settings,
                                                converter,
                                            )
                                        }
                                    } else {
                                        // No settings, use base config
                                        convert_filter_to_scoped_config(
                                            &fws.filter,
                                            &fws.settings,
                                            converter,
                                        )
                                    };

                                if let Some((filter_name, scoped_config)) = scoped_result {
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

/// Determine the effective configuration to use for a filter.
///
/// If per-scope settings specify an override, use the override config.
/// Otherwise, use the base filter configuration.
fn determine_effective_config(
    filter_data: &FilterData,
    settings: &Option<PerScopeSettings>,
) -> String {
    // Check if we have override settings with a config
    if let Some(ref s) = settings {
        if s.behavior == "override" {
            if let Some(ref override_config) = s.config {
                // Build the full config structure expected by the converter
                // The converter expects: { "type": "filter_type", "config": {...} }
                let full_config = serde_json::json!({
                    "type": filter_data.filter_type,
                    "config": override_config
                });

                info!(
                    filter_name = %filter_data.name,
                    filter_type = %filter_data.filter_type,
                    "Using per-scope override configuration"
                );

                return full_config.to_string();
            }
        }
    }

    // Default: use base configuration
    filter_data.configuration.clone()
}

/// Generate a disable scoped config for a filter.
///
/// This function creates a per-route configuration that disables the filter
/// for filters that support the DisableOnly or FullConfig per-route behavior.
///
/// # Arguments
///
/// * `filter_data` - The filter to generate disable config for
///
/// # Returns
///
/// - `Some((filter_name, HttpScopedConfig))` - The disable config to inject
/// - `None` - If this filter type doesn't support per-route disable
fn generate_disable_scoped_config(filter_data: &FilterData) -> Option<(String, HttpScopedConfig)> {
    // Parse the filter type
    let filter_type: FilterType =
        match serde_json::from_str(&format!("\"{}\"", filter_data.filter_type)) {
            Ok(ft) => ft,
            Err(e) => {
                warn!(
                    filter_type = %filter_data.filter_type,
                    error = %e,
                    "Failed to parse filter type for disable config"
                );
                return None;
            }
        };

    let metadata = filter_type.metadata();
    let per_route_behavior = metadata.per_route_behavior;

    // Only generate disable config for filters that support per-route config
    if matches!(per_route_behavior, PerRouteBehavior::NotSupported) {
        debug!(
            filter_name = %filter_data.name,
            filter_type = %filter_data.filter_type,
            "Filter doesn't support per-route config, cannot generate disable config"
        );
        return None;
    }

    let filter_name = metadata.http_filter_name.to_string();

    // Generate the appropriate disable config based on filter type
    // Note: OAuth2 does NOT support typed_per_filter_config at all
    let scoped_config = match filter_type {
        FilterType::JwtAuth => {
            HttpScopedConfig::JwtAuthn(JwtPerRouteConfig::Disabled { disabled: true })
        }
        FilterType::Compressor => {
            HttpScopedConfig::Compressor(CompressorPerRouteConfig { disabled: true })
        }
        FilterType::Mcp => HttpScopedConfig::Mcp(McpPerRouteConfig { disabled: true }),
        FilterType::Rbac => {
            HttpScopedConfig::Rbac(RbacPerRouteConfig::Disabled { disabled: true })
        }
        // For other filters, we can't generate a simple disable config
        // They would need override config or don't support disable
        _ => {
            debug!(
                filter_name = %filter_data.name,
                filter_type = %filter_data.filter_type,
                "Filter type doesn't have a simple disable config mechanism"
            );
            return None;
        }
    };

    info!(
        filter_name = %filter_data.name,
        filter_type = %filter_data.filter_type,
        envoy_filter_name = %filter_name,
        "Generated disable scoped config for filter"
    );

    Some((filter_name, scoped_config))
}

/// Convert a FilterData to its scoped config representation.
///
/// This function uses a hybrid approach:
/// 1. First tries to parse as a known `FilterConfig` enum variant (strongly typed)
/// 2. Falls back to dynamic conversion using `DynamicFilterConverter` (schema-driven)
///
/// For dynamic filters, the configuration is wrapped as `HttpScopedConfig::Typed`
/// with the protobuf bytes base64-encoded.
///
/// # Per-scope settings
///
/// If settings are provided with `behavior: "override"` and a `config` field,
/// the override config is used instead of the base filter configuration.
fn convert_filter_to_scoped_config(
    filter_data: &FilterData,
    settings: &Option<PerScopeSettings>,
    converter: &DynamicFilterConverter,
) -> Option<(String, HttpScopedConfig)> {
    // Determine which config to use: override or base
    let effective_config = determine_effective_config(filter_data, settings);

    // Try static conversion first (for known filter types)
    if let Ok(filter_config) = serde_json::from_str::<FilterConfig>(&effective_config) {
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
    let config_json: serde_json::Value = match serde_json::from_str(&effective_config) {
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
