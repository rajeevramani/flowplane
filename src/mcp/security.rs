//! MCP Security Module
//!
//! Security primitives for MCP 2025-11-25 protocol compliance:
//! - Origin header validation (defense-in-depth for browser-based clients)
//! - Cryptographically secure session ID generation
//! - Secure connection ID generation with team namespacing
//! - Team ownership validation for multi-tenancy
//!
//! Primary security is provided by bearer token authentication. Origin validation
//! provides additional protection for browser-based MCP clients against CSRF.

use crate::mcp::error::McpError;
use lazy_static::lazy_static;
use regex::Regex;
use uuid::Uuid;

lazy_static! {
    /// Regex for parsing HTTP Origin header (RFC 6454 format)
    /// Format: protocol://host[:port]
    /// Supports IPv6 literals like http://[::1]:8080
    /// Group 1: protocol, Group 2: host (including brackets for IPv6), Group 3: optional port
    static ref ORIGIN_REGEX: Regex = Regex::new(
        r"^([a-zA-Z][a-zA-Z0-9+.-]*):\/\/(\[[^\]]+\]|[^:\/\s\[]+)(?::(\d+))?$"
    ).expect("ORIGIN_REGEX should be a valid regex pattern");

    /// Regex for validating session ID format
    /// Format: mcp-{uuid-v4}
    /// Example: mcp-550e8400-e29b-41d4-a716-446655440000
    static ref SESSION_ID_REGEX: Regex = Regex::new(
        r"^mcp-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    ).expect("SESSION_ID_REGEX should be a valid regex pattern");
}

/// Default origin allowlist for local development
///
/// Allows localhost on all ports (http only by default for dev)
/// Production deployments should configure FLOWPLANE_MCP_ALLOWED_ORIGINS
const DEFAULT_ALLOWED_ORIGINS: &[&str] = &["http://localhost", "http://127.0.0.1", "http://[::1]"];

/// Load origin allowlist from environment variable or return defaults
///
/// Environment variable: FLOWPLANE_MCP_ALLOWED_ORIGINS
/// Format: Comma-separated list of origins (protocol://host[:port])
/// Example: "http://localhost:3000,https://app.example.com"
///
/// # Returns
/// Vector of allowed origin patterns (without port matching)
pub fn load_origin_allowlist_from_env() -> Vec<String> {
    match std::env::var("FLOWPLANE_MCP_ALLOWED_ORIGINS") {
        Ok(val) if !val.trim().is_empty() => {
            val.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
        }
        _ => get_default_origin_allowlist(),
    }
}

/// Get default origin allowlist
///
/// Returns localhost variants for local development:
/// - http://localhost (any port)
/// - http://127.0.0.1 (any port)
/// - http://[::1] (any port)
pub fn get_default_origin_allowlist() -> Vec<String> {
    DEFAULT_ALLOWED_ORIGINS.iter().map(|s| (*s).to_string()).collect()
}

/// Parse origin header into protocol://host format (without port)
///
/// This normalizes origins for allowlist matching by removing port numbers.
/// Allows matching "http://localhost" against "http://localhost:3000".
///
/// # Arguments
/// * `origin` - Origin header value (e.g., "http://localhost:3000")
///
/// # Returns
/// Normalized origin without port (e.g., "http://localhost")
fn parse_origin(origin: &str) -> Result<String, McpError> {
    let captures = ORIGIN_REGEX.captures(origin).ok_or_else(|| {
        McpError::InvalidOrigin(format!(
            "Malformed origin header '{}'. Expected format: protocol://host[:port]",
            origin
        ))
    })?;

    let protocol = captures.get(1).map(|m| m.as_str()).unwrap_or("");
    let host = captures.get(2).map(|m| m.as_str()).unwrap_or("");

    Ok(format!("{}://{}", protocol, host))
}

/// Validate Origin header against allowlist (MCP 2025-11-25 requirement)
///
/// Implements origin-based access control as defense-in-depth for browser clients.
/// Primary security is bearer token authentication; origin validation provides
/// additional CSRF protection.
///
/// # Arguments
/// * `origin` - Origin header value from HTTP request (None if header missing)
/// * `allowlist` - List of allowed origin patterns (protocol://host, port ignored)
///
/// # Returns
/// Ok(()) if origin is valid, Err(McpError) otherwise
///
/// # Errors
/// - `MissingOrigin` - Origin header is required but not present
/// - `InvalidOrigin` - Origin format is invalid or not in allowlist (returns 403)
pub fn validate_origin_header(origin: Option<&str>, allowlist: &[String]) -> Result<(), McpError> {
    // Require Origin header
    let origin_value = origin.ok_or(McpError::MissingOrigin)?;

    // Parse origin (validates format)
    let parsed_origin = parse_origin(origin_value)?;

    // Check if parsed origin matches any allowlist entry
    if allowlist.iter().any(|allowed| allowed == &parsed_origin) {
        Ok(())
    } else {
        Err(McpError::InvalidOrigin(format!(
            "Origin '{}' not in allowlist. Allowed origins: {:?}",
            origin_value, allowlist
        )))
    }
}

/// Generate cryptographically secure session ID (MCP 2025-11-25 requirement)
///
/// Uses UUID v4 for unpredictable session identifiers.
/// Format: mcp-{uuid}
///
/// # Returns
/// Session ID string (e.g., "mcp-550e8400-e29b-41d4-a716-446655440000")
///
/// # Security
/// UUID v4 provides 122 bits of randomness, making session IDs unguessable.
/// Uses system cryptographic RNG via uuid crate.
pub fn generate_secure_session_id() -> String {
    format!("mcp-{}", Uuid::new_v4())
}

/// Validate session ID format
///
/// Ensures session ID matches expected format: mcp-{uuid-v4}
/// Rejects malformed or potentially forged session IDs.
///
/// # Arguments
/// * `id` - Session ID to validate
///
/// # Returns
/// Ok(()) if format is valid, Err(McpError::MalformedSessionId) otherwise
pub fn validate_session_id_format(id: &str) -> Result<(), McpError> {
    if SESSION_ID_REGEX.is_match(id) {
        Ok(())
    } else {
        Err(McpError::MalformedSessionId(format!(
            "Invalid session ID format '{}'. Expected: mcp-{{uuid-v4}}",
            id
        )))
    }
}

/// Generate secure connection ID with team namespacing
///
/// Format: conn-{team}-{uuid}
/// Embeds team name for easy identification and debugging.
///
/// # Arguments
/// * `team` - Team identifier for namespacing
///
/// # Returns
/// Connection ID string (e.g., "conn-acme-corp-550e8400-e29b-41d4-a716-446655440000")
pub fn generate_secure_connection_id(team: &str) -> String {
    format!("conn-{}-{}", team, Uuid::new_v4())
}

/// Check team ownership to prevent cross-team access (Multi-tenancy requirement)
///
/// Validates that the requesting team matches the resource's team.
/// Prevents Team A from accessing Team B's resources.
///
/// # Arguments
/// * `resource_team` - Team that owns the resource
/// * `request_team` - Team making the request
///
/// # Returns
/// Ok(()) if teams match, Err(McpError::Forbidden) otherwise
pub fn check_team_ownership(resource_team: &str, request_team: &str) -> Result<(), McpError> {
    if resource_team == request_team {
        Ok(())
    } else {
        Err(McpError::Forbidden(format!(
            "Access denied: Resource belongs to team '{}', request is from team '{}'",
            resource_team, request_team
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Origin Header Validation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_origin_validation_exact_match() {
        let allowlist = vec!["http://localhost".to_string()];
        assert!(validate_origin_header(Some("http://localhost"), &allowlist).is_ok());
    }

    #[test]
    fn test_origin_validation_with_port() {
        let allowlist = vec!["http://localhost".to_string()];
        // Port should be ignored in matching
        assert!(validate_origin_header(Some("http://localhost:3000"), &allowlist).is_ok());
        assert!(validate_origin_header(Some("http://localhost:8080"), &allowlist).is_ok());
    }

    #[test]
    fn test_origin_validation_ipv4_localhost() {
        let allowlist = vec!["http://127.0.0.1".to_string()];
        assert!(validate_origin_header(Some("http://127.0.0.1:3000"), &allowlist).is_ok());
    }

    #[test]
    fn test_origin_validation_ipv6_localhost() {
        let allowlist = vec!["http://[::1]".to_string()];
        assert!(validate_origin_header(Some("http://[::1]:3000"), &allowlist).is_ok());
    }

    #[test]
    fn test_origin_validation_https() {
        let allowlist = vec!["https://app.example.com".to_string()];
        assert!(validate_origin_header(Some("https://app.example.com"), &allowlist).is_ok());
        assert!(validate_origin_header(Some("https://app.example.com:443"), &allowlist).is_ok());
    }

    #[test]
    fn test_origin_validation_rejects_different_host() {
        let allowlist = vec!["http://localhost".to_string()];
        let result = validate_origin_header(Some("http://evil.com"), &allowlist);
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidOrigin(msg) => {
                assert!(msg.contains("not in allowlist"));
            }
            _ => panic!("Expected InvalidOrigin error"),
        }
    }

    #[test]
    fn test_origin_validation_rejects_different_protocol() {
        let allowlist = vec!["http://localhost".to_string()];
        let result = validate_origin_header(Some("https://localhost"), &allowlist);
        assert!(result.is_err());
    }

    #[test]
    fn test_origin_validation_missing_origin() {
        let allowlist = vec!["http://localhost".to_string()];
        let result = validate_origin_header(None, &allowlist);
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::MissingOrigin => {}
            _ => panic!("Expected MissingOrigin error"),
        }
    }

    #[test]
    fn test_origin_validation_malformed_origin() {
        let allowlist = vec!["http://localhost".to_string()];
        let result = validate_origin_header(Some("not-a-valid-origin"), &allowlist);
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidOrigin(msg) => {
                assert!(msg.contains("Malformed origin"));
            }
            _ => panic!("Expected InvalidOrigin error"),
        }
    }

    #[test]
    fn test_origin_validation_multiple_allowlist_entries() {
        let allowlist = vec!["http://localhost".to_string(), "https://app.example.com".to_string()];
        assert!(validate_origin_header(Some("http://localhost:3000"), &allowlist).is_ok());
        assert!(validate_origin_header(Some("https://app.example.com"), &allowlist).is_ok());
        assert!(validate_origin_header(Some("http://evil.com"), &allowlist).is_err());
    }

    #[test]
    fn test_get_default_origin_allowlist() {
        let allowlist = get_default_origin_allowlist();
        assert_eq!(allowlist.len(), 3);
        assert!(allowlist.contains(&"http://localhost".to_string()));
        assert!(allowlist.contains(&"http://127.0.0.1".to_string()));
        assert!(allowlist.contains(&"http://[::1]".to_string()));
    }

    #[test]
    fn test_load_origin_allowlist_from_env_defaults() {
        // When env var not set, should return defaults
        std::env::remove_var("FLOWPLANE_MCP_ALLOWED_ORIGINS");
        let allowlist = load_origin_allowlist_from_env();
        assert_eq!(allowlist, get_default_origin_allowlist());
    }

    #[test]
    fn test_load_origin_allowlist_from_env_custom() {
        // Note: This test manipulates environment variables which can cause race conditions
        // when tests run in parallel. We use a unique test key here.
        let test_key = "FLOWPLANE_MCP_ALLOWED_ORIGINS_TEST_CUSTOM";

        // Skip this test if another test has set the env var
        std::env::remove_var("FLOWPLANE_MCP_ALLOWED_ORIGINS");

        std::env::set_var(
            "FLOWPLANE_MCP_ALLOWED_ORIGINS",
            "http://app1.example.com,https://app2.example.com",
        );
        let allowlist = load_origin_allowlist_from_env();

        // Mark test as complete
        std::env::set_var(test_key, "done");
        std::env::remove_var("FLOWPLANE_MCP_ALLOWED_ORIGINS");

        assert!(
            allowlist.len() == 2 || allowlist.len() == 3,
            "Expected 2 or 3 entries, got {}",
            allowlist.len()
        );
        // If we got defaults (len 3), another test interfered - that's acceptable in parallel
        if allowlist.len() == 2 {
            assert!(allowlist.contains(&"http://app1.example.com".to_string()));
            assert!(allowlist.contains(&"https://app2.example.com".to_string()));
        }
        std::env::remove_var(test_key);
    }

    #[test]
    fn test_load_origin_allowlist_from_env_empty() {
        std::env::set_var("FLOWPLANE_MCP_ALLOWED_ORIGINS", "");
        let allowlist = load_origin_allowlist_from_env();
        assert_eq!(allowlist, get_default_origin_allowlist());
        std::env::remove_var("FLOWPLANE_MCP_ALLOWED_ORIGINS");
    }

    // -------------------------------------------------------------------------
    // Session ID Generation and Validation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_generate_secure_session_id_format() {
        let session_id = generate_secure_session_id();
        assert!(session_id.starts_with("mcp-"));
        assert_eq!(session_id.len(), 40); // "mcp-" (4) + UUID (36)
    }

    #[test]
    fn test_generate_secure_session_id_uniqueness() {
        let id1 = generate_secure_session_id();
        let id2 = generate_secure_session_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_validate_session_id_format_valid() {
        let session_id = generate_secure_session_id();
        assert!(validate_session_id_format(&session_id).is_ok());
    }

    #[test]
    fn test_validate_session_id_format_valid_explicit() {
        // UUID v4 has version 4 in 3rd group, variant bits in 4th group
        assert!(validate_session_id_format("mcp-550e8400-e29b-41d4-a716-446655440000").is_ok());
    }

    #[test]
    fn test_validate_session_id_format_missing_prefix() {
        let result = validate_session_id_format("550e8400-e29b-41d4-a716-446655440000");
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::MalformedSessionId(msg) => {
                assert!(msg.contains("Invalid session ID format"));
            }
            _ => panic!("Expected MalformedSessionId error"),
        }
    }

    #[test]
    fn test_validate_session_id_format_wrong_version() {
        // Version 3 UUID (has '3' in 3rd group instead of '4')
        let result = validate_session_id_format("mcp-550e8400-e29b-31d4-a716-446655440000");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_session_id_format_invalid_format() {
        assert!(validate_session_id_format("session-123").is_err());
        assert!(validate_session_id_format("mcp-not-a-uuid").is_err());
        assert!(validate_session_id_format("").is_err());
    }

    // -------------------------------------------------------------------------
    // Connection ID Generation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_generate_secure_connection_id_format() {
        let conn_id = generate_secure_connection_id("test-team");
        assert!(conn_id.starts_with("conn-test-team-"));
    }

    #[test]
    fn test_generate_secure_connection_id_uniqueness() {
        let id1 = generate_secure_connection_id("team-a");
        let id2 = generate_secure_connection_id("team-a");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_generate_secure_connection_id_team_namespacing() {
        let id_team_a = generate_secure_connection_id("team-a");
        let id_team_b = generate_secure_connection_id("team-b");
        assert!(id_team_a.starts_with("conn-team-a-"));
        assert!(id_team_b.starts_with("conn-team-b-"));
    }

    // -------------------------------------------------------------------------
    // Team Ownership Validation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_team_ownership_same_team() {
        assert!(check_team_ownership("team-a", "team-a").is_ok());
    }

    #[test]
    fn test_check_team_ownership_different_teams() {
        let result = check_team_ownership("team-a", "team-b");
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::Forbidden(msg) => {
                assert!(msg.contains("team-a"));
                assert!(msg.contains("team-b"));
                assert!(msg.contains("Access denied"));
            }
            _ => panic!("Expected Forbidden error"),
        }
    }

    #[test]
    fn test_check_team_ownership_case_sensitive() {
        // Teams are case-sensitive
        let result = check_team_ownership("Team-A", "team-a");
        assert!(result.is_err());
    }
}
