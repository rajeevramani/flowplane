//! The authorization decision engine (spec/05 §3.1 semantics, re-housed per spec/10 §4).
//!
//! One pure function decides every access on every surface. Inputs are a snapshot of the
//! principal's identity ([`PrincipalCtx`], loaded by the caller from the DB) and the
//! `(resource, action, team)` being attempted. No IO, no clock, no globals — which is what
//! makes the property tests in this module possible.
//!
//! v1-to-v2 mapping notes:
//! * v1's scope strings (`admin:all`, `org:{name}:{role}`) are gone — memberships are loaded
//!   from the DB into the context, so v1's "scope org must match DB org" defense collapses
//!   into construction.
//! * The team argument carries its owning org ([`TeamRef`]), resolved by the storage layer,
//!   so the cross-org check is part of the decision instead of a separate handler call.
//! * Decisions return a [`Reason`] so audit rows can say *why* (08a §6 requires denials
//!   to be audited with cause).

use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{AgentId, AgentKind, OrgId, OrgRole, TeamId, UserId};
use std::collections::HashSet;

/// The principal's grant rows, keyed exactly like the `user_grants` / `agent_grants` tables.
///
/// The org is part of the key because a grant is only meaningful inside the org it was issued
/// in. A user may belong to several orgs; a grant held in org B must not answer a question
/// asked with org A active.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantSet {
    grants: HashSet<(Resource, Action, TeamId, OrgId)>,
}

impl GrantSet {
    pub fn new(grants: impl IntoIterator<Item = (Resource, Action, TeamId, OrgId)>) -> Self {
        Self {
            grants: grants.into_iter().collect(),
        }
    }

    /// Exact team match. The team argument already carries its org at every call site
    /// ([`TeamRef`]), and the cross-org check runs before this is consulted, so matching on
    /// `(resource, action, team)` here is sufficient — a team id belongs to exactly one org.
    pub fn has(&self, resource: Resource, action: Action, team: TeamId) -> bool {
        self.grants
            .iter()
            .any(|(r, a, t, _)| *r == resource && *a == action && *t == team)
    }

    /// "Does this principal hold `(resource, action)` on ANY team **within `org`**?"
    ///
    /// The org parameter is the whole point: without it a grant in any org the caller belongs
    /// to would satisfy a team-less request made under a different active org.
    pub fn has_any_team_in_org(&self, resource: Resource, action: Action, org: OrgId) -> bool {
        self.grants
            .iter()
            .any(|(r, a, _, o)| *r == resource && *a == action && *o == org)
    }

    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    pub fn len(&self) -> usize {
        self.grants.len()
    }
}

/// Snapshot of who is asking. Loaded once per request by the auth middleware.
#[derive(Debug, Clone, PartialEq)]
pub enum PrincipalCtx {
    User {
        user_id: UserId,
        /// True only for owners of the platform organization (v1's `admin:all`).
        platform_admin: bool,
        /// The validated request org context. D-014 allows multi-org users, so this is set by
        /// the auth middleware from an explicit selector (or the sole non-platform membership),
        /// NEVER by implicitly choosing one of several memberships.
        org: Option<(OrgId, OrgRole)>,
        /// `org` is `None` because the caller has multiple (or zero inferable) orgs and sent
        /// no selector — a tenant-scoped request must fail closed with `org_selector_required`
        /// rather than `not_found` (D-014). `false` when `org` is genuinely resolved or the
        /// caller simply has no access.
        org_selector_required: bool,
        grants: GrantSet,
    },
    Agent {
        agent_id: AgentId,
        kind: AgentKind,
        org_id: OrgId,
        grants: GrantSet,
    },
}

impl PrincipalCtx {
    /// True only for a human owner of the platform organization (governance scope).
    pub fn is_platform_admin(&self) -> bool {
        matches!(
            self,
            Self::User {
                platform_admin: true,
                ..
            }
        )
    }

    /// The human user id; `None` for agent principals.
    pub fn user_id(&self) -> Option<UserId> {
        match self {
            Self::User { user_id, .. } => Some(*user_id),
            Self::Agent { .. } => None,
        }
    }
}

/// Why a decision came out the way it did. Stable wire strings feed audit `detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    // Allow reasons
    PlatformGovernance,
    GrantMatch,
    OrgAdminImplicitTeam,
    AnyTeamGrant,
    GovernanceRead,
    OrgAdminTenantDefault,
    // Deny reasons
    AgentStructurallyDenied,
    CrossOrg,
    GovernanceWriteRequiresPlatformAdmin,
    TenantResourceInvisibleToPlatformAdmin,
    NoMatchingGrant,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlatformGovernance => "platform_governance",
            Self::GrantMatch => "grant_match",
            Self::OrgAdminImplicitTeam => "org_admin_implicit_team",
            Self::AnyTeamGrant => "any_team_grant",
            Self::GovernanceRead => "governance_read",
            Self::OrgAdminTenantDefault => "org_admin_tenant_default",
            Self::AgentStructurallyDenied => "agent_structurally_denied",
            Self::CrossOrg => "cross_org",
            Self::GovernanceWriteRequiresPlatformAdmin => {
                "governance_write_requires_platform_admin"
            }
            Self::TenantResourceInvisibleToPlatformAdmin => {
                "tenant_resource_invisible_to_platform_admin"
            }
            Self::NoMatchingGrant => "no_matching_grant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow(Reason),
    Deny(Reason),
}

impl Decision {
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allow(_))
    }

    pub fn reason(self) -> Reason {
        match self {
            Self::Allow(r) | Self::Deny(r) => r,
        }
    }
}

/// The single gate. Decision table from spec/05 §3.1, in evaluation order.
pub fn check_resource_access(
    ctx: &PrincipalCtx,
    resource: Resource,
    action: Action,
    team: Option<TeamRef>,
) -> Decision {
    // Step 0 — agent structural guard, before anything else (spec/05 §3.1 step 0).
    let (platform_admin, principal_org, grants) = match ctx {
        PrincipalCtx::Agent {
            kind,
            org_id,
            grants,
            ..
        } => match kind {
            AgentKind::GatewayTool => {
                return if resource == Resource::McpTools {
                    match team {
                        Some(team_ref) => {
                            if team_ref.org_id != *org_id {
                                Decision::Deny(Reason::CrossOrg)
                            } else if grants.has(resource, action, team_ref.id) {
                                Decision::Allow(Reason::GrantMatch)
                            } else {
                                Decision::Deny(Reason::NoMatchingGrant)
                            }
                        }
                        None => {
                            // An agent belongs to exactly one org, so its grant set is
                            // single-org by construction; scoping to `org_id` is a no-op today
                            // and stays correct if that ever changes.
                            if grants.has_any_team_in_org(resource, action, *org_id) {
                                Decision::Allow(Reason::AnyTeamGrant)
                            } else {
                                Decision::Deny(Reason::NoMatchingGrant)
                            }
                        }
                    }
                } else {
                    Decision::Deny(Reason::AgentStructurallyDenied)
                };
            }
            AgentKind::ApiConsumer => {
                return Decision::Deny(Reason::AgentStructurallyDenied);
            }
            AgentKind::CpTool => {
                // cp-tool agents are grants-only: no governance arm, no org-admin arm.
                return match team {
                    Some(team_ref) => {
                        if team_ref.org_id != *org_id {
                            Decision::Deny(Reason::CrossOrg)
                        } else if grants.has(resource, action, team_ref.id) {
                            Decision::Allow(Reason::GrantMatch)
                        } else {
                            Decision::Deny(Reason::NoMatchingGrant)
                        }
                    }
                    None => {
                        if grants.has_any_team_in_org(resource, action, *org_id) {
                            Decision::Allow(Reason::AnyTeamGrant)
                        } else {
                            Decision::Deny(Reason::NoMatchingGrant)
                        }
                    }
                };
            }
        },
        // The engine authorizes against the validated active `org`. When the middleware
        // could not resolve one (multi-org + no selector), `org` is None and every
        // tenant/org path below denies — the helpful `org_selector_required` error is
        // produced earlier at the request seam (resolve_team / service org helpers).
        PrincipalCtx::User {
            platform_admin,
            org,
            grants,
            ..
        } => (*platform_admin, *org, grants),
    };

    // Step 1 — platform-admin bypass applies to GOVERNANCE resources only (invariant 1).
    if platform_admin && resource.is_governance() {
        return Decision::Allow(Reason::PlatformGovernance);
    }

    match team {
        Some(team_ref) => {
            // Cross-org is decided before grants are even consulted: a principal in org A
            // gets NoSuchTeam-equivalent treatment for org B's teams (the API layer renders
            // this Deny as 404 — anti-enumeration, invariant 2).
            match principal_org {
                Some((org_id, role)) => {
                    if org_id != team_ref.org_id {
                        return Decision::Deny(Reason::CrossOrg);
                    }
                    // Step 2a — exact grant row.
                    if grants.has(resource, action, team_ref.id) {
                        return Decision::Allow(Reason::GrantMatch);
                    }
                    // Step 2b — org-admin implicit team access (own org only, checked above).
                    if role.is_org_admin() && !resource.is_governance() {
                        return Decision::Allow(Reason::OrgAdminImplicitTeam);
                    }
                    Decision::Deny(Reason::NoMatchingGrant)
                }
                None => {
                    // No org membership at all. A platform admin probing tenant teams is
                    // denied (invariant 1); anyone else simply has no path.
                    if platform_admin {
                        Decision::Deny(Reason::TenantResourceInvisibleToPlatformAdmin)
                    } else {
                        Decision::Deny(Reason::NoMatchingGrant)
                    }
                }
            }
        }
        None => {
            // Evaluation order here is deliberate and load-bearing (see the feature design's
            // "an any-team decision requires a resolved active org").
            //
            // Step 3a — GOVERNANCE first, unchanged. Governance resources cannot carry team
            // grants at all (the service refuses them), so consulting the grant set for them
            // could only ever match a row written by direct SQL. Keeping this branch ahead of
            // the any-team check also means governance gains no new active-org precondition,
            // and the platform-admin arm already returned at step 1.
            if resource.is_governance() {
                return if action == Action::Read && principal_org.is_some() {
                    Decision::Allow(Reason::GovernanceRead)
                } else {
                    Decision::Deny(Reason::GovernanceWriteRequiresPlatformAdmin)
                };
            }
            // Step 3b — a TENANT resource with no team requires a RESOLVED ACTIVE ORG before
            // any-team is consulted, and the grant must live in that org. Previously any grant
            // in any org the caller belonged to satisfied this, so a grant held in org B
            // answered a request made with org A active. With no active org at all (multi-org
            // caller, no selector) this falls through to the deny arms below — fail closed.
            if let Some((active_org, _)) = principal_org {
                if grants.has_any_team_in_org(resource, action, active_org) {
                    return Decision::Allow(Reason::AnyTeamGrant);
                }
            }
            // Step 3c — tenant resource without a team: org admins only.
            match principal_org {
                Some((_, role)) if role.is_org_admin() => {
                    Decision::Allow(Reason::OrgAdminTenantDefault)
                }
                _ => {
                    if platform_admin {
                        Decision::Deny(Reason::TenantResourceInvisibleToPlatformAdmin)
                    } else {
                        Decision::Deny(Reason::NoMatchingGrant)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_domain::authz::{ALL_ACTIONS, ALL_RESOURCES};

    fn user(platform_admin: bool, org: Option<(OrgId, OrgRole)>, grants: GrantSet) -> PrincipalCtx {
        PrincipalCtx::User {
            user_id: UserId::generate(),
            platform_admin,
            org_selector_required: false,
            org,
            grants,
        }
    }

    fn agent(kind: AgentKind, org_id: OrgId, grants: GrantSet) -> PrincipalCtx {
        PrincipalCtx::Agent {
            agent_id: AgentId::generate(),
            kind,
            org_id,
            grants,
        }
    }

    // ---- PrincipalCtx accessors (org read-scoping branches on these) ----

    #[test]
    fn is_platform_admin_reflects_the_user_flag_and_is_never_true_for_agents() {
        assert!(user(true, None, GrantSet::default()).is_platform_admin());
        assert!(!user(
            false,
            Some((OrgId::generate(), OrgRole::Admin)),
            GrantSet::default()
        )
        .is_platform_admin());
        for kind in [
            AgentKind::GatewayTool,
            AgentKind::CpTool,
            AgentKind::ApiConsumer,
        ] {
            assert!(!agent(kind, OrgId::generate(), GrantSet::default()).is_platform_admin());
        }
    }

    #[test]
    fn user_id_is_some_for_users_and_none_for_agents() {
        let ctx = user(false, None, GrantSet::default());
        match &ctx {
            PrincipalCtx::User { user_id, .. } => assert_eq!(ctx.user_id(), Some(*user_id)),
            PrincipalCtx::Agent { .. } => unreachable!(),
        }
        for kind in [
            AgentKind::GatewayTool,
            AgentKind::CpTool,
            AgentKind::ApiConsumer,
        ] {
            assert_eq!(
                agent(kind, OrgId::generate(), GrantSet::default()).user_id(),
                None
            );
        }
    }

    // ---- Invariant 1: platform admin is governance-only, never tenant access ----

    #[test]
    fn platform_admin_cannot_touch_any_tenant_resource_any_action_any_team() {
        let org = OrgId::generate();
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: org,
        };
        let admin = user(true, None, GrantSet::default());
        for resource in ALL_RESOURCES.iter().filter(|r| !r.is_governance()) {
            for action in ALL_ACTIONS {
                for team_arg in [Some(team), None] {
                    let decision = check_resource_access(&admin, *resource, *action, team_arg);
                    assert!(
                        !decision.is_allowed(),
                        "platform admin must be denied {resource:?}/{action:?} team={team_arg:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn platform_admin_gets_all_governance() {
        let admin = user(true, None, GrantSet::default());
        for resource in ALL_RESOURCES.iter().filter(|r| r.is_governance()) {
            for action in ALL_ACTIONS {
                let decision = check_resource_access(&admin, *resource, *action, None);
                assert_eq!(decision, Decision::Allow(Reason::PlatformGovernance));
            }
        }
    }

    // ---- Invariant 2: cross-org denied before grants are consulted ----

    #[test]
    fn cross_org_team_is_denied_even_with_matching_grant_row() {
        let my_org = OrgId::generate();
        let other_org = OrgId::generate();
        let foreign_team = TeamRef {
            id: TeamId::generate(),
            org_id: other_org,
        };
        // Hostile setup: a grant row somehow names the foreign team. The row carries the
        // foreign team's OWN org, which is the only org it could legally have — that is
        // exactly the shape that must still be denied, since the caller's active org is
        // `my_org` and cross-org is decided before grants are consulted at all.
        let grants =
            GrantSet::new([(Resource::Clusters, Action::Read, foreign_team.id, other_org)]);
        let ctx = user(false, Some((my_org, OrgRole::Owner)), grants);
        let decision =
            check_resource_access(&ctx, Resource::Clusters, Action::Read, Some(foreign_team));
        assert_eq!(decision, Decision::Deny(Reason::CrossOrg));
    }

    #[test]
    fn org_admin_implicit_access_stops_at_org_boundary() {
        let my_org = OrgId::generate();
        let other_org = OrgId::generate();
        let admin = user(false, Some((my_org, OrgRole::Admin)), GrantSet::default());
        let own_team = TeamRef {
            id: TeamId::generate(),
            org_id: my_org,
        };
        let foreign_team = TeamRef {
            id: TeamId::generate(),
            org_id: other_org,
        };
        assert!(
            check_resource_access(&admin, Resource::Listeners, Action::Create, Some(own_team))
                .is_allowed()
        );
        assert_eq!(
            check_resource_access(
                &admin,
                Resource::Listeners,
                Action::Create,
                Some(foreign_team)
            ),
            Decision::Deny(Reason::CrossOrg)
        );
    }

    // ---- Invariant 3 (structural agents) ----

    #[test]
    fn gateway_agents_only_get_granted_mcp_tools_access() {
        let org = OrgId::generate();
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: org,
        };
        let ctx = agent(
            AgentKind::GatewayTool,
            org,
            GrantSet::new([(Resource::McpTools, Action::Execute, team.id, org)]),
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::McpTools, Action::Execute, Some(team)),
            Decision::Allow(Reason::GrantMatch)
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::McpTools, Action::Read, Some(team)),
            Decision::Deny(Reason::NoMatchingGrant)
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::Clusters, Action::Read, Some(team)),
            Decision::Deny(Reason::AgentStructurallyDenied)
        );
    }

    #[test]
    fn api_consumer_agents_denied_everything() {
        let org = OrgId::generate();
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: org,
        };
        let ctx = agent(
            AgentKind::ApiConsumer,
            org,
            GrantSet::new([(Resource::McpTools, Action::Execute, team.id, org)]),
        );
        for resource in ALL_RESOURCES {
            for action in ALL_ACTIONS {
                let decision = check_resource_access(&ctx, *resource, *action, Some(team));
                assert_eq!(decision, Decision::Deny(Reason::AgentStructurallyDenied));
            }
        }
    }

    #[test]
    fn cp_tool_agent_is_grants_only_no_governance_no_org_admin_arm() {
        let org = OrgId::generate();
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: org,
        };
        let granted = agent(
            AgentKind::CpTool,
            org,
            GrantSet::new([(Resource::Clusters, Action::Create, team.id, org)]),
        );
        assert!(
            check_resource_access(&granted, Resource::Clusters, Action::Create, Some(team))
                .is_allowed()
        );
        // Same agent: no governance access regardless of action…
        assert!(
            !check_resource_access(&granted, Resource::Organizations, Action::Read, None)
                .is_allowed()
        );
        // …no access beyond the exact grant…
        assert!(
            !check_resource_access(&granted, Resource::Clusters, Action::Delete, Some(team))
                .is_allowed()
        );
        // …and cross-org teams are invisible.
        let foreign = TeamRef {
            id: TeamId::generate(),
            org_id: OrgId::generate(),
        };
        assert_eq!(
            check_resource_access(&granted, Resource::Clusters, Action::Create, Some(foreign)),
            Decision::Deny(Reason::CrossOrg)
        );
    }

    // ---- Decision-table rows (spec/05 §3.1) ----

    #[test]
    fn exact_grant_match_allows() {
        let org = OrgId::generate();
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: org,
        };
        let ctx = user(
            false,
            Some((org, OrgRole::Member)),
            GrantSet::new([(Resource::Secrets, Action::Update, team.id, org)]),
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::Secrets, Action::Update, Some(team)),
            Decision::Allow(Reason::GrantMatch)
        );
        // The grant is exact: a different action on the same team is denied.
        assert_eq!(
            check_resource_access(&ctx, Resource::Secrets, Action::Delete, Some(team)),
            Decision::Deny(Reason::NoMatchingGrant)
        );
    }

    #[test]
    fn any_team_grant_admits_list_endpoints() {
        let org = OrgId::generate();
        let team_id = TeamId::generate();
        let ctx = user(
            false,
            Some((org, OrgRole::Member)),
            GrantSet::new([(Resource::Clusters, Action::Read, team_id, org)]),
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::Clusters, Action::Read, None),
            Decision::Allow(Reason::AnyTeamGrant)
        );
    }

    #[test]
    fn governance_reads_for_org_members_writes_denied() {
        let org = OrgId::generate();
        for role in [
            OrgRole::Viewer,
            OrgRole::Member,
            OrgRole::Admin,
            OrgRole::Owner,
        ] {
            let ctx = user(false, Some((org, role)), GrantSet::default());
            assert_eq!(
                check_resource_access(&ctx, Resource::Teams, Action::Read, None),
                Decision::Allow(Reason::GovernanceRead),
                "{role:?} can read governance"
            );
            assert_eq!(
                check_resource_access(&ctx, Resource::Organizations, Action::Create, None),
                Decision::Deny(Reason::GovernanceWriteRequiresPlatformAdmin),
                "{role:?} cannot write governance"
            );
        }
    }

    #[test]
    fn tenant_resource_without_team_requires_org_admin() {
        let org = OrgId::generate();
        let admin = user(false, Some((org, OrgRole::Admin)), GrantSet::default());
        let member = user(false, Some((org, OrgRole::Member)), GrantSet::default());
        assert_eq!(
            check_resource_access(&admin, Resource::Dataplanes, Action::Create, None),
            Decision::Allow(Reason::OrgAdminTenantDefault)
        );
        assert_eq!(
            check_resource_access(&member, Resource::Dataplanes, Action::Create, None),
            Decision::Deny(Reason::NoMatchingGrant)
        );
    }

    #[test]
    fn principal_with_nothing_gets_nothing() {
        let nobody = user(false, None, GrantSet::default());
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: OrgId::generate(),
        };
        for resource in ALL_RESOURCES {
            for action in ALL_ACTIONS {
                for team_arg in [Some(team), None] {
                    assert!(
                        !check_resource_access(&nobody, *resource, *action, team_arg).is_allowed(),
                        "memberless, grantless principal must be denied everything"
                    );
                }
            }
        }
    }

    // ---- The any-team decision is scoped to the ACTIVE org ----
    //
    // A grant is authority *inside the org that issued it*. Before the org joined the grant
    // tuple, the `team: None` branch matched a grant from ANY org the caller belonged to, so a
    // multi-org user's grant in org B answered a request made with org A active.

    /// Acceptance criterion 5: same grant, same user, different active org → different verdict.
    #[test]
    fn any_team_grant_does_not_reach_outside_the_active_org() {
        let org_a = OrgId::generate();
        let org_b = OrgId::generate();
        let team_in_b = TeamId::generate();

        // The user belongs to both orgs but holds `agents:read` only on a team in B.
        let grants = GrantSet::new([(Resource::Agents, Action::Read, team_in_b, org_b)]);

        let acting_as_a = user(false, Some((org_a, OrgRole::Member)), grants.clone());
        assert_eq!(
            check_resource_access(&acting_as_a, Resource::Agents, Action::Read, None),
            Decision::Deny(Reason::NoMatchingGrant),
            "a grant held in org B must not authorize a team-less read taken with org A active"
        );

        let acting_as_b = user(false, Some((org_b, OrgRole::Member)), grants);
        assert_eq!(
            check_resource_access(&acting_as_b, Resource::Agents, Action::Read, None),
            Decision::Allow(Reason::AnyTeamGrant),
            "the same grant must still authorize the same read with org B active"
        );
    }

    /// Acceptance criterion 6: no resolved active org → denied regardless of grants held.
    /// This is the multi-org-caller-without-a-selector case (D-014), which must fail closed.
    #[test]
    fn no_resolved_active_org_denies_every_tenant_any_team_read() {
        let org = OrgId::generate();
        let team = TeamId::generate();

        for (resource, action) in [
            (Resource::Agents, Action::Read),
            (Resource::Clusters, Action::Read),
            (Resource::Secrets, Action::Update),
            (Resource::McpTools, Action::Execute),
        ] {
            let ctx = PrincipalCtx::User {
                user_id: UserId::generate(),
                platform_admin: false,
                // Multi-org caller sent no selector, so the middleware resolved no active org.
                org: None,
                org_selector_required: true,
                grants: GrantSet::new([(resource, action, team, org)]),
            };
            assert_eq!(
                check_resource_access(&ctx, resource, action, None),
                Decision::Deny(Reason::NoMatchingGrant),
                "holding {resource:?}/{action:?} must not authorize a team-less read when no \
                 active org is resolved"
            );
        }
    }

    /// Governance keeps its existing behavior and gains NO active-org precondition from the
    /// reorder — the branch runs before the any-team check, exactly as it did before.
    #[test]
    fn governance_reads_are_unaffected_by_the_active_org_scoping() {
        let org = OrgId::generate();
        let member = user(false, Some((org, OrgRole::Member)), GrantSet::default());
        assert_eq!(
            check_resource_access(&member, Resource::Users, Action::Read, None),
            Decision::Allow(Reason::GovernanceRead),
            "an org-scoped caller still reads governance resources without any grant"
        );

        // ...and a governance WRITE still requires platform admin, not an org role.
        assert_eq!(
            check_resource_access(&member, Resource::Users, Action::Create, None),
            Decision::Deny(Reason::GovernanceWriteRequiresPlatformAdmin)
        );
    }

    /// An agent's grant set is single-org by construction, so scoping the agent branch to its
    /// own org changes nothing for a legitimate agent — pinned so the no-op stays a no-op.
    #[test]
    fn agent_any_team_decisions_are_unchanged_by_org_scoping() {
        let org = OrgId::generate();
        let team = TeamId::generate();
        let ctx = agent(
            AgentKind::CpTool,
            org,
            GrantSet::new([(Resource::Clusters, Action::Read, team, org)]),
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::Clusters, Action::Read, None),
            Decision::Allow(Reason::AnyTeamGrant)
        );
        assert_eq!(
            check_resource_access(&ctx, Resource::Secrets, Action::Read, None),
            Decision::Deny(Reason::NoMatchingGrant)
        );
    }
}
