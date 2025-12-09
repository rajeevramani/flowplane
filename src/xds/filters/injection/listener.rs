//! Listener-level filter injection (protobuf-based).
//!
//! Injects filter configurations into listener HCM filter chains by modifying
//! the protobuf representation of the listener.

use super::JwtConfigMerger;
use crate::domain::FilterConfig;
use crate::storage::{FilterData, FilterRepository, ListenerRepository, RouteConfigRepository};
use crate::xds::helpers::ListenerModifier;
use crate::xds::resources::{create_jwks_cluster, BuiltResource, CLUSTER_TYPE_URL};
use crate::xds::state::XdsState;
use crate::Result;
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
/// # Arguments
///
/// * `built_listeners` - Mutable slice of built listener resources to modify
/// * `filter_repo` - Repository for loading filter configurations
/// * `listener_repo` - Repository for listener metadata
/// * `route_config_repo` - Optional repository for route config metadata
/// * `xds_state` - XDS state for cluster management and cached resource access
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

        if filters.is_empty() {
            continue;
        }

        // 3. Process and merge filters
        let (jwt_merger, other_filters) = process_filters(&filters, &built.name)?;

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
/// Uses the unified conversion framework for filter-to-protobuf conversion.
fn process_filters(
    filters: &[FilterData],
    listener_name: &str,
) -> Result<(JwtConfigMerger, Vec<HttpFilter>)> {
    let mut jwt_merger = JwtConfigMerger::new();
    let mut other_filters: Vec<HttpFilter> = Vec::new();

    for filter_data in filters {
        let filter_config: FilterConfig = match serde_json::from_str(&filter_data.configuration) {
            Ok(config) => config,
            Err(e) => {
                warn!(
                    listener = %listener_name,
                    filter_id = %filter_data.id,
                    filter_name = %filter_data.name,
                    error = %e,
                    "Failed to parse filter configuration, skipping"
                );
                continue;
            }
        };

        let filter_type = filter_config.filter_type();
        let metadata = filter_type.metadata();

        // JWT requires special handling for merging
        if let FilterConfig::JwtAuth(jwt_config) = &filter_config {
            jwt_merger.add(jwt_config);
            continue;
        }

        // Route-only filters shouldn't be injected at listener level
        if !filter_type.can_attach_to(crate::domain::AttachmentPoint::Listener) {
            debug!(
                listener = %listener_name,
                filter_name = %filter_data.name,
                filter_type = %filter_type,
                "Filter is route-level only, skipping listener injection"
            );
            continue;
        }

        // Use unified conversion for all other filters
        match filter_config.to_listener_any() {
            Ok(any) => {
                let http_filter = HttpFilter {
                    name: metadata.http_filter_name.to_string(),
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
        JwtAuthenticationConfig, JwtJwksSourceConfig, JwtProviderConfig, LocalJwksConfig,
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

        let jwt_config =
            FilterConfig::JwtAuth(JwtAuthenticationConfig { providers, ..Default::default() });

        let filter_data = make_test_filter_data(
            "jwt-filter",
            "jwt_auth",
            serde_json::to_string(&jwt_config).unwrap(),
        );

        let (jwt_merger, other_filters) = process_filters(&[filter_data], "test-listener").unwrap();

        assert!(jwt_merger.has_providers());
        assert_eq!(jwt_merger.provider_count(), 1);
        assert!(other_filters.is_empty());
    }

    #[test]
    fn test_process_filters_skips_header_mutation() {
        use crate::domain::HeaderMutationFilterConfig;

        let hm_config = FilterConfig::HeaderMutation(HeaderMutationFilterConfig {
            request_headers_to_add: vec![],
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        });

        let filter_data = make_test_filter_data(
            "hm-filter",
            "header_mutation",
            serde_json::to_string(&hm_config).unwrap(),
        );

        let (jwt_merger, other_filters) = process_filters(&[filter_data], "test-listener").unwrap();

        assert!(!jwt_merger.has_providers());
        assert!(other_filters.is_empty());
    }

    #[test]
    fn test_process_filters_local_rate_limit() {
        use crate::xds::filters::http::local_rate_limit::{
            LocalRateLimitConfig, TokenBucketConfig,
        };

        let rate_limit_config = FilterConfig::LocalRateLimit(LocalRateLimitConfig {
            stat_prefix: "test_rate_limit".to_string(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 100,
                tokens_per_fill: Some(10),
                fill_interval_ms: 1000,
            }),
            status_code: Some(429),
            filter_enabled: None,
            filter_enforced: None,
            per_downstream_connection: None,
            rate_limited_as_resource_exhausted: None,
            max_dynamic_descriptors: None,
            always_consume_default_token_bucket: None,
        });

        let filter_data = make_test_filter_data(
            "rate-limit-filter",
            "rate_limit",
            serde_json::to_string(&rate_limit_config).unwrap(),
        );

        let (jwt_merger, other_filters) = process_filters(&[filter_data], "test-listener").unwrap();

        assert!(!jwt_merger.has_providers());
        assert_eq!(other_filters.len(), 1);
        assert_eq!(other_filters[0].name, "envoy.filters.http.local_ratelimit");
    }
}
