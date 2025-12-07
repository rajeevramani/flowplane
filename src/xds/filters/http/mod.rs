//! HTTP filter registry and builders
//!
//! This module defines a common configuration model for Envoy HTTP filters and
//! helper functions to convert REST payloads into protobuf `HttpFilter`
//! messages. Individual filters (e.g. Local Rate Limit) live in dedicated
//! submodules and register their configuration structs here.

pub mod cors;
pub mod credential_injector;
pub mod custom_response;
pub mod ext_proc;
pub mod header_mutation;
pub mod health_check;
pub mod jwt_auth;
pub mod local_rate_limit;
pub mod mcp;
pub mod rate_limit;
pub mod rate_limit_quota;

use crate::xds::filters::http::cors::{
    CorsConfig as CorsFilterConfig, CorsPerRouteConfig, FILTER_CORS_POLICY_TYPE_URL,
};
use crate::xds::filters::http::credential_injector::CredentialInjectorConfig;
use crate::xds::filters::http::custom_response::{CustomResponseConfig, CustomResponsePerRouteConfig};
use crate::xds::filters::http::ext_proc::ExtProcConfig;
use crate::xds::filters::http::header_mutation::{HeaderMutationConfig, HeaderMutationPerRouteConfig};
use crate::xds::filters::http::health_check::HealthCheckConfig;
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::local_rate_limit::LocalRateLimitConfig;
use crate::xds::filters::http::mcp::{McpFilterConfig, McpPerRouteConfig, MCP_PER_ROUTE_TYPE_URL};
use crate::xds::filters::http::rate_limit::{RateLimitConfig, RateLimitPerRouteConfig};
use crate::xds::filters::http::rate_limit_quota::{RateLimitQuotaConfig, RateLimitQuotaOverrideConfig};
use crate::xds::filters::{any_from_message, invalid_config, Base64Bytes, TypedConfig};
use envoy_types::pb::envoy::extensions::filters::http::router::v3::Router as RouterFilter;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType as HttpFilterConfigType;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter;
use envoy_types::pb::envoy::extensions::filters::http::local_ratelimit::v3::LocalRateLimit as LocalRateLimitProto;
use envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::PerRouteConfig as JwtPerRouteProto;
use envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy as FilterCorsPolicyProto;
use envoy_types::pb::envoy::extensions::filters::http::header_mutation::v3::HeaderMutationPerRoute as HeaderMutationPerRouteProto;
use envoy_types::pb::envoy::extensions::filters::http::ratelimit::v3::RateLimitPerRoute as RateLimitPerRouteProto;
use envoy_types::pb::envoy::extensions::filters::http::rate_limit_quota::v3::RateLimitQuotaOverride;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use prost::Message;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Envoy's canonical router filter name
pub const ROUTER_FILTER_NAME: &str = "envoy.filters.http.router";
const LOCAL_RATE_LIMIT_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit";
const JWT_AUTHN_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.PerRouteConfig";
const HEADER_MUTATION_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutationPerRoute";
const RATE_LIMIT_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.ratelimit.v3.RateLimitPerRoute";
const RATE_LIMIT_QUOTA_OVERRIDE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.rate_limit_quota.v3.RateLimitQuotaOverride";
const CUSTOM_RESPONSE_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.config.route.v3.FilterConfig";

/// REST representation of an HTTP filter entry
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HttpFilterConfigEntry {
    /// Optional override for the filter name used in Envoy configuration
    #[serde(default)]
    pub name: Option<String>,
    /// Whether the filter should be marked optional in Envoy
    #[serde(default)]
    pub is_optional: bool,
    /// Whether the filter should be disabled
    #[serde(default)]
    pub disabled: bool,
    /// Filter type and configuration
    pub filter: HttpFilterKind,
}

/// Supported HTTP filter types
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HttpFilterKind {
    /// Built-in Envoy router filter
    Router,
    /// Envoy CORS filter
    Cors(CorsFilterConfig),
    /// Envoy Local Rate Limit filter
    LocalRateLimit(local_rate_limit::LocalRateLimitConfig),
    /// Envoy JWT authentication filter
    JwtAuthn(jwt_auth::JwtAuthenticationConfig),
    /// Envoy distributed Rate Limit filter
    RateLimit(RateLimitConfig),
    /// Envoy Rate Limit Quota filter
    RateLimitQuota(RateLimitQuotaConfig),
    /// Envoy Header Mutation filter
    HeaderMutation(HeaderMutationConfig),
    /// Envoy Health Check filter
    HealthCheck(HealthCheckConfig),
    /// Envoy Credential Injector filter
    CredentialInjector(CredentialInjectorConfig),
    /// Envoy Custom Response filter
    CustomResponse(CustomResponseConfig),
    /// Envoy External Processor filter
    ExtProc(ExtProcConfig),
    /// Envoy MCP (Model Context Protocol) filter for AI/LLM gateway traffic
    Mcp(McpFilterConfig),
    /// Arbitrary filter expressed as a typed config payload
    Custom {
        #[serde(flatten)]
        config: TypedConfig,
    },
}

impl HttpFilterKind {
    fn is_router(&self) -> bool {
        matches!(self, Self::Router)
    }

    fn default_name(&self) -> &'static str {
        match self {
            Self::Router => ROUTER_FILTER_NAME,
            Self::Cors(_) => "envoy.filters.http.cors",
            Self::LocalRateLimit(_) => "envoy.filters.http.local_ratelimit",
            Self::JwtAuthn(_) => "envoy.filters.http.jwt_authn",
            Self::RateLimit(_) => "envoy.filters.http.ratelimit",
            Self::RateLimitQuota(_) => "envoy.filters.http.rate_limit_quota",
            Self::HeaderMutation(_) => "envoy.filters.http.header_mutation",
            Self::HealthCheck(_) => "envoy.filters.http.health_check",
            Self::CredentialInjector(_) => "envoy.filters.http.credential_injector",
            Self::CustomResponse(_) => "envoy.filters.http.custom_response",
            Self::ExtProc(_) => "envoy.filters.http.ext_proc",
            Self::Mcp(_) => "envoy.filters.http.mcp",
            Self::Custom { .. } => "custom.http.filter",
        }
    }

    fn to_any(&self) -> Result<Option<EnvoyAny>, crate::Error> {
        match self {
            Self::Router => Ok(Some(any_from_message(
                "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router",
                &RouterFilter::default(),
            ))),
            Self::Cors(_cfg) => {
                // Validate the config but use the empty marker for the HTTP filter chain
                _cfg.policy.validate()?;
                Ok(Some(cors::filter_marker_any()))
            }
            Self::LocalRateLimit(cfg) => cfg.to_any().map(Some),
            Self::JwtAuthn(cfg) => cfg.to_any().map(Some),
            Self::RateLimit(cfg) => cfg.to_any().map(Some),
            Self::RateLimitQuota(cfg) => cfg.to_any().map(Some),
            Self::HeaderMutation(cfg) => cfg.to_any().map(Some),
            Self::HealthCheck(cfg) => cfg.to_any().map(Some),
            Self::CredentialInjector(cfg) => cfg.to_any().map(Some),
            Self::CustomResponse(cfg) => cfg.to_any().map(Some),
            Self::ExtProc(cfg) => cfg.to_any().map(Some),
            Self::Mcp(cfg) => cfg.to_any().map(Some),
            Self::Custom { config } => Ok(Some(config.to_any())),
        }
    }
}

/// Scoped configuration for HTTP filters (e.g. per-route overrides)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum HttpScopedConfig {
    /// Local Rate Limit config expressed in structured form
    LocalRateLimit(LocalRateLimitConfig),
    /// JWT auth per-route overrides
    JwtAuthn(JwtPerRouteConfig),
    /// CORS per-route policy overrides
    Cors(CorsPerRouteConfig),
    /// Header mutation per-route overrides
    HeaderMutation(HeaderMutationPerRouteConfig),
    /// Rate limit per-route overrides
    RateLimit(RateLimitPerRouteConfig),
    /// Rate limit quota per-route overrides
    RateLimitQuota(RateLimitQuotaOverrideConfig),
    /// Custom response per-route overrides
    CustomResponse(CustomResponsePerRouteConfig),
    /// MCP per-route overrides
    Mcp(McpPerRouteConfig),
    /// Raw typed config (type URL + base64 protobuf)
    Typed(TypedConfig),
}

impl HttpScopedConfig {
    /// Convert scoped configuration into Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        match self {
            Self::Typed(config) => Ok(config.to_any()),
            Self::LocalRateLimit(cfg) => cfg.to_any(),
            Self::Cors(cfg) => cfg.to_any(),
            Self::JwtAuthn(cfg) => {
                let proto = cfg.to_proto()?;
                Ok(any_from_message(JWT_AUTHN_PER_ROUTE_TYPE_URL, &proto))
            }
            Self::HeaderMutation(cfg) => cfg.to_any(),
            Self::RateLimit(cfg) => cfg.to_any(),
            Self::RateLimitQuota(cfg) => cfg.to_any(),
            Self::CustomResponse(cfg) => cfg.to_any(),
            Self::Mcp(cfg) => cfg.to_any(),
        }
    }

    /// Build scoped configuration from Envoy Any payload
    pub fn from_any(any: &EnvoyAny) -> Result<Self, crate::Error> {
        if any.type_url == LOCAL_RATE_LIMIT_TYPE_URL {
            let proto = LocalRateLimitProto::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!("Failed to decode local rate limit config: {}", err))
            })?;
            let cfg = LocalRateLimitConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::LocalRateLimit(cfg));
        }

        if any.type_url == FILTER_CORS_POLICY_TYPE_URL {
            let proto = FilterCorsPolicyProto::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!("Failed to decode CORS per-route config: {}", err))
            })?;
            let cfg = CorsPerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::Cors(cfg));
        }

        if any.type_url == JWT_AUTHN_PER_ROUTE_TYPE_URL {
            let proto = JwtPerRouteProto::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!("Failed to decode JWT per-route config: {}", err))
            })?;
            let cfg = JwtPerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::JwtAuthn(cfg));
        }

        if any.type_url == HEADER_MUTATION_PER_ROUTE_TYPE_URL {
            let proto =
                HeaderMutationPerRouteProto::decode(any.value.as_slice()).map_err(|err| {
                    crate::Error::config(format!(
                        "Failed to decode header mutation per-route config: {}",
                        err
                    ))
                })?;
            let cfg = HeaderMutationPerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::HeaderMutation(cfg));
        }

        if any.type_url == RATE_LIMIT_PER_ROUTE_TYPE_URL {
            let proto = RateLimitPerRouteProto::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!(
                    "Failed to decode rate limit per-route config: {}",
                    err
                ))
            })?;
            let cfg = RateLimitPerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::RateLimit(cfg));
        }

        if any.type_url == RATE_LIMIT_QUOTA_OVERRIDE_TYPE_URL {
            let proto = RateLimitQuotaOverride::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!(
                    "Failed to decode rate limit quota override config: {}",
                    err
                ))
            })?;
            let cfg = RateLimitQuotaOverrideConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::RateLimitQuota(cfg));
        }

        if any.type_url == CUSTOM_RESPONSE_PER_ROUTE_TYPE_URL {
            use envoy_types::pb::envoy::config::route::v3::FilterConfig;
            let proto = FilterConfig::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!(
                    "Failed to decode custom response per-route config: {}",
                    err
                ))
            })?;
            let cfg = CustomResponsePerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::CustomResponse(cfg));
        }

        if any.type_url == MCP_PER_ROUTE_TYPE_URL {
            use envoy_types::pb::envoy::config::route::v3::FilterConfig;
            let proto = FilterConfig::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!("Failed to decode MCP per-route config: {}", err))
            })?;
            let cfg = McpPerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::Mcp(cfg));
        }

        Ok(HttpScopedConfig::Typed(TypedConfig {
            type_url: any.type_url.clone(),
            value: Base64Bytes(any.value.clone()),
        }))
    }
}

/// Build ordered Envoy HTTP filter list and ensure router filter is last.
pub fn build_http_filters(
    entries: &[HttpFilterConfigEntry],
) -> Result<Vec<HttpFilter>, crate::Error> {
    let mut filters = Vec::with_capacity(entries.len().max(1));
    let mut router_filter: Option<HttpFilter> = None;

    for entry in entries {
        let name = entry.name.clone().unwrap_or_else(|| entry.filter.default_name().to_string());

        let config_any = entry.filter.to_any()?;
        let filter = HttpFilter {
            name: name.clone(),
            is_optional: entry.is_optional,
            disabled: entry.disabled,
            config_type: config_any.map(HttpFilterConfigType::TypedConfig),
        };

        if entry.filter.is_router() || name == ROUTER_FILTER_NAME {
            if router_filter.is_some() {
                return Err(invalid_config("Multiple router filters specified"));
            }
            router_filter = Some(filter);
        } else {
            filters.push(filter);
        }
    }

    // Append router filter, using default if none provided
    filters.push(router_filter.unwrap_or_else(default_router_filter));

    Ok(filters)
}

fn default_router_filter() -> HttpFilter {
    HttpFilter {
        name: ROUTER_FILTER_NAME.to_string(),
        is_optional: false,
        disabled: false,
        config_type: Some(HttpFilterConfigType::TypedConfig(any_from_message(
            "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router",
            &RouterFilter::default(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::filters::http::cors::{
        CorsOriginMatcher, CorsPerRouteConfig, CorsPolicyConfig,
    };

    #[test]
    fn router_is_appended_when_missing() {
        let filters = build_http_filters(&[]).expect("build filters");
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].name, ROUTER_FILTER_NAME);
    }

    #[test]
    fn router_must_be_unique() {
        let entries = vec![
            HttpFilterConfigEntry {
                name: None,
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::Router,
            },
            HttpFilterConfigEntry {
                name: None,
                is_optional: false,
                disabled: false,
                filter: HttpFilterKind::Router,
            },
        ];

        let err = build_http_filters(&entries).expect_err("duplicate router should fail");
        assert!(matches!(err, crate::Error::Config { .. }));
    }

    #[test]
    fn custom_filter_is_preserved() {
        let entries = vec![HttpFilterConfigEntry {
            name: Some("envoy.filters.http.custom".into()),
            is_optional: true,
            disabled: false,
            filter: HttpFilterKind::Custom {
                config: TypedConfig {
                    type_url: "type.googleapis.com/test.Custom".into(),
                    value: crate::xds::filters::Base64Bytes(vec![1, 2, 3]),
                },
            },
        }];

        let filters = build_http_filters(&entries).expect("build filters");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].name, "envoy.filters.http.custom");
        assert!(filters[0].is_optional);
        assert_eq!(filters[1].name, ROUTER_FILTER_NAME);
    }

    #[test]
    fn cors_filter_emits_expected_typed_config() {
        let policy = CorsPolicyConfig {
            allow_origin: vec![CorsOriginMatcher::Exact { value: "https://example.com".into() }],
            ..Default::default()
        };

        let entries = vec![HttpFilterConfigEntry {
            name: None,
            is_optional: false,
            disabled: false,
            filter: HttpFilterKind::Cors(CorsFilterConfig { policy }),
        }];

        let filters = build_http_filters(&entries).expect("build filters");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].name, "envoy.filters.http.cors");

        let typed = filters[0]
            .config_type
            .as_ref()
            .and_then(|config| match config {
                HttpFilterConfigType::TypedConfig(any) => Some(any),
                _ => None,
            })
            .expect("typed config present");

        // CORS filter uses an empty marker in the HTTP filter chain
        assert_eq!(typed.type_url, crate::xds::filters::http::cors::CORS_FILTER_TYPE_URL);
    }

    #[test]
    fn jwt_per_route_round_trip() {
        let scoped = HttpScopedConfig::JwtAuthn(JwtPerRouteConfig::RequirementName {
            requirement_name: "primary".into(),
        });

        let any = scoped.to_any().expect("to_any");
        assert_eq!(any.type_url, JWT_AUTHN_PER_ROUTE_TYPE_URL);

        let restored = HttpScopedConfig::from_any(&any).expect("from_any");
        match restored {
            HttpScopedConfig::JwtAuthn(JwtPerRouteConfig::RequirementName { requirement_name }) => {
                assert_eq!(requirement_name, "primary");
            }
            other => panic!("unexpected scoped config: {:?}", other),
        }
    }

    #[test]
    fn cors_scoped_round_trip() {
        let scoped = HttpScopedConfig::Cors(CorsPerRouteConfig {
            policy: CorsPolicyConfig {
                allow_origin: vec![CorsOriginMatcher::Exact {
                    value: "https://service.example.com".into(),
                }],
                allow_methods: vec!["GET".into()],
                ..Default::default()
            },
        });

        let any = scoped.to_any().expect("to_any");
        assert_eq!(any.type_url, FILTER_CORS_POLICY_TYPE_URL);

        let restored = HttpScopedConfig::from_any(&any).expect("from_any");
        match restored {
            HttpScopedConfig::Cors(config) => {
                assert_eq!(config.policy.allow_methods, vec!["GET"]);
            }
            other => panic!("unexpected scoped config: {:?}", other),
        }
    }

    #[test]
    fn cors_filter_and_route_config_use_correct_types() {
        // Test that CORS filter chain uses empty Cors marker
        let policy = CorsPolicyConfig {
            allow_origin: vec![CorsOriginMatcher::Exact { value: "*".into() }],
            allow_methods: vec!["GET".into(), "POST".into()],
            allow_headers: vec!["content-type".into()],
            max_age: Some(3600),
            ..Default::default()
        };

        // 1. Test HTTP filter chain produces empty Cors marker
        let filter_entry = HttpFilterConfigEntry {
            name: None,
            is_optional: false,
            disabled: false,
            filter: HttpFilterKind::Cors(CorsFilterConfig { policy: policy.clone() }),
        };

        let filters = build_http_filters(&[filter_entry]).expect("build filters");
        let typed = filters[0]
            .config_type
            .as_ref()
            .and_then(|config| match config {
                HttpFilterConfigType::TypedConfig(any) => Some(any),
                _ => None,
            })
            .expect("typed config present");

        // Should use empty Cors marker type URL
        assert_eq!(typed.type_url, crate::xds::filters::http::cors::CORS_FILTER_TYPE_URL);

        // 2. Test route-level config uses CorsPolicy with correct type URL
        let route_config = HttpScopedConfig::Cors(CorsPerRouteConfig { policy });
        let route_any = route_config.to_any().expect("to_any");

        // Should use CorsPolicy type URL for route-level config
        assert_eq!(route_any.type_url, FILTER_CORS_POLICY_TYPE_URL);

        // Verify it can be decoded back
        let restored = HttpScopedConfig::from_any(&route_any).expect("from_any");
        match restored {
            HttpScopedConfig::Cors(config) => {
                assert_eq!(config.policy.allow_methods, vec!["GET", "POST"]);
                assert_eq!(config.policy.max_age, Some(3600));
            }
            other => panic!("unexpected scoped config: {:?}", other),
        }
    }

    #[test]
    fn header_mutation_scoped_round_trip() {
        use crate::xds::filters::http::header_mutation::HeaderMutationEntry;

        let scoped = HttpScopedConfig::HeaderMutation(HeaderMutationPerRouteConfig {
            request_headers_to_add: vec![HeaderMutationEntry {
                key: "x-route-custom".into(),
                value: "custom-val".into(),
                append: true,
            }],
            request_headers_to_remove: vec!["x-remove".into()],
            response_headers_to_add: Vec::new(),
            response_headers_to_remove: vec!["server".into()],
        });

        let any = scoped.to_any().expect("to_any");
        assert_eq!(any.type_url, HEADER_MUTATION_PER_ROUTE_TYPE_URL);

        let restored = HttpScopedConfig::from_any(&any).expect("from_any");
        match restored {
            HttpScopedConfig::HeaderMutation(config) => {
                assert_eq!(config.request_headers_to_add.len(), 1);
                assert_eq!(config.request_headers_to_add[0].key, "x-route-custom");
                assert!(config.request_headers_to_add[0].append);
                assert_eq!(config.request_headers_to_remove, vec!["x-remove"]);
                assert_eq!(config.response_headers_to_remove, vec!["server"]);
            }
            other => panic!("unexpected scoped config: {:?}", other),
        }
    }

    #[test]
    fn rate_limit_scoped_round_trip() {
        let scoped = HttpScopedConfig::RateLimit(RateLimitPerRouteConfig {
            domain: Some("route-ratelimit-domain".into()),
            include_vh_rate_limits: false,
        });

        let any = scoped.to_any().expect("to_any");
        assert_eq!(any.type_url, RATE_LIMIT_PER_ROUTE_TYPE_URL);

        let restored = HttpScopedConfig::from_any(&any).expect("from_any");
        match restored {
            HttpScopedConfig::RateLimit(config) => {
                assert_eq!(config.domain, Some("route-ratelimit-domain".into()));
                assert!(!config.include_vh_rate_limits);
            }
            other => panic!("unexpected scoped config: {:?}", other),
        }
    }

    #[test]
    fn rate_limit_quota_scoped_round_trip() {
        let scoped = HttpScopedConfig::RateLimitQuota(RateLimitQuotaOverrideConfig {
            domain: "quota-override-domain".into(),
        });

        let any = scoped.to_any().expect("to_any");
        assert_eq!(any.type_url, RATE_LIMIT_QUOTA_OVERRIDE_TYPE_URL);

        let restored = HttpScopedConfig::from_any(&any).expect("from_any");
        match restored {
            HttpScopedConfig::RateLimitQuota(config) => {
                assert_eq!(config.domain, "quota-override-domain");
            }
            other => panic!("unexpected scoped config: {:?}", other),
        }
    }
}
