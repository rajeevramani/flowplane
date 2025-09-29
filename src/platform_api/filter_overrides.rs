use std::collections::HashMap;

use serde_json::Value;

use crate::errors::Error;
use crate::xds::filters::http::cors::{
    CorsOriginMatcher, CorsPerRouteConfig, CorsPolicyConfig, FractionalPercentDenominator,
    RuntimeFractionalPercentConfig,
};
use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
use crate::xds::filters::http::HttpScopedConfig;

const CORS_FILTER_NAME: &str = "envoy.filters.http.cors";
const JWT_FILTER_NAME: &str = "envoy.filters.http.jwt_authn";

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
            "authn" => JWT_FILTER_NAME,
            // Allow callers to specify a fully-qualified filter name directly.
            other if other.contains('.') => other,
            other => {
                return Err(Error::validation(format!(
                    "Unsupported filter override alias '{}'; expected 'cors', 'authn', or a fully qualified filter name",
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
            "cors" => HttpScopedConfig::Cors(parse_cors_override(raw)?),
            "authn" => HttpScopedConfig::JwtAuthn(parse_authn_override(raw)?),
            other if other.contains('.') => HttpScopedConfig::Typed(parse_typed_override(raw)?),
            other => {
                return Err(Error::validation(format!("Unsupported filter override '{}'", other)));
            }
        };
        entries.push((alias.clone(), scoped));
    }

    Ok(entries)
}

fn parse_cors_override(value: &Value) -> Result<CorsPerRouteConfig, Error> {
    let config = match value {
        Value::String(template) => {
            if template == "allow-authenticated" {
                cors_allow_authenticated_template()
            } else {
                return Err(Error::validation(format!(
                    "Unknown CORS template '{}'. Supported templates: allow-authenticated",
                    template
                )));
            }
        }
        Value::Object(_) => serde_json::from_value::<CorsPerRouteConfig>(value.clone())
            .map_err(|err| Error::validation(format!("Invalid CORS override: {err}")))?,
        _ => {
            return Err(Error::validation(
                "CORS override must be a string template or structured object",
            ));
        }
    };

    config.policy.validate().map_err(|err| Error::validation(err.to_string()))?;
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
}
