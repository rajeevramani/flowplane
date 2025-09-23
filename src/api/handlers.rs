use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tracing::{error, info};
use validator::Validate;

use crate::{
    errors::Error,
    storage::{ClusterData, CreateClusterRequest},
};

use super::error::ApiError;
use super::routes::ApiState;

#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateClusterBody {
    #[validate(length(min = 1))]
    pub name: String,

    #[validate(length(min = 1))]
    pub service_name: String,

    pub endpoints: Vec<Value>,

    #[serde(default)]
    pub connect_timeout_seconds: Option<u64>,

    #[serde(default)]
    pub use_tls: Option<bool>,

    #[serde(default)]
    pub tls_server_name: Option<String>,

    #[serde(default)]
    pub dns_lookup_family: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResponse {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub version: i64,
}

impl From<ClusterData> for ClusterResponse {
    fn from(value: ClusterData) -> Self {
        Self {
            id: value.id,
            name: value.name,
            service_name: value.service_name,
            version: value.version,
        }
    }
}

pub async fn create_cluster_handler(
    State(state): State<ApiState>,
    Json(payload): Json<CreateClusterBody>,
) -> Result<(StatusCode, Json<ClusterResponse>), ApiError> {
    payload
        .validate()
        .map_err(|err| ApiError::from(Error::from(err)))?;

    let CreateClusterBody {
        name,
        service_name,
        endpoints,
        connect_timeout_seconds,
        use_tls,
        tls_server_name,
        dns_lookup_family,
    } = payload;

    if endpoints.is_empty() {
        return Err(ApiError::from(Error::validation(
            "endpoints cannot be empty",
        )));
    }

    if endpoints
        .iter()
        .any(|ep| crate::xds::resources::parse_endpoint(ep).is_none())
    {
        return Err(ApiError::from(Error::validation(
            "each endpoint must be 'host:port' or an object with host/port",
        )));
    }

    let repository = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Cluster repository not configured"))?;

    let mut configuration = Map::new();
    configuration.insert("type".to_string(), Value::String("STATIC".to_string()));
    configuration.insert("endpoints".to_string(), Value::Array(endpoints));
    configuration.insert(
        "connect_timeout_seconds".to_string(),
        json!(connect_timeout_seconds.unwrap_or(5)),
    );

    if let Some(use_tls) = use_tls {
        configuration.insert("use_tls".to_string(), Value::Bool(use_tls));
    }

    if let Some(tls_server_name) = tls_server_name {
        if !tls_server_name.is_empty() {
            configuration.insert(
                "tls_server_name".to_string(),
                Value::String(tls_server_name),
            );
        }
    }

    if let Some(dns_lookup_family) = dns_lookup_family {
        if !dns_lookup_family.is_empty() {
            configuration.insert(
                "dns_lookup_family".to_string(),
                Value::String(dns_lookup_family),
            );
        }
    }

    let request = CreateClusterRequest {
        name,
        service_name,
        configuration: Value::Object(configuration),
    };

    let created = repository.create(request).await.map_err(ApiError::from)?;

    info!(
        cluster_id = %created.id,
        cluster_name = %created.name,
        "Cluster created via API"
    );

    state
        .xds_state
        .refresh_clusters_from_repository()
        .await
        .map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after cluster creation");
            ApiError::from(err)
        })?;

    Ok((StatusCode::CREATED, Json(created.into())))
}
