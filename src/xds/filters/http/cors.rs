//! CORS HTTP filter configuration helpers

use std::convert::TryFrom;

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::RuntimeFractionalPercent;
use envoy_types::pb::envoy::config::route::v3::{
    cors_policy::EnabledSpecifier as RouteEnabledSpecifier, CorsPolicy as RouteCorsPolicy,
};
use envoy_types::pb::envoy::extensions::filters::http::cors::v3::{
    Cors as CorsFilter, CorsPolicy as FilterCorsPolicy,
};
use envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern;
use envoy_types::pb::envoy::r#type::matcher::v3::{RegexMatcher, StringMatcher};
use envoy_types::pb::envoy::r#type::v3::fractional_percent;
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, BoolValue};
use http::{header::HeaderName, Method};
use regex::Regex;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const FILTER_CORS_POLICY_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.cors.v3.CorsPolicy";
pub const ROUTE_CORS_POLICY_TYPE_URL: &str = "type.googleapis.com/envoy.config.route.v3.CorsPolicy";
pub const CORS_FILTER_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.cors.v3.Cors";
const MAX_AGE_LIMIT_SECONDS: u64 = 315_576_000_000; // 10,000 years

/// Representation of a single CORS origin match rule using Envoy's `StringMatcher`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CorsOriginMatcher {
    Exact { value: String },
    Prefix { value: String },
    Suffix { value: String },
    Contains { value: String },
    Regex { value: String },
}

impl CorsOriginMatcher {
    fn to_proto(&self) -> Result<StringMatcher, crate::Error> {
        let mut matcher = StringMatcher { ignore_case: false, ..Default::default() };

        matcher.match_pattern = Some(match self {
            CorsOriginMatcher::Exact { value } => MatchPattern::Exact(value.clone()),
            CorsOriginMatcher::Prefix { value } => MatchPattern::Prefix(value.clone()),
            CorsOriginMatcher::Suffix { value } => MatchPattern::Suffix(value.clone()),
            CorsOriginMatcher::Contains { value } => MatchPattern::Contains(value.clone()),
            CorsOriginMatcher::Regex { value } => {
                Regex::new(value).map_err(|err| {
                    invalid_config(format!("Invalid CORS regex origin matcher: {err}"))
                })?;
                MatchPattern::SafeRegex(RegexMatcher { regex: value.clone(), ..Default::default() })
            }
        });

        Ok(matcher)
    }

    fn from_proto(proto: &StringMatcher) -> Result<Self, crate::Error> {
        if proto.ignore_case {
            return Err(invalid_config(
                "CORS origin matcher does not support case-insensitive matching",
            ));
        }

        match proto
            .match_pattern
            .as_ref()
            .ok_or_else(|| invalid_config("CORS origin matcher requires a match pattern"))?
        {
            MatchPattern::Exact(value) => Ok(CorsOriginMatcher::Exact { value: value.clone() }),
            MatchPattern::Prefix(value) => Ok(CorsOriginMatcher::Prefix { value: value.clone() }),
            MatchPattern::Suffix(value) => Ok(CorsOriginMatcher::Suffix { value: value.clone() }),
            MatchPattern::Contains(value) => {
                Ok(CorsOriginMatcher::Contains { value: value.clone() })
            }
            MatchPattern::SafeRegex(regex) => {
                Ok(CorsOriginMatcher::Regex { value: regex.regex.clone() })
            }
            MatchPattern::Custom(_) => Err(invalid_config(
                "CORS origin matcher does not support custom extension matchers",
            )),
        }
    }

    fn validate(&self) -> Result<(), crate::Error> {
        match self {
            CorsOriginMatcher::Exact { value }
            | CorsOriginMatcher::Prefix { value }
            | CorsOriginMatcher::Suffix { value }
            | CorsOriginMatcher::Contains { value } => {
                if value.is_empty() {
                    return Err(invalid_config("CORS origin matcher value cannot be empty"));
                }
            }
            CorsOriginMatcher::Regex { value } => {
                if value.is_empty() {
                    return Err(invalid_config("CORS regex origin matcher value cannot be empty"));
                }
                Regex::new(value).map_err(|err| {
                    invalid_config(format!("Invalid CORS regex origin matcher: {err}"))
                })?;
            }
        }

        Ok(())
    }
}

/// Representation of Envoy's `RuntimeFractionalPercent` for enabling/shadowing behaviour.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RuntimeFractionalPercentConfig {
    /// Optional runtime key for dynamic overrides
    #[serde(default)]
    pub runtime_key: Option<String>,
    /// Numerator value for the fraction
    pub numerator: u32,
    /// Denominator specifying the unit of the numerator
    #[serde(default)]
    pub denominator: FractionalPercentDenominator,
}

impl RuntimeFractionalPercentConfig {
    fn to_proto(&self) -> RuntimeFractionalPercent {
        RuntimeFractionalPercent {
            runtime_key: self.runtime_key.clone().unwrap_or_default(),
            default_value: Some(envoy_types::pb::envoy::r#type::v3::FractionalPercent {
                numerator: self.numerator,
                denominator: self.denominator.to_proto_value(),
            }),
        }
    }

    fn from_proto(proto: &RuntimeFractionalPercent) -> Result<Self, crate::Error> {
        let default_value = proto
            .default_value
            .as_ref()
            .ok_or_else(|| invalid_config("RuntimeFractionalPercent missing default_value"))?;

        Ok(Self {
            runtime_key: if proto.runtime_key.is_empty() {
                None
            } else {
                Some(proto.runtime_key.clone())
            },
            numerator: default_value.numerator,
            denominator: FractionalPercentDenominator::from_proto_value(default_value.denominator)?,
        })
    }
}

/// Denominator options mirroring Envoy enum values.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FractionalPercentDenominator {
    Hundred,
    TenThousand,
    Million,
}

impl Default for FractionalPercentDenominator {
    fn default() -> Self {
        Self::Hundred
    }
}

impl FractionalPercentDenominator {
    fn to_proto_value(self) -> i32 {
        match self {
            Self::Hundred => fractional_percent::DenominatorType::Hundred as i32,
            Self::TenThousand => fractional_percent::DenominatorType::TenThousand as i32,
            Self::Million => fractional_percent::DenominatorType::Million as i32,
        }
    }

    fn from_proto_value(value: i32) -> Result<Self, crate::Error> {
        match fractional_percent::DenominatorType::try_from(value) {
            Ok(fractional_percent::DenominatorType::Hundred) => Ok(Self::Hundred),
            Ok(fractional_percent::DenominatorType::TenThousand) => Ok(Self::TenThousand),
            Ok(fractional_percent::DenominatorType::Million) => Ok(Self::Million),
            Err(_) => {
                Err(invalid_config(format!("Unsupported fractional percent denominator: {value}")))
            }
        }
    }
}

/// Declarative representation of Envoy's `CorsPolicy` message.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CorsPolicyConfig {
    /// String matchers describing allowed origins. Must not be empty.
    #[serde(default)]
    pub allow_origin: Vec<CorsOriginMatcher>,
    /// HTTP methods allowed for cross-origin requests.
    #[serde(default)]
    pub allow_methods: Vec<String>,
    /// Request headers allowed in cross-origin requests.
    #[serde(default)]
    pub allow_headers: Vec<String>,
    /// Response headers exposed to cross-origin clients.
    #[serde(default)]
    pub expose_headers: Vec<String>,
    /// Max age in seconds for preflight cache. Optional.
    #[serde(default)]
    pub max_age: Option<u64>,
    /// Whether credentials are allowed.
    #[serde(default)]
    pub allow_credentials: Option<bool>,
    /// Percentage of requests where filter is enforced.
    #[serde(default)]
    pub filter_enabled: Option<RuntimeFractionalPercentConfig>,
    /// Percentage of requests where policy is evaluated but not enforced.
    #[serde(default)]
    pub shadow_enabled: Option<RuntimeFractionalPercentConfig>,
    /// Allow requests targeting a more private network.
    #[serde(default)]
    pub allow_private_network_access: Option<bool>,
    /// Forward preflight requests that do not match configured origins.
    #[serde(default)]
    pub forward_not_matching_preflights: Option<bool>,
}

impl CorsPolicyConfig {
    /// Validate configuration for logical and security rules.
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.allow_origin.is_empty() {
            return Err(invalid_config(
                "CORS configuration requires at least one allowed origin matcher",
            ));
        }

        for matcher in &self.allow_origin {
            matcher.validate()?;
        }

        for method in &self.allow_methods {
            if method.trim().is_empty() {
                return Err(invalid_config("CORS allow_methods entries cannot be empty"));
            }
            if method.trim() != "*" {
                Method::from_bytes(method.trim().as_bytes()).map_err(|_| {
                    invalid_config(format!("Invalid HTTP method in CORS allow_methods: {method}"))
                })?;
            }
        }

        for header in &self.allow_headers {
            validate_header_name(header, "allow_headers")?;
        }

        for header in &self.expose_headers {
            validate_header_name(header, "expose_headers")?;
        }

        if let Some(true) = self.allow_credentials {
            let has_wildcard = self.allow_origin.iter().any(|matcher| match matcher {
                CorsOriginMatcher::Exact { value } => value.trim() == "*",
                _ => false,
            });

            if has_wildcard {
                return Err(invalid_config(
                    "CORS allow_credentials cannot be true when allow_origin contains '*'",
                ));
            }
        }

        if let Some(max_age) = self.max_age {
            if max_age > MAX_AGE_LIMIT_SECONDS {
                return Err(invalid_config(format!(
                    "CORS max_age must be <= {MAX_AGE_LIMIT_SECONDS} seconds"
                )));
            }
        }

        Ok(())
    }

    fn to_filter_proto(&self) -> Result<FilterCorsPolicy, crate::Error> {
        self.validate()?;

        let mut policy = FilterCorsPolicy {
            allow_origin_string_match: self
                .allow_origin
                .iter()
                .map(CorsOriginMatcher::to_proto)
                .collect::<Result<_, _>>()?,
            allow_methods: join_header_values(&self.allow_methods),
            allow_headers: join_header_values(&self.allow_headers),
            expose_headers: join_header_values(&self.expose_headers),
            max_age: self.max_age.map(|value| value.to_string()).unwrap_or_default(),
            ..Default::default()
        };

        if let Some(allow_credentials) = self.allow_credentials {
            policy.allow_credentials = Some(BoolValue { value: allow_credentials });
        }

        if let Some(filter_enabled) = &self.filter_enabled {
            policy.filter_enabled = Some(filter_enabled.to_proto());
        }

        if let Some(shadow_enabled) = &self.shadow_enabled {
            policy.shadow_enabled = Some(shadow_enabled.to_proto());
        }

        if let Some(allow_private_network_access) = self.allow_private_network_access {
            policy.allow_private_network_access =
                Some(BoolValue { value: allow_private_network_access });
        }

        if let Some(forward) = self.forward_not_matching_preflights {
            policy.forward_not_matching_preflights = Some(BoolValue { value: forward });
        }

        Ok(policy)
    }

    #[allow(dead_code)]
    fn to_route_proto(&self) -> Result<RouteCorsPolicy, crate::Error> {
        self.validate()?;

        let mut policy = RouteCorsPolicy {
            allow_origin_string_match: self
                .allow_origin
                .iter()
                .map(CorsOriginMatcher::to_proto)
                .collect::<Result<_, _>>()?,
            allow_methods: join_header_values(&self.allow_methods),
            allow_headers: join_header_values(&self.allow_headers),
            expose_headers: join_header_values(&self.expose_headers),
            max_age: self.max_age.map(|value| value.to_string()).unwrap_or_default(),
            ..Default::default()
        };

        if let Some(allow_credentials) = self.allow_credentials {
            policy.allow_credentials = Some(BoolValue { value: allow_credentials });
        }

        if let Some(shadow_enabled) = &self.shadow_enabled {
            policy.shadow_enabled = Some(shadow_enabled.to_proto());
        }

        if let Some(allow_private_network_access) = self.allow_private_network_access {
            policy.allow_private_network_access =
                Some(BoolValue { value: allow_private_network_access });
        }

        if let Some(forward) = self.forward_not_matching_preflights {
            policy.forward_not_matching_preflights = Some(BoolValue { value: forward });
        }

        if let Some(filter_enabled) = &self.filter_enabled {
            policy.enabled_specifier =
                Some(RouteEnabledSpecifier::FilterEnabled(filter_enabled.to_proto()));
        }

        Ok(policy)
    }

    fn from_filter_proto(proto: &FilterCorsPolicy) -> Result<Self, crate::Error> {
        let config = Self {
            allow_origin: proto
                .allow_origin_string_match
                .iter()
                .map(CorsOriginMatcher::from_proto)
                .collect::<Result<_, _>>()?,
            allow_methods: split_header_field(&proto.allow_methods),
            allow_headers: split_header_field(&proto.allow_headers),
            expose_headers: split_header_field(&proto.expose_headers),
            max_age: parse_optional_u64(proto.max_age.as_str())?,
            allow_credentials: proto.allow_credentials.as_ref().map(|value| value.value),
            filter_enabled: proto
                .filter_enabled
                .as_ref()
                .map(RuntimeFractionalPercentConfig::from_proto)
                .transpose()?,
            shadow_enabled: proto
                .shadow_enabled
                .as_ref()
                .map(RuntimeFractionalPercentConfig::from_proto)
                .transpose()?,
            allow_private_network_access: proto
                .allow_private_network_access
                .as_ref()
                .map(|value| value.value),
            forward_not_matching_preflights: proto
                .forward_not_matching_preflights
                .as_ref()
                .map(|value| value.value),
        };

        config.validate()?;
        Ok(config)
    }

    #[allow(dead_code)]
    fn from_route_proto(proto: &RouteCorsPolicy) -> Result<Self, crate::Error> {
        let config = Self {
            allow_origin: proto
                .allow_origin_string_match
                .iter()
                .map(CorsOriginMatcher::from_proto)
                .collect::<Result<_, _>>()?,
            allow_methods: split_header_field(&proto.allow_methods),
            allow_headers: split_header_field(&proto.allow_headers),
            expose_headers: split_header_field(&proto.expose_headers),
            max_age: parse_optional_u64(proto.max_age.as_str())?,
            allow_credentials: proto.allow_credentials.as_ref().map(|value| value.value),
            filter_enabled: proto
                .enabled_specifier
                .as_ref()
                .map(|specifier| match specifier {
                    RouteEnabledSpecifier::FilterEnabled(value) => value,
                })
                .map(RuntimeFractionalPercentConfig::from_proto)
                .transpose()?,
            shadow_enabled: proto
                .shadow_enabled
                .as_ref()
                .map(RuntimeFractionalPercentConfig::from_proto)
                .transpose()?,
            allow_private_network_access: proto
                .allow_private_network_access
                .as_ref()
                .map(|value| value.value),
            forward_not_matching_preflights: proto
                .forward_not_matching_preflights
                .as_ref()
                .map(|value| value.value),
        };

        config.validate()?;
        Ok(config)
    }
}

fn join_header_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn split_header_field(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

fn parse_optional_u64(value: &str) -> Result<Option<u64>, crate::Error> {
    if value.trim().is_empty() {
        return Ok(None);
    }

    let parsed = value
        .trim()
        .parse::<u64>()
        .map_err(|_| invalid_config(format!("Invalid non-numeric CORS max_age value: {value}")))?;

    if parsed > MAX_AGE_LIMIT_SECONDS {
        return Err(invalid_config(format!(
            "CORS max_age must be <= {MAX_AGE_LIMIT_SECONDS} seconds"
        )));
    }

    Ok(Some(parsed))
}

fn validate_header_name(value: &str, field: &str) -> Result<(), crate::Error> {
    if value.trim().is_empty() {
        return Err(invalid_config(format!("CORS {field} entries cannot be empty")));
    }

    if value.trim() != "*" {
        HeaderName::from_bytes(value.trim().as_bytes()).map_err(|_| {
            invalid_config(format!("Invalid header name '{value}' in CORS {field}"))
        })?;
    }

    Ok(())
}

/// Filter-level configuration wrapper (typed config on HttpFilter entries).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CorsConfig {
    pub policy: CorsPolicyConfig,
}

impl CorsConfig {
    /// Convert to filter typed config Any payload.
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let policy = self.policy.to_filter_proto()?;
        Ok(any_from_message(FILTER_CORS_POLICY_TYPE_URL, &policy))
    }

    /// Build filter configuration from proto payload.
    pub fn from_proto(proto: &FilterCorsPolicy) -> Result<Self, crate::Error> {
        Ok(Self { policy: CorsPolicyConfig::from_filter_proto(proto)? })
    }
}

/// Optional per-route override for the CORS filter.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CorsPerRouteConfig {
    pub policy: CorsPolicyConfig,
}

impl CorsPerRouteConfig {
    /// Convert to per-route typed config Any payload.
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let policy = self.policy.to_filter_proto()?;
        Ok(any_from_message(FILTER_CORS_POLICY_TYPE_URL, &policy))
    }

    /// Deserialize per-route configuration from proto.
    pub fn from_proto(proto: &FilterCorsPolicy) -> Result<Self, crate::Error> {
        Ok(Self { policy: CorsPolicyConfig::from_filter_proto(proto)? })
    }
}

/// Empty message used to enable the Envoy CORS filter in the HTTP filter chain.
pub fn filter_marker_any() -> EnvoyAny {
    any_from_message(CORS_FILTER_TYPE_URL, &CorsFilter {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_policy() -> CorsPolicyConfig {
        CorsPolicyConfig {
            allow_origin: vec![CorsOriginMatcher::Exact { value: "https://example.com".into() }],
            allow_methods: vec!["GET".into(), "POST".into()],
            allow_headers: vec!["x-request-id".into()],
            expose_headers: vec!["x-response-id".into()],
            max_age: Some(600),
            allow_credentials: Some(true),
            filter_enabled: Some(RuntimeFractionalPercentConfig {
                runtime_key: Some("cors.enabled".into()),
                numerator: 50,
                denominator: FractionalPercentDenominator::Hundred,
            }),
            shadow_enabled: Some(RuntimeFractionalPercentConfig {
                runtime_key: None,
                numerator: 10,
                denominator: FractionalPercentDenominator::Hundred,
            }),
            allow_private_network_access: Some(false),
            forward_not_matching_preflights: Some(true),
        }
    }

    #[test]
    fn validate_rejects_empty_origins() {
        let policy = CorsPolicyConfig { allow_origin: vec![], ..Default::default() };
        let err = policy.validate().expect_err("validation should fail");
        assert!(format!("{err}").contains("requires at least one allowed origin"));
    }

    #[test]
    fn validate_rejects_wildcard_credentials() {
        let policy = CorsPolicyConfig {
            allow_origin: vec![CorsOriginMatcher::Exact { value: "*".into() }],
            allow_credentials: Some(true),
            ..Default::default()
        };

        let err = policy.validate().expect_err("validation should fail");
        assert!(format!("{err}").contains("allow_credentials"));
    }

    #[test]
    fn filter_proto_round_trip() {
        let config = CorsConfig { policy: sample_policy() };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, FILTER_CORS_POLICY_TYPE_URL);

        let proto = FilterCorsPolicy::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CorsConfig::from_proto(&proto).expect("from proto");
        assert_eq!(round_tripped.policy.allow_methods, vec!["GET", "POST"]);
        assert_eq!(round_tripped.policy.max_age, Some(600));
    }

    #[test]
    fn route_proto_round_trip() {
        let config = CorsPerRouteConfig { policy: sample_policy() };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, FILTER_CORS_POLICY_TYPE_URL);

        let proto = FilterCorsPolicy::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CorsPerRouteConfig::from_proto(&proto).expect("from proto");
        assert_eq!(round_tripped.policy.allow_headers, vec!["x-request-id"]);
        assert_eq!(round_tripped.policy.forward_not_matching_preflights, Some(true));
    }

    #[test]
    fn header_validation_rejects_invalid_header() {
        let policy = CorsPolicyConfig {
            allow_origin: vec![CorsOriginMatcher::Exact { value: "https://example.com".into() }],
            allow_headers: vec!["invalid header".into()],
            ..Default::default()
        };

        let err = policy.validate().expect_err("validation should fail");
        assert!(format!("{err}").contains("Invalid header name"));
    }

    #[test]
    fn filter_marker_any_returns_expected_type_url() {
        let any = filter_marker_any();
        assert_eq!(any.type_url, CORS_FILTER_TYPE_URL);
    }
}
