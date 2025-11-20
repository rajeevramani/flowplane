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
    Extension, Json,
};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, require_resource_access},
    auth::models::AuthContext,
    errors::Error,
    services::ClusterService,
};

use validation::{cluster_parts_from_body, cluster_response_from_data, ClusterConfigParts};

// === Helper Functions ===

/// Verify that a cluster belongs to one of the user's teams or is global.
/// Returns the cluster if authorized, otherwise returns NotFound error (to avoid leaking existence).
async fn verify_cluster_access(
    cluster: crate::storage::ClusterData,
    team_scopes: &[String],
) -> Result<crate::storage::ClusterData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(cluster);
    }

    // Check if cluster is global (team = NULL) or belongs to one of user's teams
    match &cluster.team {
        None => Ok(cluster), // Global cluster, accessible to all
        Some(cluster_team) => {
            if team_scopes.contains(cluster_team) {
                Ok(cluster)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team,
                        cluster_team,
                        "clusters",
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!("Cluster with name '{}' not found", cluster.name)))
            }
        }
    }
}

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
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<types::CreateClusterBody>,
) -> Result<(StatusCode, Json<types::ClusterResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "clusters", "write", Some(&payload.team))?;

    let ClusterConfigParts { name, service_name, config } =
        cluster_parts_from_body(payload.clone());

    // Use explicit team from request
    let team = Some(payload.team.clone());

    let service = ClusterService::new(state.xds_state.clone());
    let created = service
        .create_cluster(name, service_name, config.clone(), team)
        .await
        .map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(types::ClusterResponse {
            name: created.name.clone(),
            team: created.team.unwrap_or_else(|| "unknown".to_string()),
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
    Extension(context): Extension<AuthContext>,
    Query(params): Query<types::ListClustersQuery>,
) -> Result<Json<Vec<types::ClusterResponse>>, ApiError> {
    // Authorization: require clusters:read scope
    require_resource_access(&context, "clusters", "read", None)?;

    // Extract team scopes from auth context for filtering
    let team_scopes = extract_team_scopes(&context);

    // Get repository and apply team filtering
    let repository = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Cluster repository unavailable"))?;

    let rows = repository
        .list_by_teams(&team_scopes, true, params.limit, params.offset) // REST API: include default resources
        .await
        .map_err(ApiError::from)?;

    let service = ClusterService::new(state.xds_state.clone());
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
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<types::ClusterResponse>, ApiError> {
    // Authorization: require clusters:read scope
    require_resource_access(&context, "clusters", "read", None)?;

    // Extract team scopes for access verification
    let team_scopes = extract_team_scopes(&context);

    let service = ClusterService::new(state.xds_state.clone());
    let cluster = service.get_cluster(&name).await.map_err(ApiError::from)?;

    // Verify the cluster belongs to one of the user's teams or is global
    let cluster = verify_cluster_access(cluster, &team_scopes).await?;

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
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
    Json(payload): Json<types::CreateClusterBody>,
) -> Result<Json<types::ClusterResponse>, ApiError> {
    // Authorization: require clusters:write scope
    require_resource_access(&context, "clusters", "write", None)?;

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

    // Extract team scopes and verify access before updating
    let team_scopes = extract_team_scopes(&context);
    let service = ClusterService::new(state.xds_state.clone());

    // Get existing cluster to verify access
    let existing = service.get_cluster(&name).await.map_err(ApiError::from)?;
    verify_cluster_access(existing, &team_scopes).await?;

    // Perform the update
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
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require clusters:write scope (delete is a write operation)
    require_resource_access(&context, "clusters", "write", None)?;

    // Extract team scopes and verify access before deleting
    let team_scopes = extract_team_scopes(&context);
    let service = ClusterService::new(state.xds_state.clone());

    // Get existing cluster to verify access
    let existing = service.get_cluster(&name).await.map_err(ApiError::from)?;
    verify_cluster_access(existing, &team_scopes).await?;

    // Perform the deletion
    service.delete_cluster(&name).await.map_err(ApiError::from)?;
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
    use sqlx::Executor;
    use std::sync::Arc;

    use crate::auth::models::AuthContext;
    use crate::config::SimpleXdsConfig;
    use crate::storage::{create_pool, DatabaseConfig};
    use crate::xds::XdsState;

    use types::{
        CircuitBreakerThresholdsRequest, CircuitBreakersRequest, CreateClusterBody,
        EndpointRequest, HealthCheckRequest, ListClustersQuery, OutlierDetectionRequest,
    };

    /// Create an admin AuthContext for testing with full permissions
    fn admin_context() -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("test-token"),
            "test-admin".to_string(),
            vec!["admin:all".to_string()],
        )
    }

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
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                import_id TEXT,
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
            team: "test-team".into(),
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

        let response = create_cluster_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(body.clone()),
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
        let state = setup_state().await;
        let mut body = sample_request();
        body.endpoints.clear();

        let err = create_cluster_handler(State(state), Extension(admin_context()), Json(body))
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
            create_cluster_handler(State(state.clone()), Extension(admin_context()), Json(body))
                .await
                .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let response = list_clusters_handler(
            State(state),
            Extension(admin_context()),
            Query(ListClustersQuery::default()),
        )
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
            create_cluster_handler(State(state.clone()), Extension(admin_context()), Json(body))
                .await
                .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let response = get_cluster_handler(
            State(state),
            Extension(admin_context()),
            Path("api-cluster".to_string()),
        )
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

        let (_status, Json(created)) = create_cluster_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(body.clone()),
        )
        .await
        .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        body.service_name = Some("renamed".into());
        body.lb_policy = Some("LEAST_REQUEST".into());

        let response = update_cluster_handler(
            State(state.clone()),
            Extension(admin_context()),
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
            create_cluster_handler(State(state.clone()), Extension(admin_context()), Json(body))
                .await
                .expect("create cluster");
        assert_eq!(created.name, "api-cluster");

        let status = delete_cluster_handler(
            State(state.clone()),
            Extension(admin_context()),
            Path("api-cluster".to_string()),
        )
        .await
        .expect("delete cluster");
        assert_eq!(status, StatusCode::NO_CONTENT);

        let repo = state.xds_state.cluster_repository.as_ref().cloned().expect("repository");
        let result = repo.get_by_name("api-cluster").await;
        assert!(result.is_err());
    }

    // === Team Isolation Tests ===

    /// Create a team-scoped AuthContext for testing
    fn team_context(team: &str, resource: &str, actions: &[&str]) -> AuthContext {
        let scopes =
            actions.iter().map(|action| format!("team:{}:{}:{}", team, resource, action)).collect();
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("test-token"),
            format!("{}-user", team),
            scopes,
        )
    }

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
        };

        repo.create(request).await?;
        Ok(())
    }

    #[tokio::test]
    async fn list_clusters_filters_by_team() {
        let state = setup_state().await;

        // Insert clusters for different teams
        insert_cluster_with_team(&state, "team-a-cluster", Some("team-a"))
            .await
            .expect("insert team-a cluster");
        insert_cluster_with_team(&state, "team-b-cluster", Some("team-b"))
            .await
            .expect("insert team-b cluster");
        insert_cluster_with_team(&state, "global-cluster", None)
            .await
            .expect("insert global cluster");

        // User with team-a scope should see team-a and global clusters
        let team_a_context = team_context("team-a", "clusters", &["read"]);
        let response = list_clusters_handler(
            State(state.clone()),
            Extension(team_a_context),
            Query(ListClustersQuery::default()),
        )
        .await
        .expect("list clusters for team-a");

        let clusters = response.0;
        assert_eq!(clusters.len(), 2);
        let names: Vec<&str> = clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"team-a-cluster"));
        assert!(names.contains(&"global-cluster"));
        assert!(!names.contains(&"team-b-cluster"));

        // User with team-b scope should see team-b and global clusters
        let team_b_context = team_context("team-b", "clusters", &["read"]);
        let response = list_clusters_handler(
            State(state.clone()),
            Extension(team_b_context),
            Query(ListClustersQuery::default()),
        )
        .await
        .expect("list clusters for team-b");

        let clusters = response.0;
        assert_eq!(clusters.len(), 2);
        let names: Vec<&str> = clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"team-b-cluster"));
        assert!(names.contains(&"global-cluster"));
        assert!(!names.contains(&"team-a-cluster"));

        // Admin should see all clusters
        let response = list_clusters_handler(
            State(state.clone()),
            Extension(admin_context()),
            Query(ListClustersQuery::default()),
        )
        .await
        .expect("list clusters for admin");

        let clusters = response.0;
        assert_eq!(clusters.len(), 3);
    }

    #[tokio::test]
    async fn get_cluster_rejects_cross_team_access() {
        let state = setup_state().await;

        // Insert a cluster for team-a
        insert_cluster_with_team(&state, "team-a-cluster", Some("team-a"))
            .await
            .expect("insert team-a cluster");

        // User from team-b tries to get team-a's cluster - should get 404
        let team_b_context = team_context("team-b", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path("team-a-cluster".to_string()),
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
        let team_a_context = team_context("team-a", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path("team-a-cluster".to_string()),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_cluster_allows_global_cluster_access() {
        let state = setup_state().await;

        // Insert a global cluster (no team)
        insert_cluster_with_team(&state, "global-cluster", None)
            .await
            .expect("insert global cluster");

        // Users from any team can access global clusters
        let team_a_context = team_context("team-a", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path("global-cluster".to_string()),
        )
        .await;
        assert!(result.is_ok());

        let team_b_context = team_context("team-b", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path("global-cluster".to_string()),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn update_cluster_rejects_cross_team_access() {
        let state = setup_state().await;

        // Insert a cluster for team-a
        insert_cluster_with_team(&state, "team-a-cluster", Some("team-a"))
            .await
            .expect("insert team-a cluster");

        let mut update_body = sample_request();
        update_body.name = "team-a-cluster".to_string();
        update_body.service_name = Some("updated".to_string());

        // User from team-b tries to update team-a's cluster - should get 404
        let team_b_context = team_context("team-b", "clusters", &["write"]);
        let result = update_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path("team-a-cluster".to_string()),
            Json(update_body.clone()),
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
        let team_a_context = team_context("team-a", "clusters", &["write"]);
        let result = update_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path("team-a-cluster".to_string()),
            Json(update_body),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_cluster_rejects_cross_team_access() {
        let state = setup_state().await;

        // Insert clusters for different teams
        insert_cluster_with_team(&state, "team-a-cluster", Some("team-a"))
            .await
            .expect("insert team-a cluster");
        insert_cluster_with_team(&state, "team-b-cluster", Some("team-b"))
            .await
            .expect("insert team-b cluster");

        // User from team-a tries to delete team-b's cluster - should get 404
        let team_a_context = team_context("team-a", "clusters", &["write"]);
        let result = delete_cluster_handler(
            State(state.clone()),
            Extension(team_a_context.clone()),
            Path("team-b-cluster".to_string()),
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
            Path("team-a-cluster".to_string()),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn team_scoped_user_creates_team_scoped_cluster() {
        let state = setup_state().await;
        let mut body = sample_request();
        body.team = "team-a".to_string(); // Explicitly set team to match user's scope

        // Team-scoped user creates a cluster
        let team_a_context = team_context("team-a", "clusters", &["write"]);
        let (_status, Json(created)) =
            create_cluster_handler(State(state.clone()), Extension(team_a_context), Json(body))
                .await
                .expect("create cluster");

        // Verify the cluster was assigned to team-a
        let repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let stored = repo.get_by_name(&created.name).await.expect("stored cluster");
        assert_eq!(stored.team, Some("team-a".to_string()));

        // Verify that team-b user cannot access it
        let team_b_context = team_context("team-b", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(team_b_context),
            Path(created.name.clone()),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn admin_user_creates_team_owned_cluster() {
        let state = setup_state().await;
        let mut body = sample_request();
        body.team = "platform".to_string(); // Admin explicitly specifies team

        // Admin creates a cluster for a specific team
        let (_status, Json(created)) =
            create_cluster_handler(State(state.clone()), Extension(admin_context()), Json(body))
                .await
                .expect("create cluster");

        // Verify the cluster is owned by the platform team
        let repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let stored = repo.get_by_name(&created.name).await.expect("stored cluster");
        assert_eq!(stored.team, Some("platform".to_string()));

        // Verify that team users with platform team scope can access it
        let platform_team_context = team_context("platform", "clusters", &["read"]);
        let result = get_cluster_handler(
            State(state.clone()),
            Extension(platform_team_context),
            Path(created.name.clone()),
        )
        .await;
        assert!(result.is_ok());

        // Verify that team-a users cannot access platform team cluster
        let team_a_context = team_context("team-a", "clusters", &["read"]);
        let result_team_a = get_cluster_handler(
            State(state.clone()),
            Extension(team_a_context),
            Path(created.name.clone()),
        )
        .await;
        assert!(result_team_a.is_err());
    }
}
