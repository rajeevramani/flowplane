//! Health check endpoint for monitoring and readiness probes

use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Service status (always "ok" when responding)
    #[schema(example = "ok")]
    pub status: String,
}

/// Health check endpoint
///
/// Returns 200 OK when the API server is operational.
/// This endpoint is unauthenticated and suitable for:
/// - Kubernetes liveness/readiness probes
/// - Docker healthchecks
/// - Load balancer health checks
/// - Monitoring systems
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    )
)]
pub async fn health_handler() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse { status: "ok".to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_handler_returns_ok() {
        let (status, Json(response)) = health_handler().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.status, "ok");
    }
}
