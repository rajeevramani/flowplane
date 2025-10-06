//! Cluster configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy cluster configurations through
//! the REST API, with validation and XDS state synchronization.

mod types;
mod validation;

// Re-export public types for backward compatibility
pub use types::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, ListClustersQuery, OutlierDetectionRequest,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::{
    api::{error::ApiError, routes::ApiState},
    errors::Error,
    services::ClusterService,
};

use validation::{cluster_parts_from_body, cluster_response_from_data, ClusterConfigParts};

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/clusters",
    request_body = CreateClusterBody,
    responses(
        (status = 201, description = "Cluster created", body = ClusterResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Cluster repository unavailable")
    ),
    tag = "clusters"
)]
pub async fn create_cluster_handler(
    State(state): State<ApiState>,
    Json(payload): Json<types::CreateClusterBody>,
) -> Result<(StatusCode, Json<types::ClusterResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    let ClusterConfigParts { name, service_name, config } = cluster_parts_from_body(payload);

    let service = ClusterService::new(state.xds_state.clone());
    let created =
        service.create_cluster(name, service_name, config.clone()).await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(types::ClusterResponse {
            name: created.name.clone(),
            service_name: created.service_name,
            config,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/clusters",
    params(
        ("limit" = Option<i32>, Query, description = "Maximum number of clusters to return"),
        ("offset" = Option<i32>, Query, description = "Offset for paginated results"),
    ),
    responses(
        (status = 200, description = "List of clusters", body = [ClusterResponse]),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "clusters"
)]
pub async fn list_clusters_handler(
    State(state): State<ApiState>,
    Query(params): Query<types::ListClustersQuery>,
) -> Result<Json<Vec<types::ClusterResponse>>, ApiError> {
    let service = ClusterService::new(state.xds_state.clone());
    let rows = service.list_clusters(params.limit, params.offset).await.map_err(ApiError::from)?;

    let mut clusters = Vec::with_capacity(rows.len());
    for row in rows {
        clusters.push(cluster_response_from_data(&service, row)?);
    }

    Ok(Json(clusters))
}

#[utoipa::path(
    get,
    path = "/api/v1/clusters/{name}",
    params(("name" = String, Path, description = "Name of the cluster")),
    responses(
        (status = 200, description = "Cluster details", body = ClusterResponse),
        (status = 404, description = "Cluster not found"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "clusters"
)]
pub async fn get_cluster_handler(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<types::ClusterResponse>, ApiError> {
    let service = ClusterService::new(state.xds_state.clone());
    let cluster = service.get_cluster(&name).await.map_err(ApiError::from)?;
    let response = cluster_response_from_data(&service, cluster)?;
    Ok(Json(response))
}

#[utoipa::path(
    put,
    path = "/api/v1/clusters/{name}",
    params(("name" = String, Path, description = "Name of the cluster")),
    request_body = CreateClusterBody,
    responses(
        (status = 200, description = "Cluster updated", body = ClusterResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Cluster not found"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "clusters"
)]
pub async fn update_cluster_handler(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Json(payload): Json<types::CreateClusterBody>,
) -> Result<Json<types::ClusterResponse>, ApiError> {
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    let ClusterConfigParts { name: payload_name, service_name, config } =
        cluster_parts_from_body(payload);

    if payload_name != name {
        return Err(ApiError::BadRequest(format!(
            "Payload cluster name '{}' does not match path '{}'",
            payload_name, name
        )));
    }

    let service = ClusterService::new(state.xds_state.clone());
    let updated =
        service.update_cluster(&name, service_name, config).await.map_err(ApiError::from)?;

    let response = cluster_response_from_data(&service, updated)?;
    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/clusters/{name}",
    params(("name" = String, Path, description = "Name of the cluster")),
    responses(
        (status = 204, description = "Cluster deleted"),
        (status = 404, description = "Cluster not found"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "clusters"
)]
pub async fn delete_cluster_handler(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let service = ClusterService::new(state.xds_state.clone());
    service.delete_cluster(&name).await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::response::IntoResponse;
    use axum::Json;
    use serde_json::Value;
    use sqlx::Executor;
    use std::sync::Arc;

    use crate::config::SimpleXdsConfig;
    use crate::storage::{create_pool, DatabaseConfig};
    use crate::xds::XdsState;

    use types::{
        CircuitBreakerThresholdsRequest, CircuitBreakersRequest, CreateClusterBody,
        EndpointRequest, HealthCheckRequest, ListClustersQuery, OutlierDetectionRequest,
    };

    fn create_test_config() -> DatabaseConfig {
        DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        }
    }

    async fn setup_state() -> ApiState {
        let pool = create_pool(&create_test_config()).await.expect("pool");

        // Create clusters table for repository usage.
        pool.execute(
            r#"
            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                service_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'platform_api')),
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, version)
            )
        "#,
        )
        .await
        .expect("create table");

        let state = XdsState::with_database(SimpleXdsConfig::default(), pool);
        ApiState { xds_state: Arc::new(state) }
    }

    fn sample_request() -> CreateClusterBody {
        CreateClusterBody {
            name: "api-cluster".into(),
            service_name: None,
            endpoints: vec![EndpointRequest { host: "10.0.0.1".into(), port: 8080 }],
            connect_timeout_seconds: Some(7),
            use_tls: Some(true),
            tls_server_name: Some("api.local".into()),
            dns_lookup_family: Some("AUTO".into()),
            lb_policy: Some("ROUND_ROBIN".into()),
            health_checks: vec![HealthCheckRequest {
                r#type: "http".into(),
                path: Some("/health".into()),
                host: None,
                method: Some("GET".into()),
                interval_seconds: Some(5),
                timeout_seconds: Some(2),
                healthy_threshold: Some(2),
                unhealthy_threshold: Some(3),
                expected_statuses: Some(vec![200]),
            }],
            circuit_breakers: Some(CircuitBreakersRequest {
                default: Some(CircuitBreakerThresholdsRequest {
                    max_connections: Some(100),
                    max_pending_requests: Some(50),
                    max_requests: Some(200),
                    max_retries: Some(3),
                }),
                high: None,
            }),
            outlier_detection: Some(OutlierDetectionRequest {
                consecutive_5xx: Some(5),
                interval_seconds: Some(10),
                base_ejection_time_seconds: Some(60),
                max_ejection_percent: Some(50),
            }),
        }
    }

    #[tokio::test]
    async fn create_cluster_applies_defaults_and_persists() {
        let state = setup_state().await;
        let body = sample_request();

        let response = create_cluster_handler(State(state.clone()), Json(body.clone()))
            .await
            .expect("handler response");

        assert_eq!(response.0, StatusCode::CREATED);
        let payload = response.1 .0;
        assert_eq!(payload.name, "api-cluster");
        // service name defaults to cluster name when omitted.
        assert_eq!(payload.service_name, "api-cluster");
        assert_eq!(payload.config.endpoints.len(), 1);
        assert!(payload.config.use_tls.unwrap());

        // verify row persisted.
        let repo = state.xds_state.cluster_repository.as_ref().cloned().expect("repository");
        let stored = repo.get_by_name("api-cluster").await.expect("stored cluster");
        let config: Value = serde_json::from_str(&stored.configuration).expect("json");
        assert_eq!(config["useTls"], Value::Bool(true));
    }

    #[tokio::test]
    async fn create_cluster_validates_missing_endpoints() {
        let state = setup_state().await;
        let mut body = sample_request();
        body.endpoints.clear();

        let err = create_cluster_handler(State(state), Json(body))
            .await
            .expect_err("expected validation error");

        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_clusters_returns_created_cluster() {
        let state = setup_state().await;
        let body = sample_request();

        let (_status, Json(created)) =
            create_cluster_handler(State(state.clone()), Json(body)).await.expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let response = list_clusters_handler(State(state), Query(ListClustersQuery::default()))
            .await
            .expect("list clusters");

        let clusters = response.0;
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].name, "api-cluster");
    }

    #[tokio::test]
    async fn get_cluster_returns_cluster_details() {
        let state = setup_state().await;
        let body = sample_request();

        let (_status, Json(created)) =
            create_cluster_handler(State(state.clone()), Json(body)).await.expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let response = get_cluster_handler(State(state), Path("api-cluster".to_string()))
            .await
            .expect("get cluster");

        let cluster = response.0;
        assert_eq!(cluster.name, "api-cluster");
        assert_eq!(cluster.config.endpoints.len(), 1);
    }

    #[tokio::test]
    async fn update_cluster_persists_changes() {
        let state = setup_state().await;
        let mut body = sample_request();

        let (_status, Json(created)) =
            create_cluster_handler(State(state.clone()), Json(body.clone()))
                .await
                .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        body.service_name = Some("renamed".into());
        body.lb_policy = Some("LEAST_REQUEST".into());

        let response = update_cluster_handler(
            State(state.clone()),
            Path("api-cluster".to_string()),
            Json(body),
        )
        .await
        .expect("update cluster");

        let cluster = response.0;
        assert_eq!(cluster.service_name, "renamed");
        assert_eq!(cluster.config.lb_policy.as_deref(), Some("LEAST_REQUEST"));

        let repo = state.xds_state.cluster_repository.as_ref().cloned().expect("repository");
        let stored = repo.get_by_name("api-cluster").await.expect("stored cluster");
        assert_eq!(stored.version, 2);
    }

    #[tokio::test]
    async fn delete_cluster_removes_record() {
        let state = setup_state().await;
        let body = sample_request();

        let (_status, Json(created)) =
            create_cluster_handler(State(state.clone()), Json(body)).await.expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let status = delete_cluster_handler(State(state.clone()), Path("api-cluster".to_string()))
            .await
            .expect("delete cluster");
        assert_eq!(status, StatusCode::NO_CONTENT);

        let repo = state.xds_state.cluster_repository.as_ref().cloned().expect("repository");
        let result = repo.get_by_name("api-cluster").await;
        assert!(result.is_err());
    }
}
