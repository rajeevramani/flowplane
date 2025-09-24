//! Modular filter utilities for Envoy resources
//!
//! Provides reusable data structures and builders for Envoy filters so
//! listener, route and cluster configuration code can share logic while
//! allowing new filters to be added incrementally.

pub mod http;

use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use envoy_types::pb::google::protobuf::Any;
use prost::Message;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Wrapper for binary protobuf payloads serialized as base64 in JSON
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

/// Generic representation of a typed Envoy protobuf Any payload
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TypedConfig {
    pub type_url: String,
    #[serde(default)]
    pub value: Base64Bytes,
}

impl TypedConfig {
    /// Create a typed config from a prost message
    pub fn from_message<M: Message>(type_url: impl Into<String>, msg: &M) -> Self {
        Self {
            type_url: type_url.into(),
            value: Base64Bytes(msg.encode_to_vec()),
        }
    }

    /// Convert to Envoy Any structure
    pub fn to_any(&self) -> Any {
        Any {
            type_url: self.type_url.clone(),
            value: self.value.0.clone(),
        }
    }
}

/// Helper for building `Any` values from prost messages
pub fn any_from_message<M: Message>(type_url: impl Into<String>, msg: &M) -> Any {
    TypedConfig::from_message(type_url, msg).to_any()
}

/// Error helper for invalid filter configuration
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
        let msg = TestMessage {
            field: "hello".into(),
        };
        let typed = TypedConfig::from_message("type.googleapis.com/test.Message", &msg);
        let any = typed.to_any();
        assert_eq!(any.type_url, "type.googleapis.com/test.Message");
        assert!(!any.value.is_empty());
    }
}
