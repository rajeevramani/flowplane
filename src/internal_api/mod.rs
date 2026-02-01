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
pub mod dataplanes;
pub mod error;
pub mod filters;
pub mod learning;
pub mod listeners;
pub mod openapi;
pub mod routes;
pub mod schemas;
pub mod types;
pub mod virtual_hosts;

#[cfg(test)]
mod tests;

pub use auth::InternalAuthContext;
pub use clusters::ClusterOperations;
pub use dataplanes::DataplaneOperations;
pub use error::InternalError;
pub use filters::FilterOperations;
pub use learning::LearningSessionOperations;
pub use listeners::ListenerOperations;
pub use openapi::OpenApiOperations;
pub use routes::{RouteConfigOperations, RouteOperations};
pub use schemas::AggregatedSchemaOperations;
pub use types::*;
pub use virtual_hosts::VirtualHostOperations;
