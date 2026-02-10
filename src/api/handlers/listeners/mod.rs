//! Listener configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy listener configurations through
//! the REST API. All operations are delegated to the internal API layer (ListenerOperations)
//! which provides unified validation, access control, and XDS state synchronization.

mod types;
mod validation;

// Re-export public types for backward compatibility
pub use types::{
    CreateListenerBody, ListenerAccessLogInput, ListenerFilterChainInput, ListenerFilterInput,
    ListenerFilterTypeInput, ListenerResponse, ListenerTlsContextInput, ListenerTracingInput,
    UpdateListenerBody,
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
        error::ApiError,
        handlers::team_access::{require_resource_access_resolved, team_repo_from_state},
        routes::ApiState,
    },
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
    internal_api::auth::InternalAuthContext,
    internal_api::listeners::ListenerOperations,
    internal_api::types::{
        CreateListenerRequest as InternalCreateListenerRequest,
        ListListenersRequest as InternalListListenersRequest,
        UpdateListenerRequest as InternalUpdateListenerRequest,
    },
};

use validation::{
    listener_config_from_create, listener_config_from_update, listener_response_from_data,
    validate_create_listener_body, validate_update_listener_body,
};

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/listeners",
    request_body = CreateListenerBody,
    responses(
        (status = 201, description = "Listener created", body = ListenerResponse),
        (status = 400, description = "Invalid listener payload"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "Listeners"
)]
#[instrument(skip(state, payload), fields(team = %payload.team, listener_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<types::CreateListenerBody>,
) -> Result<(StatusCode, Json<types::ListenerResponse>), ApiError> {
    // REST-specific validation
    validate_create_listener_body(&payload)?;

    // Verify user has write access to the specified team
    require_resource_access_resolved(
        &state,
        &context,
        "listeners",
        "write",
        Some(&payload.team),
        context.org_id.as_ref(),
    )
    .await?;

    // Build ListenerConfig from REST body
    let config = listener_config_from_create(&payload)?;

    // Create internal API request
    let internal_request = InternalCreateListenerRequest {
        name: payload.name.clone(),
        address: payload.address.clone(),
        port: payload.port,
        protocol: payload.protocol.clone(),
        team: Some(payload.team.clone()),
        config,
        dataplane_id: payload.dataplane_id.clone(),
    };

    // Delegate to internal API layer
    let ops = ListenerOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let result = ops.create(internal_request, &auth).await?;

    let response = listener_response_from_data(result.data)?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/listeners",
    params(PaginationQuery),
    responses(
        (status = 200, description = "List of listeners", body = PaginatedResponse<ListenerResponse>),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "Listeners"
)]
pub async fn list_listeners_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<types::ListenerResponse>>, ApiError> {
    // Authorization: require listeners:read scope
    require_resource_access(&context, "listeners", "read", None)?;

    let (limit, offset) = params.clamp(1000);

    // Create internal API request (REST API: include default resources)
    let internal_request = InternalListListenersRequest {
        limit: Some(limit as i32),
        offset: Some(offset as i32),
        include_defaults: true,
    };

    // Delegate to internal API layer
    let ops = ListenerOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let result = ops.list(internal_request, &auth).await?;
    let total = result.count as i64;

    let mut listeners = Vec::with_capacity(result.listeners.len());
    for row in result.listeners {
        listeners.push(listener_response_from_data(row)?);
    }

    Ok(Json(PaginatedResponse::new(listeners, total, limit, offset)))
}

#[utoipa::path(
    get,
    path = "/api/v1/listeners/{name}",
    params(("name" = String, Path, description = "Name of the listener")),
    responses(
        (status = 200, description = "Listener details", body = ListenerResponse),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "Listeners"
)]
#[instrument(skip(state), fields(listener_name = %name, user_id = ?context.user_id))]
pub async fn get_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<types::ListenerResponse>, ApiError> {
    // Authorization: require listeners:read scope
    require_resource_access(&context, "listeners", "read", None)?;

    // Delegate to internal API layer (includes team access verification)
    let ops = ListenerOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let listener = ops.get(&name, &auth).await?;

    let response = listener_response_from_data(listener)?;
    Ok(Json(response))
}

#[utoipa::path(
    put,
    path = "/api/v1/listeners/{name}",
    request_body = UpdateListenerBody,
    params(("name" = String, Path, description = "Name of the listener")),
    responses(
        (status = 200, description = "Listener updated", body = ListenerResponse),
        (status = 400, description = "Invalid listener payload"),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "Listeners"
)]
#[instrument(skip(state, payload), fields(listener_name = %name, user_id = ?context.user_id))]
pub async fn update_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
    Json(payload): Json<types::UpdateListenerBody>,
) -> Result<Json<types::ListenerResponse>, ApiError> {
    // Authorization: require listeners:write scope
    require_resource_access(&context, "listeners", "write", None)?;

    // REST-specific validation
    validate_update_listener_body(&payload)?;

    // Build ListenerConfig from REST body
    let config = listener_config_from_update(name.clone(), &payload)?;

    // Create internal API request
    let internal_request = InternalUpdateListenerRequest {
        address: Some(payload.address.clone()),
        port: Some(payload.port),
        protocol: payload.protocol.clone(),
        config,
        dataplane_id: payload.dataplane_id.clone(),
    };

    // Delegate to internal API layer (includes team access verification and XDS refresh)
    let ops = ListenerOperations::new(state.xds_state.clone());
    let team_repo = team_repo_from_state(&state)?;
    let auth = InternalAuthContext::from_rest_with_org(&context, team_repo)
        .await
        .resolve_teams(team_repo)
        .await?;
    let result = ops.update(&name, internal_request, &auth).await?;

    let response = listener_response_from_data(result.data)?;
    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/listeners/{name}",
    params(("name" = String, Path, description = "Name of the listener")),
    responses(
        (status = 204, description = "Listener deleted"),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "Listeners"
)]
#[instrument(skip(state), fields(listener_name = %name, user_id = ?context.user_id))]
pub async fn delete_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require listeners:write scope (delete is a write operation)
    require_resource_access(&context, "listeners", "write", None)?;

    // Delegate to internal API layer (includes default listener protection, team access, and XDS refresh)
    let ops = ListenerOperations::new(state.xds_state.clone());
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
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::{
        api::test_utils::{
            admin_auth_context, minimal_auth_context, readonly_resource_auth_context,
            resource_auth_context,
        },
        config::SimpleXdsConfig,
        storage::test_helpers::{TestDatabase, TEST_TEAM_ID},
        xds::resources::LISTENER_TYPE_URL,
        xds::route::{
            PathMatch, RouteActionConfig, RouteConfig as InlineRouteConfig, RouteMatchConfig,
            RouteRule, VirtualHostConfig,
        },
        xds::XdsState,
    };
    use axum::{response::IntoResponse, Extension};
    use tokio::time::{sleep, Duration};

    use types::{
        CreateListenerBody, ListenerFilterChainInput, ListenerFilterInput, ListenerFilterTypeInput,
        UpdateListenerBody,
    };
    use validation::convert_filter_type;

    // Use test_utils::admin_auth_context() for admin permissions

    async fn build_state() -> (TestDatabase, Arc<XdsState>, ApiState) {
        let test_db = TestDatabase::new("listener_handler").await;
        let pool = test_db.pool.clone();

        // Insert test dataplanes (TestDatabase runs all migrations automatically)
        // After FK migration, dataplanes.team stores team UUIDs (FK to teams.id)
        sqlx::query(
            r#"
            INSERT INTO dataplanes (id, team, name, gateway_host, description)
            VALUES ('dp-test-123', $1, 'test-dataplane', '10.0.0.1', 'Test dataplane')
        "#,
        )
        .bind(TEST_TEAM_ID)
        .execute(&pool)
        .await
        .unwrap();

        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        let stats_cache = Arc::new(crate::services::stats_cache::StatsCache::with_defaults());
        let mcp_connection_manager = crate::mcp::create_connection_manager();
        let mcp_session_manager = crate::mcp::create_session_manager();
        let certificate_rate_limiter = Arc::new(crate::api::rate_limit::RateLimiter::from_env());
        let api_state = ApiState {
            xds_state: state.clone(),
            filter_schema_registry: None,
            stats_cache,
            mcp_connection_manager,
            mcp_session_manager,
            certificate_rate_limiter,
            auth_config: Arc::new(crate::config::AuthConfig::default()),
            auth_rate_limiters: Arc::new(crate::api::routes::AuthRateLimiters::from_env()),
        };
        (test_db, state, api_state)
    }

    #[test]
    fn convert_http_filter_requires_route_source() {
        let result = convert_filter_type(&ListenerFilterTypeInput::HttpConnectionManager {
            route_config_name: None,
            inline_route_config: None,
            access_log: None,
            tracing: None,
            http_filters: Vec::new(),
        });

        assert!(result.is_err());
    }

    #[test]
    fn convert_http_filter_with_inline_route() {
        let route_config = InlineRouteConfig {
            name: "inline-route".to_string(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "vh".to_string(),
                domains: vec!["*".to_string()],
                routes: vec![RouteRule {
                    name: Some("all".to_string()),
                    r#match: RouteMatchConfig {
                        path: PathMatch::Prefix("/".to_string()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "backend".to_string(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                        retry_policy: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        };
        let inline_route = serde_json::to_value(&route_config).unwrap();

        let result = convert_filter_type(&ListenerFilterTypeInput::HttpConnectionManager {
            route_config_name: None,
            inline_route_config: Some(inline_route),
            access_log: None,
            tracing: None,
            http_filters: Vec::new(),
        });

        assert!(result.is_ok());
        use crate::xds::listener::FilterType;
        match result.unwrap() {
            FilterType::HttpConnectionManager { inline_route_config: Some(config), .. } => {
                assert_eq!(config.name, "inline-route");
            }
            other => panic!("expected HTTP connection manager, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn create_listener_handler_persists_and_refreshes_state() {
        let (_db, state, api_state) = build_state().await;

        let payload = CreateListenerBody {
            team: "test-team".to_string(),
            name: "edge-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 10000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("primary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
            dataplane_id: "dp-test-123".to_string(),
        };

        let (status, Json(resp)) = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create listener");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(resp.name, "edge-listener");
        assert_eq!(resp.port, Some(10000));

        // Allow async cache refresh to complete.
        sleep(Duration::from_millis(50)).await;

        let cached = state.cached_resources(LISTENER_TYPE_URL);
        assert_eq!(cached.len(), 1, "listener cache should contain one entry");
    }

    #[tokio::test]
    async fn update_listener_handler_updates_repository() {
        let (_db, state, api_state) = build_state().await;

        // Seed a listener so we can update it.
        let initial = CreateListenerBody {
            team: "test-team".to_string(),
            name: "edge-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 10000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("primary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
            dataplane_id: "dp-test-123".to_string(),
        };

        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(initial),
        )
        .await
        .expect("seed listener");

        let update_payload = UpdateListenerBody {
            address: "127.0.0.1".to_string(),
            port: 11000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("secondary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
            dataplane_id: None, // Don't change dataplane on update
        };

        let Json(updated) = update_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Path("edge-listener".to_string()),
            Json(update_payload),
        )
        .await
        .expect("update listener");

        assert_eq!(updated.address, "127.0.0.1");
        assert_eq!(updated.port, Some(11000));
        assert_eq!(updated.version, 2);

        // Ensure cache reflects latest version.
        sleep(Duration::from_millis(50)).await;
        let cached = state.cached_resources(LISTENER_TYPE_URL);
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].name, "edge-listener");
        assert_eq!(cached[0].version, state.get_version_number());
    }

    // === Sample Data Helpers ===

    fn sample_create_listener() -> CreateListenerBody {
        CreateListenerBody {
            team: "test-team".to_string(),
            name: "edge-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 10000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("primary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
            dataplane_id: "dp-test-123".to_string(),
        }
    }

    fn sample_update_listener() -> UpdateListenerBody {
        UpdateListenerBody {
            address: "127.0.0.1".to_string(),
            port: 11000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("secondary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
            dataplane_id: None,
        }
    }

    // === CRUD Tests ===

    #[tokio::test]
    async fn list_listeners_returns_entries() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create listener");

        let result = list_listeners_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await;

        assert!(result.is_ok());
        let resp = result.unwrap().0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].name, "edge-listener");
    }

    #[tokio::test]
    async fn get_listener_returns_details() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create listener");

        let result = get_listener_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Path("edge-listener".to_string()),
        )
        .await;

        assert!(result.is_ok());
        let listener = result.unwrap().0;
        assert_eq!(listener.name, "edge-listener");
        assert_eq!(listener.port, Some(10000));
    }

    #[tokio::test]
    async fn delete_listener_removes_record() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create listener");

        let status = delete_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Path("edge-listener".to_string()),
        )
        .await
        .expect("delete listener");

        assert_eq!(status, StatusCode::NO_CONTENT);

        // Verify it's gone
        let result = get_listener_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Path("edge-listener".to_string()),
        )
        .await;
        assert!(result.is_err());
    }

    // === Authorization Tests ===

    #[tokio::test]
    async fn create_listener_with_listeners_write_scope() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        let result = create_listener_handler(
            State(api_state),
            Extension(resource_auth_context("listeners")),
            Json(payload),
        )
        .await;

        assert!(result.is_ok());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_listener_fails_without_write_scope() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        let result = create_listener_handler(
            State(api_state),
            Extension(readonly_resource_auth_context("listeners")),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_listener_fails_with_no_permissions() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        let result = create_listener_handler(
            State(api_state),
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
    async fn list_listeners_requires_read_scope() {
        let (_db, _state, api_state) = build_state().await;

        let result = list_listeners_handler(
            State(api_state),
            Extension(minimal_auth_context()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_listener_requires_read_scope() {
        let (_db, _state, api_state) = build_state().await;

        let result = get_listener_handler(
            State(api_state),
            Extension(minimal_auth_context()),
            Path("any-listener".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_listener_requires_write_scope() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        // Create first with admin
        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create listener");

        // Try to update with readonly scope
        let update = sample_update_listener();
        let result = update_listener_handler(
            State(api_state),
            Extension(readonly_resource_auth_context("listeners")),
            Path("edge-listener".to_string()),
            Json(update),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_listener_requires_write_scope() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        // Create first with admin
        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await
        .expect("create listener");

        // Try to delete with readonly scope
        let result = delete_listener_handler(
            State(api_state),
            Extension(readonly_resource_auth_context("listeners")),
            Path("edge-listener".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // === Error Handling Tests ===

    #[tokio::test]
    async fn get_listener_not_found() {
        let (_db, _state, api_state) = build_state().await;

        let result = get_listener_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Path("non-existent-listener".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_listener_not_found() {
        let (_db, _state, api_state) = build_state().await;
        let update = sample_update_listener();

        let result = update_listener_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Path("non-existent-listener".to_string()),
            Json(update),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_listener_not_found() {
        let (_db, _state, api_state) = build_state().await;

        let result = delete_listener_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Path("non-existent-listener".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_listener_duplicate_name_returns_error() {
        let (_db, _state, api_state) = build_state().await;
        let payload = sample_create_listener();

        // Create first listener
        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_auth_context()),
            Json(payload.clone()),
        )
        .await
        .expect("create first listener");

        // Try to create duplicate - should fail
        let result = create_listener_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Json(payload),
        )
        .await;

        assert!(result.is_err());
        // The exact status code depends on the internal error mapping
    }

    // === Pagination Tests ===

    #[tokio::test]
    async fn list_listeners_with_pagination() {
        let (_db, _state, api_state) = build_state().await;

        // Create multiple listeners (vary port to avoid partial unique index violation)
        for i in 0..5 {
            let mut payload = sample_create_listener();
            payload.name = format!("listener-{}", i);
            payload.port = 10000 + i as u16;
            let _ = create_listener_handler(
                State(api_state.clone()),
                Extension(admin_auth_context()),
                Json(payload),
            )
            .await
            .expect("create listener");
        }

        // List with limit
        let result = list_listeners_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Query(PaginationQuery { limit: 2, offset: 0 }),
        )
        .await;

        assert!(result.is_ok());
        let listeners = &result.unwrap().0.items;
        assert_eq!(listeners.len(), 2);
    }

    #[tokio::test]
    async fn list_listeners_with_offset() {
        let (_db, _state, api_state) = build_state().await;

        // Create multiple listeners (vary port to avoid partial unique index violation)
        for i in 0..5 {
            let mut payload = sample_create_listener();
            payload.name = format!("listener-{}", i);
            payload.port = 10000 + i as u16;
            let _ = create_listener_handler(
                State(api_state.clone()),
                Extension(admin_auth_context()),
                Json(payload),
            )
            .await
            .expect("create listener");
        }

        // List with offset
        let result = list_listeners_handler(
            State(api_state),
            Extension(admin_auth_context()),
            Query(PaginationQuery { limit: 10, offset: 2 }),
        )
        .await;

        assert!(result.is_ok());
        let listeners = &result.unwrap().0.items;
        assert_eq!(listeners.len(), 3); // 5 total - 2 offset = 3 remaining
    }
}
