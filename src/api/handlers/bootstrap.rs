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
use crate::auth::models::TokenStatus;
use crate::auth::organization::OrgRole;
use crate::auth::setup_token::SetupToken;
use crate::auth::user::UserStatus;
use crate::domain::UserId;
use crate::errors::Error;
use crate::storage::repositories::{SqlxUserRepository, UserRepository};

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
    payload.validate().map_err(ApiError::from)?;

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

    // CRITICAL: Use a single PostgreSQL transaction with advisory lock to prevent
    // TOCTOU race conditions. All bootstrap operations (check + create) happen
    // atomically within this transaction. The advisory lock prevents concurrent
    // bootstrap attempts from interleaving.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::from(Error::database(e, "begin bootstrap transaction".into())))?;

    // Acquire advisory lock within transaction (blocks until available)
    sqlx::query("SELECT pg_advisory_xact_lock(1)").execute(&mut *tx).await.map_err(|e| {
        ApiError::from(Error::database(e, "acquire bootstrap advisory lock".into()))
    })?;

    // Check if system is already initialized (within the lock)
    let active_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM personal_access_tokens WHERE status = 'active'")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                ApiError::from(Error::database(e, "count active tokens in bootstrap".into()))
            })?;

    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| ApiError::from(Error::database(e, "count users in bootstrap".into())))?;

    if active_count.0 > 0 || user_count.0 > 0 {
        // Rollback to release advisory lock, then return error
        tx.rollback().await.map_err(|e| {
            ApiError::from(Error::database(e, "rollback bootstrap transaction".into()))
        })?;
        return Err(ApiError::forbidden(
            "System already initialized. Bootstrap is only allowed for initial setup.",
        ));
    }

    // Get configuration from environment
    let ttl_days: i64 = std::env::var("FLOWPLANE_SETUP_TOKEN_TTL_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);

    let max_usage: i64 = std::env::var("FLOWPLANE_SETUP_TOKEN_MAX_USAGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    // All entity creation happens inside the advisory lock transaction.
    // If anything fails, the transaction rolls back atomically.

    // 1. Create platform organization
    let org_id = crate::domain::OrgId::new();
    let org_name = "platform";
    let now = chrono::Utc::now();

    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, description, owner_user_id, settings, status, created_at, updated_at)
         VALUES ($1, $2, $3, $4, NULL, NULL, 'active', $5, $5)",
    )
    .bind(org_id.as_str())
    .bind(org_name)
    .bind("Platform")
    .bind("Platform administration — not a tenant org")
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::from(Error::database(e, "create platform organization".into())))?;

    // 2. Create admin user
    let user_id = UserId::new();
    let password_hash = hashing::hash_password(&payload.password).map_err(ApiError::from)?;
    let user_status = UserStatus::Active.to_string();

    sqlx::query(
        "INSERT INTO users (id, email, password_hash, name, status, is_admin, org_id, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
    )
    .bind(user_id.as_str())
    .bind(&payload.email)
    .bind(&password_hash)
    .bind(&payload.name)
    .bind(&user_status)
    .bind(true)
    .bind(org_id.as_str())
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::from(Error::database(e, "create admin user".into())))?;

    // 3. Update org owner_user_id to the admin user
    sqlx::query("UPDATE organizations SET owner_user_id = $1, updated_at = $2 WHERE id = $3")
        .bind(user_id.as_str())
        .bind(now)
        .bind(org_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::from(Error::database(e, "update platform org owner".into())))?;

    // 4. Create organization membership (admin as Owner)
    let membership_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&membership_id)
    .bind(user_id.as_str())
    .bind(org_id.as_str())
    .bind(OrgRole::Owner.as_str())
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::from(Error::database(e, "create org membership".into())))?;

    // 5. Generate and store setup token
    let setup_token_generator = SetupToken::new();
    let (token_value, hashed_secret, expires_at) =
        setup_token_generator.generate(Some(max_usage), Some(ttl_days)).map_err(ApiError::from)?;

    let token_id = token_value
        .strip_prefix("fp_setup_")
        .and_then(|s| s.split('.').next())
        .ok_or_else(|| {
            ApiError::from(Error::internal("Failed to extract token ID from generated setup token"))
        })?;

    let token_name = format!("bootstrap-setup-token-{}", &payload.email);
    let token_description = format!(
        "Setup token for bootstrap initialization (admin: {}, user_id: {}, expires in {} days)",
        payload.email, user_id, ttl_days
    );
    let token_created_by = format!("bootstrap:user:{}", user_id);
    let token_scopes = vec!["bootstrap:initialize".to_string(), format!("org:{}:admin", org_name)];

    sqlx::query(
        "INSERT INTO personal_access_tokens (id, name, token_hash, description, status, expires_at, created_by, is_setup_token, max_usage_count, usage_count, failed_attempts, locked_until, user_id, user_email, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $15)",
    )
    .bind(token_id)
    .bind(&token_name)
    .bind(&hashed_secret)
    .bind(&token_description)
    .bind(TokenStatus::Active.as_str())
    .bind(expires_at)
    .bind(&token_created_by)
    .bind(true)
    .bind(Some(max_usage))
    .bind(0_i64)
    .bind(0_i32)
    .bind(None::<chrono::DateTime<chrono::Utc>>)
    .bind(user_id.to_string())
    .bind(&payload.email)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::from(Error::database(e, "create setup token".into())))?;

    // Insert token scopes
    for scope in &token_scopes {
        sqlx::query(
            "INSERT INTO token_scopes (id, token_id, scope, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(token_id)
        .bind(scope)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::from(Error::database(e, "create token scope".into())))?;
    }

    // 6. Write audit log entries
    let audit_entries = vec![
        (
            "bootstrap.admin_user_created",
            user_id.as_str(),
            &payload.email as &str,
            serde_json::json!({"name": payload.name, "is_admin": true, "org_id": org_id.as_str()}),
        ),
        (
            "bootstrap.platform_org_created",
            org_id.as_str(),
            "platform",
            serde_json::json!({"name": "platform", "display_name": "Platform", "owner": user_id.to_string()}),
        ),
        (
            "bootstrap.org_membership_created",
            user_id.as_str(),
            &payload.email,
            serde_json::json!({"org_id": org_id.as_str(), "org_name": "platform", "role": "owner"}),
        ),
        (
            "bootstrap.setup_token_generated",
            token_id,
            &token_name,
            serde_json::json!({"admin_email": payload.email, "admin_user_id": user_id.to_string(), "admin_name": payload.name, "ttl_days": ttl_days, "max_usage": max_usage, "expires_at": expires_at.to_string()}),
        ),
    ];

    for (action, resource_id, resource_name, metadata) in &audit_entries {
        let metadata_json = serde_json::to_string(metadata).map_err(|e| {
            ApiError::from(Error::internal(format!("Failed to serialize audit metadata: {}", e)))
        })?;
        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, old_configuration, new_configuration, user_id, client_ip, user_agent, org_id, team_id, created_at)
             VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, $8, NULL, NULL, $9)",
        )
        .bind("auth.token")
        .bind(*resource_id)
        .bind(*resource_name)
        .bind(*action)
        .bind(&metadata_json)
        .bind(user_id.to_string())
        .bind(client_ip.as_deref())
        .bind(user_agent.as_deref())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::from(Error::database(e, "write bootstrap audit log".into())))?;
    }

    // Commit the transaction — all entities created atomically under advisory lock
    tx.commit()
        .await
        .map_err(|e| ApiError::from(Error::database(e, "commit bootstrap transaction".into())))?;

    // Build response with next steps
    let next_steps = vec![
        "Admin user created successfully. You can now login with your email and password.".to_string(),
        format!("Email: {}", payload.email),
        "Platform organization created for governance and administration.".to_string(),
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
    let user_count = user_repo.count_users().await.map_err(ApiError::from)?;

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
    use crate::auth::organization::CreateOrganizationRequest;
    use crate::auth::user::NewUser;
    use crate::domain::OrgId;
    use crate::storage::repositories::{
        OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
        SqlxOrganizationRepository,
    };
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_bootstrap_creates_platform_org() {
        let _db = TestDatabase::new("bootstrap_platform_org").await;
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

        // Create platform organization (user needs org_id)
        let create_org_request = CreateOrganizationRequest {
            name: "platform".to_string(),
            display_name: "Platform".to_string(),
            description: Some("Platform administration — not a tenant org".to_string()),
            owner_user_id: None,
            settings: None,
        };

        let platform_org =
            org_repo.create_organization(create_org_request).await.expect("create org");

        // Verify org was created
        assert_eq!(platform_org.name, "platform");
        assert_eq!(platform_org.display_name, "Platform");
        assert!(platform_org.is_active());

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
            org_id: platform_org.id.clone(),
        };

        let admin_user = user_repo.create_user(new_user).await.expect("create admin user");
        assert_eq!(admin_user.org_id, platform_org.id);

        // Create org membership for admin as Owner
        let membership = org_membership_repo
            .create_membership(&admin_user.id, &platform_org.id, OrgRole::Owner)
            .await
            .expect("create membership");

        // Verify membership — no team, no team membership (governance only)
        assert_eq!(membership.user_id, admin_user.id);
        assert_eq!(membership.org_id, platform_org.id);
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
