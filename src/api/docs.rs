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
        crate::api::handlers::bootstrap::bootstrap_initialize_handler,
        crate::api::handlers::auth::create_token_handler,
        crate::api::handlers::auth::list_tokens_handler,
        crate::api::handlers::auth::get_token_handler,
        crate::api::handlers::auth::update_token_handler,
        crate::api::handlers::auth::revoke_token_handler,
        crate::api::handlers::auth::rotate_token_handler,
        crate::api::handlers::auth::create_session_handler,
        crate::api::handlers::auth::get_session_info_handler,
        crate::api::handlers::auth::logout_handler,
        crate::api::handlers::clusters::create_cluster_handler,
        crate::api::handlers::clusters::list_clusters_handler,
        crate::api::handlers::clusters::get_cluster_handler,
        crate::api::handlers::clusters::update_cluster_handler,
        crate::api::handlers::clusters::delete_cluster_handler,
        crate::api::handlers::routes::create_route_handler,
        crate::api::handlers::routes::list_routes_handler,
        crate::api::handlers::routes::get_route_handler,
        crate::api::handlers::routes::update_route_handler,
        crate::api::handlers::routes::delete_route_handler,
        crate::api::handlers::listeners::create_listener_handler,
        crate::api::handlers::listeners::list_listeners_handler,
        crate::api::handlers::listeners::get_listener_handler,
        crate::api::handlers::listeners::update_listener_handler,
        crate::api::handlers::listeners::delete_listener_handler,
        crate::api::handlers::api_definitions::import_openapi_handler,
        crate::api::handlers::api_definitions::append_route_handler,
        crate::api::handlers::api_definitions::list_api_definitions_handler,
        crate::api::handlers::api_definitions::get_api_definition_handler,
        crate::api::handlers::api_definitions::update_api_definition_handler,
        crate::api::handlers::teams::get_team_bootstrap_handler,
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
            crate::api::handlers::bootstrap::BootstrapInitializeRequest,
            crate::api::handlers::bootstrap::BootstrapInitializeResponse,
            CreateClusterBody,
            EndpointRequest,
            HealthCheckRequest,
            CircuitBreakersRequest,
            CircuitBreakerThresholdsRequest,
            OutlierDetectionRequest,
            ClusterResponse,
            CreateTokenBody,
            UpdateTokenBody,
            PersonalAccessToken,
            TokenSecretResponse,
            crate::api::handlers::auth::CreateSessionBody,
            crate::api::handlers::auth::CreateSessionResponseBody,
            crate::api::handlers::auth::SessionInfoResponse,
            ClusterSpec,
            EndpointSpec,
            CircuitBreakersSpec,
            CircuitBreakerThresholdsSpec,
            HealthCheckSpec,
            OutlierDetectionSpec,
            crate::api::handlers::routes::RouteDefinition,
            crate::api::handlers::routes::VirtualHostDefinition,
            crate::api::handlers::routes::RouteRuleDefinition,
            crate::api::handlers::routes::RouteMatchDefinition,
            crate::api::handlers::routes::PathMatchDefinition,
            crate::api::handlers::routes::RouteActionDefinition,
            crate::api::handlers::routes::WeightedClusterDefinition,
            crate::api::handlers::routes::RouteResponse,
            crate::api::handlers::listeners::ListenerResponse,
            crate::api::handlers::listeners::CreateListenerBody,
            crate::api::handlers::listeners::UpdateListenerBody,
            crate::api::handlers::api_definitions::OpenApiSpecBody,
            crate::api::handlers::api_definitions::ImportOpenApiQuery,
            crate::validation::requests::api_definition::AppendRouteBody,
            crate::validation::requests::api_definition::RouteBody,
            crate::validation::requests::api_definition::RouteMatchBody,
            crate::validation::requests::api_definition::RouteClusterBody,
            crate::validation::requests::api_definition::RouteRewriteBody,
            crate::api::handlers::api_definitions::CreateApiDefinitionResponse,
            crate::api::handlers::api_definitions::AppendRouteResponse,
            crate::api::handlers::api_definitions::ApiDefinitionSummary,
            crate::api::handlers::api_definitions::ListDefinitionsQuery,
            crate::api::handlers::teams::BootstrapQuery,
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
        (name = "listeners", description = "Operations for managing Envoy listeners"),
        (name = "tokens", description = "Personal access token management APIs"),
        (name = "platform-api", description = "Platform API Abstraction endpoints"),
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
        assert!(openapi.paths.paths.contains_key("/api/v1/routes"));
        assert!(openapi.paths.paths.contains_key("/api/v1/routes/{name}"));
        assert!(openapi.paths.paths.contains_key("/api/v1/tokens"));
    }

    #[test]
    fn openapi_includes_all_endpoints() {
        let openapi = ApiDoc::openapi();
        let paths = &openapi.paths.paths;

        // Bootstrap endpoint (1)
        assert!(
            paths.contains_key("/api/v1/bootstrap/initialize"),
            "Missing POST /api/v1/bootstrap/initialize"
        );

        // Auth/Session endpoints (3)
        assert!(paths.contains_key("/api/v1/auth/sessions"), "Missing POST /api/v1/auth/sessions");
        assert!(
            paths.contains_key("/api/v1/auth/sessions/me"),
            "Missing GET /api/v1/auth/sessions/me"
        );
        assert!(
            paths.contains_key("/api/v1/auth/sessions/logout"),
            "Missing POST /api/v1/auth/sessions/logout"
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

        // Route endpoints (5)
        assert!(paths.contains_key("/api/v1/routes"), "Missing GET/POST /api/v1/routes");
        assert!(
            paths.contains_key("/api/v1/routes/{name}"),
            "Missing GET/PUT/DELETE /api/v1/routes/{{name}}"
        );

        // Listener endpoints (5)
        assert!(paths.contains_key("/api/v1/listeners"), "Missing GET/POST /api/v1/listeners");
        assert!(
            paths.contains_key("/api/v1/listeners/{name}"),
            "Missing GET/PUT/DELETE /api/v1/listeners/{{name}}"
        );

        // API Definition endpoints (5)
        assert!(
            paths.contains_key("/api/v1/api-definitions"),
            "Missing GET /api/v1/api-definitions"
        );
        assert!(
            paths.contains_key("/api/v1/api-definitions/from-openapi"),
            "Missing POST /api/v1/api-definitions/from-openapi"
        );
        assert!(
            paths.contains_key("/api/v1/api-definitions/{id}"),
            "Missing GET /api/v1/api-definitions/{{id}}"
        );
        assert!(
            paths.contains_key("/api/v1/api-definitions/{id}/routes"),
            "Missing POST /api/v1/api-definitions/{{id}}/routes"
        );

        // Team endpoints (1)
        assert!(
            paths.contains_key("/api/v1/teams/{team}/bootstrap"),
            "Missing GET /api/v1/teams/{{team}}/bootstrap"
        );

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

        // Route schemas
        assert!(schemas.contains_key("RouteDefinition"), "Missing RouteDefinition schema");
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
        assert!(schemas.contains_key("RouteResponse"), "Missing RouteResponse schema");

        // Listener schemas
        assert!(schemas.contains_key("ListenerResponse"), "Missing ListenerResponse schema");
        assert!(schemas.contains_key("CreateListenerBody"), "Missing CreateListenerBody schema");
        assert!(schemas.contains_key("UpdateListenerBody"), "Missing UpdateListenerBody schema");

        // Platform API schemas
        assert!(schemas.contains_key("AppendRouteBody"), "Missing AppendRouteBody schema");
        assert!(schemas.contains_key("RouteBody"), "Missing RouteBody schema");
        assert!(schemas.contains_key("RouteMatchBody"), "Missing RouteMatchBody schema");
        assert!(schemas.contains_key("RouteClusterBody"), "Missing RouteClusterBody schema");
        assert!(schemas.contains_key("RouteRewriteBody"), "Missing RouteRewriteBody schema");
        assert!(
            schemas.contains_key("CreateApiDefinitionResponse"),
            "Missing CreateApiDefinitionResponse schema"
        );
        assert!(schemas.contains_key("AppendRouteResponse"), "Missing AppendRouteResponse schema");
        assert!(
            schemas.contains_key("ApiDefinitionSummary"),
            "Missing ApiDefinitionSummary schema"
        );
        assert!(
            schemas.contains_key("ListDefinitionsQuery"),
            "Missing ListDefinitionsQuery schema"
        );
        assert!(schemas.contains_key("BootstrapQuery"), "Missing BootstrapQuery schema");
        assert!(schemas.contains_key("OpenApiSpecBody"), "Missing OpenApiSpecBody schema");
        assert!(schemas.contains_key("ImportOpenApiQuery"), "Missing ImportOpenApiQuery schema");

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
        assert!(tag_names.contains(&"listeners"), "Missing 'listeners' tag");
        assert!(tag_names.contains(&"tokens"), "Missing 'tokens' tag");
        assert!(tag_names.contains(&"platform-api"), "Missing 'platform-api' tag");
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

    #[test]
    fn openapi_platform_api_schemas_have_examples() {
        let openapi = ApiDoc::openapi();
        let schemas = &openapi.components.as_ref().expect("components").schemas;

        // Verify AppendRouteBody has examples
        let append_route_schema =
            schemas.get("AppendRouteBody").expect("AppendRouteBody schema should exist");

        if let RefOr::T(Schema::Object(obj)) = append_route_schema {
            assert!(obj.example.is_some(), "AppendRouteBody should have an example");
        } else {
            panic!("AppendRouteBody should be an object schema");
        }
    }
}
