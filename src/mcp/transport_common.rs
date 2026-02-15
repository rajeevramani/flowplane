//! MCP Transport Common Utilities
//!
//! Provides shared functionality for all MCP transport implementations (HTTP, HTTP+API, SSE).
//! Consolidates duplicate code for team extraction, authorization, header parsing, and validation.
//!
//! # MCP 2025-11-25 Compliance
//! - Protocol version validation (exact match required)
//! - Origin header validation (defense-in-depth for browser clients)
//! - Session ID format validation
//! - Response mode determination (JSON vs SSE)

use axum::http::HeaderMap;
use tracing::debug;

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{JsonRpcError, JsonRpcId, JsonRpcResponse, PROTOCOL_VERSION};

#[allow(unused_imports)]
use crate::mcp::protocol::error_codes;
use crate::storage::DbPool;

/// Scope configuration for method authorization
///
/// Different transport endpoints use different scope prefixes:
/// - Control Plane (CP): `mcp:read`, `mcp:execute`, `cp:read`
/// - API: `api:read`, `api:execute`, no resource scope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeConfig {
    /// Scope required for read operations (e.g., "mcp:read", "api:read")
    pub read_scope: &'static str,

    /// Scope required for execute operations (e.g., "mcp:execute", "api:execute")
    pub execute_scope: &'static str,

    /// Optional scope for resource read operations (e.g., Some("cp:read"), None for API)
    pub resource_read_scope: Option<&'static str>,
}

/// Control Plane scope configuration
pub const CP_SCOPES: ScopeConfig = ScopeConfig {
    read_scope: "mcp:read",
    execute_scope: "mcp:execute",
    resource_read_scope: Some("cp:read"),
};

/// API scope configuration
pub const API_SCOPES: ScopeConfig =
    ScopeConfig { read_scope: "api:read", execute_scope: "api:execute", resource_read_scope: None };

/// MCP 2025-11-25 protocol headers
///
/// Extracted from HTTP headers for protocol compliance and security validation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpHeaders {
    /// MCP protocol version from "MCP-Protocol-Version" header
    pub protocol_version: Option<String>,

    /// Session ID from "MCP-Session-Id" header for session tracking
    pub session_id: Option<String>,

    /// Origin header for CSRF protection (defense-in-depth)
    pub origin: Option<String>,
}

/// Response mode for MCP requests
///
/// Determines whether to return JSON-RPC response directly or via SSE stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMode {
    /// Return JSON-RPC response directly (default)
    Json,

    /// Stream response via Server-Sent Events
    Sse,
}

/// Extract team name from query parameter or auth context
///
/// Priority order:
/// 1. Query parameter `?team=<name>` (highest priority)
/// 2. Token scopes with pattern `team:{name}:*`
/// 3. Admin users with `admin:all` MUST provide team via query parameter
///
/// # Arguments
/// * `team_query` - Optional team name from query parameter
/// * `context` - Authentication context with token scopes
///
/// # Returns
/// Team name on success, descriptive error message on failure
pub fn extract_team(team_query: Option<&str>, context: &AuthContext) -> Result<String, String> {
    // Platform admin with only admin:all (no org/team scopes) cannot specify teams.
    // Governance/audit tools operate without team context.
    // Tool-level auth (check_scope_grants_authorization) also blocks resource tools
    // for admin:all, but this provides defense-in-depth.
    if context.has_scope("admin:all") {
        let has_org_or_team_scopes =
            context.scopes().any(|s| s.starts_with("org:") || s.starts_with("team:"));
        if !has_org_or_team_scopes {
            return Err("Platform admin cannot specify team for MCP operations. \
                 Governance/audit tools do not require team context. \
                 Use org-scoped token for resource operations."
                .to_string());
        }
    }

    // Priority 1: Query parameter
    if let Some(team) = team_query {
        debug!(team = %team, "Team extracted from query parameter");
        return Ok(team.to_string());
    }

    // Priority 2: Extract team from scopes (pattern: team:{name}:*)
    for scope in context.scopes() {
        if let Some(team_part) = scope.strip_prefix("team:") {
            if let Some(team_name) = team_part.split(':').next() {
                debug!(team = %team_name, scope = %scope, "Team extracted from token scope");
                return Ok(team_name.to_string());
            }
        }
    }

    Err("Unable to determine team. Please provide team via query parameter".to_string())
}

/// Validate that a team belongs to the caller's org.
///
/// When team is provided via query parameter (not from token scopes), there's a risk
/// the caller specifies a team from a different org. This function queries the DB to
/// verify the team belongs to the org_id from the caller's auth context.
///
/// # Arguments
/// * `team` - Team name to validate
/// * `org_id` - Organization ID from the caller's auth context
/// * `db_pool` - Database connection pool
///
/// # Returns
/// `Ok(())` if team belongs to org, `Err(message)` if not
pub async fn validate_team_org_membership(
    team: &str,
    org_id: &crate::domain::OrgId,
    db_pool: &DbPool,
) -> Result<(), String> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT COUNT(*) FROM teams WHERE name = $1 AND org_id = $2")
            .bind(team)
            .bind(org_id.as_str())
            .fetch_optional(db_pool)
            .await
            .map_err(|e| format!("Failed to validate team membership: {}", e))?;

    let count = row.map(|r| r.0).unwrap_or(0);
    if count == 0 {
        return Err(format!("Team '{}' not found in your organization", team));
    }

    Ok(())
}

/// Check if auth context has required scope for the given MCP method
///
/// Uses configurable scope prefixes to support both CP and API endpoints.
/// Special methods (`initialize`, `initialized`, `ping`) require no scope.
/// Platform admin (`admin:all`) is restricted to `tools/list` and `tools/call` only
/// (governance/audit access). Tool-level auth further restricts which tools can be called.
///
/// # Arguments
/// * `method` - MCP method name (e.g., "tools/list", "tools/call")
/// * `context` - Authentication context with token scopes
/// * `config` - Scope configuration (defines read/execute/resource scopes)
///
/// # Returns
/// `Ok(())` if authorized, `Err(message)` with required scope on failure
pub fn check_method_authorization(
    method: &str,
    context: &AuthContext,
    config: &ScopeConfig,
) -> Result<(), String> {
    // Special methods require no scope
    match method {
        "initialize"
        | "initialized"
        | "ping"
        | "notifications/initialized"
        | "notifications/cancelled" => {
            return Ok(());
        }
        _ => {}
    }

    // Platform admin: limited MCP method access for governance/audit only.
    // admin:all grants tools/list (discover audit tools) and tools/call (invoke audit tools).
    // Tool-level auth (check_scope_grants_authorization) further restricts to governance tools.
    if context.has_scope("admin:all") {
        return match method {
            "tools/list" | "tools/call" => Ok(()),
            _ => Err(format!(
                "Platform admin does not have access to MCP method '{}'. \
                 Use org-scoped token for resource operations.",
                method
            )),
        };
    }

    // Method-specific authorization
    match method {
        // Read operations
        "tools/list" | "resources/list" | "prompts/list" => {
            if context.has_scope(config.read_scope) {
                Ok(())
            } else {
                Err(format!(
                    "Missing required scope '{}' for method '{}'",
                    config.read_scope, method
                ))
            }
        }

        // Execute operations
        "tools/call" | "prompts/get" => {
            if context.has_scope(config.execute_scope) {
                Ok(())
            } else {
                Err(format!(
                    "Missing required scope '{}' for method '{}'",
                    config.execute_scope, method
                ))
            }
        }

        // Resource read operations (only for CP endpoints)
        "resources/read" => {
            if let Some(resource_scope) = config.resource_read_scope {
                if context.has_scope(resource_scope) {
                    Ok(())
                } else {
                    Err(format!(
                        "Missing required scope '{}' for method '{}'",
                        resource_scope, method
                    ))
                }
            } else {
                // No resource scope configured - deny
                Err("Resource operations not supported for this endpoint".to_string())
            }
        }

        // Logging operations (use read scope)
        "logging/setLevel" => {
            if context.has_scope(config.read_scope) {
                Ok(())
            } else {
                Err(format!(
                    "Missing required scope '{}' for method '{}'",
                    config.read_scope, method
                ))
            }
        }

        // Unknown methods - allow (handler will deal with it)
        _ => Ok(()),
    }
}

/// Extract MCP 2025-11-25 protocol headers from HTTP request
///
/// Extracts and returns MCP-specific headers for protocol compliance:
/// - `MCP-Protocol-Version`: Required for version negotiation
/// - `MCP-Session-Id`: Optional for session tracking
/// - `Origin`: Optional but validated when present (CSRF protection)
///
/// # Arguments
/// * `headers` - HTTP header map from request
///
/// # Returns
/// Struct containing extracted header values (None if header missing)
///
/// # Note
/// Header names are case-insensitive per HTTP spec.
pub fn extract_mcp_headers(headers: &HeaderMap) -> McpHeaders {
    let protocol_version =
        headers.get("mcp-protocol-version").and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let session_id =
        headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let origin = headers.get("origin").and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    McpHeaders { protocol_version, session_id, origin }
}

/// Determine response mode from Accept header
///
/// Checks the Accept header to determine if client wants SSE streaming
/// or standard JSON response.
///
/// # Arguments
/// * `accept_header` - Value of Accept header (None if not present)
///
/// # Returns
/// - `ResponseMode::Sse` if Accept header contains "text/event-stream"
/// - `ResponseMode::Json` otherwise (default)
pub fn determine_response_mode(accept_header: Option<&str>) -> ResponseMode {
    match accept_header {
        Some(accept) if accept.contains("text/event-stream") => ResponseMode::Sse,
        _ => ResponseMode::Json,
    }
}

/// Validate MCP protocol version (MCP 2025-11-25 requirement)
///
/// Ensures client protocol version exactly matches server's supported version.
/// No backward compatibility - clients must use MCP 2025-11-25.
///
/// # Arguments
/// * `version` - Protocol version string from client (e.g., "2025-11-25")
///
/// # Returns
/// - `Ok(())` if version matches exactly
/// - `Err(McpError::UnsupportedProtocolVersion)` otherwise
pub fn validate_protocol_version(version: &str) -> Result<(), McpError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(McpError::UnsupportedProtocolVersion {
            client: version.to_string(),
            supported: vec![PROTOCOL_VERSION.to_string()],
        })
    }
}

/// Get database pool from API state
///
/// Extracts the database pool from xDS state cluster repository.
///
/// # Arguments
/// * `state` - API state containing xDS state and repositories
///
/// # Returns
/// Cloned database pool on success, error message on failure
pub fn get_db_pool(state: &ApiState) -> Result<DbPool, String> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| "Database not available".to_string())?;

    Ok(cluster_repo.pool().clone())
}

/// Create JSON-RPC error response
///
/// Helper function to construct a well-formed JSON-RPC 2.0 error response.
///
/// # Arguments
/// * `code` - JSON-RPC error code (use constants from `error_codes`)
/// * `message` - Human-readable error message
/// * `id` - Request ID (None for parse errors or notifications)
///
/// # Returns
/// Fully constructed `JsonRpcResponse` with error field populated
pub fn error_response_json(code: i32, message: String, id: Option<JsonRpcId>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(JsonRpcError { code, message, data: None }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TokenId;

    // Helper to create test context
    fn test_context(scopes: Vec<&str>) -> AuthContext {
        AuthContext::new(
            TokenId::from_str_unchecked("test-token-1"),
            "test-token".to_string(),
            scopes.into_iter().map(|s| s.to_string()).collect(),
        )
    }

    // -------------------------------------------------------------------------
    // Team Extraction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_extract_team_from_query_parameter() {
        let context = test_context(vec![]);
        let result = extract_team(Some("acme-corp"), &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "acme-corp");
    }

    #[test]
    fn test_extract_team_from_scope() {
        let context = test_context(vec!["team:acme-corp:mcp:read"]);
        let result = extract_team(None, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "acme-corp");
    }

    #[test]
    fn test_extract_team_query_takes_priority_over_scope() {
        let context = test_context(vec!["team:old-team:mcp:read"]);
        let result = extract_team(Some("new-team"), &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "new-team");
    }

    #[test]
    fn test_extract_team_admin_only_blocked() {
        // admin:all without org/team scopes cannot specify teams at all
        let context = test_context(vec!["admin:all"]);
        let result = extract_team(None, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Platform admin cannot specify team"));
    }

    #[test]
    fn test_extract_team_admin_only_with_query_still_blocked() {
        // admin:all without org/team scopes blocked even with query param (defense-in-depth)
        let context = test_context(vec!["admin:all"]);
        let result = extract_team(Some("target-team"), &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Platform admin cannot specify team"));
    }

    #[test]
    fn test_extract_team_admin_with_org_scopes_uses_query() {
        // admin:all WITH org scopes can specify teams (e.g., dual-role token)
        let context = test_context(vec!["admin:all", "org:acme:admin", "team:acme-eng:cp:read"]);
        let result = extract_team(Some("acme-eng"), &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "acme-eng");
    }

    #[test]
    fn test_extract_team_admin_with_org_scopes_extracts_from_scope() {
        // admin:all WITH team scopes falls through to scope extraction
        let context = test_context(vec!["admin:all", "team:acme-eng:cp:read"]);
        let result = extract_team(None, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "acme-eng");
    }

    #[test]
    fn test_extract_team_no_team_found() {
        let context = test_context(vec!["some:other:scope"]);
        let result = extract_team(None, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unable to determine team"));
    }

    // -------------------------------------------------------------------------
    // Authorization Tests - CP Scopes
    // -------------------------------------------------------------------------

    #[test]
    fn test_cp_authorization_initialize_no_scope() {
        let context = test_context(vec![]);
        assert!(check_method_authorization("initialize", &context, &CP_SCOPES).is_ok());
        assert!(check_method_authorization("initialized", &context, &CP_SCOPES).is_ok());
        assert!(check_method_authorization("ping", &context, &CP_SCOPES).is_ok());
    }

    #[test]
    fn test_cp_authorization_tools_list_with_read() {
        let context = test_context(vec!["mcp:read"]);
        assert!(check_method_authorization("tools/list", &context, &CP_SCOPES).is_ok());
        assert!(check_method_authorization("resources/list", &context, &CP_SCOPES).is_ok());
    }

    #[test]
    fn test_cp_authorization_tools_list_without_read() {
        let context = test_context(vec![]);
        let result = check_method_authorization("tools/list", &context, &CP_SCOPES);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mcp:read"));
    }

    #[test]
    fn test_cp_authorization_tools_call_with_execute() {
        let context = test_context(vec!["mcp:execute"]);
        assert!(check_method_authorization("tools/call", &context, &CP_SCOPES).is_ok());
    }

    #[test]
    fn test_cp_authorization_tools_call_without_execute() {
        let context = test_context(vec!["mcp:read"]);
        let result = check_method_authorization("tools/call", &context, &CP_SCOPES);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mcp:execute"));
    }

    #[test]
    fn test_cp_authorization_resources_read_with_cp_read() {
        let context = test_context(vec!["cp:read"]);
        assert!(check_method_authorization("resources/read", &context, &CP_SCOPES).is_ok());
    }

    #[test]
    fn test_cp_authorization_resources_read_without_cp_read() {
        let context = test_context(vec!["mcp:read"]);
        let result = check_method_authorization("resources/read", &context, &CP_SCOPES);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cp:read"));
    }

    #[test]
    fn test_cp_authorization_admin_governance_only() {
        let context = test_context(vec!["admin:all"]);
        // admin:all grants tools/list and tools/call (for audit tool access)
        assert!(check_method_authorization("tools/list", &context, &CP_SCOPES).is_ok());
        assert!(check_method_authorization("tools/call", &context, &CP_SCOPES).is_ok());

        // admin:all does NOT grant resources/read, resources/list, prompts, logging
        assert!(check_method_authorization("resources/read", &context, &CP_SCOPES).is_err());
        assert!(check_method_authorization("resources/list", &context, &CP_SCOPES).is_err());
        assert!(check_method_authorization("prompts/list", &context, &CP_SCOPES).is_err());
        assert!(check_method_authorization("prompts/get", &context, &CP_SCOPES).is_err());
        assert!(check_method_authorization("logging/setLevel", &context, &CP_SCOPES).is_err());
    }

    #[test]
    fn test_cp_authorization_admin_still_allows_special_methods() {
        let context = test_context(vec!["admin:all"]);
        // Special methods always allowed (no auth needed)
        assert!(check_method_authorization("initialize", &context, &CP_SCOPES).is_ok());
        assert!(check_method_authorization("initialized", &context, &CP_SCOPES).is_ok());
        assert!(check_method_authorization("ping", &context, &CP_SCOPES).is_ok());
    }

    // -------------------------------------------------------------------------
    // Authorization Tests - API Scopes
    // -------------------------------------------------------------------------

    #[test]
    fn test_api_authorization_tools_list_with_api_read() {
        let context = test_context(vec!["api:read"]);
        assert!(check_method_authorization("tools/list", &context, &API_SCOPES).is_ok());
    }

    #[test]
    fn test_api_authorization_tools_list_with_mcp_read_fails() {
        let context = test_context(vec!["mcp:read"]);
        let result = check_method_authorization("tools/list", &context, &API_SCOPES);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("api:read"));
    }

    #[test]
    fn test_api_authorization_resources_read_not_supported() {
        let context = test_context(vec!["api:read"]);
        let result = check_method_authorization("resources/read", &context, &API_SCOPES);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Resource operations not supported"));
    }

    // -------------------------------------------------------------------------
    // MCP Header Extraction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_extract_mcp_headers_all_present() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-protocol-version", "2025-11-25".parse().unwrap());
        headers.insert("mcp-session-id", "mcp-123".parse().unwrap());
        headers.insert("origin", "http://localhost:3000".parse().unwrap());

        let result = extract_mcp_headers(&headers);
        assert_eq!(result.protocol_version, Some("2025-11-25".to_string()));
        assert_eq!(result.session_id, Some("mcp-123".to_string()));
        assert_eq!(result.origin, Some("http://localhost:3000".to_string()));
    }

    #[test]
    fn test_extract_mcp_headers_partial() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-protocol-version", "2025-11-25".parse().unwrap());

        let result = extract_mcp_headers(&headers);
        assert_eq!(result.protocol_version, Some("2025-11-25".to_string()));
        assert_eq!(result.session_id, None);
        assert_eq!(result.origin, None);
    }

    #[test]
    fn test_extract_mcp_headers_none() {
        let headers = HeaderMap::new();
        let result = extract_mcp_headers(&headers);
        assert_eq!(result.protocol_version, None);
        assert_eq!(result.session_id, None);
        assert_eq!(result.origin, None);
    }

    // -------------------------------------------------------------------------
    // Response Mode Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_determine_response_mode_json_default() {
        assert_eq!(determine_response_mode(None), ResponseMode::Json);
        assert_eq!(determine_response_mode(Some("application/json")), ResponseMode::Json);
    }

    #[test]
    fn test_determine_response_mode_sse() {
        assert_eq!(determine_response_mode(Some("text/event-stream")), ResponseMode::Sse);
    }

    #[test]
    fn test_determine_response_mode_sse_with_other_types() {
        assert_eq!(
            determine_response_mode(Some("text/event-stream, application/json")),
            ResponseMode::Sse
        );
    }

    // -------------------------------------------------------------------------
    // Protocol Version Validation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_validate_protocol_version_exact_match() {
        assert!(validate_protocol_version("2025-11-25").is_ok());
    }

    #[test]
    fn test_validate_protocol_version_mismatch() {
        let result = validate_protocol_version("2024-11-05");
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::UnsupportedProtocolVersion { client, supported } => {
                assert_eq!(client, "2024-11-05");
                assert_eq!(supported, vec!["2025-11-25"]);
            }
            _ => panic!("Expected UnsupportedProtocolVersion error"),
        }
    }

    #[test]
    fn test_validate_protocol_version_empty() {
        let result = validate_protocol_version("");
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // Error Response Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_error_response_json() {
        let response = error_response_json(
            error_codes::INVALID_REQUEST,
            "Test error".to_string(),
            Some(JsonRpcId::String("req-1".to_string())),
        );

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, Some(JsonRpcId::String("req-1".to_string())));
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, error_codes::INVALID_REQUEST);
        assert_eq!(error.message, "Test error");
    }

    #[test]
    fn test_error_response_json_no_id() {
        let response =
            error_response_json(error_codes::PARSE_ERROR, "Parse failed".to_string(), None);

        assert_eq!(response.id, None);
        assert!(response.error.is_some());
    }
}
