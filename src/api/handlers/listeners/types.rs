//! Listener handler DTOs and type definitions

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::xds::{filters::http::HttpFilterConfigEntry, listener::ListenerConfig};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerResponse {
    pub name: String,
    pub address: String,
    pub port: Option<u16>,
    pub protocol: String,
    pub version: i64,
    #[schema(value_type = Object)]
    pub config: ListenerConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListListenersQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateListenerBody {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainInput>,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateListenerBody {
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainInput>,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterChainInput {
    pub name: Option<String>,
    pub filters: Vec<ListenerFilterInput>,
    #[serde(default)]
    pub tls_context: Option<ListenerTlsContextInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterInput {
    pub name: String,
    #[serde(flatten)]
    pub filter_type: ListenerFilterTypeInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ListenerFilterTypeInput {
    #[serde(rename_all = "camelCase")]
    HttpConnectionManager {
        route_config_name: Option<String>,
        #[schema(value_type = Object)]
        inline_route_config: Option<Value>,
        #[serde(default)]
        access_log: Option<ListenerAccessLogInput>,
        #[serde(default)]
        tracing: Option<ListenerTracingInput>,
        #[serde(default)]
        #[schema(value_type = Vec<Object>)]
        http_filters: Vec<HttpFilterConfigEntry>,
    },
    #[serde(rename_all = "camelCase")]
    TcpProxy {
        cluster: String,
        #[serde(default)]
        access_log: Option<ListenerAccessLogInput>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerTlsContextInput {
    pub cert_chain_file: Option<String>,
    pub private_key_file: Option<String>,
    pub ca_cert_file: Option<String>,
    pub require_client_certificate: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerAccessLogInput {
    pub path: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerTracingInput {
    pub provider: String,
    #[schema(value_type = Object)]
    pub config: Value,
}
