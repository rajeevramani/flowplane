//! HTTP API facade for the Flowplane control plane.
//!
//! This module wires together the API router, handlers, and server boot logic.

pub mod docs;
pub mod error;
pub mod gateway_handlers;
pub mod handlers;
pub mod listener_handlers;
pub mod route_handlers;
pub mod routes;
pub mod server;

pub use server::start_api_server;
