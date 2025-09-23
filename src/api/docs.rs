use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::api::handlers::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};
use crate::xds::{
    CircuitBreakerThresholdsSpec, CircuitBreakersSpec, ClusterSpec, EndpointSpec, HealthCheckSpec,
    OutlierDetectionSpec,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::api::handlers::create_cluster_handler,
        crate::api::handlers::list_clusters_handler,
        crate::api::handlers::get_cluster_handler,
        crate::api::handlers::update_cluster_handler,
        crate::api::handlers::delete_cluster_handler
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
            ClusterSpec,
            EndpointSpec,
            CircuitBreakersSpec,
            CircuitBreakerThresholdsSpec,
            HealthCheckSpec,
            OutlierDetectionSpec
        )
    ),
    tags(
        (name = "clusters", description = "Operations for managing Envoy clusters")
    )
)]
pub struct ApiDoc;

pub fn docs_router() -> Router {
    SwaggerUi::new("/swagger-ui")
        .url("/api-docs/openapi.json", ApiDoc::openapi())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::openapi::{schema::Schema, RefOr};

    #[test]
    fn openapi_includes_cluster_contract() {
        let openapi = ApiDoc::openapi();

        // Validate schema requirements.
        let schemas = openapi
            .components
            .as_ref()
            .expect("components")
            .schemas
            .clone();

        let request_schema = schemas
            .get("CreateClusterBody")
            .expect("CreateClusterBody schema");
        let request_object = match request_schema {
            RefOr::T(Schema::Object(obj)) => obj,
            RefOr::T(_) => panic!("expected object schema"),
            RefOr::Ref(_) => panic!("expected inline schema, found ref"),
        };

        let required: Vec<_> = request_object.required.iter().cloned().collect();
        assert!(required.contains(&"name".to_string()));
        assert!(required.contains(&"endpoints".to_string()));
        assert!(!required.contains(&"serviceName".to_string()));

        // Ensure clusters endpoint is documented.
        assert!(openapi.paths.paths.contains_key("/api/v1/clusters"));
        assert!(openapi.paths.paths.contains_key("/api/v1/clusters/{name}"));
    }
}
