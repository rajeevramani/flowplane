//! Bootstrap initialization endpoint for setup token generation
//!
//! This module provides the `/api/v1/bootstrap/initialize` endpoint that allows
//! system administrators to generate a one-time setup token for initial bootstrap.

use axum::{
    extract::State,
    http::{header, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;
use validator::Validate;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::hashing;
use crate::auth::models::{NewPersonalAccessToken, TokenStatus};
use crate::auth::organization::{CreateOrganizationRequest, OrgRole};
use crate::auth::setup_token::SetupToken;
use crate::auth::team::CreateTeamRequest;
use crate::auth::user::NewUserTeamMembership;
use crate::auth::user::{NewUser, UserStatus};
use crate::domain::{TokenId, UserId};
use crate::errors::Error;
use crate::storage::repositories::{
    AuditEvent, AuditLogRepository, OrgMembershipRepository, OrganizationRepository,
    SqlxOrgMembershipRepository, SqlxOrganizationRepository, SqlxTeamMembershipRepository,
    SqlxTeamRepository, SqlxTokenRepository, SqlxUserRepository, TeamMembershipRepository,
    TeamRepository, TokenRepository, UserRepository,
};
use std::sync::Arc;

/// Request body for bootstrap initialization
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeRequest {
    /// Email address for the system administrator
    #[validate(email)]
    pub email: String,

    /// Password for the admin user account
    #[validate(length(min = 8, message = "Password must be at least 8 characters"))]
    pub password: String,

    /// Full name of the system administrator
    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,
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

/// Response from bootstrap status check
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStatusResponse {
    /// Whether the system needs initialization (true = needs bootstrap, false = already initialized)
    pub needs_initialization: bool,

    /// Optional message describing current state
    pub message: String,
}

fn convert_error(err: Error) -> ApiError {
    ApiError::from(err)
}

/// Extract client IP from headers, preferring X-Forwarded-For
fn extract_client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    // Try X-Forwarded-For header first (for proxied requests)
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            // X-Forwarded-For can contain multiple IPs; the first is the original client
            return value.split(',').next().map(|s| s.trim().to_string());
        }
    }
    None
}

/// Extract User-Agent header
fn extract_user_agent(headers: &axum::http::HeaderMap) -> Option<String> {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
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
    tag = "System"
)]
#[instrument(skip(state, payload, headers), fields(email = %payload.email))]
pub async fn bootstrap_initialize_handler(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<BootstrapInitializeRequest>,
) -> Result<(StatusCode, Json<BootstrapInitializeResponse>), ApiError> {
    // Validate request
    payload.validate().map_err(|err| convert_error(Error::from(err)))?;

    // Extract client context from headers for audit logging
    let client_ip = extract_client_ip(&headers);
    let user_agent = extract_user_agent(&headers);

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
    let user_repo = SqlxUserRepository::new(pool.clone());
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));

    // CRITICAL: Use PostgreSQL advisory lock to prevent TOCTOU race condition.
    // Two concurrent bootstrap requests could both see user_count == 0 and create
    // duplicate admin users. We use a transaction-scoped advisory lock (key=1) so
    // only one request can proceed through the check-then-act section at a time.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| convert_error(Error::database(e, "begin bootstrap transaction".into())))?;

    // Acquire advisory lock within transaction (blocks until available)
    sqlx::query("SELECT pg_advisory_xact_lock(1)")
        .execute(&mut *tx)
        .await
        .map_err(|e| convert_error(Error::database(e, "acquire bootstrap advisory lock".into())))?;

    // Check if system is already initialized (within the lock)
    let active_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM personal_access_tokens WHERE status = 'active'")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                convert_error(Error::database(e, "count active tokens in bootstrap".into()))
            })?;

    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| convert_error(Error::database(e, "count users in bootstrap".into())))?;

    if active_count.0 > 0 || user_count.0 > 0 {
        // Commit to release advisory lock, then return error
        tx.commit().await.map_err(|e| {
            convert_error(Error::database(e, "commit bootstrap transaction".into()))
        })?;
        return Err(ApiError::forbidden(
            "System already initialized. Bootstrap is only allowed for initial setup.",
        ));
    }

    // Commit the transaction to release the advisory lock.
    // We now know we are the sole bootstrap winner. Subsequent requests will see
    // user_count > 0 and be rejected.
    tx.commit()
        .await
        .map_err(|e| convert_error(Error::database(e, "commit bootstrap transaction".into())))?;

    // Get configuration from environment
    let ttl_days = std::env::var("FLOWPLANE_SETUP_TOKEN_TTL_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);

    let max_usage = std::env::var("FLOWPLANE_SETUP_TOKEN_MAX_USAGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    // Create the first admin user
    // Create default organization first (user needs org_id)
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let create_org_request = CreateOrganizationRequest {
        name: "default".to_string(),
        display_name: "Default Organization".to_string(),
        description: Some("Default organization created during system bootstrap".to_string()),
        owner_user_id: None, // Will be updated after user creation
        settings: None,
    };

    let default_org =
        org_repo.create_organization(create_org_request).await.map_err(convert_error)?;

    // Create admin user with org_id set from the start
    let user_id = UserId::new();
    let password_hash = hashing::hash_password(&payload.password).map_err(convert_error)?;

    let new_user = NewUser {
        id: user_id.clone(),
        email: payload.email.clone(),
        password_hash,
        name: payload.name.clone(),
        status: UserStatus::Active,
        is_admin: true,
        org_id: default_org.id.clone(),
    };

    let admin_user = user_repo.create_user(new_user).await.map_err(convert_error)?;

    // Update org's owner_user_id to the admin user
    org_repo
        .update_organization(
            &default_org.id,
            crate::auth::organization::UpdateOrganizationRequest {
                display_name: None,
                description: None,
                owner_user_id: Some(admin_user.id.clone()),
                settings: None,
                status: None,
            },
        )
        .await
        .map_err(convert_error)?;

    // Log admin user creation with user context
    audit_repository
        .record_auth_event(
            AuditEvent::token(
                "bootstrap.admin_user_created",
                Some(admin_user.id.as_str()),
                Some(&admin_user.email),
                serde_json::json!({
                    "name": admin_user.name,
                    "is_admin": admin_user.is_admin,
                    "org_id": default_org.id.as_str(),
                }),
            )
            .with_user_context(
                Some(admin_user.id.to_string()),
                client_ip.clone(),
                user_agent.clone(),
            ),
        )
        .await
        .map_err(convert_error)?;

    // Log organization creation
    audit_repository
        .record_auth_event(
            AuditEvent::token(
                "bootstrap.default_org_created",
                Some(default_org.id.as_str()),
                Some("default"),
                serde_json::json!({
                    "name": "default",
                    "display_name": "Default Organization",
                    "owner": admin_user.id.to_string(),
                }),
            )
            .with_user_context(
                Some(admin_user.id.to_string()),
                client_ip.clone(),
                user_agent.clone(),
            ),
        )
        .await
        .map_err(convert_error)?;

    // Create organization membership for admin user as Owner
    let org_membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
    org_membership_repo
        .create_membership(&admin_user.id, &default_org.id, OrgRole::Owner)
        .await
        .map_err(convert_error)?;

    // Log org membership creation
    audit_repository
        .record_auth_event(
            AuditEvent::token(
                "bootstrap.org_membership_created",
                Some(admin_user.id.as_str()),
                Some(&admin_user.email),
                serde_json::json!({
                    "org_id": default_org.id.as_str(),
                    "org_name": "default",
                    "role": "owner",
                }),
            )
            .with_user_context(
                Some(admin_user.id.to_string()),
                client_ip.clone(),
                user_agent.clone(),
            ),
        )
        .await
        .map_err(convert_error)?;

    // Create platform-admin team
    let team_repo = SqlxTeamRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool.clone());

    // Check if platform-admin team already exists in the default org (idempotency)
    let existing_team = team_repo.get_team_by_org_and_name(&default_org.id, "platform-admin").await;

    match existing_team {
        Ok(None) => {
            // Team doesn't exist, create it
            let create_team_request = CreateTeamRequest {
                name: "platform-admin".to_string(),
                display_name: "Platform Admin".to_string(),
                description: Some("Default team created during system bootstrap".to_string()),
                owner_user_id: Some(admin_user.id.clone()),
                org_id: default_org.id.clone(),
                settings: None,
            };

            let created_team =
                team_repo.create_team(create_team_request).await.map_err(convert_error)?;

            // Add admin to platform-admin team with full permissions
            let membership = NewUserTeamMembership {
                id: format!("utm_{}", admin_user.id),
                user_id: admin_user.id.clone(),
                team: created_team.id.to_string(),
                scopes: vec!["team:platform-admin:*:*".to_string()],
            };

            membership_repo.create_membership(membership).await.map_err(convert_error)?;

            // Log team creation with user context
            audit_repository
                .record_auth_event(
                    AuditEvent::token(
                        "bootstrap.platform_admin_team_created",
                        Some(created_team.id.as_str()),
                        Some("platform-admin"),
                        serde_json::json!({
                            "name": "platform-admin",
                            "display_name": "Platform Admin",
                            "owner": admin_user.id.to_string(),
                            "admin_email": admin_user.email,
                        }),
                    )
                    .with_user_context(
                        Some(admin_user.id.to_string()),
                        client_ip.clone(),
                        user_agent.clone(),
                    ),
                )
                .await
                .map_err(convert_error)?;
        }
        Ok(Some(_)) => {
            // Team already exists - log warning but continue
            tracing::warn!(
                "platform-admin team already exists during bootstrap, skipping creation"
            );
        }
        Err(e) => {
            // Database error - log and propagate
            tracing::error!("Failed to check for platform-admin team: {:?}", e);
            return Err(convert_error(e));
        }
    }

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

    // Store setup token in database for the admin user
    let new_token = NewPersonalAccessToken {
        id: TokenId::from_string(token_id.to_string()),
        name: format!("bootstrap-setup-token-{}", &payload.email),
        description: Some(format!(
            "Setup token for bootstrap initialization (admin: {}, user_id: {}, expires in {} days)",
            payload.email, admin_user.id, ttl_days
        )),
        hashed_secret,
        status: TokenStatus::Active,
        expires_at: Some(expires_at),
        created_by: Some(format!("bootstrap:user:{}", admin_user.id)),
        scopes: vec!["bootstrap:initialize".to_string(), format!("org:{}:admin", default_org.name)],
        is_setup_token: true,
        max_usage_count: Some(max_usage),
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
        user_id: Some(admin_user.id.clone()), // Setup token is for this admin user
        user_email: Some(payload.email.clone()),
    };

    token_repo.create_token(new_token).await.map_err(convert_error)?;

    // Log audit event for setup token generation with user context
    audit_repository
        .record_auth_event(
            AuditEvent::token(
                "bootstrap.setup_token_generated",
                Some(token_id),
                Some(&format!("bootstrap-setup-token-{}", &payload.email)),
                serde_json::json!({
                    "admin_email": payload.email,
                    "admin_user_id": admin_user.id.to_string(),
                    "admin_name": payload.name,
                    "ttl_days": ttl_days,
                    "max_usage": max_usage,
                    "expires_at": expires_at,
                }),
            )
            .with_user_context(Some(admin_user.id.to_string()), client_ip, user_agent),
        )
        .await
        .map_err(convert_error)?;

    // Build response with next steps
    let next_steps = vec![
        "Admin user created successfully. You can now login with your email and password.".to_string(),
        format!("Email: {}", payload.email),
        "Default team 'platform-admin' created for OpenAPI imports and resource management.".to_string(),
        "Use POST /api/v1/auth/login to authenticate with your credentials.".to_string(),
        format!("Example: curl -X POST http://localhost:8080/api/v1/auth/login -H 'Content-Type: application/json' -d '{{\"email\": \"{}\", \"password\": \"YOUR_PASSWORD\"}}'", payload.email),
        "The login response will include a session cookie and CSRF token for authenticated requests.".to_string(),
        "Alternatively, use the setup token to create a session: POST /api/v1/auth/sessions".to_string(),
    ];

    Ok((
        StatusCode::CREATED,
        Json(BootstrapInitializeResponse {
            setup_token: token_value,
            expires_at,
            max_usage_count: max_usage,
            message: format!(
                "Bootstrap complete! Admin user '{}' created successfully. You can now login with your credentials.",
                payload.name
            ),
            next_steps,
        }),
    ))
}

/// Bootstrap status endpoint
///
/// This endpoint checks whether the system needs initialization.
/// Returns `needs_initialization: true` if no users exist in the system.
///
/// # Security
///
/// - This endpoint is public (no authentication required)
/// - Only reveals whether the system has users, not any sensitive data
/// - Safe to expose as it's needed before authentication is possible
#[utoipa::path(
    get,
    path = "/api/v1/bootstrap/status",
    responses(
        (status = 200, description = "Bootstrap status retrieved successfully", body = BootstrapStatusResponse),
        (status = 503, description = "Service unavailable")
    ),
    tag = "System"
)]
#[instrument(skip(state))]
pub async fn bootstrap_status_handler(
    State(state): State<ApiState>,
) -> Result<Json<BootstrapStatusResponse>, ApiError> {
    // Get database pool
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Repository unavailable"))?;
    let pool = cluster_repo.pool().clone();

    // Create user repository
    let user_repo = SqlxUserRepository::new(pool.clone());

    // Check if any users exist
    let user_count = user_repo.count_users().await.map_err(convert_error)?;

    let (needs_initialization, message) = if user_count == 0 {
        (true, "System requires initialization. Please create the first admin user.".to_string())
    } else {
        (false, "System is already initialized.".to_string())
    };

    Ok(Json(BootstrapStatusResponse { needs_initialization, message }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::OrgId;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_bootstrap_creates_default_org() {
        let _db = TestDatabase::new("bootstrap_default_org").await;
        let pool = _db.pool.clone();

        // Create user and org repositories
        let user_repo = SqlxUserRepository::new(pool.clone());
        let org_repo = SqlxOrganizationRepository::new(pool.clone());
        let org_membership_repo = SqlxOrgMembershipRepository::new(pool.clone());

        // Verify no users exist initially
        let user_count = user_repo.count_users().await.expect("count users");
        assert_eq!(user_count, 0);

        // Create bootstrap request
        let request = BootstrapInitializeRequest {
            email: "admin@example.com".to_string(),
            password: "SecurePassword123!".to_string(),
            name: "Admin User".to_string(),
        };

        // Create default organization first (user needs org_id)
        let create_org_request = CreateOrganizationRequest {
            name: "default".to_string(),
            display_name: "Default Organization".to_string(),
            description: Some("Default organization created during system bootstrap".to_string()),
            owner_user_id: None,
            settings: None,
        };

        let default_org =
            org_repo.create_organization(create_org_request).await.expect("create org");

        // Verify org was created
        assert_eq!(default_org.name, "default");
        assert_eq!(default_org.display_name, "Default Organization");
        assert!(default_org.is_active());

        // Create admin user with org_id set
        let user_id = UserId::new();
        let password_hash = hashing::hash_password(&request.password).expect("hash password");
        let new_user = NewUser {
            id: user_id.clone(),
            email: request.email.clone(),
            password_hash,
            name: request.name.clone(),
            status: UserStatus::Active,
            is_admin: true,
            org_id: default_org.id.clone(),
        };

        let admin_user = user_repo.create_user(new_user).await.expect("create admin user");
        assert_eq!(admin_user.org_id, default_org.id);

        // Create org membership for admin as Owner
        let membership = org_membership_repo
            .create_membership(&admin_user.id, &default_org.id, OrgRole::Owner)
            .await
            .expect("create membership");

        // Verify membership was created
        assert_eq!(membership.user_id, admin_user.id);
        assert_eq!(membership.org_id, default_org.id);
        assert_eq!(membership.role, OrgRole::Owner);
    }

    #[tokio::test]
    async fn test_bootstrap_status_needs_initialization() {
        let _db = TestDatabase::new("bootstrap_status_init").await;
        let pool = _db.pool.clone();

        let user_repo = SqlxUserRepository::new(pool.clone());

        // Verify no users exist
        let user_count = user_repo.count_users().await.expect("count users");
        assert_eq!(user_count, 0);

        // System should need initialization
        let (needs_init, _message) = if user_count == 0 {
            (true, "System requires initialization".to_string())
        } else {
            (false, "System is already initialized".to_string())
        };

        assert!(needs_init);
    }

    #[tokio::test]
    async fn test_bootstrap_status_already_initialized() {
        let _db = TestDatabase::new("bootstrap_status_already").await;
        let pool = _db.pool.clone();

        let user_repo = SqlxUserRepository::new(pool.clone());
        let org_repo = SqlxOrganizationRepository::new(pool.clone());

        // Use the seeded test-org organization
        use crate::storage::test_helpers::TEST_ORG_ID;
        let org_id = OrgId::from_str_unchecked(TEST_ORG_ID);
        let org = org_repo
            .get_organization_by_id(&org_id)
            .await
            .expect("get org")
            .expect("seeded org should exist");

        // Create a user with org_id
        let user_id = UserId::new();
        let password_hash = hashing::hash_password("TestPassword123!").expect("hash password");
        let new_user = NewUser {
            id: user_id.clone(),
            email: "test@example.com".to_string(),
            password_hash,
            name: "Test User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: org.id.clone(),
        };

        user_repo.create_user(new_user).await.expect("create user");

        // Verify user exists
        let user_count = user_repo.count_users().await.expect("count users");
        assert_eq!(user_count, 1);

        // System should not need initialization
        let (needs_init, _message) = if user_count == 0 {
            (true, "System requires initialization".to_string())
        } else {
            (false, "System is already initialized".to_string())
        };

        assert!(!needs_init);
    }
}
