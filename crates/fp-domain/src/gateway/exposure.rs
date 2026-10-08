//! Shortcut membership and provenance, never a duplicate gateway spec.
use super::{cluster::Cluster, listener::Listener, route_config::RouteConfig};
use crate::{ClusterId, ExposureId, ListenerId, OrgId, RouteConfigId, TeamId};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone)]
pub struct Exposure {
    pub id: ExposureId,
    pub team_id: TeamId,
    pub org_id: OrgId,
    pub name: String,
    pub cluster_id: ClusterId,
    pub route_config_id: RouteConfigId,
    pub listener_id: ListenerId,
    pub virtual_host: String,
    pub route_name: String,
    pub cleanup_listener: bool,
    pub cleanup_route_config: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExposeRequest {
    pub name: String,
    pub upstream: String,
    pub path: String,
    pub port: Option<u16>,
    pub public_base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExposureMode {
    Created,
}
impl ExposureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResourceDisposition {
    Deleted,
    Retained,
}
impl ResourceDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Retained => "retained",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExposedService {
    pub name: String,
    pub upstream: String,
    pub path: String,
    pub port: u16,
    pub mode: ExposureMode,
    pub cluster: Cluster,
    pub route_config: RouteConfig,
    pub listener: Listener,
    pub curl_url: Option<String>,
    pub endpoint_source: ExposeEndpointSource,
}

#[derive(Debug, Clone)]
pub struct UnexposedService {
    pub name: String,
    pub cluster_name: String,
    pub route_config_name: String,
    pub listener_name: String,
    pub cluster_disposition: ResourceDisposition,
    pub route_config_disposition: ResourceDisposition,
    pub listener_disposition: ResourceDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposeEndpointSource {
    ListenerPublicBaseUrl,
    Unconfigured,
}
impl ExposeEndpointSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ListenerPublicBaseUrl => "listener.public_base_url",
            Self::Unconfigured => "unconfigured",
        }
    }
}
