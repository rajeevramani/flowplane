//! HTTP request handlers organized by resource type

pub mod api_definitions;
pub mod auth;
pub mod clusters;
pub mod gateways;
pub mod listeners;
pub mod routes;

// Re-export handler functions for backward compatibility
pub use api_definitions::{
    append_route_handler, create_api_definition_handler, get_api_definition_handler,
    get_bootstrap_handler, import_openapi_handler, list_api_definitions_handler,
};
pub use auth::{
    create_token_handler, get_token_handler, list_tokens_handler, revoke_token_handler,
    rotate_token_handler, update_token_handler,
};
pub use clusters::{
    create_cluster_handler, delete_cluster_handler, get_cluster_handler, list_clusters_handler,
    update_cluster_handler,
};
pub use gateways::create_gateway_from_openapi_handler;
pub use listeners::{
    create_listener_handler, delete_listener_handler, get_listener_handler, list_listeners_handler,
    update_listener_handler,
};
pub use routes::{
    create_route_handler, delete_route_handler, get_route_handler, list_routes_handler,
    update_route_handler,
};

// Re-export DTOs for OpenAPI docs
pub use clusters::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};
