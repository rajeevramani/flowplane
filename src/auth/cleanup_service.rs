//! Background maintenance routines for personal access tokens.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;

use crate::auth::models::{TokenStatus, UpdatePersonalAccessToken};
use crate::errors::Result;
use crate::observability::metrics;
use crate::storage::repository::{
    AuditEvent, AuditLogRepository, SqlxTokenRepository, TokenRepository,
};

#[derive(Clone)]
pub struct CleanupService {
    repository: Arc<dyn TokenRepository>,
    audit_repository: Arc<AuditLogRepository>,
}

impl CleanupService {
    pub fn new(
        repository: Arc<dyn TokenRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { repository, audit_repository }
    }

    pub fn with_sqlx(
        pool: crate::storage::DbPool,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self::new(Arc::new(SqlxTokenRepository::new(pool)), audit_repository)
    }

    /// Scan for expired tokens and transition them to `expired` status.
    pub async fn run_once(&self) -> Result<()> {
        let tokens = self.repository.list_tokens(1000, 0, None).await?;
        let now = Utc::now();

        for token in tokens {
            if token.status == TokenStatus::Active {
                if let Some(expiry) = token.expires_at {
                    if expiry < now {
                        let update = UpdatePersonalAccessToken {
                            name: None,
                            description: None,
                            status: Some(TokenStatus::Expired),
                            expires_at: Some(Some(expiry)),
                            scopes: None,
                        };
                        let updated = self.repository.update_metadata(&token.id, update).await?;
                        self.audit_repository
                            .record_auth_event(AuditEvent::token(
                                "auth.token.expired",
                                Some(token.id.as_str()),
                                Some(&updated.name),
                                json!({ "expired_at": expiry }),
                            ))
                            .await?;
                    }
                }
            }
        }

        let active = self.repository.count_active_tokens().await?;
        metrics::set_active_tokens(active as usize).await;

        Ok(())
    }
}
