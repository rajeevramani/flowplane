//! Modular filter utilities for Envoy HTTP filters.
//!
//! This module provides type-safe configuration builders for Envoy HTTP filters,
//! allowing listener, route, and cluster configuration code to share common logic.
//! Each filter module handles conversion between high-level Rust types and Envoy
//! protobuf messages.
//!
//! # Architecture
//!
//! Filter modules follow a consistent pattern:
//! - High-level Rust configuration structs with serde support
//! - Validation methods ensuring configuration correctness
//! - `to_any()` methods converting to Envoy protobuf `Any` messages
//! - `from_proto()` methods for deserialization
//!
//! # Available Filters
//!
//! - **Header Mutation**: Add, modify, or remove HTTP headers
//! - **CORS**: Cross-Origin Resource Sharing policy configuration
//! - **JWT Auth**: JSON Web Token authentication
//! - **Rate Limiting**: Request rate limiting (global, local, quota-based)
//! - **Custom Response**: Override responses with custom status/body
//! - **Health Check**: HTTP health check endpoints
//! - **External Processor**: Integration with external processing services
//! - **Credential Injector**: Inject authentication credentials
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::xds::filters::http::{HeaderMutationConfig, HeaderMutationEntry};
//!
//! // Create a header mutation filter configuration
//! let config = HeaderMutationConfig {
//!     request_headers_to_add: vec![
//!         HeaderMutationEntry {
//!             key: "X-Custom-Header".to_string(),
//!             value: "my-value".to_string(),
//!             append: false,
//!         }
//!     ],
//!     request_headers_to_remove: vec!["X-Unwanted-Header".to_string()],
//!     ..Default::default()
//! };
//!
//! // Convert to Envoy protobuf Any for xDS
//! let any = config.to_any()?;
//! ```

pub mod http;

use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use envoy_types::pb::google::protobuf::Any;
use prost::Message;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Wrapper for binary protobuf payloads serialized as base64 in JSON.
///
/// Used to represent Envoy protobuf `Any` message values in JSON APIs.
/// Automatically handles base64 encoding/decoding during serialization.
#[derive(Debug, Clone, PartialEq, Eq, Default, ToSchema)]
pub struct Base64Bytes(pub Vec<u8>);

impl Serialize for Base64Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let encoded = BASE64_ENGINE.encode(&self.0);
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = BASE64_ENGINE
            .decode(encoded.as_bytes())
            .map_err(|err| serde::de::Error::custom(err.to_string()))?;
        Ok(Base64Bytes(decoded))
    }
}

/// Generic representation of a typed Envoy protobuf `Any` payload.
///
/// Provides JSON-serializable representation of Envoy's `google.protobuf.Any` type,
/// which wraps typed protobuf messages with their type URL.
///
/// # Fields
///
/// - `type_url`: Fully-qualified protobuf type (e.g., "type.googleapis.com/envoy.config.route.v3.RouteConfiguration")
/// - `value`: Base64-encoded protobuf message bytes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TypedConfig {
    pub type_url: String,
    #[serde(default)]
    pub value: Base64Bytes,
}

impl TypedConfig {
    /// Creates a typed config from a prost message.
    ///
    /// # Arguments
    ///
    /// * `type_url` - Fully-qualified type URL for the message
    /// * `msg` - The protobuf message to encode
    pub fn from_message<M: Message>(type_url: impl Into<String>, msg: &M) -> Self {
        Self { type_url: type_url.into(), value: Base64Bytes(msg.encode_to_vec()) }
    }

    /// Converts to Envoy `Any` structure for xDS protocol.
    pub fn to_any(&self) -> Any {
        Any { type_url: self.type_url.clone(), value: self.value.0.clone() }
    }
}

/// Helper for building Envoy `Any` values from prost messages.
///
/// Convenience function that combines `TypedConfig::from_message()` and `to_any()`.
///
/// # Arguments
///
/// * `type_url` - Fully-qualified protobuf type URL
/// * `msg` - The protobuf message to encode
///
/// # Returns
///
/// An Envoy `google.protobuf.Any` message ready for xDS
pub fn any_from_message<M: Message>(type_url: impl Into<String>, msg: &M) -> Any {
    TypedConfig::from_message(type_url, msg).to_any()
}

/// Error helper for invalid filter configuration.
///
/// Creates a configuration error with the given message. Used throughout
/// filter validation to provide consistent error reporting.
pub fn invalid_config(msg: impl Into<String>) -> crate::Error {
    crate::Error::config(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[derive(Clone, PartialEq, Eq, Message)]
    struct TestMessage {
        #[prost(string, tag = "1")]
        field: String,
    }

    #[test]
    fn base64_round_trip() {
        let original = Base64Bytes(vec![1, 2, 3, 4]);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"AQIDBA==\"");

        let decoded: Base64Bytes = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn typed_config_from_message() {
        let msg = TestMessage { field: "hello".into() };
        let typed = TypedConfig::from_message("type.googleapis.com/test.Message", &msg);
        let any = typed.to_any();
        assert_eq!(any.type_url, "type.googleapis.com/test.Message");
        assert!(!any.value.is_empty());
    }
}
