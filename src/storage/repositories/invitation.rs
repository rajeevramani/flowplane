//! Invitation repository for invite-only registration.

use crate::auth::invitation::{Invitation, InvitationStatus};
use crate::auth::organization::OrgRole;
use crate::domain::{InvitationId, OrgId, UserId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use std::str::FromStr;
use tracing::instrument;

/// Database row for invitations.
#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)]
struct InvitationRow {
    pub id: String,
    pub org_id: String,
    pub email: String,
    pub role: String,
    pub token_hash: String,
    pub status: String,
    pub invited_by: Option<String>,
    pub accepted_by: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<InvitationRow> for Invitation {
    type Error = FlowplaneError;

    fn try_from(row: InvitationRow) -> Result<Self> {
        let status = InvitationStatus::from_str(&row.status).map_err(|e| {
            FlowplaneError::validation(format!("Invalid invitation status '{}': {}", row.status, e))
        })?;

        let role = OrgRole::from_str(&row.role).map_err(|e| {
            FlowplaneError::validation(format!("Invalid invitation role '{}': {}", row.role, e))
        })?;

        Ok(Invitation {
            id: InvitationId::from_string(row.id),
            org_id: OrgId::from_string(row.org_id),
            email: row.email,
            role,
            status,
            invited_by: row.invited_by.map(UserId::from_string),
            accepted_by: row.accepted_by.map(UserId::from_string),
            expires_at: row.expires_at,
            accepted_at: row.accepted_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Row for getting invitation + token_hash (for verification).
#[derive(Debug, Clone, FromRow)]
pub struct InvitationWithHash {
    pub id: String,
    pub org_id: String,
    pub email: String,
    pub role: String,
    pub token_hash: String,
    pub status: String,
    pub invited_by: Option<String>,
    pub accepted_by: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Repository trait for invitation operations.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait InvitationRepository: Send + Sync {
    async fn create_invitation(
        &self,
        id: &InvitationId,
        org_id: &OrgId,
        email: &str,
        role: &OrgRole,
        token_hash: &str,
        invited_by: Option<&UserId>,
        expires_at: DateTime<Utc>,
    ) -> Result<Invitation>;

    async fn get_invitation_by_id(&self, id: &InvitationId) -> Result<Option<Invitation>>;

    async fn get_invitation_with_hash(
        &self,
        id: &InvitationId,
    ) -> Result<Option<InvitationWithHash>>;

    async fn list_invitations_by_org(
        &self,
        org_id: &OrgId,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Invitation>>;

    async fn count_invitations_by_org(&self, org_id: &OrgId) -> Result<i64>;

    async fn has_pending_invitation(&self, email: &str, org_id: &OrgId) -> Result<bool>;

    async fn accept_invitation(
        &self,
        id: &InvitationId,
        accepted_by: &UserId,
    ) -> Result<Invitation>;

    async fn revoke_invitation(&self, id: &InvitationId) -> Result<()>;

    async fn expire_stale_invitations(&self) -> Result<u64>;
}

/// SQLx-based invitation repository implementation.
#[derive(Debug, Clone)]
pub struct SqlxInvitationRepository {
    pool: DbPool,
}

impl SqlxInvitationRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl InvitationRepository for SqlxInvitationRepository {
    #[instrument(skip(self, token_hash), fields(invitation_id = %id, org_id = %org_id, email = %email))]
    async fn create_invitation(
        &self,
        id: &InvitationId,
        org_id: &OrgId,
        email: &str,
        role: &OrgRole,
        token_hash: &str,
        invited_by: Option<&UserId>,
        expires_at: DateTime<Utc>,
    ) -> Result<Invitation> {
        let invited_by_str = invited_by.map(|u| u.as_str().to_string());

        let row = sqlx::query_as::<_, InvitationRow>(
            r#"
            INSERT INTO invitations (id, org_id, email, role, token_hash, status, invited_by, expires_at)
            VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7)
            RETURNING id, org_id, email, role, token_hash, status, invited_by, accepted_by,
                      expires_at, accepted_at, created_at, updated_at
            "#,
        )
        .bind(id.as_str())
        .bind(org_id.as_str())
        .bind(email)
        .bind(role.as_str())
        .bind(token_hash)
        .bind(&invited_by_str)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let Some(db_err) = e.as_database_error() {
                if db_err.code().as_deref() == Some("23505") {
                    return FlowplaneError::conflict(
                        "An invitation for this email is already pending. You can revoke it from the invitations list and create a new one.",
                        "invitation",
                    );
                }
            }
            FlowplaneError::internal(format!("Failed to create invitation: {}", e))
        })?;

        Invitation::try_from(row)
    }

    #[instrument(skip(self), fields(invitation_id = %id))]
    async fn get_invitation_by_id(&self, id: &InvitationId) -> Result<Option<Invitation>> {
        let row = sqlx::query_as::<_, InvitationRow>(
            r#"
            SELECT id, org_id, email, role, token_hash, status, invited_by, accepted_by,
                   expires_at, accepted_at, created_at, updated_at
            FROM invitations
            WHERE id = $1
            "#,
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::internal(format!("Failed to get invitation: {}", e)))?;

        row.map(Invitation::try_from).transpose()
    }

    #[instrument(skip(self), fields(invitation_id = %id))]
    async fn get_invitation_with_hash(
        &self,
        id: &InvitationId,
    ) -> Result<Option<InvitationWithHash>> {
        sqlx::query_as::<_, InvitationWithHash>(
            r#"
            SELECT id, org_id, email, role, token_hash, status, invited_by, accepted_by,
                   expires_at, accepted_at, created_at, updated_at
            FROM invitations
            WHERE id = $1
            "#,
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::internal(format!("Failed to get invitation: {}", e)))
    }

    #[instrument(skip(self), fields(org_id = %org_id, limit, offset))]
    async fn list_invitations_by_org(
        &self,
        org_id: &OrgId,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Invitation>> {
        let rows = sqlx::query_as::<_, InvitationRow>(
            r#"
            SELECT id, org_id, email, role, token_hash, status, invited_by, accepted_by,
                   expires_at, accepted_at, created_at, updated_at
            FROM invitations
            WHERE org_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(org_id.as_str())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::internal(format!("Failed to list invitations: {}", e)))?;

        rows.into_iter().map(Invitation::try_from).collect()
    }

    #[instrument(skip(self), fields(org_id = %org_id))]
    async fn count_invitations_by_org(&self, org_id: &OrgId) -> Result<i64> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM invitations WHERE org_id = $1")
            .bind(org_id.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|e| FlowplaneError::internal(format!("Failed to count invitations: {}", e)))?;

        Ok(count)
    }

    #[instrument(skip(self), fields(email = %email, org_id = %org_id))]
    async fn has_pending_invitation(&self, email: &str, org_id: &OrgId) -> Result<bool> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM invitations WHERE email = $1 AND org_id = $2 AND status = 'pending'",
        )
        .bind(email)
        .bind(org_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            FlowplaneError::internal(format!("Failed to check pending invitation: {}", e))
        })?;

        Ok(count > 0)
    }

    #[instrument(skip(self), fields(invitation_id = %id, accepted_by = %accepted_by))]
    async fn accept_invitation(
        &self,
        id: &InvitationId,
        accepted_by: &UserId,
    ) -> Result<Invitation> {
        let row = sqlx::query_as::<_, InvitationRow>(
            r#"
            UPDATE invitations
            SET status = 'accepted', accepted_by = $2, accepted_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND status = 'pending'
            RETURNING id, org_id, email, role, token_hash, status, invited_by, accepted_by,
                      expires_at, accepted_at, created_at, updated_at
            "#,
        )
        .bind(id.as_str())
        .bind(accepted_by.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::internal(format!("Failed to accept invitation: {}", e)))?
        .ok_or_else(|| FlowplaneError::not_found("invitation", id.as_str()))?;

        Invitation::try_from(row)
    }

    #[instrument(skip(self), fields(invitation_id = %id))]
    async fn revoke_invitation(&self, id: &InvitationId) -> Result<()> {
        let result = sqlx::query(
            "UPDATE invitations SET status = 'revoked', updated_at = NOW() WHERE id = $1 AND status = 'pending'",
        )
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::internal(format!("Failed to revoke invitation: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("invitation", id.as_str()));
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn expire_stale_invitations(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE invitations SET status = 'expired', updated_at = NOW() WHERE status = 'pending' AND expires_at < NOW()",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            FlowplaneError::internal(format!("Failed to expire stale invitations: {}", e))
        })?;

        Ok(result.rows_affected())
    }
}
