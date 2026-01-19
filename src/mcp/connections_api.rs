//! MCP Connections Management API
//!
//! Provides REST endpoints for listing and monitoring active MCP connections and sessions.
//! Supports both SSE (real-time streaming) connections and HTTP-only sessions.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tracing::debug;

use crate::api::routes::ApiState;
use crate::auth::models::AuthContext;
use crate::mcp::protocol::{ConnectionInfo, ConnectionType, ConnectionsListResult};

/// Query parameters for connections list endpoint
#[derive(Debug, Deserialize)]
pub struct ConnectionsQuery {
    pub team: Option<String>,
}

/// Extract team name from query or auth context
fn extract_team(query: &ConnectionsQuery, context: &AuthContext) -> Result<String, String> {
    // Query parameter takes priority
    if let Some(team) = &query.team {
        return Ok(team.clone());
    }

    // Admin must provide team
    if context.has_scope("admin:all") {
        return Err("Admin users must specify team via query parameter".to_string());
    }

    // Extract from scopes (pattern: team:{name}:*)
    for scope in context.scopes() {
        if let Some(team_part) = scope.strip_prefix("team:") {
            if let Some(team_name) = team_part.split(':').next() {
                return Ok(team_name.to_string());
            }
        }
    }

    Err("Unable to determine team. Please provide team via query parameter".to_string())
}

/// GET /api/v1/mcp/connections
///
/// List active MCP connections and sessions for a team.
///
/// This endpoint returns both:
/// - **SSE connections**: Real-time streaming connections established via `/api/v1/mcp/sse`
/// - **HTTP sessions**: Stateless HTTP-only sessions from clients using `/api/v1/mcp`
///
/// # Authentication
/// Requires a valid bearer token with `mcp:read` scope.
///
/// # Query Parameters
/// - `team`: Optional team name. Required for admin users.
///
/// # Response
/// Returns a list of active connections and sessions with metadata including client info,
/// protocol version, initialization status, and connection type.
#[utoipa::path(
    get,
    path = "/api/v1/mcp/connections",
    params(
        ("team" = Option<String>, Query, description = "Team name (required for admin users)")
    ),
    responses(
        (status = 200, description = "List of active connections and sessions", body = ConnectionsListResult),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    ),
    tag = "MCP Protocol"
)]
pub async fn list_connections_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<ConnectionsQuery>,
) -> Result<Json<ConnectionsListResult>, (StatusCode, String)> {
    // Check authorization
    if !context.has_scope("mcp:read") && !context.has_scope("admin:all") {
        return Err((StatusCode::FORBIDDEN, "Missing required scope 'mcp:read'".to_string()));
    }

    // Extract team
    let team = extract_team(&query, &context).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    debug!(team = %team, "Listing MCP connections and sessions");

    // Get SSE connections
    let connection_manager = &state.mcp_connection_manager;
    let mut connections = connection_manager.list_team_connections(&team);

    // Get HTTP sessions and convert to ConnectionInfo
    let session_manager = &state.mcp_session_manager;
    let http_sessions = session_manager.list_sessions_by_team(&team);

    // Reference timestamp for converting Instant to DateTime
    let now_instant = std::time::Instant::now();
    let now_utc = Utc::now();

    for (session_id, session) in http_sessions {
        // Convert Instant timestamps to DateTime<Utc>
        let created_elapsed = now_instant.saturating_duration_since(session.created_at);
        let activity_elapsed = now_instant.saturating_duration_since(session.last_activity);

        let created_at: DateTime<Utc> =
            now_utc - chrono::Duration::from_std(created_elapsed).unwrap_or_default();
        let last_activity: DateTime<Utc> =
            now_utc - chrono::Duration::from_std(activity_elapsed).unwrap_or_default();

        connections.push(ConnectionInfo {
            connection_id: session_id,
            team: session.team.unwrap_or_else(|| team.clone()),
            created_at: created_at.to_rfc3339(),
            last_activity: last_activity.to_rfc3339(),
            log_level: format!("{:?}", session.log_level).to_lowercase(),
            client_info: session.client_info,
            protocol_version: session.protocol_version,
            initialized: session.initialized,
            connection_type: ConnectionType::Http,
        });
    }

    let total_count = connections.len();

    debug!(team = %team, sse_count = connections.iter().filter(|c| c.connection_type == ConnectionType::Sse).count(),
           http_count = connections.iter().filter(|c| c.connection_type == ConnectionType::Http).count(),
           "Listed MCP connections");

    Ok(Json(ConnectionsListResult { connections, total_count }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TokenId;

    fn create_test_context_with_scopes(scopes: Vec<String>) -> AuthContext {
        AuthContext::new(TokenId::from_str_unchecked("token-1"), "test-token".to_string(), scopes)
    }

    #[test]
    fn test_extract_team_from_query() {
        let query = ConnectionsQuery { team: Some("test-team".to_string()) };
        let context = create_test_context_with_scopes(vec![]);

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-team");
    }

    #[test]
    fn test_extract_team_from_scope() {
        let query = ConnectionsQuery { team: None };
        let context = create_test_context_with_scopes(vec!["team:my-team:mcp:read".to_string()]);

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my-team");
    }

    #[test]
    fn test_extract_team_admin_requires_query() {
        let query = ConnectionsQuery { team: None };
        let context = create_test_context_with_scopes(vec!["admin:all".to_string()]);

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Admin users must specify team"));
    }

    #[test]
    fn test_extract_team_query_overrides_scope() {
        let query = ConnectionsQuery { team: Some("override-team".to_string()) };
        let context = create_test_context_with_scopes(vec!["team:scope-team:mcp:read".to_string()]);

        let result = extract_team(&query, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "override-team");
    }

    #[test]
    fn test_extract_team_no_team_no_scope() {
        let query = ConnectionsQuery { team: None };
        let context = create_test_context_with_scopes(vec!["mcp:read".to_string()]);

        let result = extract_team(&query, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unable to determine team"));
    }
}
