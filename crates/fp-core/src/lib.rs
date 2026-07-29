//! Flowplane core. In S1 this hosts server configuration; from S2 it becomes the only
//! mutation path (services + authorization engine, spec/10 §2).

pub mod authz;
pub mod config;
#[cfg(feature = "dev-oidc")]
pub mod dev;
pub mod mcp_declarations;
pub mod oidc;
pub mod services;

pub use authz::{check_resource_access, Decision, GrantSet, PrincipalCtx, Reason};
pub use config::ServerConfig;
pub use fp_domain::authz::TeamRef;
pub use oidc::{OidcConfig, OidcValidator, ValidatedClaims};
