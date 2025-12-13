//! Compressor HTTP filter configuration helpers
//!
//! This module provides configuration types for the Envoy compressor filter,
//! which compresses response bodies to reduce bandwidth usage.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::{RuntimeFeatureFlag, TypedExtensionConfig};
use envoy_types::pb::envoy::extensions::compression::gzip::compressor::v3::Gzip as GzipCompressor;
use envoy_types::pb::envoy::extensions::filters::http::compressor::v3::{
    compressor::ResponseDirectionConfig as ResponseDirectionConfigProto,
    Compressor as CompressorProto, CompressorPerRoute as CompressorPerRouteProto,
};
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, BoolValue, UInt32Value};
use prost::Message;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Type URLs for compressor filter configuration
pub const COMPRESSOR_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.compressor.v3.Compressor";
pub const COMPRESSOR_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.compressor.v3.CompressorPerRoute";
const GZIP_COMPRESSOR_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.compression.gzip.compressor.v3.Gzip";

/// Compression level options for gzip
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompressionLevel {
    /// Best compression speed (level 1)
    #[default]
    BestSpeed,
    /// Best compression ratio (level 9)
    BestCompression,
    /// Default compression level (level 6)
    DefaultCompression,
}

impl CompressionLevel {
    fn to_proto_value(self) -> i32 {
        match self {
            Self::BestSpeed => 1,
            Self::BestCompression => 2,
            Self::DefaultCompression => 0,
        }
    }

    fn from_proto_value(value: i32) -> Self {
        match value {
            1 => Self::BestSpeed,
            2 => Self::BestCompression,
            _ => Self::DefaultCompression,
        }
    }
}

/// Compression strategy options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompressionStrategy {
    /// Default strategy - balanced
    #[default]
    DefaultStrategy,
    /// Filtered strategy - for data with lots of small values
    Filtered,
    /// Huffman only - force Huffman encoding
    HuffmanOnly,
    /// RLE strategy - for run-length encoding
    Rle,
    /// Fixed strategy - prevent dynamic Huffman codes
    Fixed,
}

impl CompressionStrategy {
    fn to_proto_value(self) -> i32 {
        match self {
            Self::DefaultStrategy => 0,
            Self::Filtered => 1,
            Self::HuffmanOnly => 2,
            Self::Rle => 3,
            Self::Fixed => 4,
        }
    }

    fn from_proto_value(value: i32) -> Self {
        match value {
            1 => Self::Filtered,
            2 => Self::HuffmanOnly,
            3 => Self::Rle,
            4 => Self::Fixed,
            _ => Self::DefaultStrategy,
        }
    }
}

/// Gzip compressor library configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct GzipConfig {
    /// Memory level (1-9). Higher values use more memory but are faster.
    #[serde(default)]
    pub memory_level: Option<u32>,
    /// Window bits (9-15). Higher values give better compression.
    #[serde(default)]
    pub window_bits: Option<u32>,
    /// Compression level
    #[serde(default)]
    pub compression_level: CompressionLevel,
    /// Compression strategy
    #[serde(default)]
    pub compression_strategy: CompressionStrategy,
    /// Size of zlib's internal compression chunk buffer in bytes.
    #[serde(default)]
    pub chunk_size: Option<u32>,
}

impl GzipConfig {
    fn validate(&self) -> Result<(), crate::Error> {
        if let Some(memory_level) = self.memory_level {
            if !(1..=9).contains(&memory_level) {
                return Err(invalid_config("Compressor gzip memory_level must be between 1 and 9"));
            }
        }
        if let Some(window_bits) = self.window_bits {
            if !(9..=15).contains(&window_bits) {
                return Err(invalid_config("Compressor gzip window_bits must be between 9 and 15"));
            }
        }
        Ok(())
    }

    fn to_proto(&self) -> Result<GzipCompressor, crate::Error> {
        self.validate()?;
        Ok(GzipCompressor {
            memory_level: self.memory_level.map(|v| UInt32Value { value: v }),
            window_bits: self.window_bits.map(|v| UInt32Value { value: v }),
            compression_level: self.compression_level.to_proto_value(),
            compression_strategy: self.compression_strategy.to_proto_value(),
            chunk_size: self.chunk_size.map(|v| UInt32Value { value: v }),
        })
    }

    fn from_proto(proto: &GzipCompressor) -> Self {
        Self {
            memory_level: proto.memory_level.as_ref().map(|v| v.value),
            window_bits: proto.window_bits.as_ref().map(|v| v.value),
            compression_level: CompressionLevel::from_proto_value(proto.compression_level),
            compression_strategy: CompressionStrategy::from_proto_value(proto.compression_strategy),
            chunk_size: proto.chunk_size.as_ref().map(|v| v.value),
        }
    }
}

/// Compressor library selection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompressorLibrary {
    /// Gzip compression (most compatible)
    Gzip(GzipConfig),
}

impl Default for CompressorLibrary {
    fn default() -> Self {
        Self::Gzip(GzipConfig::default())
    }
}

impl CompressorLibrary {
    fn to_typed_extension_config(&self) -> Result<TypedExtensionConfig, crate::Error> {
        match self {
            Self::Gzip(config) => {
                let gzip_proto = config.to_proto()?;
                Ok(TypedExtensionConfig {
                    name: "gzip".to_string(),
                    typed_config: Some(EnvoyAny {
                        type_url: GZIP_COMPRESSOR_TYPE_URL.to_string(),
                        value: gzip_proto.encode_to_vec(),
                    }),
                })
            }
        }
    }

    fn from_typed_extension_config(config: &TypedExtensionConfig) -> Result<Self, crate::Error> {
        let any = config
            .typed_config
            .as_ref()
            .ok_or_else(|| invalid_config("Compressor library configuration is required"))?;

        if any.type_url == GZIP_COMPRESSOR_TYPE_URL {
            let gzip = GzipCompressor::decode(any.value.as_slice())
                .map_err(|e| invalid_config(format!("Failed to decode gzip config: {}", e)))?;
            Ok(Self::Gzip(GzipConfig::from_proto(&gzip)))
        } else {
            Err(invalid_config(format!("Unsupported compressor library: {}", any.type_url)))
        }
    }
}

/// Common compressor configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CommonCompressorConfig {
    /// Minimum response size in bytes to trigger compression.
    #[serde(default)]
    pub min_content_length: Option<u32>,
    /// Content types that should be compressed.
    #[serde(default)]
    pub content_type: Vec<String>,
    /// Whether to disable compression on responses with ETag header.
    #[serde(default)]
    pub disable_on_etag_header: Option<bool>,
    /// Whether to remove Accept-Encoding header after compression decision.
    #[serde(default)]
    pub remove_accept_encoding_header: Option<bool>,
}

impl CommonCompressorConfig {
    fn validate(&self) -> Result<(), crate::Error> {
        for content_type in &self.content_type {
            if content_type.trim().is_empty() {
                return Err(invalid_config("Compressor content_type entries cannot be empty"));
            }
        }
        Ok(())
    }
}

/// Response direction configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct ResponseDirectionConfig {
    /// Common compressor configuration
    #[serde(default)]
    pub common_config: CommonCompressorConfig,
}

/// Compressor filter configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CompressorConfig {
    /// Response direction configuration
    #[serde(default)]
    pub response_direction_config: ResponseDirectionConfig,
    /// Compressor library to use
    #[serde(default)]
    pub compressor_library: CompressorLibrary,
}

impl CompressorConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        self.response_direction_config.common_config.validate()?;
        match &self.compressor_library {
            CompressorLibrary::Gzip(gzip) => gzip.validate()?,
        }
        Ok(())
    }

    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let common_config = &self.response_direction_config.common_config;

        // Build common config proto
        let common_proto =
            envoy_types::pb::envoy::extensions::filters::http::compressor::v3::compressor::CommonDirectionConfig {
                enabled: Some(RuntimeFeatureFlag {
                    default_value: Some(BoolValue { value: true }),
                    runtime_key: String::new(),
                }),
                min_content_length: common_config.min_content_length.map(|v| UInt32Value { value: v }),
                content_type: common_config.content_type.clone(),
            };

        // Build response direction config proto
        let response_direction_proto = ResponseDirectionConfigProto {
            common_config: Some(common_proto),
            disable_on_etag_header: common_config.disable_on_etag_header.unwrap_or(false),
            remove_accept_encoding_header: common_config
                .remove_accept_encoding_header
                .unwrap_or(false),
            ..Default::default()
        };

        let proto = CompressorProto {
            response_direction_config: Some(response_direction_proto),
            request_direction_config: None,
            compressor_library: Some(self.compressor_library.to_typed_extension_config()?),
            ..Default::default()
        };

        Ok(any_from_message(COMPRESSOR_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &CompressorProto) -> Result<Self, crate::Error> {
        let response_config = proto.response_direction_config.as_ref();

        let common_config = response_config
            .and_then(|r| r.common_config.as_ref())
            .map(|c| CommonCompressorConfig {
                min_content_length: c.min_content_length.as_ref().map(|v| v.value),
                content_type: c.content_type.clone(),
                disable_on_etag_header: response_config.map(|r| r.disable_on_etag_header),
                remove_accept_encoding_header: response_config
                    .map(|r| r.remove_accept_encoding_header),
            })
            .unwrap_or_default();

        let compressor_library = proto
            .compressor_library
            .as_ref()
            .map(CompressorLibrary::from_typed_extension_config)
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            response_direction_config: ResponseDirectionConfig { common_config },
            compressor_library,
        })
    }
}

/// Per-route compressor configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CompressorPerRouteConfig {
    /// Whether to disable compression for this route
    #[serde(default)]
    pub disabled: bool,
}

impl CompressorPerRouteConfig {
    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        use envoy_types::pb::envoy::extensions::filters::http::compressor::v3::compressor_per_route::Override;

        let proto = CompressorPerRouteProto {
            r#override: if self.disabled { Some(Override::Disabled(true)) } else { None },
        };

        Ok(any_from_message(COMPRESSOR_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &CompressorPerRouteProto) -> Result<Self, crate::Error> {
        use envoy_types::pb::envoy::extensions::filters::http::compressor::v3::compressor_per_route::Override;

        let disabled = match &proto.r#override {
            Some(Override::Disabled(v)) => *v,
            _ => false,
        };

        Ok(Self { disabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_compressor_config() {
        let config = CompressorConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_gzip_validation_memory_level() {
        let config = CompressorConfig {
            compressor_library: CompressorLibrary::Gzip(GzipConfig {
                memory_level: Some(10), // Invalid: must be 1-9
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_gzip_validation_window_bits() {
        let config = CompressorConfig {
            compressor_library: CompressorLibrary::Gzip(GzipConfig {
                window_bits: Some(16), // Invalid: must be 9-15
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_gzip_config() {
        let config = CompressorConfig {
            response_direction_config: ResponseDirectionConfig {
                common_config: CommonCompressorConfig {
                    min_content_length: Some(1024),
                    content_type: vec!["application/json".to_string(), "text/html".to_string()],
                    disable_on_etag_header: Some(true),
                    remove_accept_encoding_header: Some(false),
                },
            },
            compressor_library: CompressorLibrary::Gzip(GzipConfig {
                memory_level: Some(5),
                window_bits: Some(12),
                compression_level: CompressionLevel::BestSpeed,
                compression_strategy: CompressionStrategy::DefaultStrategy,
                chunk_size: Some(4096),
            }),
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, COMPRESSOR_TYPE_URL);
    }

    #[test]
    fn test_compressor_to_any_produces_valid_proto() {
        let config = CompressorConfig {
            response_direction_config: ResponseDirectionConfig {
                common_config: CommonCompressorConfig {
                    min_content_length: Some(100),
                    content_type: vec!["application/json".to_string()],
                    ..Default::default()
                },
            },
            compressor_library: CompressorLibrary::Gzip(GzipConfig::default()),
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, COMPRESSOR_TYPE_URL);
        assert!(!any.value.is_empty());

        // Verify we can decode it back
        let proto = CompressorProto::decode(any.value.as_slice()).expect("decode");
        assert!(proto.response_direction_config.is_some());
        assert!(proto.compressor_library.is_some());
    }

    #[test]
    fn test_per_route_disabled() {
        let config = CompressorPerRouteConfig { disabled: true };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, COMPRESSOR_PER_ROUTE_TYPE_URL);
    }
}
