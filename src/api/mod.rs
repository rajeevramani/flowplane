//! HTTP API facade for the Flowplane control plane.
//!
//! This module wires together the API router, handlers, and server boot logic.

pub mod docs;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod rate_limit;
pub mod routes;
pub mod server;

// Test utilities for handler testing - available in tests
#[cfg(test)]
pub mod test_utils;

pub use server::start_api_server;
