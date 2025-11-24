//! Route configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy route configurations through
//! the REST API, with validation and XDS state synchronization.

mod types;
mod validation;

// Re-export public types for backward compatibility
pub use types::{
    HeaderMatchDefinition, ListRoutesQuery, PathMatchDefinition, QueryParameterMatchDefinition,
    RouteActionDefinition, RouteDefinition, RouteMatchDefinition, RouteResponse,
    RouteRuleDefinition, VirtualHostDefinition, WeightedClusterDefinition,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::{error, info};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
    auth::models::AuthContext,
    errors::Error,
    openapi::defaults::is_default_gateway_route,
    storage::{CreateRouteRepositoryRequest, RouteData, UpdateRouteRepositoryRequest},
};

use validation::{
    require_route_repository, route_response_from_data, summarize_route, validate_route_config,
    validate_route_payload,
};

// === Helper Functions ===

/// Verify that a route belongs to one of the user's teams or is global.
/// Returns the route if authorized, otherwise returns NotFound error (to avoid leaking existence).
async fn verify_route_access(
    route: RouteData,
    team_scopes: &[String],
) -> Result<RouteData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(route);
    }

    // Check if route is global (team = NULL) or belongs to one of user's teams
    match &route.team {
        None => Ok(route), // Global route, accessible to all
        Some(route_team) => {
            if team_scopes.contains(route_team) {
                Ok(route)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team, route_team, "routes",
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!("Route with name '{}' not found", route.name)))
            }
        }
    }
}

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/routes",
    request_body = RouteDefinition,
    responses(
        (status = 201, description = "Route created", body = RouteResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Route repository unavailable"),
    ),
    tag = "routes"
)]
pub async fn create_route_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<RouteDefinition>,
) -> Result<(StatusCode, Json<RouteResponse>), ApiError> {
    validate_route_payload(&payload)?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "routes", "write", Some(&payload.team))?;

    let route_repository = require_route_repository(&state)?;

    let xds_config = payload.to_xds_config().and_then(validate_route_config)?;

    let (path_prefix, cluster_summary) = summarize_route(&payload);
    let configuration = serde_json::to_value(&xds_config).map_err(|err| {
        ApiError::from(Error::internal(format!("Failed to serialize route definition: {}", err)))
    })?;

    // Use explicit team from request
    let team = Some(payload.team.clone());

    let request = CreateRouteRepositoryRequest {
        name: payload.name.clone(),
        path_prefix,
        cluster_name: cluster_summary,
        configuration,
        team,
        import_id: None,
        route_order: None,
        headers: None,
    };

    let created = route_repository.create(request).await.map_err(ApiError::from)?;

    info!(route_id = %created.id, route_name = %created.name, "Route created via API");

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after route creation");
        ApiError::from(err)
    })?;

    let response = RouteResponse {
        name: created.name,
        team: created.team.unwrap_or_default(),
        path_prefix: created.path_prefix,
        cluster_targets: created.cluster_name,
        import_id: created.import_id,
        route_order: created.route_order,
        config: payload,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/routes",
    params(
        ("limit" = Option<i32>, Query, description = "Maximum number of routes to return"),
        ("offset" = Option<i32>, Query, description = "Offset for paginated results"),
    ),
    responses(
        (status = 200, description = "List of routes", body = [RouteResponse]),
        (status = 503, description = "Route repository unavailable"),
    ),
    tag = "routes"
)]
pub async fn list_routes_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<types::ListRoutesQuery>,
) -> Result<Json<Vec<RouteResponse>>, ApiError> {
    // Authorization: require routes:read scope
    require_resource_access(&context, "routes", "read", None)?;

    // Extract team scopes from auth context for filtering
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_repository(&state)?;
    let rows = repository
        .list_by_teams(&team_scopes, true, params.limit, params.offset) // REST API: include default resources
        .await
        .map_err(ApiError::from)?;

    let mut routes = Vec::with_capacity(rows.len());
    for row in rows {
        routes.push(route_response_from_data(row)?);
    }

    Ok(Json(routes))
}

#[utoipa::path(
    get,
    path = "/api/v1/routes/{name}",
    params(("name" = String, Path, description = "Name of the route configuration")),
    responses(
        (status = 200, description = "Route details", body = RouteResponse),
        (status = 404, description = "Route not found"),
        (status = 503, description = "Route repository unavailable"),
    ),
    tag = "routes"
)]
pub async fn get_route_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<RouteResponse>, ApiError> {
    // Authorization: require routes:read scope
    require_resource_access(&context, "routes", "read", None)?;

    // Extract team scopes for access verification
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_repository(&state)?;
    let route = repository.get_by_name(&name).await.map_err(ApiError::from)?;

    // Verify the route belongs to one of the user's teams or is global
    let route = verify_route_access(route, &team_scopes).await?;

    Ok(Json(route_response_from_data(route)?))
}

#[utoipa::path(
    put,
    path = "/api/v1/routes/{name}",
    params(("name" = String, Path, description = "Name of the route configuration")),
    request_body = RouteDefinition,
    responses(
        (status = 200, description = "Route updated", body = RouteResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route not found"),
        (status = 503, description = "Route repository unavailable"),
    ),
    tag = "routes"
)]
pub async fn update_route_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
    Json(payload): Json<RouteDefinition>,
) -> Result<Json<RouteResponse>, ApiError> {
    // Authorization: require routes:write scope
    require_resource_access(&context, "routes", "write", None)?;

    validate_route_payload(&payload)?;

    if payload.name != name {
        return Err(ApiError::BadRequest(format!(
            "Payload route name '{}' does not match path '{}'",
            payload.name, name
        )));
    }

    // Extract team scopes and verify access before updating
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_repository(&state)?;
    let existing = repository.get_by_name(&payload.name).await.map_err(ApiError::from)?;

    // Verify the route belongs to one of the user's teams or is global
    verify_route_access(existing.clone(), &team_scopes).await?;

    let xds_config = payload.to_xds_config().and_then(validate_route_config)?;
    let (path_prefix, cluster_summary) = summarize_route(&payload);
    let configuration = serde_json::to_value(&xds_config).map_err(|err| {
        ApiError::from(Error::internal(format!("Failed to serialize route definition: {}", err)))
    })?;

    let update_request = UpdateRouteRepositoryRequest {
        path_prefix: Some(path_prefix.clone()),
        cluster_name: Some(cluster_summary.clone()),
        configuration: Some(configuration),
        team: None, // Don't modify team on update unless explicitly set
    };

    let updated = repository.update(&existing.id, update_request).await.map_err(ApiError::from)?;

    info!(route_id = %updated.id, route_name = %updated.name, "Route updated via API");

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after route update");
        ApiError::from(err)
    })?;

    let response = RouteResponse {
        name: updated.name,
        team: updated.team.unwrap_or_default(),
        path_prefix,
        cluster_targets: cluster_summary,
        import_id: updated.import_id,
        route_order: updated.route_order,
        config: payload,
    };

    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/routes/{name}",
    params(("name" = String, Path, description = "Name of the route configuration")),
    responses(
        (status = 204, description = "Route deleted"),
        (status = 404, description = "Route not found"),
        (status = 503, description = "Route repository unavailable"),
    ),
    tag = "routes"
)]
pub async fn delete_route_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require routes:write scope (delete is a write operation)
    require_resource_access(&context, "routes", "write", None)?;

    if is_default_gateway_route(&name) {
        return Err(ApiError::Conflict(
            "The default gateway route configuration cannot be deleted".to_string(),
        ));
    }

    // Extract team scopes and verify access before deleting
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_repository(&state)?;
    let existing = repository.get_by_name(&name).await.map_err(ApiError::from)?;

    // Verify the route belongs to one of the user's teams or is global
    verify_route_access(existing.clone(), &team_scopes).await?;

    repository.delete(&existing.id).await.map_err(ApiError::from)?;

    info!(route_id = %existing.id, route_name = %existing.name, "Route deleted via API");

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after route deletion");
        ApiError::from(err)
    })?;

    Ok(StatusCode::NO_CONTENT)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, Extension, Json};
    use serde_json::json;
    use sqlx::Executor;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::auth::models::AuthContext;
    use crate::config::SimpleXdsConfig;
    use crate::storage::{create_pool, CreateClusterRequest, DatabaseConfig};
    use crate::xds::filters::http::{
        local_rate_limit::{
            FractionalPercentDenominator, LocalRateLimitConfig, RuntimeFractionalPercentConfig,
            TokenBucketConfig,
        },
        HttpScopedConfig,
    };
    use crate::xds::route::RouteConfig as XdsRouteConfig;
    use crate::xds::XdsState;

    use types::{
        PathMatchDefinition, RouteActionDefinition, RouteDefinition, RouteMatchDefinition,
        RouteRuleDefinition, VirtualHostDefinition, WeightedClusterDefinition,
    };

    /// Create an admin AuthContext for testing with full permissions
    fn admin_context() -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("test-token"),
            "test-admin".to_string(),
            vec!["admin:all".to_string()],
        )
    }

    async fn setup_state() -> ApiState {
        let pool = create_pool(&DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        })
        .await
        .expect("pool");

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
            );

            CREATE TABLE IF NOT EXISTS routes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                path_prefix TEXT NOT NULL,
                cluster_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                import_id TEXT,
                route_order INTEGER,
                headers TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, version)
            );
        "#,
        )
        .await
        .expect("create tables");

        let state = XdsState::with_database(SimpleXdsConfig::default(), pool.clone());
        let api_state = ApiState { xds_state: Arc::new(state) };

        // Seed a cluster for route references
        let cluster_repo =
            api_state.xds_state.cluster_repository.as_ref().cloned().expect("cluster repo");

        cluster_repo
            .create(CreateClusterRequest {
                name: "api-cluster".into(),
                service_name: "api-cluster".into(),
                configuration: json!({
                    "endpoints": ["127.0.0.1:8080"]
                }),
                team: None, // Test cluster without team assignment
                import_id: None,
            })
            .await
            .expect("seed cluster");

        cluster_repo
            .create(CreateClusterRequest {
                name: "shadow".into(),
                service_name: "shadow".into(),
                configuration: json!({
                    "endpoints": ["127.0.0.1:8181"]
                }),
                team: None, // Test cluster without team assignment
                import_id: None,
            })
            .await
            .expect("seed shadow cluster");

        api_state
    }

    fn sample_route_definition() -> RouteDefinition {
        RouteDefinition {
            team: "test-team".into(),
            name: "primary-routes".into(),
            virtual_hosts: vec![VirtualHostDefinition {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRuleDefinition {
                    name: Some("api".into()),
                    r#match: RouteMatchDefinition {
                        path: PathMatchDefinition::Prefix { value: "/api".into() },
                        headers: vec![],
                        query_parameters: vec![],
                    },
                    action: RouteActionDefinition::Forward {
                        cluster: "api-cluster".into(),
                        timeout_seconds: Some(5),
                        prefix_rewrite: None,
                        template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        }
    }

    #[tokio::test]
    async fn create_route_persists_configuration() {
        let state = setup_state().await;

        let payload = sample_route_definition();
        let (status, Json(created)) = create_route_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create route");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created.name, "primary-routes");
        assert_eq!(created.config.virtual_hosts.len(), 1);

        let repo = state.xds_state.route_repository.as_ref().cloned().expect("route repo");
        let stored = repo.get_by_name("primary-routes").await.expect("stored route");
        assert_eq!(stored.path_prefix, "/api");
        assert!(stored.cluster_name.contains("api-cluster"));
    }

    #[tokio::test]
    async fn list_routes_returns_entries() {
        let state = setup_state().await;

        let payload = sample_route_definition();
        let (status, _) =
            create_route_handler(State(state.clone()), Extension(admin_context()), Json(payload))
                .await
                .expect("create route");
        assert_eq!(status, StatusCode::CREATED);

        let response = list_routes_handler(
            State(state),
            Extension(admin_context()),
            Query(types::ListRoutesQuery::default()),
        )
        .await
        .expect("list routes");

        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].name, "primary-routes");
    }

    #[tokio::test]
    async fn get_route_returns_definition() {
        let state = setup_state().await;
        let payload = sample_route_definition();
        let (status, _) =
            create_route_handler(State(state.clone()), Extension(admin_context()), Json(payload))
                .await
                .expect("create route");
        assert_eq!(status, StatusCode::CREATED);

        let response = get_route_handler(
            State(state),
            Extension(admin_context()),
            Path("primary-routes".into()),
        )
        .await
        .expect("get route");

        assert_eq!(response.0.name, "primary-routes");
        assert_eq!(response.0.config.virtual_hosts[0].routes.len(), 1);
    }

    #[tokio::test]
    async fn update_route_applies_changes() {
        let state = setup_state().await;
        let mut payload = sample_route_definition();
        let (status, _) = create_route_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create route");
        assert_eq!(status, StatusCode::CREATED);

        payload.virtual_hosts[0].routes[0].action = RouteActionDefinition::Weighted {
            clusters: vec![
                WeightedClusterDefinition {
                    name: "api-cluster".into(),
                    weight: 60,
                    typed_per_filter_config: HashMap::new(),
                },
                WeightedClusterDefinition {
                    name: "shadow".into(),
                    weight: 40,
                    typed_per_filter_config: HashMap::new(),
                },
            ],
            total_weight: Some(100),
        };
        payload.virtual_hosts[0].routes[0].typed_per_filter_config.insert(
            "envoy.filters.http.local_ratelimit".into(),
            HttpScopedConfig::LocalRateLimit(LocalRateLimitConfig {
                stat_prefix: "per_route".into(),
                token_bucket: Some(TokenBucketConfig {
                    max_tokens: 10,
                    tokens_per_fill: Some(10),
                    fill_interval_ms: 60_000,
                }),
                status_code: Some(429),
                filter_enabled: Some(RuntimeFractionalPercentConfig {
                    runtime_key: None,
                    numerator: 100,
                    denominator: FractionalPercentDenominator::Hundred,
                }),
                filter_enforced: Some(RuntimeFractionalPercentConfig {
                    runtime_key: None,
                    numerator: 100,
                    denominator: FractionalPercentDenominator::Hundred,
                }),
                per_downstream_connection: Some(false),
                rate_limited_as_resource_exhausted: None,
                max_dynamic_descriptors: None,
                always_consume_default_token_bucket: Some(false),
            }),
        );

        let response = update_route_handler(
            State(state.clone()),
            Extension(admin_context()),
            Path("primary-routes".into()),
            Json(payload.clone()),
        )
        .await
        .expect("update route");

        assert!(response.0.cluster_targets.contains("api-cluster"));
        if let Some(HttpScopedConfig::LocalRateLimit(cfg)) = response.0.config.virtual_hosts[0]
            .routes[0]
            .typed_per_filter_config
            .get("envoy.filters.http.local_ratelimit")
        {
            let bucket = cfg.token_bucket.as_ref().expect("route-level token bucket present");
            assert_eq!(bucket.max_tokens, 10);
            assert_eq!(bucket.tokens_per_fill, Some(10));
        } else {
            panic!("expected local rate limit override in response");
        }

        let repo = state.xds_state.route_repository.as_ref().cloned().expect("route repo");
        let stored = repo.get_by_name("primary-routes").await.expect("stored route");
        let stored_config: XdsRouteConfig = serde_json::from_str(&stored.configuration).unwrap();
        assert!(stored_config.virtual_hosts[0].routes[0]
            .typed_per_filter_config
            .contains_key("envoy.filters.http.local_ratelimit"));
        assert_eq!(stored.version, 2);
    }

    #[tokio::test]
    async fn delete_route_removes_row() {
        let state = setup_state().await;
        let payload = sample_route_definition();
        let (status, _) =
            create_route_handler(State(state.clone()), Extension(admin_context()), Json(payload))
                .await
                .expect("create route");
        assert_eq!(status, StatusCode::CREATED);

        let status = delete_route_handler(
            State(state.clone()),
            Extension(admin_context()),
            Path("primary-routes".into()),
        )
        .await
        .expect("delete route");

        assert_eq!(status, StatusCode::NO_CONTENT);

        let repo = state.xds_state.route_repository.as_ref().cloned().expect("route repo");
        assert!(repo.get_by_name("primary-routes").await.is_err());
    }

    #[tokio::test]
    async fn template_route_supports_rewrite() {
        let state = setup_state().await;

        let mut payload = sample_route_definition();
        payload.name = "template-route".into();
        payload.virtual_hosts[0].routes[0].r#match.path =
            PathMatchDefinition::Template { template: "/api/v1/users/{user_id}".into() };
        payload.virtual_hosts[0].routes[0].action = RouteActionDefinition::Forward {
            cluster: "api-cluster".into(),
            timeout_seconds: Some(5),
            prefix_rewrite: None,
            template_rewrite: Some("/users/{user_id}".into()),
        };

        let (status, Json(created)) = create_route_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create template route");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created.name, "template-route");
        let route = &created.config.virtual_hosts[0].routes[0];
        assert!(matches!(route.r#match.path, PathMatchDefinition::Template { .. }));
        if let RouteActionDefinition::Forward { template_rewrite, .. } = &route.action {
            assert_eq!(template_rewrite.as_deref(), Some("/users/{user_id}"));
        } else {
            panic!("expected forward action");
        }

        let repo = state.xds_state.route_repository.as_ref().cloned().expect("route repo");
        let stored = repo.get_by_name("template-route").await.expect("stored template route");
        assert_eq!(stored.path_prefix, "template:/api/v1/users/{user_id}".to_string());
    }
}
