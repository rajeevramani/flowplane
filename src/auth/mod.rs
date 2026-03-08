//! Authentication and authorization module entry point.
//!
//! With Zitadel as the sole auth provider, this module provides:
//! - Zitadel JWT validation middleware
//! - Authorization (team/org isolation)
//! - Domain models for orgs, teams, tokens

pub mod authorization;
pub mod cache;
pub mod middleware;
pub mod models;
pub mod organization;
pub mod permissions;
pub mod scope_registry;
pub mod team;
pub mod user;
pub mod zitadel;
pub mod zitadel_admin;

pub use organization::{
    CreateOrganizationRequest, OrgMembershipResponse, OrgRole, OrgStatus, Organization,
    OrganizationMembership, OrganizationResponse, UpdateOrganizationRequest,
};
pub use team::{CreateTeamRequest, Team, TeamStatus, UpdateTeamRequest};
