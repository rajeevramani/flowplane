//! Business logic for issuing and managing personal access tokens.

use std::sync::Arc;

use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::Utc;
use rand::{distributions::Alphanumeric, rngs::OsRng, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;
use tracing::{field, info, instrument};
use utoipa::ToSchema;
use validator::Validate;

use crate::auth::hashing;
use crate::auth::models::{
    AuthContext, NewPersonalAccessToken, PersonalAccessToken, TokenStatus,
    UpdatePersonalAccessToken,
};
use crate::auth::validation::{CreateTokenRequest, UpdateTokenRequest};
use crate::domain::TokenId;
use crate::errors::{Error, Result};
use crate::observability::metrics;
use crate::secrets::SecretsClient;
use crate::storage::repository::{
    AuditEvent, AuditLogRepository, SqlxTokenRepository, TokenRepository,
};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenSecretResponse {
    pub id: String,
    pub token: String,
}

#[derive(Clone)]
pub struct TokenService {
    repository: Arc<dyn TokenRepository>,
    audit_repository: Arc<AuditLogRepository>,
    argon2: Arc<Argon2<'static>>,
}

impl TokenService {
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

    pub fn hash_secret(&self, secret: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = self
            .argon2
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|err| Error::internal(format!("Failed to hash token secret: {}", err)))?;
        Ok(hash.to_string())
    }

    pub fn verify_secret(&self, stored: &str, candidate: &str) -> Result<bool> {
        let parsed = PasswordHash::new(stored)
            .map_err(|err| Error::internal(format!("Invalid password hash: {}", err)))?;
        Ok(self.argon2.verify_password(candidate.as_bytes(), &parsed).is_ok())
    }

    #[instrument(skip(self, bootstrap_secret, secrets_client), fields(correlation_id = field::Empty))]
    pub async fn ensure_bootstrap_token<T: SecretsClient>(
        &self,
        bootstrap_secret: &str,
        secrets_client: Option<&T>,
    ) -> Result<Option<String>> {
        tracing::Span::current().record("correlation_id", field::display(&uuid::Uuid::new_v4()));

        if self.repository.count_tokens().await? > 0 {
            let active_count = self.repository.count_active_tokens().await?;
            metrics::set_active_tokens(active_count as usize).await;
            return Ok(None);
        }

        // OPTIONAL: Store in secrets backend if provided
        if let Some(client) = secrets_client {
            match client.set_secret("bootstrap_token", bootstrap_secret).await {
                Ok(_) => {
                    info!("Stored bootstrap token in secrets backend for future rotation support");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to store bootstrap token in secrets backend. \
                         Bootstrap token rotation will not be available. \
                         This is expected in development without Vault."
                    );
                    // Continue execution - don't fail the bootstrap process
                }
            }
        } else {
            info!(
                "No secrets backend configured. Bootstrap token created from environment variable. \
                 Rotation via API will not be available without Vault."
            );
        }

        // Hash the provided bootstrap token
        let hashed_secret = self.hash_secret(bootstrap_secret)?;
        let id_str = uuid::Uuid::new_v4().to_string();
        let id = TokenId::from_string(id_str.clone());

        let new_token = NewPersonalAccessToken {
            id: id.clone(),
            name: "bootstrap-admin".into(),
            description: Some("Bootstrap admin token from environment".into()),
            hashed_secret,
            status: TokenStatus::Active,
            expires_at: None,
            created_by: Some("system".into()),
            scopes: vec![
                "admin:all".into(), // Grant full admin access
            ],
        };

        self.repository.create_token(new_token).await?;

        let token_value = format!("fp_pat_{}.{}", id_str, bootstrap_secret);

        self.record_event(
            "auth.token.bootstrap_seeded",
            Some(&id_str),
            Some("bootstrap-admin"),
            json!({ "name": "bootstrap-admin", "source": "environment" }),
        )
        .await?;

        metrics::record_token_created(1).await;
        let active_count = self.repository.count_active_tokens().await?;
        metrics::set_active_tokens(active_count as usize).await;

        info!(token_id = %id_str, "bootstrap personal access token seeded from environment");
        Ok(Some(token_value))
    }

    #[instrument(
        skip(self, payload),
        fields(token_name = field::Empty, correlation_id = field::Empty)
    )]
    pub async fn create_token(&self, payload: CreateTokenRequest) -> Result<TokenSecretResponse> {
        payload.validate().map_err(Error::from)?;

        tracing::Span::current().record("token_name", field::display(&payload.name));
        let correlation_id = uuid::Uuid::new_v4();
        tracing::Span::current().record("correlation_id", field::display(&correlation_id));

        let id_str = uuid::Uuid::new_v4().to_string();
        let id = TokenId::from_string(id_str.clone());
        let secret = Self::generate_secret();
        let token_value = format!("fp_pat_{}.{}", id_str, secret);
        let hashed_secret = self.hash_secret(&secret)?;

        // Apply default 30-day expiry if not specified
        let expires_at =
            payload.expires_at.or_else(|| Some(Utc::now() + chrono::Duration::days(30)));

        let new_token = NewPersonalAccessToken {
            id: id.clone(),
            name: payload.name.clone(),
            description: payload.description.clone(),
            hashed_secret,
            status: TokenStatus::Active,
            expires_at,
            created_by: payload.created_by.clone(),
            scopes: payload.scopes.clone(),
        };

        self.repository.create_token(new_token).await?;
        self.record_event(
            "auth.token.created",
            Some(&id_str),
            Some(&payload.name),
            json!({ "scopes": payload.scopes, "created_by": payload.created_by }),
        )
        .await?;
        metrics::record_token_created(payload.scopes.len()).await;
        let active_count = self.repository.count_active_tokens().await?;
        metrics::set_active_tokens(active_count as usize).await;
        info!(%correlation_id, token_id = %id_str, "personal access token created");

        Ok(TokenSecretResponse { id: id_str, token: token_value })
    }

    #[instrument(
        skip(self),
        fields(limit = field::Empty, offset = field::Empty, correlation_id = field::Empty)
    )]
    pub async fn list_tokens(&self, limit: i64, offset: i64) -> Result<Vec<PersonalAccessToken>> {
        tracing::Span::current().record("limit", field::display(&limit));
        tracing::Span::current().record("offset", field::display(&offset));
        tracing::Span::current().record("correlation_id", field::display(&uuid::Uuid::new_v4()));
        self.repository.list_tokens(limit, offset).await
    }

    #[instrument(skip(self), fields(token_id = field::Empty, correlation_id = field::Empty))]
    pub async fn get_token(&self, id: &str) -> Result<PersonalAccessToken> {
        tracing::Span::current().record("token_id", field::display(id));
        tracing::Span::current().record("correlation_id", field::display(&uuid::Uuid::new_v4()));
        let token_id = TokenId::from_str_unchecked(id);
        self.repository.get_token(&token_id).await
    }

    #[instrument(
        skip(self, payload),
        fields(token_id = field::Empty, correlation_id = field::Empty)
    )]
    pub async fn update_token(
        &self,
        id: &str,
        payload: UpdateTokenRequest,
    ) -> Result<PersonalAccessToken> {
        payload.validate().map_err(Error::from)?;

        tracing::Span::current().record("token_id", field::display(id));
        let correlation_id = uuid::Uuid::new_v4();
        tracing::Span::current().record("correlation_id", field::display(&correlation_id));

        let status = if let Some(status) = payload.status.as_ref() {
            Some(TokenStatus::from_str(status).map_err(|_| Error::validation("Invalid status"))?)
        } else {
            None
        };

        let update = UpdatePersonalAccessToken {
            name: payload.name.clone(),
            description: payload.description.clone(),
            status,
            expires_at: payload.expires_at,
            scopes: payload.scopes.clone(),
        };

        let token_id = TokenId::from_str_unchecked(id);
        let token = self.repository.update_metadata(&token_id, update).await?;
        self.record_event(
            "auth.token.updated",
            Some(id),
            Some(&token.name),
            json!({
                "status": token.status.as_str(),
                "expires_at": token.expires_at,
                "scopes": token.scopes,
            }),
        )
        .await?;
        let active_count = self.repository.count_active_tokens().await?;
        metrics::set_active_tokens(active_count as usize).await;
        info!(%correlation_id, token_id = %token.id, "personal access token updated");
        Ok(token)
    }

    #[instrument(skip(self), fields(token_id = field::Empty, correlation_id = field::Empty))]
    pub async fn revoke_token(&self, id: &str) -> Result<PersonalAccessToken> {
        tracing::Span::current().record("token_id", field::display(id));
        let correlation_id = uuid::Uuid::new_v4();
        tracing::Span::current().record("correlation_id", field::display(&correlation_id));

        let update = UpdatePersonalAccessToken {
            name: None,
            description: None,
            status: Some(TokenStatus::Revoked),
            expires_at: None,
            scopes: Some(Vec::new()),
        };
        let token_id = TokenId::from_str_unchecked(id);
        let token = self.repository.update_metadata(&token_id, update).await?;
        self.record_event(
            "auth.token.revoked",
            Some(id),
            Some(&token.name),
            json!({ "status": token.status.as_str() }),
        )
        .await?;
        metrics::record_token_revoked().await;
        let active_count = self.repository.count_active_tokens().await?;
        metrics::set_active_tokens(active_count as usize).await;
        info!(%correlation_id, token_id = %token.id, "personal access token revoked");
        Ok(token)
    }

    #[instrument(skip(self), fields(token_id = field::Empty, correlation_id = field::Empty))]
    pub async fn rotate_token(&self, id: &str) -> Result<TokenSecretResponse> {
        tracing::Span::current().record("token_id", field::display(id));
        let correlation_id = uuid::Uuid::new_v4();
        tracing::Span::current().record("correlation_id", field::display(&correlation_id));

        let secret = Self::generate_secret();
        let token_value = format!("fp_pat_{}.{}", id, secret);
        let hashed_secret = self.hash_secret(&secret)?;

        let token_id = TokenId::from_str_unchecked(id);
        self.repository.rotate_secret(&token_id, hashed_secret).await?;
        self.record_event(
            "auth.token.rotated",
            Some(id),
            None,
            json!({ "rotated_at": Utc::now() }),
        )
        .await?;
        metrics::record_token_rotated().await;
        info!(%correlation_id, token_id = %id, "personal access token rotated");

        Ok(TokenSecretResponse { id: id.to_string(), token: token_value })
    }

    fn generate_secret() -> String {
        OsRng.sample_iter(&Alphanumeric).take(48).map(char::from).collect()
    }

    async fn record_event(
        &self,
        action: &str,
        resource_id: Option<&str>,
        resource_name: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<()> {
        self.audit_repository
            .record_auth_event(AuditEvent::token(action, resource_id, resource_name, metadata))
            .await
    }

    pub fn to_auth_context(&self, token: &PersonalAccessToken) -> AuthContext {
        AuthContext::new(token.id.clone(), token.name.clone(), token.scopes.clone())
    }

    /// Rotate the bootstrap token using a secrets backend.
    ///
    /// This method rotates the bootstrap token by:
    /// 1. Generating a new cryptographically secure secret using the secrets client
    /// 2. Updating the bootstrap token in the database with the new hashed secret
    /// 3. Recording the rotation in the audit log
    ///
    /// # Arguments
    ///
    /// * `secrets_client` - The secrets client to use for generating the new secret
    ///
    /// # Returns
    ///
    /// The new bootstrap token value in the format `fp_pat_{id}.{secret}`
    ///
    /// # Errors
    ///
    /// - Returns an error if no bootstrap token exists
    /// - Returns an error if the secrets client fails
    /// - Returns an error if the database update fails
    ///
    /// # Security
    ///
    /// The new secret is:
    /// - Generated using a cryptographically secure random number generator
    /// - Stored in the secrets backend for rotation tracking
    /// - Hashed using Argon2 before storage in the database
    /// - Audited with full rotation metadata
    #[instrument(skip(self, secrets_client), fields(correlation_id = field::Empty))]
    pub async fn rotate_bootstrap_token<T: SecretsClient>(
        &self,
        secrets_client: &T,
    ) -> Result<String> {
        let correlation_id = uuid::Uuid::new_v4();
        tracing::Span::current().record("correlation_id", field::display(&correlation_id));

        // Find the bootstrap token
        let tokens = self.repository.list_tokens(1000, 0).await?;
        let bootstrap_token = tokens
            .iter()
            .find(|t| t.name == "bootstrap-admin")
            .ok_or_else(|| Error::not_found("token", "bootstrap-admin"))?;

        // Rotate the secret in the secrets backend
        let new_secret = secrets_client
            .rotate_secret("bootstrap_token")
            .await
            .map_err(|e| Error::internal(format!("Failed to rotate bootstrap secret: {}", e)))?;

        // Hash the new secret
        let hashed_secret = self.hash_secret(&new_secret)?;

        // Update the token in the database
        self.repository.rotate_secret(&bootstrap_token.id, hashed_secret).await?;

        // Record the rotation event
        self.record_event(
            "auth.token.bootstrap_rotated",
            Some(bootstrap_token.id.as_ref()),
            Some("bootstrap-admin"),
            json!({
                "rotated_at": Utc::now(),
                "correlation_id": correlation_id.to_string()
            }),
        )
        .await?;

        metrics::record_token_rotated().await;
        info!(%correlation_id, token_id = %bootstrap_token.id, "bootstrap token rotated via secrets backend");

        // Return the new token value
        Ok(format!("fp_pat_{}.{}", bootstrap_token.id, new_secret))
    }
}
