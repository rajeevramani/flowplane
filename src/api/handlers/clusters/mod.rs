//! Cluster configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy cluster configurations through
//! the REST API, with validation and XDS state synchronization.
//!
//! The handlers use the internal API layer (`ClusterOperations`) for unified
//! validation and team-based access control.

mod types;
mod validation;

// Re-export public types for backward compatibility
pub use types::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};

use super::pagination::{PaginatedResponse, PaginationQuery};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::instrument;

use crate::{
    api::{
        error::{ApiError, JsonBody},
        handlers::team_access::{
            require_resource_access_resolved, resolve_rest_auth, resolve_rest_auth_for_team,
            resolve_team_name,
        },
        routes::ApiState,
    },
    auth::models::AuthContext,
    internal_api::{
        ClusterOperations, CreateClusterRequest, ListClustersRequest, UpdateClusterRequest,
    },
    services::ClusterService,
};

use validation::{cluster_parts_from_body, cluster_response_from_data, ClusterConfigParts};

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/clusters",
    request_body = CreateClusterBody,
    responses(
        (status = 201, description = "Cluster created", body = ClusterResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 503, description = "Cluster repository unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name")
    ),
    tag = "Clusters"
)]
#[instrument(skip(state, payload), fields(team = %team, cluster_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_cluster_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    JsonBody(payload): JsonBody<types::CreateClusterBody>,
) -> Result<(StatusCode, Json<types::ClusterResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    // Authorization
    require_resource_access_resolved(&state, &context, "clusters", "create", Some(&team)).await?;

    // Convert REST body to internal request
    let ClusterConfigParts { name, service_name, config } =
        cluster_parts_from_body(payload.clone())?;

    let internal_req =
        CreateClusterRequest { name, service_name, team: Some(team.clone()), config };

    // Use internal API layer
    let ops = ClusterOperations::new(state.xds_state.clone());
    let auth = resolve_rest_auth(&state, &context).await?;
    let result = ops.create(internal_req, &auth).await?;

    // Convert to response
    let service = ClusterService::new(state.xds_state.clone());
    let response = cluster_response_from_data(&service, result.data)?;

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/clusters",
    params(
        ("team" = String, Path, description = "Team name"),
        PaginationQuery
    ),
    responses(
        (status = 200, description = "List of clusters", body = PaginatedResponse<ClusterResponse>),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "Clusters"
)]
#[instrument(skip(state, params), fields(team = %team, user_id = ?context.user_id, limit = %params.limit, offset = %params.offset))]
pub async fn list_clusters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<types::ClusterResponse>>, ApiError> {
    // Authorization
    require_resource_access_resolved(&state, &context, "clusters", "read", Some(&team)).await?;

    let (limit, offset) = params.clamp(1000);

    // Use internal API layer, scoped to the requested team only.
    let ops = ClusterOperations::new(state.xds_state.clone());
    let mut auth = resolve_rest_auth(&state, &context).await?;
    let team_id = resolve_team_name(&state, &team, context.org_id.as_ref()).await?;
    auth.allowed_teams = vec![team_id];
    let list_req = ListClustersRequest {
        limit: Some(limit as i32),
        offset: Some(offset as i32),
        include_defaults: true, // REST API includes default resources
    };

    let result = ops.list(list_req, &auth).await?;
    let total = result.count as i64;

    // Convert to response DTOs
    let service = ClusterService::new(state.xds_state.clone());
    let mut clusters = Vec::with_capacity(result.clusters.len());
    for row in result.clusters {
        clusters.push(cluster_response_from_data(&service, row)?);
    }

    Ok(Json(PaginatedResponse::new(clusters, total, limit, offset)))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/clusters/{name}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Name of the cluster")
    ),
    responses(
        (status = 200, description = "Cluster details", body = ClusterResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Cluster not found"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "Clusters"
)]
#[instrument(skip(state), fields(team = %path.0, cluster_name = %path.1, user_id = ?context.user_id))]
pub async fn get_cluster_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
) -> Result<Json<types::ClusterResponse>, ApiError> {
    let (team, name) = path;

    // Reject names with control characters (null bytes etc.) before hitting DB
    crate::api::error::validate_path_name(&name, "Cluster")?;

    // Authorization
    require_resource_access_resolved(&state, &context, "clusters", "read", Some(&team)).await?;

    // Use internal API layer — scope auth to the URL-path team for isolation
    let ops = ClusterOperations::new(state.xds_state.clone());
    let auth = resolve_rest_auth_for_team(&state, &context, &team).await?;
    let cluster = ops.get(&name, &auth).await?;

    // Convert to response DTO
    let service = ClusterService::new(state.xds_state.clone());
    let response = cluster_response_from_data(&service, cluster)?;

    Ok(Json(response))
}

#[utoipa::path(
    put,
    path = "/api/v1/teams/{team}/clusters/{name}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Name of the cluster")
    ),
    request_body = CreateClusterBody,
    responses(
        (status = 200, description = "Cluster updated", body = ClusterResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Cluster not found"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "Clusters"
)]
#[instrument(skip(state, payload), fields(team = %path.0, cluster_name = %path.1, user_id = ?context.user_id))]
pub async fn update_cluster_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
    JsonBody(payload): JsonBody<types::CreateClusterBody>,
) -> Result<Json<types::ClusterResponse>, ApiError> {
    let (team, name) = path;

    // Authorization
    require_resource_access_resolved(&state, &context, "clusters", "update", Some(&team)).await?;

    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    let ClusterConfigParts { name: payload_name, service_name, config } =
        cluster_parts_from_body(payload)?;

    if payload_name != name {
        return Err(ApiError::BadRequest(format!(
            "Payload cluster name '{}' does not match path '{}'",
            payload_name, name
        )));
    }

    // Convert to internal request
    let internal_req = UpdateClusterRequest { service_name: Some(service_name), config };

    // Use internal API layer — scope auth to the URL-path team for isolation
    let ops = ClusterOperations::new(state.xds_state.clone());
    let auth = resolve_rest_auth_for_team(&state, &context, &team).await?;
    let result = ops.update(&name, internal_req, &auth).await?;

    // Convert to response DTO
    let service = ClusterService::new(state.xds_state.clone());
    let response = cluster_response_from_data(&service, result.data)?;

    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/teams/{team}/clusters/{name}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Name of the cluster")
    ),
    responses(
        (status = 204, description = "Cluster deleted"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Cluster not found"),
        (status = 503, description = "Cluster repository unavailable"),
    ),
    tag = "Clusters"
)]
#[instrument(skip(state), fields(team = %path.0, cluster_name = %path.1, user_id = ?context.user_id))]
pub async fn delete_cluster_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (team, name) = path;

    // Authorization
    require_resource_access_resolved(&state, &context, "clusters", "delete", Some(&team)).await?;

    // Use internal API layer — scope auth to the URL-path team for isolation
    let ops = ClusterOperations::new(state.xds_state.clone());
    let auth = resolve_rest_auth_for_team(&state, &context, &team).await?;
    ops.delete(&name, &auth).await?;

    Ok(StatusCode::NO_CONTENT)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::response::IntoResponse;
    use axum::{Extension, Json};
    use serde_json::Value;

    use crate::api::test_utils::{create_test_state, scoped_team_auth_context, team_auth_context};
    use crate::storage::test_helpers::{PLATFORM_TEAM_ID, TEAM_A_ID, TEAM_B_ID};

    use types::{
        CircuitBreakerThresholdsRequest, CircuitBreakersRequest, CreateClusterBody,
        EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
    };
    // PaginationQuery and PaginatedResponse come from `use super::*;`

    // Use test_utils::create_test_state() which runs full migrations
    // and test_utils::team_auth_context() for team-scoped permissions

    async fn setup_state() -> (crate::storage::test_helpers::TestDatabase, ApiState) {
        use crate::api::test_utils::TestTeamBuilder;

        let (_db, state) = create_test_state().await;

        // Create commonly used teams for tests
        let cluster_repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let pool = cluster_repo.pool().clone();

        // Create teams that tests commonly reference
        for team_name in &["test-team", "team-a", "team-b", "platform"] {
            TestTeamBuilder::new(team_name).insert(&pool).await;
        }

        (_db, state)
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
                min_hosts: Some(3),
            }),
            protocol_type: None,
        }
    }

    #[tokio::test]
    async fn create_cluster_applies_defaults_and_persists() {
        let (_db, state) = setup_state().await;
        let body = sample_request();

        let response = create_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            JsonBody(body.clone()),
        )
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
        let (_db, state) = setup_state().await;
        let mut body = sample_request();
        body.endpoints.clear();

        let err = create_cluster_handler(
            State(state),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            JsonBody(body),
        )
        .await
        .expect_err("expected validation error");

        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_clusters_returns_created_cluster() {
        let (_db, state) = setup_state().await;
        let body = sample_request();

        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            JsonBody(body),
        )
        .await
        .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let response = list_clusters_handler(
            State(state),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await
        .expect("list clusters");

        let clusters = &response.0.items;
        // Seed data adds global clusters; verify our created cluster is present
        assert!(clusters.iter().any(|c| c.name == "api-cluster"));
    }

    #[tokio::test]
    async fn get_cluster_returns_cluster_details() {
        let (_db, state) = setup_state().await;
        let body = sample_request();

        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            JsonBody(body),
        )
        .await
        .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let response = get_cluster_handler(
            State(state),
            Extension(team_auth_context("test-team")),
            Path(("test-team".to_string(), "api-cluster".to_string())),
        )
        .await
        .expect("get cluster");

        let cluster = response.0;
        assert_eq!(cluster.name, "api-cluster");
        assert_eq!(cluster.config.endpoints.len(), 1);
    }

    #[tokio::test]
    async fn update_cluster_persists_changes() {
        let (_db, state) = setup_state().await;
        let mut body = sample_request();

        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            JsonBody(body.clone()),
        )
        .await
        .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        body.service_name = Some("renamed".into());
        body.lb_policy = Some("LEAST_REQUEST".into());

        let response = update_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path(("test-team".to_string(), "api-cluster".to_string())),
            JsonBody(body),
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
        let (_db, state) = setup_state().await;
        let body = sample_request();

        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path("test-team".to_string()),
            JsonBody(body),
        )
        .await
        .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let status = delete_cluster_handler(
            State(state.clone()),
            Extension(team_auth_context("test-team")),
            Path(("test-team".to_string(), "api-cluster".to_string())),
        )
        .await
        .expect("delete cluster");
        assert_eq!(status, StatusCode::NO_CONTENT);

        let repo = state.xds_state.cluster_repository.as_ref().cloned().expect("repository");
        let result = repo.get_by_name("api-cluster").await;
        assert!(result.is_err());
    }

    // === Team Isolation Tests ===

    /// Directly insert a cluster into the database with a team assignment
    async fn insert_cluster_with_team(
        state: &ApiState,
        name: &str,
        team: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let config = serde_json::json!({
            "endpoints": [{"host": "10.0.0.1", "port": 8080}],
            "connectTimeout": "7s",
            "useTls": false,
        });

        let request = crate::storage::CreateClusterRequest {
            name: name.to_string(),
            service_name: name.to_string(),
            configuration: config,
            team: team.map(String::from),
            import_id: None,
        };

        repo.create(request).await?;
        Ok(())
    }

    #[tokio::test]
    async fn list_clusters_filters_by_team() {
        let (_db, state) = setup_state().await;

        // Insert clusters for different teams
        insert_cluster_with_team(&state, "team-a-cluster", Some(TEAM_A_ID))
            .await
            .expect("insert team-a cluster");
        insert_cluster_with_team(&state, "team-b-cluster", Some(TEAM_B_ID))
            .await
            .expect("insert team-b cluster");
        insert_cluster_with_team(&state, "global-cluster", None)
            .await
            .expect("insert global cluster");

        // User with team-a scope should see team-a and global clusters
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["read"]);
        let response = list_clusters_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path("team-a".to_string()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await
        .expect("list clusters for team-a");

        let clusters = &response.0.items;
        // Should see team-a-cluster + global clusters (seed + test-created)
        let names: Vec<&str> = clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"team-a-cluster"));
        assert!(names.contains(&"global-cluster"));
        assert!(!names.contains(&"team-b-cluster"));

        // User with team-b scope should see team-b and global clusters
        let team_b_context = scoped_team_auth_context("team-b", "clusters", &["read"]);
        let response = list_clusters_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path("team-b".to_string()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await
        .expect("list clusters for team-b");

        let clusters = &response.0.items;
        let names: Vec<&str> = clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"team-b-cluster"));
        assert!(names.contains(&"global-cluster"));
        assert!(!names.contains(&"team-a-cluster"));
    }

    #[tokio::test]
    async fn get_cluster_rejects_cross_team_access() {
        let (_db, state) = setup_state().await;

        // Insert a cluster for team-a
        insert_cluster_with_team(&state, "team-a-cluster", Some(TEAM_A_ID))
            .await
            .expect("insert team-a cluster");

        // User from team-b tries to get team-a's cluster - should get 404
        let team_b_context = scoped_team_auth_context("team-b", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path(("team-b".to_string(), "team-a-cluster".to_string())),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ApiError::NotFound(msg) => {
                assert!(msg.contains("team-a-cluster"));
                assert!(msg.contains("not found"));
            }
            other => panic!("Expected NotFound error, got {:?}", other),
        }

        // User from team-a can access their own cluster
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path(("team-a".to_string(), "team-a-cluster".to_string())),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_cluster_allows_global_cluster_access() {
        let (_db, state) = setup_state().await;

        // Insert a global cluster (no team)
        insert_cluster_with_team(&state, "global-cluster", None)
            .await
            .expect("insert global cluster");

        // Users from any team can access global clusters
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path(("team-a".to_string(), "global-cluster".to_string())),
        )
        .await;
        assert!(result.is_ok());

        let team_b_context = scoped_team_auth_context("team-b", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path(("team-b".to_string(), "global-cluster".to_string())),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn update_cluster_rejects_cross_team_access() {
        let (_db, state) = setup_state().await;

        // Insert a cluster for team-a
        insert_cluster_with_team(&state, "team-a-cluster", Some(TEAM_A_ID))
            .await
            .expect("insert team-a cluster");

        let mut update_body = sample_request();
        update_body.name = "team-a-cluster".to_string();
        update_body.service_name = Some("updated".to_string());

        // User from team-b tries to update team-a's cluster - should get 404
        let team_b_context = scoped_team_auth_context("team-b", "clusters", &["update"]);
        let result = update_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path(("team-b".to_string(), "team-a-cluster".to_string())),
            JsonBody(update_body.clone()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ApiError::NotFound(msg) => {
                assert!(msg.contains("team-a-cluster"));
            }
            other => panic!("Expected NotFound error, got {:?}", other),
        }

        // User from team-a can update their own cluster
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["update"]);
        let result = update_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path(("team-a".to_string(), "team-a-cluster".to_string())),
            JsonBody(update_body),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_cluster_rejects_cross_team_access() {
        let (_db, state) = setup_state().await;

        // Insert clusters for different teams
        insert_cluster_with_team(&state, "team-a-cluster", Some(TEAM_A_ID))
            .await
            .expect("insert team-a cluster");
        insert_cluster_with_team(&state, "team-b-cluster", Some(TEAM_B_ID))
            .await
            .expect("insert team-b cluster");

        // User from team-a tries to delete team-b's cluster - should get 404
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["delete"]);
        let result = delete_cluster_handler(
            State(state.clone()),
            Extension(team_a_context.clone()),
            Path(("team-a".to_string(), "team-b-cluster".to_string())),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ApiError::NotFound(msg) => {
                assert!(msg.contains("team-b-cluster"));
            }
            other => panic!("Expected NotFound error, got {:?}", other),
        }

        // Verify team-b cluster still exists
        let repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let cluster = repo.get_by_name("team-b-cluster").await;
        assert!(cluster.is_ok());

        // User from team-a can delete their own cluster
        let result = delete_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path(("team-a".to_string(), "team-a-cluster".to_string())),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn team_scoped_user_creates_team_scoped_cluster() {
        let (_db, state) = setup_state().await;
        let body = sample_request();

        // Team-scoped user creates a cluster
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["create"]);
        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path("team-a".to_string()),
            JsonBody(body),
        )
        .await
        .expect("create cluster");

        // Verify the cluster was assigned to team-a
        let repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let stored = repo.get_by_name(&created.name).await.expect("stored cluster");
        assert_eq!(stored.team, Some(TEAM_A_ID.to_string()));

        // Verify that team-b user cannot access it
        let team_b_context = scoped_team_auth_context("team-b", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path(("team-b".to_string(), created.name.clone())),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn team_user_creates_team_owned_cluster() {
        let (_db, state) = setup_state().await;
        let body = sample_request();

        // Team user creates a cluster for their team
        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(scoped_team_auth_context("platform", "clusters", &["create"])),
            Path("platform".to_string()),
            JsonBody(body),
        )
        .await
        .expect("create cluster");

        // Verify the cluster is owned by the platform team
        let repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let stored = repo.get_by_name(&created.name).await.expect("stored cluster");
        assert_eq!(stored.team, Some(PLATFORM_TEAM_ID.to_string()));

        // Verify that team users with platform team scope can access it
        let platform_team_context = scoped_team_auth_context("platform", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(platform_team_context),
            Path(("platform".to_string(), created.name.clone())),
        )
        .await;
        assert!(result.is_ok());

        // Verify that team-a users cannot access platform team cluster
        let team_a_context = scoped_team_auth_context("team-a", "clusters", &["read"]);
        let result_team_a = get_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path(("team-a".to_string(), created.name.clone())),
        )
        .await;
        assert!(result_team_a.is_err());
    }
}
