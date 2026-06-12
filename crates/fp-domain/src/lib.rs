//! Flowplane domain model.
//!
//! Pure types only: no async, no IO, no SQL (spec/10 §2). Everything observable by other
//! crates — errors, identifiers, lifecycle states, event types — originates here so all
//! surfaces speak one language.

pub mod authz;
pub mod error;
pub mod event;
pub mod gateway;
pub mod id;
pub mod identity;

pub use error::{DomainError, DomainResult, ErrorCode};
pub use id::{
    AgentId, AuditEntryId, ClusterId, GrantId, ListenerId, MembershipId, OrgId, RequestId,
    RouteConfigId, TeamId, UserId,
};
pub use identity::{
    validate_name, Agent, AgentKind, EntityStatus, OrgRole, Organization, Team, User,
};
