//! Listener-level filter injection (protobuf-based).
//!
//! Injects filter configurations into listener HCM filter chains by modifying
//! the protobuf representation of the listener.
//!
//! This module uses the dynamic filter schema system for converting filter
//! configurations to Envoy protobuf messages.

use super::JwtConfigMerger;
use crate::domain::filter_schema::FilterSchemaRegistry;
use crate::domain::{AttachmentPoint, CustomWasmFilterId};
use crate::storage::{FilterData, FilterRepository, ListenerRepository, RouteConfigRepository};
use crate::xds::filters::dynamic_conversion::DynamicFilterConverter;
use crate::xds::filters::http::jwt_auth::JwtAuthenticationConfig;
use crate::xds::filters::http::wasm::{WasmCodeSource, WasmConfig, WasmLocalSource, WasmVmConfig};
use crate::xds::helpers::ListenerModifier;
use crate::xds::resources::{create_jwks_cluster, BuiltResource, CLUSTER_TYPE_URL};
use crate::xds::state::XdsState;
use crate::Result;
use base64::Engine;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter;
use std::collections::HashSet;
use tracing::{debug, info, warn};

/// Inject listener-attached filters into the HTTP connection manager filter chain.
///
/// This function:
/// 1. For each listener, queries the database for attached filters
/// 2. Also queries filters attached to the route configuration used by the listener
/// 3. Merges compatible filters (specifically JWT authentication)
/// 4. Injects the filters into the listener's HCM filter chain
///
/// Uses the dynamic filter schema system to convert filter configurations to
/// Envoy protobuf messages without hardcoded type-specific conversion code.
///
/// # Arguments
///
/// * `built_listeners` - Mutable slice of built listener resources to modify
/// * `filter_repo` - Repository for loading filter configurations
/// * `listener_repo` - Repository for listener metadata
/// * `route_config_repo` - Optional repository for route config metadata
/// * `xds_state` - XDS state for cluster management and cached resource access
/// * `schema_registry` - Filter schema registry for dynamic conversion
///
/// # Returns
///
/// Ok(()) on success, or an error if filter injection fails.
#[allow(clippy::too_many_arguments)]
pub async fn inject_listener_filters(
    built_listeners: &mut [BuiltResource],
    filter_repo: &FilterRepository,
    listener_repo: &ListenerRepository,
    route_config_repo: Option<&RouteConfigRepository>,
    xds_state: &XdsState,
    schema_registry: &FilterSchemaRegistry,
) -> Result<()> {
    for built in built_listeners.iter_mut() {
        // Get the listener data by name to retrieve the ListenerId
        let listener_data = match listener_repo.get_by_name(&built.name).await {
            Ok(data) => data,
            Err(e) => {
                debug!(
                    listener = %built.name,
                    error = %e,
                    "Could not find listener in database, skipping filter injection"
                );
                continue;
            }
        };

        // 1. Get filters attached directly to this listener
        let mut filters = match filter_repo.list_listener_filters(&listener_data.id).await {
            Ok(filters) => filters,
            Err(e) => {
                warn!(
                    listener = %built.name,
                    error = %e,
                    "Failed to load listener filters, skipping"
                );
                Vec::new()
            }
        };

        // Create a ListenerModifier to access route config names
        let modifier_temp = match ListenerModifier::decode(&built.resource.value, &built.name) {
            Ok(m) => m,
            Err(e) => {
                warn!(listener = %built.name, error = %e, "Failed to decode listener");
                continue;
            }
        };

        // 2. Find route configs used by this listener and get their filters
        if let Some(route_config_repo) = route_config_repo {
            let route_config_names = modifier_temp.get_route_config_names();

            for route_config_name in route_config_names {
                match route_config_repo.get_by_name(&route_config_name).await {
                    Ok(route_config) => {
                        match filter_repo.list_route_config_filters(&route_config.id).await {
                            Ok(route_config_filters) => {
                                if !route_config_filters.is_empty() {
                                    info!(
                                        listener = %built.name,
                                        route_config_name = %route_config_name,
                                        count = route_config_filters.len(),
                                        "Found filters attached to route config used by listener"
                                    );
                                    filters.extend(route_config_filters);
                                }
                            }
                            Err(e) => {
                                warn!(
                                    listener = %built.name,
                                    route_config_name = %route_config_name,
                                    error = %e,
                                    "Failed to list filters for route config"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        debug!(
                            listener = %built.name,
                            route_config_name = %route_config_name,
                            error = %e,
                            "Failed to look up route config by name"
                        );
                    }
                }
            }
        }

        // Deduplicate filters based on ID, keeping the first occurrence
        // This prevents the same filter from being injected twice if it's attached
        // to both the listener and the route config
        let mut seen_ids = HashSet::new();
        filters.retain(|f| seen_ids.insert(f.id.to_string()));

        if filters.is_empty() {
            continue;
        }

        // 2.5. Resolve custom WASM filter types to standard wasm configs with inline_bytes
        let mut resolved_filters = filters.clone();
        resolve_custom_wasm_filters(&mut resolved_filters, xds_state, &built.name).await;

        // 3. Process and merge filters using dynamic schema-driven conversion
        let (jwt_merger, other_filters) =
            process_filters(&resolved_filters, &built.name, schema_registry)?;

        // Build the final list of HTTP filters to inject
        let mut http_filters_to_inject = other_filters;

        // Merge JWT configs and create filter if present
        if jwt_merger.has_providers() {
            let merged_config = jwt_merger.finish();

            // Auto-create JWKS clusters for remote providers
            auto_create_jwks_clusters(&merged_config, xds_state, &built.name);

            // Create the JWT filter
            match merged_config.to_any() {
                Ok(any) => {
                    http_filters_to_inject.push(HttpFilter {
                        name: "envoy.filters.http.jwt_authn".to_string(),
                        config_type: Some(
                            envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(any)
                        ),
                        is_optional: false,
                        disabled: false,
                    });
                }
                Err(e) => {
                    warn!(
                        listener = %built.name,
                        error = %e,
                        "Failed to convert merged JWT config to protobuf, skipping"
                    );
                }
            }
        }

        if http_filters_to_inject.is_empty() {
            continue;
        }

        // 4. Inject filters into listener using ListenerModifier
        let mut modifier = match ListenerModifier::decode(&built.resource.value, &built.name) {
            Ok(m) => m,
            Err(e) => {
                warn!(listener = %built.name, error = %e, "Failed to decode listener");
                continue;
            }
        };

        for http_filter in http_filters_to_inject {
            if http_filter.name == "envoy.filters.http.jwt_authn" {
                // For JWT, always replace because we've merged everything
                modifier.replace_or_add_filter(http_filter.clone())?;
                info!(
                    listener = %built.name,
                    filter_name = %http_filter.name,
                    "Replaced/added JWT filter with merged configuration"
                );
            } else {
                modifier.add_filter_before_router(http_filter.clone(), false)?;
                info!(
                    listener = %built.name,
                    filter_name = %http_filter.name,
                    "Injected filter into listener HCM"
                );
            }
        }

        // If we modified the listener, update the built resource
        if let Some(encoded) = modifier.finish_if_modified() {
            built.resource.value = encoded;
            info!(
                listener = %built.name,
                "Re-encoded listener with injected filters"
            );
        }
    }

    Ok(())
}

/// Process filter data and return a JWT merger and list of other HTTP filters.
///
/// Uses the dynamic filter schema system for converting filter configurations
/// to Envoy protobuf messages. This enables adding new filter types via YAML
/// schemas without modifying this code.
fn process_filters(
    filters: &[FilterData],
    listener_name: &str,
    schema_registry: &FilterSchemaRegistry,
) -> Result<(JwtConfigMerger, Vec<HttpFilter>)> {
    let mut jwt_merger = JwtConfigMerger::new();
    let mut other_filters: Vec<HttpFilter> = Vec::new();
    let converter = DynamicFilterConverter::new(schema_registry);

    for filter_data in filters {
        let filter_type = &filter_data.filter_type;

        // Get schema for this filter type
        let schema = match schema_registry.get(filter_type) {
            Some(s) => s,
            None => {
                warn!(
                    listener = %listener_name,
                    filter_name = %filter_data.name,
                    filter_type = %filter_type,
                    "Unknown filter type, no schema found, skipping"
                );
                continue;
            }
        };

        // Parse the JSON configuration
        let config_json: serde_json::Value = match serde_json::from_str(&filter_data.configuration)
        {
            Ok(json) => json,
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_id = %filter_data.id,
                    filter_name = %filter_data.name,
                    error = %e,
                    "Failed to parse filter configuration JSON, skipping"
                );
                continue;
            }
        };

        // Extract the inner config (handle both wrapped and unwrapped formats)
        let inner_config = if let Some(obj) = config_json.as_object() {
            if let Some(config) = obj.get("config") {
                config.clone()
            } else {
                config_json.clone()
            }
        } else {
            config_json.clone()
        };

        // JWT requires special handling for merging multiple providers
        if filter_type == "jwt_auth" {
            match serde_json::from_value::<JwtAuthenticationConfig>(inner_config.clone()) {
                Ok(jwt_config) => {
                    jwt_merger.add(&jwt_config);
                    continue;
                }
                Err(e) => {
                    warn!(
                        listener = %listener_name,
                        filter_name = %filter_data.name,
                        error = %e,
                        "Failed to parse JWT config, skipping"
                    );
                    continue;
                }
            }
        }

        // Route-only filters shouldn't be injected at listener level
        if !schema.capabilities.attachment_points.contains(&AttachmentPoint::Listener) {
            debug!(
                listener = %listener_name,
                filter_name = %filter_data.name,
                filter_type = %filter_type,
                "Filter is route-level only, skipping listener injection"
            );
            continue;
        }

        // Try to use strongly-typed FilterConfig conversion first (proper protobuf),
        // fall back to dynamic Struct-based conversion for unknown filter types
        // Note: or_else always returns Some, so expect is safe here
        let any_result = try_typed_conversion(filter_type, &inner_config)
            .or_else(|| {
                // Fall back to dynamic conversion for unknown filter types
                Some(converter.to_listener_any(filter_type, &inner_config))
            })
            .expect("BUG: or_else guarantees Some - this should never fail");

        match any_result {
            Ok(any) => {
                let http_filter = HttpFilter {
                    name: schema.envoy.http_filter_name.clone(),
                    config_type: Some(
                        envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(any),
                    ),
                    is_optional: false,
                    disabled: false,
                };
                other_filters.push(http_filter);
                info!(
                    listener = %listener_name,
                    filter_name = %filter_data.name,
                    filter_type = %filter_type,
                    "Prepared filter for injection"
                );
            }
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_name = %filter_data.name,
                    filter_type = %filter_type,
                    error = %e,
                    "Failed to convert filter config to protobuf, skipping"
                );
            }
        }
    }

    Ok((jwt_merger, other_filters))
}

/// Try to convert filter config using strongly-typed FilterConfig conversion.
///
/// This uses the proper protobuf conversion for known filter types, which produces
/// correct Envoy protobuf messages that can be deserialized properly.
///
/// Returns None if the filter type is not a known built-in type, allowing the caller
/// to fall back to dynamic Struct-based conversion.
fn try_typed_conversion(
    filter_type: &str,
    config: &serde_json::Value,
) -> Option<Result<envoy_types::pb::google::protobuf::Any>> {
    use crate::xds::filters::http::compressor::CompressorConfig;
    use crate::xds::filters::http::cors::CorsConfig;
    use crate::xds::filters::http::custom_response::CustomResponseConfig;
    use crate::xds::filters::http::ext_authz::ExtAuthzConfig;
    use crate::xds::filters::http::header_mutation::HeaderMutationConfig;
    use crate::xds::filters::http::local_rate_limit::LocalRateLimitConfig;
    use crate::xds::filters::http::mcp::McpFilterConfig;

    match filter_type {
        "ext_authz" => {
            let config: ExtAuthzConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!(
                        "Invalid ext_authz config: {}",
                        e
                    ))))
                }
            };
            Some(config.to_any())
        }
        "compressor" => {
            let config: CompressorConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!(
                        "Invalid compressor config: {}",
                        e
                    ))))
                }
            };
            Some(config.to_any())
        }
        "cors" => {
            // CORS filter needs the empty Cors marker in the HCM chain, not the CorsPolicy.
            // The CorsPolicy is applied per-route via typed_per_filter_config.
            // Validate the config to catch errors early, but return the empty marker.
            let config: CorsConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!("Invalid cors config: {}", e))))
                }
            };
            // Validate the policy
            if let Err(e) = config.policy.validate() {
                return Some(Err(e));
            }
            // Return empty CORS marker for HCM chain
            Some(Ok(crate::xds::filters::http::cors::filter_marker_any()))
        }
        "header_mutation" => {
            // Parse and convert using proper protobuf
            let config: HeaderMutationConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!(
                        "Invalid header_mutation config: {}",
                        e
                    ))))
                }
            };
            Some(config.to_any())
        }
        "local_rate_limit" => {
            let config: LocalRateLimitConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!(
                        "Invalid local_rate_limit config: {}",
                        e
                    ))))
                }
            };
            Some(config.to_any())
        }
        "custom_response" => {
            let config: CustomResponseConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!(
                        "Invalid custom_response config: {}",
                        e
                    ))))
                }
            };
            Some(config.to_any())
        }
        "mcp" => {
            let config: McpFilterConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!("Invalid mcp config: {}", e))))
                }
            };
            Some(config.to_any())
        }
        "rbac" => {
            use crate::xds::filters::http::rbac::RbacConfig;
            let config: RbacConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!("Invalid rbac config: {}", e))))
                }
            };
            Some(config.to_any())
        }
        "oauth2" => {
            use crate::xds::filters::http::oauth2::OAuth2Config;
            let config: OAuth2Config = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!("Invalid oauth2 config: {}", e))))
                }
            };
            Some(config.to_any())
        }
        "wasm" => {
            use crate::xds::filters::http::wasm::WasmConfig;
            let config: WasmConfig = match serde_json::from_value(config.clone()) {
                Ok(c) => c,
                Err(e) => {
                    return Some(Err(crate::Error::config(format!("Invalid wasm config: {}", e))))
                }
            };
            Some(config.to_any())
        }
        // JWT is handled specially in process_filters, so we don't need it here
        // Unknown filter types return None to fall back to dynamic conversion
        _ => None,
    }
}

/// Resolve custom WASM filter types to standard wasm configs with inline_bytes.
///
/// This function processes filters with types starting with "custom_wasm_" and:
/// 1. Extracts the custom filter ID from the type name
/// 2. Fetches the WASM binary from the repository
/// 3. Builds a WasmConfig with inline_bytes containing the base64-encoded binary
/// 4. Transforms the filter to use the standard "wasm" type
///
/// This allows custom user-uploaded WASM filters to be injected into the
/// listener filter chain using the same mechanism as built-in WASM filters.
async fn resolve_custom_wasm_filters(
    filters: &mut [FilterData],
    xds_state: &XdsState,
    listener_name: &str,
) {
    let custom_wasm_repo = match &xds_state.custom_wasm_filter_repository {
        Some(repo) => repo,
        None => return, // No repository configured, nothing to resolve
    };

    for filter in filters.iter_mut() {
        if !filter.filter_type.starts_with("custom_wasm_") {
            continue;
        }

        let custom_id = match filter.filter_type.strip_prefix("custom_wasm_") {
            Some(id) => id,
            None => continue,
        };

        let filter_id = CustomWasmFilterId::from_string(custom_id.to_string());

        // Fetch the custom filter metadata
        let custom_filter = match custom_wasm_repo.get_by_id(&filter_id).await {
            Ok(data) => data,
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_name = %filter.name,
                    filter_type = %filter.filter_type,
                    error = %e,
                    "Failed to fetch custom WASM filter metadata, skipping"
                );
                continue;
            }
        };

        // Fetch the WASM binary
        let wasm_binary = match custom_wasm_repo.get_wasm_binary(&filter_id).await {
            Ok(binary) => binary,
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_name = %filter.name,
                    custom_filter_name = %custom_filter.name,
                    error = %e,
                    "Failed to fetch custom WASM binary, skipping"
                );
                continue;
            }
        };

        // Parse the user's configuration
        let user_config: serde_json::Value = match serde_json::from_str(&filter.configuration) {
            Ok(json) => json,
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_name = %filter.name,
                    error = %e,
                    "Failed to parse custom WASM filter configuration, skipping"
                );
                continue;
            }
        };

        // Extract the inner config (handle both wrapped and unwrapped formats)
        let inner_config = if let Some(obj) = user_config.as_object() {
            if let Some(config) = obj.get("config") {
                config.clone()
            } else {
                user_config.clone()
            }
        } else {
            user_config.clone()
        };

        // Base64-encode the WASM binary
        let inline_bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm_binary);

        // Build the WasmConfig with inline_bytes
        let wasm_config = WasmConfig {
            name: custom_filter.name.clone(),
            root_id: String::new(),
            vm_config: WasmVmConfig {
                vm_id: format!("custom_wasm_{}", custom_filter.id),
                runtime: custom_filter.runtime.clone(),
                code: WasmCodeSource {
                    local: Some(WasmLocalSource {
                        filename: None,
                        inline_bytes: Some(inline_bytes_b64),
                        inline_string: None,
                    }),
                    remote: None,
                },
                configuration: None,
                allow_precompiled: false,
                nack_on_code_cache_miss: false,
            },
            configuration: Some(inner_config),
            failure_policy: Some(custom_filter.failure_policy.clone()),
        };

        // Serialize the WasmConfig as the new configuration
        match serde_json::to_string(&wasm_config) {
            Ok(new_config) => {
                info!(
                    listener = %listener_name,
                    filter_name = %filter.name,
                    custom_filter_name = %custom_filter.name,
                    wasm_size = wasm_binary.len(),
                    "Resolved custom WASM filter to inline binary"
                );

                // Transform the filter to use standard wasm type
                filter.filter_type = "wasm".to_string();
                filter.configuration = new_config;
            }
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_name = %filter.name,
                    error = %e,
                    "Failed to serialize WASM config, skipping"
                );
            }
        }
    }
}

/// Auto-create JWKS clusters for remote JWT providers.
fn auto_create_jwks_clusters(
    config: &crate::xds::filters::http::jwt_auth::JwtAuthenticationConfig,
    xds_state: &XdsState,
    listener_name: &str,
) {
    use crate::xds::filters::http::jwt_auth::JwtJwksSourceConfig;

    let existing_clusters: HashSet<String> =
        xds_state.cached_resources(CLUSTER_TYPE_URL).iter().map(|r| r.name.clone()).collect();

    for (provider_name, provider_config) in &config.providers {
        if let JwtJwksSourceConfig::Remote(remote) = &provider_config.jwks {
            let cluster_name = &remote.http_uri.cluster;
            let jwks_uri = &remote.http_uri.uri;

            if !existing_clusters.contains(cluster_name) {
                match create_jwks_cluster(cluster_name, jwks_uri) {
                    Ok(cluster) => {
                        if xds_state
                            .apply_built_resources(CLUSTER_TYPE_URL, vec![cluster])
                            .is_some()
                        {
                            info!(
                                cluster = %cluster_name,
                                provider = %provider_name,
                                jwks_uri = %jwks_uri,
                                listener = %listener_name,
                                "Auto-created JWKS cluster for JWT provider"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            cluster = %cluster_name,
                            provider = %provider_name,
                            error = %e,
                            "Failed to create JWKS cluster"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::FilterId;
    use crate::xds::filters::http::jwt_auth::{
        JwtJwksSourceConfig, JwtProviderConfig, LocalJwksConfig,
    };
    use chrono::Utc;
    use std::collections::HashMap;

    fn make_test_filter_data(name: &str, filter_type: &str, config_json: String) -> FilterData {
        FilterData {
            id: FilterId::new(),
            name: name.to_string(),
            team: "test-team".to_string(),
            filter_type: filter_type.to_string(),
            description: None,
            configuration: config_json,
            version: 1,
            source: "test".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_process_filters_jwt_merging() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();

        let mut providers = HashMap::new();
        providers.insert(
            "test-provider".to_string(),
            JwtProviderConfig {
                issuer: Some("https://issuer.example.com".to_string()),
                jwks: JwtJwksSourceConfig::Local(LocalJwksConfig {
                    inline_string: Some(r#"{"keys":[]}"#.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let jwt_config = JwtAuthenticationConfig { providers, ..Default::default() };

        // Create JSON config in the format stored in the database
        let config_json = serde_json::json!({
            "type": "jwt_auth",
            "config": jwt_config
        });

        let filter_data = make_test_filter_data(
            "jwt-filter",
            "jwt_auth",
            serde_json::to_string(&config_json).unwrap(),
        );

        let (jwt_merger, other_filters) =
            process_filters(&[filter_data], "test-listener", &registry).unwrap();

        assert!(jwt_merger.has_providers());
        assert_eq!(jwt_merger.provider_count(), 1);
        assert!(other_filters.is_empty());
    }

    #[test]
    fn test_process_filters_header_mutation() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();

        // Header mutation can be installed on listeners (empty config) and configured per-route
        let config_json = serde_json::json!({
            "type": "header_mutation",
            "config": {
                "request_headers_to_add": [{"key": "X-Test", "value": "test"}],
                "request_headers_to_remove": [],
                "response_headers_to_add": [],
                "response_headers_to_remove": []
            }
        });

        let filter_data = make_test_filter_data(
            "hm-filter",
            "header_mutation",
            serde_json::to_string(&config_json).unwrap(),
        );

        let (jwt_merger, other_filters) =
            process_filters(&[filter_data], "test-listener", &registry).unwrap();

        assert!(!jwt_merger.has_providers());
        // Header mutation should be processed for listener injection
        assert_eq!(other_filters.len(), 1);
        assert_eq!(other_filters[0].name, "envoy.filters.http.header_mutation");
    }

    #[test]
    fn test_process_filters_local_rate_limit() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();

        let config_json = serde_json::json!({
            "type": "local_rate_limit",
            "config": {
                "stat_prefix": "test_rate_limit",
                "token_bucket": {
                    "max_tokens": 100,
                    "tokens_per_fill": 10,
                    "fill_interval_ms": 1000
                },
                "status_code": 429
            }
        });

        let filter_data = make_test_filter_data(
            "rate-limit-filter",
            "local_rate_limit",
            serde_json::to_string(&config_json).unwrap(),
        );

        let (jwt_merger, other_filters) =
            process_filters(&[filter_data], "test-listener", &registry).unwrap();

        assert!(!jwt_merger.has_providers());
        assert_eq!(other_filters.len(), 1);
        assert_eq!(other_filters[0].name, "envoy.filters.http.local_ratelimit");
    }

    #[test]
    fn test_process_filters_custom_response() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();

        let config_json = serde_json::json!({
            "type": "custom_response",
            "config": {
                "matchers": [{
                    "status_code": {"type": "exact", "code": 429},
                    "response": {
                        "status_code": 429,
                        "body": "Rate limited"
                    }
                }]
            }
        });

        let filter_data = make_test_filter_data(
            "custom-response-filter",
            "custom_response",
            serde_json::to_string(&config_json).unwrap(),
        );

        let (jwt_merger, other_filters) =
            process_filters(&[filter_data], "test-listener", &registry).unwrap();

        assert!(!jwt_merger.has_providers());
        assert_eq!(other_filters.len(), 1);
        assert_eq!(other_filters[0].name, "envoy.filters.http.custom_response");
    }

    #[test]
    fn test_process_filters_mcp() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();

        let config_json = serde_json::json!({
            "type": "mcp",
            "config": {
                "traffic_mode": "pass_through"
            }
        });

        let filter_data = make_test_filter_data(
            "mcp-filter",
            "mcp",
            serde_json::to_string(&config_json).unwrap(),
        );

        let (jwt_merger, other_filters) =
            process_filters(&[filter_data], "test-listener", &registry).unwrap();

        assert!(!jwt_merger.has_providers());
        assert_eq!(other_filters.len(), 1);
        assert_eq!(other_filters[0].name, "envoy.filters.http.mcp");
    }
}
