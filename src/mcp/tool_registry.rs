//! MCP Tool Authorization Registry
//!
//! Centralized mapping of tool names to required authorization scopes.
//! This is the single source of truth for tool->scope mappings.
//!
//! # Scope Hierarchy
//!
//! The authorization system supports hierarchical scopes:
//! - `admin:all` - Bypasses all checks
//! - `cp:read` - Grants read access to all CP resources
//! - `cp:write` - Grants write access to all CP resources
//! - `{resource}:read` - Grants read access to specific resource
//! - `{resource}:write` - Grants write access to specific resource
//! - `{resource}:delete` - Grants delete access to specific resource
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

use std::collections::HashMap;
use std::sync::LazyLock;

/// Tool authorization requirements
#[derive(Debug, Clone)]
pub struct ToolAuthorization {
    /// Resource name (e.g., "clusters", "listeners", "secrets")
    pub resource: &'static str,
    /// Required action (e.g., "read", "write", "delete", "create")
    pub action: &'static str,
    /// Human-readable description of scope requirements
    pub description: &'static str,
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
                description: "List clusters requires clusters:read or cp:read",
            },
        );
        m.insert(
            "cp_get_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "read",
                description: "Get cluster requires clusters:read or cp:read",
            },
        );
        m.insert(
            "cp_create_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "write",
                description: "Create cluster requires clusters:write or cp:write",
            },
        );
        m.insert(
            "cp_update_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "write",
                description: "Update cluster requires clusters:write or cp:write",
            },
        );
        m.insert(
            "cp_delete_cluster",
            ToolAuthorization {
                resource: "clusters",
                action: "delete",
                description: "Delete cluster requires clusters:delete or cp:write",
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
                description: "List listeners requires listeners:read or cp:read",
            },
        );
        m.insert(
            "cp_get_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "read",
                description: "Get listener requires listeners:read or cp:read",
            },
        );
        m.insert(
            "cp_create_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "write",
                description: "Create listener requires listeners:write or cp:write",
            },
        );
        m.insert(
            "cp_update_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "write",
                description: "Update listener requires listeners:write or cp:write",
            },
        );
        m.insert(
            "cp_delete_listener",
            ToolAuthorization {
                resource: "listeners",
                action: "delete",
                description: "Delete listener requires listeners:delete or cp:write",
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
                description: "List routes requires routes:read or cp:read",
            },
        );
        m.insert(
            "cp_get_route",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get route requires routes:read or cp:read",
            },
        );
        m.insert(
            "cp_create_route",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Create route requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_update_route",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Update route requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_delete_route",
            ToolAuthorization {
                resource: "routes",
                action: "delete",
                description: "Delete route requires routes:delete or cp:write",
            },
        );
        m.insert(
            "cp_create_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Create route config requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_update_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Update route config requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_delete_route_config",
            ToolAuthorization {
                resource: "routes",
                action: "delete",
                description: "Delete route config requires routes:delete or cp:write",
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
                description: "List filters requires filters:read or cp:read",
            },
        );
        m.insert(
            "cp_get_filter",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "Get filter requires filters:read or cp:read",
            },
        );
        m.insert(
            "cp_create_filter",
            ToolAuthorization {
                resource: "filters",
                action: "write",
                description: "Create filter requires filters:write or cp:write",
            },
        );
        m.insert(
            "cp_update_filter",
            ToolAuthorization {
                resource: "filters",
                action: "write",
                description: "Update filter requires filters:write or cp:write",
            },
        );
        m.insert(
            "cp_delete_filter",
            ToolAuthorization {
                resource: "filters",
                action: "delete",
                description: "Delete filter requires filters:delete or cp:write",
            },
        );

        // ============================================================================
        // FILTER ATTACHMENT TOOLS (FUTURE - Phase 2)
        // ============================================================================
        m.insert(
            "cp_attach_filter",
            ToolAuthorization {
                resource: "filters",
                action: "write",
                description: "Attach filter requires filters:write or cp:write",
            },
        );
        m.insert(
            "cp_detach_filter",
            ToolAuthorization {
                resource: "filters",
                action: "write",
                description: "Detach filter requires filters:write or cp:write",
            },
        );
        m.insert(
            "cp_list_filter_attachments",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "List filter attachments requires filters:read or cp:read",
            },
        );

        // ============================================================================
        // VIRTUAL HOST TOOLS (FUTURE - Phase 1)
        // ============================================================================
        m.insert(
            "cp_list_virtual_hosts",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "List virtual hosts requires routes:read or cp:read",
            },
        );
        m.insert(
            "cp_get_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get virtual host requires routes:read or cp:read",
            },
        );
        m.insert(
            "cp_create_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Create virtual host requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_update_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Update virtual host requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_delete_virtual_host",
            ToolAuthorization {
                resource: "routes",
                action: "delete",
                description: "Delete virtual host requires routes:delete or cp:write",
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
                description: "List secrets requires secrets:read",
            },
        );
        m.insert(
            "cp_get_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "read",
                description: "Get secret metadata requires secrets:read",
            },
        );
        m.insert(
            "cp_create_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "write",
                description: "Create secret requires secrets:write",
            },
        );
        m.insert(
            "cp_update_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "write",
                description: "Update secret requires secrets:write",
            },
        );
        m.insert(
            "cp_delete_secret",
            ToolAuthorization {
                resource: "secrets",
                action: "delete",
                description: "Delete secret requires secrets:delete",
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
                description: "List certificates requires proxy-certificates:read",
            },
        );
        m.insert(
            "cp_get_certificate",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "read",
                description: "Get certificate requires proxy-certificates:read",
            },
        );
        m.insert(
            "cp_create_certificate",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "create",
                description: "Create certificate requires proxy-certificates:create",
            },
        );
        m.insert(
            "cp_delete_certificate",
            ToolAuthorization {
                resource: "proxy-certificates",
                action: "delete",
                description: "Delete certificate requires proxy-certificates:delete",
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
                description: "List WASM filters requires custom-wasm-filters:read",
            },
        );
        m.insert(
            "cp_get_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "read",
                description: "Get WASM filter requires custom-wasm-filters:read",
            },
        );
        m.insert(
            "cp_upload_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "write",
                description: "Upload WASM filter requires custom-wasm-filters:write",
            },
        );
        m.insert(
            "cp_update_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "write",
                description: "Update WASM filter requires custom-wasm-filters:write",
            },
        );
        m.insert(
            "cp_delete_wasm_filter",
            ToolAuthorization {
                resource: "custom-wasm-filters",
                action: "delete",
                description: "Delete WASM filter requires custom-wasm-filters:delete",
            },
        );

        // ============================================================================
        // LEARNING SESSION TOOLS (FUTURE - requires learning-sessions:* scope)
        // ============================================================================
        m.insert(
            "cp_list_learning_sessions",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "read",
                description: "List learning sessions requires learning-sessions:read",
            },
        );
        m.insert(
            "cp_get_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "read",
                description: "Get learning session requires learning-sessions:read",
            },
        );
        m.insert(
            "cp_start_learning",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "write",
                description: "Start learning session requires learning-sessions:write",
            },
        );
        m.insert(
            "cp_create_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "write",
                description: "Create learning session requires learning-sessions:write",
            },
        );
        m.insert(
            "cp_stop_learning",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "write",
                description: "Stop learning session requires learning-sessions:write",
            },
        );
        m.insert(
            "cp_delete_learning_session",
            ToolAuthorization {
                resource: "learning-sessions",
                action: "delete",
                description: "Delete learning session requires learning-sessions:delete",
            },
        );

        // ============================================================================
        // AGGREGATED SCHEMA TOOLS (FUTURE - requires aggregated-schemas:* scope)
        // ============================================================================
        m.insert(
            "cp_list_schemas",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "List schemas requires aggregated-schemas:read",
            },
        );
        m.insert(
            "cp_list_aggregated_schemas",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "List aggregated schemas requires aggregated-schemas:read",
            },
        );
        m.insert(
            "cp_get_schema",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "Get schema requires aggregated-schemas:read",
            },
        );
        m.insert(
            "cp_get_aggregated_schema",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "read",
                description: "Get aggregated schema requires aggregated-schemas:read",
            },
        );
        m.insert(
            "cp_export_schema",
            ToolAuthorization {
                resource: "aggregated-schemas",
                action: "write",
                description: "Export schema requires aggregated-schemas:write",
            },
        );

        // ============================================================================
        // DATAPLANE TOOLS (FUTURE - requires dataplanes:* scope)
        // ============================================================================
        m.insert(
            "cp_list_dataplanes",
            ToolAuthorization {
                resource: "dataplanes",
                action: "read",
                description: "List dataplanes requires dataplanes:read",
            },
        );
        m.insert(
            "cp_get_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "read",
                description: "Get dataplane requires dataplanes:read",
            },
        );
        m.insert(
            "cp_register_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "write",
                description: "Register dataplane requires dataplanes:write",
            },
        );
        m.insert(
            "cp_create_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "write",
                description: "Create dataplane requires dataplanes:write",
            },
        );
        m.insert(
            "cp_update_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "write",
                description: "Update dataplane requires dataplanes:write",
            },
        );
        m.insert(
            "cp_deregister_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "write",
                description: "Deregister dataplane requires dataplanes:write",
            },
        );
        m.insert(
            "cp_delete_dataplane",
            ToolAuthorization {
                resource: "dataplanes",
                action: "delete",
                description: "Delete dataplane requires dataplanes:delete or dataplanes:write",
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
                description: "List reports requires reports:read",
            },
        );
        m.insert(
            "cp_get_report",
            ToolAuthorization {
                resource: "reports",
                action: "read",
                description: "Get report requires reports:read",
            },
        );

        // ============================================================================
        // OPENAPI IMPORT TOOLS (FUTURE)
        // ============================================================================
        m.insert(
            "cp_import_openapi",
            ToolAuthorization {
                resource: "routes",
                action: "write",
                description: "Import OpenAPI spec requires routes:write or cp:write",
            },
        );
        m.insert(
            "cp_list_openapi_imports",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "List OpenAPI imports requires routes:read or cp:read",
            },
        );
        m.insert(
            "cp_get_openapi_import",
            ToolAuthorization {
                resource: "routes",
                action: "read",
                description: "Get OpenAPI import details requires routes:read or cp:read",
            },
        );

        // ============================================================================
        // FILTER TYPE TOOLS (FUTURE)
        // ============================================================================
        m.insert(
            "cp_list_filter_types",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "List filter types requires filters:read or cp:read",
            },
        );
        m.insert(
            "cp_get_filter_type",
            ToolAuthorization {
                resource: "filters",
                action: "read",
                description: "Get filter type requires filters:read or cp:read",
            },
        );

        m
    });

/// Static authorization for gateway API tools
static GATEWAY_AUTH: ToolAuthorization = ToolAuthorization {
    resource: "api",
    action: "execute",
    description: "Execute gateway API tool requires api:execute",
};

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

/// Check if a scope grants the required authorization
///
/// Implements hierarchical scope matching:
/// 1. `admin:all` grants everything
/// 2. `cp:read` grants all `{resource}:read` for CP resources
/// 3. `cp:write` grants all `{resource}:write` and `{resource}:delete` for CP resources
/// 4. Exact match `{resource}:{action}` grants specific access
///
/// # Arguments
/// * `scopes` - Iterator of scope strings the user has
/// * `auth` - Required authorization
///
/// # Returns
/// * `true` if any scope grants the required authorization
/// * `false` otherwise
pub fn check_scope_grants_authorization<'a>(
    scopes: impl Iterator<Item = &'a str>,
    auth: &ToolAuthorization,
) -> bool {
    // CP resources that fall under cp:read/cp:write umbrella
    const CP_RESOURCES: &[&str] = &["clusters", "listeners", "routes", "filters"];

    for scope in scopes {
        // admin:all bypasses everything
        if scope == "admin:all" {
            return true;
        }

        // Exact match
        let required_scope = format!("{}:{}", auth.resource, auth.action);
        if scope == required_scope {
            return true;
        }

        // Check broad CP scopes for core resources
        if CP_RESOURCES.contains(&auth.resource) {
            if auth.action == "read" && scope == "cp:read" {
                return true;
            }
            if (auth.action == "write" || auth.action == "delete") && scope == "cp:write" {
                return true;
            }
        }

        // Check api:execute for gateway tools
        if auth.resource == "api" && auth.action == "execute" && scope == "api:execute" {
            return true;
        }
    }

    false
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
        assert_eq!(auth.action, "write");

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
        assert_eq!(auth.action, "write");
    }

    #[test]
    fn test_get_tool_authorization_route_tools() {
        let auth = get_tool_authorization("cp_list_routes").unwrap();
        assert_eq!(auth.resource, "routes");
        assert_eq!(auth.action, "read");

        let auth = get_tool_authorization("cp_create_route_config").unwrap();
        assert_eq!(auth.resource, "routes");
        assert_eq!(auth.action, "write");
    }

    #[test]
    fn test_get_tool_authorization_filter_tools() {
        let auth = get_tool_authorization("cp_list_filters").unwrap();
        assert_eq!(auth.resource, "filters");
        assert_eq!(auth.action, "read");

        let auth = get_tool_authorization("cp_create_filter").unwrap();
        assert_eq!(auth.resource, "filters");
        assert_eq!(auth.action, "write");
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

    #[test]
    fn test_check_scope_grants_authorization_admin_all() {
        let auth = ToolAuthorization { resource: "clusters", action: "write", description: "" };

        // admin:all grants everything
        assert!(check_scope_grants_authorization(["admin:all"].iter().copied(), &auth));
    }

    #[test]
    fn test_check_scope_grants_authorization_exact_match() {
        let auth = ToolAuthorization { resource: "clusters", action: "read", description: "" };

        assert!(check_scope_grants_authorization(["clusters:read"].iter().copied(), &auth));
        assert!(!check_scope_grants_authorization(["clusters:write"].iter().copied(), &auth));
        assert!(!check_scope_grants_authorization(["listeners:read"].iter().copied(), &auth));
    }

    #[test]
    fn test_check_scope_grants_authorization_cp_read() {
        let auth = ToolAuthorization { resource: "clusters", action: "read", description: "" };

        // cp:read grants all core resource reads
        assert!(check_scope_grants_authorization(["cp:read"].iter().copied(), &auth));

        // cp:read does NOT grant writes
        let write_auth =
            ToolAuthorization { resource: "clusters", action: "write", description: "" };
        assert!(!check_scope_grants_authorization(["cp:read"].iter().copied(), &write_auth));
    }

    #[test]
    fn test_check_scope_grants_authorization_cp_write() {
        let write_auth =
            ToolAuthorization { resource: "listeners", action: "write", description: "" };
        let delete_auth =
            ToolAuthorization { resource: "filters", action: "delete", description: "" };

        // cp:write grants write and delete for core resources
        assert!(check_scope_grants_authorization(["cp:write"].iter().copied(), &write_auth));
        assert!(check_scope_grants_authorization(["cp:write"].iter().copied(), &delete_auth));

        // cp:write does NOT grant reads
        let read_auth = ToolAuthorization { resource: "clusters", action: "read", description: "" };
        assert!(!check_scope_grants_authorization(["cp:write"].iter().copied(), &read_auth));
    }

    #[test]
    fn test_check_scope_grants_authorization_sensitive_resources() {
        // Sensitive resources (secrets, wasm) are NOT covered by cp:read/cp:write
        let secrets_read =
            ToolAuthorization { resource: "secrets", action: "read", description: "" };

        // cp:read does NOT grant secrets:read
        assert!(!check_scope_grants_authorization(["cp:read"].iter().copied(), &secrets_read));

        // Must have exact scope
        assert!(check_scope_grants_authorization(["secrets:read"].iter().copied(), &secrets_read));

        // Same for WASM filters
        let wasm_write =
            ToolAuthorization { resource: "custom-wasm-filters", action: "write", description: "" };
        assert!(!check_scope_grants_authorization(["cp:write"].iter().copied(), &wasm_write));
        assert!(check_scope_grants_authorization(
            ["custom-wasm-filters:write"].iter().copied(),
            &wasm_write
        ));
    }

    #[test]
    fn test_check_scope_grants_authorization_multiple_scopes() {
        let auth = ToolAuthorization { resource: "clusters", action: "write", description: "" };

        // Having multiple scopes where one matches
        assert!(check_scope_grants_authorization(
            ["mcp:execute", "cp:read", "clusters:write"].iter().copied(),
            &auth
        ));

        // Having multiple scopes where none match
        assert!(!check_scope_grants_authorization(
            ["mcp:execute", "cp:read", "listeners:write"].iter().copied(),
            &auth
        ));
    }

    #[test]
    fn test_check_scope_grants_authorization_api_execute() {
        let auth = ToolAuthorization { resource: "api", action: "execute", description: "" };

        assert!(check_scope_grants_authorization(["api:execute"].iter().copied(), &auth));
        assert!(!check_scope_grants_authorization(["api:read"].iter().copied(), &auth));
        assert!(!check_scope_grants_authorization(["cp:write"].iter().copied(), &auth));
    }
}
