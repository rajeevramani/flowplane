//! HTTP filter registry and builders
//!
//! This module defines a common configuration model for Envoy HTTP filters and
//! helper functions to convert REST payloads into protobuf `HttpFilter`
//! messages. Individual filters (e.g. Local Rate Limit) live in dedicated
//! submodules and register their configuration structs here.

pub mod jwt_auth;
pub mod local_rate_limit;

use crate::xds::filters::{any_from_message, invalid_config, Base64Bytes, TypedConfig};
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::local_rate_limit::LocalRateLimitConfig;
use envoy_types::pb::envoy::extensions::filters::http::router::v3::Router as RouterFilter;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType as HttpFilterConfigType;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter;
use envoy_types::pb::envoy::extensions::filters::http::local_ratelimit::v3::LocalRateLimit as LocalRateLimitProto;
use envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::PerRouteConfig as JwtPerRouteProto;
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
    /// Envoy Local Rate Limit filter
    LocalRateLimit(local_rate_limit::LocalRateLimitConfig),
    /// Envoy JWT authentication filter
    JwtAuthn(jwt_auth::JwtAuthenticationConfig),
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
            Self::LocalRateLimit(_) => "envoy.filters.http.local_ratelimit",
            Self::JwtAuthn(_) => "envoy.filters.http.jwt_authn",
            Self::Custom { .. } => "custom.http.filter",
        }
    }

    fn to_any(&self) -> Result<Option<EnvoyAny>, crate::Error> {
        match self {
            Self::Router => Ok(Some(any_from_message(
                "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router",
                &RouterFilter::default(),
            ))),
            Self::LocalRateLimit(cfg) => cfg.to_any().map(Some),
            Self::JwtAuthn(cfg) => cfg.to_any().map(Some),
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
    /// Raw typed config (type URL + base64 protobuf)
    Typed(TypedConfig),
}

impl HttpScopedConfig {
    /// Convert scoped configuration into Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        match self {
            Self::Typed(config) => Ok(config.to_any()),
            Self::LocalRateLimit(cfg) => cfg.to_any(),
            Self::JwtAuthn(cfg) => {
                let proto = cfg.to_proto()?;
                Ok(any_from_message(JWT_AUTHN_PER_ROUTE_TYPE_URL, &proto))
            }
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

        if any.type_url == JWT_AUTHN_PER_ROUTE_TYPE_URL {
            let proto = JwtPerRouteProto::decode(any.value.as_slice()).map_err(|err| {
                crate::Error::config(format!("Failed to decode JWT per-route config: {}", err))
            })?;
            let cfg = JwtPerRouteConfig::from_proto(&proto)?;
            return Ok(HttpScopedConfig::JwtAuthn(cfg));
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
        let name = entry
            .name
            .clone()
            .unwrap_or_else(|| entry.filter.default_name().to_string());

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
        assert!(matches!(err, crate::Error::Config(_)));
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
}
