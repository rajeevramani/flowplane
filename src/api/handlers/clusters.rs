use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::{errors::Error, services::ClusterService, storage::ClusterData, xds::ClusterSpec};

use crate::api::error::ApiError;
use crate::api::routes::ApiState;

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "name": "my-service-cluster",
    "serviceName": "inventory-service",
    "endpoints": [
        {"host": "service1.example.com", "port": 8080},
        {"host": "service2.example.com", "port": 8080}
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "service.example.com",
    "lbPolicy": "ROUND_ROBIN",
    "healthChecks": [
        {
            "type": "http",
            "path": "/health",
            "intervalSeconds": 10,
            "timeoutSeconds": 5,
            "healthyThreshold": 2,
            "unhealthyThreshold": 3
        }
    ],
    "circuitBreakers": {
        "default": {
            "maxConnections": 100,
            "maxPendingRequests": 50,
            "maxRequests": 200,
            "maxRetries": 3
        }
    },
    "outlierDetection": {
        "consecutive5xx": 5,
        "intervalSeconds": 30,
        "baseEjectionTimeSeconds": 30,
        "maxEjectionPercent": 50
    }
}))]
pub struct CreateClusterBody {
    /// Unique name for the cluster.
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "my-service-cluster")]
    pub name: String,

    /// Optional service identifier exposed to clients (defaults to `name`).
    #[serde(default)]
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "inventory-service")]
    pub service_name: Option<String>,

    /// Upstream endpoints (host & port) for this cluster. At least one is required.
    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<EndpointRequest>)]
    pub endpoints: Vec<EndpointRequest>,

    /// Connection timeout in seconds (default: 5).
    #[serde(default)]
    #[schema(default = 5)]
    pub connect_timeout_seconds: Option<u64>,

    /// Enable TLS for upstream connections (default: false).
    #[serde(default)]
    #[schema(default = false)]
    pub use_tls: Option<bool>,

    /// Optional SNI server name to present during TLS handshake.
    #[serde(default)]
    pub tls_server_name: Option<String>,

    /// DNS lookup family for hostname endpoints (`AUTO`, `V4_ONLY`, `V6_ONLY`, `V4_PREFERRED`, `ALL`).
    #[serde(default)]
    #[schema(example = "AUTO")]
    pub dns_lookup_family: Option<String>,

    /// Load-balancer policy (`ROUND_ROBIN`, `LEAST_REQUEST`, `RANDOM`, `RING_HASH`, `MAGLEV`, `CLUSTER_PROVIDED`).
    #[serde(default)]
    #[schema(example = "ROUND_ROBIN")]
    pub lb_policy: Option<String>,

    /// Active health-check definitions.
    #[serde(default)]
    #[schema(value_type = Vec<HealthCheckRequest>)]
    pub health_checks: Vec<HealthCheckRequest>,

    /// Circuit breaker thresholds applied to the cluster.
    #[serde(default)]
    #[schema(value_type = CircuitBreakersRequest)]
    pub circuit_breakers: Option<CircuitBreakersRequest>,

    /// Passive outlier detection configuration.
    #[serde(default)]
    pub outlier_detection: Option<OutlierDetectionRequest>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({"host": "httpbin.org", "port": 443}))]
pub struct EndpointRequest {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "httpbin.org")]
    pub host: String,

    #[validate(range(min = 1, max = 65535))]
    #[schema(example = 443)]
    pub port: u16,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "type": "http",
    "path": "/health",
    "method": "GET",
    "intervalSeconds": 10,
    "timeoutSeconds": 5,
    "healthyThreshold": 2,
    "unhealthyThreshold": 3,
    "expectedStatuses": [200, 204]
}))]
pub struct HealthCheckRequest {
    #[serde(default = "default_health_check_type")]
    #[schema(example = "http")]
    pub r#type: String,

    /// HTTP path probed when `type` is `http` (default: `/health`).
    pub path: Option<String>,
    /// Host header override for HTTP health checks.
    pub host: Option<String>,
    /// HTTP method used for health probes.
    pub method: Option<String>,
    /// Interval between health probes in seconds.
    #[schema(example = 10)]
    pub interval_seconds: Option<u64>,
    /// Timeout for health probes in seconds.
    #[schema(example = 5)]
    pub timeout_seconds: Option<u64>,
    /// Number of consecutive successful probes before marking healthy.
    pub healthy_threshold: Option<u32>,
    /// Number of consecutive failed probes before marking unhealthy.
    pub unhealthy_threshold: Option<u32>,
    /// HTTP status codes treated as successful responses.
    pub expected_statuses: Option<Vec<u32>>,
}

fn default_health_check_type() -> String {
    "http".to_string()
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "default": {
        "maxConnections": 100,
        "maxPendingRequests": 50,
        "maxRequests": 200,
        "maxRetries": 3
    }
}))]
pub struct CircuitBreakersRequest {
    /// Thresholds applied to default-priority traffic.
    pub default: Option<CircuitBreakerThresholdsRequest>,
    /// Thresholds applied to high-priority traffic.
    pub high: Option<CircuitBreakerThresholdsRequest>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "maxConnections": 100,
    "maxPendingRequests": 50,
    "maxRequests": 200,
    "maxRetries": 3
}))]
pub struct CircuitBreakerThresholdsRequest {
    /// Maximum concurrent upstream connections.
    pub max_connections: Option<u32>,
    /// Maximum number of pending requests allowed.
    pub max_pending_requests: Option<u32>,
    /// Maximum in-flight requests.
    pub max_requests: Option<u32>,
    /// Maximum simultaneous retries.
    pub max_retries: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "consecutive5xx": 5,
    "intervalSeconds": 30,
    "baseEjectionTimeSeconds": 30,
    "maxEjectionPercent": 50
}))]
pub struct OutlierDetectionRequest {
    /// Number of consecutive 5xx responses before ejecting a host.
    pub consecutive_5xx: Option<u32>,
    /// Interval between outlier analysis sweeps (seconds).
    pub interval_seconds: Option<u64>,
    /// Base ejection time (seconds).
    pub base_ejection_time_seconds: Option<u64>,
    /// Maximum percentage of hosts that can be ejected simultaneously.
    pub max_ejection_percent: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "name": "my-service-cluster",
    "serviceName": "my-service-cluster",
    "config": {
        "endpoints": [
            {"host": "service1.example.com", "port": 8080},
            {"host": "service2.example.com", "port": 8080}
        ],
        "connectTimeoutSeconds": 5,
        "useTls": true,
        "tlsServerName": "service.example.com",
        "lbPolicy": "ROUND_ROBIN",
        "healthChecks": [
            {
                "type": "http",
                "path": "/health",
                "method": "GET",
                "intervalSeconds": 10,
                "timeoutSeconds": 5,
                "healthyThreshold": 2,
                "unhealthyThreshold": 3
            }
        ],
        "circuitBreakers": {
            "default": {
                "maxConnections": 100,
                "maxPendingRequests": 50,
                "maxRequests": 200,
                "maxRetries": 3
            }
        },
        "outlierDetection": {
            "consecutive5xx": 5,
            "intervalSeconds": 30,
            "baseEjectionTimeSeconds": 30,
            "maxEjectionPercent": 50
        }
    }
}))]
pub struct ClusterResponse {
    pub name: String,
    pub service_name: String,
    pub config: ClusterSpec,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListClustersQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Debug)]
struct ClusterConfigParts {
    name: String,
    service_name: String,
    config: ClusterSpec,
}

fn cluster_parts_from_body(payload: CreateClusterBody) -> ClusterConfigParts {
    let CreateClusterBody {
        name,
        service_name,
        endpoints,
        connect_timeout_seconds,
        use_tls,
        tls_server_name,
        dns_lookup_family,
        lb_policy,
        health_checks,
        circuit_breakers,
        outlier_detection,
    } = payload;

    let service_name = service_name.unwrap_or_else(|| name.clone());

    let config = ClusterSpec {
        endpoints: endpoints
            .into_iter()
            .map(|ep| crate::xds::EndpointSpec::Address { host: ep.host, port: ep.port })
            .collect(),
        connect_timeout_seconds,
        use_tls,
        tls_server_name,
        dns_lookup_family,
        lb_policy,
        health_checks: health_checks
            .into_iter()
            .map(|hc| {
                let HealthCheckRequest {
                    r#type,
                    path,
                    host,
                    method,
                    interval_seconds,
                    timeout_seconds,
                    healthy_threshold,
                    unhealthy_threshold,
                    expected_statuses,
                } = hc;

                match r#type.to_lowercase().as_str() {
                    "tcp" => crate::xds::HealthCheckSpec::Tcp {
                        interval_seconds,
                        timeout_seconds,
                        healthy_threshold,
                        unhealthy_threshold,
                    },
                    _ => crate::xds::HealthCheckSpec::Http {
                        path: path.unwrap_or_else(|| "/health".to_string()),
                        host,
                        method,
                        interval_seconds,
                        timeout_seconds,
                        healthy_threshold,
                        unhealthy_threshold,
                        expected_statuses,
                    },
                }
            })
            .collect(),
        circuit_breakers: circuit_breakers.map(|cb| crate::xds::CircuitBreakersSpec {
            default: cb.default.map(|d| crate::xds::CircuitBreakerThresholdsSpec {
                max_connections: d.max_connections,
                max_pending_requests: d.max_pending_requests,
                max_requests: d.max_requests,
                max_retries: d.max_retries,
            }),
            high: cb.high.map(|h| crate::xds::CircuitBreakerThresholdsSpec {
                max_connections: h.max_connections,
                max_pending_requests: h.max_pending_requests,
                max_requests: h.max_requests,
                max_retries: h.max_retries,
            }),
        }),
        outlier_detection: outlier_detection.map(|od| crate::xds::OutlierDetectionSpec {
            consecutive_5xx: od.consecutive_5xx,
            interval_seconds: od.interval_seconds,
            base_ejection_time_seconds: od.base_ejection_time_seconds,
            max_ejection_percent: od.max_ejection_percent,
        }),
        ..Default::default()
    };

    ClusterConfigParts { name, service_name, config }
}

fn cluster_response_from_data(
    service: &ClusterService,
    data: ClusterData,
) -> Result<ClusterResponse, ApiError> {
    let config = service.parse_config(&data).map_err(ApiError::from)?;
    Ok(ClusterResponse { name: data.name, service_name: data.service_name, config })
}

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
    Json(payload): Json<CreateClusterBody>,
) -> Result<(StatusCode, Json<ClusterResponse>), ApiError> {
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    let ClusterConfigParts { name, service_name, config } = cluster_parts_from_body(payload);

    let service = ClusterService::new(state.xds_state.clone());
    let created =
        service.create_cluster(name, service_name, config.clone()).await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(ClusterResponse {
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
    Query(params): Query<ListClustersQuery>,
) -> Result<Json<Vec<ClusterResponse>>, ApiError> {
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
) -> Result<Json<ClusterResponse>, ApiError> {
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
    Json(payload): Json<CreateClusterBody>,
) -> Result<Json<ClusterResponse>, ApiError> {
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
