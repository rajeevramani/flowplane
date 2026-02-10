//! HTTP request handlers organized by resource type

pub mod aggregated_schemas;
pub mod audit_log;
pub mod auth;
pub mod bootstrap;
pub mod clusters;
pub mod custom_wasm_filters;
pub mod dataplanes;
pub mod filters;
pub mod health;
pub mod hierarchy;
pub mod invitations;
pub mod learning_sessions;
pub mod listeners;
pub mod mcp_routes;
pub mod mcp_tools;
pub mod openapi_import;
pub mod openapi_utils;
pub mod organizations;
pub mod proxy_certificates;
pub mod reporting;
pub mod route_configs;
pub mod route_views;
pub mod scopes;
pub mod secrets;
pub mod stats;
pub mod team_access;
pub mod teams;
pub mod users;

// Re-export handler functions for backward compatibility
pub use aggregated_schemas::{
    compare_aggregated_schemas_handler, export_aggregated_schema_handler,
    export_multiple_schemas_handler, get_aggregated_schema_handler,
    list_aggregated_schemas_handler,
};
pub use audit_log::list_audit_logs;
pub use auth::{
    change_password_handler, create_session_handler, create_token_handler,
    get_session_info_handler, get_token_handler, list_tokens_handler, login_handler,
    logout_handler, refresh_session_handler, revoke_token_handler, rotate_token_handler,
    update_token_handler,
};
pub use bootstrap::{bootstrap_initialize_handler, bootstrap_status_handler};
pub use clusters::{
    create_cluster_handler, delete_cluster_handler, get_cluster_handler, list_clusters_handler,
    update_cluster_handler,
};
pub use custom_wasm_filters::{
    create_custom_wasm_filter_handler, delete_custom_wasm_filter_handler,
    download_wasm_binary_handler, get_custom_wasm_filter_handler, list_custom_wasm_filters_handler,
    update_custom_wasm_filter_handler,
};
pub use dataplanes::{
    create_dataplane_handler, delete_dataplane_handler, generate_envoy_config_handler,
    get_dataplane_handler, list_all_dataplanes_handler, list_dataplanes_handler,
    update_dataplane_handler,
};
pub use filters::{
    attach_filter_handler, attach_filter_to_listener_handler, configure_filter_handler,
    create_filter_handler, delete_filter_handler, detach_filter_from_listener_handler,
    detach_filter_handler, get_filter_handler, get_filter_status_handler, get_filter_type_handler,
    install_filter_handler, list_filter_configurations_handler, list_filter_installations_handler,
    list_filter_types_handler, list_filters_handler, list_listener_filters_handler,
    list_route_filters_handler, reload_filter_schemas_handler, remove_filter_configuration_handler,
    uninstall_filter_handler, update_filter_handler,
};
pub use health::health_handler;
pub use invitations::{
    accept_invitation_handler, create_invitation_handler, list_invitations_handler,
    revoke_invitation_handler, validate_invitation_handler,
};
pub use learning_sessions::{
    create_learning_session_handler, delete_learning_session_handler, get_learning_session_handler,
    list_learning_sessions_handler,
};
pub use listeners::{
    create_listener_handler, delete_listener_handler, get_listener_handler, list_listeners_handler,
    update_listener_handler,
};
pub use mcp_routes::{
    bulk_disable_mcp_handler, bulk_enable_mcp_handler, disable_mcp_handler, enable_mcp_handler,
    get_mcp_status_handler, refresh_mcp_schema_handler, BulkMcpDisableRequest,
    BulkMcpDisableResponse, BulkMcpEnableRequest, BulkMcpEnableResponse, EnableMcpRequestBody,
    McpStatusResponse, RefreshSchemaResponse,
};
pub use mcp_tools::{
    apply_learned_schema_handler, check_learned_schema_handler, get_mcp_tool_handler,
    list_mcp_tools_handler, update_mcp_tool_handler,
};
pub use openapi_import::{delete_import_handler, get_import_handler, list_imports_handler};
pub use organizations::{
    admin_add_org_member, admin_create_organization, admin_delete_organization,
    admin_get_organization, admin_list_org_members, admin_list_organizations,
    admin_remove_org_member, admin_update_org_member_role, admin_update_organization,
    create_org_team, get_current_org, list_org_teams,
};
pub use proxy_certificates::{
    generate_certificate_handler, get_certificate_handler, list_certificates_handler,
    revoke_certificate_handler,
};
pub use reporting::list_route_flows_handler;
pub use route_configs::{
    create_route_config_handler, delete_route_config_handler, get_route_config_handler,
    list_route_configs_handler, update_route_config_handler,
};
pub use route_views::{get_route_stats_handler, list_route_views_handler};
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
    remove_team_membership, update_team_membership_scopes, update_user,
};

// Re-export team access utilities for use across handlers
pub use team_access::{get_effective_team_scopes, verify_team_access, TeamOwned};

// Re-export hierarchy handlers for route hierarchy filter attachment
pub use hierarchy::{
    attach_filter_to_route_rule_handler, attach_filter_to_virtual_host_handler,
    detach_filter_from_route_rule_handler, detach_filter_from_virtual_host_handler,
    list_route_rule_filters_handler, list_route_rules_handler, list_virtual_host_filters_handler,
    list_virtual_hosts_handler,
};

// Re-export DTOs for OpenAPI docs
pub use aggregated_schemas::{
    AggregatedSchemaResponse, CompareSchemaQuery, ExportMultipleSchemasRequest, ExportSchemaQuery,
    ListAggregatedSchemasQuery, OpenApiExportResponse, SchemaComparisonResponse,
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
pub use mcp_tools::{ListMcpToolsQuery, ListMcpToolsResponse, McpToolResponse, UpdateMcpToolBody};
pub use organizations::{
    AddOrgMemberRequest, CurrentOrgResponse, ListOrgMembersResponse, ListOrgTeamsResponse,
    ListOrganizationsQuery, ListOrganizationsResponse, UpdateOrgMemberRoleRequest,
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

// Custom WASM filter DTOs
pub use custom_wasm_filters::{
    CreateCustomWasmFilterRequest, CustomWasmFilterResponse, ListCustomFiltersQuery,
    ListCustomWasmFiltersResponse, UpdateCustomWasmFilterRequest,
};

// Dataplane DTOs
pub use dataplanes::{
    BootstrapQuery as DataplaneBootstrapQuery, CreateDataplaneBody, DataplaneResponse,
    ListDataplanesQuery, ListDataplanesResponse, UpdateDataplaneBody,
};

// Hierarchy DTOs for route hierarchy filter attachment
pub use hierarchy::{
    AttachFilterRequest as HierarchyAttachFilterRequest, FilterResponse as HierarchyFilterResponse,
    ListRouteRulesResponse, ListVirtualHostsResponse, RouteRuleFiltersResponse, RouteRuleResponse,
    VirtualHostFiltersResponse, VirtualHostResponse,
};
