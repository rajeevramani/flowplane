//! HTTP request handlers organized by resource type

pub mod aggregated_schemas;
pub mod api_definitions;
pub mod auth;
pub mod bootstrap;
pub mod clusters;
pub mod health;
pub mod learning_sessions;
pub mod listeners;
pub mod reporting;
pub mod routes;
pub mod teams;

// Re-export handler functions for backward compatibility
pub use aggregated_schemas::{
    compare_aggregated_schemas_handler, export_aggregated_schema_handler,
    get_aggregated_schema_handler, list_aggregated_schemas_handler,
};
pub use api_definitions::{
    append_route_handler, create_api_definition_handler, get_api_definition_handler,
    import_openapi_handler, list_api_definitions_handler, update_api_definition_handler,
};
pub use auth::{
    create_token_handler, get_token_handler, list_tokens_handler, revoke_token_handler,
    rotate_token_handler, update_token_handler,
};
pub use bootstrap::bootstrap_initialize_handler;
pub use clusters::{
    create_cluster_handler, delete_cluster_handler, get_cluster_handler, list_clusters_handler,
    update_cluster_handler,
};
pub use health::health_handler;
pub use learning_sessions::{
    create_learning_session_handler, delete_learning_session_handler, get_learning_session_handler,
    list_learning_sessions_handler,
};
pub use listeners::{
    create_listener_handler, delete_listener_handler, get_listener_handler, list_listeners_handler,
    update_listener_handler,
};
pub use reporting::list_route_flows_handler;
pub use routes::{
    create_route_handler, delete_route_handler, get_route_handler, list_routes_handler,
    update_route_handler,
};
pub use teams::get_team_bootstrap_handler;

// Re-export DTOs for OpenAPI docs
pub use aggregated_schemas::{
    AggregatedSchemaResponse, CompareSchemaQuery, ExportSchemaQuery, ListAggregatedSchemasQuery,
    OpenApiExportResponse, SchemaComparisonResponse,
};
pub use clusters::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};
pub use learning_sessions::{
    CreateLearningSessionBody, LearningSessionResponse, ListLearningSessionsQuery,
};
pub use teams::BootstrapQuery;
