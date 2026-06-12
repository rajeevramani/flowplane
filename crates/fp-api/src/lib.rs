//! Flowplane REST surface (S1: health, readiness, metrics, error envelope, request ids).
//!
//! Resource endpoints land in S4; this crate establishes the conventions they must follow:
//! every response is JSON, every failure is the spec/10 §8 envelope, every request carries a
//! request id through error body, logs, and traces.

pub mod auth;
pub mod error;
pub mod identity_api;
pub mod middleware;
pub mod orgs_api;
pub mod resources;
pub mod routes;
pub mod state;
pub mod throttle;

pub use error::ApiError;
pub use routes::build_router;
pub use state::AppState;
