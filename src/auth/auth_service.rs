//! Authentication utilities for validating personal access tokens.

use std::sync::Arc;

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use chrono::Utc;
use tracing::{field, info, instrument};

use crate::auth::{
    hashing,
    models::{AuthContext, AuthError, TokenStatus},
};
use crate::domain::TokenId;
use crate::observability::metrics;
use crate::storage::repository::{
    AuditEvent, AuditLogRepository, SqlxTokenRepository, TokenRepository,
};

#[derive(Clone)]
pub struct AuthService {
    repository: Arc<dyn TokenRepository>,
    audit_repository: Arc<AuditLogRepository>,
    argon2: Arc<Argon2<'static>>,
}

impl AuthService {
    pub fn new(
        repository: Arc<dyn TokenRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { repository, audit_repository, argon2: Arc::new(hashing::password_hasher()) }
    }

    pub fn with_sqlx(
        pool: crate::storage::DbPool,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self::new(Arc::new(SqlxTokenRepository::new(pool)), audit_repository)
    }

    #[instrument(skip(self, header, client_ip, user_agent), fields(token_id = field::Empty))]
    pub async fn authenticate(
        &self,
        header: &str,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> std::result::Result<AuthContext, AuthError> {
        let token = header.trim();
        if token.is_empty() {
            metrics::record_authentication("missing_bearer").await;
            return Err(AuthError::MissingBearer);
        }

        let token = token.strip_prefix("Bearer ").unwrap_or(token);

        let Some(stripped) = token.strip_prefix("fp_pat_") else {
            metrics::record_authentication("malformed").await;
            return Err(AuthError::MalformedBearer);
        };

        let mut parts = stripped.splitn(2, '.');
        let id = if let Some(id) = parts.next() {
            id
        } else {
            metrics::record_authentication("malformed").await;
            return Err(AuthError::MalformedBearer);
        };
        let secret = if let Some(secret) = parts.next() {
            secret
        } else {
            metrics::record_authentication("malformed").await;
            return Err(AuthError::MalformedBearer);
        };

        let token_id = TokenId::from_str_unchecked(id);
        let record = match self.repository.find_active_for_auth(&token_id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                metrics::record_authentication("not_found").await;
                return Err(AuthError::TokenNotFound);
            }
            Err(err) => {
                metrics::record_authentication("error").await;
                return Err(AuthError::from(err));
            }
        };

        let (token, hashed_secret) = record;
        tracing::Span::current().record("token_id", token.id.as_str());

        if token.status != TokenStatus::Active {
            metrics::record_authentication("inactive").await;
            return Err(AuthError::InactiveToken);
        }

        if let Some(expiry) = token.expires_at {
            if expiry < Utc::now() {
                metrics::record_authentication("expired").await;
                return Err(AuthError::ExpiredToken);
            }
        }

        let parsed_hash =
            PasswordHash::new(&hashed_secret).map_err(|_| AuthError::MalformedBearer)?;
        if self.argon2.verify_password(secret.as_bytes(), &parsed_hash).is_err() {
            metrics::record_authentication("invalid_secret").await;
            return Err(AuthError::TokenNotFound);
        }

        self.repository.update_last_used(&token.id, Utc::now()).await.map_err(AuthError::from)?;

        // Record audit event with user context
        let user_id_for_audit = token.user_id.as_ref().map(|id| id.to_string());
        self.audit_repository
            .record_auth_event(
                AuditEvent::token(
                    "auth.token.authenticated",
                    Some(token.id.as_str()),
                    Some(&token.name),
                    serde_json::json!({ "scopes": token.scopes }),
                )
                .with_user_context(
                    user_id_for_audit,
                    client_ip.clone(),
                    user_agent.clone(),
                ),
            )
            .await
            .map_err(AuthError::from)?;

        metrics::record_authentication("success").await;
        info!(token_id = %token.id, "personal access token authenticated");

        // Use with_user if the token has user information (for proper user-scoped filtering)
        // Otherwise use new (for system tokens or tokens without user association)
        let context = if let (Some(user_id), Some(user_email)) = (&token.user_id, &token.user_email)
        {
            AuthContext::with_user(
                token.id.clone(),
                token.name,
                user_id.clone(),
                user_email.clone(),
                token.scopes,
            )
        } else {
            AuthContext::new(token.id.clone(), token.name, token.scopes)
        };

        // Add client context to the auth context
        Ok(context.with_request_context(client_ip, user_agent))
    }
}
