//! Route configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy route configurations through
//! the REST API, with validation and XDS state synchronization.

mod types;
mod validation;

// Re-export public types
pub use types::{
    HeaderMatchDefinition, ListRouteConfigsQuery, PathMatchDefinition,
    QueryParameterMatchDefinition, RouteActionDefinition, RouteConfigDefinition,
    RouteConfigResponse, RouteMatchDefinition, RouteRuleDefinition, VirtualHostDefinition,
    WeightedClusterDefinition,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::{error, info, instrument};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
    auth::models::AuthContext,
    domain::RouteConfigId,
    errors::Error,
    openapi::defaults::is_default_gateway_route,
    storage::{
        CreateRouteConfigRepositoryRequest, RouteConfigData, UpdateRouteConfigRepositoryRequest,
    },
};

use validation::{
    require_route_config_repository, route_config_response_from_data, summarize_route_config,
    validate_route_config, validate_route_config_payload,
};

// === Helper Functions ===

/// Verify that a route config belongs to one of the user's teams or is global.
/// Returns the route config if authorized, otherwise returns NotFound error (to avoid leaking existence).
async fn verify_route_config_access(
    route_config: RouteConfigData,
    team_scopes: &[String],
) -> Result<RouteConfigData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(route_config);
    }

    // Check if route config is global (team = NULL) or belongs to one of user's teams
    match &route_config.team {
        None => Ok(route_config), // Global route config, accessible to all
        Some(route_team) => {
            if team_scopes.contains(route_team) {
                Ok(route_config)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team, route_team, "routes",
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!(
                    "Route with name '{}' not found",
                    route_config.name
                )))
            }
        }
    }
}

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/route-configs",
    request_body = RouteConfigDefinition,
    responses(
        (status = 201, description = "Route config created", body = RouteConfigResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Route config repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state, payload), fields(team = %payload.team, route_config_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_route_config_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<RouteConfigDefinition>,
) -> Result<(StatusCode, Json<RouteConfigResponse>), ApiError> {
    validate_route_config_payload(&payload)?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "routes", "write", Some(&payload.team))?;

    let route_config_repository = require_route_config_repository(&state)?;

    let xds_config = payload.to_xds_config().and_then(validate_route_config)?;

    let (path_prefix, cluster_summary) = summarize_route_config(&payload);
    let configuration = serde_json::to_value(&xds_config).map_err(|err| {
        ApiError::from(Error::internal(format!("Failed to serialize route definition: {}", err)))
    })?;

    // Use explicit team from request
    let team = Some(payload.team.clone());

    let request = CreateRouteConfigRepositoryRequest {
        name: payload.name.clone(),
        path_prefix,
        cluster_name: cluster_summary,
        configuration,
        team,
        import_id: None,
        route_order: None,
        headers: None,
    };

    let created = route_config_repository.create(request).await.map_err(ApiError::from)?;

    info!(route_config_id = %created.id, route_config_name = %created.name, "Route config created via API");

    // Sync route hierarchy (extract virtual hosts and routes to database tables)
    if let Some(ref sync_service) = state.xds_state.route_hierarchy_sync_service {
        let route_config_id = RouteConfigId::from_string(created.id.to_string());
        if let Err(err) = sync_service.sync(&route_config_id, &xds_config).await {
            error!(error = %err, route_config_id = %created.id, "Failed to sync route hierarchy after creation");
            // Continue anyway - the route config was created, hierarchy sync is optional
        }
    }

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after route config creation");
        ApiError::from(err)
    })?;

    let response = RouteConfigResponse {
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
    path = "/api/v1/route-configs",
    params(
        ("limit" = Option<i32>, Query, description = "Maximum number of route configs to return"),
        ("offset" = Option<i32>, Query, description = "Offset for paginated results"),
    ),
    responses(
        (status = 200, description = "List of route configs", body = [RouteConfigResponse]),
        (status = 503, description = "Route config repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state, params), fields(user_id = ?context.user_id, limit = ?params.limit, offset = ?params.offset))]
pub async fn list_route_configs_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<types::ListRouteConfigsQuery>,
) -> Result<Json<Vec<RouteConfigResponse>>, ApiError> {
    // Authorization: require routes:read scope
    require_resource_access(&context, "routes", "read", None)?;

    // Extract team scopes from auth context for filtering
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_config_repository(&state)?;
    let rows = repository
        .list_by_teams(&team_scopes, true, params.limit, params.offset) // REST API: include default resources
        .await
        .map_err(ApiError::from)?;

    let mut routes = Vec::with_capacity(rows.len());
    for row in rows {
        routes.push(route_config_response_from_data(row)?);
    }

    Ok(Json(routes))
}

#[utoipa::path(
    get,
    path = "/api/v1/route-configs/{name}",
    params(("name" = String, Path, description = "Name of the route configuration")),
    responses(
        (status = 200, description = "Route config details", body = RouteConfigResponse),
        (status = 404, description = "Route config not found"),
        (status = 503, description = "Route config repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state), fields(route_config_name = %name, user_id = ?context.user_id))]
pub async fn get_route_config_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<RouteConfigResponse>, ApiError> {
    // Authorization: require routes:read scope
    require_resource_access(&context, "routes", "read", None)?;

    // Extract team scopes for access verification
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_config_repository(&state)?;
    let route_config = repository.get_by_name(&name).await.map_err(ApiError::from)?;

    // Verify the route config belongs to one of the user's teams or is global
    let route_config = verify_route_config_access(route_config, &team_scopes).await?;

    Ok(Json(route_config_response_from_data(route_config)?))
}

#[utoipa::path(
    put,
    path = "/api/v1/route-configs/{name}",
    params(("name" = String, Path, description = "Name of the route configuration")),
    request_body = RouteConfigDefinition,
    responses(
        (status = 200, description = "Route config updated", body = RouteConfigResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route config not found"),
        (status = 503, description = "Route config repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state, payload), fields(route_config_name = %name, user_id = ?context.user_id))]
pub async fn update_route_config_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
    Json(payload): Json<RouteConfigDefinition>,
) -> Result<Json<RouteConfigResponse>, ApiError> {
    // Authorization: require routes:write scope
    require_resource_access(&context, "routes", "write", None)?;

    validate_route_config_payload(&payload)?;

    if payload.name != name {
        return Err(ApiError::BadRequest(format!(
            "Payload route name '{}' does not match path '{}'",
            payload.name, name
        )));
    }

    // Extract team scopes and verify access before updating
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_route_config_repository(&state)?;
    let existing = repository.get_by_name(&payload.name).await.map_err(ApiError::from)?;

    // Verify the route config belongs to one of the user's teams or is global
    verify_route_config_access(existing.clone(), &team_scopes).await?;

    let xds_config = payload.to_xds_config().and_then(validate_route_config)?;
    let (path_prefix, cluster_summary) = summarize_route_config(&payload);
    let configuration = serde_json::to_value(&xds_config).map_err(|err| {
        ApiError::from(Error::internal(format!("Failed to serialize route definition: {}", err)))
    })?;

    let update_request = UpdateRouteConfigRepositoryRequest {
        path_prefix: Some(path_prefix.clone()),
        cluster_name: Some(cluster_summary.clone()),
        configuration: Some(configuration),
        team: None, // Don't modify team on update unless explicitly set
    };

    let updated = repository.update(&existing.id, update_request).await.map_err(ApiError::from)?;

    info!(route_config_id = %updated.id, route_config_name = %updated.name, "Route config updated via API");

    // Sync route hierarchy (re-sync virtual hosts and routes after update)
    if let Some(ref sync_service) = state.xds_state.route_hierarchy_sync_service {
        let route_config_id = RouteConfigId::from_string(updated.id.to_string());
        if let Err(err) = sync_service.sync(&route_config_id, &xds_config).await {
            error!(error = %err, route_config_id = %updated.id, "Failed to sync route hierarchy after update");
            // Continue anyway - the route config was updated, hierarchy sync is optional
        }
    }

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after route config update");
        ApiError::from(err)
    })?;

    let response = RouteConfigResponse {
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
    path = "/api/v1/route-configs/{name}",
    params(("name" = String, Path, description = "Name of the route configuration")),
    responses(
        (status = 204, description = "Route config deleted"),
        (status = 404, description = "Route config not found"),
        (status = 503, description = "Route config repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state), fields(route_config_name = %name, user_id = ?context.user_id))]
pub async fn delete_route_config_handler(
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

    let repository = require_route_config_repository(&state)?;
    let existing = repository.get_by_name(&name).await.map_err(ApiError::from)?;

    // Verify the route config belongs to one of the user's teams or is global
    verify_route_config_access(existing.clone(), &team_scopes).await?;

    repository.delete(&existing.id).await.map_err(ApiError::from)?;

    info!(route_config_id = %existing.id, route_config_name = %existing.name, "Route config deleted via API");

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after route config deletion");
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
        PathMatchDefinition, RouteActionDefinition, RouteConfigDefinition, RouteMatchDefinition,
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

            CREATE TABLE IF NOT EXISTS route_configs (
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
        let stats_cache = Arc::new(crate::services::stats_cache::StatsCache::with_defaults());
        let api_state =
            ApiState { xds_state: Arc::new(state), filter_schema_registry: None, stats_cache };

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

    fn sample_route_config_definition() -> RouteConfigDefinition {
        RouteConfigDefinition {
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
                        retry_policy: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        }
    }

    #[tokio::test]
    async fn create_route_config_persists_configuration() {
        let state = setup_state().await;

        let payload = sample_route_config_definition();
        let (status, Json(created)) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create route config");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created.name, "primary-routes");
        assert_eq!(created.config.virtual_hosts.len(), 1);

        let repo =
            state.xds_state.route_config_repository.as_ref().cloned().expect("route config repo");
        let stored = repo.get_by_name("primary-routes").await.expect("stored route config");
        assert_eq!(stored.path_prefix, "/api");
        assert!(stored.cluster_name.contains("api-cluster"));
    }

    #[tokio::test]
    async fn list_route_configs_returns_entries() {
        let state = setup_state().await;

        let payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload),
        )
        .await
        .expect("create route config");
        assert_eq!(status, StatusCode::CREATED);

        let response = list_route_configs_handler(
            State(state),
            Extension(admin_context()),
            Query(types::ListRouteConfigsQuery::default()),
        )
        .await
        .expect("list route configs");

        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].name, "primary-routes");
    }

    #[tokio::test]
    async fn get_route_config_returns_definition() {
        let state = setup_state().await;
        let payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload),
        )
        .await
        .expect("create route config");
        assert_eq!(status, StatusCode::CREATED);

        let response = get_route_config_handler(
            State(state),
            Extension(admin_context()),
            Path("primary-routes".into()),
        )
        .await
        .expect("get route config");

        assert_eq!(response.0.name, "primary-routes");
        assert_eq!(response.0.config.virtual_hosts[0].routes.len(), 1);
    }

    #[tokio::test]
    async fn update_route_config_applies_changes() {
        let state = setup_state().await;
        let mut payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create route config");
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

        let response = update_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Path("primary-routes".into()),
            Json(payload.clone()),
        )
        .await
        .expect("update route config");

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

        let repo =
            state.xds_state.route_config_repository.as_ref().cloned().expect("route config repo");
        let stored = repo.get_by_name("primary-routes").await.expect("stored route config");
        let stored_config: XdsRouteConfig = serde_json::from_str(&stored.configuration).unwrap();
        assert!(stored_config.virtual_hosts[0].routes[0]
            .typed_per_filter_config
            .contains_key("envoy.filters.http.local_ratelimit"));
        assert_eq!(stored.version, 2);
    }

    #[tokio::test]
    async fn delete_route_config_removes_row() {
        let state = setup_state().await;
        let payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload),
        )
        .await
        .expect("create route config");
        assert_eq!(status, StatusCode::CREATED);

        let status = delete_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Path("primary-routes".into()),
        )
        .await
        .expect("delete route config");

        assert_eq!(status, StatusCode::NO_CONTENT);

        let repo =
            state.xds_state.route_config_repository.as_ref().cloned().expect("route config repo");
        assert!(repo.get_by_name("primary-routes").await.is_err());
    }

    #[tokio::test]
    async fn template_route_config_supports_rewrite() {
        let state = setup_state().await;

        let mut payload = sample_route_config_definition();
        payload.name = "template-route".into();
        payload.virtual_hosts[0].routes[0].r#match.path =
            PathMatchDefinition::Template { template: "/api/v1/users/{user_id}".into() };
        payload.virtual_hosts[0].routes[0].action = RouteActionDefinition::Forward {
            cluster: "api-cluster".into(),
            timeout_seconds: Some(5),
            prefix_rewrite: None,
            template_rewrite: Some("/users/{user_id}".into()),
            retry_policy: None,
        };

        let (status, Json(created)) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create template route config");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created.name, "template-route");
        let route = &created.config.virtual_hosts[0].routes[0];
        assert!(matches!(route.r#match.path, PathMatchDefinition::Template { .. }));
        if let RouteActionDefinition::Forward { template_rewrite, .. } = &route.action {
            assert_eq!(template_rewrite.as_deref(), Some("/users/{user_id}"));
        } else {
            panic!("expected forward action");
        }

        let repo =
            state.xds_state.route_config_repository.as_ref().cloned().expect("route config repo");
        let stored = repo.get_by_name("template-route").await.expect("stored template route");
        assert_eq!(stored.path_prefix, "template:/api/v1/users/{user_id}".to_string());
    }
}
