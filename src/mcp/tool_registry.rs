//! MCP Tool Authorization Registry
//!
//! Centralized mapping of tool names to (resource, action) pairs used by
//! check_resource_access() to enforce team-scoped permissions.
//!
//! # Example
//!
//! ```rust
//! use flowplane::mcp::tool_registry::get_tool_authorization;
//!
//! let auth = get_tool_authorization("cp_list_clusters").unwrap();
//! assert_eq!(auth.resource, "clusters");
//! assert_eq!(auth.action, "read");
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Risk level for MCP tool operations
///
/// Ordered from safest to most dangerous. The `Ord` derive uses variant
/// declaration order, so `Safe < Low < Medium < High < Critical`.
/// This enables future enforcement: `if risk >= RiskLevel::High { require_approval() }`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    /// Read-only, no side effects
    Safe,
    /// Easily reversible, additive (create operations)
    Low,
    /// Affects live traffic (update, attach/detach)
    Medium,
    /// Potential outage (delete listener, delete route_config, detach auth)
    High,
    /// Organization-wide impact
    Critical,
}

impl RiskLevel {
    /// Returns the risk level as a static string for display
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Safe => "SAFE",
            RiskLevel::Low => "LOW",
            RiskLevel::Medium => "MEDIUM",
            RiskLevel::High => "HIGH",
            RiskLevel::Critical => "CRITICAL",
        }
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tool authorization requirements
#[derive(Debug, Clone)]
pub struct ToolAuthorization {
    /// Resource name (e.g., "clusters", "listeners", "secrets")
    pub resource: &'static str,
    /// Required action (e.g., "read", "create", "update", "delete", "execute")
    pub action: &'static str,
    /// Human-readable description of scope requirements
    pub description: &'static str,
    /// Risk level for this operation
    pub risk_level: RiskLevel,
}

/// Static registry of tool authorizations
///
/// This HashMap is built once at program start and provides O(1) lookup
/// for tool authorization requirements.
static TOOL_AUTHORIZATIONS: LazyLock<HashMap<&'static str, ToolAuthorization>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();

        // ============================================================================
        // CLUSTER TOOLS
        // ============================================================================
        m.insert(
            "cp_list_clusters",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "List clusters requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Get cluster requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "create",
                description: "Create cluster requires grant: clusters:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "update",
                description: "Update cluster requires grant: clusters:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "delete",
                description: "Delete cluster requires grant: clusters:delete",
                risk_level: RiskLevel::High,
            },
        );
        m.insert(
            "cp_get_cluster_health",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Get cluster health requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // LISTENER TOOLS
        // ============================================================================
        m.insert(
            "cp_list_listeners",
            ToolAuthorization {
                resource: "listeners",
                action: "read",
                description: "List listeners requires grant: listeners:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "read",
                description: "Get listener requires grant: listeners:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "create",
                description: "Create listener requires grant: listeners:create",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_update_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "update",
                description: "Update listener requires grant: listeners:update",
                risk_level: RiskLevel::High,
            },
        );
        m.insert(
            "cp_delete_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "delete",
                description: "Delete listener requires grant: listeners:delete",
                risk_level: RiskLevel::High,
            },
        );
        m.insert(
            "cp_query_port",
            ToolAuthorization {
                resource: "listeners",
                action: "read",
                description: "Query port requires grant: listeners:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_listener_status",
            ToolAuthorization {
                resource: "listeners",
                action: "read",
                description: "Get listener status requires grant: listeners:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // ROUTE TOOLS
        // ============================================================================
        m.insert(
            "cp_list_routes",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "List routes requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_route",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get route requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_route",
            ToolAuthorization {
                resource: "routes",
                action: "create",
                description: "Create route requires grant: routes:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_route",
            ToolAuthorization {
                resource: "routes",
                action: "update",
                description: "Update route requires grant: routes:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_route",
            ToolAuthorization {
                resource: "routes",
                action: "delete",
                description: "Delete route requires grant: routes:delete",
                risk_level: RiskLevel::High,
            },
        );
        m.insert(
            "cp_list_route_configs",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "List route configs requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get route config requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "create",
                description: "Create route config requires grant: routes:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "update",
                description: "Update route config requires grant: routes:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "delete",
                description: "Delete route config requires grant: routes:delete",
                risk_level: RiskLevel::High,
            },
        );
        m.insert(
            "cp_query_path",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Query path requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // FILTER TOOLS
        // ============================================================================
        m.insert(
            "cp_list_filters",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "List filters requires grant: filters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_filter",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "Get filter requires grant: filters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_filter",
            ToolAuthorization {
                resource: "filters",
                action: "create",
                description: "Create filter requires grant: filters:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_filter",
            ToolAuthorization {
                resource: "filters",
                action: "update",
                description: "Update filter requires grant: filters:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_filter",
            ToolAuthorization {
                resource: "filters",
                action: "delete",
                description: "Delete filter requires grant: filters:delete",
                risk_level: RiskLevel::High,
            },
        );

        // ============================================================================
        // FILTER ATTACHMENT TOOLS
        // ============================================================================
        m.insert(
            "cp_attach_filter",
            ToolAuthorization {
                resource: "filters",
                action: "update",
                description: "Attach filter requires grant: filters:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_detach_filter",
            ToolAuthorization {
                resource: "filters",
                action: "update",
                description: "Detach filter requires grant: filters:update",
                risk_level: RiskLevel::High,
            },
        );
        m.insert(
            "cp_list_filter_attachments",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "List filter attachments requires grant: filters:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // VIRTUAL HOST TOOLS
        // ============================================================================
        m.insert(
            "cp_list_virtual_hosts",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "List virtual hosts requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get virtual host requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "create",
                description: "Create virtual host requires grant: routes:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "update",
                description: "Update virtual host requires grant: routes:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "delete",
                description: "Delete virtual host requires grant: routes:delete",
                risk_level: RiskLevel::High,
            },
        );

        // ============================================================================
        // SECRET TOOLS (FUTURE - requires secrets:* scope)
        // ============================================================================
        m.insert(
            "cp_list_secrets",
            ToolAuthorization {
                resource: "secrets",
                action: "read",
                description: "List secrets requires grant: secrets:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "read",
                description: "Get secret metadata requires grant: secrets:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "create",
                description: "Create secret requires grant: secrets:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "update",
                description: "Update secret requires grant: secrets:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "delete",
                description: "Delete secret requires grant: secrets:delete",
                risk_level: RiskLevel::High,
            },
        );

        // ============================================================================
        // CERTIFICATE TOOLS (FUTURE - requires proxy-certificates:* scope)
        // ============================================================================
        m.insert(
            "cp_list_certificates",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "read",
                description: "List certificates requires grant: proxy-certificates:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_certificate",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "read",
                description: "Get certificate requires grant: proxy-certificates:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_create_certificate",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "create",
                description: "Create certificate requires grant: proxy-certificates:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_delete_certificate",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "delete",
                description: "Delete certificate requires grant: proxy-certificates:delete",
                risk_level: RiskLevel::High,
            },
        );

        // ============================================================================
        // CUSTOM WASM FILTER TOOLS (FUTURE - requires custom-wasm-filters:* scope)
        // ============================================================================
        m.insert(
            "cp_list_wasm_filters",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "read",
                description: "List WASM filters requires grant: custom-wasm-filters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "read",
                description: "Get WASM filter requires grant: custom-wasm-filters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_upload_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "create",
                description: "Upload WASM filter requires grant: custom-wasm-filters:create",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_update_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "update",
                description: "Update WASM filter requires grant: custom-wasm-filters:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "delete",
                description: "Delete WASM filter requires grant: custom-wasm-filters:delete",
                risk_level: RiskLevel::High,
            },
        );

        // ============================================================================
        // LEARNING SESSION TOOLS
        // ============================================================================
        m.insert(
            "cp_list_learning_sessions",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "read",
                description: "List learning sessions requires grant: learning-sessions:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "read",
                description: "Get learning session requires grant: learning-sessions:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_start_learning",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "execute",
                description: "Start learning session requires grant: learning-sessions:execute",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_create_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "create",
                description: "Create learning session requires grant: learning-sessions:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_stop_learning",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "execute",
                description: "Stop learning session requires grant: learning-sessions:execute",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_delete_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "delete",
                description: "Delete learning session requires grant: learning-sessions:delete",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_activate_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "execute",
                description: "Activate learning session requires grant: learning-sessions:execute",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "ops_learning_session_health",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "read",
                description: "Learning session health check requires grant: learning-sessions:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // AGGREGATED SCHEMA TOOLS
        // ============================================================================
        m.insert(
            "cp_list_schemas",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "List schemas requires grant: aggregated-schemas:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_list_aggregated_schemas",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "List aggregated schemas requires grant: aggregated-schemas:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_schema",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "Get schema requires grant: aggregated-schemas:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_aggregated_schema",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "Get aggregated schema requires grant: aggregated-schemas:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_export_schema",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "execute",
                description: "Export schema requires grant: aggregated-schemas:execute",
                risk_level: RiskLevel::Low,
            },
        );

        // ============================================================================
        // DATAPLANE TOOLS
        // ============================================================================
        m.insert(
            "cp_list_dataplanes",
            ToolAuthorization {
                resource: "dataplanes",
                action: "read",
                description: "List dataplanes requires grant: dataplanes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "read",
                description: "Get dataplane requires grant: dataplanes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_register_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "create",
                description: "Register dataplane requires grant: dataplanes:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_create_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "create",
                description: "Create dataplane requires grant: dataplanes:create",
                risk_level: RiskLevel::Low,
            },
        );
        m.insert(
            "cp_update_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "update",
                description: "Update dataplane requires grant: dataplanes:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_deregister_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "update",
                description: "Deregister dataplane requires grant: dataplanes:update",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_delete_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "delete",
                description: "Delete dataplane requires grant: dataplanes:delete",
                risk_level: RiskLevel::Medium,
            },
        );

        // ============================================================================
        // REPORT TOOLS (FUTURE - requires reports:* scope)
        // ============================================================================
        m.insert(
            "cp_list_reports",
            ToolAuthorization {
                resource: "reports",
                action: "read",
                description: "List reports requires grant: reports:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_report",
            ToolAuthorization {
                resource: "reports",
                action: "read",
                description: "Get report requires grant: reports:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // OPENAPI IMPORT TOOLS
        // ============================================================================
        m.insert(
            "cp_import_openapi",
            ToolAuthorization {
                resource: "routes",
                action: "create",
                description: "Import OpenAPI spec requires grant: routes:create",
                risk_level: RiskLevel::Medium,
            },
        );
        m.insert(
            "cp_list_openapi_imports",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "List OpenAPI imports requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_openapi_import",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get OpenAPI import details requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // FILTER TYPE TOOLS
        // ============================================================================
        m.insert(
            "cp_list_filter_types",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "List filter types requires grant: filters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_get_filter_type",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "Get filter type requires grant: filters:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // DEVOPS AGENT WORKFLOW TOOLS
        // ============================================================================
        m.insert(
            "devops_get_deployment_status",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Get deployment status requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // OPS AGENT DIAGNOSTIC TOOLS
        // ============================================================================
        m.insert(
            "ops_trace_request",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Trace request path requires grant: routes:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "ops_topology",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "View topology requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "ops_config_validate",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Validate config requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );

        m.insert(
            "ops_xds_delivery_status",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "xDS delivery status requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "ops_nack_history",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "NACK history requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );

        m.insert(
            "ops_xds_delivery_status",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "xDS delivery status requires clusters:read or cp:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "ops_nack_history",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "NACK history requires clusters:read or cp:read",
                risk_level: RiskLevel::Safe,
            },
        );

        // ============================================================================
        // AUDIT TOOLS
        // ============================================================================
        m.insert(
            "ops_audit_query",
            ToolAuthorization {
                resource: "audit",
                action: "read",
                description: "Query audit logs requires grant: audit:read",
                risk_level: RiskLevel::Low,
            },
        );

        // ============================================================================
        // DEV AGENT WORKFLOW TOOLS
        // ============================================================================
        m.insert(
            "dev_preflight_check",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Pre-creation validation requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_query_service",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Query service summary requires grant: clusters:read",
                risk_level: RiskLevel::Safe,
            },
        );
        m.insert(
            "cp_export_schema_openapi",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "Export schemas as OpenAPI requires grant: aggregated-schemas:read",
                risk_level: RiskLevel::Safe,
            },
        );

        m
    });

/// Static authorization for gateway API tools
static GATEWAY_AUTH: ToolAuthorization = ToolAuthorization {
    resource: "api",
    action: "execute",
    description: "Execute gateway API tool requires grant: api:execute",
    risk_level: RiskLevel::Medium,
};

/// Check whether a (resource_type, action) pair is valid for cp-tool grants.
///
/// Returns `true` if any CP tool in `TOOL_AUTHORIZATIONS` has this exact
/// (resource, action) pair. Gateway tools are excluded (resource != "api").
pub fn is_valid_cp_grant_pair(resource_type: &str, action: &str) -> bool {
    TOOL_AUTHORIZATIONS.values().any(|auth| auth.resource == resource_type && auth.action == action)
}

/// Get authorization requirements for a tool
///
/// Returns the resource and action required to execute the tool.
/// For CP tools, checks the registry. For gateway API tools (starting with "api_"),
/// returns the gateway authorization.
///
/// # Arguments
/// * `tool_name` - Name of the tool (e.g., "cp_list_clusters", "api_getUser")
///
/// # Returns
/// * `Some(&ToolAuthorization)` - Authorization requirements for known tools
/// * `None` - Tool is not registered (unknown tool)
pub fn get_tool_authorization(tool_name: &str) -> Option<&'static ToolAuthorization> {
    // Check exact match first
    if let Some(auth) = TOOL_AUTHORIZATIONS.get(tool_name) {
        return Some(auth);
    }

    // Gateway API tools: any tool starting with "api_"
    if tool_name.starts_with("api_") {
        return Some(&GATEWAY_AUTH);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_tool_authorization_cluster_tools() {
        let auth = get_tool_authorization("cp_list_clusters").unwrap();
        assert_eq!(auth.resource, "clusters");
        assert_eq!(auth.action, "read");

        let auth = get_tool_authorization("cp_create_cluster").unwrap();
        assert_eq!(auth.resource, "clusters");
        assert_eq!(auth.action, "create");

        let auth = get_tool_authorization("cp_delete_cluster").unwrap();
        assert_eq!(auth.resource, "clusters");
        assert_eq!(auth.action, "delete");
    }

    #[test]
    fn test_get_tool_authorization_listener_tools() {
        let auth = get_tool_authorization("cp_list_listeners").unwrap();
        assert_eq!(auth.resource, "listeners");
        assert_eq!(auth.action, "read");

        let auth = get_tool_authorization("cp_create_listener").unwrap();
        assert_eq!(auth.resource, "listeners");
        assert_eq!(auth.action, "create");
    }

    #[test]
    fn test_get_tool_authorization_route_tools() {
        let auth = get_tool_authorization("cp_list_routes").unwrap();
        assert_eq!(auth.resource, "routes");
        assert_eq!(auth.action, "read");

        let auth = get_tool_authorization("cp_create_route_config").unwrap();
        assert_eq!(auth.resource, "routes");
        assert_eq!(auth.action, "create");
    }

    #[test]
    fn test_get_tool_authorization_filter_tools() {
        let auth = get_tool_authorization("cp_list_filters").unwrap();
        assert_eq!(auth.resource, "filters");
        assert_eq!(auth.action, "read");

        let auth = get_tool_authorization("cp_create_filter").unwrap();
        assert_eq!(auth.resource, "filters");
        assert_eq!(auth.action, "create");
    }

    #[test]
    fn test_get_tool_authorization_gateway_api_tools() {
        let auth = get_tool_authorization("api_getUser").unwrap();
        assert_eq!(auth.resource, "api");
        assert_eq!(auth.action, "execute");

        let auth = get_tool_authorization("api_createOrder").unwrap();
        assert_eq!(auth.resource, "api");
        assert_eq!(auth.action, "execute");
    }

    #[test]
    fn test_get_tool_authorization_unknown_tool() {
        assert!(get_tool_authorization("unknown_tool").is_none());
        assert!(get_tool_authorization("foo_bar").is_none());
    }

    #[test]
    fn test_get_tool_authorization_future_tools() {
        // Secrets
        let auth = get_tool_authorization("cp_list_secrets").unwrap();
        assert_eq!(auth.resource, "secrets");

        // Learning sessions
        let auth = get_tool_authorization("cp_start_learning").unwrap();
        assert_eq!(auth.resource, "learning-sessions");

        // WASM filters
        let auth = get_tool_authorization("cp_upload_wasm_filter").unwrap();
        assert_eq!(auth.resource, "custom-wasm-filters");
    }

    // ============================================================================
    // Risk Level Tests
    // ============================================================================

    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::Safe < RiskLevel::Low);
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
    }

    #[test]
    fn test_risk_level_display() {
        assert_eq!(RiskLevel::Safe.to_string(), "SAFE");
        assert_eq!(RiskLevel::Low.to_string(), "LOW");
        assert_eq!(RiskLevel::Medium.to_string(), "MEDIUM");
        assert_eq!(RiskLevel::High.to_string(), "HIGH");
        assert_eq!(RiskLevel::Critical.to_string(), "CRITICAL");
    }

    #[test]
    fn test_risk_level_serde() {
        let json = serde_json::to_string(&RiskLevel::High).unwrap();
        assert_eq!(json, r#""HIGH""#);

        let level: RiskLevel = serde_json::from_str(r#""MEDIUM""#).unwrap();
        assert_eq!(level, RiskLevel::Medium);
    }

    #[test]
    fn test_all_tools_have_risk_level() {
        // Verify every tool in the registry has a risk_level assigned
        for (name, auth) in TOOL_AUTHORIZATIONS.iter() {
            // Just verify it's not Critical (no current tools should be Critical)
            assert_ne!(
                auth.risk_level,
                RiskLevel::Critical,
                "Tool '{}' should not be Critical risk",
                name
            );
        }
    }

    #[test]
    fn test_read_tools_are_safe() {
        let read_tools = [
            "cp_list_clusters",
            "cp_get_cluster",
            "cp_get_cluster_health",
            "cp_list_listeners",
            "cp_get_listener",
            "cp_get_listener_status",
            "cp_query_port",
            "cp_list_routes",
            "cp_list_route_configs",
            "cp_get_route_config",
            "cp_get_route",
            "cp_query_path",
            "cp_list_filters",
            "cp_get_filter",
            "cp_list_filter_attachments",
            "cp_list_virtual_hosts",
            "cp_get_virtual_host",
            "cp_list_aggregated_schemas",
            "cp_get_aggregated_schema",
            "cp_list_learning_sessions",
            "cp_get_learning_session",
            "cp_list_openapi_imports",
            "cp_get_openapi_import",
            "cp_list_dataplanes",
            "cp_get_dataplane",
            "cp_list_filter_types",
            "cp_get_filter_type",
            "devops_get_deployment_status",
            "ops_trace_request",
            "ops_topology",
            "ops_config_validate",
            "dev_preflight_check",
            "cp_query_service",
            "cp_export_schema_openapi",
            "ops_learning_session_health",
            "cp_list_secrets",
            "cp_get_secret",
        ];

        for tool_name in &read_tools {
            let auth = get_tool_authorization(tool_name)
                .unwrap_or_else(|| panic!("Tool '{}' not found in registry", tool_name));
            assert_eq!(
                auth.risk_level,
                RiskLevel::Safe,
                "Read-only tool '{}' should be SAFE risk",
                tool_name
            );
        }
    }

    #[test]
    fn test_delete_tools_are_high() {
        let high_tools = [
            "cp_delete_cluster",
            "cp_delete_listener",
            "cp_delete_route_config",
            "cp_delete_route",
            "cp_delete_virtual_host",
            "cp_delete_filter",
            "cp_detach_filter",
            "cp_update_listener",
            "cp_delete_secret",
        ];

        for tool_name in &high_tools {
            let auth = get_tool_authorization(tool_name)
                .unwrap_or_else(|| panic!("Tool '{}' not found in registry", tool_name));
            assert_eq!(
                auth.risk_level,
                RiskLevel::High,
                "Tool '{}' should be HIGH risk",
                tool_name
            );
        }
    }
}
