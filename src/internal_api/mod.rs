//! Internal API Layer
//!
//! This module provides a unified internal API layer that sits between HTTP/MCP handlers
//! and the service layer. It eliminates code duplication by centralizing:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting
//!
//! Both REST handlers and MCP tools use this layer, ensuring consistent behavior
//! across all entry points.

pub mod auth;
pub mod clusters;
pub mod error;
pub mod types;

pub use auth::InternalAuthContext;
pub use clusters::ClusterOperations;
pub use error::InternalError;
pub use types::*;
