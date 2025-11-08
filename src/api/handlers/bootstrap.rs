//! Bootstrap initialization endpoint for self-service admin token creation
//!
//! This module provides the `/api/v1/bootstrap/initialize` endpoint that allows
//! system administrators to exchange a setup token for an admin personal access token.

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::token_service::{TokenSecretResponse, TokenService};
use crate::auth::validation::CreateTokenRequest;
use crate::errors::Error;
use crate::storage::repository::{AuditEvent, AuditLogRepository};
use std::sync::Arc;

/// Request body for bootstrap initialization
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeRequest {
    /// The setup token to exchange for an admin token
    #[validate(length(min = 20))]
    pub setup_token: String,

    /// Optional name for the admin token (defaults to "admin")
    #[validate(length(min = 3, max = 64))]
    pub token_name: Option<String>,

    /// Optional description for the admin token
    pub description: Option<String>,
}

/// Response from bootstrap initialization
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeResponse {
    /// The admin token details
    #[serde(flatten)]
    pub token: TokenSecretResponse,

    /// Message confirming bootstrap completion
    pub message: String,
}

fn convert_error(err: Error) -> ApiError {
    ApiError::from(err)
}

/// Bootstrap initialization endpoint
///
/// This endpoint allows system administrators to exchange a setup token for an
/// admin personal access token. The setup token is validated and consumed during
/// this process.
///
/// # Security
///
/// - Setup tokens must be active and not expired
/// - Setup tokens must not exceed their max usage count
/// - This operation is atomic (setup token is marked used immediately)
/// - Audit log entries are created for all bootstrap attempts
#[utoipa::path(
    post,
    path = "/api/v1/bootstrap/initialize",
    request_body = BootstrapInitializeRequest,
    responses(
        (status = 201, description = "Bootstrap successful", body = BootstrapInitializeResponse),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Invalid or expired setup token"),
        (status = 403, description = "Admin tokens already exist"),
        (status = 503, description = "Service unavailable")
    ),
    tag = "bootstrap"
)]
pub async fn bootstrap_initialize_handler(
    State(state): State<ApiState>,
    Json(payload): Json<BootstrapInitializeRequest>,
) -> Result<(StatusCode, Json<BootstrapInitializeResponse>), ApiError> {
    // Validate request
    payload.validate().map_err(|err| convert_error(Error::from(err)))?;

    // Get database pool
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Token repository unavailable"))?;
    let pool = cluster_repo.pool().clone();

    // Create token service
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::with_sqlx(pool.clone(), audit_repository.clone());

    // Check if admin tokens already exist
    let active_count = service.count_active_tokens().await.map_err(convert_error)?;
    if active_count > 0 {
        return Err(ApiError::forbidden(
            "Admin tokens already exist. Bootstrap initialization is only allowed for initial setup."
        ));
    }

    // Validate setup token format
    let setup_token_value = &payload.setup_token;
    if !setup_token_value.starts_with("fp_setup_") {
        return Err(ApiError::unauthorized("Invalid setup token format"));
    }

    // Parse setup token (format: fp_setup_{id}.{secret})
    let parts: Vec<&str> = setup_token_value
        .strip_prefix("fp_setup_")
        .ok_or_else(|| ApiError::unauthorized("Invalid setup token format"))?
        .splitn(2, '.')
        .collect();

    if parts.len() != 2 {
        return Err(ApiError::unauthorized("Invalid setup token format"));
    }

    let setup_token_id = parts[0];
    let setup_token_secret = parts[1];

    // Validate setup token with token service
    // This checks:
    // - Token exists and is active
    // - Token is a setup token (is_setup_token = true)
    // - Token has not expired
    // - Token has not exceeded max usage count
    let setup_token_verified =
        service.verify_setup_token(setup_token_id, setup_token_secret).await.map_err(|_e| {
            // Don't leak information about why validation failed
            ApiError::unauthorized("Setup token not found or invalid")
        })?;

    if !setup_token_verified {
        return Err(ApiError::unauthorized("Setup token validation failed"));
    }

    // Create admin token
    let token_name = payload.token_name.unwrap_or_else(|| "admin".to_string());
    let create_request = CreateTokenRequest {
        name: token_name.clone(),
        description: payload.description.or_else(|| {
            Some("Administrator token created via bootstrap initialization".to_string())
        }),
        expires_at: None, // Admin tokens don't expire by default
        scopes: vec!["admin:all".to_string()], // Full admin access
        created_by: Some("bootstrap".to_string()),
    };

    let admin_token = service.create_token(create_request).await.map_err(convert_error)?;

    // Increment setup token usage count (marking it as used)
    service.increment_setup_token_usage(setup_token_id).await.map_err(convert_error)?;

    // Log bootstrap event
    audit_repository
        .record_auth_event(AuditEvent {
            action: "bootstrap.initialize".to_string(),
            resource_id: Some(admin_token.id.clone()),
            resource_name: Some(token_name.clone()),
            metadata: serde_json::json!({
                "setup_token_id": setup_token_id,
                "admin_token_scopes": ["admin:all"],
            }),
        })
        .await
        .map_err(convert_error)?;

    Ok((
        StatusCode::CREATED,
        Json(BootstrapInitializeResponse {
            token: admin_token,
            message: "Bootstrap initialization successful. Admin token created.".to_string(),
        }),
    ))
}
