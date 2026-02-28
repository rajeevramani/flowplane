//! Bootstrap initialization endpoint — disabled during Zitadel migration.
//!
//! The previous bootstrap flow created local admin users, password hashes, and
//! setup tokens. With Zitadel as the sole auth provider, bootstrap will be
//! redesigned in Task 2.6 to provision the first project/roles via the Zitadel
//! Management API.
//!
//! For now, the status endpoint remains functional and the initialize endpoint
//! returns a clear error directing users to Zitadel.

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;

/// Request body for bootstrap initialization (preserved for OpenAPI docs)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeRequest {
    /// Placeholder — will be redesigned for Zitadel
    pub project_name: Option<String>,
}

/// Response from bootstrap initialization
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeResponse {
    pub message: String,
    pub next_steps: Vec<String>,
}

/// Response from bootstrap status check
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStatusResponse {
    /// Whether the system needs initialization
    pub needs_initialization: bool,
    /// Message describing current state
    pub message: String,
}

/// Bootstrap initialization endpoint — temporarily disabled.
///
/// Returns an error directing the user to Zitadel for initial setup.
/// Will be redesigned in Task 2.6.
#[utoipa::path(
    post,
    path = "/api/v1/bootstrap/initialize",
    request_body = BootstrapInitializeRequest,
    responses(
        (status = 501, description = "Bootstrap is being redesigned for Zitadel"),
    ),
    tag = "System"
)]
#[instrument(skip(_state, _payload))]
pub async fn bootstrap_initialize_handler(
    State(_state): State<ApiState>,
    Json(_payload): Json<BootstrapInitializeRequest>,
) -> Result<(StatusCode, Json<BootstrapInitializeResponse>), ApiError> {
    Ok((
        StatusCode::NOT_IMPLEMENTED,
        Json(BootstrapInitializeResponse {
            message: "Bootstrap is being redesigned for Zitadel. Use the Zitadel console to \
                      create your project, roles, and initial users."
                .to_string(),
            next_steps: vec![
                "Open the Zitadel console at the configured FLOWPLANE_ZITADEL_ISSUER URL"
                    .to_string(),
                "Create a project and assign role grants to your users".to_string(),
                "Set FLOWPLANE_ZITADEL_PROJECT_ID to your project ID".to_string(),
            ],
        }),
    ))
}

/// Bootstrap status endpoint
///
/// Checks whether the system needs initialization by verifying that
/// the Zitadel project ID is configured.
#[utoipa::path(
    get,
    path = "/api/v1/bootstrap/status",
    responses(
        (status = 200, description = "Bootstrap status", body = BootstrapStatusResponse),
    ),
    tag = "System"
)]
#[instrument(skip(_state))]
pub async fn bootstrap_status_handler(
    State(_state): State<ApiState>,
) -> Result<Json<BootstrapStatusResponse>, ApiError> {
    let project_id = std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID").unwrap_or_default();

    let (needs_initialization, message) = if project_id.is_empty() {
        (
            true,
            "FLOWPLANE_ZITADEL_PROJECT_ID is not set. Create a project in Zitadel and configure it."
                .to_string(),
        )
    } else {
        (false, format!("Zitadel project configured (project_id: {project_id})."))
    };

    Ok(Json(BootstrapStatusResponse { needs_initialization, message }))
}
