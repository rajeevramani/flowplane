//! User and team membership repository for user management
//!
//! This module provides CRUD operations for users and team memberships,
//! including user authentication and team access management.

use crate::auth::user::{
    NewUser, NewUserTeamMembership, UpdateUser, User, UserStatus, UserTeamMembership,
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
struct UserRow {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub name: String,
    pub status: String,
    pub is_admin: bool,
    pub org_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct UserTeamMembershipRow {
    pub id: String,
    pub user_id: String,
    pub team: String,
    pub scopes: String, // JSON array stored as string
    pub created_at: DateTime<Utc>,
}

// Repository traits

#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Create a new user
    async fn create_user(&self, user: NewUser) -> Result<User>;

    /// Get a user by ID
    async fn get_user(&self, id: &UserId) -> Result<Option<User>>;

    /// Get a user by email
    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>>;

    /// Get a user with their password hash for authentication
    async fn get_user_with_password(&self, email: &str) -> Result<Option<(User, String)>>;

    /// Update a user's details
    async fn update_user(&self, id: &UserId, update: UpdateUser) -> Result<User>;

    /// Update a user's password hash
    async fn update_password(&self, id: &UserId, password_hash: String) -> Result<()>;

    /// Update a user's organization ID
    async fn update_user_org(&self, id: &UserId, org_id: &crate::domain::OrgId) -> Result<()>;

    /// List all users (with pagination)
    async fn list_users(&self, limit: i64, offset: i64) -> Result<Vec<User>>;

    /// Count total users
    async fn count_users(&self) -> Result<i64>;

    /// Count users by status
    async fn count_users_by_status(&self, status: UserStatus) -> Result<i64>;

    /// Delete a user (this will cascade delete team memberships)
    async fn delete_user(&self, id: &UserId) -> Result<()>;
}

#[async_trait]
pub trait TeamMembershipRepository: Send + Sync {
    /// Create a new team membership for a user
    async fn create_membership(
        &self,
        membership: NewUserTeamMembership,
    ) -> Result<UserTeamMembership>;

    /// Get all team memberships for a user
    async fn list_user_memberships(&self, user_id: &UserId) -> Result<Vec<UserTeamMembership>>;

    /// Get all users in a team
    async fn list_team_members(&self, team: &str) -> Result<Vec<UserTeamMembership>>;

    /// Get all distinct teams across all memberships
    async fn list_all_teams(&self) -> Result<Vec<String>>;

    /// Get a specific membership by ID
    async fn get_membership(&self, id: &str) -> Result<Option<UserTeamMembership>>;

    /// Get a user's membership for a specific team
    async fn get_user_team_membership(
        &self,
        user_id: &UserId,
        team: &str,
    ) -> Result<Option<UserTeamMembership>>;

    /// Update scopes for a membership
    async fn update_membership_scopes(
        &self,
        id: &str,
        scopes: Vec<String>,
    ) -> Result<UserTeamMembership>;

    /// Delete a team membership
    async fn delete_membership(&self, id: &str) -> Result<()>;

    /// Delete all memberships for a user in a team
    async fn delete_user_team_membership(&self, user_id: &UserId, team: &str) -> Result<()>;
}

// PostgreSQL implementations

#[derive(Debug, Clone)]
pub struct SqlxUserRepository {
    pool: DbPool,
}

impl SqlxUserRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    fn row_to_user(&self, row: UserRow) -> Result<User> {
        let status = UserStatus::from_str(&row.status).map_err(|_| {
            FlowplaneError::validation(format!("Unknown user status '{}'", row.status))
        })?;

        Ok(User {
            id: UserId::from_string(row.id),
            email: row.email,
            name: row.name,
            status,
            is_admin: row.is_admin,
            org_id: OrgId::from_string(row.org_id),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    #[instrument(skip(self, user), fields(user_email = %user.email, user_id = %user.id), name = "db_create_user")]
    async fn create_user(&self, user: NewUser) -> Result<User> {
        let id = user.id.to_string();
        let status = user.status.to_string();

        sqlx::query(
            r#"
            INSERT INTO users (id, email, password_hash, name, status, is_admin, org_id, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(&id)
        .bind(&user.email)
        .bind(&user.password_hash)
        .bind(&user.name)
        .bind(&status)
        .bind(user.is_admin)
        .bind(user.org_id.as_str())
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to create user".to_string(),
        })?;

        self.get_user(&user.id)
            .await?
            .ok_or_else(|| FlowplaneError::internal("User not found after creation"))
    }

    #[instrument(skip(self), fields(user_id = %id), name = "db_get_user")]
    async fn get_user(&self, id: &UserId) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, name, status, is_admin, org_id, created_at, updated_at FROM users WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch user".to_string(),
        })?;

        row.map(|r| self.row_to_user(r)).transpose()
    }

    #[instrument(skip(self), fields(user_email = %email), name = "db_get_user_by_email")]
    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, name, status, is_admin, org_id, created_at, updated_at FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch user by email".to_string(),
        })?;

        row.map(|r| self.row_to_user(r)).transpose()
    }

    #[instrument(skip(self), fields(user_email = %email), name = "db_get_user_with_password")]
    async fn get_user_with_password(&self, email: &str) -> Result<Option<(User, String)>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, name, status, is_admin, org_id, created_at, updated_at FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch user with password".to_string(),
        })?;

        if let Some(row) = row {
            let password_hash = row.password_hash.clone();
            let user = self.row_to_user(row)?;
            Ok(Some((user, password_hash)))
        } else {
            Ok(None)
        }
    }

    #[instrument(skip(self, update), fields(user_id = %id), name = "db_update_user")]
    async fn update_user(&self, id: &UserId, update: UpdateUser) -> Result<User> {
        let current = self
            .get_user(id)
            .await?
            .ok_or_else(|| FlowplaneError::not_found("User", id.to_string()))?;

        let email = update.email.unwrap_or(current.email);
        let name = update.name.unwrap_or(current.name);
        let status = update.status.unwrap_or(current.status).to_string();
        let is_admin = update.is_admin.unwrap_or(current.is_admin);

        sqlx::query(
            r#"
            UPDATE users
            SET email = $1, name = $2, status = $3, is_admin = $4, updated_at = $5
            WHERE id = $6
            "#,
        )
        .bind(&email)
        .bind(&name)
        .bind(&status)
        .bind(is_admin)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to update user".to_string(),
        })?;

        self.get_user(id)
            .await?
            .ok_or_else(|| FlowplaneError::internal("User not found after update"))
    }

    #[instrument(skip(self, password_hash), fields(user_id = %id), name = "db_update_password")]
    async fn update_password(&self, id: &UserId, password_hash: String) -> Result<()> {
        sqlx::query("UPDATE users SET password_hash = $1, updated_at = $2 WHERE id = $3")
            .bind(&password_hash)
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to update password".to_string(),
            })?;

        Ok(())
    }

    #[instrument(skip(self), fields(user_id = %id), name = "db_update_user_org")]
    async fn update_user_org(&self, id: &UserId, org_id: &crate::domain::OrgId) -> Result<()> {
        sqlx::query("UPDATE users SET org_id = $1, updated_at = $2 WHERE id = $3")
            .bind(org_id.as_str())
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to update user organization".to_string(),
            })?;

        Ok(())
    }

    #[instrument(skip(self), fields(limit = limit, offset = offset), name = "db_list_users")]
    async fn list_users(&self, limit: i64, offset: i64) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, name, status, is_admin, org_id, created_at, updated_at FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to list users".to_string(),
        })?;

        rows.into_iter().map(|r| self.row_to_user(r)).collect()
    }

    #[instrument(skip(self), name = "db_count_users")]
    async fn count_users(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to count users".to_string(),
            })?;

        Ok(count)
    }

    #[instrument(skip(self), fields(status = %status), name = "db_count_users_by_status")]
    async fn count_users_by_status(&self, status: UserStatus) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE status = $1")
            .bind(status.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to count users by status".to_string(),
            })?;

        Ok(count)
    }

    #[instrument(skip(self), fields(user_id = %id), name = "db_delete_user")]
    async fn delete_user(&self, id: &UserId) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to delete user".to_string(),
            })?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SqlxTeamMembershipRepository {
    pool: DbPool,
}

impl SqlxTeamMembershipRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    fn row_to_membership(&self, row: UserTeamMembershipRow) -> Result<UserTeamMembership> {
        // Parse scopes JSON array
        let scopes: Vec<String> = serde_json::from_str(&row.scopes).map_err(|err| {
            FlowplaneError::internal(format!("Failed to parse scopes JSON: {}", err))
        })?;

        Ok(UserTeamMembership {
            id: row.id,
            user_id: UserId::from_string(row.user_id),
            team: row.team,
            scopes,
            created_at: row.created_at,
        })
    }
}

#[async_trait]
impl TeamMembershipRepository for SqlxTeamMembershipRepository {
    #[instrument(skip(self, membership), fields(user_id = %membership.user_id, team = %membership.team), name = "db_create_membership")]
    async fn create_membership(
        &self,
        membership: NewUserTeamMembership,
    ) -> Result<UserTeamMembership> {
        let scopes_json = serde_json::to_string(&membership.scopes).map_err(|err| {
            FlowplaneError::internal(format!("Failed to serialize scopes: {}", err))
        })?;

        sqlx::query(
            r#"
            INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(&membership.id)
        .bind(membership.user_id.to_string())
        .bind(&membership.team)
        .bind(&scopes_json)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to create team membership".to_string(),
        })?;

        self.get_membership(&membership.id)
            .await?
            .ok_or_else(|| FlowplaneError::internal("Membership not found after creation"))
    }

    #[instrument(skip(self), fields(user_id = %user_id), name = "db_list_user_memberships")]
    async fn list_user_memberships(&self, user_id: &UserId) -> Result<Vec<UserTeamMembership>> {
        let rows = sqlx::query_as::<_, UserTeamMembershipRow>(
            "SELECT id, user_id, team, scopes, created_at FROM user_team_memberships WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to list user memberships".to_string(),
        })?;

        rows.into_iter().map(|r| self.row_to_membership(r)).collect()
    }

    #[instrument(skip(self), fields(team = %team), name = "db_list_team_members")]
    async fn list_team_members(&self, team: &str) -> Result<Vec<UserTeamMembership>> {
        let rows = sqlx::query_as::<_, UserTeamMembershipRow>(
            "SELECT id, user_id, team, scopes, created_at FROM user_team_memberships WHERE team = $1 ORDER BY created_at",
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to list team members".to_string(),
        })?;

        rows.into_iter().map(|r| self.row_to_membership(r)).collect()
    }

    #[instrument(skip(self), name = "db_list_all_teams")]
    async fn list_all_teams(&self) -> Result<Vec<String>> {
        #[derive(FromRow)]
        struct TeamRow {
            team: String,
        }

        let rows = sqlx::query_as::<_, TeamRow>(
            "SELECT DISTINCT team FROM user_team_memberships ORDER BY team",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to list all teams".to_string(),
        })?;

        Ok(rows.into_iter().map(|r| r.team).collect())
    }

    #[instrument(skip(self), fields(membership_id = %id), name = "db_get_membership")]
    async fn get_membership(&self, id: &str) -> Result<Option<UserTeamMembership>> {
        let row = sqlx::query_as::<_, UserTeamMembershipRow>(
            "SELECT id, user_id, team, scopes, created_at FROM user_team_memberships WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch membership".to_string(),
        })?;

        row.map(|r| self.row_to_membership(r)).transpose()
    }

    #[instrument(skip(self), fields(user_id = %user_id, team = %team), name = "db_get_user_team_membership")]
    async fn get_user_team_membership(
        &self,
        user_id: &UserId,
        team: &str,
    ) -> Result<Option<UserTeamMembership>> {
        let row = sqlx::query_as::<_, UserTeamMembershipRow>(
            "SELECT id, user_id, team, scopes, created_at FROM user_team_memberships WHERE user_id = $1 AND team = $2",
        )
        .bind(user_id.to_string())
        .bind(team)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch user team membership".to_string(),
        })?;

        row.map(|r| self.row_to_membership(r)).transpose()
    }

    #[instrument(skip(self, scopes), fields(membership_id = %id), name = "db_update_membership_scopes")]
    async fn update_membership_scopes(
        &self,
        id: &str,
        scopes: Vec<String>,
    ) -> Result<UserTeamMembership> {
        let scopes_json = serde_json::to_string(&scopes).map_err(|err| {
            FlowplaneError::internal(format!("Failed to serialize scopes: {}", err))
        })?;

        sqlx::query("UPDATE user_team_memberships SET scopes = $1 WHERE id = $2")
            .bind(&scopes_json)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to update membership scopes".to_string(),
            })?;

        self.get_membership(id)
            .await?
            .ok_or_else(|| FlowplaneError::internal("Membership not found after update"))
    }

    #[instrument(skip(self), fields(membership_id = %id), name = "db_delete_membership")]
    async fn delete_membership(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM user_team_memberships WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to delete membership".to_string(),
            })?;

        Ok(())
    }

    #[instrument(skip(self), fields(user_id = %user_id, team = %team), name = "db_delete_user_team_membership")]
    async fn delete_user_team_membership(&self, user_id: &UserId, team: &str) -> Result<()> {
        sqlx::query("DELETE FROM user_team_memberships WHERE user_id = $1 AND team = $2")
            .bind(user_id.to_string())
            .bind(team)
            .execute(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to delete user team membership".to_string(),
            })?;

        Ok(())
    }
}
