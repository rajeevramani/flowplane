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

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{JsonRpcError, JsonRpcId, JsonRpcResponse, SUPPORTED_VERSIONS};
use axum::http::HeaderMap;

#[allow(unused_imports)]
use crate::mcp::protocol::error_codes;
use crate::storage::DbPool;

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

/// Extract all team names the caller has explicit scope for.
///
/// Parses all team names from token scopes (patterns like `team:X:resource:action`
/// and `team:X:*:*`). Returns deduplicated vec of team names in order of appearance.
///
/// For `admin:all`-only tokens (no org/team scopes), returns empty vec since
/// governance/audit tools operate without team context. Per-tool-call auth handles
/// resource authorization.
///
/// # Arguments
/// * `context` - Authentication context with token scopes
///
/// # Returns
/// Vec of team names the caller has explicit access to (may be empty)
pub fn extract_teams(context: &AuthContext) -> Vec<String> {
    // admin:all without org/team scopes → governance only, no team context
    if context.has_scope("admin:all") {
        let has_org_or_team_scopes =
            context.scopes().any(|s| s.starts_with("org:") || s.starts_with("team:"));
        if !has_org_or_team_scopes {
            return vec![];
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut teams = Vec::new();

    for scope in context.scopes() {
        if let Some(team_part) = scope.strip_prefix("team:") {
            if let Some(team_name) = team_part.split(':').next() {
                if !team_name.is_empty() && seen.insert(team_name.to_string()) {
                    teams.push(team_name.to_string());
                }
            }
        }
    }

    teams
}

/// Resolve a team name to its UUID.
///
/// After the FK migration, many tables (mcp_tools, aggregated_api_schemas, etc.)
/// store team as a UUID, not a name. This function queries the teams table to
/// convert a team name to its UUID. If the input is already a UUID, it returns it as-is.
///
/// # Arguments
/// * `team_name` - Team name (or UUID) to resolve
/// * `db_pool` - Database connection pool
///
/// # Returns
/// Team UUID string on success, error message on failure
pub async fn resolve_team_id(team_name: &str, db_pool: &DbPool) -> Result<String, String> {
    // If it already looks like a UUID, return as-is
    if uuid::Uuid::parse_str(team_name).is_ok() {
        return Ok(team_name.to_string());
    }

    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM teams WHERE name = $1")
        .bind(team_name)
        .fetch_optional(db_pool)
        .await
        .map_err(|e| format!("Failed to resolve team name '{}': {}", team_name, e))?;

    row.map(|r| r.0).ok_or_else(|| format!("Team '{}' not found", team_name))
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

/// Validate MCP protocol version
///
/// Accepts any version listed in SUPPORTED_VERSIONS. Clients should prefer the
/// current PROTOCOL_VERSION, but older supported versions are also accepted.
///
/// # Arguments
/// * `version` - Protocol version string from client (e.g., "2025-11-25")
///
/// # Returns
/// - `Ok(())` if version is in SUPPORTED_VERSIONS
/// - `Err(McpError::UnsupportedProtocolVersion)` otherwise
pub fn validate_protocol_version(version: &str) -> Result<(), McpError> {
    if SUPPORTED_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(McpError::UnsupportedProtocolVersion {
            client: version.to_string(),
            supported: SUPPORTED_VERSIONS.iter().map(|v| v.to_string()).collect(),
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
    // extract_teams() Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_extract_teams_single_team() {
        let context = test_context(vec!["team:acme-corp:mcp:read"]);
        let teams = extract_teams(&context);
        assert_eq!(teams, vec!["acme-corp".to_string()]);
    }

    #[test]
    fn test_extract_teams_multiple_teams() {
        let context = test_context(vec![
            "team:team-a:cp:read",
            "team:team-a:cp:create",
            "team:team-b:cp:read",
        ]);
        let teams = extract_teams(&context);
        assert_eq!(teams.len(), 2);
        assert!(teams.contains(&"team-a".to_string()));
        assert!(teams.contains(&"team-b".to_string()));
    }

    #[test]
    fn test_extract_teams_deduplicates() {
        let context =
            test_context(vec!["team:acme:cp:read", "team:acme:cp:create", "team:acme:api:read"]);
        let teams = extract_teams(&context);
        assert_eq!(teams, vec!["acme".to_string()]);
    }

    #[test]
    fn test_extract_teams_admin_only_returns_empty() {
        let context = test_context(vec!["admin:all"]);
        let teams = extract_teams(&context);
        assert!(teams.is_empty());
    }

    #[test]
    fn test_extract_teams_admin_with_team_scopes() {
        let context =
            test_context(vec!["admin:all", "team:eng:cp:read", "team:platform:cp:create"]);
        let teams = extract_teams(&context);
        assert_eq!(teams.len(), 2);
        assert!(teams.contains(&"eng".to_string()));
        assert!(teams.contains(&"platform".to_string()));
    }

    #[test]
    fn test_extract_teams_no_team_scopes_returns_empty() {
        let context = test_context(vec!["some:other:scope"]);
        let teams = extract_teams(&context);
        assert!(teams.is_empty());
    }

    #[test]
    fn test_extract_teams_empty_scopes_returns_empty() {
        let context = test_context(vec![]);
        let teams = extract_teams(&context);
        assert!(teams.is_empty());
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
        // Both supported versions must be accepted
        assert!(validate_protocol_version("2025-11-25").is_ok());
        assert!(validate_protocol_version("2025-03-26").is_ok());
    }

    #[test]
    fn test_validate_protocol_version_2025_03_26_accepted() {
        assert!(validate_protocol_version("2025-03-26").is_ok());
    }

    #[test]
    fn test_validate_protocol_version_mismatch() {
        let result = validate_protocol_version("2024-11-05");
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::UnsupportedProtocolVersion { client, supported } => {
                assert_eq!(client, "2024-11-05");
                // Both supported versions must appear in the error
                assert!(supported.contains(&"2025-11-25".to_string()));
                assert!(supported.contains(&"2025-03-26".to_string()));
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
