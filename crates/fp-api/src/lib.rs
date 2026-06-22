//! Flowplane REST surface (S1: health, readiness, metrics, error envelope, request ids).
//!
//! Resource endpoints land in S4; this crate establishes the conventions they must follow:
//! every response is JSON, every failure is the spec/10 §8 envelope, every request carries a
//! request id through error body, logs, and traces.

pub mod ai_api;
pub mod api_lifecycle_api;
pub mod auth;
pub mod dataplanes_api;
pub mod discovery_api;
pub mod error;
pub mod expose_api;
pub mod extract;
pub mod identity_api;
pub mod learning_api;
pub mod mcp_api;
pub mod middleware;
pub mod orgs_api;
pub mod resources;
pub mod route_generation_api;
pub mod routes;
pub mod secrets_api;
pub mod state;
pub mod throttle;
pub mod xds_api;

pub use error::ApiError;
pub use routes::build_router;
pub use state::AppState;
