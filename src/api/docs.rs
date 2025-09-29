use axum::Router;
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

#[allow(unused_imports)]
use crate::api::auth_handlers::{CreateTokenBody, UpdateTokenBody};
#[allow(unused_imports)]
use crate::api::handlers::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};
#[allow(unused_imports)]
use crate::auth::{models::PersonalAccessToken, token_service::TokenSecretResponse};
#[allow(unused_imports)]
use crate::xds::{
    CircuitBreakerThresholdsSpec, CircuitBreakersSpec, ClusterSpec, EndpointSpec, HealthCheckSpec,
    OutlierDetectionSpec,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::api::auth_handlers::create_token_handler,
        crate::api::auth_handlers::list_tokens_handler,
        crate::api::auth_handlers::get_token_handler,
        crate::api::auth_handlers::update_token_handler,
        crate::api::auth_handlers::revoke_token_handler,
        crate::api::auth_handlers::rotate_token_handler,
        crate::api::handlers::create_cluster_handler,
        crate::api::handlers::list_clusters_handler,
        crate::api::handlers::get_cluster_handler,
        crate::api::handlers::update_cluster_handler,
        crate::api::handlers::delete_cluster_handler,
        crate::api::route_handlers::create_route_handler,
        crate::api::route_handlers::list_routes_handler,
        crate::api::route_handlers::get_route_handler,
        crate::api::route_handlers::update_route_handler,
        crate::api::route_handlers::delete_route_handler,
        crate::api::listener_handlers::create_listener_handler,
        crate::api::listener_handlers::list_listeners_handler,
        crate::api::listener_handlers::get_listener_handler,
        crate::api::listener_handlers::update_listener_handler,
        crate::api::listener_handlers::delete_listener_handler,
        crate::api::gateway_handlers::create_gateway_from_openapi_handler,
        crate::api::platform_api_handlers::create_api_definition_handler,
        crate::api::platform_api_handlers::append_route_handler,
        crate::api::platform_api_handlers::list_api_definitions_handler,
        crate::api::platform_api_handlers::get_api_definition_handler
        ,crate::api::platform_api_handlers::get_bootstrap_handler
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
            crate::api::route_handlers::RouteDefinition,
            crate::api::route_handlers::VirtualHostDefinition,
            crate::api::route_handlers::RouteRuleDefinition,
            crate::api::route_handlers::RouteMatchDefinition,
            crate::api::route_handlers::PathMatchDefinition,
            crate::api::route_handlers::RouteActionDefinition,
            crate::api::route_handlers::WeightedClusterDefinition,
            crate::api::route_handlers::RouteResponse,
            crate::api::listener_handlers::ListenerResponse,
            crate::api::listener_handlers::CreateListenerBody,
            crate::api::listener_handlers::UpdateListenerBody,
            crate::api::gateway_handlers::GatewayQuery,
            crate::api::gateway_handlers::OpenApiSpecBody,
            crate::openapi::GatewaySummary,
            crate::validation::requests::api_definition::CreateApiDefinitionBody,
            crate::validation::requests::api_definition::AppendRouteBody,
            crate::validation::requests::api_definition::RouteBody,
            crate::validation::requests::api_definition::RouteMatchBody,
            crate::validation::requests::api_definition::RouteClusterBody,
            crate::validation::requests::api_definition::RouteRewriteBody,
            crate::validation::requests::api_definition::IsolationListenerBody,
            crate::api::platform_api_handlers::CreateApiDefinitionResponse,
            crate::api::platform_api_handlers::AppendRouteResponse,
            crate::api::platform_api_handlers::ApiDefinitionSummary,
            crate::api::platform_api_handlers::ListDefinitionsQuery
            ,crate::api::platform_api_handlers::BootstrapQuery
        )
    ),
    tags(
        (name = "clusters", description = "Operations for managing Envoy clusters"),
        (name = "listeners", description = "Operations for managing Envoy listeners"),
        (name = "gateways", description = "Operations for importing gateway configurations from OpenAPI specifications"),
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
