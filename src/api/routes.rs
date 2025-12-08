use std::sync::Arc;

use axum::{
    http::{header, HeaderName, HeaderValue, Method},
    middleware,
    routing::{delete, get, patch, post, put},
    Router,
};
use tower_http::cors::CorsLayer;

use crate::auth::{
    auth_service::AuthService,
    middleware::{authenticate, ensure_dynamic_scopes},
    session::SessionService,
};
use crate::observability::trace_http_requests;
use crate::storage::repository::AuditLogRepository;
use crate::xds::XdsState;

use super::{
    docs,
    handlers::{
        add_team_membership, admin_create_team, admin_delete_team, admin_get_team,
        admin_list_teams, admin_update_team, attach_filter_handler,
        attach_filter_to_listener_handler, attach_filter_to_route_rule_handler,
        attach_filter_to_virtual_host_handler, bootstrap_initialize_handler,
        bootstrap_status_handler, change_password_handler, compare_aggregated_schemas_handler,
        create_cluster_handler, create_filter_handler, create_learning_session_handler,
        create_listener_handler, create_route_config_handler, create_session_handler,
        create_token_handler, create_user, delete_cluster_handler, delete_filter_handler,
        delete_learning_session_handler, delete_listener_handler, delete_route_config_handler,
        delete_user, detach_filter_from_listener_handler, detach_filter_from_route_rule_handler,
        detach_filter_from_virtual_host_handler, detach_filter_handler,
        export_aggregated_schema_handler, generate_certificate_handler,
        get_aggregated_schema_handler, get_certificate_handler, get_cluster_handler,
        get_filter_handler, get_learning_session_handler, get_listener_handler,
        get_mtls_status_handler, get_route_config_handler, get_session_info_handler,
        get_team_bootstrap_handler, get_token_handler, get_user, health_handler,
        list_aggregated_schemas_handler, list_all_scopes_handler, list_audit_logs,
        list_certificates_handler, list_clusters_handler, list_filters_handler,
        list_learning_sessions_handler, list_listener_filters_handler, list_listeners_handler,
        list_route_configs_handler, list_route_filters_handler, list_route_flows_handler,
        list_route_rule_filters_handler, list_route_rules_handler, list_scopes_handler,
        list_teams_handler, list_tokens_handler, list_user_teams, list_users,
        list_virtual_host_filters_handler, list_virtual_hosts_handler, login_handler,
        logout_handler, remove_team_membership, revoke_certificate_handler, revoke_token_handler,
        rotate_token_handler, update_cluster_handler, update_filter_handler,
        update_listener_handler, update_route_config_handler, update_token_handler, update_user,
    },
};

#[derive(Clone)]
pub struct ApiState {
    pub xds_state: Arc<XdsState>,
}

/// Build CORS layer from environment configuration
fn build_cors_layer() -> CorsLayer {
    // Read allowed origin from environment variable, default to localhost for development
    let allowed_origin = std::env::var("FLOWPLANE_UI_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());

    tracing::info!(
        allowed_origin = %allowed_origin,
        "Configuring CORS for UI integration"
    );

    CorsLayer::new()
        // Allow specific origin (not wildcard for security with credentials)
        .allow_origin(
            allowed_origin
                .parse::<HeaderValue>()
                .unwrap_or_else(|_| HeaderValue::from_static("http://localhost:3000")),
        )
        // Allow credentials (cookies, authorization headers)
        .allow_credentials(true)
        // Allow common HTTP methods
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        // Allow headers needed for authentication and CSRF protection
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            HeaderName::from_static("x-csrf-token"),
        ])
        // Expose CSRF token header so UI can read it
        .expose_headers([HeaderName::from_static("x-csrf-token")])
}

pub fn build_router(state: Arc<XdsState>) -> Router {
    let api_state = ApiState { xds_state: state.clone() };

    let cluster_repo = match &state.cluster_repository {
        Some(repo) => repo.clone(),
        None => return docs::docs_router(),
    };

    let auth_layer = {
        let pool = cluster_repo.pool().clone();
        let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
        let auth_service = Arc::new(AuthService::with_sqlx(pool.clone(), audit_repository.clone()));
        let token_repo =
            Arc::new(crate::storage::repository::SqlxTokenRepository::new(pool.clone()));
        let session_service = Arc::new(SessionService::new(token_repo, audit_repository));

        // Create a tuple state with both services
        let auth_state = (auth_service, session_service);
        middleware::from_fn_with_state(auth_state, authenticate)
    };

    let dynamic_scope_layer = middleware::from_fn(ensure_dynamic_scopes);

    // Create OpenTelemetry HTTP tracing middleware
    // This creates spans for all HTTP requests with method, path, status, and latency
    let trace_layer = middleware::from_fn(trace_http_requests);

    let secured_api = Router::new()
        // Password change endpoint (authenticated users only)
        .route("/api/v1/auth/change-password", post(change_password_handler))
        // Token management endpoints
        .route("/api/v1/tokens", get(list_tokens_handler))
        .route("/api/v1/tokens", post(create_token_handler))
        .route("/api/v1/tokens/{id}", get(get_token_handler))
        .route("/api/v1/tokens/{id}", patch(update_token_handler))
        .route("/api/v1/tokens/{id}", delete(revoke_token_handler))
        .route("/api/v1/tokens/{id}/rotate", post(rotate_token_handler))
        // Cluster endpoints
        .route("/api/v1/clusters", get(list_clusters_handler))
        .route("/api/v1/clusters", post(create_cluster_handler))
        .route("/api/v1/clusters/{name}", get(get_cluster_handler))
        .route("/api/v1/clusters/{name}", put(update_cluster_handler))
        .route("/api/v1/clusters/{name}", delete(delete_cluster_handler))
        // Route config endpoints
        .route("/api/v1/route-configs", get(list_route_configs_handler))
        .route("/api/v1/route-configs", post(create_route_config_handler))
        .route("/api/v1/route-configs/{name}", get(get_route_config_handler))
        .route("/api/v1/route-configs/{name}", put(update_route_config_handler))
        .route("/api/v1/route-configs/{name}", delete(delete_route_config_handler))
        // Filter endpoints
        .route("/api/v1/filters", get(list_filters_handler))
        .route("/api/v1/filters", post(create_filter_handler))
        .route("/api/v1/filters/{id}", get(get_filter_handler))
        .route("/api/v1/filters/{id}", put(update_filter_handler))
        .route("/api/v1/filters/{id}", delete(delete_filter_handler))
        // Route config filter attachment endpoints
        .route("/api/v1/route-configs/{route_config_id}/filters", get(list_route_filters_handler))
        .route("/api/v1/route-configs/{route_config_id}/filters", post(attach_filter_handler))
        .route("/api/v1/route-configs/{route_config_id}/filters/{filter_id}", delete(detach_filter_handler))
        // Hierarchical filter attachment endpoints - Virtual Hosts
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts",
            get(list_virtual_hosts_handler),
        )
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters",
            get(list_virtual_host_filters_handler),
        )
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters",
            post(attach_filter_to_virtual_host_handler),
        )
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters/{filter_id}",
            delete(detach_filter_from_virtual_host_handler),
        )
        // Hierarchical filter attachment endpoints - Routes
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes",
            get(list_route_rules_handler),
        )
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters",
            get(list_route_rule_filters_handler),
        )
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters",
            post(attach_filter_to_route_rule_handler),
        )
        .route(
            "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters/{filter_id}",
            delete(detach_filter_from_route_rule_handler),
        )
        // Listener-Filter attachment endpoints
        .route("/api/v1/listeners/{listener_id}/filters", get(list_listener_filters_handler))
        .route("/api/v1/listeners/{listener_id}/filters", post(attach_filter_to_listener_handler))
        .route(
            "/api/v1/listeners/{listener_id}/filters/{filter_id}",
            delete(detach_filter_from_listener_handler),
        )
        // OpenAPI import endpoints
        .route(
            "/api/v1/openapi/import",
            post(super::handlers::openapi_import::import_openapi_handler),
        )
        .route(
            "/api/v1/openapi/imports",
            get(super::handlers::openapi_import::list_imports_handler),
        )
        .route(
            "/api/v1/openapi/imports/{id}",
            get(super::handlers::openapi_import::get_import_handler),
        )
        .route(
            "/api/v1/openapi/imports/{id}",
            delete(super::handlers::openapi_import::delete_import_handler),
        )
        // Team endpoints
        .route("/api/v1/teams", get(list_teams_handler))
        .route("/api/v1/teams/{team}/bootstrap", get(get_team_bootstrap_handler))
        // mTLS status endpoint
        .route("/api/v1/mtls/status", get(get_mtls_status_handler))
        // Proxy certificate endpoints (mTLS)
        .route("/api/v1/teams/{team}/proxy-certificates", get(list_certificates_handler))
        .route("/api/v1/teams/{team}/proxy-certificates", post(generate_certificate_handler))
        .route("/api/v1/teams/{team}/proxy-certificates/{id}", get(get_certificate_handler))
        .route("/api/v1/teams/{team}/proxy-certificates/{id}/revoke", post(revoke_certificate_handler))
        // Listener endpoints
        .route("/api/v1/listeners", get(list_listeners_handler))
        .route("/api/v1/listeners", post(create_listener_handler))
        .route("/api/v1/listeners/{name}", get(get_listener_handler))
        .route("/api/v1/listeners/{name}", put(update_listener_handler))
        .route("/api/v1/listeners/{name}", delete(delete_listener_handler))
        // Learning session endpoints (team-scoped like other resources)
        .route("/api/v1/learning-sessions", get(list_learning_sessions_handler))
        .route("/api/v1/learning-sessions", post(create_learning_session_handler))
        .route("/api/v1/learning-sessions/{id}", get(get_learning_session_handler))
        .route("/api/v1/learning-sessions/{id}", delete(delete_learning_session_handler))
        // Aggregated schema endpoints (API catalog)
        .route("/api/v1/aggregated-schemas", get(list_aggregated_schemas_handler))
        .route("/api/v1/aggregated-schemas/{id}", get(get_aggregated_schema_handler))
        .route("/api/v1/aggregated-schemas/{id}/compare", get(compare_aggregated_schemas_handler))
        .route("/api/v1/aggregated-schemas/{id}/export", get(export_aggregated_schema_handler))
        // Reporting endpoints
        .route("/api/v1/reports/route-flows", get(list_route_flows_handler))
        // User management endpoints (admin only)
        .route("/api/v1/users", get(list_users))
        .route("/api/v1/users", post(create_user))
        .route("/api/v1/users/{id}", get(get_user))
        .route("/api/v1/users/{id}", put(update_user))
        .route("/api/v1/users/{id}", delete(delete_user))
        .route("/api/v1/users/{id}/teams", get(list_user_teams))
        .route("/api/v1/users/{id}/teams", post(add_team_membership))
        .route("/api/v1/users/{id}/teams/{team}", delete(remove_team_membership))
        // Admin team management endpoints (admin only)
        .route("/api/v1/admin/teams", get(admin_list_teams))
        .route("/api/v1/admin/teams", post(admin_create_team))
        .route("/api/v1/admin/teams/{id}", get(admin_get_team))
        .route("/api/v1/admin/teams/{id}", put(admin_update_team))
        .route("/api/v1/admin/teams/{id}", delete(admin_delete_team))
        // Audit log endpoints (admin only)
        .route("/api/v1/audit-logs", get(list_audit_logs))
        // Admin scopes endpoint (includes hidden scopes like admin:all)
        .route("/api/v1/admin/scopes", get(list_all_scopes_handler))
        .with_state(api_state.clone())
        .layer(trace_layer) // Add OpenTelemetry HTTP tracing BEFORE auth layers
        .layer(dynamic_scope_layer)
        .layer(auth_layer);

    // Public endpoints (no authentication required)
    let public_api = Router::new()
        .route("/health", get(health_handler))
        .route("/api/v1/bootstrap/status", get(bootstrap_status_handler))
        .route("/api/v1/bootstrap/initialize", post(bootstrap_initialize_handler))
        .route("/api/v1/auth/login", post(login_handler))
        .route("/api/v1/auth/sessions", post(create_session_handler))
        .route("/api/v1/auth/sessions/me", get(get_session_info_handler))
        .route("/api/v1/auth/sessions/logout", post(logout_handler))
        // Scopes endpoint (public - needed for token creation UI)
        .route("/api/v1/scopes", get(list_scopes_handler))
        .with_state(api_state);

    // Build CORS layer for UI integration
    let cors_layer = build_cors_layer();

    // Apply CORS layer to all routes
    secured_api.merge(public_api).merge(docs::docs_router()).layer(cors_layer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cors_layer_allows_configured_origin() {
        // Set environment variable for test
        std::env::set_var("FLOWPLANE_UI_ORIGIN", "https://app.example.com");

        let cors_layer = build_cors_layer();

        // The CorsLayer is built successfully
        // Actual CORS behavior is tested via integration tests with HTTP requests
        drop(cors_layer);

        // Clean up
        std::env::remove_var("FLOWPLANE_UI_ORIGIN");
    }

    #[test]
    fn test_cors_layer_defaults_to_localhost() {
        // Ensure no environment variable is set
        std::env::remove_var("FLOWPLANE_UI_ORIGIN");

        let cors_layer = build_cors_layer();

        // The CorsLayer is built successfully with default localhost
        // Actual CORS behavior is tested via integration tests with HTTP requests
        drop(cors_layer);
    }
}
