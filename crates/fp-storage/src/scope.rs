//! The scoping types every repository method requires (spec/10 §4).
//!
//! There is deliberately no way to run a tenant-table query without stating *whose* data you
//! are touching: `TeamScope::Team` puts the team predicate into the SQL itself, and
//! `TeamScope::PlatformAdmin` is an explicit, greppable, audit-carrying admission that a
//! query crosses tenants (only the governance/admin paths construct it). This universalizes
//! the pattern v1 reached only in its newest code (spec/03 §6.2) and makes the v1 failure
//! mode — a handler forgetting `verify_team_access()` — unrepresentable.

use fp_domain::TeamId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamScope {
    /// Query is confined to one team; the predicate is part of the SQL.
    Team(TeamId),
    /// Cross-tenant query from a platform-governance path. The reason string lands in audit
    /// detail and logs — "because admin" is not enough at 2am.
    PlatformAdmin { reason: &'static str },
}

impl TeamScope {
    /// The team id when scoped; `None` only for the explicit platform-admin escape hatch.
    pub fn team_id(&self) -> Option<TeamId> {
        match self {
            Self::Team(id) => Some(*id),
            Self::PlatformAdmin { .. } => None,
        }
    }
}
