//! Route configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy route configurations through
//! the REST API. All operations are delegated to the internal API layer (RouteConfigOperations)
//! which provides unified validation, access control, and XDS state synchronization.

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
use tracing::instrument;

use crate::{
    api::{
        error::ApiError,
        handlers::team_access::{require_resource_access_resolved, team_repo_from_state},
        routes::ApiState,
    },
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
    internal_api::auth::InternalAuthContext,
    internal_api::routes::RouteConfigOperations,
    internal_api::types::{
        CreateRouteConfigRequest as InternalCreateRouteConfigRequest,
        ListRouteConfigsRequest as InternalListRouteConfigsRequest,
        UpdateRouteConfigRequest as InternalUpdateRouteConfigRequest,
    },
};

use validation::{
    route_config_response_from_data, validate_route_config, validate_route_config_payload,
};

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
    // REST-specific validation
    validate_route_config_payload(&payload)?;

    // Verify user has write access to the specified team
    require_resource_access_resolved(
        &state,
        &context,
        "routes",
        "write",
        Some(&payload.team),
        context.org_id.as_ref(),
    )
    .await?;

    // Validate and convert to XDS config
    let xds_config = payload.to_xds_config().and_then(validate_route_config)?;
    let config_value = serde_json::to_value(&xds_config).map_err(|err| {
        ApiError::BadRequest(format!("Failed to serialize route config: {}", err))
    })?;

    // Create internal API request
    let internal_request = InternalCreateRouteConfigRequest {
        name: payload.name.clone(),
        team: Some(payload.team.clone()),
        config: config_value,
    };

    // Delegate to internal API layer (includes XDS refresh and route hierarchy sync)
    let ops = RouteConfigOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let result = ops.create(internal_request, &auth).await?;

    let response = RouteConfigResponse {
        name: result.data.name,
        team: result.data.team.unwrap_or_default(),
        path_prefix: result.data.path_prefix,
        cluster_targets: result.data.cluster_name,
        import_id: result.data.import_id,
        route_order: result.data.route_order,
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

    // Create internal API request (REST API: include default resources)
    let internal_request = InternalListRouteConfigsRequest {
        limit: params.limit,
        offset: params.offset,
        include_defaults: true,
    };

    // Delegate to internal API layer
    let ops = RouteConfigOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let result = ops.list(internal_request, &auth).await?;

    let mut routes = Vec::with_capacity(result.routes.len());
    for row in result.routes {
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

    // Delegate to internal API layer (includes team access verification)
    let ops = RouteConfigOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let route_config = ops.get(&name, &auth).await?;

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

    // REST-specific validation
    validate_route_config_payload(&payload)?;

    if payload.name != name {
        return Err(ApiError::BadRequest(format!(
            "Payload route name '{}' does not match path '{}'",
            payload.name, name
        )));
    }

    // Validate and convert to XDS config
    let xds_config = payload.to_xds_config().and_then(validate_route_config)?;
    let config_value = serde_json::to_value(&xds_config).map_err(|err| {
        ApiError::BadRequest(format!("Failed to serialize route config: {}", err))
    })?;

    // Create internal API request
    let internal_request = InternalUpdateRouteConfigRequest { config: config_value };

    // Delegate to internal API layer (includes team access, XDS refresh, and route hierarchy sync)
    let ops = RouteConfigOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let result = ops.update(&name, internal_request, &auth).await?;

    let response = RouteConfigResponse {
        name: result.data.name,
        team: result.data.team.unwrap_or_default(),
        path_prefix: result.data.path_prefix,
        cluster_targets: result.data.cluster_name,
        import_id: result.data.import_id,
        route_order: result.data.route_order,
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

    // Delegate to internal API layer (includes default route protection, team access, and XDS refresh)
    let ops = RouteConfigOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    ops.delete(&name, &auth).await?;

    Ok(StatusCode::NO_CONTENT)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, response::IntoResponse, Extension, Json};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::api::test_utils::{
        admin_auth_context, minimal_auth_context, readonly_resource_auth_context,
        resource_auth_context,
    };
    use crate::config::SimpleXdsConfig;
    use crate::storage::{test_helpers::TestDatabase, CreateClusterRequest};
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

    // Use test_utils::admin_auth_context() for admin permissions

    async fn setup_state() -> (TestDatabase, ApiState) {
        let test_db = TestDatabase::new("route_configs_handler").await;
        let pool = test_db.pool.clone();

        let state = XdsState::with_database(SimpleXdsConfig::default(), pool.clone());
        let stats_cache = Arc::new(crate::services::stats_cache::StatsCache::with_defaults());
        let mcp_connection_manager = crate::mcp::create_connection_manager();
        let mcp_session_manager = crate::mcp::create_session_manager();
        let certificate_rate_limiter = Arc::new(crate::api::rate_limit::RateLimiter::from_env());
        let api_state = ApiState {
            xds_state: Arc::new(state),
            filter_schema_registry: None,
            stats_cache,
            mcp_connection_manager,
            mcp_session_manager,
            certificate_rate_limiter,
            auth_config: Arc::new(crate::config::AuthConfig::default()),
            auth_rate_limiters: Arc::new(crate::api::routes::AuthRateLimiters::from_env()),
        };

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

        (test_db, api_state)
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
        let (_db, state) = setup_state().await;

        let payload = sample_route_config_definition();
        let (status, Json(created)) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
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
        let (_db, state) = setup_state().await;

        let payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create route config");
        assert_eq!(status, StatusCode::CREATED);

        let response = list_route_configs_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(types::ListRouteConfigsQuery::default()),
        )
        .await
        .expect("list route configs");

        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].name, "primary-routes");
    }

    #[tokio::test]
    async fn get_route_config_returns_definition() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create route config");
        assert_eq!(status, StatusCode::CREATED);

        let response = get_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("primary-routes".into()),
        )
        .await
        .expect("get route config");

        assert_eq!(response.0.name, "primary-routes");
        assert_eq!(response.0.config.virtual_hosts[0].routes.len(), 1);
    }

    #[tokio::test]
    async fn update_route_config_applies_changes() {
        let (_db, state) = setup_state().await;
        let mut payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
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
            Extension(admin_auth_context()),
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
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();
        let (status, _) = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create route config");
        assert_eq!(status, StatusCode::CREATED);

        let status = delete_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
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
        let (_db, state) = setup_state().await;

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
            Extension(admin_auth_context()),
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

    // === Authorization Tests ===

    #[tokio::test]
    async fn create_route_config_with_routes_write_scope() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();

        let result = create_route_config_handler(
            State(state),
            Extension(resource_auth_context("routes")),
            Json(payload),
        )
        .await;

        assert!(result.is_ok());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_route_config_fails_without_write_scope() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();

        let result = create_route_config_handler(
            State(state),
            Extension(readonly_resource_auth_context("routes")),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_route_config_fails_with_no_permissions() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();

        let result = create_route_config_handler(
            State(state),
            Extension(minimal_auth_context()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_route_configs_requires_read_scope() {
        let (_db, state) = setup_state().await;

        let result = list_route_configs_handler(
            State(state),
            Extension(minimal_auth_context()),
            Query(types::ListRouteConfigsQuery::default()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_route_config_requires_read_scope() {
        let (_db, state) = setup_state().await;

        let result = get_route_config_handler(
            State(state),
            Extension(minimal_auth_context()),
            Path("any-route".into()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_route_config_requires_write_scope() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();

        // Create first with admin
        let _ = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create route config");

        // Try to update with readonly scope
        let result = update_route_config_handler(
            State(state),
            Extension(readonly_resource_auth_context("routes")),
            Path("primary-routes".into()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_route_config_requires_write_scope() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();

        // Create first with admin
        let _ = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create route config");

        // Try to delete with readonly scope
        let result = delete_route_config_handler(
            State(state),
            Extension(readonly_resource_auth_context("routes")),
            Path("primary-routes".into()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // === Error Handling Tests ===

    #[tokio::test]
    async fn get_route_config_not_found() {
        let (_db, state) = setup_state().await;

        let result = get_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-route".into()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_route_config_not_found() {
        let (_db, state) = setup_state().await;
        let mut payload = sample_route_config_definition();
        // Set the payload name to match the path parameter
        payload.name = "non-existent-route".to_string();

        let result = update_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-route".into()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_route_config_not_found() {
        let (_db, state) = setup_state().await;

        let result = delete_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-route".into()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_route_config_duplicate_name_returns_error() {
        let (_db, state) = setup_state().await;
        let payload = sample_route_config_definition();

        // Create first route config
        let _ = create_route_config_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create first route config");

        // Try to create duplicate - should fail
        let result = create_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        // The exact status code depends on the internal error mapping
        // What matters is that the duplicate is rejected
    }

    // === Validation Tests ===

    #[tokio::test]
    async fn create_route_config_validates_empty_virtual_hosts() {
        let (_db, state) = setup_state().await;

        let mut payload = sample_route_config_definition();
        payload.virtual_hosts = vec![];

        let result = create_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_route_config_validates_empty_routes() {
        let (_db, state) = setup_state().await;

        let mut payload = sample_route_config_definition();
        payload.virtual_hosts[0].routes = vec![];

        let result = create_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_route_config_validates_empty_domains() {
        let (_db, state) = setup_state().await;

        let mut payload = sample_route_config_definition();
        payload.virtual_hosts[0].domains = vec![];

        let result = create_route_config_handler(
            State(state),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // === Edge Case Tests ===

    #[tokio::test]
    async fn list_route_configs_with_pagination() {
        let (_db, state) = setup_state().await;

        // Create multiple route configs
        for i in 0..5 {
            let mut payload = sample_route_config_definition();
            payload.name = format!("route-{}", i);
            let _ = create_route_config_handler(
                State(state.clone()),
                Extension(admin_auth_context()),
                Json(payload),
            )
            .await
            .expect("create route config");
        }

        // List with limit
        let result = list_route_configs_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(types::ListRouteConfigsQuery { limit: Some(2), offset: Some(0) }),
        )
        .await;

        assert!(result.is_ok());
        let routes = result.unwrap().0;
        assert_eq!(routes.len(), 2);
    }

    #[tokio::test]
    async fn list_route_configs_with_offset() {
        let (_db, state) = setup_state().await;

        // Create multiple route configs
        for i in 0..5 {
            let mut payload = sample_route_config_definition();
            payload.name = format!("route-{}", i);
            let _ = create_route_config_handler(
                State(state.clone()),
                Extension(admin_auth_context()),
                Json(payload),
            )
            .await
            .expect("create route config");
        }

        // List with offset
        let result = list_route_configs_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(types::ListRouteConfigsQuery { limit: Some(10), offset: Some(2) }),
        )
        .await;

        assert!(result.is_ok());
        let routes = result.unwrap().0;
        assert_eq!(routes.len(), 3); // 5 total - 2 offset = 3 remaining
    }
}
