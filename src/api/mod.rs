//! # REST API Components
//!
//! This module provides the REST API implementation for the Magaya control plane,
//! including HTTP routing, middleware, and request/response handling.

pub mod handlers;
pub mod middleware;
pub mod routes;

pub use handlers::*;
pub use middleware::*;
pub use routes::create_router;

use crate::AppState;
use axum::Router;

/// Create the main application router with all routes and middleware
pub fn create_router(state: AppState) -> Router {
    routes::create_router(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[tokio::test]
    async fn test_create_router() {
        let config = AppConfig::default();
        let state = AppState::new(config).await.unwrap();
        let router = create_router(state);

        // Router should be created successfully
        assert!(!format!("{:?}", router).is_empty());
    }
}