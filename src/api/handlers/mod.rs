//! HTTP request handlers organized by resource type

pub mod aggregated_schemas;
pub mod audit_log;
pub mod auth;
pub mod bootstrap;
pub mod clusters;
pub mod filters;
pub mod health;
pub mod hierarchy;
pub mod learning_sessions;
pub mod listeners;
pub mod openapi_import;
pub mod proxy_certificates;
pub mod reporting;
pub mod route_configs;
pub mod scopes;
pub mod secrets;
pub mod stats;
pub mod teams;
pub mod users;

// Re-export handler functions for backward compatibility
pub use aggregated_schemas::{
    compare_aggregated_schemas_handler, export_aggregated_schema_handler,
    get_aggregated_schema_handler, list_aggregated_schemas_handler,
};
pub use audit_log::list_audit_logs;
pub use auth::{
    change_password_handler, create_session_handler, create_token_handler,
    get_session_info_handler, get_token_handler, list_tokens_handler, login_handler,
    logout_handler, revoke_token_handler, rotate_token_handler, update_token_handler,
};
pub use bootstrap::{bootstrap_initialize_handler, bootstrap_status_handler};
pub use clusters::{
    create_cluster_handler, delete_cluster_handler, get_cluster_handler, list_clusters_handler,
    update_cluster_handler,
};
pub use filters::{
    attach_filter_handler, attach_filter_to_listener_handler, create_filter_handler,
    delete_filter_handler, detach_filter_from_listener_handler, detach_filter_handler,
    get_filter_handler, get_filter_type_handler, list_filter_types_handler, list_filters_handler,
    list_listener_filters_handler, list_route_filters_handler, reload_filter_schemas_handler,
    update_filter_handler,
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
pub use openapi_import::{delete_import_handler, get_import_handler, list_imports_handler};
pub use proxy_certificates::{
    generate_certificate_handler, get_certificate_handler, list_certificates_handler,
    revoke_certificate_handler,
};
pub use reporting::list_route_flows_handler;
pub use route_configs::{
    create_route_config_handler, delete_route_config_handler, get_route_config_handler,
    list_route_configs_handler, update_route_config_handler,
};
pub use scopes::{list_all_scopes_handler, list_scopes_handler, ListScopesResponse};
pub use secrets::{
    create_secret_handler, delete_secret_handler, get_secret_handler, list_secrets_handler,
    update_secret_handler,
};
pub use stats::{
    get_app_handler, get_stats_cluster_handler, get_stats_clusters_handler,
    get_stats_enabled_handler, get_stats_overview_handler, list_apps_handler,
    set_app_status_handler, AppStatusResponse, ClusterStatsResponse, ClustersStatsResponse,
    ListAppsResponse, SetAppStatusRequest, StatsEnabledResponse, StatsOverviewResponse,
};
pub use teams::{
    admin_create_team, admin_delete_team, admin_get_team, admin_list_teams, admin_update_team,
    get_mtls_status_handler, get_team_bootstrap_handler, list_teams_handler,
};
pub use users::{
    add_team_membership, create_user, delete_user, get_user, list_user_teams, list_users,
    remove_team_membership, update_user,
};

// Re-export hierarchy handlers for route hierarchy filter attachment
pub use hierarchy::{
    attach_filter_to_route_rule_handler, attach_filter_to_virtual_host_handler,
    detach_filter_from_route_rule_handler, detach_filter_from_virtual_host_handler,
    list_route_rule_filters_handler, list_route_rules_handler, list_virtual_host_filters_handler,
    list_virtual_hosts_handler,
};

// Re-export DTOs for OpenAPI docs
pub use aggregated_schemas::{
    AggregatedSchemaResponse, CompareSchemaQuery, ExportSchemaQuery, ListAggregatedSchemasQuery,
    OpenApiExportResponse, SchemaComparisonResponse,
};
pub use clusters::{
    CircuitBreakerThresholdsRequest, CircuitBreakersRequest, ClusterResponse, CreateClusterBody,
    EndpointRequest, HealthCheckRequest, OutlierDetectionRequest,
};
pub use filters::{
    AttachFilterRequest, CreateFilterRequest, FilterResponse, FilterTypeFormSection,
    FilterTypeInfo, FilterTypeUiHints, FilterTypesResponse, ListFiltersQuery,
    ListenerFiltersResponse, RouteFiltersResponse, UpdateFilterRequest,
};
pub use learning_sessions::{
    CreateLearningSessionBody, LearningSessionResponse, ListLearningSessionsQuery,
};
pub use proxy_certificates::{
    CertificateMetadata, GenerateCertificateRequest, GenerateCertificateResponse,
    ListCertificatesQuery, ListCertificatesResponse, RevokeCertificateRequest,
};
pub use secrets::{
    CreateSecretRequest, ListSecretsQuery, SecretResponse, TeamPath, TeamSecretPath,
    UpdateSecretRequest,
};
pub use teams::{
    AdminListTeamsQuery, AdminListTeamsResponse, BootstrapQuery, ListTeamsResponse,
    MtlsStatusResponse,
};
pub use users::ListUsersResponse;

// Hierarchy DTOs for route hierarchy filter attachment
pub use hierarchy::{
    AttachFilterRequest as HierarchyAttachFilterRequest, FilterResponse as HierarchyFilterResponse,
    ListRouteRulesResponse, ListVirtualHostsResponse, RouteRuleFiltersResponse, RouteRuleResponse,
    VirtualHostFiltersResponse, VirtualHostResponse,
};
