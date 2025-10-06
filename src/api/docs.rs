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
        crate::api::handlers::auth::create_token_handler,
        crate::api::handlers::auth::list_tokens_handler,
        crate::api::handlers::auth::get_token_handler,
        crate::api::handlers::auth::update_token_handler,
        crate::api::handlers::auth::revoke_token_handler,
        crate::api::handlers::auth::rotate_token_handler,
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
        crate::api::handlers::api_definitions::create_api_definition_handler,
        crate::api::handlers::api_definitions::import_openapi_handler,
        crate::api::handlers::api_definitions::append_route_handler,
        crate::api::handlers::api_definitions::list_api_definitions_handler,
        crate::api::handlers::api_definitions::get_api_definition_handler,
        crate::api::handlers::api_definitions::get_bootstrap_handler
    ),
    components(
        schemas(
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
            crate::validation::requests::api_definition::CreateApiDefinitionBody,
            crate::validation::requests::api_definition::AppendRouteBody,
            crate::validation::requests::api_definition::RouteBody,
            crate::validation::requests::api_definition::RouteMatchBody,
            crate::validation::requests::api_definition::RouteClusterBody,
            crate::validation::requests::api_definition::RouteRewriteBody,
            crate::validation::requests::api_definition::IsolationListenerBody,
            crate::api::handlers::api_definitions::CreateApiDefinitionResponse,
            crate::api::handlers::api_definitions::AppendRouteResponse,
            crate::api::handlers::api_definitions::ApiDefinitionSummary,
            crate::api::handlers::api_definitions::ListDefinitionsQuery,
            crate::api::handlers::api_definitions::BootstrapQuery,
            // Commonly used HTTP filter configurations
            CorsPolicyConfig,
            CustomResponseConfig,
            HeaderMutationConfig,
            HealthCheckConfig,
            LocalRateLimitConfig,
            RateLimitConfig
        )
    ),
    tags(
        (name = "clusters", description = "Operations for managing Envoy clusters"),
        (name = "listeners", description = "Operations for managing Envoy listeners"),
        (name = "tokens", description = "Personal access token management APIs"),
        (name = "platform-api", description = "Platform API Abstraction endpoints")
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

        // API Definition endpoints (6)
        assert!(
            paths.contains_key("/api/v1/api-definitions"),
            "Missing GET/POST /api/v1/api-definitions"
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
            paths.contains_key("/api/v1/api-definitions/{id}/bootstrap"),
            "Missing GET /api/v1/api-definitions/{{id}}/bootstrap"
        );
        assert!(
            paths.contains_key("/api/v1/api-definitions/{id}/routes"),
            "Missing POST /api/v1/api-definitions/{{id}}/routes"
        );
    }

    #[test]
    fn openapi_includes_required_schemas() {
        let openapi = ApiDoc::openapi();
        let schemas = &openapi.components.as_ref().expect("components").schemas;

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
        assert!(
            schemas.contains_key("CreateApiDefinitionBody"),
            "Missing CreateApiDefinitionBody schema"
        );
        assert!(schemas.contains_key("AppendRouteBody"), "Missing AppendRouteBody schema");
        assert!(schemas.contains_key("RouteBody"), "Missing RouteBody schema");
        assert!(schemas.contains_key("RouteMatchBody"), "Missing RouteMatchBody schema");
        assert!(schemas.contains_key("RouteClusterBody"), "Missing RouteClusterBody schema");
        assert!(schemas.contains_key("RouteRewriteBody"), "Missing RouteRewriteBody schema");
        assert!(
            schemas.contains_key("IsolationListenerBody"),
            "Missing IsolationListenerBody schema"
        );
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
    }

    #[test]
    fn openapi_includes_required_tags() {
        let openapi = ApiDoc::openapi();
        let tags = openapi.tags.as_ref().expect("tags should be present");

        let tag_names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();

        assert!(tag_names.contains(&"clusters"), "Missing 'clusters' tag");
        assert!(tag_names.contains(&"listeners"), "Missing 'listeners' tag");
        assert!(tag_names.contains(&"tokens"), "Missing 'tokens' tag");
        assert!(tag_names.contains(&"platform-api"), "Missing 'platform-api' tag");
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

        // Verify CreateApiDefinitionBody has examples
        let create_def_schema = schemas
            .get("CreateApiDefinitionBody")
            .expect("CreateApiDefinitionBody schema should exist");

        if let RefOr::T(Schema::Object(obj)) = create_def_schema {
            assert!(obj.example.is_some(), "CreateApiDefinitionBody should have an example");
        } else {
            panic!("CreateApiDefinitionBody should be an object schema");
        }

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
