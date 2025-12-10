//! Dynamic filter configuration conversion to Envoy protobuf.
//!
//! This module provides schema-driven conversion of filter configurations to
//! Envoy protobuf messages without requiring compile-time match arms for each
//! filter type.
//!
//! # Architecture
//!
//! The converter uses a hybrid approach:
//! 1. **Known filters**: Use existing strongly-typed conversion code
//! 2. **Unknown/custom filters**: Convert JSON to `google.protobuf.Struct`
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::xds::filters::dynamic_conversion::DynamicFilterConverter;
//! use flowplane::domain::filter_schema::FilterSchemaRegistry;
//!
//! let registry = FilterSchemaRegistry::with_builtin_schemas();
//! let converter = DynamicFilterConverter::new(&registry);
//!
//! let config = serde_json::json!({
//!     "stat_prefix": "test",
//!     "token_bucket": {
//!         "max_tokens": 100,
//!         "fill_interval_ms": 1000
//!     }
//! });
//!
//! let any = converter.to_listener_any("local_rate_limit", &config)?;
//! ```

use crate::domain::filter_schema::{FilterSchemaDefinition, FilterSchemaRegistry};
use crate::domain::PerRouteBehavior;
use crate::Result;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use prost::Message;
use prost_types::{ListValue, Struct, Value};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

/// Type URL for google.protobuf.Struct used for dynamic filter configs.
const STRUCT_TYPE_URL: &str = "type.googleapis.com/google.protobuf.Struct";

/// Converts dynamic filter configurations to Envoy protobuf messages.
///
/// Uses the filter schema registry to determine conversion behavior and
/// type URLs. For filters without specific conversion code, falls back to
/// `google.protobuf.Struct` representation.
pub struct DynamicFilterConverter<'a> {
    registry: &'a FilterSchemaRegistry,
}

impl<'a> DynamicFilterConverter<'a> {
    /// Create a new converter with a reference to the schema registry.
    pub fn new(registry: &'a FilterSchemaRegistry) -> Self {
        Self { registry }
    }

    /// Convert filter configuration JSON to Envoy Any for listener-level injection.
    ///
    /// # Arguments
    ///
    /// * `filter_type` - The filter type name (e.g., "header_mutation")
    /// * `config` - The filter configuration as JSON
    ///
    /// # Returns
    ///
    /// An Envoy `Any` protobuf message ready for the HTTP filter chain.
    pub fn to_listener_any(&self, filter_type: &str, config: &JsonValue) -> Result<EnvoyAny> {
        let schema = self
            .registry
            .get(filter_type)
            .ok_or_else(|| crate::Error::config(format!("Unknown filter type: {}", filter_type)))?;

        // Use the schema's type URL with Struct-wrapped JSON
        let struct_value = json_to_struct(config)?;
        let any = EnvoyAny {
            type_url: schema.envoy.type_url.clone(),
            value: struct_value.encode_to_vec(),
        };

        Ok(any)
    }

    /// Convert filter configuration to per-route Envoy Any.
    ///
    /// Returns `None` if the filter doesn't support per-route configuration.
    ///
    /// # Arguments
    ///
    /// * `filter_type` - The filter type name
    /// * `config` - The per-route configuration as JSON
    ///
    /// # Returns
    ///
    /// Optional tuple of (filter_name, Any) for per-route injection.
    pub fn to_per_route_any(
        &self,
        filter_type: &str,
        config: &JsonValue,
    ) -> Result<Option<(String, EnvoyAny)>> {
        let schema = self
            .registry
            .get(filter_type)
            .ok_or_else(|| crate::Error::config(format!("Unknown filter type: {}", filter_type)))?;

        // Check if per-route is supported
        if matches!(schema.capabilities.per_route_behavior, PerRouteBehavior::NotSupported) {
            return Ok(None);
        }

        let per_route_type_url = match &schema.envoy.per_route_type_url {
            Some(url) => url.clone(),
            None => {
                // No per-route type URL configured, per-route not supported
                return Ok(None);
            }
        };

        let struct_value = json_to_struct(config)?;
        let any = EnvoyAny { type_url: per_route_type_url, value: struct_value.encode_to_vec() };

        Ok(Some((schema.envoy.http_filter_name.clone(), any)))
    }

    /// Create an empty placeholder filter configuration.
    ///
    /// For filters that don't require configuration, creates a minimal
    /// valid protobuf message.
    ///
    /// # Arguments
    ///
    /// * `filter_type` - The filter type name
    ///
    /// # Returns
    ///
    /// An empty Envoy `Any` message with the correct type URL.
    pub fn create_empty_any(&self, filter_type: &str) -> Result<EnvoyAny> {
        let schema = self
            .registry
            .get(filter_type)
            .ok_or_else(|| crate::Error::config(format!("Unknown filter type: {}", filter_type)))?;

        // Create an empty struct
        let empty_struct = Struct { fields: BTreeMap::new() };
        let any = EnvoyAny {
            type_url: schema.envoy.type_url.clone(),
            value: empty_struct.encode_to_vec(),
        };

        Ok(any)
    }

    /// Get the HTTP filter name for a filter type.
    pub fn get_filter_name(&self, filter_type: &str) -> Option<&str> {
        self.registry.get(filter_type).map(|s| s.envoy.http_filter_name.as_str())
    }

    /// Check if a filter type is known to the registry.
    pub fn is_known_filter(&self, filter_type: &str) -> bool {
        self.registry.contains(filter_type)
    }

    /// Get schema for a filter type.
    pub fn get_schema(&self, filter_type: &str) -> Option<&FilterSchemaDefinition> {
        self.registry.get(filter_type)
    }
}

/// Convert a JSON value to a protobuf Struct.
///
/// This provides a generic way to pass JSON configuration to Envoy filters
/// using `google.protobuf.Struct` as the value type.
pub fn json_to_struct(json: &JsonValue) -> Result<Struct> {
    match json {
        JsonValue::Object(map) => {
            let mut fields = BTreeMap::new();
            for (key, value) in map {
                fields.insert(key.clone(), json_to_proto_value(value)?);
            }
            Ok(Struct { fields })
        }
        _ => Err(crate::Error::config("Filter configuration must be a JSON object".to_string())),
    }
}

/// Convert a JSON value to a protobuf Value.
fn json_to_proto_value(json: &JsonValue) -> Result<Value> {
    let kind = match json {
        JsonValue::Null => prost_types::value::Kind::NullValue(0),
        JsonValue::Bool(b) => prost_types::value::Kind::BoolValue(*b),
        JsonValue::Number(n) => {
            // Protobuf only has double for numbers
            let num = n.as_f64().ok_or_else(|| {
                crate::Error::config(format!("Cannot convert number {} to f64", n))
            })?;
            prost_types::value::Kind::NumberValue(num)
        }
        JsonValue::String(s) => prost_types::value::Kind::StringValue(s.clone()),
        JsonValue::Array(arr) => {
            let values: Result<Vec<Value>> = arr.iter().map(json_to_proto_value).collect();
            prost_types::value::Kind::ListValue(ListValue { values: values? })
        }
        JsonValue::Object(map) => {
            let mut fields = BTreeMap::new();
            for (key, value) in map {
                fields.insert(key.clone(), json_to_proto_value(value)?);
            }
            prost_types::value::Kind::StructValue(Struct { fields })
        }
    };

    Ok(Value { kind: Some(kind) })
}

/// Convert a protobuf Struct back to JSON.
///
/// Useful for debugging and round-trip testing.
pub fn struct_to_json(s: &Struct) -> JsonValue {
    let mut map = serde_json::Map::new();
    for (key, value) in &s.fields {
        map.insert(key.clone(), proto_value_to_json(value));
    }
    JsonValue::Object(map)
}

/// Convert a protobuf Value back to JSON.
fn proto_value_to_json(value: &Value) -> JsonValue {
    match &value.kind {
        Some(prost_types::value::Kind::NullValue(_)) => JsonValue::Null,
        Some(prost_types::value::Kind::BoolValue(b)) => JsonValue::Bool(*b),
        Some(prost_types::value::Kind::NumberValue(n)) => JsonValue::Number(
            serde_json::Number::from_f64(*n).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Some(prost_types::value::Kind::StringValue(s)) => JsonValue::String(s.clone()),
        Some(prost_types::value::Kind::ListValue(list)) => {
            JsonValue::Array(list.values.iter().map(proto_value_to_json).collect())
        }
        Some(prost_types::value::Kind::StructValue(s)) => struct_to_json(s),
        None => JsonValue::Null,
    }
}

/// Create an Envoy Any with a Struct payload and custom type URL.
///
/// This allows wrapping JSON config with a specific Envoy filter type URL,
/// which Envoy will interpret based on the type_url field.
pub fn create_struct_any(type_url: &str, config: &JsonValue) -> Result<EnvoyAny> {
    let struct_value = json_to_struct(config)?;
    Ok(EnvoyAny { type_url: type_url.to_string(), value: struct_value.encode_to_vec() })
}

/// Create a generic Struct-typed Any (useful for custom filters).
pub fn create_generic_struct_any(config: &JsonValue) -> Result<EnvoyAny> {
    create_struct_any(STRUCT_TYPE_URL, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_to_struct_simple() {
        let json = json!({
            "key": "value",
            "number": 42,
            "boolean": true
        });

        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();
        assert_eq!(s.fields.len(), 3);

        // Verify round-trip - note that numbers come back as f64
        let back = struct_to_json(&s);
        assert_eq!(back["key"], "value");
        assert_eq!(back["number"].as_f64().unwrap() as i64, 42);
        assert_eq!(back["boolean"], true);
    }

    #[test]
    fn test_json_to_struct_nested() {
        let json = json!({
            "outer": {
                "inner": {
                    "deep": "value"
                }
            }
        });

        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();

        let back = struct_to_json(&s);
        assert_eq!(back["outer"]["inner"]["deep"], "value");
    }

    #[test]
    fn test_json_to_struct_array() {
        let json = json!({
            "items": [1, 2, 3],
            "strings": ["a", "b", "c"]
        });

        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();

        let back = struct_to_json(&s);
        // Numbers come back as f64 in protobuf Struct
        let items: Vec<i64> =
            back["items"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as i64).collect();
        assert_eq!(items, vec![1, 2, 3]);
        assert_eq!(back["strings"], json!(["a", "b", "c"]));
    }

    #[test]
    fn test_json_to_struct_null() {
        let json = json!({
            "nullable": null
        });

        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();

        let back = struct_to_json(&s);
        assert!(back["nullable"].is_null());
    }

    #[test]
    fn test_json_to_struct_rejects_non_object() {
        let json = json!("not an object");
        let result = json_to_struct(&json);
        assert!(result.is_err());

        let json = json!([1, 2, 3]);
        let result = json_to_struct(&json);
        assert!(result.is_err());
    }

    #[test]
    fn test_converter_with_registry() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        assert!(converter.is_known_filter("header_mutation"));
        assert!(converter.is_known_filter("jwt_auth"));
        assert!(!converter.is_known_filter("unknown_filter"));
    }

    #[test]
    fn test_converter_to_listener_any() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        let config = json!({
            "request_headers_to_add": [{
                "key": "X-Test",
                "value": "test-value",
                "append": false
            }]
        });

        let result = converter.to_listener_any("header_mutation", &config);
        assert!(result.is_ok());
        let any = result.unwrap();
        assert!(any.type_url.contains("header_mutation"));
    }

    #[test]
    fn test_converter_to_listener_any_unknown_filter() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        let config = json!({"key": "value"});
        let result = converter.to_listener_any("unknown_filter", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_converter_to_per_route_any() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        // Header mutation supports per-route
        let config = json!({
            "request_headers_to_add": [{
                "key": "X-Route-Header",
                "value": "route-value"
            }]
        });

        let result = converter.to_per_route_any("header_mutation", &config);
        assert!(result.is_ok());
        let opt = result.unwrap();
        assert!(opt.is_some());
        let (filter_name, any) = opt.unwrap();
        assert_eq!(filter_name, "envoy.filters.http.header_mutation");
        assert!(any.type_url.contains("HeaderMutationPerRoute"));
    }

    #[test]
    fn test_converter_create_empty_any() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        let result = converter.create_empty_any("header_mutation");
        assert!(result.is_ok());
        let any = result.unwrap();
        assert!(any.type_url.contains("header_mutation"));
        // Empty BTreeMap Struct encodes to empty bytes since there are no fields
        // This is valid protobuf encoding for an empty message
    }

    #[test]
    fn test_converter_get_filter_name() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        assert_eq!(
            converter.get_filter_name("header_mutation"),
            Some("envoy.filters.http.header_mutation")
        );
        assert_eq!(converter.get_filter_name("jwt_auth"), Some("envoy.filters.http.jwt_authn"));
        assert_eq!(converter.get_filter_name("unknown"), None);
    }

    #[test]
    fn test_create_struct_any() {
        let config = json!({
            "key": "value",
            "nested": {
                "inner": 42
            }
        });

        let result = create_struct_any("type.googleapis.com/test.Config", &config);
        assert!(result.is_ok());
        let any = result.unwrap();
        assert_eq!(any.type_url, "type.googleapis.com/test.Config");
        assert!(!any.value.is_empty());
    }

    #[test]
    fn test_create_generic_struct_any() {
        let config = json!({"key": "value"});
        let result = create_generic_struct_any(&config);
        assert!(result.is_ok());
        let any = result.unwrap();
        assert_eq!(any.type_url, STRUCT_TYPE_URL);
    }

    #[test]
    fn test_complex_filter_config() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        // Local rate limit config
        let config = json!({
            "stat_prefix": "test_rate_limit",
            "token_bucket": {
                "max_tokens": 100,
                "tokens_per_fill": 10,
                "fill_interval_ms": 1000
            },
            "status_code": 429
        });

        let result = converter.to_listener_any("local_rate_limit", &config);
        assert!(result.is_ok());
        let any = result.unwrap();
        assert!(any.type_url.contains("local_ratelimit"));
    }

    #[test]
    fn test_mcp_filter_per_route_disable_only() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        // MCP has DisableOnly per-route behavior
        let config = json!({
            "disabled": true
        });

        let result = converter.to_per_route_any("mcp", &config);
        assert!(result.is_ok());
        // DisableOnly still returns Some since it has a per_route_type_url
        let opt = result.unwrap();
        assert!(opt.is_some());
    }

    #[test]
    fn test_jwt_auth_listener_config() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let converter = DynamicFilterConverter::new(&registry);

        let config = json!({
            "providers": {
                "test-provider": {
                    "issuer": "https://issuer.example.com",
                    "jwks": {
                        "local": {
                            "inline_string": "{\"keys\":[]}"
                        }
                    }
                }
            }
        });

        let result = converter.to_listener_any("jwt_auth", &config);
        assert!(result.is_ok());
        let any = result.unwrap();
        assert!(any.type_url.contains("jwt_authn"));
    }

    #[test]
    fn test_float_number_conversion() {
        let json = json!({
            "float_value": 2.12345,
            "int_value": 42,
            "negative": -10.5
        });

        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();
        let back = struct_to_json(&s);

        // Due to f64 representation, we check approximate values
        assert!((back["float_value"].as_f64().unwrap() - 2.12345).abs() < 0.0001);
        assert_eq!(back["int_value"].as_f64().unwrap() as i64, 42);
        assert!((back["negative"].as_f64().unwrap() - (-10.5)).abs() < 0.0001);
    }

    #[test]
    fn test_empty_object() {
        let json = json!({});
        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();
        assert!(s.fields.is_empty());

        let back = struct_to_json(&s);
        assert!(back.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_mixed_array() {
        let json = json!({
            "mixed": [1, "two", true, null, {"nested": "object"}]
        });

        let result = json_to_struct(&json);
        assert!(result.is_ok());
        let s = result.unwrap();
        let back = struct_to_json(&s);

        let arr = back["mixed"].as_array().unwrap();
        assert_eq!(arr.len(), 5);
        // Numbers come back as f64
        assert_eq!(arr[0].as_f64().unwrap() as i64, 1);
        assert_eq!(arr[1], "two");
        assert_eq!(arr[2], true);
        assert!(arr[3].is_null());
        assert_eq!(arr[4]["nested"], "object");
    }
}
