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
use crate::errors::{Error, Result};
use crate::observability::metrics;
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

    #[instrument(skip(self), fields(correlation_id = field::Empty))]
    pub async fn ensure_bootstrap_token(&self) -> Result<Option<TokenSecretResponse>> {
        tracing::Span::current().record("correlation_id", field::display(&uuid::Uuid::new_v4()));

        if self.repository.count_tokens().await? > 0 {
            let active_count = self.repository.count_active_tokens().await?;
            metrics::set_active_tokens(active_count as usize).await;
            return Ok(None);
        }

        let request = CreateTokenRequest {
            name: "bootstrap-admin".into(),
            description: Some("Initial bootstrap token".into()),
            expires_at: None,
            scopes: vec![
                "tokens:read".into(),
                "tokens:write".into(),
                "clusters:read".into(),
                "clusters:write".into(),
                "routes:read".into(),
                "routes:write".into(),
                "listeners:read".into(),
                "listeners:write".into(),
                "gateways:import".into(),
            ],
            created_by: Some("system".into()),
        };

        let response = self.create_token(request).await?;
        self.record_event(
            "auth.token.seeded",
            Some(response.id.as_str()),
            Some("bootstrap-admin"),
            json!({ "name": "bootstrap-admin" }),
        )
        .await?;
        info!(token_id = %response.id, "bootstrap personal access token seeded");
        Ok(Some(response))
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

        let id = uuid::Uuid::new_v4().to_string();
        let secret = Self::generate_secret();
        let token_value = format!("fp_pat_{}.{}", id, secret);
        let hashed_secret = self.hash_secret(&secret)?;

        let new_token = NewPersonalAccessToken {
            id: id.clone(),
            name: payload.name.clone(),
            description: payload.description.clone(),
            hashed_secret,
            status: TokenStatus::Active,
            expires_at: payload.expires_at,
            created_by: payload.created_by.clone(),
            scopes: payload.scopes.clone(),
        };

        self.repository.create_token(new_token).await?;
        self.record_event(
            "auth.token.created",
            Some(&id),
            Some(&payload.name),
            json!({ "scopes": payload.scopes, "created_by": payload.created_by }),
        )
        .await?;
        metrics::record_token_created(payload.scopes.len()).await;
        let active_count = self.repository.count_active_tokens().await?;
        metrics::set_active_tokens(active_count as usize).await;
        info!(%correlation_id, token_id = %id, "personal access token created");

        Ok(TokenSecretResponse { id, token: token_value })
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
        self.repository.get_token(id).await
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

        let token = self.repository.update_metadata(id, update).await?;
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
        let token = self.repository.update_metadata(id, update).await?;
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

        self.repository.rotate_secret(id, hashed_secret).await?;
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
}
