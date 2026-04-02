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
    /// Display name from OIDC profile (name claim or userinfo endpoint).
    pub user_name: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    /// Organization ID for this user (if org-scoped)
    pub org_id: Option<OrgId>,
    /// Organization name for this user (if org-scoped)
    pub org_name: Option<String>,
    /// Org-level scopes derived from org_memberships (e.g., "admin:all", "org:acme:admin").
    /// These are NOT resource grants — they encode org role membership.
    org_scopes: HashSet<String>,
    /// Unified grants loaded from the `grants` table (resource + gateway-tool + route).
    pub grants: Vec<Grant>,
    /// Agent context for machine users (None for human users).
    pub agent_context: Option<AgentContext>,
}

impl AuthContext {
    pub fn new(token_id: TokenId, token_name: String, org_scopes: Vec<String>) -> Self {
        Self {
            token_id,
            token_name,
            user_id: None,
            user_email: None,
            user_name: None,
            client_ip: None,
            user_agent: None,
            org_id: None,
            org_name: None,
            org_scopes: org_scopes.into_iter().collect(),
            grants: Vec::new(),
            agent_context: None,
        }
    }

    pub fn with_user(
        token_id: TokenId,
        token_name: String,
        user_id: UserId,
        user_email: String,
        org_scopes: Vec<String>,
    ) -> Self {
        Self {
            token_id,
            token_name,
            user_id: Some(user_id),
            user_email: Some(user_email),
            user_name: None,
            client_ip: None,
            user_agent: None,
            org_id: None,
            org_name: None,
            org_scopes: org_scopes.into_iter().collect(),
            grants: Vec::new(),
            agent_context: None,
        }
    }

    /// Attach agent context and grants for all users (human + machine).
    pub fn with_grants(mut self, grants: Vec<Grant>, agent_context: Option<AgentContext>) -> Self {
        self.grants = grants;
        self.agent_context = agent_context;
        self
    }

    /// Set the organization context for this auth context.
    pub fn with_org(mut self, org_id: OrgId, org_name: String) -> Self {
        self.org_id = Some(org_id);
        self.org_name = Some(org_name);
        self
    }

    /// Set the user's display name (from OIDC name claim or userinfo endpoint).
    pub fn with_user_name(mut self, name: String) -> Self {
        if !name.is_empty() {
            self.user_name = Some(name);
        }
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

    /// Check if the user has an org-level scope (e.g., "admin:all", "org:acme:admin").
    ///
    /// This checks org_scopes only — NOT resource grants.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.org_scopes.contains(scope)
    }

    /// Iterate over org-level scopes.
    pub fn org_scopes(&self) -> impl Iterator<Item = &String> {
        self.org_scopes.iter()
    }

    /// Check if the user has a resource grant matching (resource, action) for a specific team.
    ///
    /// `team_name` is the team name (not UUID) — grants store both team_id and team_name.
    pub fn has_grant(&self, resource: &str, action: &str, team_name: &str) -> bool {
        self.grants.iter().any(|g| {
            g.grant_type == GrantType::Resource
                && g.resource_type.as_deref() == Some(resource)
                && g.action.as_deref() == Some(action)
                && g.team_name == team_name
        })
    }

    /// Check if the user has a resource grant matching (resource, action) for ANY team.
    pub fn has_any_grant(&self, resource: &str, action: &str) -> bool {
        self.grants.iter().any(|g| {
            g.grant_type == GrantType::Resource
                && g.resource_type.as_deref() == Some(resource)
                && g.action.as_deref() == Some(action)
        })
    }

    /// Get unique team names from resource grants.
    pub fn grant_team_names(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut teams = Vec::new();
        for g in &self.grants {
            if g.grant_type == GrantType::Resource && seen.insert(g.team_name.clone()) {
                teams.push(g.team_name.clone());
            }
        }
        teams.sort();
        teams
    }

    /// Get unique team IDs from resource grants.
    pub fn grant_team_ids(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for g in &self.grants {
            if g.grant_type == GrantType::Resource && seen.insert(g.team_id.clone()) {
                ids.push(g.team_id.clone());
            }
        }
        ids
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

/// Unified grant loaded from the `grants` table.
///
/// Covers all grant types: resource (CP tools + human permissions),
/// gateway-tool, and route (API consumer).
#[derive(Debug, Clone)]
pub struct Grant {
    pub grant_type: GrantType,
    pub team_id: String,   // UUID FK to teams.id
    pub team_name: String, // team name (for comparison in check_resource_access)
    // Resource grants
    pub resource_type: Option<String>,
    pub action: Option<String>,
    // Route/gateway grants
    pub route_id: Option<String>,
    pub allowed_methods: Vec<String>,
}

/// Grant type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantType {
    /// CP resource grants (clusters, routes, etc.) — for both humans and agents
    Resource,
    /// Gateway-tool grants — agent-only (MCP gateway tool access)
    GatewayTool,
    /// Route grants — agent-only (direct data plane access via Envoy RBAC)
    Route,
}

impl GrantType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "resource" => Some(Self::Resource),
            "gateway-tool" => Some(Self::GatewayTool),
            "route" => Some(Self::Route),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resource => "resource",
            Self::GatewayTool => "gateway-tool",
            Self::Route => "route",
        }
    }
}

/// A grant record loaded from the `grants` table (full row, for API responses).
#[derive(Debug, Clone)]
pub struct AgentGrant {
    pub id: String,
    pub principal_id: String,
    pub org_id: String,
    pub team_id: String,
    pub grant_type: String,            // "resource" | "gateway-tool" | "route"
    pub resource_type: Option<String>, // for resource grants
    pub action: Option<String>,        // for resource grants
    pub route_id: Option<String>,      // for gateway-tool/route grants
    pub allowed_methods: Option<Vec<String>>, // for gateway-tool/route grants
    pub created_by: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
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
    fn auth_context_org_scope_checks() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["admin:all".into(), "org:acme:admin".into()],
        );

        assert!(ctx.has_scope("admin:all"));
        assert!(ctx.has_scope("org:acme:admin"));
        assert!(!ctx.has_scope("clusters:read")); // resource scopes are grants now
        assert_eq!(ctx.org_scopes().count(), 2);
    }

    #[test]
    fn has_grant_checks() {
        let mut ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["org:acme:member".into()],
        );
        ctx.grants = vec![
            Grant {
                grant_type: GrantType::Resource,
                team_id: "team-uuid-1".into(),
                team_name: "engineering".into(),
                resource_type: Some("clusters".into()),
                action: Some("read".into()),
                route_id: None,
                allowed_methods: vec![],
            },
            Grant {
                grant_type: GrantType::Resource,
                team_id: "team-uuid-1".into(),
                team_name: "engineering".into(),
                resource_type: Some("routes".into()),
                action: Some("create".into()),
                route_id: None,
                allowed_methods: vec![],
            },
        ];

        assert!(ctx.has_grant("clusters", "read", "engineering"));
        assert!(ctx.has_grant("routes", "create", "engineering"));
        assert!(!ctx.has_grant("clusters", "create", "engineering"));
        assert!(!ctx.has_grant("clusters", "read", "other-team"));
        assert!(ctx.has_any_grant("clusters", "read"));
        assert!(!ctx.has_any_grant("clusters", "delete"));
    }

    #[test]
    fn grant_team_names_deduplicates() {
        let mut ctx =
            AuthContext::new(TokenId::from_string("token-1".to_string()), "demo".into(), vec![]);
        ctx.grants = vec![
            Grant {
                grant_type: GrantType::Resource,
                team_id: "t1".into(),
                team_name: "engineering".into(),
                resource_type: Some("clusters".into()),
                action: Some("read".into()),
                route_id: None,
                allowed_methods: vec![],
            },
            Grant {
                grant_type: GrantType::Resource,
                team_id: "t1".into(),
                team_name: "engineering".into(),
                resource_type: Some("routes".into()),
                action: Some("read".into()),
                route_id: None,
                allowed_methods: vec![],
            },
            Grant {
                grant_type: GrantType::Resource,
                team_id: "t2".into(),
                team_name: "payments".into(),
                resource_type: Some("clusters".into()),
                action: Some("read".into()),
                route_id: None,
                allowed_methods: vec![],
            },
        ];

        assert_eq!(ctx.grant_team_names(), vec!["engineering", "payments"]);
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

    #[test]
    fn grant_type_roundtrip() {
        assert_eq!(GrantType::parse("resource"), Some(GrantType::Resource));
        assert_eq!(GrantType::parse("gateway-tool"), Some(GrantType::GatewayTool));
        assert_eq!(GrantType::parse("route"), Some(GrantType::Route));
        assert_eq!(GrantType::parse("invalid"), None);
        assert_eq!(GrantType::Resource.as_str(), "resource");
    }
}
