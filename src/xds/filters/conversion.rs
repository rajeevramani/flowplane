//! Unified filter configuration conversion framework.
//!
//! This module provides a unified interface for converting filter configurations
//! to Envoy protobuf messages. It eliminates scattered match statements across
//! the codebase by providing a single trait that each filter implements.
//!
//! # Architecture
//!
//! The framework consists of:
//! - `FilterConfigConversion` trait - defines conversion methods for filters
//! - Unified dispatcher methods on `FilterConfig` enum
//! - Helper functions for creating empty listener filters
//!
//! # Adding a New Filter
//!
//! To add a new filter:
//! 1. Add variant to `FilterType` and `FilterConfig` enums
//! 2. Add metadata entry in `filter_registry()` function
//! 3. Create filter config module in `src/xds/filters/http/`
//! 4. Implement `to_listener_any()` and `to_per_route_any()` methods
//! 5. Add match arm in `FilterConfig` dispatcher methods

use crate::domain::{FilterConfig, FilterType, PerRouteBehavior};
use crate::xds::filters::http::custom_response::CustomResponsePerRouteConfig;
use crate::xds::filters::http::header_mutation::{
    HeaderMutationEntry, HeaderMutationPerRouteConfig,
};
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::HttpScopedConfig;
use crate::Result;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;

impl FilterConfig {
    /// Convert filter configuration to Envoy Any payload for listener-level injection.
    ///
    /// This method converts the high-level filter configuration to an Envoy protobuf
    /// message suitable for inclusion in the HTTP connection manager filter chain.
    ///
    /// # Returns
    ///
    /// - `Ok(EnvoyAny)` - The protobuf message for the listener filter chain
    /// - `Err(Error)` - If the configuration is invalid or conversion fails
    pub fn to_listener_any(&self) -> Result<EnvoyAny> {
        match self {
            FilterConfig::HeaderMutation(config) => {
                use crate::xds::filters::http::header_mutation::HeaderMutationConfig;
                let hm_config = HeaderMutationConfig {
                    request_headers_to_add: config
                        .request_headers_to_add
                        .iter()
                        .map(|e| HeaderMutationEntry {
                            key: e.key.clone(),
                            value: e.value.clone(),
                            append: e.append,
                        })
                        .collect(),
                    request_headers_to_remove: config.request_headers_to_remove.clone(),
                    response_headers_to_add: config
                        .response_headers_to_add
                        .iter()
                        .map(|e| HeaderMutationEntry {
                            key: e.key.clone(),
                            value: e.value.clone(),
                            append: e.append,
                        })
                        .collect(),
                    response_headers_to_remove: config.response_headers_to_remove.clone(),
                };
                hm_config.to_any()
            }
            FilterConfig::JwtAuth(config) => config.to_any(),
            FilterConfig::LocalRateLimit(config) => config.to_any(),
            FilterConfig::CustomResponse(config) => config.to_any(),
            FilterConfig::Mcp(config) => config.to_any(),
        }
    }

    /// Convert filter configuration to Envoy Any payload for per-route injection.
    ///
    /// This method converts the filter configuration to an appropriate per-route
    /// configuration suitable for `typed_per_filter_config`. The behavior varies
    /// by filter type according to `PerRouteBehavior`:
    ///
    /// - `FullConfig`: Returns full configuration override
    /// - `ReferenceOnly`: Returns reference to listener-level config
    /// - `DisableOnly`: Returns disabled flag configuration
    /// - `NotSupported`: Returns `None`
    ///
    /// # Returns
    ///
    /// - `Ok(Some((filter_name, Any)))` - The per-route config to inject
    /// - `Ok(None)` - If this filter type doesn't support per-route config
    /// - `Err(Error)` - If configuration is invalid
    pub fn to_per_route_config(&self) -> Result<Option<(String, HttpScopedConfig)>> {
        let filter_type = self.filter_type();
        let metadata = filter_type.metadata();

        // Check if per-route is supported
        if matches!(metadata.per_route_behavior, PerRouteBehavior::NotSupported) {
            return Ok(None);
        }

        match self {
            FilterConfig::HeaderMutation(hm_config) => {
                let per_route = HeaderMutationPerRouteConfig {
                    request_headers_to_add: hm_config
                        .request_headers_to_add
                        .iter()
                        .map(|e| HeaderMutationEntry {
                            key: e.key.clone(),
                            value: e.value.clone(),
                            append: e.append,
                        })
                        .collect(),
                    request_headers_to_remove: hm_config.request_headers_to_remove.clone(),
                    response_headers_to_add: hm_config
                        .response_headers_to_add
                        .iter()
                        .map(|e| HeaderMutationEntry {
                            key: e.key.clone(),
                            value: e.value.clone(),
                            append: e.append,
                        })
                        .collect(),
                    response_headers_to_remove: hm_config.response_headers_to_remove.clone(),
                };

                Ok(Some((
                    metadata.http_filter_name.to_string(),
                    HttpScopedConfig::HeaderMutation(per_route),
                )))
            }
            FilterConfig::JwtAuth(jwt_config) => {
                // JWT uses requirement_name to reference listener-level config
                let per_route = jwt_config
                    .providers
                    .keys()
                    .next()
                    .map(|name| JwtPerRouteConfig::RequirementName {
                        requirement_name: name.clone(),
                    })
                    .unwrap_or(JwtPerRouteConfig::Disabled { disabled: true });

                Ok(Some((
                    metadata.http_filter_name.to_string(),
                    HttpScopedConfig::JwtAuthn(per_route),
                )))
            }
            FilterConfig::LocalRateLimit(config) => Ok(Some((
                metadata.http_filter_name.to_string(),
                HttpScopedConfig::LocalRateLimit(config.clone()),
            ))),
            FilterConfig::CustomResponse(config) => {
                // CustomResponse now supports full per-route matchers
                let per_route = CustomResponsePerRouteConfig::from_listener_config(config);
                Ok(Some((
                    metadata.http_filter_name.to_string(),
                    HttpScopedConfig::CustomResponse(per_route),
                )))
            }
            FilterConfig::Mcp(_config) => {
                // MCP only supports disable-only per-route config
                // When attached at route level, we don't want to disable it
                // So we skip injection (the listener handles the behavior)
                Ok(None)
            }
        }
    }
}

/// Create an empty listener HTTP filter entry for the given filter type.
///
/// This creates a minimal filter configuration that serves as a placeholder
/// in the HCM filter chain, allowing per-route overrides via `typed_per_filter_config`.
///
/// Returns `None` for filters that require valid configuration (like JWT auth,
/// local rate limit) and cannot work as empty placeholders.
///
/// # Arguments
///
/// * `filter_type` - The type of filter to create
///
/// # Returns
///
/// - `Some(HttpFilterKind)` - An empty filter configuration
/// - `None` - If this filter type cannot be created as an empty placeholder
pub fn create_empty_listener_filter(
    filter_type: FilterType,
) -> Option<crate::xds::filters::http::HttpFilterKind> {
    use crate::xds::filters::http::custom_response::CustomResponseConfig;
    use crate::xds::filters::http::header_mutation::HeaderMutationConfig;
    use crate::xds::filters::http::mcp::McpFilterConfig;
    use crate::xds::filters::http::HttpFilterKind;

    let metadata = filter_type.metadata();

    // Filters that require listener-level config cannot be empty placeholders
    // except for HeaderMutation which works as pass-through when empty
    if metadata.requires_listener_config && filter_type != FilterType::HeaderMutation {
        tracing::debug!(
            filter_name = %metadata.http_filter_name,
            "Filter requires listener-attached config, skipping empty placeholder"
        );
        return None;
    }

    match filter_type {
        FilterType::HeaderMutation => {
            Some(HttpFilterKind::HeaderMutation(HeaderMutationConfig::default()))
        }
        FilterType::CustomResponse => {
            Some(HttpFilterKind::CustomResponse(CustomResponseConfig::default()))
        }
        FilterType::Mcp => Some(HttpFilterKind::Mcp(McpFilterConfig::default())),
        FilterType::Cors => {
            use crate::xds::filters::http::cors::{CorsConfig, CorsPolicyConfig};
            Some(HttpFilterKind::Cors(CorsConfig { policy: CorsPolicyConfig::default() }))
        }
        // These filters require valid configuration
        FilterType::JwtAuth
        | FilterType::LocalRateLimit
        | FilterType::RateLimit
        | FilterType::ExtAuthz => {
            tracing::debug!(
                filter_name = %metadata.http_filter_name,
                "Filter requires valid configuration, cannot create empty placeholder"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{HeaderMutationEntry as DomainHeaderEntry, HeaderMutationFilterConfig};
    use crate::xds::filters::http::custom_response::{
        CustomResponseConfig, LocalResponsePolicy, ResponseMatcherRule, StatusCodeMatcher,
    };
    use crate::xds::filters::http::jwt_auth::{
        JwtAuthenticationConfig, JwtJwksSourceConfig, JwtProviderConfig, LocalJwksConfig,
    };
    use crate::xds::filters::http::local_rate_limit::{LocalRateLimitConfig, TokenBucketConfig};
    use crate::xds::filters::http::mcp::McpFilterConfig;
    use std::collections::HashMap;

    #[test]
    fn test_header_mutation_to_listener_any() {
        let config = FilterConfig::HeaderMutation(HeaderMutationFilterConfig {
            request_headers_to_add: vec![DomainHeaderEntry {
                key: "X-Test".to_string(),
                value: "value".to_string(),
                append: false,
            }],
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        });

        let result = config.to_listener_any();
        assert!(result.is_ok());
        let any = result.unwrap();
        assert!(any.type_url.contains("header_mutation"));
    }

    #[test]
    fn test_header_mutation_to_per_route_config() {
        let config = FilterConfig::HeaderMutation(HeaderMutationFilterConfig {
            request_headers_to_add: vec![DomainHeaderEntry {
                key: "X-Test".to_string(),
                value: "value".to_string(),
                append: false,
            }],
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        });

        let result = config.to_per_route_config();
        assert!(result.is_ok());
        let opt = result.unwrap();
        assert!(opt.is_some());
        let (name, scoped) = opt.unwrap();
        assert_eq!(name, "envoy.filters.http.header_mutation");
        assert!(matches!(scoped, HttpScopedConfig::HeaderMutation(_)));
    }

    #[test]
    fn test_jwt_auth_to_per_route_config() {
        let mut providers = HashMap::new();
        providers.insert(
            "test-provider".to_string(),
            JwtProviderConfig {
                issuer: Some("https://issuer.example.com".to_string()),
                jwks: JwtJwksSourceConfig::Local(LocalJwksConfig {
                    inline_string: Some("{\"keys\":[]}".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let config =
            FilterConfig::JwtAuth(JwtAuthenticationConfig { providers, ..Default::default() });

        let result = config.to_per_route_config();
        assert!(result.is_ok());
        let opt = result.unwrap();
        assert!(opt.is_some());
        let (name, scoped) = opt.unwrap();
        assert_eq!(name, "envoy.filters.http.jwt_authn");
        match scoped {
            HttpScopedConfig::JwtAuthn(JwtPerRouteConfig::RequirementName { requirement_name }) => {
                assert_eq!(requirement_name, "test-provider");
            }
            _ => panic!("Expected JwtAuthn RequirementName"),
        }
    }

    #[test]
    fn test_local_rate_limit_to_per_route_config() {
        let config = FilterConfig::LocalRateLimit(LocalRateLimitConfig {
            stat_prefix: "test".to_string(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 100,
                tokens_per_fill: Some(50),
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

        let result = config.to_per_route_config();
        assert!(result.is_ok());
        let opt = result.unwrap();
        assert!(opt.is_some());
        let (name, scoped) = opt.unwrap();
        assert_eq!(name, "envoy.filters.http.local_ratelimit");
        assert!(matches!(scoped, HttpScopedConfig::LocalRateLimit(_)));
    }

    #[test]
    fn test_custom_response_to_per_route_config() {
        let config = FilterConfig::CustomResponse(CustomResponseConfig {
            matchers: vec![ResponseMatcherRule {
                status_code: StatusCodeMatcher::Exact { code: 429 },
                response: LocalResponsePolicy::json_error(429, "rate limited"),
            }],
            custom_response_matcher: None,
        });

        let result = config.to_per_route_config();
        assert!(result.is_ok());
        let opt = result.unwrap();
        assert!(opt.is_some());
        let (name, scoped) = opt.unwrap();
        assert_eq!(name, "envoy.filters.http.custom_response");
        assert!(matches!(scoped, HttpScopedConfig::CustomResponse(_)));
    }

    #[test]
    fn test_mcp_to_per_route_config_returns_none() {
        let config = FilterConfig::Mcp(McpFilterConfig::default());

        let result = config.to_per_route_config();
        assert!(result.is_ok());
        // MCP per-route is None because it's DisableOnly behavior
        // and we don't want to inject disabled config
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_create_empty_listener_filter_header_mutation() {
        let result = create_empty_listener_filter(FilterType::HeaderMutation);
        assert!(result.is_some());
    }

    #[test]
    fn test_create_empty_listener_filter_jwt_returns_none() {
        let result = create_empty_listener_filter(FilterType::JwtAuth);
        assert!(result.is_none());
    }

    #[test]
    fn test_create_empty_listener_filter_local_rate_limit_returns_none() {
        let result = create_empty_listener_filter(FilterType::LocalRateLimit);
        assert!(result.is_none());
    }
}
