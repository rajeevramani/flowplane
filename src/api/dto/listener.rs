//! Listener DTOs for API request/response handling

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::xds::filters::http::HttpFilterConfigEntry;

/// Response DTO for listener details
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerResponseDto {
    pub name: String,
    pub address: String,
    pub port: Option<u16>,
    pub protocol: String,
    pub version: i64,
    #[schema(value_type = Object)]
    pub config: crate::xds::listener::ListenerConfig,
}

/// Request body for creating a listener
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateListenerDto {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainDto>,
    #[serde(default)]
    pub protocol: Option<String>,
}

/// Request body for updating a listener
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateListenerDto {
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainDto>,
    #[serde(default)]
    pub protocol: Option<String>,
}

/// Filter chain configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterChainDto {
    pub name: Option<String>,
    pub filters: Vec<ListenerFilterDto>,
    #[serde(default)]
    pub tls_context: Option<ListenerTlsContextDto>,
}

/// Filter configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterDto {
    pub name: String,
    #[serde(flatten)]
    pub filter_type: ListenerFilterTypeDto,
}

/// Filter type variants DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ListenerFilterTypeDto {
    #[serde(rename_all = "camelCase")]
    HttpConnectionManager {
        route_config_name: Option<String>,
        #[schema(value_type = Object)]
        inline_route_config: Option<Value>,
        #[serde(default)]
        access_log: Option<ListenerAccessLogDto>,
        #[serde(default)]
        tracing: Option<ListenerTracingDto>,
        #[serde(default)]
        #[schema(value_type = Vec<Object>)]
        http_filters: Vec<HttpFilterConfigEntry>,
    },
    #[serde(rename_all = "camelCase")]
    TcpProxy {
        cluster: String,
        #[serde(default)]
        access_log: Option<ListenerAccessLogDto>,
    },
}

/// TLS context configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerTlsContextDto {
    pub cert_chain_file: Option<String>,
    pub private_key_file: Option<String>,
    pub ca_cert_file: Option<String>,
    pub require_client_certificate: Option<bool>,
}

/// Access log configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerAccessLogDto {
    pub path: Option<String>,
    pub format: Option<String>,
}

/// Tracing configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerTracingDto {
    pub provider: Option<String>,
    pub max_path_tag_length: Option<u32>,
}
