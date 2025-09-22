use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tracing::info;
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

    #[validate(length(min = 1))]
    pub endpoints: Vec<String>,

    #[serde(default)]
    pub connect_timeout_seconds: Option<u64>,
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

    let repository = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Cluster repository not configured"))?;

    let configuration = serde_json::json!({
        "type": "EDS",
        "endpoints": payload.endpoints,
        "connect_timeout_seconds": payload.connect_timeout_seconds.unwrap_or(5),
    });

    let request = CreateClusterRequest {
        name: payload.name,
        service_name: payload.service_name,
        configuration,
    };

    let created = repository.create(request).await.map_err(ApiError::from)?;

    info!(
        cluster_id = %created.id,
        cluster_name = %created.name,
        "Cluster created via API"
    );

    state.xds_state.increment_version();

    Ok((StatusCode::CREATED, Json(created.into())))
}
