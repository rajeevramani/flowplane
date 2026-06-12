//! Authorization vocabulary (spec/05 §3, spec/10 §4).
//!
//! [`Resource`] and [`Action`] form the closed grant vocabulary. Surface declarations
//! (REST routes, MCP tools, CLI commands) each carry a `(Resource, Action)` pair, and the
//! grant table stores the same pair — one vocabulary, no phantom entries (kills spec/02 §7.6).
//! The decision engine itself lives in `fp-core::authz`.

use crate::error::{DomainError, DomainResult};
use crate::id::{OrgId, TeamId};
use serde::{Deserialize, Serialize};

/// A team reference carrying its owning org, as resolved from the database. The
/// authorization engine takes this (not a bare TeamId) so the cross-org check is part of
/// every decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamRef {
    pub id: TeamId,
    pub org_id: OrgId,
}

/// Every authorizable resource kind. The enum grows as slices add subsystems; variants are
/// never removed or renamed (grant rows reference them by wire string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resource {
    // -- Governance resources (platform scope; spec/05 §3.1 step 1) --
    Organizations,
    Users,
    Teams,
    Audit,
    Platform,
    // -- Tenant resources (team scope) --
    Clusters,
    RouteConfigs,
    Listeners,
    Filters,
    Secrets,
    Dataplanes,
    ProxyCertificates,
    Agents,
    Grants,
    ApiDefinitions,
    LearningSessions,
    McpTools,
    RateLimits,
    AiProviders,
    AiRoutes,
    AiBudgets,
    Stats,
}

impl Resource {
    /// Governance resources are manageable by the platform admin and readable by org-scoped
    /// callers; they are NEVER team-scoped. Everything else is a tenant resource, invisible
    /// to a pure platform-admin context (invariant 1, spec/05 §3.2).
    pub fn is_governance(self) -> bool {
        matches!(
            self,
            Self::Organizations | Self::Users | Self::Teams | Self::Audit | Self::Platform
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Organizations => "organizations",
            Self::Users => "users",
            Self::Teams => "teams",
            Self::Audit => "audit",
            Self::Platform => "platform",
            Self::Clusters => "clusters",
            Self::RouteConfigs => "route-configs",
            Self::Listeners => "listeners",
            Self::Filters => "filters",
            Self::Secrets => "secrets",
            Self::Dataplanes => "dataplanes",
            Self::ProxyCertificates => "proxy-certificates",
            Self::Agents => "agents",
            Self::Grants => "grants",
            Self::ApiDefinitions => "api-definitions",
            Self::LearningSessions => "learning-sessions",
            Self::McpTools => "mcp-tools",
            Self::RateLimits => "rate-limits",
            Self::AiProviders => "ai-providers",
            Self::AiRoutes => "ai-routes",
            Self::AiBudgets => "ai-budgets",
            Self::Stats => "stats",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        ALL_RESOURCES
            .iter()
            .copied()
            .find(|r| r.as_str() == raw)
            .ok_or_else(|| DomainError::validation(format!("\"{raw}\" is not a known resource")))
    }
}

/// Complete list, used by parsers, OpenAPI docs, and exhaustive property tests.
pub const ALL_RESOURCES: &[Resource] = &[
    Resource::Organizations,
    Resource::Users,
    Resource::Teams,
    Resource::Audit,
    Resource::Platform,
    Resource::Clusters,
    Resource::RouteConfigs,
    Resource::Listeners,
    Resource::Filters,
    Resource::Secrets,
    Resource::Dataplanes,
    Resource::ProxyCertificates,
    Resource::Agents,
    Resource::Grants,
    Resource::ApiDefinitions,
    Resource::LearningSessions,
    Resource::McpTools,
    Resource::RateLimits,
    Resource::AiProviders,
    Resource::AiRoutes,
    Resource::AiBudgets,
    Resource::Stats,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    Execute,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Execute => "execute",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        ALL_ACTIONS
            .iter()
            .copied()
            .find(|a| a.as_str() == raw)
            .ok_or_else(|| DomainError::validation(format!("\"{raw}\" is not a known action")))
    }

    pub fn is_mutation(self) -> bool {
        !matches!(self, Self::Read)
    }
}

pub const ALL_ACTIONS: &[Action] = &[
    Action::Read,
    Action::Create,
    Action::Update,
    Action::Delete,
    Action::Execute,
];

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn wire_strings_round_trip_for_every_variant() {
        for r in ALL_RESOURCES {
            assert_eq!(Resource::parse(r.as_str()).ok(), Some(*r));
            let json = serde_json::to_value(r).expect("serialize");
            assert_eq!(json, serde_json::Value::String(r.as_str().into()));
        }
        for a in ALL_ACTIONS {
            assert_eq!(Action::parse(a.as_str()).ok(), Some(*a));
        }
    }

    #[test]
    fn governance_set_is_exactly_the_five_platform_resources() {
        let governance: Vec<_> = ALL_RESOURCES
            .iter()
            .filter(|r| r.is_governance())
            .map(|r| r.as_str())
            .collect();
        assert_eq!(
            governance,
            vec!["organizations", "users", "teams", "audit", "platform"]
        );
    }

    #[test]
    fn unknown_strings_are_rejected() {
        assert!(
            Resource::parse("admin-orgs").is_err(),
            "v1 tool-artifact names must not parse"
        );
        assert!(Action::parse("administer").is_err());
    }
}
