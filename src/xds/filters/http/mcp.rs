//! Model Context Protocol (MCP) HTTP filter configuration
//!
//! This module provides configuration for Envoy's MCP filter, which enables
//! traffic inspection and validation for AI/LLM gateway traffic. The MCP
//! protocol uses JSON-RPC 2.0 for POST requests and SSE for streaming responses.
//!
//! See: https://docs.rs/envoy-types/latest/envoy_types/pb/envoy/extensions/filters/http/mcp/v3/

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::extensions::filters::http::mcp::v3::Mcp as McpProto;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use prost::Message;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Type URL for MCP filter configuration
pub const MCP_TYPE_URL: &str = "type.googleapis.com/envoy.extensions.filters.http.mcp.v3.Mcp";

/// Type URL for MCP per-route configuration
pub const MCP_PER_ROUTE_TYPE_URL: &str = "type.googleapis.com/envoy.config.route.v3.FilterConfig";

/// Traffic mode for the MCP filter
///
/// Configures how the filter handles non-MCP traffic passing through the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrafficMode {
    /// Pass all traffic through without MCP spec checking (default)
    ///
    /// All HTTP requests are proxied normally regardless of whether they
    /// follow the MCP specification.
    #[default]
    PassThrough,

    /// Reject requests that don't follow the MCP spec
    ///
    /// Only valid MCP requests are allowed:
    /// - POST requests with JSON-RPC 2.0 messages
    /// - GET requests for SSE streams (with Accept: text/event-stream)
    ///
    /// Non-MCP traffic receives an error response.
    RejectNoMcp,
}

impl TrafficMode {
    /// Convert from protobuf enum value
    pub fn from_proto(value: i32) -> Self {
        match value {
            1 => TrafficMode::RejectNoMcp,
            _ => TrafficMode::PassThrough,
        }
    }

    /// Convert to protobuf enum value
    pub fn to_proto(self) -> i32 {
        match self {
            TrafficMode::PassThrough => 0,
            TrafficMode::RejectNoMcp => 1,
        }
    }
}

/// Configuration for the MCP HTTP filter
///
/// The MCP filter inspects HTTP traffic for Model Context Protocol compliance.
/// MCP is used for AI/LLM tool integrations and uses:
/// - JSON-RPC 2.0 over HTTP POST for tool calls
/// - Server-Sent Events (SSE) for streaming responses
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct McpFilterConfig {
    /// How to handle traffic that doesn't conform to MCP protocol
    #[serde(default)]
    pub traffic_mode: TrafficMode,
}

impl McpFilterConfig {
    /// Create a new MCP filter config with pass-through mode (default)
    pub fn pass_through() -> Self {
        Self { traffic_mode: TrafficMode::PassThrough }
    }

    /// Create a new MCP filter config that rejects non-MCP traffic
    pub fn reject_non_mcp() -> Self {
        Self { traffic_mode: TrafficMode::RejectNoMcp }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        // TrafficMode is an enum, so it's always valid
        // Add additional validation here if needed in the future
        Ok(())
    }

    /// Convert configuration to Envoy protobuf Any message
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let proto = McpProto { traffic_mode: self.traffic_mode.to_proto() };

        Ok(any_from_message(MCP_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy protobuf message
    pub fn from_proto(proto: &McpProto) -> Result<Self, crate::Error> {
        let config = Self { traffic_mode: TrafficMode::from_proto(proto.traffic_mode) };

        config.validate()?;
        Ok(config)
    }

    /// Parse from Envoy Any message
    pub fn from_any(any: &EnvoyAny) -> Result<Self, crate::Error> {
        if any.type_url != MCP_TYPE_URL {
            return Err(invalid_config(format!(
                "Expected type URL {}, got {}",
                MCP_TYPE_URL, any.type_url
            )));
        }

        let proto = McpProto::decode(any.value.as_slice())
            .map_err(|err| crate::Error::config(format!("Failed to decode MCP config: {}", err)))?;

        Self::from_proto(&proto)
    }
}

/// Per-route configuration for MCP filter
///
/// Allows disabling the MCP filter for specific routes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct McpPerRouteConfig {
    /// Whether to disable MCP filtering for this route
    #[serde(default)]
    pub disabled: bool,
}

impl McpPerRouteConfig {
    /// Validate per-route configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        // Simple boolean flag, always valid
        Ok(())
    }

    /// Convert to Envoy Any payload for typed_per_filter_config
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        use envoy_types::pb::envoy::config::route::v3::FilterConfig;

        self.validate()?;

        let proto = FilterConfig { disabled: self.disabled, is_optional: false, config: None };

        Ok(any_from_message(MCP_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(
        proto: &envoy_types::pb::envoy::config::route::v3::FilterConfig,
    ) -> Result<Self, crate::Error> {
        let config = Self { disabled: proto.disabled };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_pass_through() {
        let config = McpFilterConfig::default();
        assert_eq!(config.traffic_mode, TrafficMode::PassThrough);
    }

    #[test]
    fn pass_through_constructor() {
        let config = McpFilterConfig::pass_through();
        assert_eq!(config.traffic_mode, TrafficMode::PassThrough);
    }

    #[test]
    fn reject_non_mcp_constructor() {
        let config = McpFilterConfig::reject_non_mcp();
        assert_eq!(config.traffic_mode, TrafficMode::RejectNoMcp);
    }

    #[test]
    fn config_validates_successfully() {
        let config = McpFilterConfig { traffic_mode: TrafficMode::RejectNoMcp };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_to_any_produces_valid_proto() {
        let config = McpFilterConfig { traffic_mode: TrafficMode::RejectNoMcp };
        let any = config.to_any().expect("to_any");

        assert_eq!(any.type_url, MCP_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn config_round_trip() {
        let original = McpFilterConfig { traffic_mode: TrafficMode::RejectNoMcp };

        let any = original.to_any().expect("to_any");
        let proto = McpProto::decode(any.value.as_slice()).expect("decode");
        let restored = McpFilterConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(restored.traffic_mode, TrafficMode::RejectNoMcp);
    }

    #[test]
    fn config_from_any_round_trip() {
        let original = McpFilterConfig { traffic_mode: TrafficMode::PassThrough };

        let any = original.to_any().expect("to_any");
        let restored = McpFilterConfig::from_any(&any).expect("from_any");

        assert_eq!(restored.traffic_mode, TrafficMode::PassThrough);
    }

    #[test]
    fn traffic_mode_proto_conversion() {
        assert_eq!(TrafficMode::PassThrough.to_proto(), 0);
        assert_eq!(TrafficMode::RejectNoMcp.to_proto(), 1);

        assert_eq!(TrafficMode::from_proto(0), TrafficMode::PassThrough);
        assert_eq!(TrafficMode::from_proto(1), TrafficMode::RejectNoMcp);
        // Unknown values default to PassThrough
        assert_eq!(TrafficMode::from_proto(99), TrafficMode::PassThrough);
    }

    #[test]
    fn traffic_mode_serde() {
        let config = McpFilterConfig { traffic_mode: TrafficMode::RejectNoMcp };
        let json = serde_json::to_string(&config).expect("serialize");

        // Should serialize as snake_case string
        assert!(json.contains("reject_no_mcp"), "JSON: {}", json);

        let parsed: McpFilterConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.traffic_mode, TrafficMode::RejectNoMcp);
    }

    #[test]
    fn per_route_config_default() {
        let config = McpPerRouteConfig::default();
        assert!(!config.disabled);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn per_route_config_disabled() {
        let config = McpPerRouteConfig { disabled: true };
        assert!(config.validate().is_ok());

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, MCP_PER_ROUTE_TYPE_URL);
    }

    #[test]
    fn per_route_proto_round_trip() {
        use envoy_types::pb::envoy::config::route::v3::FilterConfig;

        let config = McpPerRouteConfig { disabled: true };
        let any = config.to_any().expect("to_any");

        let proto = FilterConfig::decode(any.value.as_slice()).expect("decode proto");
        assert!(proto.disabled);

        let restored = McpPerRouteConfig::from_proto(&proto).expect("from_proto");
        assert!(restored.disabled);
    }
}
