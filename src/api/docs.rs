use axum::Router;
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

#[allow(unused_imports)]
use crate::api::handlers::auth::{CreateTokenBody, UpdateTokenBody};
#[allow(unused_imports)]
use crate::api::handlers::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};
#[allow(unused_imports)]
use crate::auth::{models::PersonalAccessToken, token_service::TokenSecretResponse};
#[allow(unused_imports)]
use crate::xds::filters::http::{
    cors::CorsPolicyConfig, custom_response::CustomResponseConfig,
    header_mutation::HeaderMutationConfig, health_check::HealthCheckConfig,
    local_rate_limit::LocalRateLimitConfig, rate_limit::RateLimitConfig,
};
#[allow(unused_imports)]
use crate::xds::{
    CircuitBreakerThresholdsSpec, CircuitBreakersSpec, ClusterSpec, EndpointSpec, HealthCheckSpec,
    OutlierDetectionSpec,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::api::handlers::health::health_handler,
        // Bootstrap endpoints
        crate::api::handlers::bootstrap::bootstrap_initialize_handler,
        crate::api::handlers::bootstrap::bootstrap_status_handler,
        // Auth endpoints
        crate::api::handlers::auth::create_token_handler,
        crate::api::handlers::auth::list_tokens_handler,
        crate::api::handlers::auth::get_token_handler,
        crate::api::handlers::auth::update_token_handler,
        crate::api::handlers::auth::revoke_token_handler,
        crate::api::handlers::auth::rotate_token_handler,
        crate::api::handlers::auth::create_session_handler,
        crate::api::handlers::auth::get_session_info_handler,
        crate::api::handlers::auth::logout_handler,
        crate::api::handlers::auth::login_handler,
        crate::api::handlers::auth::change_password_handler,
        // Cluster endpoints
        crate::api::handlers::clusters::create_cluster_handler,
        crate::api::handlers::clusters::list_clusters_handler,
        crate::api::handlers::clusters::get_cluster_handler,
        crate::api::handlers::clusters::update_cluster_handler,
        crate::api::handlers::clusters::delete_cluster_handler,
        // Route config endpoints
        crate::api::handlers::route_configs::create_route_config_handler,
        crate::api::handlers::route_configs::list_route_configs_handler,
        crate::api::handlers::route_configs::get_route_config_handler,
        crate::api::handlers::route_configs::update_route_config_handler,
        crate::api::handlers::route_configs::delete_route_config_handler,
        // Listener endpoints
        crate::api::handlers::listeners::create_listener_handler,
        crate::api::handlers::listeners::list_listeners_handler,
        crate::api::handlers::listeners::get_listener_handler,
        crate::api::handlers::listeners::update_listener_handler,
        crate::api::handlers::listeners::delete_listener_handler,
        // Team endpoints
        crate::api::handlers::teams::get_team_bootstrap_handler,
        crate::api::handlers::teams::list_teams_handler,
        crate::api::handlers::teams::admin_create_team,
        crate::api::handlers::teams::admin_list_teams,
        crate::api::handlers::teams::admin_get_team,
        crate::api::handlers::teams::admin_update_team,
        crate::api::handlers::teams::admin_delete_team,
        // User management endpoints
        crate::api::handlers::users::create_user,
        crate::api::handlers::users::list_users,
        crate::api::handlers::users::get_user,
        crate::api::handlers::users::update_user,
        crate::api::handlers::users::delete_user,
        crate::api::handlers::users::list_user_teams,
        crate::api::handlers::users::add_team_membership,
        crate::api::handlers::users::remove_team_membership,
        // Scope endpoints
        crate::api::handlers::scopes::list_scopes_handler,
        crate::api::handlers::scopes::list_all_scopes_handler,
        // OpenAPI import endpoints
        crate::api::handlers::openapi_import::import_openapi_handler,
        crate::api::handlers::openapi_import::list_imports_handler,
        crate::api::handlers::openapi_import::get_import_handler,
        crate::api::handlers::openapi_import::delete_import_handler,
        // Audit log endpoints
        crate::api::handlers::audit_log::list_audit_logs,
        // Reporting endpoints
        crate::api::handlers::reporting::list_route_flows_handler,
        // Learning session endpoints
        crate::api::handlers::learning_sessions::create_learning_session_handler,
        crate::api::handlers::learning_sessions::list_learning_sessions_handler,
        crate::api::handlers::learning_sessions::get_learning_session_handler,
        crate::api::handlers::learning_sessions::delete_learning_session_handler,
        // Aggregated schema endpoints
        crate::api::handlers::aggregated_schemas::list_aggregated_schemas_handler,
        crate::api::handlers::aggregated_schemas::get_aggregated_schema_handler,
        crate::api::handlers::aggregated_schemas::compare_aggregated_schemas_handler,
        crate::api::handlers::aggregated_schemas::export_aggregated_schema_handler
    ),
    components(
        schemas(
            crate::api::handlers::health::HealthResponse,
            // Bootstrap schemas
            crate::api::handlers::bootstrap::BootstrapInitializeRequest,
            crate::api::handlers::bootstrap::BootstrapInitializeResponse,
            crate::api::handlers::bootstrap::BootstrapStatusResponse,
            // Cluster schemas
            CreateClusterBody,
            EndpointRequest,
            HealthCheckRequest,
            CircuitBreakersRequest,
            CircuitBreakerThresholdsRequest,
            OutlierDetectionRequest,
            ClusterResponse,
            ClusterSpec,
            EndpointSpec,
            CircuitBreakersSpec,
            CircuitBreakerThresholdsSpec,
            HealthCheckSpec,
            OutlierDetectionSpec,
            // Auth/Token schemas
            CreateTokenBody,
            UpdateTokenBody,
            PersonalAccessToken,
            TokenSecretResponse,
            crate::api::handlers::auth::CreateSessionBody,
            crate::api::handlers::auth::CreateSessionResponseBody,
            crate::api::handlers::auth::SessionInfoResponse,
            crate::api::handlers::auth::LoginBody,
            crate::api::handlers::auth::LoginResponseBody,
            crate::api::handlers::auth::ChangePasswordBody,
            // Route config schemas
            crate::api::handlers::route_configs::RouteConfigDefinition,
            crate::api::handlers::route_configs::VirtualHostDefinition,
            crate::api::handlers::route_configs::RouteRuleDefinition,
            crate::api::handlers::route_configs::RouteMatchDefinition,
            crate::api::handlers::route_configs::PathMatchDefinition,
            crate::api::handlers::route_configs::RouteActionDefinition,
            crate::api::handlers::route_configs::WeightedClusterDefinition,
            crate::api::handlers::route_configs::RouteConfigResponse,
            // Listener schemas
            crate::api::handlers::listeners::ListenerResponse,
            crate::api::handlers::listeners::CreateListenerBody,
            crate::api::handlers::listeners::UpdateListenerBody,
            // Team schemas
            crate::api::handlers::teams::BootstrapQuery,
            crate::api::handlers::teams::ListTeamsResponse,
            crate::api::handlers::teams::AdminListTeamsResponse,
            crate::auth::team::Team,
            crate::auth::team::CreateTeamRequest,
            crate::auth::team::UpdateTeamRequest,
            // User management schemas
            crate::api::handlers::users::ListUsersResponse,
            crate::auth::user::CreateUserRequest,
            crate::auth::user::UpdateUserRequest,
            crate::auth::user::UserResponse,
            crate::auth::user::UserWithTeamsResponse,
            crate::auth::user::CreateTeamMembershipRequest,
            crate::auth::user::UserTeamMembership,
            // Scope schemas
            crate::api::handlers::scopes::ListScopesResponse,
            crate::storage::repositories::ScopeDefinition,
            // OpenAPI import schemas
            crate::api::handlers::openapi_import::ImportResponse,
            crate::api::handlers::openapi_import::ListImportsResponse,
            crate::api::handlers::openapi_import::ImportSummary,
            crate::api::handlers::openapi_import::ImportDetailsResponse,
            crate::api::handlers::openapi_import::OpenApiSpecBody,
            // Audit log schemas
            crate::api::handlers::audit_log::ListAuditLogsResponse,
            crate::storage::repositories::AuditLogEntry,
            // Commonly used HTTP filter configurations
            CorsPolicyConfig,
            CustomResponseConfig,
            HeaderMutationConfig,
            HealthCheckConfig,
            LocalRateLimitConfig,
            RateLimitConfig,
            // Reporting schemas
            crate::api::handlers::reporting::ListRouteFlowsResponse,
            crate::api::handlers::reporting::RouteFlowEntry,
            crate::api::handlers::reporting::RouteFlowListener,
            crate::api::handlers::reporting::ListRouteFlowsQuery,
            // Learning session schemas
            crate::api::handlers::learning_sessions::CreateLearningSessionBody,
            crate::api::handlers::learning_sessions::LearningSessionResponse,
            crate::api::handlers::learning_sessions::ListLearningSessionsQuery,
            // Aggregated schema schemas
            crate::api::handlers::aggregated_schemas::AggregatedSchemaResponse,
            crate::api::handlers::aggregated_schemas::ListAggregatedSchemasQuery,
            crate::api::handlers::aggregated_schemas::CompareSchemaQuery,
            crate::api::handlers::aggregated_schemas::SchemaComparisonResponse,
            crate::api::handlers::aggregated_schemas::SchemaDifferences,
            crate::api::handlers::aggregated_schemas::ExportSchemaQuery,
            crate::api::handlers::aggregated_schemas::OpenApiExportResponse,
            crate::api::handlers::aggregated_schemas::OpenApiInfo
        )
    ),
    tags(
        (name = "auth", description = "Authentication and session management"),
        (name = "bootstrap", description = "Bootstrap initialization for first-time setup"),
        (name = "clusters", description = "Operations for managing Envoy clusters"),
        (name = "route-configs", description = "Operations for managing Envoy route configurations"),
        (name = "listeners", description = "Operations for managing Envoy listeners"),
        (name = "tokens", description = "Personal access token management APIs"),
        (name = "teams", description = "Team management and bootstrap configuration"),
        (name = "admin", description = "Administrative operations for teams and system management"),
        (name = "users", description = "User management operations (admin only)"),
        (name = "scopes", description = "Scope discovery and management"),
        (name = "openapi-import", description = "Import OpenAPI specifications to create routes and clusters"),
        (name = "audit", description = "Audit log queries (admin only)"),
        (name = "reports", description = "Platform visibility and reporting endpoints"),
        (name = "learning-sessions", description = "API schema learning and traffic observation"),
        (name = "aggregated-schemas", description = "Learned API schemas and catalog management")
    ),
    security(
        ("bearerAuth" = [])
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};

        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

pub fn docs_router() -> Router {
    SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::openapi::{schema::Schema, RefOr};

    #[test]
    fn openapi_includes_cluster_contract() {
        let openapi = ApiDoc::openapi();

        // Validate schema requirements.
        let schemas = openapi.components.as_ref().expect("components").schemas.clone();

        let request_schema = schemas.get("CreateClusterBody").expect("CreateClusterBody schema");
        let request_object = match request_schema {
            RefOr::T(Schema::Object(obj)) => obj,
            RefOr::T(_) => panic!("expected object schema"),
            RefOr::Ref(_) => panic!("expected inline schema, found ref"),
        };

        let required = request_object.required.clone();
        assert!(required.contains(&"name".to_string()));
        assert!(required.contains(&"endpoints".to_string()));
        assert!(!required.contains(&"serviceName".to_string()));

        // Ensure clusters endpoint is documented.
        assert!(openapi.paths.paths.contains_key("/api/v1/clusters"));
        assert!(openapi.paths.paths.contains_key("/api/v1/clusters/{name}"));
        assert!(openapi.paths.paths.contains_key("/api/v1/route-configs"));
        assert!(openapi.paths.paths.contains_key("/api/v1/route-configs/{name}"));
        assert!(openapi.paths.paths.contains_key("/api/v1/tokens"));
    }

    #[test]
    fn openapi_includes_all_endpoints() {
        let openapi = ApiDoc::openapi();
        let paths = &openapi.paths.paths;

        // Bootstrap endpoints (2)
        assert!(
            paths.contains_key("/api/v1/bootstrap/initialize"),
            "Missing POST /api/v1/bootstrap/initialize"
        );
        assert!(
            paths.contains_key("/api/v1/bootstrap/status"),
            "Missing GET /api/v1/bootstrap/status"
        );

        // Auth/Session endpoints (5)
        assert!(paths.contains_key("/api/v1/auth/sessions"), "Missing POST /api/v1/auth/sessions");
        assert!(
            paths.contains_key("/api/v1/auth/sessions/me"),
            "Missing GET /api/v1/auth/sessions/me"
        );
        assert!(
            paths.contains_key("/api/v1/auth/sessions/logout"),
            "Missing POST /api/v1/auth/sessions/logout"
        );
        assert!(paths.contains_key("/api/v1/auth/login"), "Missing POST /api/v1/auth/login");
        assert!(
            paths.contains_key("/api/v1/auth/change-password"),
            "Missing POST /api/v1/auth/change-password"
        );

        // Token endpoints (6)
        assert!(paths.contains_key("/api/v1/tokens"), "Missing GET/POST /api/v1/tokens");
        assert!(
            paths.contains_key("/api/v1/tokens/{id}"),
            "Missing GET/PATCH/DELETE /api/v1/tokens/{{id}}"
        );
        assert!(
            paths.contains_key("/api/v1/tokens/{id}/rotate"),
            "Missing POST /api/v1/tokens/{{id}}/rotate"
        );

        // Cluster endpoints (5)
        assert!(paths.contains_key("/api/v1/clusters"), "Missing GET/POST /api/v1/clusters");
        assert!(
            paths.contains_key("/api/v1/clusters/{name}"),
            "Missing GET/PUT/DELETE /api/v1/clusters/{{name}}"
        );

        // Route config endpoints (5)
        assert!(
            paths.contains_key("/api/v1/route-configs"),
            "Missing GET/POST /api/v1/route-configs"
        );
        assert!(
            paths.contains_key("/api/v1/route-configs/{name}"),
            "Missing GET/PUT/DELETE /api/v1/route-configs/{{name}}"
        );

        // Listener endpoints (5)
        assert!(paths.contains_key("/api/v1/listeners"), "Missing GET/POST /api/v1/listeners");
        assert!(
            paths.contains_key("/api/v1/listeners/{name}"),
            "Missing GET/PUT/DELETE /api/v1/listeners/{{name}}"
        );

        // Team endpoints (7)
        assert!(paths.contains_key("/api/v1/teams"), "Missing GET /api/v1/teams");
        assert!(
            paths.contains_key("/api/v1/teams/{team}/bootstrap"),
            "Missing GET /api/v1/teams/{{team}}/bootstrap"
        );
        assert!(paths.contains_key("/api/v1/admin/teams"), "Missing GET/POST /api/v1/admin/teams");
        assert!(
            paths.contains_key("/api/v1/admin/teams/{id}"),
            "Missing GET/PUT/DELETE /api/v1/admin/teams/{{id}}"
        );

        // User management endpoints (8)
        assert!(paths.contains_key("/api/v1/users"), "Missing GET/POST /api/v1/users");
        assert!(
            paths.contains_key("/api/v1/users/{id}"),
            "Missing GET/PUT/DELETE /api/v1/users/{{id}}"
        );
        assert!(
            paths.contains_key("/api/v1/users/{id}/teams"),
            "Missing GET/POST /api/v1/users/{{id}}/teams"
        );
        assert!(
            paths.contains_key("/api/v1/users/{id}/teams/{team}"),
            "Missing DELETE /api/v1/users/{{id}}/teams/{{team}}"
        );

        // Scope endpoints (2)
        assert!(paths.contains_key("/api/v1/scopes"), "Missing GET /api/v1/scopes");
        assert!(paths.contains_key("/api/v1/admin/scopes"), "Missing GET /api/v1/admin/scopes");

        // OpenAPI import endpoints (4)
        assert!(
            paths.contains_key("/api/v1/openapi/import"),
            "Missing POST /api/v1/openapi/import"
        );
        assert!(
            paths.contains_key("/api/v1/openapi/imports"),
            "Missing GET /api/v1/openapi/imports"
        );
        assert!(
            paths.contains_key("/api/v1/openapi/imports/{id}"),
            "Missing GET/DELETE /api/v1/openapi/imports/{{id}}"
        );

        // Audit log endpoints (1)
        assert!(paths.contains_key("/api/v1/audit-logs"), "Missing GET /api/v1/audit-logs");

        // Learning session endpoints (4)
        assert!(
            paths.contains_key("/api/v1/learning-sessions"),
            "Missing GET/POST /api/v1/learning-sessions"
        );
        assert!(
            paths.contains_key("/api/v1/learning-sessions/{id}"),
            "Missing GET/DELETE /api/v1/learning-sessions/{{id}}"
        );

        // Aggregated schema endpoints (4)
        assert!(
            paths.contains_key("/api/v1/aggregated-schemas"),
            "Missing GET /api/v1/aggregated-schemas"
        );
        assert!(
            paths.contains_key("/api/v1/aggregated-schemas/{id}"),
            "Missing GET /api/v1/aggregated-schemas/{{id}}"
        );
        assert!(
            paths.contains_key("/api/v1/aggregated-schemas/{id}/compare"),
            "Missing GET /api/v1/aggregated-schemas/{{id}}/compare"
        );
        assert!(
            paths.contains_key("/api/v1/aggregated-schemas/{id}/export"),
            "Missing GET /api/v1/aggregated-schemas/{{id}}/export"
        );

        // Reporting endpoints (1)
        assert!(
            paths.contains_key("/api/v1/reports/route-flows"),
            "Missing GET /api/v1/reports/route-flows"
        );
    }

    #[test]
    fn openapi_includes_required_schemas() {
        let openapi = ApiDoc::openapi();
        let schemas = &openapi.components.as_ref().expect("components").schemas;

        // Bootstrap schemas
        assert!(
            schemas.contains_key("BootstrapInitializeRequest"),
            "Missing BootstrapInitializeRequest schema"
        );
        assert!(
            schemas.contains_key("BootstrapInitializeResponse"),
            "Missing BootstrapInitializeResponse schema"
        );
        assert!(
            schemas.contains_key("BootstrapStatusResponse"),
            "Missing BootstrapStatusResponse schema"
        );

        // Cluster schemas
        assert!(schemas.contains_key("CreateClusterBody"), "Missing CreateClusterBody schema");
        assert!(schemas.contains_key("ClusterResponse"), "Missing ClusterResponse schema");
        assert!(schemas.contains_key("EndpointRequest"), "Missing EndpointRequest schema");
        assert!(schemas.contains_key("HealthCheckRequest"), "Missing HealthCheckRequest schema");
        assert!(
            schemas.contains_key("CircuitBreakersRequest"),
            "Missing CircuitBreakersRequest schema"
        );
        assert!(
            schemas.contains_key("CircuitBreakerThresholdsRequest"),
            "Missing CircuitBreakerThresholdsRequest schema"
        );
        assert!(
            schemas.contains_key("OutlierDetectionRequest"),
            "Missing OutlierDetectionRequest schema"
        );

        // XDS schemas
        assert!(schemas.contains_key("ClusterSpec"), "Missing ClusterSpec schema");
        assert!(schemas.contains_key("EndpointSpec"), "Missing EndpointSpec schema");
        assert!(schemas.contains_key("HealthCheckSpec"), "Missing HealthCheckSpec schema");
        assert!(schemas.contains_key("CircuitBreakersSpec"), "Missing CircuitBreakersSpec schema");
        assert!(
            schemas.contains_key("CircuitBreakerThresholdsSpec"),
            "Missing CircuitBreakerThresholdsSpec schema"
        );
        assert!(
            schemas.contains_key("OutlierDetectionSpec"),
            "Missing OutlierDetectionSpec schema"
        );

        // Token schemas
        assert!(schemas.contains_key("CreateTokenBody"), "Missing CreateTokenBody schema");
        assert!(schemas.contains_key("UpdateTokenBody"), "Missing UpdateTokenBody schema");
        assert!(schemas.contains_key("PersonalAccessToken"), "Missing PersonalAccessToken schema");
        assert!(schemas.contains_key("TokenSecretResponse"), "Missing TokenSecretResponse schema");

        // Session schemas
        assert!(schemas.contains_key("CreateSessionBody"), "Missing CreateSessionBody schema");
        assert!(
            schemas.contains_key("CreateSessionResponseBody"),
            "Missing CreateSessionResponseBody schema"
        );
        assert!(schemas.contains_key("SessionInfoResponse"), "Missing SessionInfoResponse schema");
        assert!(schemas.contains_key("LoginBody"), "Missing LoginBody schema");
        assert!(schemas.contains_key("LoginResponseBody"), "Missing LoginResponseBody schema");
        assert!(schemas.contains_key("ChangePasswordBody"), "Missing ChangePasswordBody schema");

        // Route config schemas
        assert!(
            schemas.contains_key("RouteConfigDefinition"),
            "Missing RouteConfigDefinition schema"
        );
        assert!(
            schemas.contains_key("VirtualHostDefinition"),
            "Missing VirtualHostDefinition schema"
        );
        assert!(schemas.contains_key("RouteRuleDefinition"), "Missing RouteRuleDefinition schema");
        assert!(
            schemas.contains_key("RouteMatchDefinition"),
            "Missing RouteMatchDefinition schema"
        );
        assert!(schemas.contains_key("PathMatchDefinition"), "Missing PathMatchDefinition schema");
        assert!(
            schemas.contains_key("RouteActionDefinition"),
            "Missing RouteActionDefinition schema"
        );
        assert!(
            schemas.contains_key("WeightedClusterDefinition"),
            "Missing WeightedClusterDefinition schema"
        );
        assert!(schemas.contains_key("RouteConfigResponse"), "Missing RouteConfigResponse schema");

        // Listener schemas
        assert!(schemas.contains_key("ListenerResponse"), "Missing ListenerResponse schema");
        assert!(schemas.contains_key("CreateListenerBody"), "Missing CreateListenerBody schema");
        assert!(schemas.contains_key("UpdateListenerBody"), "Missing UpdateListenerBody schema");

        // Team schemas
        assert!(schemas.contains_key("BootstrapQuery"), "Missing BootstrapQuery schema");
        assert!(schemas.contains_key("ListTeamsResponse"), "Missing ListTeamsResponse schema");
        assert!(
            schemas.contains_key("AdminListTeamsResponse"),
            "Missing AdminListTeamsResponse schema"
        );
        assert!(schemas.contains_key("Team"), "Missing Team schema");
        assert!(schemas.contains_key("CreateTeamRequest"), "Missing CreateTeamRequest schema");
        assert!(schemas.contains_key("UpdateTeamRequest"), "Missing UpdateTeamRequest schema");

        // User management schemas
        assert!(schemas.contains_key("ListUsersResponse"), "Missing ListUsersResponse schema");
        assert!(schemas.contains_key("CreateUserRequest"), "Missing CreateUserRequest schema");
        assert!(schemas.contains_key("UpdateUserRequest"), "Missing UpdateUserRequest schema");
        assert!(schemas.contains_key("UserResponse"), "Missing UserResponse schema");
        assert!(
            schemas.contains_key("UserWithTeamsResponse"),
            "Missing UserWithTeamsResponse schema"
        );
        assert!(
            schemas.contains_key("CreateTeamMembershipRequest"),
            "Missing CreateTeamMembershipRequest schema"
        );
        assert!(schemas.contains_key("UserTeamMembership"), "Missing UserTeamMembership schema");

        // Scope schemas
        assert!(schemas.contains_key("ListScopesResponse"), "Missing ListScopesResponse schema");
        assert!(schemas.contains_key("ScopeDefinition"), "Missing ScopeDefinition schema");

        // OpenAPI import schemas
        assert!(schemas.contains_key("ImportResponse"), "Missing ImportResponse schema");
        assert!(schemas.contains_key("ListImportsResponse"), "Missing ListImportsResponse schema");
        assert!(schemas.contains_key("ImportSummary"), "Missing ImportSummary schema");
        assert!(
            schemas.contains_key("ImportDetailsResponse"),
            "Missing ImportDetailsResponse schema"
        );
        assert!(schemas.contains_key("OpenApiSpecBody"), "Missing OpenApiSpecBody schema");

        // Audit log schemas
        assert!(
            schemas.contains_key("ListAuditLogsResponse"),
            "Missing ListAuditLogsResponse schema"
        );
        assert!(schemas.contains_key("AuditLogEntry"), "Missing AuditLogEntry schema");

        // HTTP filter schemas
        assert!(schemas.contains_key("CorsPolicyConfig"), "Missing CorsPolicyConfig schema");
        assert!(
            schemas.contains_key("CustomResponseConfig"),
            "Missing CustomResponseConfig schema"
        );
        assert!(
            schemas.contains_key("HeaderMutationConfig"),
            "Missing HeaderMutationConfig schema"
        );
        assert!(schemas.contains_key("HealthCheckConfig"), "Missing HealthCheckConfig schema");
        assert!(
            schemas.contains_key("LocalRateLimitConfig"),
            "Missing LocalRateLimitConfig schema"
        );
        assert!(schemas.contains_key("RateLimitConfig"), "Missing RateLimitConfig schema");

        // Learning session schemas
        assert!(
            schemas.contains_key("CreateLearningSessionBody"),
            "Missing CreateLearningSessionBody schema"
        );
        assert!(
            schemas.contains_key("LearningSessionResponse"),
            "Missing LearningSessionResponse schema"
        );
        assert!(
            schemas.contains_key("ListLearningSessionsQuery"),
            "Missing ListLearningSessionsQuery schema"
        );

        // Aggregated schema schemas
        assert!(
            schemas.contains_key("AggregatedSchemaResponse"),
            "Missing AggregatedSchemaResponse schema"
        );
        assert!(
            schemas.contains_key("ListAggregatedSchemasQuery"),
            "Missing ListAggregatedSchemasQuery schema"
        );
        assert!(schemas.contains_key("CompareSchemaQuery"), "Missing CompareSchemaQuery schema");
        assert!(
            schemas.contains_key("SchemaComparisonResponse"),
            "Missing SchemaComparisonResponse schema"
        );
        assert!(schemas.contains_key("SchemaDifferences"), "Missing SchemaDifferences schema");
        assert!(schemas.contains_key("ExportSchemaQuery"), "Missing ExportSchemaQuery schema");
        assert!(
            schemas.contains_key("OpenApiExportResponse"),
            "Missing OpenApiExportResponse schema"
        );
        assert!(schemas.contains_key("OpenApiInfo"), "Missing OpenApiInfo schema");
    }

    #[test]
    fn openapi_includes_required_tags() {
        let openapi = ApiDoc::openapi();
        let tags = openapi.tags.as_ref().expect("tags should be present");

        let tag_names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();

        assert!(tag_names.contains(&"auth"), "Missing 'auth' tag");
        assert!(tag_names.contains(&"bootstrap"), "Missing 'bootstrap' tag");
        assert!(tag_names.contains(&"clusters"), "Missing 'clusters' tag");
        assert!(tag_names.contains(&"route-configs"), "Missing 'route-configs' tag");
        assert!(tag_names.contains(&"listeners"), "Missing 'listeners' tag");
        assert!(tag_names.contains(&"tokens"), "Missing 'tokens' tag");
        assert!(tag_names.contains(&"teams"), "Missing 'teams' tag");
        assert!(tag_names.contains(&"admin"), "Missing 'admin' tag");
        assert!(tag_names.contains(&"users"), "Missing 'users' tag");
        assert!(tag_names.contains(&"scopes"), "Missing 'scopes' tag");
        assert!(tag_names.contains(&"openapi-import"), "Missing 'openapi-import' tag");
        assert!(tag_names.contains(&"audit"), "Missing 'audit' tag");
        assert!(tag_names.contains(&"reports"), "Missing 'reports' tag");
        assert!(tag_names.contains(&"learning-sessions"), "Missing 'learning-sessions' tag");
        assert!(tag_names.contains(&"aggregated-schemas"), "Missing 'aggregated-schemas' tag");
    }

    #[test]
    fn openapi_has_security_scheme() {
        let openapi = ApiDoc::openapi();
        let components = openapi.components.as_ref().expect("components should be present");
        let security_schemes = &components.security_schemes;

        assert!(security_schemes.contains_key("bearerAuth"), "Missing bearerAuth security scheme");
    }
}
