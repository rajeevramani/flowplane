use std::sync::Arc;

use axum::{
    middleware,
    routing::{delete, get, patch, post, put},
    Router,
};

use crate::auth::{
    auth_service::AuthService,
    middleware::{authenticate, ensure_dynamic_scopes},
};
use crate::observability::trace_http_requests;
use crate::storage::repository::AuditLogRepository;
use crate::xds::XdsState;

use super::{
    docs,
    handlers::{
        append_route_handler, compare_aggregated_schemas_handler, create_api_definition_handler,
        create_cluster_handler, create_learning_session_handler, create_listener_handler,
        create_route_handler, create_token_handler, delete_cluster_handler,
        delete_learning_session_handler, delete_listener_handler, delete_route_handler,
        export_aggregated_schema_handler, get_aggregated_schema_handler,
        get_api_definition_handler, get_cluster_handler, get_learning_session_handler,
        get_listener_handler, get_route_handler, get_team_bootstrap_handler, get_token_handler,
        health_handler, import_openapi_handler, list_aggregated_schemas_handler,
        list_api_definitions_handler, list_clusters_handler, list_learning_sessions_handler,
        list_listeners_handler, list_route_flows_handler, list_routes_handler, list_tokens_handler,
        revoke_token_handler, rotate_token_handler, update_api_definition_handler,
        update_cluster_handler, update_listener_handler, update_route_handler,
        update_token_handler,
    },
};

#[derive(Clone)]
pub struct ApiState {
    pub xds_state: Arc<XdsState>,
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
        let auth_service = Arc::new(AuthService::with_sqlx(pool, audit_repository));
        middleware::from_fn_with_state(auth_service, authenticate)
    };

    let dynamic_scope_layer = middleware::from_fn(ensure_dynamic_scopes);

    // Create OpenTelemetry HTTP tracing middleware
    // This creates spans for all HTTP requests with method, path, status, and latency
    let trace_layer = middleware::from_fn(trace_http_requests);

    let secured_api = Router::new()
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
        // Route endpoints
        .route("/api/v1/routes", get(list_routes_handler))
        .route("/api/v1/routes", post(create_route_handler))
        .route("/api/v1/routes/{name}", get(get_route_handler))
        .route("/api/v1/routes/{name}", put(update_route_handler))
        .route("/api/v1/routes/{name}", delete(delete_route_handler))
        // API definition endpoints
        .route("/api/v1/api-definitions", get(list_api_definitions_handler))
        .route("/api/v1/api-definitions", post(create_api_definition_handler))
        .route("/api/v1/api-definitions/from-openapi", post(import_openapi_handler))
        .route("/api/v1/api-definitions/{id}", get(get_api_definition_handler))
        .route("/api/v1/api-definitions/{id}", patch(update_api_definition_handler))
        .route("/api/v1/api-definitions/{id}/routes", post(append_route_handler))
        // Team endpoints
        .route("/api/v1/teams/{team}/bootstrap", get(get_team_bootstrap_handler))
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
        .with_state(api_state.clone())
        .layer(trace_layer) // Add OpenTelemetry HTTP tracing BEFORE auth layers
        .layer(dynamic_scope_layer)
        .layer(auth_layer);

    // Public endpoints (no authentication required)
    let public_api = Router::new().route("/health", get(health_handler)).with_state(api_state);

    secured_api.merge(public_api).merge(docs::docs_router())
}
