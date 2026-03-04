//! Authentication context and error types used by the auth middleware.

use std::collections::HashSet;
use thiserror::Error;

use serde::{Deserialize, Serialize};

use crate::domain::{OrgId, TokenId, UserId};
use crate::errors::Error;

/// Request-scoped authentication context derived from a valid token or session.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub token_id: TokenId,
    pub token_name: String,
    pub user_id: Option<UserId>,
    pub user_email: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    /// Organization ID for this user (if org-scoped)
    pub org_id: Option<OrgId>,
    /// Organization name for this user (if org-scoped)
    pub org_name: Option<String>,
    scopes: HashSet<String>,
    /// Agent context for machine users (None for human users).
    pub agent_context: Option<AgentContext>,
    /// CP-tool grants loaded from DB for cp-tool agents (empty for human users).
    pub cp_grants: Vec<CpGrant>,
}

impl AuthContext {
    pub fn new(token_id: TokenId, token_name: String, scopes: Vec<String>) -> Self {
        Self {
            token_id,
            token_name,
            user_id: None,
            user_email: None,
            client_ip: None,
            user_agent: None,
            org_id: None,
            org_name: None,
            scopes: scopes.into_iter().collect(),
            agent_context: None,
            cp_grants: Vec::new(),
        }
    }

    pub fn with_user(
        token_id: TokenId,
        token_name: String,
        user_id: UserId,
        user_email: String,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            token_id,
            token_name,
            user_id: Some(user_id),
            user_email: Some(user_email),
            client_ip: None,
            user_agent: None,
            org_id: None,
            org_name: None,
            scopes: scopes.into_iter().collect(),
            agent_context: None,
            cp_grants: Vec::new(),
        }
    }

    /// Attach agent context and grants for machine users.
    pub fn with_agent_data(
        mut self,
        agent_context: Option<AgentContext>,
        cp_grants: Vec<CpGrant>,
    ) -> Self {
        self.agent_context = agent_context;
        self.cp_grants = cp_grants;
        self
    }

    /// Set the organization context for this auth context.
    pub fn with_org(mut self, org_id: OrgId, org_name: String) -> Self {
        self.org_id = Some(org_id);
        self.org_name = Some(org_name);
        self
    }

    /// Set the client IP and user agent for this context.
    pub fn with_request_context(
        mut self,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        self.client_ip = client_ip;
        self.user_agent = user_agent;
        self
    }

    /// Extract user context for audit logging.
    ///
    /// Returns a tuple of (user_id, client_ip, user_agent) suitable for
    /// use with `AuditEvent::with_user_context()`.
    pub fn to_audit_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.user_id.as_ref().map(|id| id.to_string()),
            self.client_ip.clone(),
            self.user_agent.clone(),
        )
    }

    /// Extract organization context for audit logging.
    ///
    /// Returns a tuple of (org_id, team_id) suitable for
    /// use with `AuditEvent::with_org_context()`.
    /// Note: team_id is not directly available on AuthContext, so it returns None.
    /// Callers that have team context should set it explicitly.
    pub fn to_org_audit_context(&self) -> (Option<String>, Option<String>) {
        (self.org_id.as_ref().map(|id| id.to_string()), None)
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        if self.scopes.contains(scope) {
            return true;
        }

        // Support wildcard matching for team scopes.
        // If the user has "team:X:*:*", it matches "team:X:resource:action".
        // If the user has "team:X:resource:*", it matches "team:X:resource:action".
        if let Some(rest) = scope.strip_prefix("team:") {
            let parts: Vec<&str> = rest.splitn(3, ':').collect();
            if parts.len() == 3 {
                let team = parts[0];
                let resource = parts[1];
                let wildcard_all = format!("team:{}:*:*", team);
                if self.scopes.contains(&wildcard_all) {
                    return true;
                }
                let wildcard_action = format!("team:{}:{}:*", team, resource);
                if self.scopes.contains(&wildcard_action) {
                    return true;
                }
            }
        }

        false
    }

    pub fn scopes(&self) -> impl Iterator<Item = &String> {
        self.scopes.iter()
    }

    /// Remove all org-scoped permissions from this context.
    ///
    /// Used when an org scope is present but the org cannot be resolved from the database.
    /// This prevents granting org-level permissions for non-existent or unresolvable orgs.
    pub fn strip_org_scopes(mut self) -> Self {
        self.scopes.retain(|s| !s.starts_with("org:"));
        self
    }
}

/// Agent context — determines which category of tools/resources a machine user can access.
/// NULL in the database for human users; non-null for agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentContext {
    /// Control-plane MCP tools (manage clusters, routes, listeners, etc.)
    CpTool,
    /// Gateway API MCP tools (proxy calls to upstream services)
    GatewayTool,
    /// Direct data plane access (route-level permissions via Envoy filters)
    ApiConsumer,
}

impl AgentContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CpTool => "cp-tool",
            Self::GatewayTool => "gateway-tool",
            Self::ApiConsumer => "api-consumer",
        }
    }

    pub fn from_db(s: Option<&str>) -> Option<Self> {
        match s? {
            "cp-tool" => Some(Self::CpTool),
            "gateway-tool" => Some(Self::GatewayTool),
            "api-consumer" => Some(Self::ApiConsumer),
            _ => None,
        }
    }
}

impl std::fmt::Display for AgentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A grant record loaded from the `agent_grants` table.
#[derive(Debug, Clone)]
pub struct AgentGrant {
    pub id: String,
    pub agent_id: String,
    pub org_id: String,
    pub team: String,
    pub grant_type: String,            // "cp-tool" | "gateway-tool" | "route"
    pub resource_type: Option<String>, // for cp-tool grants
    pub action: Option<String>,        // for cp-tool grants
    pub route_id: Option<String>,      // for gateway-tool/route grants
    pub allowed_methods: Option<Vec<String>>, // for gateway-tool/route grants
    pub created_by: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Typed cp-tool grant for cached permission checks.
#[derive(Debug, Clone)]
pub struct CpGrant {
    pub resource_type: String,
    pub action: String,
    pub team: String,
}

/// Typed route/gateway grant for cached permission checks.
#[derive(Debug, Clone)]
pub struct RouteGrant {
    pub route_id: String,
    pub allowed_methods: Vec<String>,
    pub team: String,
}

/// Errors returned by authentication middleware/services.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("unauthorized: bearer token missing")]
    MissingBearer,
    #[error("unauthorized: malformed bearer token")]
    MalformedBearer,
    #[error("unauthorized: token not found")]
    TokenNotFound,
    #[error("unauthorized: token inactive")]
    InactiveToken,
    #[error("unauthorized: token expired")]
    ExpiredToken,
    #[error("forbidden: missing required scope")]
    Forbidden,
    #[error(transparent)]
    Persistence(#[from] Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_context_scope_checks() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["clusters:read".into(), "clusters:write".into()],
        );

        assert!(ctx.has_scope("clusters:read"));
        assert!(!ctx.has_scope("routes:read"));
        assert_eq!(ctx.scopes().count(), 2);
    }

    #[test]
    fn strip_org_scopes_removes_only_org_scopes() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec![
                "org:default:admin".into(),
                "org:acme:member".into(),
                "clusters:read".into(),
                "team:platform-admin:*:*".into(),
            ],
        );

        let stripped = ctx.strip_org_scopes();
        assert!(!stripped.has_scope("org:default:admin"));
        assert!(!stripped.has_scope("org:acme:member"));
        assert!(stripped.has_scope("clusters:read"));
        assert!(stripped.has_scope("team:platform-admin:*:*"));
        assert_eq!(stripped.scopes().count(), 2);
    }

    #[test]
    fn strip_org_scopes_noop_when_no_org_scopes() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["clusters:read".into(), "admin:all".into()],
        );

        let stripped = ctx.strip_org_scopes();
        assert!(stripped.has_scope("clusters:read"));
        assert!(stripped.has_scope("admin:all"));
        assert_eq!(stripped.scopes().count(), 2);
    }

    #[test]
    fn has_scope_wildcard_team_all() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["team:engineering:*:*".into()],
        );

        assert!(ctx.has_scope("team:engineering:clusters:read"));
        assert!(ctx.has_scope("team:engineering:routes:write"));
        assert!(ctx.has_scope("team:engineering:dataplanes:read"));
        assert!(!ctx.has_scope("team:other:clusters:read"));
        assert!(ctx.has_scope("team:engineering:*:*"));
    }

    #[test]
    fn has_scope_wildcard_team_resource() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["team:eng:clusters:*".into()],
        );

        assert!(ctx.has_scope("team:eng:clusters:read"));
        assert!(ctx.has_scope("team:eng:clusters:write"));
        assert!(ctx.has_scope("team:eng:clusters:delete"));
        assert!(!ctx.has_scope("team:eng:routes:read"));
    }

    #[test]
    fn has_scope_no_wildcard_fallback() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["team:eng:clusters:read".into()],
        );

        assert!(ctx.has_scope("team:eng:clusters:read"));
        assert!(!ctx.has_scope("team:eng:clusters:write"));
    }

    #[test]
    fn agent_context_serialization() {
        assert_eq!(AgentContext::CpTool.as_str(), "cp-tool");
        assert_eq!(AgentContext::GatewayTool.as_str(), "gateway-tool");
        assert_eq!(AgentContext::ApiConsumer.as_str(), "api-consumer");
    }

    #[test]
    fn agent_context_from_db() {
        assert_eq!(AgentContext::from_db(Some("cp-tool")), Some(AgentContext::CpTool));
        assert_eq!(AgentContext::from_db(Some("gateway-tool")), Some(AgentContext::GatewayTool));
        assert_eq!(AgentContext::from_db(Some("api-consumer")), Some(AgentContext::ApiConsumer));
        assert_eq!(AgentContext::from_db(None), None);
        assert_eq!(AgentContext::from_db(Some("invalid")), None);
    }

    #[test]
    fn agent_context_serde_roundtrip() {
        let json = serde_json::to_string(&AgentContext::GatewayTool).unwrap();
        assert_eq!(json, "\"gateway-tool\"");
        let parsed: AgentContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AgentContext::GatewayTool);
    }
}
