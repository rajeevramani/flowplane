//! Listener configuration HTTP handlers
//!
//! This module provides CRUD operations for Envoy listener configurations through
//! the REST API, with validation and XDS state synchronization.

mod types;
mod validation;

// Re-export public types for backward compatibility
pub use types::{
    CreateListenerBody, ListListenersQuery, ListenerAccessLogInput, ListenerFilterChainInput,
    ListenerFilterInput, ListenerFilterTypeInput, ListenerResponse, ListenerTlsContextInput,
    ListenerTracingInput, UpdateListenerBody,
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
    openapi::defaults::is_default_gateway_listener,
    storage::{CreateListenerRequest, ListenerData, UpdateListenerRequest},
};

use validation::{
    listener_config_from_create, listener_config_from_update, listener_response_from_data,
    require_listener_repository, validate_create_listener_body, validate_update_listener_body,
};

// === Helper Functions ===

/// Verify that a listener belongs to one of the user's teams or is global.
/// Returns the listener if authorized, otherwise returns NotFound error (to avoid leaking existence).
async fn verify_listener_access(
    listener: ListenerData,
    team_scopes: &[String],
) -> Result<ListenerData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(listener);
    }

    // Check if listener is global (team = NULL) or belongs to one of user's teams
    match &listener.team {
        None => Ok(listener), // Global listener, accessible to all
        Some(listener_team) => {
            if team_scopes.contains(listener_team) {
                Ok(listener)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team,
                        listener_team,
                        "listeners",
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!("Listener with name '{}' not found", listener.name)))
            }
        }
    }
}

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
    tag = "listeners"
)]
pub async fn create_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<types::CreateListenerBody>,
) -> Result<(StatusCode, Json<types::ListenerResponse>), ApiError> {
    validate_create_listener_body(&payload)?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "listeners", "write", Some(&payload.team))?;

    let repository = require_listener_repository(&state)?;
    let config = listener_config_from_create(&payload)?;
    let configuration = serde_json::to_value(&config).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to serialize listener configuration: {}",
            err
        )))
    })?;

    // Use explicit team from request
    let team = Some(payload.team.clone());

    let request = CreateListenerRequest {
        name: payload.name.clone(),
        address: payload.address.clone(),
        port: Some(payload.port as i64),
        protocol: payload.protocol.clone(),
        configuration,
        team,
        import_id: None,
    };

    let created = repository.create(request).await.map_err(ApiError::from)?;
    info!(listener_id = %created.id, listener_name = %created.name, "Listener created via API");

    state.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after listener creation");
        ApiError::from(err)
    })?;

    let response = listener_response_from_data(created)?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/listeners",
    params(
        ("limit" = Option<i32>, Query, description = "Maximum number of listeners to return"),
        ("offset" = Option<i32>, Query, description = "Offset for paginated results"),
    ),
    responses(
        (status = 200, description = "List of listeners", body = [ListenerResponse]),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "listeners"
)]
pub async fn list_listeners_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<types::ListListenersQuery>,
) -> Result<Json<Vec<types::ListenerResponse>>, ApiError> {
    // Authorization: require listeners:read scope
    require_resource_access(&context, "listeners", "read", None)?;

    // Extract team scopes from auth context for filtering
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_listener_repository(&state)?;
    let rows = repository
        .list_by_teams(&team_scopes, true, params.limit, params.offset) // REST API: include default resources
        .await
        .map_err(ApiError::from)?;

    let mut listeners = Vec::with_capacity(rows.len());
    for row in rows {
        listeners.push(listener_response_from_data(row)?);
    }

    Ok(Json(listeners))
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
    tag = "listeners"
)]
pub async fn get_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<types::ListenerResponse>, ApiError> {
    // Authorization: require listeners:read scope
    require_resource_access(&context, "listeners", "read", None)?;

    // Extract team scopes for access verification
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_listener_repository(&state)?;
    let listener = repository.get_by_name(&name).await.map_err(ApiError::from)?;

    // Verify the listener belongs to one of the user's teams or is global
    let listener = verify_listener_access(listener, &team_scopes).await?;

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
    tag = "listeners"
)]
pub async fn update_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
    Json(payload): Json<types::UpdateListenerBody>,
) -> Result<Json<types::ListenerResponse>, ApiError> {
    // Authorization: require listeners:write scope
    require_resource_access(&context, "listeners", "write", None)?;

    validate_update_listener_body(&payload)?;

    // Extract team scopes and verify access before updating
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_listener_repository(&state)?;
    let existing = repository.get_by_name(&name).await.map_err(ApiError::from)?;
    verify_listener_access(existing.clone(), &team_scopes).await?;

    let config = listener_config_from_update(name.clone(), &payload)?;
    let configuration = serde_json::to_value(&config).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to serialize listener configuration: {}",
            err
        )))
    })?;

    let request = UpdateListenerRequest {
        address: Some(payload.address.clone()),
        port: Some(Some(payload.port as i64)),
        protocol: payload.protocol.clone(),
        configuration: Some(configuration),
        team: None, // Don't modify team on update unless explicitly set
    };

    let updated = repository.update(&existing.id, request).await.map_err(ApiError::from)?;

    info!(listener_id = %existing.id, listener_name = %name, "Listener updated via API");

    state.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after listener update");
        ApiError::from(err)
    })?;

    let response = listener_response_from_data(updated)?;
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
    tag = "listeners"
)]
pub async fn delete_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require listeners:write scope (delete is a write operation)
    require_resource_access(&context, "listeners", "write", None)?;

    if is_default_gateway_listener(&name) {
        return Err(ApiError::Conflict(
            "The default gateway listener cannot be deleted".to_string(),
        ));
    }

    // Extract team scopes and verify access before deleting
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };

    let repository = require_listener_repository(&state)?;
    let existing = repository.get_by_name(&name).await.map_err(ApiError::from)?;
    verify_listener_access(existing.clone(), &team_scopes).await?;

    repository.delete(&existing.id).await.map_err(ApiError::from)?;

    info!(listener_id = %existing.id, listener_name = %name, "Listener deleted via API");

    state.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
        error!(error = %err, "Failed to refresh xDS caches after listener deletion");
        ApiError::from(err)
    })?;

    Ok(StatusCode::NO_CONTENT)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::{
        auth::models::AuthContext,
        config::SimpleXdsConfig,
        storage::DbPool,
        xds::resources::LISTENER_TYPE_URL,
        xds::route::{
            PathMatch, RouteActionConfig, RouteConfig as InlineRouteConfig, RouteMatchConfig,
            RouteRule, VirtualHostConfig,
        },
        xds::XdsState,
    };
    use axum::Extension;
    use sqlx::sqlite::SqlitePoolOptions;
    use tokio::time::{sleep, Duration};

    use types::{
        CreateListenerBody, ListenerFilterChainInput, ListenerFilterInput, ListenerFilterTypeInput,
        UpdateListenerBody,
    };
    use validation::convert_filter_type;

    /// Create an admin AuthContext for testing with full permissions
    fn admin_context() -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("test-token"),
            "test-admin".to_string(),
            vec!["admin:all".to_string()],
        )
    }

    async fn create_test_pool() -> DbPool {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .expect("create sqlite pool");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                service_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS routes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                path_prefix TEXT NOT NULL,
                cluster_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS listeners (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                address TEXT NOT NULL,
                port INTEGER,
                protocol TEXT NOT NULL DEFAULT 'HTTP',
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                import_id TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    async fn build_state() -> (Arc<XdsState>, ApiState) {
        let pool = create_test_pool().await;
        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        let api_state = ApiState { xds_state: state.clone() };
        (state, api_state)
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
        let (state, api_state) = build_state().await;

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
        };

        let (status, Json(resp)) = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_context()),
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
        let (state, api_state) = build_state().await;

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
        };

        let _ = create_listener_handler(
            State(api_state.clone()),
            Extension(admin_context()),
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
        };

        let Json(updated) = update_listener_handler(
            State(api_state.clone()),
            Extension(admin_context()),
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
}
