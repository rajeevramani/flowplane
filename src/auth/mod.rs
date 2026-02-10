//! Authentication and authorization module entry point.
//!
//! This module exposes the authentication stack for Flowplane: existing JWT helpers
//! plus the personal access token services, middleware, and validation layers.

pub mod auth_service;
pub mod authorization;
pub mod cleanup_service;
pub mod hashing;
pub mod invitation;
pub mod invitation_service;
pub mod jwt;
pub mod login_service;
pub mod middleware;
pub mod models;
pub mod organization;
pub mod scope_registry;
pub mod session;
pub mod setup_token;
pub mod team;
pub mod token_service;
pub mod user;
pub mod user_service;
pub mod user_validation;
pub mod validation;

pub use hashing::{hash_password, verify_password};
pub use jwt::{AuthService as JwtAuthService, Claims, Role};
pub use organization::{
    CreateOrganizationRequest, OrgMembershipResponse, OrgRole, OrgStatus, Organization,
    OrganizationMembership, OrganizationResponse, UpdateOrganizationRequest,
};
pub use team::{CreateTeamRequest, Team, TeamStatus, UpdateTeamRequest};
pub use user::{
    ChangePasswordRequest, CreateTeamMembershipRequest, CreateUserRequest, LoginRequest, NewUser,
    NewUserTeamMembership, UpdateTeamMembershipRequest, UpdateUser, UpdateUserRequest, User,
    UserResponse, UserStatus, UserTeamMembership, UserWithTeamsResponse,
};
