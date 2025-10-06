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
}
