use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    http::{header, HeaderName, Method},
    middleware,
    routing::{delete, get, patch, post, put},
    Router,
};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::auth::middleware::{authenticate, ensure_dynamic_scopes};
use crate::domain::SharedFilterSchemaRegistry;
use crate::observability::trace_http_requests;
use crate::services::stats_cache::{StatsCache, StatsCacheConfig};
use crate::xds::XdsState;

use super::{
    docs,
    handlers::custom_wasm_filters::{
        create_custom_wasm_filter_handler, delete_custom_wasm_filter_handler,
        download_wasm_binary_handler, get_custom_wasm_filter_handler,
        list_custom_wasm_filters_handler, update_custom_wasm_filter_handler,
    },
    handlers::dataplanes::{
        create_dataplane_handler, delete_dataplane_handler, generate_envoy_config_handler,
        get_dataplane_handler, list_all_dataplanes_handler, list_dataplanes_handler,
        update_dataplane_handler,
    },
    handlers::secrets::{
        create_secret_handler, create_secret_reference_handler, delete_secret_handler,
        get_secret_handler,
    },
    handlers::{
        add_team_member, admin_add_org_member, admin_create_organization, admin_create_team,
        admin_delete_organization, admin_delete_team, admin_get_organization, admin_get_team,
        admin_invite_org_member, admin_list_org_members, admin_list_organizations,
        admin_list_teams, admin_remove_org_member, admin_resource_summary_handler,
        admin_update_org_member_role, admin_update_organization, admin_update_team,
        apply_learned_schema_handler, attach_filter_handler, attach_filter_to_listener_handler,
        attach_filter_to_route_rule_handler, attach_filter_to_virtual_host_handler,
        auth_session_handler, bootstrap_initialize_handler, bootstrap_status_handler,
        bulk_disable_mcp_handler, bulk_enable_mcp_handler, check_learned_schema_handler,
        compare_aggregated_schemas_handler, configure_filter_handler, create_agent_grant,
        create_cluster_handler, create_filter_handler, create_learning_session_handler,
        create_listener_handler, create_org_agent, create_org_team, create_route_config_handler,
        dcr_register_handler, delete_agent_grant, delete_cluster_handler, delete_filter_handler,
        delete_learning_session_handler, delete_listener_handler, delete_org_agent,
        delete_org_team, delete_route_config_handler, detach_filter_from_listener_handler,
        detach_filter_from_route_rule_handler, detach_filter_from_virtual_host_handler,
        detach_filter_handler, disable_mcp_handler, enable_mcp_handler,
        export_aggregated_schema_handler, export_multiple_schemas_handler,
        generate_certificate_handler, get_aggregated_schema_handler, get_app_handler,
        get_certificate_handler, get_cluster_handler, get_current_org, get_filter_handler,
        get_filter_status_handler, get_filter_type_handler, get_learning_session_handler,
        get_listener_handler, get_mcp_status_handler, get_mcp_tool_handler,
        get_mtls_status_handler, get_route_config_handler, get_route_stats_handler,
        get_stats_cluster_handler, get_stats_clusters_handler, get_stats_enabled_handler,
        get_stats_overview_handler, health_handler, install_filter_handler, list_agent_grants,
        list_aggregated_schemas_handler, list_all_scopes_handler, list_apps_handler,
        list_audit_logs, list_certificates_handler, list_clusters_handler,
        list_filter_configurations_handler, list_filter_installations_handler,
        list_filter_types_handler, list_filters_handler, list_learning_sessions_handler,
        list_listener_filters_handler, list_listeners_handler, list_mcp_tools_handler,
        list_org_agents, list_org_teams, list_route_configs_handler, list_route_filters_handler,
        list_route_flows_handler, list_route_rule_filters_handler, list_route_rules_handler,
        list_route_views_handler, list_scopes_handler, list_secrets_handler, list_team_members,
        list_teams_handler, list_virtual_host_filters_handler, list_virtual_hosts_handler,
        refresh_mcp_schema_handler, reload_filter_schemas_handler,
        remove_filter_configuration_handler, remove_team_member, revoke_certificate_handler,
        set_app_status_handler, uninstall_filter_handler, update_cluster_handler,
        update_filter_handler, update_listener_handler, update_mcp_tool_handler,
        update_org_agent_scopes, update_org_team, update_route_config_handler,
        update_secret_handler, update_team_member_scopes,
    },
};

#[derive(Clone)]
pub struct ApiState {
    pub xds_state: Arc<XdsState>,
    pub filter_schema_registry: Option<SharedFilterSchemaRegistry>,
    pub stats_cache: Arc<StatsCache>,
    pub mcp_connection_manager: crate::mcp::SharedConnectionManager,
    pub mcp_session_manager: crate::mcp::SharedSessionManager,
    /// Rate limiter for certificate generation (prevents Vault PKI exhaustion)
    pub certificate_rate_limiter: Arc<super::rate_limit::RateLimiter>,
    /// Authentication configuration (cookie_secure, invite settings, proxy config)
    pub auth_config: Arc<crate::config::AuthConfig>,
    /// Zitadel Management API client for user provisioning (invite flow).
    /// `None` when `FLOWPLANE_ZITADEL_ADMIN_PAT` is not configured.
    pub zitadel_admin: Option<Arc<crate::auth::zitadel_admin::ZitadelAdminClient>>,
    /// Permission cache shared with auth middleware for cache invalidation
    /// after membership mutations.
    pub permission_cache: Option<Arc<crate::auth::cache::PermissionCache>>,
}

/// Get the UI static files directory path from environment or default
fn get_ui_static_dir() -> Option<PathBuf> {
    let ui_dir = std::env::var("FLOWPLANE_UI_DIR").unwrap_or_else(|_| "./ui/build".to_string());

    let path = PathBuf::from(&ui_dir);
    if path.exists() && path.is_dir() {
        tracing::info!(ui_dir = %ui_dir, "UI static files directory found");
        Some(path)
    } else {
        tracing::debug!(ui_dir = %ui_dir, "UI static files directory not found, UI will not be served");
        None
    }
}

/// Build CORS layer from environment configuration
/// Supports multiple origins via comma-separated FLOWPLANE_UI_ORIGIN
/// Example: FLOWPLANE_UI_ORIGIN=http://localhost:3000,http://localhost:6274
fn build_cors_layer() -> CorsLayer {
    // Read allowed origins from environment variable
    // Default includes localhost:3000 (UI) and localhost:6274 (MCP Inspector)
    let allowed_origins_str = std::env::var("FLOWPLANE_UI_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:3000,http://localhost:6274".to_string());

    // Parse comma-separated origins into a list
    let allowed_origins: Vec<String> = allowed_origins_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    tracing::info!(
        allowed_origins = ?allowed_origins,
        "Configuring CORS for UI integration"
    );

    // Create origin predicate that checks against allowed list
    let origins = allowed_origins.clone();
    let allow_origin = AllowOrigin::predicate(move |origin, _request_parts| {
        origin.to_str().map(|o| origins.iter().any(|allowed| allowed == o)).unwrap_or(false)
    });

    CorsLayer::new()
        // Allow specific origins (checked via predicate)
        .allow_origin(allow_origin)
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
        // Allow headers needed for authentication, CSRF, and MCP protocol
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::ACCEPT,
            HeaderName::from_static("x-csrf-token"),
            // MCP 2025-03-26 protocol headers
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("mcp-session-id"),
            HeaderName::from_static("last-event-id"),
        ])
        // Expose headers so clients can read them
        .expose_headers([
            HeaderName::from_static("x-csrf-token"),
            // MCP session ID must be readable by clients
            HeaderName::from_static("mcp-session-id"),
        ])
}

pub fn build_router(state: Arc<XdsState>) -> Router {
    build_router_with_registry(state, None)
}

pub fn build_router_with_registry(
    state: Arc<XdsState>,
    filter_schema_registry: Option<SharedFilterSchemaRegistry>,
) -> Router {
    // Create stats cache with default config (10 second TTL, 100 max entries)
    let stats_cache = Arc::new(StatsCache::new(StatsCacheConfig::default()));

    // Create MCP connection manager for SSE streaming
    let mcp_connection_manager = crate::mcp::create_connection_manager();

    // Create MCP session manager for HTTP-only connections
    let mcp_session_manager = crate::mcp::create_session_manager();

    // Create rate limiter for certificate generation (prevents Vault PKI exhaustion)
    let certificate_rate_limiter = Arc::new(super::rate_limit::RateLimiter::from_env());

    // Load auth config from environment
    let auth_config = Arc::new(crate::config::AuthConfig::from_env());

    // Startup safety checks for cookie_secure
    if !auth_config.cookie_secure {
        tracing::warn!("Cookie Secure flag is DISABLED — only safe for local HTTP development");
        if auth_config.base_url.starts_with("https://") {
            panic!("FLOWPLANE_COOKIE_SECURE=false with HTTPS base_url is a dangerous misconfiguration — refusing to start");
        }
    }

    // Early return: if no cluster repository is configured, only serve docs
    if state.cluster_repository.is_none() {
        return docs::docs_router();
    }

    // Create permission cache (shared between middleware and handlers for cache invalidation)
    let permission_cache = Arc::new(crate::auth::cache::PermissionCache::from_env());

    // Create Zitadel admin client for user provisioning (invite endpoint)
    let zitadel_admin = crate::auth::zitadel_admin::ZitadelAdminClient::from_env().map(Arc::new);

    let (auth_layer, api_permission_cache) = match crate::auth::zitadel::ZitadelConfig::from_env() {
        Some(zitadel_config) => {
            tracing::info!(issuer = %zitadel_config.issuer, "Zitadel JWT auth enabled");
            let pool = state
                .pool
                .clone()
                .expect("DB pool required for Zitadel auth middleware — start with --database");
            let zitadel_state = crate::auth::zitadel::ZitadelAuthState {
                jwks_cache: crate::auth::zitadel::JwksCache::new(&zitadel_config),
                config: std::sync::Arc::new(zitadel_config),
                pool,
                permission_cache: permission_cache.clone(),
            };
            (
                tower::util::Either::Left(middleware::from_fn_with_state(
                    zitadel_state,
                    authenticate,
                )),
                Some(permission_cache),
            )
        }
        None => {
            tracing::warn!(
                "Zitadel not configured — starting in degraded mode (auth endpoints return 503)"
            );
            (tower::util::Either::Right(middleware::from_fn(reject_all_auth)), None)
        }
    };

    let api_state = ApiState {
        xds_state: state.clone(),
        filter_schema_registry,
        stats_cache,
        mcp_connection_manager,
        mcp_session_manager,
        certificate_rate_limiter,
        auth_config,
        zitadel_admin,
        permission_cache: api_permission_cache,
    };

    let dynamic_scope_layer = middleware::from_fn(ensure_dynamic_scopes);

    // Create OpenTelemetry HTTP tracing middleware
    // This creates spans for all HTTP requests with method, path, status, and latency
    let trace_layer = middleware::from_fn(trace_http_requests);

    let secured_api = Router::new()
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
        // Route views endpoints (UI flattened view)
        .route("/api/v1/route-views", get(list_route_views_handler))
        .route("/api/v1/route-views/stats", get(get_route_stats_handler))
        // Filter endpoints
        .route("/api/v1/filters", get(list_filters_handler))
        .route("/api/v1/filters", post(create_filter_handler))
        .route("/api/v1/filters/{id}", get(get_filter_handler))
        .route("/api/v1/filters/{id}", put(update_filter_handler))
        .route("/api/v1/filters/{id}", delete(delete_filter_handler))
        // Filter Install/Configure endpoints (redesign)
        .route("/api/v1/filters/{filter_id}/status", get(get_filter_status_handler))
        .route("/api/v1/filters/{filter_id}/installations", get(list_filter_installations_handler))
        .route("/api/v1/filters/{filter_id}/installations", post(install_filter_handler))
        .route("/api/v1/filters/{filter_id}/installations/{listener_id}", delete(uninstall_filter_handler))
        .route("/api/v1/filters/{filter_id}/configurations", get(list_filter_configurations_handler))
        .route("/api/v1/filters/{filter_id}/configurations", post(configure_filter_handler))
        .route(
            "/api/v1/filters/{filter_id}/configurations/{scope_type}/{scope_id}",
            delete(remove_filter_configuration_handler),
        )
        // Filter types endpoints (dynamic filter framework)
        .route("/api/v1/filter-types", get(list_filter_types_handler))
        .route("/api/v1/filter-types/{filter_type}", get(get_filter_type_handler))
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
        // mTLS status endpoint
        .route("/api/v1/mtls/status", get(get_mtls_status_handler))
        // Proxy certificate endpoints (mTLS)
        .route("/api/v1/teams/{team}/proxy-certificates", get(list_certificates_handler))
        .route("/api/v1/teams/{team}/proxy-certificates", post(generate_certificate_handler))
        .route("/api/v1/teams/{team}/proxy-certificates/{id}", get(get_certificate_handler))
        .route("/api/v1/teams/{team}/proxy-certificates/{id}/revoke", post(revoke_certificate_handler))
        // Secrets endpoints (SDS)
        .route("/api/v1/teams/{team}/secrets", get(list_secrets_handler))
        .route("/api/v1/teams/{team}/secrets", post(create_secret_handler))
        .route("/api/v1/teams/{team}/secrets/reference", post(create_secret_reference_handler))
        .route("/api/v1/teams/{team}/secrets/{secret_id}", get(get_secret_handler))
        .route("/api/v1/teams/{team}/secrets/{secret_id}", put(update_secret_handler))
        .route("/api/v1/teams/{team}/secrets/{secret_id}", delete(delete_secret_handler))
        // Custom WASM filter endpoints
        .route("/api/v1/teams/{team}/custom-filters", get(list_custom_wasm_filters_handler))
        .route("/api/v1/teams/{team}/custom-filters", post(create_custom_wasm_filter_handler))
        .route("/api/v1/teams/{team}/custom-filters/{id}", get(get_custom_wasm_filter_handler))
        .route("/api/v1/teams/{team}/custom-filters/{id}", put(update_custom_wasm_filter_handler))
        .route("/api/v1/teams/{team}/custom-filters/{id}", delete(delete_custom_wasm_filter_handler))
        .route("/api/v1/teams/{team}/custom-filters/{id}/download", get(download_wasm_binary_handler))
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
        // Dataplane endpoints (team-scoped Envoy instances with gateway_host)
        .route("/api/v1/dataplanes", get(list_all_dataplanes_handler))
        .route("/api/v1/teams/{team}/dataplanes", get(list_dataplanes_handler))
        .route("/api/v1/teams/{team}/dataplanes", post(create_dataplane_handler))
        .route("/api/v1/teams/{team}/dataplanes/{name}", get(get_dataplane_handler))
        .route("/api/v1/teams/{team}/dataplanes/{name}", put(update_dataplane_handler))
        .route("/api/v1/teams/{team}/dataplanes/{name}", delete(delete_dataplane_handler))
        .route("/api/v1/teams/{team}/dataplanes/{name}/envoy-config", get(generate_envoy_config_handler))
        // Aggregated schema endpoints (API catalog)
        .route("/api/v1/aggregated-schemas", get(list_aggregated_schemas_handler))
        .route("/api/v1/aggregated-schemas/{id}", get(get_aggregated_schema_handler))
        .route("/api/v1/aggregated-schemas/{id}/compare", get(compare_aggregated_schemas_handler))
        .route("/api/v1/aggregated-schemas/{id}/export", get(export_aggregated_schema_handler))
        .route("/api/v1/aggregated-schemas/export", post(export_multiple_schemas_handler))
        // Reporting endpoints
        .route("/api/v1/reports/route-flows", get(list_route_flows_handler))
        // Auth session endpoint (any authenticated user — returns DB-sourced permissions)
        .route("/api/v1/auth/session", get(auth_session_handler))
        // Admin team management endpoints (admin only)
        .route("/api/v1/admin/teams", get(admin_list_teams))
        .route("/api/v1/admin/teams", post(admin_create_team))
        .route("/api/v1/admin/teams/{id}", get(admin_get_team))
        .route("/api/v1/admin/teams/{id}", put(admin_update_team))
        .route("/api/v1/admin/teams/{id}", delete(admin_delete_team))
        // Org-scoped endpoints (authenticated users)
        .route("/api/v1/orgs/current", get(get_current_org))
        .route("/api/v1/orgs/{org_name}/teams", get(list_org_teams))
        .route("/api/v1/orgs/{org_name}/teams", post(create_org_team))
        .route("/api/v1/orgs/{org_name}/teams/{team_name}", put(update_org_team).delete(delete_org_team))
        .route("/api/v1/orgs/{org_name}/teams/{team_name}/members", get(list_team_members).post(add_team_member))
        .route(
            "/api/v1/orgs/{org_name}/teams/{team_name}/members/{user_id}",
            put(update_team_member_scopes).delete(remove_team_member),
        )
        .route(
            "/api/v1/orgs/{org_name}/agents",
            get(list_org_agents).post(create_org_agent),
        )
        .route(
            "/api/v1/orgs/{org_name}/agents/{agent_name}",
            delete(delete_org_agent),
        )
        .route(
            "/api/v1/orgs/{org_name}/agents/{agent_name}/scopes",
            put(update_org_agent_scopes),
        )
        .route(
            "/api/v1/orgs/{org_name}/agents/{agent_name}/grants",
            get(list_agent_grants).post(create_agent_grant),
        )
        .route(
            "/api/v1/orgs/{org_name}/agents/{agent_name}/grants/{grant_id}",
            delete(delete_agent_grant),
        )
        // Admin organization management endpoints
        .route("/api/v1/admin/organizations", get(admin_list_organizations))
        .route("/api/v1/admin/organizations", post(admin_create_organization))
        .route("/api/v1/admin/organizations/{id}", get(admin_get_organization))
        .route("/api/v1/admin/organizations/{id}", put(admin_update_organization))
        .route("/api/v1/admin/organizations/{id}", delete(admin_delete_organization))
        .route("/api/v1/admin/organizations/{id}/members", get(admin_list_org_members))
        .route("/api/v1/admin/organizations/{id}/members", post(admin_add_org_member))
        .route("/api/v1/admin/organizations/{id}/members/{user_id}", put(admin_update_org_member_role))
        .route("/api/v1/admin/organizations/{id}/members/{user_id}", delete(admin_remove_org_member))
        .route("/api/v1/admin/organizations/{id}/invite", post(admin_invite_org_member))
        // Admin resource summary (platform admin dashboard)
        .route("/api/v1/admin/resources/summary", get(admin_resource_summary_handler))
        // Audit log endpoints (admin only)
        .route("/api/v1/audit-logs", get(list_audit_logs))
        // Admin scopes endpoint (includes hidden scopes like admin:all)
        .route("/api/v1/admin/scopes", get(list_all_scopes_handler))
        // Admin filter schema management
        .route("/api/v1/admin/filter-schemas/reload", post(reload_filter_schemas_handler))
        // Admin app management endpoints
        .route("/api/v1/admin/apps", get(list_apps_handler))
        .route("/api/v1/admin/apps/{app_id}", get(get_app_handler))
        .route("/api/v1/admin/apps/{app_id}", put(set_app_status_handler))
        // Stats dashboard endpoints
        .route("/api/v1/stats/enabled", get(get_stats_enabled_handler))
        .route("/api/v1/teams/{team}/stats/overview", get(get_stats_overview_handler))
        .route("/api/v1/teams/{team}/stats/clusters", get(get_stats_clusters_handler))
        .route("/api/v1/teams/{team}/stats/clusters/{cluster}", get(get_stats_cluster_handler))
        // MCP protocol endpoint - unified CP and Gateway API tools (Streamable HTTP 2025-11-25)
        .route(
            "/api/v1/mcp",
            post(crate::mcp::post_handler)
                .get(crate::mcp::get_handler)
                .delete(crate::mcp::delete_handler),
        )
        .route("/api/v1/mcp/connections", get(crate::mcp::list_connections_handler))
        // MCP tools management endpoints
        .route("/api/v1/teams/{team}/mcp/tools", get(list_mcp_tools_handler))
        .route("/api/v1/teams/{team}/mcp/tools/{name}", get(get_mcp_tool_handler))
        .route("/api/v1/teams/{team}/mcp/tools/{name}", patch(update_mcp_tool_handler))
        // MCP route enablement endpoints
        .route("/api/v1/teams/{team}/routes/{route_id}/mcp/status", get(get_mcp_status_handler))
        .route("/api/v1/teams/{team}/routes/{route_id}/mcp/enable", post(enable_mcp_handler))
        .route("/api/v1/teams/{team}/routes/{route_id}/mcp/disable", post(disable_mcp_handler))
        .route("/api/v1/teams/{team}/routes/{route_id}/mcp/refresh", post(refresh_mcp_schema_handler))
        // MCP bulk operations
        .route("/api/v1/teams/{team}/mcp/bulk-enable", post(bulk_enable_mcp_handler))
        .route("/api/v1/teams/{team}/mcp/bulk-disable", post(bulk_disable_mcp_handler))
        // MCP learned schema operations
        .route("/api/v1/teams/{team}/mcp/routes/{route_id}/learned-schema", get(check_learned_schema_handler))
        .route("/api/v1/teams/{team}/mcp/routes/{route_id}/apply-learned", post(apply_learned_schema_handler))
        // DCR endpoint (authenticated — org admin required, merged into secured router)
        .route("/api/v1/oauth/register", post(dcr_register_handler))
        .with_state(api_state.clone())
        .layer(trace_layer) // Add OpenTelemetry HTTP tracing BEFORE auth layers
        .layer(dynamic_scope_layer)
        .layer(auth_layer);

    // Public endpoints (no authentication required)
    let public_api = Router::new()
        .route("/health", get(health_handler))
        .route("/api/v1/bootstrap/status", get(bootstrap_status_handler))
        .route("/api/v1/bootstrap/initialize", post(bootstrap_initialize_handler))
        // Scopes endpoint (public - needed for token creation UI)
        .route("/api/v1/scopes", get(list_scopes_handler))
        .with_state(api_state.clone());

    // OAuth metadata endpoint (public, points to Zitadel)
    let metadata_router =
        if let Some(meta_state) = super::handlers::oauth::OAuthMetadataState::from_env() {
            tracing::info!("OAuth metadata enabled at /.well-known/openid-configuration");
            Router::new()
                .route(
                    "/.well-known/openid-configuration",
                    get(super::handlers::oauth::openid_configuration_handler),
                )
                .with_state(meta_state)
        } else {
            Router::new()
        };

    // Auth config endpoint (public, returns OIDC config for SPA runtime initialization)
    let auth_config_router =
        if let Some(auth_config_state) = super::handlers::oauth::AuthConfigState::from_env() {
            tracing::info!("Auth config enabled at GET /api/v1/auth/config");
            Router::new()
                .route("/api/v1/auth/config", get(super::handlers::oauth::auth_config_handler))
                .with_state(auth_config_state)
        } else {
            tracing::info!("Auth config disabled (FLOWPLANE_ZITADEL_SPA_CLIENT_ID not set)");
            Router::new()
        };

    // Build CORS layer for UI integration
    let cors_layer = build_cors_layer();

    // Build the API router with CORS
    let api_router = secured_api
        .merge(public_api)
        .merge(metadata_router)
        .merge(auth_config_router)
        .merge(docs::docs_router())
        .layer(cors_layer);

    // Check if UI static files directory exists and add fallback service
    if let Some(ui_dir) = get_ui_static_dir() {
        let index_file = ui_dir.join("index.html");
        let serve_dir = ServeDir::new(&ui_dir).not_found_service(ServeFile::new(&index_file));

        tracing::info!("Serving UI from {:?}", ui_dir);

        // API routes take precedence, then fall back to static files
        api_router.fallback_service(serve_dir)
    } else {
        api_router
    }
}

/// Reject-all auth middleware for degraded mode (Zitadel not configured).
/// Returns 503 Service Unavailable for all authenticated endpoints.
async fn reject_all_auth(
    _req: axum::extract::Request,
    _next: middleware::Next,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({"error": "Auth not configured — run setup-zitadel"})),
    )
        .into_response()
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

    #[test]
    fn test_cors_layer_allows_multiple_origins() {
        // Set environment variable with multiple origins
        std::env::set_var(
            "FLOWPLANE_UI_ORIGIN",
            "http://localhost:3000,http://localhost:6274,https://app.example.com",
        );

        let cors_layer = build_cors_layer();

        // The CorsLayer is built successfully with multiple origins
        // Actual CORS behavior is tested via integration tests with HTTP requests
        drop(cors_layer);

        // Clean up
        std::env::remove_var("FLOWPLANE_UI_ORIGIN");
    }
}
