//! Organization repository for organization lifecycle management
//!
//! This module provides CRUD operations for organizations and organization
//! memberships, supporting the multi-tenancy governance layer.

use crate::auth::organization::{
    CreateOrganizationRequest, OrgRole, OrgStatus, Organization, OrganizationMembership,
    UpdateOrganizationRequest,
};
use crate::domain::{OrgId, UserId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use std::str::FromStr;
use tracing::instrument;

// Database row structures

#[derive(Debug, Clone, FromRow)]
struct OrganizationRow {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub owner_user_id: Option<String>,
    pub settings: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<OrganizationRow> for Organization {
    type Error = FlowplaneError;

    fn try_from(row: OrganizationRow) -> Result<Self> {
        let status = OrgStatus::from_str(&row.status).map_err(|e| {
            FlowplaneError::validation(format!(
                "Invalid organization status '{}': {}",
                row.status, e
            ))
        })?;

        let settings = if let Some(json_str) = row.settings {
            Some(serde_json::from_str(&json_str).map_err(|e| {
                FlowplaneError::validation(format!("Invalid organization settings JSON: {}", e))
            })?)
        } else {
            None
        };

        Ok(Organization {
            id: OrgId::from_string(row.id),
            name: row.name,
            display_name: row.display_name,
            description: row.description,
            owner_user_id: row.owner_user_id.map(|id| id.into()),
            settings,
            status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
struct OrgMembershipRow {
    pub id: String,
    pub user_id: String,
    pub org_id: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// Row type for membership queries that JOIN with organizations and users
#[derive(Debug, Clone, FromRow)]
struct OrgMembershipWithNameRow {
    pub id: String,
    pub user_id: String,
    pub org_id: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub org_name: String,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
}

fn parse_org_role(role_str: &str) -> Result<OrgRole> {
    OrgRole::from_str(role_str).map_err(|e| {
        FlowplaneError::validation(format!("Invalid organization role '{}': {}", role_str, e))
    })
}

impl TryFrom<OrgMembershipRow> for OrganizationMembership {
    type Error = FlowplaneError;

    fn try_from(row: OrgMembershipRow) -> Result<Self> {
        Ok(OrganizationMembership {
            id: row.id,
            user_id: UserId::from_string(row.user_id),
            org_id: OrgId::from_string(row.org_id),
            org_name: String::new(),
            role: parse_org_role(&row.role)?,
            created_at: row.created_at,
            user_name: None,
            user_email: None,
        })
    }
}

impl TryFrom<OrgMembershipWithNameRow> for OrganizationMembership {
    type Error = FlowplaneError;

    fn try_from(row: OrgMembershipWithNameRow) -> Result<Self> {
        Ok(OrganizationMembership {
            id: row.id,
            user_id: UserId::from_string(row.user_id),
            org_id: OrgId::from_string(row.org_id),
            org_name: row.org_name,
            role: parse_org_role(&row.role)?,
            created_at: row.created_at,
            user_name: row.user_name,
            user_email: row.user_email,
        })
    }
}

// Repository traits

#[async_trait]
pub trait OrganizationRepository: Send + Sync {
    async fn create_organization(&self, request: CreateOrganizationRequest)
        -> Result<Organization>;
    async fn get_organization_by_id(&self, id: &OrgId) -> Result<Option<Organization>>;
    async fn get_organization_by_name(&self, name: &str) -> Result<Option<Organization>>;
    async fn list_organizations(&self, limit: i64, offset: i64) -> Result<Vec<Organization>>;
    async fn count_organizations(&self) -> Result<i64>;
    async fn update_organization(
        &self,
        id: &OrgId,
        update: UpdateOrganizationRequest,
    ) -> Result<Organization>;
    async fn delete_organization(&self, id: &OrgId) -> Result<()>;
    async fn is_name_available(&self, name: &str) -> Result<bool>;
}

#[async_trait]
pub trait OrgMembershipRepository: Send + Sync {
    async fn create_membership(
        &self,
        user_id: &UserId,
        org_id: &OrgId,
        role: OrgRole,
    ) -> Result<OrganizationMembership>;
    async fn get_membership(
        &self,
        user_id: &UserId,
        org_id: &OrgId,
    ) -> Result<Option<OrganizationMembership>>;
    async fn get_membership_by_id(&self, id: &str) -> Result<Option<OrganizationMembership>>;
    async fn list_org_members(&self, org_id: &OrgId) -> Result<Vec<OrganizationMembership>>;
    async fn list_user_memberships(&self, user_id: &UserId) -> Result<Vec<OrganizationMembership>>;
    async fn update_membership_role(
        &self,
        user_id: &UserId,
        org_id: &OrgId,
        role: OrgRole,
        grant_pairs: &[(&str, &str)],
        created_by: &str,
    ) -> Result<OrganizationMembership>;
    async fn delete_membership(&self, user_id: &UserId, org_id: &OrgId) -> Result<()>;
}

// SQLx implementations

pub struct SqlxOrganizationRepository {
    pool: DbPool,
}

impl SqlxOrganizationRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OrganizationRepository for SqlxOrganizationRepository {
    #[instrument(skip(self, request), fields(org_name = %request.name), name = "db_create_organization")]
    async fn create_organization(
        &self,
        request: CreateOrganizationRequest,
    ) -> Result<Organization> {
        let id = OrgId::new();
        let now = Utc::now();
        let settings_json = request
            .settings
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid settings JSON: {}", e)))?;

        let row = sqlx::query_as::<_, OrganizationRow>(
            "INSERT INTO organizations (
                id, name, display_name, description, owner_user_id, settings, status, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *",
        )
        .bind(id.as_str())
        .bind(&request.name)
        .bind(&request.display_name)
        .bind(&request.description)
        .bind(request.owner_user_id.as_ref().map(|id| id.as_str()))
        .bind(settings_json.as_deref())
        .bind(OrgStatus::Active.as_str())
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to create organization".to_string(),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(org_id = %id), name = "db_get_organization_by_id")]
    async fn get_organization_by_id(&self, id: &OrgId) -> Result<Option<Organization>> {
        let row = sqlx::query_as::<_, OrganizationRow>("SELECT * FROM organizations WHERE id = $1")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to fetch organization by ID: {}", id),
            })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(org_name = %name), name = "db_get_organization_by_name")]
    async fn get_organization_by_name(&self, name: &str) -> Result<Option<Organization>> {
        let row =
            sqlx::query_as::<_, OrganizationRow>("SELECT * FROM organizations WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to fetch organization by name: {}", name),
                })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(limit = limit, offset = offset), name = "db_list_organizations")]
    async fn list_organizations(&self, limit: i64, offset: i64) -> Result<Vec<Organization>> {
        let rows = sqlx::query_as::<_, OrganizationRow>(
            "SELECT * FROM organizations ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list organizations".to_string(),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), name = "db_count_organizations")]
    async fn count_organizations(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM organizations")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to count organizations".to_string(),
            })?;

        Ok(count)
    }

    #[instrument(skip(self, update), fields(org_id = %id), name = "db_update_organization")]
    async fn update_organization(
        &self,
        id: &OrgId,
        update: UpdateOrganizationRequest,
    ) -> Result<Organization> {
        let mut tx = self.pool.begin().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to begin transaction for organization update".to_string(),
        })?;

        // SELECT ... FOR UPDATE to lock the row and prevent TOCTOU races
        let current = sqlx::query_as::<_, OrganizationRow>(
            "SELECT * FROM organizations WHERE id = $1 FOR UPDATE",
        )
        .bind(id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to fetch organization for update: {}", id),
        })?
        .ok_or_else(|| FlowplaneError::not_found("Organization", id.as_str()))?;

        let current_org = Organization::try_from(current)?;

        let display_name = update.display_name.unwrap_or(current_org.display_name);
        let description = update.description.or(current_org.description);
        let owner_user_id = update.owner_user_id.or(current_org.owner_user_id);
        let settings = update.settings.or(current_org.settings);
        let status = update.status.unwrap_or(current_org.status);

        let settings_json = settings
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid settings JSON: {}", e)))?;

        let row = sqlx::query_as::<_, OrganizationRow>(
            "UPDATE organizations SET
                display_name = $2,
                description = $3,
                owner_user_id = $4,
                settings = $5,
                status = $6,
                updated_at = $7
            WHERE id = $1
            RETURNING *",
        )
        .bind(id.as_str())
        .bind(&display_name)
        .bind(description.as_deref())
        .bind(owner_user_id.as_ref().map(|id| id.as_str()))
        .bind(settings_json.as_deref())
        .bind(status.as_str())
        .bind(Utc::now())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to update organization: {}", id),
        })?;

        tx.commit().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to commit organization update".to_string(),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(org_id = %id), name = "db_delete_organization")]
    async fn delete_organization(&self, id: &OrgId) -> Result<()> {
        let result = sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await;

        match result {
            Ok(result) => {
                if result.rows_affected() == 0 {
                    Err(FlowplaneError::not_found("Organization", id.as_str()))
                } else {
                    Ok(())
                }
            }
            Err(e) => {
                // Check for FK violation (PostgreSQL error code 23503)
                if let Some(db_err) = e.as_database_error() {
                    if db_err.code().as_deref() == Some("23503") {
                        return Err(FlowplaneError::validation(
                            "Cannot delete organization with active teams or members. Remove all teams and members first.",
                        ));
                    }
                }
                Err(FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete organization: {}", id),
                })
            }
        }
    }

    #[instrument(skip(self), fields(org_name = %name), name = "db_is_org_name_available")]
    async fn is_name_available(&self, name: &str) -> Result<bool> {
        let count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM organizations WHERE name = $1")
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check name availability: {}", name),
                })?;

        Ok(count == 0)
    }
}

// Organization membership repository

pub struct SqlxOrgMembershipRepository {
    pool: DbPool,
}

impl SqlxOrgMembershipRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OrgMembershipRepository for SqlxOrgMembershipRepository {
    #[instrument(skip(self), fields(user_id = %user_id, org_id = %org_id, role = %role), name = "db_create_org_membership")]
    async fn create_membership(
        &self,
        user_id: &UserId,
        org_id: &OrgId,
        role: OrgRole,
    ) -> Result<OrganizationMembership> {
        let id = uuid::Uuid::new_v4().to_string();

        let row = sqlx::query_as::<_, OrgMembershipWithNameRow>(
            "WITH inserted AS (
                INSERT INTO organization_memberships (id, user_id, org_id, role, created_at)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING *
            )
            SELECT i.id, i.user_id, i.org_id, i.role, i.created_at, o.name AS org_name,
                   u.name AS user_name, u.email AS user_email
            FROM inserted i
            JOIN organizations o ON o.id = i.org_id
            LEFT JOIN users u ON u.id = i.user_id",
        )
        .bind(&id)
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .bind(role.as_str())
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to create organization membership".to_string(),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(user_id = %user_id, org_id = %org_id), name = "db_get_org_membership")]
    async fn get_membership(
        &self,
        user_id: &UserId,
        org_id: &OrgId,
    ) -> Result<Option<OrganizationMembership>> {
        let row = sqlx::query_as::<_, OrgMembershipWithNameRow>(
            "SELECT om.id, om.user_id, om.org_id, om.role, om.created_at, o.name AS org_name,
                    u.name AS user_name, u.email AS user_email
            FROM organization_memberships om
            JOIN organizations o ON o.id = om.org_id
            LEFT JOIN users u ON u.id = om.user_id
            WHERE om.user_id = $1 AND om.org_id = $2",
        )
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch organization membership".to_string(),
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(membership_id = %id), name = "db_get_org_membership_by_id")]
    async fn get_membership_by_id(&self, id: &str) -> Result<Option<OrganizationMembership>> {
        let row = sqlx::query_as::<_, OrgMembershipWithNameRow>(
            "SELECT om.id, om.user_id, om.org_id, om.role, om.created_at, o.name AS org_name,
                    u.name AS user_name, u.email AS user_email
            FROM organization_memberships om
            JOIN organizations o ON o.id = om.org_id
            LEFT JOIN users u ON u.id = om.user_id
            WHERE om.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch organization membership by ID".to_string(),
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    #[instrument(skip(self), fields(org_id = %org_id), name = "db_list_org_members")]
    async fn list_org_members(&self, org_id: &OrgId) -> Result<Vec<OrganizationMembership>> {
        let rows = sqlx::query_as::<_, OrgMembershipWithNameRow>(
            "SELECT om.id, om.user_id, om.org_id, om.role, om.created_at, o.name AS org_name,
                    u.name AS user_name, u.email AS user_email
            FROM organization_memberships om
            JOIN organizations o ON o.id = om.org_id
            LEFT JOIN users u ON u.id = om.user_id
            WHERE om.org_id = $1
            ORDER BY om.created_at",
        )
        .bind(org_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list org members".to_string(),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self), fields(user_id = %user_id), name = "db_list_user_org_memberships")]
    async fn list_user_memberships(&self, user_id: &UserId) -> Result<Vec<OrganizationMembership>> {
        let rows = sqlx::query_as::<_, OrgMembershipWithNameRow>(
            "SELECT om.id, om.user_id, om.org_id, om.role, om.created_at, o.name AS org_name,
                    u.name AS user_name, u.email AS user_email
            FROM organization_memberships om
            JOIN organizations o ON o.id = om.org_id
            LEFT JOIN users u ON u.id = om.user_id
            WHERE om.user_id = $1
            ORDER BY om.created_at",
        )
        .bind(user_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list user org memberships".to_string(),
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    #[instrument(skip(self, grant_pairs, created_by), fields(user_id = %user_id, org_id = %org_id, role = %role), name = "db_update_org_membership_role")]
    async fn update_membership_role(
        &self,
        user_id: &UserId,
        org_id: &OrgId,
        role: OrgRole,
        grant_pairs: &[(&str, &str)],
        created_by: &str,
    ) -> Result<OrganizationMembership> {
        let mut tx = self.pool.begin().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to begin transaction for membership role update".to_string(),
        })?;

        // Lock the target membership row and get current role
        let current_role = sqlx::query_scalar::<_, String>(
            "SELECT role FROM organization_memberships
            WHERE user_id = $1 AND org_id = $2 FOR UPDATE",
        )
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch membership for update".to_string(),
        })?
        .ok_or_else(|| {
            FlowplaneError::not_found(
                "OrganizationMembership",
                format!("user={}, org={}", user_id, org_id),
            )
        })?;

        // Atomically check last-owner constraint before downgrading
        if current_role == OrgRole::Owner.as_str() && role != OrgRole::Owner {
            let owner_count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM organization_memberships
                WHERE org_id = $1 AND role = 'owner'",
            )
            .bind(org_id.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to count org owners".to_string(),
            })?;

            if owner_count <= 1 {
                return Err(FlowplaneError::conflict(
                    "Cannot downgrade the last owner of an organization",
                    "OrganizationMembership",
                ));
            }
        }

        let row = sqlx::query_as::<_, OrgMembershipWithNameRow>(
            "WITH updated AS (
                UPDATE organization_memberships SET role = $3
                WHERE user_id = $1 AND org_id = $2
                RETURNING *
            )
            SELECT upd.id, upd.user_id, upd.org_id, upd.role, upd.created_at, o.name AS org_name,
                   usr.name AS user_name, usr.email AS user_email
            FROM updated upd
            JOIN organizations o ON o.id = upd.org_id
            LEFT JOIN users usr ON usr.id = upd.user_id",
        )
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .bind(role.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to update organization membership role".to_string(),
        })?;

        // Grant sync: delete all existing grants for this user in this org, then
        // re-insert based on new role. Admin/Owner roles get implicit access (no explicit
        // grants needed), so grant_pairs will be empty for those roles.
        sqlx::query("DELETE FROM grants WHERE principal_id = $1 AND org_id = $2")
            .bind(user_id.as_str())
            .bind(org_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to delete existing grants on role change".to_string(),
            })?;

        if !grant_pairs.is_empty() {
            // Fetch all team_ids the user belongs to within this org
            let team_ids: Vec<String> = sqlx::query_scalar(
                "SELECT utm.team FROM user_team_memberships utm
                 JOIN teams t ON t.id = utm.team
                 WHERE utm.user_id = $1 AND t.org_id = $2",
            )
            .bind(user_id.as_str())
            .bind(org_id.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to fetch team memberships for grant sync".to_string(),
            })?;

            for team_id in &team_ids {
                for (resource_type, action) in grant_pairs {
                    let grant_id = uuid::Uuid::new_v4().to_string();
                    sqlx::query(
                        "INSERT INTO grants \
                         (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by) \
                         VALUES ($1, $2, $3, $4, 'resource', $5, $6, $7) \
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(&grant_id)
                    .bind(user_id.as_str())
                    .bind(org_id.as_str())
                    .bind(team_id)
                    .bind(resource_type)
                    .bind(action)
                    .bind(created_by)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| FlowplaneError::Database {
                        source: e,
                        context: "Failed to insert default grant on role change".to_string(),
                    })?;
                }
            }
        }

        tx.commit().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to commit membership role update".to_string(),
        })?;

        row.try_into()
    }

    #[instrument(skip(self), fields(user_id = %user_id, org_id = %org_id), name = "db_delete_org_membership")]
    async fn delete_membership(&self, user_id: &UserId, org_id: &OrgId) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to begin transaction for membership deletion".to_string(),
        })?;

        // Lock the target membership row and get current role
        let current_role = sqlx::query_scalar::<_, String>(
            "SELECT role FROM organization_memberships
            WHERE user_id = $1 AND org_id = $2 FOR UPDATE",
        )
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to fetch membership for deletion".to_string(),
        })?
        .ok_or_else(|| {
            FlowplaneError::not_found(
                "OrganizationMembership",
                format!("user={}, org={}", user_id, org_id),
            )
        })?;

        // Atomically check last-owner constraint before deleting an owner
        if current_role == OrgRole::Owner.as_str() {
            let owner_count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM organization_memberships
                WHERE org_id = $1 AND role = 'owner'",
            )
            .bind(org_id.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to count org owners".to_string(),
            })?;

            if owner_count <= 1 {
                return Err(FlowplaneError::conflict(
                    "Cannot remove the last owner of an organization",
                    "OrganizationMembership",
                ));
            }
        }

        // Delete all grants for this user in the org before removing the membership.
        // This prevents orphaned grants that would give removed users continued access.
        sqlx::query("DELETE FROM grants WHERE principal_id = $1 AND org_id = $2")
            .bind(user_id.as_str())
            .bind(org_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to delete grants on org membership removal".to_string(),
            })?;

        sqlx::query("DELETE FROM organization_memberships WHERE user_id = $1 AND org_id = $2")
            .bind(user_id.as_str())
            .bind(org_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to delete organization membership".to_string(),
            })?;

        tx.commit().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to commit membership deletion".to_string(),
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_create_and_get_organization() {
        let _db = TestDatabase::new("org_create_get").await;
        let pool = _db.pool.clone();
        let repo = SqlxOrganizationRepository::new(pool);

        let request = CreateOrganizationRequest {
            name: "acme-corp".to_string(),
            display_name: "Acme Corporation".to_string(),
            description: Some("A test organization".to_string()),
            owner_user_id: None,
            settings: None,
        };

        let created = repo.create_organization(request).await.expect("create org");

        assert_eq!(created.name, "acme-corp");
        assert_eq!(created.display_name, "Acme Corporation");
        assert_eq!(created.status, OrgStatus::Active);

        // Get by ID
        let by_id = repo.get_organization_by_id(&created.id).await.expect("get by id");
        assert!(by_id.is_some());
        assert_eq!(by_id.as_ref().unwrap().id, created.id);

        // Get by name
        let by_name = repo.get_organization_by_name("acme-corp").await.expect("get by name");
        assert!(by_name.is_some());
        assert_eq!(by_name.unwrap().id, created.id);
    }

    #[tokio::test]
    async fn test_create_org_duplicate_name_fails() {
        let _db = TestDatabase::new("org_dup_name").await;
        let pool = _db.pool.clone();
        let repo = SqlxOrganizationRepository::new(pool);

        let request = CreateOrganizationRequest {
            name: "unique-org".to_string(),
            display_name: "Unique Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        };

        repo.create_organization(request.clone()).await.expect("first create");
        let result = repo.create_organization(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_organization() {
        let _db = TestDatabase::new("org_update").await;
        let pool = _db.pool.clone();
        let repo = SqlxOrganizationRepository::new(pool);

        let request = CreateOrganizationRequest {
            name: "update-org".to_string(),
            display_name: "Update Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        };

        let created = repo.create_organization(request).await.expect("create org");

        let update = UpdateOrganizationRequest {
            display_name: Some("Updated Org".to_string()),
            description: Some("Updated description".to_string()),
            owner_user_id: None,
            settings: None,
            status: Some(OrgStatus::Suspended),
        };

        let updated = repo.update_organization(&created.id, update).await.expect("update org");

        assert_eq!(updated.name, "update-org"); // Name is immutable
        assert_eq!(updated.display_name, "Updated Org");
        assert_eq!(updated.description.as_deref(), Some("Updated description"));
        assert_eq!(updated.status, OrgStatus::Suspended);
    }

    #[tokio::test]
    async fn test_list_organizations() {
        let _db = TestDatabase::new("org_list").await;
        let pool = _db.pool.clone();
        let repo = SqlxOrganizationRepository::new(pool);

        for i in 1..=3 {
            let request = CreateOrganizationRequest {
                name: format!("org-{}", i),
                display_name: format!("Org {}", i),
                description: None,
                owner_user_id: None,
                settings: None,
            };
            repo.create_organization(request).await.expect("create org");
        }

        let orgs = repo.list_organizations(10, 0).await.expect("list orgs");
        // 3 created + 1 from seed data (test-org)
        assert_eq!(orgs.len(), 4);

        let count = repo.count_organizations().await.expect("count orgs");
        assert_eq!(count, 4);
    }

    #[tokio::test]
    async fn test_delete_organization() {
        let _db = TestDatabase::new("org_delete").await;
        let pool = _db.pool.clone();
        let repo = SqlxOrganizationRepository::new(pool);

        let request = CreateOrganizationRequest {
            name: "delete-org".to_string(),
            display_name: "Delete Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        };

        let created = repo.create_organization(request).await.expect("create org");

        repo.delete_organization(&created.id).await.expect("delete org");

        let fetched = repo.get_organization_by_id(&created.id).await.expect("get deleted org");
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_is_name_available() {
        let _db = TestDatabase::new("org_name_available").await;
        let pool = _db.pool.clone();
        let repo = SqlxOrganizationRepository::new(pool);

        assert!(repo.is_name_available("new-org").await.expect("check availability"));

        let request = CreateOrganizationRequest {
            name: "taken-org".to_string(),
            display_name: "Taken Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        };
        repo.create_organization(request).await.expect("create org");

        assert!(!repo.is_name_available("taken-org").await.expect("check availability"));
    }

    #[tokio::test]
    async fn test_org_membership_crud() {
        let _db = TestDatabase::new("org_membership_crud").await;
        let pool = _db.pool.clone();

        // Create an org
        let org_repo = SqlxOrganizationRepository::new(pool.clone());
        let org = org_repo
            .create_organization(CreateOrganizationRequest {
                name: "membership-org".to_string(),
                display_name: "Membership Org".to_string(),
                description: None,
                owner_user_id: None,
                settings: None,
            })
            .await
            .expect("create org");

        // Create a user
        use crate::auth::user::{NewUser, UserStatus};
        use crate::storage::repositories::{SqlxUserRepository, UserRepository};

        let user_repo = SqlxUserRepository::new(pool.clone());
        let user_id = UserId::new();
        let password_hash = "dummy-hash-zitadel-handles-auth".to_string();
        let user = user_repo
            .create_user(NewUser {
                id: user_id.clone(),
                email: "member@test.com".to_string(),
                password_hash,
                name: "Test Member".to_string(),
                status: UserStatus::Active,
                is_admin: false,
            })
            .await
            .expect("create user");

        // Create membership
        let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
        let membership = membership_repo
            .create_membership(&user.id, &org.id, OrgRole::Member)
            .await
            .expect("create membership");

        assert_eq!(membership.user_id, user.id);
        assert_eq!(membership.org_id, org.id);
        assert_eq!(membership.role, OrgRole::Member);

        // Get membership
        let fetched =
            membership_repo.get_membership(&user.id, &org.id).await.expect("get membership");
        assert!(fetched.is_some());

        // Update role (Admin gets no explicit grants — implicit access)
        let updated = membership_repo
            .update_membership_role(&user.id, &org.id, OrgRole::Admin, &[], "system")
            .await
            .expect("update role");
        assert_eq!(updated.role, OrgRole::Admin);

        // List org members
        let members = membership_repo.list_org_members(&org.id).await.expect("list members");
        assert_eq!(members.len(), 1);

        // Delete membership
        membership_repo.delete_membership(&user.id, &org.id).await.expect("delete membership");

        let deleted = membership_repo
            .get_membership(&user.id, &org.id)
            .await
            .expect("get deleted membership");
        assert!(deleted.is_none());
    }

    /// Helper: count grants for a principal in an org
    #[allow(dead_code)]
    async fn count_grants_for_user_in_org(pool: &DbPool, user_id: &str, org_id: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM grants WHERE principal_id = $1 AND org_id = $2",
        )
        .bind(user_id)
        .bind(org_id)
        .fetch_one(pool)
        .await
        .expect("count grants")
    }

    /// Helper: count grants for a principal in a team
    #[allow(dead_code)]
    async fn count_grants_for_user_in_team(pool: &DbPool, user_id: &str, team_id: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM grants WHERE principal_id = $1 AND team_id = $2",
        )
        .bind(user_id)
        .bind(team_id)
        .fetch_one(pool)
        .await
        .expect("count grants in team")
    }

    /// Set up a test org, user, team, org membership, team membership, and grants.
    /// Returns (pool, org_id, user_id, team_id).
    #[allow(dead_code)]
    async fn setup_user_with_grants(
        db_name: &str,
    ) -> (crate::storage::DbPool, OrgId, UserId, String) {
        let _db = TestDatabase::new(db_name).await;
        let pool = _db.pool.clone();

        use crate::auth::user::{NewUser, UserStatus};
        use crate::storage::repositories::{SqlxUserRepository, UserRepository};

        let org_repo = SqlxOrganizationRepository::new(pool.clone());
        let org = org_repo
            .create_organization(CreateOrganizationRequest {
                name: format!("{}-org", db_name),
                display_name: "Test Org".to_string(),
                description: None,
                owner_user_id: None,
                settings: None,
            })
            .await
            .expect("create org");

        let user_repo = SqlxUserRepository::new(pool.clone());
        let user_id = UserId::new();
        let user = user_repo
            .create_user(NewUser {
                id: user_id,
                email: format!("user@{}.test", db_name),
                password_hash: "hash".to_string(),
                name: "Test User".to_string(),
                status: UserStatus::Active,
                is_admin: false,
            })
            .await
            .expect("create user");

        // Create a team in the org
        let team_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO teams (id, name, org_id, display_name) VALUES ($1, $2, $3, $4)")
            .bind(&team_id)
            .bind(format!("{}-team", db_name))
            .bind(org.id.as_str())
            .bind("Test Team")
            .execute(&pool)
            .await
            .expect("create team");

        // Add user to team via user_team_memberships
        let utm_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO user_team_memberships (id, user_id, team) VALUES ($1, $2, $3)")
            .bind(&utm_id)
            .bind(user.id.as_str())
            .bind(&team_id)
            .execute(&pool)
            .await
            .expect("create team membership");

        // Add org membership
        let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
        membership_repo
            .create_membership(&user.id, &org.id, OrgRole::Member)
            .await
            .expect("create org membership");

        // Insert a grant for the user in this team+org
        let grant_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO grants (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by) \
             VALUES ($1, $2, $3, $4, 'resource', 'clusters', 'read', $5)",
        )
        .bind(&grant_id)
        .bind(user.id.as_str())
        .bind(org.id.as_str())
        .bind(&team_id)
        .bind(user.id.as_str())
        .execute(&pool)
        .await
        .expect("insert grant");

        // Leak _db so it lives as long as pool — caller must hold the returned pool
        std::mem::forget(_db);

        (pool, org.id, user.id, team_id)
    }

    #[cfg(feature = "postgres_tests")]
    #[tokio::test]
    async fn test_delete_membership_removes_grants() {
        let (pool, org_id, user_id, _team_id) =
            setup_user_with_grants("del_membership_grants").await;

        let count_before =
            count_grants_for_user_in_org(&pool, user_id.as_str(), org_id.as_str()).await;
        assert_eq!(count_before, 1, "grant should exist before deletion");

        let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
        membership_repo.delete_membership(&user_id, &org_id).await.expect("delete membership");

        let count_after =
            count_grants_for_user_in_org(&pool, user_id.as_str(), org_id.as_str()).await;
        assert_eq!(count_after, 0, "grants should be deleted with membership");
    }

    #[cfg(feature = "postgres_tests")]
    #[tokio::test]
    async fn test_update_membership_role_to_admin_removes_grants() {
        let (pool, org_id, user_id, _team_id) =
            setup_user_with_grants("role_admin_removes_grants").await;

        let count_before =
            count_grants_for_user_in_org(&pool, user_id.as_str(), org_id.as_str()).await;
        assert_eq!(count_before, 1, "grant should exist before role change");

        let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
        // Promote to Admin: implicit access — all explicit grants should be removed
        membership_repo
            .update_membership_role(&user_id, &org_id, OrgRole::Admin, &[], "system")
            .await
            .expect("update role");

        let count_after =
            count_grants_for_user_in_org(&pool, user_id.as_str(), org_id.as_str()).await;
        assert_eq!(count_after, 0, "explicit grants should be cleared for admin role");
    }

    #[cfg(feature = "postgres_tests")]
    #[tokio::test]
    async fn test_update_membership_role_to_member_inserts_default_grants() {
        let (pool, org_id, user_id, team_id) =
            setup_user_with_grants("role_member_inserts_grants").await;

        // First promote to admin (clears grants)
        let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
        membership_repo
            .update_membership_role(&user_id, &org_id, OrgRole::Admin, &[], "system")
            .await
            .expect("promote to admin");

        let count_after_admin =
            count_grants_for_user_in_org(&pool, user_id.as_str(), org_id.as_str()).await;
        assert_eq!(count_after_admin, 0, "no grants after admin promotion");

        // Demote back to Member: should get default read grants per team
        let member_pairs: Vec<(&str, &str)> = vec![("clusters", "read"), ("routes", "read")];
        membership_repo
            .update_membership_role(
                &user_id,
                &org_id,
                OrgRole::Member,
                &member_pairs,
                user_id.as_str(),
            )
            .await
            .expect("demote to member");

        let count_after_member =
            count_grants_for_user_in_team(&pool, user_id.as_str(), &team_id).await;
        assert_eq!(
            count_after_member,
            member_pairs.len() as i64,
            "default grants should be inserted per team"
        );
    }

    #[cfg(feature = "postgres_tests")]
    #[tokio::test]
    async fn test_update_membership_role_demote_replaces_grants() {
        let (pool, org_id, user_id, team_id) =
            setup_user_with_grants("role_demote_replaces_grants").await;

        // User starts with 1 grant (clusters:read from setup)
        let count_before =
            count_grants_for_user_in_org(&pool, user_id.as_str(), org_id.as_str()).await;
        assert_eq!(count_before, 1);

        // Demote to Viewer with different grants
        let viewer_pairs: Vec<(&str, &str)> = vec![("routes", "read")];
        let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
        membership_repo
            .update_membership_role(
                &user_id,
                &org_id,
                OrgRole::Viewer,
                &viewer_pairs,
                user_id.as_str(),
            )
            .await
            .expect("demote to viewer");

        // Old grants (clusters:read) gone, new grants (routes:read) inserted
        let count_after = count_grants_for_user_in_team(&pool, user_id.as_str(), &team_id).await;
        assert_eq!(count_after, 1, "exactly viewer grants should exist");

        // Verify it's the right grant
        let grant_action: Option<String> = sqlx::query_scalar(
            "SELECT action FROM grants WHERE principal_id = $1 AND team_id = $2 AND resource_type = 'routes'",
        )
        .bind(user_id.as_str())
        .bind(&team_id)
        .fetch_optional(&pool)
        .await
        .expect("fetch grant action");
        assert_eq!(grant_action.as_deref(), Some("read"));
    }
}
