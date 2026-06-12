//! Flowplane core. In S1 this hosts server configuration; from S2 it becomes the only
//! mutation path (services + authorization engine, spec/10 §2).

pub mod authz;
pub mod config;
#[cfg(feature = "dev-oidc")]
pub mod dev;
pub mod oidc;

pub use authz::{check_resource_access, Decision, GrantSet, PrincipalCtx, Reason, TeamRef};
pub use config::ServerConfig;
pub use oidc::{OidcConfig, OidcValidator, ValidatedClaims};
