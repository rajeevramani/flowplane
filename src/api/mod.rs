//! HTTP API facade for the Magaya control plane.
//!
//! This module wires together the API router, handlers, and server boot logic.

pub mod docs;
pub mod error;
pub mod handlers;
pub mod routes;
pub mod server;

pub use server::start_api_server;
