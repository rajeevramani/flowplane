//! Listener filter chain manipulation utilities
//!
//! This module provides helper functions for modifying listener configurations
//! to add or remove HTTP filters from the filter chain. Used by the automatic
//! listener filter chain management system.

use crate::domain::FilterType as DomainFilterType;
use crate::xds::filters::conversion::create_empty_listener_filter;
use crate::xds::filters::http::{HttpFilterConfigEntry, HttpFilterKind, ROUTER_FILTER_NAME};
use crate::xds::listener::{FilterType, ListenerConfig};

#[cfg(test)]
use crate::xds::listener::{FilterChainConfig, FilterConfig};

/// Check if a listener configuration has the specified HTTP filter.
///
/// Searches all filter chains and HTTP connection managers for the filter.
pub fn listener_has_http_filter(config: &ListenerConfig, filter_name: &str) -> bool {
    for chain in &config.filter_chains {
        for filter in &chain.filters {
            if let FilterType::HttpConnectionManager { http_filters, .. } = &filter.filter_type {
                for hf in http_filters {
                    let name =
                        hf.name.as_deref().unwrap_or_else(|| get_filter_default_name(&hf.filter));
                    if name == filter_name {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Add an HTTP filter to a listener configuration, inserting before the router filter.
///
/// The filter is added as an empty/pass-through filter that enables per-route
/// `typed_per_filter_config` overrides.
///
/// # Arguments
///
/// * `config` - The listener configuration to modify
/// * `filter_name` - The Envoy HTTP filter name (e.g., "envoy.filters.http.header_mutation")
///
/// # Returns
///
/// `true` if the filter was added, `false` if it already existed
pub fn add_http_filter_before_router(config: &mut ListenerConfig, filter_name: &str) -> bool {
    let mut added = false;

    for chain in &mut config.filter_chains {
        for filter in &mut chain.filters {
            if let FilterType::HttpConnectionManager { http_filters, .. } = &mut filter.filter_type
            {
                // Check if filter already exists
                let already_exists = http_filters.iter().any(|hf| {
                    let name =
                        hf.name.as_deref().unwrap_or_else(|| get_filter_default_name(&hf.filter));
                    name == filter_name
                });

                if already_exists {
                    continue;
                }

                // Find router position (should be last)
                let router_pos = http_filters.iter().position(|hf| {
                    let name =
                        hf.name.as_deref().unwrap_or_else(|| get_filter_default_name(&hf.filter));
                    is_router_filter(&hf.filter) || name == ROUTER_FILTER_NAME
                });

                // Create the new filter entry based on filter name
                // Some filters (like JWT) cannot be created as empty placeholders
                let new_filter = match create_empty_http_filter_entry(filter_name) {
                    Some(filter) => filter,
                    None => {
                        // Filter requires proper config, skip adding placeholder
                        // The filter will be injected later via inject_listener_auto_filters
                        continue;
                    }
                };

                match router_pos {
                    Some(pos) => http_filters.insert(pos, new_filter),
                    None => http_filters.push(new_filter),
                }

                added = true;
            }
        }
    }

    added
}

/// Remove an HTTP filter from a listener configuration.
///
/// # Arguments
///
/// * `config` - The listener configuration to modify
/// * `filter_name` - The Envoy HTTP filter name to remove
///
/// # Returns
///
/// `true` if the filter was removed, `false` if it wasn't found
pub fn remove_http_filter_from_listener(config: &mut ListenerConfig, filter_name: &str) -> bool {
    let mut removed = false;

    for chain in &mut config.filter_chains {
        for filter in &mut chain.filters {
            if let FilterType::HttpConnectionManager { http_filters, .. } = &mut filter.filter_type
            {
                let original_len = http_filters.len();
                http_filters.retain(|hf| {
                    let name =
                        hf.name.as_deref().unwrap_or_else(|| get_filter_default_name(&hf.filter));
                    name != filter_name
                });
                if http_filters.len() < original_len {
                    removed = true;
                }
            }
        }
    }

    removed
}

/// Get the default name for an HttpFilterKind
fn get_filter_default_name(filter: &HttpFilterKind) -> &'static str {
    match filter {
        HttpFilterKind::Router => ROUTER_FILTER_NAME,
        HttpFilterKind::Compressor(_) => "envoy.filters.http.compressor",
        HttpFilterKind::Cors(_) => "envoy.filters.http.cors",
        HttpFilterKind::LocalRateLimit(_) => "envoy.filters.http.local_ratelimit",
        HttpFilterKind::JwtAuthn(_) => "envoy.filters.http.jwt_authn",
        HttpFilterKind::RateLimit(_) => "envoy.filters.http.ratelimit",
        HttpFilterKind::RateLimitQuota(_) => "envoy.filters.http.rate_limit_quota",
        HttpFilterKind::HeaderMutation(_) => "envoy.filters.http.header_mutation",
        HttpFilterKind::HealthCheck(_) => "envoy.filters.http.health_check",
        HttpFilterKind::CredentialInjector(_) => "envoy.filters.http.credential_injector",
        HttpFilterKind::CustomResponse(_) => "envoy.filters.http.custom_response",
        HttpFilterKind::ExtProc(_) => "envoy.filters.http.ext_proc",
        HttpFilterKind::Mcp(_) => "envoy.filters.http.mcp",
        HttpFilterKind::Custom { .. } => "custom.http.filter",
    }
}

/// Check if this filter is the router filter
fn is_router_filter(filter: &HttpFilterKind) -> bool {
    matches!(filter, HttpFilterKind::Router)
}

/// Create an empty HTTP filter entry for the given filter name.
///
/// The filter is created with default/empty configuration, which allows
/// per-route overrides via `typed_per_filter_config` to work.
///
/// Returns `None` for filters that require valid configuration (like JWT auth,
/// local rate limit) and cannot work as empty placeholders.
///
/// Uses the unified conversion framework via `FilterType::from_http_filter_name()`
/// and `create_empty_listener_filter()`.
fn create_empty_http_filter_entry(filter_name: &str) -> Option<HttpFilterConfigEntry> {
    // Look up the filter type from the Envoy filter name
    let filter_type = match DomainFilterType::from_http_filter_name(filter_name) {
        Some(ft) => ft,
        None => {
            tracing::warn!(
                filter_name = %filter_name,
                "Unknown HTTP filter type, cannot create empty placeholder"
            );
            return None;
        }
    };

    // Use the unified conversion to create an empty filter
    let filter = create_empty_listener_filter(filter_type)?;

    Some(HttpFilterConfigEntry {
        name: Some(filter_name.to_string()),
        is_optional: false,
        disabled: false,
        filter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_listener_config(http_filters: Vec<HttpFilterConfigEntry>) -> ListenerConfig {
        ListenerConfig {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: Some("test-route".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters,
                    },
                }],
                tls_context: None,
            }],
        }
    }

    #[test]
    fn test_listener_has_http_filter_empty() {
        let config = create_test_listener_config(vec![]);
        assert!(!listener_has_http_filter(&config, "envoy.filters.http.header_mutation"));
    }

    #[test]
    fn test_listener_has_http_filter_with_router_only() {
        let config = create_test_listener_config(vec![HttpFilterConfigEntry {
            name: None,
            is_optional: false,
            disabled: false,
            filter: HttpFilterKind::Router,
        }]);
        assert!(listener_has_http_filter(&config, ROUTER_FILTER_NAME));
        assert!(!listener_has_http_filter(&config, "envoy.filters.http.header_mutation"));
    }

    #[test]
    fn test_listener_has_http_filter_with_header_mutation() {
        let config = create_test_listener_config(vec![
            HttpFilterConfigEntry {
                name: Some("envoy.filters.http.header_mutation".to_string()),
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::HeaderMutation(
                    crate::xds::filters::http::header_mutation::HeaderMutationConfig::default(),
                ),
            },
            HttpFilterConfigEntry {
                name: None,
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::Router,
            },
        ]);
        assert!(listener_has_http_filter(&config, "envoy.filters.http.header_mutation"));
        assert!(listener_has_http_filter(&config, ROUTER_FILTER_NAME));
    }

    #[test]
    fn test_add_http_filter_before_router() {
        let mut config = create_test_listener_config(vec![HttpFilterConfigEntry {
            name: None,
            is_optional: false,
            disabled: false,
            filter: HttpFilterKind::Router,
        }]);

        let added =
            add_http_filter_before_router(&mut config, "envoy.filters.http.header_mutation");
        assert!(added);

        // Verify filter was added
        assert!(listener_has_http_filter(&config, "envoy.filters.http.header_mutation"));

        // Verify filter is before router
        if let FilterType::HttpConnectionManager { http_filters, .. } =
            &config.filter_chains[0].filters[0].filter_type
        {
            assert_eq!(http_filters.len(), 2);
            let first_name = http_filters[0].name.as_deref().unwrap();
            assert_eq!(first_name, "envoy.filters.http.header_mutation");
        } else {
            panic!("Expected HttpConnectionManager");
        }
    }

    #[test]
    fn test_add_http_filter_idempotent() {
        let mut config = create_test_listener_config(vec![
            HttpFilterConfigEntry {
                name: Some("envoy.filters.http.header_mutation".to_string()),
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::HeaderMutation(
                    crate::xds::filters::http::header_mutation::HeaderMutationConfig::default(),
                ),
            },
            HttpFilterConfigEntry {
                name: None,
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::Router,
            },
        ]);

        // Try to add again - should not duplicate
        let added =
            add_http_filter_before_router(&mut config, "envoy.filters.http.header_mutation");
        assert!(!added);

        // Verify still only 2 filters
        if let FilterType::HttpConnectionManager { http_filters, .. } =
            &config.filter_chains[0].filters[0].filter_type
        {
            assert_eq!(http_filters.len(), 2);
        }
    }

    #[test]
    fn test_remove_http_filter_from_listener() {
        let mut config = create_test_listener_config(vec![
            HttpFilterConfigEntry {
                name: Some("envoy.filters.http.header_mutation".to_string()),
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::HeaderMutation(
                    crate::xds::filters::http::header_mutation::HeaderMutationConfig::default(),
                ),
            },
            HttpFilterConfigEntry {
                name: None,
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::Router,
            },
        ]);

        let removed =
            remove_http_filter_from_listener(&mut config, "envoy.filters.http.header_mutation");
        assert!(removed);

        // Verify filter was removed
        assert!(!listener_has_http_filter(&config, "envoy.filters.http.header_mutation"));

        // Verify router still exists
        assert!(listener_has_http_filter(&config, ROUTER_FILTER_NAME));

        // Verify only router remains
        if let FilterType::HttpConnectionManager { http_filters, .. } =
            &config.filter_chains[0].filters[0].filter_type
        {
            assert_eq!(http_filters.len(), 1);
        }
    }

    #[test]
    fn test_remove_nonexistent_filter() {
        let mut config = create_test_listener_config(vec![HttpFilterConfigEntry {
            name: None,
            is_optional: false,
            disabled: false,
            filter: HttpFilterKind::Router,
        }]);

        let removed =
            remove_http_filter_from_listener(&mut config, "envoy.filters.http.header_mutation");
        assert!(!removed);
    }
}
