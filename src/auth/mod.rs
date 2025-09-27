//! Authentication and authorization module entry point.
//!
//! This module exposes the authentication stack for Flowplane: existing JWT helpers
//! plus the personal access token services, middleware, and validation layers.

pub mod auth_service;
pub mod cleanup_service;
mod hashing;
pub mod jwt;
pub mod middleware;
pub mod models;
pub mod token_service;
pub mod validation;

pub use jwt::{AuthService as JwtAuthService, Claims, Role};
