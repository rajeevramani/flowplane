//! Bootstrap initialization endpoint for setup token generation
//!
//! This module provides the `/api/v1/bootstrap/initialize` endpoint that allows
//! system administrators to generate a one-time setup token for initial bootstrap.

use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::models::{NewPersonalAccessToken, TokenStatus};
use crate::auth::setup_token::SetupToken;
use crate::domain::TokenId;
use crate::errors::Error;
use crate::storage::repository::{
    AuditEvent, AuditLogRepository, SqlxTokenRepository, TokenRepository,
};
use std::sync::Arc;

/// Request body for bootstrap initialization
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeRequest {
    /// Email address for the system administrator
    #[validate(email)]
    pub admin_email: String,
}

/// Response from bootstrap initialization
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeResponse {
    /// The generated setup token (single-use)
    pub setup_token: String,

    /// When the setup token expires
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,

    /// Maximum number of times this token can be used (typically 1)
    pub max_usage_count: i64,

    /// Message with instructions
    pub message: String,

    /// Next step instructions
    pub next_steps: Vec<String>,
}

fn convert_error(err: Error) -> ApiError {
    ApiError::from(err)
}

/// Bootstrap initialization endpoint
///
/// This endpoint generates a one-time setup token for initial system bootstrap.
/// The setup token can then be used with `/api/v1/auth/sessions` to create an
/// authenticated session.
///
/// # Security
///
/// - This endpoint can ONLY be called when the system is uninitialized (no active tokens exist)
/// - Setup tokens are single-use with a short TTL (7 days by default)
/// - Setup tokens expire after first use or when TTL expires
/// - All bootstrap attempts are logged to the audit log
///
/// # Environment Variables
///
/// - `FLOWPLANE_SETUP_TOKEN_TTL_DAYS`: TTL in days (default: 7)
/// - `FLOWPLANE_SETUP_TOKEN_MAX_USAGE`: Max usage count (default: 1)
#[utoipa::path(
    post,
    path = "/api/v1/bootstrap/initialize",
    request_body = BootstrapInitializeRequest,
    responses(
        (status = 201, description = "Setup token generated successfully", body = BootstrapInitializeResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "System already initialized - tokens exist"),
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

    // Create repositories
    let token_repo = SqlxTokenRepository::new(pool.clone());
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));

    // CRITICAL: Check if system is already initialized (has active tokens)
    let active_count = token_repo.count_active_tokens().await.map_err(convert_error)?;
    if active_count > 0 {
        return Err(ApiError::forbidden(
            "System already initialized. Bootstrap is only allowed for initial setup.",
        ));
    }

    // Get configuration from environment
    let ttl_days = std::env::var("FLOWPLANE_SETUP_TOKEN_TTL_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);

    let max_usage = std::env::var("FLOWPLANE_SETUP_TOKEN_MAX_USAGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    // Generate setup token
    let setup_token_generator = SetupToken::new();
    let (token_value, hashed_secret, expires_at) =
        setup_token_generator.generate(Some(max_usage), Some(ttl_days)).map_err(convert_error)?;

    // Extract token ID from token value (format: fp_setup_{id}.{secret})
    let token_id = token_value
        .strip_prefix("fp_setup_")
        .and_then(|s| s.split('.').next())
        .ok_or_else(|| {
            convert_error(Error::internal("Failed to extract token ID from generated setup token"))
        })?;

    // Store setup token in database
    let new_token = NewPersonalAccessToken {
        id: TokenId::from_string(token_id.to_string()),
        name: format!("bootstrap-setup-token-{}", &payload.admin_email),
        description: Some(format!(
            "Setup token for bootstrap initialization (admin: {}, expires in {} days)",
            payload.admin_email, ttl_days
        )),
        hashed_secret,
        status: TokenStatus::Active,
        expires_at: Some(expires_at),
        created_by: Some("bootstrap".to_string()),
        scopes: vec!["bootstrap:initialize".to_string()],
        is_setup_token: true,
        max_usage_count: Some(max_usage),
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
    };

    token_repo.create_token(new_token).await.map_err(convert_error)?;

    // Log audit event
    audit_repository
        .record_auth_event(AuditEvent {
            action: "bootstrap.setup_token_generated".to_string(),
            resource_id: Some(token_id.to_string()),
            resource_name: Some(format!("bootstrap-setup-token-{}", &payload.admin_email)),
            metadata: serde_json::json!({
                "admin_email": payload.admin_email,
                "ttl_days": ttl_days,
                "max_usage": max_usage,
                "expires_at": expires_at,
            }),
        })
        .await
        .map_err(convert_error)?;

    // Build response with next steps
    let next_steps = vec![
        "Use this setup token to create a session by calling POST /api/v1/auth/sessions".to_string(),
        format!("Example: curl -X POST http://localhost:8080/api/v1/auth/sessions -H 'Content-Type: application/json' -d '{{\"setupToken\": \"{}\"}}'", token_value),
        "The session will include a session cookie and CSRF token for authenticated requests".to_string(),
        "IMPORTANT: This setup token is single-use and will be revoked after creating a session".to_string(),
    ];

    Ok((
        StatusCode::CREATED,
        Json(BootstrapInitializeResponse {
            setup_token: token_value,
            expires_at,
            max_usage_count: max_usage,
            message: format!(
                "Setup token generated successfully for {}. Use this token to create your first session.",
                payload.admin_email
            ),
            next_steps,
        }),
    ))
}
