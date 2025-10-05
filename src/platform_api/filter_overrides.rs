use std::collections::HashMap;

use serde_json::Value;

use crate::errors::Error;
use crate::xds::filters::http::cors::{
    CorsOriginMatcher, CorsPerRouteConfig, CorsPolicyConfig, FractionalPercentDenominator,
    RuntimeFractionalPercentConfig,
};
use crate::xds::filters::http::custom_response::CustomResponsePerRouteConfig;
use crate::xds::filters::http::header_mutation::HeaderMutationPerRouteConfig;
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::rate_limit::RateLimitPerRouteConfig;
use crate::xds::filters::http::rate_limit_quota::RateLimitQuotaOverrideConfig;
use crate::xds::filters::http::{local_rate_limit::LocalRateLimitConfig, HttpScopedConfig};

const CORS_FILTER_NAME: &str = "envoy.filters.http.cors";
const JWT_FILTER_NAME: &str = "envoy.filters.http.jwt_authn";
const LOCAL_RATE_LIMIT_FILTER_NAME: &str = "envoy.filters.http.local_ratelimit";
const HEADER_MUTATION_FILTER_NAME: &str = "envoy.filters.http.header_mutation";
const RATE_LIMIT_FILTER_NAME: &str = "envoy.filters.http.ratelimit";
const RATE_LIMIT_QUOTA_FILTER_NAME: &str = "envoy.filters.http.rate_limit_quota";
const CUSTOM_RESPONSE_FILTER_NAME: &str = "envoy.filters.http.custom_response";

/// Validate filter overrides without mutating the payload.
pub fn validate_filter_overrides(filters: &Option<Value>) -> Result<(), Error> {
    parse_filter_overrides(filters).map(|_| ())
}

/// Convert filter overrides into a canonical JSON representation suitable for persistence.
pub fn canonicalize_filter_overrides(filters: Option<Value>) -> Result<Option<Value>, Error> {
    let entries = parse_filter_overrides(&filters)?;
    if entries.is_empty() {
        return Ok(None);
    }

    let mut canonical = serde_json::Map::new();
    for (alias, scoped) in entries {
        let value = serde_json::to_value(&scoped).map_err(|err| {
            Error::validation(format!("failed to serialize filter override '{alias}': {err}"))
        })?;
        canonical.insert(alias, value);
    }

    Ok(Some(Value::Object(canonical)))
}

/// Build the typed-per-filter configuration map used when constructing Envoy Route rules.
pub fn typed_per_filter_config(
    filters: &Option<Value>,
) -> Result<HashMap<String, HttpScopedConfig>, Error> {
    let entries = parse_filter_overrides(filters)?;
    let mut map = HashMap::new();
    for (alias, scoped) in entries {
        let filter_name = match alias.as_str() {
            "cors" => CORS_FILTER_NAME,
            "jwt_authn" | "authn" => JWT_FILTER_NAME, // Support both for backward compatibility
            "rate_limit" => LOCAL_RATE_LIMIT_FILTER_NAME,
            "header_mutation" => HEADER_MUTATION_FILTER_NAME,
            "ratelimit" => RATE_LIMIT_FILTER_NAME, // Distributed rate limit
            "rate_limit_quota" => RATE_LIMIT_QUOTA_FILTER_NAME,
            "custom_response" => CUSTOM_RESPONSE_FILTER_NAME,
            // Allow callers to specify a fully-qualified filter name directly.
            other if other.contains('.') => other,
            other => {
                return Err(Error::validation(format!(
                    "Unsupported filter override alias '{}'; expected 'cors', 'jwt_authn', 'rate_limit', 'header_mutation', 'ratelimit', 'rate_limit_quota', 'custom_response', or a fully qualified filter name",
                    other
                )));
            }
        };
        map.insert(filter_name.to_string(), scoped);
    }
    Ok(map)
}

fn parse_filter_overrides(
    filters: &Option<Value>,
) -> Result<Vec<(String, HttpScopedConfig)>, Error> {
    let Some(value) = filters else {
        return Ok(Vec::new());
    };

    let obj = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Object(map) => map,
        _ => {
            return Err(Error::validation(
                "filters override must be a JSON object keyed by filter name",
            ));
        }
    };

    let mut entries = Vec::with_capacity(obj.len());
    for (alias, raw) in obj {
        let scoped = match alias.as_str() {
            "cors" => {
                // parse_cors_override returns None for "disabled"
                if let Some(cfg) = parse_cors_override(raw)? {
                    Some(HttpScopedConfig::Cors(cfg))
                } else {
                    // "disabled" - skip this filter (don't add to typed_per_filter_config)
                    continue;
                }
            }
            "jwt_authn" | "authn" => Some(HttpScopedConfig::JwtAuthn(parse_authn_override(raw)?)),
            "rate_limit" => {
                let cfg: LocalRateLimitConfig =
                    serde_json::from_value(raw.clone()).map_err(|err| {
                        Error::validation(format!("Invalid local rate limit override: {err}"))
                    })?;
                Some(HttpScopedConfig::LocalRateLimit(cfg))
            }
            "header_mutation" => {
                let cfg: HeaderMutationPerRouteConfig = serde_json::from_value(raw.clone())
                    .map_err(|err| {
                        Error::validation(format!("Invalid header mutation override: {err}"))
                    })?;
                Some(HttpScopedConfig::HeaderMutation(cfg))
            }
            "ratelimit" => {
                let cfg: RateLimitPerRouteConfig =
                    serde_json::from_value(raw.clone()).map_err(|err| {
                        Error::validation(format!("Invalid rate limit override: {err}"))
                    })?;
                Some(HttpScopedConfig::RateLimit(cfg))
            }
            "rate_limit_quota" => {
                let cfg: RateLimitQuotaOverrideConfig = serde_json::from_value(raw.clone())
                    .map_err(|err| {
                        Error::validation(format!("Invalid rate limit quota override: {err}"))
                    })?;
                Some(HttpScopedConfig::RateLimitQuota(cfg))
            }
            "custom_response" => {
                // Support "disabled" string or structured config
                if let Value::String(s) = raw {
                    if s.trim().eq_ignore_ascii_case("disabled") {
                        Some(HttpScopedConfig::CustomResponse(CustomResponsePerRouteConfig {
                            disabled: true,
                        }))
                    } else {
                        return Err(Error::validation(
                            "Invalid custom_response override: expected 'disabled' or object".to_string()
                        ));
                    }
                } else {
                    let cfg: CustomResponsePerRouteConfig = serde_json::from_value(raw.clone())
                        .map_err(|err| {
                            Error::validation(format!("Invalid custom response override: {err}"))
                        })?;
                    Some(HttpScopedConfig::CustomResponse(cfg))
                }
            }
            other if other.contains('.') => {
                Some(HttpScopedConfig::Typed(parse_typed_override(raw)?))
            }
            other => {
                return Err(Error::validation(format!("Unsupported filter override '{}'", other)));
            }
        };
        if let Some(cfg) = scoped {
            entries.push((alias.clone(), cfg));
        }
    }

    Ok(entries)
}

fn parse_cors_override(value: &Value) -> Result<Option<CorsPerRouteConfig>, Error> {
    let config = match value {
        Value::String(template) => {
            let trimmed = template.trim();
            if trimmed.eq_ignore_ascii_case("disabled") {
                // Returning None means "don't include this filter in typed_per_filter_config"
                return Ok(None);
            } else if trimmed == "allow-authenticated" {
                Some(cors_allow_authenticated_template())
            } else {
                return Err(Error::validation(format!(
                    "Unknown CORS template '{}'. Supported templates: disabled, allow-authenticated",
                    template
                )));
            }
        }
        Value::Object(_) => Some(
            serde_json::from_value::<CorsPerRouteConfig>(value.clone())
                .map_err(|err| Error::validation(format!("Invalid CORS override: {err}")))?,
        ),
        _ => {
            return Err(Error::validation(
                "CORS override must be a string template or structured object",
            ));
        }
    };

    if let Some(cfg) = &config {
        cfg.policy.validate().map_err(|err| Error::validation(err.to_string()))?;
    }
    Ok(config)
}

fn parse_authn_override(value: &Value) -> Result<JwtPerRouteConfig, Error> {
    match value {
        Value::String(template) => {
            let trimmed = template.trim();
            if trimmed.eq_ignore_ascii_case("disabled") {
                Ok(JwtPerRouteConfig::Disabled { disabled: true })
            } else if trimmed.is_empty() {
                Err(Error::validation("authn override template cannot be empty"))
            } else {
                Ok(JwtPerRouteConfig::RequirementName { requirement_name: trimmed.to_string() })
            }
        }
        Value::Object(_) => serde_json::from_value::<JwtPerRouteConfig>(value.clone())
            .map_err(|err| Error::validation(format!("Invalid authn override: {err}"))),
        _ => {
            Err(Error::validation("authn override must be a string template or structured object"))
        }
    }
}

fn parse_typed_override(value: &Value) -> Result<crate::xds::filters::TypedConfig, Error> {
    serde_json::from_value::<crate::xds::filters::TypedConfig>(value.clone())
        .map_err(|err| Error::validation(format!("Invalid typed override configuration: {err}")))
}

fn cors_allow_authenticated_template() -> CorsPerRouteConfig {
    let policy = CorsPolicyConfig {
        allow_origin: vec![CorsOriginMatcher::Regex { value: "^https://.+$".into() }],
        allow_methods: vec![
            "GET".into(),
            "POST".into(),
            "PUT".into(),
            "PATCH".into(),
            "DELETE".into(),
            "OPTIONS".into(),
        ],
        allow_headers: vec![
            "Authorization".into(),
            "Content-Type".into(),
            "X-Requested-With".into(),
        ],
        expose_headers: vec!["X-Request-Id".into()],
        max_age: Some(86400),
        allow_credentials: Some(true),
        filter_enabled: Some(RuntimeFractionalPercentConfig {
            runtime_key: Some("cors.platform_api.enabled".into()),
            numerator: 100,
            denominator: FractionalPercentDenominator::Hundred,
        }),
        shadow_enabled: Some(RuntimeFractionalPercentConfig {
            runtime_key: Some("cors.platform_api.shadow".into()),
            numerator: 0,
            denominator: FractionalPercentDenominator::Hundred,
        }),
        ..Default::default()
    };

    CorsPerRouteConfig { policy }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalizes_cors_template() {
        let filters = json!({ "cors": "allow-authenticated" });
        let canonical = canonicalize_filter_overrides(Some(filters)).expect("canonicalize");
        let canonical_value = canonical.expect("some value");
        let obj = canonical_value.as_object().expect("object");
        assert!(obj.contains_key("cors"));
    }

    #[test]
    fn rejects_unknown_alias() {
        let filters = json!({ "rate-limit": {"foo": "bar"} });
        let err = canonicalize_filter_overrides(Some(filters)).expect_err("should fail");
        assert!(format!("{err}").contains("Unsupported filter override"));
    }

    #[test]
    fn builds_typed_map_with_actual_filter_names() {
        let filters = json!({ "authn": "oidc-default" });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert!(map.contains_key(JWT_FILTER_NAME));
    }

    #[test]
    fn header_mutation_override_parses_correctly() {
        let filters = json!({
            "header_mutation": {
                "request_headers_to_add": [
                    {"key": "x-custom", "value": "test", "append": false}
                ],
                "request_headers_to_remove": ["x-old"],
                "response_headers_to_add": [],
                "response_headers_to_remove": []
            }
        });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert!(map.contains_key(HEADER_MUTATION_FILTER_NAME));

        match map.get(HEADER_MUTATION_FILTER_NAME) {
            Some(HttpScopedConfig::HeaderMutation(cfg)) => {
                assert_eq!(cfg.request_headers_to_add.len(), 1);
                assert_eq!(cfg.request_headers_to_add[0].key, "x-custom");
                assert_eq!(cfg.request_headers_to_remove, vec!["x-old"]);
            }
            _ => panic!("Expected HeaderMutation config"),
        }
    }

    #[test]
    fn ratelimit_override_parses_correctly() {
        let filters = json!({
            "ratelimit": {
                "stage": 0,
                "disable_key": "disabled"
            }
        });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert!(map.contains_key(RATE_LIMIT_FILTER_NAME));

        match map.get(RATE_LIMIT_FILTER_NAME) {
            Some(HttpScopedConfig::RateLimit(_cfg)) => {
                // Successfully parsed distributed rate limit config
            }
            _ => panic!("Expected RateLimit config"),
        }
    }

    #[test]
    fn rate_limit_quota_override_parses_correctly() {
        let filters = json!({
            "rate_limit_quota": {
                "domain": "test-domain"
            }
        });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert!(map.contains_key(RATE_LIMIT_QUOTA_FILTER_NAME));

        match map.get(RATE_LIMIT_QUOTA_FILTER_NAME) {
            Some(HttpScopedConfig::RateLimitQuota(cfg)) => {
                assert_eq!(cfg.domain, "test-domain");
            }
            _ => panic!("Expected RateLimitQuota config"),
        }
    }

    #[test]
    fn all_new_filter_aliases_work_together() {
        let filters = json!({
            "header_mutation": {
                "request_headers_to_add": [{"key": "x-test", "value": "1", "append": false}],
                "request_headers_to_remove": [],
                "response_headers_to_add": [],
                "response_headers_to_remove": []
            },
            "ratelimit": {
                "stage": 0
            },
            "rate_limit_quota": {
                "domain": "combined-test-domain"
            }
        });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert_eq!(map.len(), 3);
        assert!(map.contains_key(HEADER_MUTATION_FILTER_NAME));
        assert!(map.contains_key(RATE_LIMIT_FILTER_NAME));
        assert!(map.contains_key(RATE_LIMIT_QUOTA_FILTER_NAME));
    }

    #[test]
    fn custom_response_override_disabled_string() {
        let filters = json!({
            "custom_response": "disabled"
        });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert!(map.contains_key(CUSTOM_RESPONSE_FILTER_NAME));

        match map.get(CUSTOM_RESPONSE_FILTER_NAME) {
            Some(HttpScopedConfig::CustomResponse(cfg)) => {
                assert!(cfg.disabled);
            }
            _ => panic!("Expected CustomResponse config"),
        }
    }

    #[test]
    fn custom_response_override_object() {
        let filters = json!({
            "custom_response": {
                "disabled": false
            }
        });
        let map = typed_per_filter_config(&Some(filters)).expect("map");
        assert!(map.contains_key(CUSTOM_RESPONSE_FILTER_NAME));

        match map.get(CUSTOM_RESPONSE_FILTER_NAME) {
            Some(HttpScopedConfig::CustomResponse(cfg)) => {
                assert!(!cfg.disabled);
            }
            _ => panic!("Expected CustomResponse config"),
        }
    }

    #[test]
    fn custom_response_override_canonicalizes() {
        let filters = json!({
            "custom_response": "disabled"
        });
        let canonical = canonicalize_filter_overrides(Some(filters)).expect("canonicalize");
        let canonical_value = canonical.expect("some value");
        let obj = canonical_value.as_object().expect("object");
        assert!(obj.contains_key("custom_response"));
    }

    #[test]
    fn custom_response_invalid_string_rejected() {
        let filters = json!({
            "custom_response": "invalid"
        });
        let err = typed_per_filter_config(&Some(filters)).expect_err("should fail");
        assert!(format!("{err}").contains("expected 'disabled' or object"));
    }
}
