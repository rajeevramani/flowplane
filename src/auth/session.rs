//! Session token management for web-based authentication.
//!
//! This module provides session token creation from setup tokens, CSRF protection,
//! session validation, and secure cookie building for HTTP-only session management.

use std::sync::Arc;

use argon2::Argon2;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{field, info, instrument};
use utoipa::ToSchema;

use crate::auth::hashing;
use crate::auth::models::{NewPersonalAccessToken, PersonalAccessToken, TokenStatus};
use crate::domain::TokenId;
use crate::errors::{AuthErrorType, Error, Result};
use crate::observability::metrics;
use crate::storage::repository::{AuditEvent, AuditLogRepository, TokenRepository};

/// Default session token expiration (24 hours)
pub const DEFAULT_SESSION_EXPIRATION_HOURS: i64 = 24;

/// CSRF token byte length (32 bytes = 256 bits of entropy)
const CSRF_TOKEN_BYTES: usize = 32;

/// Session cookie name
pub const SESSION_COOKIE_NAME: &str = "fp_session";

/// CSRF token header name
pub const CSRF_HEADER_NAME: &str = "X-CSRF-Token";

/// Request to create a session from a setup token
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// The setup token to exchange for a session
    pub setup_token: String,
}

/// Response containing session details
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    /// The session token ID
    pub session_id: String,
    /// The complete session token value
    pub session_token: String,
    /// CSRF token for state-changing requests
    pub csrf_token: String,
    /// When the session expires
    pub expires_at: DateTime<Utc>,
    /// Teams the user has access to (extracted from setup token scopes)
    pub teams: Vec<String>,
    /// Scopes granted to this session
    pub scopes: Vec<String>,
}

/// Session information for validation
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// The session token
    pub token: PersonalAccessToken,
    /// Teams extracted from scopes
    pub teams: Vec<String>,
}

/// Secure cookie builder result
#[derive(Debug, Clone)]
pub struct SessionCookie {
    /// Cookie name
    pub name: String,
    /// Cookie value (the session token)
    pub value: String,
    /// Cookie expiration
    pub expires: DateTime<Utc>,
    /// HTTP-only flag
    pub http_only: bool,
    /// Secure flag (HTTPS only)
    pub secure: bool,
    /// SameSite setting
    pub same_site: SameSitePolicy,
    /// Cookie path
    pub path: String,
}

/// SameSite cookie policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSitePolicy {
    Strict,
    Lax,
    None,
}

/// Extract team names from a list of scopes
///
/// Scopes formatted as "team:{team_name}:*" or "team:{team_name}:resource:action"
/// are parsed to extract the unique team names.
pub fn extract_teams_from_scopes(scopes: &[String]) -> Vec<String> {
    scopes
        .iter()
        .filter_map(|scope| {
            // Extract team from scopes like "team:team_name:*" or "team:team_name:resource"
            if scope.starts_with("team:") {
                let parts: Vec<&str> = scope.split(':').collect();
                if parts.len() >= 2 {
                    return Some(parts[1].to_string());
                }
            }
            None
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

/// Session service for managing web-based authentication sessions
#[derive(Clone)]
pub struct SessionService {
    token_repository: Arc<dyn TokenRepository>,
    audit_repository: Arc<AuditLogRepository>,
    argon2: Arc<Argon2<'static>>,
}

impl SessionService {
    /// Create a new session service
    pub fn new(
        token_repository: Arc<dyn TokenRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { token_repository, audit_repository, argon2: Arc::new(hashing::password_hasher()) }
    }

    /// Create a session token from a valid setup token
    ///
    /// # Arguments
    ///
    /// * `setup_token` - The setup token value (format: fp_setup_{id}.{secret})
    ///
    /// # Returns
    ///
    /// A `SessionResponse` containing the session token, CSRF token, and metadata
    ///
    /// # Errors
    ///
    /// - If the setup token format is invalid
    /// - If the setup token is not found, expired, or revoked
    /// - If the setup token has exceeded its usage count
    /// - If database operations fail
    #[instrument(skip(self, setup_token), fields(correlation_id = field::Empty))]
    pub async fn create_session_from_setup_token(
        &self,
        setup_token: &str,
    ) -> Result<SessionResponse> {
        tracing::Span::current().record("correlation_id", field::display(&uuid::Uuid::new_v4()));

        // Parse setup token (format: fp_setup_{id}.{secret})
        let (token_id, secret) = self.parse_setup_token(setup_token)?;

        // Validate and consume the setup token
        let setup_token_data =
            self.token_repository.get_setup_token_for_validation(&token_id).await?;

        // Verify token status
        if setup_token_data.status != "active" {
            return Err(Error::auth("Setup token is not active", AuthErrorType::InvalidToken));
        }

        // Verify not expired
        if let Some(expires_at) = setup_token_data.expires_at {
            if expires_at < Utc::now() {
                return Err(Error::auth("Setup token has expired", AuthErrorType::ExpiredToken));
            }
        }

        // Verify usage count
        if let Some(max_usage) = setup_token_data.max_usage_count {
            if setup_token_data.usage_count >= max_usage {
                return Err(Error::auth(
                    "Setup token has exceeded usage limit",
                    AuthErrorType::InvalidToken,
                ));
            }
        }

        // Verify secret matches
        if !self.verify_secret(&setup_token_data.token_hash, &secret)? {
            // Record failed attempt
            self.token_repository.record_failed_setup_token_attempt(&token_id).await?;
            return Err(Error::auth(
                "Invalid setup token secret",
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Increment usage count
        self.token_repository.increment_setup_token_usage(&token_id).await?;

        // Get the full setup token to extract scopes
        let setup_token_obj =
            self.token_repository.get_token(&TokenId::from_string(token_id.clone())).await?;

        // Extract teams from scopes (format: "team:{team_name}:*")
        let teams = self.extract_teams_from_scopes(&setup_token_obj.scopes);

        // Generate session token (24-hour expiration)
        let session_id = uuid::Uuid::new_v4().to_string();
        let session_secret = self.generate_session_secret()?;
        let session_token_value = format!("fp_session_{}.{}", session_id, session_secret);
        let hashed_session_secret = self.hash_secret(&session_secret)?;

        let expires_at = Utc::now() + Duration::hours(DEFAULT_SESSION_EXPIRATION_HOURS);

        // Create session token in database
        let new_session_token = NewPersonalAccessToken {
            id: TokenId::from_string(session_id.clone()),
            name: format!("session-{}", token_id),
            description: Some(format!("Session created from setup token {}", token_id)),
            hashed_secret: hashed_session_secret,
            status: TokenStatus::Active,
            expires_at: Some(expires_at),
            created_by: Some(format!("setup_token:{}", token_id)),
            scopes: setup_token_obj.scopes.clone(),
            is_setup_token: false,
            max_usage_count: None,
            usage_count: 0,
            failed_attempts: 0,
            locked_until: None,
            user_id: None, // Setup token sessions don't have user info yet
            user_email: None,
        };

        self.token_repository.create_token(new_session_token).await?;

        // Generate CSRF token
        let csrf_token = self.generate_csrf_token()?;

        // Store CSRF token in database
        self.token_repository
            .store_csrf_token(&TokenId::from_string(session_id.clone()), &csrf_token)
            .await?;

        // Record audit event
        self.record_event(
            "auth.session.created",
            Some(&session_id),
            Some(&format!("session-{}", token_id)),
            json!({
                "setup_token_id": token_id,
                "teams": teams,
                "expires_at": expires_at,
            }),
        )
        .await?;

        metrics::record_token_created(1).await;

        info!(
            session_id = %session_id,
            setup_token_id = %token_id,
            "Session created from setup token"
        );

        Ok(SessionResponse {
            session_id,
            session_token: session_token_value,
            csrf_token,
            expires_at,
            teams,
            scopes: setup_token_obj.scopes,
        })
    }

    /// Create a session token from user authentication
    ///
    /// # Arguments
    ///
    /// * `user_id` - The user ID
    /// * `user_email` - The user's email address
    /// * `scopes` - The scopes to grant to the session
    /// * `client_ip` - Optional client IP for audit logging
    /// * `user_agent` - Optional user agent for audit logging
    ///
    /// # Returns
    ///
    /// A `SessionResponse` containing the session token, CSRF token, and metadata
    ///
    /// # Errors
    ///
    /// - If database operations fail
    #[instrument(skip(self, user_email, scopes, client_ip, user_agent), fields(user_id = %user_id, correlation_id = field::Empty))]
    pub async fn create_session_from_user(
        &self,
        user_id: &crate::domain::UserId,
        user_email: &str,
        scopes: Vec<String>,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<SessionResponse> {
        tracing::Span::current().record("correlation_id", field::display(&uuid::Uuid::new_v4()));

        // Extract teams from scopes (format: "team:{team_name}:*")
        let teams = extract_teams_from_scopes(&scopes);

        // Generate session token (24-hour expiration)
        let session_id = uuid::Uuid::new_v4().to_string();
        let session_secret = self.generate_session_secret()?;
        let session_token_value = format!("fp_session_{}.{}", session_id, session_secret);
        let hashed_session_secret = self.hash_secret(&session_secret)?;

        let expires_at = Utc::now() + Duration::hours(DEFAULT_SESSION_EXPIRATION_HOURS);

        // Create session token in database
        let new_session_token = NewPersonalAccessToken {
            id: TokenId::from_string(session_id.clone()),
            name: format!("session-{}", user_id),
            description: Some(format!("Session for user {}", user_email)),
            hashed_secret: hashed_session_secret,
            status: TokenStatus::Active,
            expires_at: Some(expires_at),
            created_by: Some(format!("user:{}", user_id)),
            scopes: scopes.clone(),
            is_setup_token: false,
            max_usage_count: None,
            usage_count: 0,
            failed_attempts: 0,
            locked_until: None,
            user_id: Some(user_id.clone()),
            user_email: Some(user_email.to_string()),
        };

        self.token_repository.create_token(new_session_token).await?;

        // Generate CSRF token
        let csrf_token = self.generate_csrf_token()?;

        // Store CSRF token in database
        self.token_repository
            .store_csrf_token(&TokenId::from_string(session_id.clone()), &csrf_token)
            .await?;

        // Record audit event with user context
        let event = AuditEvent::token(
            "auth.session.created_from_login",
            Some(&session_id),
            Some(&format!("session-{}", user_id)),
            json!({
                "user_id": user_id.to_string(),
                "user_email": user_email,
                "teams": teams,
                "expires_at": expires_at,
            }),
        )
        .with_user_context(Some(user_id.to_string()), client_ip, user_agent);
        self.audit_repository.record_auth_event(event).await?;

        metrics::record_token_created(1).await;

        info!(
            session_id = %session_id,
            user_id = %user_id,
            "Session created from user login"
        );

        Ok(SessionResponse {
            session_id,
            session_token: session_token_value,
            csrf_token,
            expires_at,
            teams,
            scopes,
        })
    }

    /// Generate a cryptographically secure CSRF token
    ///
    /// # Returns
    ///
    /// A base64-encoded URL-safe CSRF token (no padding)
    ///
    /// # Security
    ///
    /// Uses 32 bytes (256 bits) of cryptographically secure random entropy from OsRng
    pub fn generate_csrf_token(&self) -> Result<String> {
        let mut bytes = [0u8; CSRF_TOKEN_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Validate a session token
    ///
    /// # Arguments
    ///
    /// * `session_token` - The session token value (format: fp_session_{id}.{secret})
    ///
    /// # Returns
    ///
    /// `SessionInfo` containing the validated token and extracted team information
    ///
    /// # Errors
    ///
    /// - If the token format is invalid
    /// - If the token is not found, expired, or revoked
    /// - If the secret doesn't match
    #[instrument(skip(self, session_token))]
    pub async fn validate_session(&self, session_token: &str) -> Result<SessionInfo> {
        // Parse session token (format: fp_session_{id}.{secret})
        let (token_id, secret) = self.parse_session_token(session_token)?;

        // Get token from database
        let token =
            self.token_repository.get_token(&TokenId::from_string(token_id.clone())).await?;

        // Verify token status
        if token.status != TokenStatus::Active {
            return Err(Error::auth("Session token is not active", AuthErrorType::InvalidToken));
        }

        // Verify not expired
        if let Some(expires_at) = token.expires_at {
            if expires_at < Utc::now() {
                return Err(Error::auth("Session token has expired", AuthErrorType::ExpiredToken));
            }
        }

        // Verify secret (we need to get the hash from database)
        let (stored_token, stored_hash) = self
            .token_repository
            .find_active_for_auth(&TokenId::from_string(token_id))
            .await?
            .ok_or_else(|| Error::auth("Session token not found", AuthErrorType::InvalidToken))?;

        if !self.verify_secret(&stored_hash, &secret)? {
            return Err(Error::auth(
                "Invalid session token secret",
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Extract teams from scopes
        let teams = self.extract_teams_from_scopes(&stored_token.scopes);

        // Update last used timestamp
        self.token_repository.update_last_used(&stored_token.id, Utc::now()).await?;

        Ok(SessionInfo { token: stored_token, teams })
    }

    /// Build a secure HTTP-only session cookie
    ///
    /// # Arguments
    ///
    /// * `session_token` - The session token value
    /// * `expires_at` - When the session expires
    /// * `secure` - Whether to set the Secure flag (should be true in production)
    ///
    /// # Returns
    ///
    /// A `SessionCookie` with security flags set appropriately
    pub fn build_session_cookie(
        &self,
        session_token: &str,
        expires_at: DateTime<Utc>,
        secure: bool,
    ) -> SessionCookie {
        SessionCookie {
            name: SESSION_COOKIE_NAME.to_string(),
            value: session_token.to_string(),
            expires: expires_at,
            http_only: true,
            secure,
            same_site: SameSitePolicy::Strict,
            path: "/".to_string(),
        }
    }

    /// Validate CSRF token for a session
    ///
    /// # Arguments
    ///
    /// * `token_id` - The session token ID
    /// * `csrf_token` - The CSRF token to validate
    ///
    /// # Returns
    ///
    /// `Ok(())` if the CSRF token is valid, error otherwise
    ///
    /// # Errors
    ///
    /// - If the stored CSRF token is not found
    /// - If the provided CSRF token doesn't match the stored one
    #[instrument(skip(self, csrf_token))]
    pub async fn validate_csrf_token(&self, token_id: &TokenId, csrf_token: &str) -> Result<()> {
        let stored_csrf = self
            .token_repository
            .get_csrf_token(token_id)
            .await?
            .ok_or_else(|| Error::auth("CSRF token not found", AuthErrorType::InvalidToken))?;

        if stored_csrf != csrf_token {
            return Err(Error::auth("CSRF token mismatch", AuthErrorType::InvalidToken));
        }

        Ok(())
    }

    // Private helper methods

    fn parse_setup_token(&self, token: &str) -> Result<(String, String)> {
        // Format: fp_setup_{id}.{secret}
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 2 {
            return Err(Error::auth("Invalid setup token format", AuthErrorType::InvalidToken));
        }

        let id_part = parts[0];
        if !id_part.starts_with("fp_setup_") {
            return Err(Error::auth("Invalid setup token prefix", AuthErrorType::InvalidToken));
        }

        let id = id_part.strip_prefix("fp_setup_").unwrap().to_string();
        let secret = parts[1].to_string();

        Ok((id, secret))
    }

    fn parse_session_token(&self, token: &str) -> Result<(String, String)> {
        // Format: fp_session_{id}.{secret}
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 2 {
            return Err(Error::auth("Invalid session token format", AuthErrorType::InvalidToken));
        }

        let id_part = parts[0];
        if !id_part.starts_with("fp_session_") {
            return Err(Error::auth("Invalid session token prefix", AuthErrorType::InvalidToken));
        }

        let id = id_part.strip_prefix("fp_session_").unwrap().to_string();
        let secret = parts[1].to_string();

        Ok((id, secret))
    }

    fn generate_session_secret(&self) -> Result<String> {
        let mut bytes = [0u8; 64];
        OsRng.fill_bytes(&mut bytes);
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    fn hash_secret(&self, secret: &str) -> Result<String> {
        use argon2::password_hash::SaltString;
        use argon2::PasswordHasher;

        let salt = SaltString::generate(&mut OsRng);
        let hash = self
            .argon2
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|err| Error::internal(format!("Failed to hash secret: {}", err)))?;
        Ok(hash.to_string())
    }

    fn verify_secret(&self, stored: &str, candidate: &str) -> Result<bool> {
        use argon2::{PasswordHash, PasswordVerifier};

        let parsed = PasswordHash::new(stored)
            .map_err(|err| Error::internal(format!("Invalid password hash: {}", err)))?;
        Ok(self.argon2.verify_password(candidate.as_bytes(), &parsed).is_ok())
    }

    fn extract_teams_from_scopes(&self, scopes: &[String]) -> Vec<String> {
        extract_teams_from_scopes(scopes)
    }

    async fn record_event(
        &self,
        event_type: &str,
        token_id: Option<&str>,
        token_name: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<()> {
        let event = AuditEvent::token(event_type, token_id, token_name, metadata);

        self.audit_repository.record_auth_event(event).await
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;

    async fn create_test_pool() -> crate::storage::DbPool {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        };
        create_pool(&config).await.unwrap()
    }

    async fn create_test_service() -> SessionService {
        let pool = create_test_pool().await;
        let token_repo =
            Arc::new(crate::storage::repository::SqlxTokenRepository::new(pool.clone()));
        let audit_repo = Arc::new(AuditLogRepository::new(pool));
        SessionService::new(token_repo, audit_repo)
    }

    #[tokio::test]
    async fn test_generate_csrf_token() {
        let service = create_test_service().await;

        let token1 = service.generate_csrf_token().unwrap();
        let token2 = service.generate_csrf_token().unwrap();

        // Tokens should be different
        assert_ne!(token1, token2);

        // Tokens should be base64 URL-safe encoded
        assert!(!token1.contains('+'));
        assert!(!token1.contains('/'));
        assert!(!token1.contains('='));

        // Tokens should have expected length (32 bytes base64-encoded without padding)
        // 32 bytes = 43 characters in base64 without padding
        assert_eq!(token1.len(), 43);
    }

    #[tokio::test]
    async fn test_extract_teams_from_scopes() {
        let service = create_test_service().await;

        let scopes = vec![
            "team:acme:*".to_string(),
            "team:globex:listeners:read".to_string(),
            "admin:all".to_string(),
            "team:initech:clusters:write".to_string(),
        ];

        let teams = service.extract_teams_from_scopes(&scopes);

        assert_eq!(teams.len(), 3);
        assert!(teams.contains(&"acme".to_string()));
        assert!(teams.contains(&"globex".to_string()));
        assert!(teams.contains(&"initech".to_string()));
    }

    #[tokio::test]
    async fn test_build_session_cookie() {
        let service = create_test_service().await;

        let expires_at = Utc::now() + Duration::hours(24);
        let cookie = service.build_session_cookie("fp_session_test.secret", expires_at, true);

        assert_eq!(cookie.name, SESSION_COOKIE_NAME);
        assert_eq!(cookie.value, "fp_session_test.secret");
        assert_eq!(cookie.expires, expires_at);
        assert!(cookie.http_only);
        assert!(cookie.secure);
        assert_eq!(cookie.same_site, SameSitePolicy::Strict);
        assert_eq!(cookie.path, "/");
    }

    #[tokio::test]
    async fn test_parse_setup_token_valid() {
        let service = create_test_service().await;

        let token = "fp_setup_12345.abcdef";
        let result = service.parse_setup_token(token);

        assert!(result.is_ok());
        let (id, secret) = result.unwrap();
        assert_eq!(id, "12345");
        assert_eq!(secret, "abcdef");
    }

    #[tokio::test]
    async fn test_parse_setup_token_invalid_format() {
        let service = create_test_service().await;

        // Missing dot separator
        let result = service.parse_setup_token("fp_setup_12345abcdef");
        assert!(result.is_err());

        // Wrong prefix
        let result = service.parse_setup_token("fp_pat_12345.abcdef");
        assert!(result.is_err());

        // Empty
        let result = service.parse_setup_token("");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parse_session_token_valid() {
        let service = create_test_service().await;

        let token = "fp_session_12345.abcdef";
        let result = service.parse_session_token(token);

        assert!(result.is_ok());
        let (id, secret) = result.unwrap();
        assert_eq!(id, "12345");
        assert_eq!(secret, "abcdef");
    }

    #[tokio::test]
    async fn test_hash_and_verify_secret() {
        let service = create_test_service().await;

        let secret = "test_secret_123";
        let hashed = service.hash_secret(secret).unwrap();

        // Verify correct secret
        assert!(service.verify_secret(&hashed, secret).unwrap());

        // Verify incorrect secret
        assert!(!service.verify_secret(&hashed, "wrong_secret").unwrap());
    }
}
