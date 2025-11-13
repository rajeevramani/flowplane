//! User management service for admin operations.
//!
//! This module provides business logic for user CRUD operations and team membership
//! management. All operations should be restricted to admin users only.

use std::sync::Arc;

use crate::auth::hashing;
use crate::auth::user::{
    NewUser, NewUserTeamMembership, UpdateUser, User, UserStatus, UserTeamMembership,
};
use crate::domain::UserId;
use crate::errors::{Error, Result};
use crate::storage::repositories::user::{TeamMembershipRepository, UserRepository};
use crate::storage::repositories::{AuditEvent, AuditLogRepository};

/// Service for managing users and team memberships (admin-only operations).
#[derive(Clone)]
pub struct UserService {
    user_repository: Arc<dyn UserRepository>,
    membership_repository: Arc<dyn TeamMembershipRepository>,
    audit_repository: Arc<AuditLogRepository>,
}

impl UserService {
    /// Create a new UserService with the given repositories.
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        membership_repository: Arc<dyn TeamMembershipRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { user_repository, membership_repository, audit_repository }
    }

    /// Create a new user account.
    ///
    /// This will hash the password and create the user in the database.
    /// An audit log entry will be recorded.
    pub async fn create_user(
        &self,
        email: String,
        password: String,
        name: String,
        is_admin: bool,
        created_by: Option<String>,
    ) -> Result<User> {
        // Normalize email
        let email = User::normalize_email(&email);

        // Check if email already exists
        if let Some(_existing) = self.user_repository.get_user_by_email(&email).await? {
            return Err(Error::validation(format!("User with email '{}' already exists", email)));
        }

        // Hash password
        let password_hash = hashing::hash_password(&password)?;

        // Create user
        let user_id = UserId::new();
        let new_user = NewUser {
            id: user_id.clone(),
            email: email.clone(),
            password_hash,
            name: name.clone(),
            status: UserStatus::Active,
            is_admin,
        };

        let user = self.user_repository.create_user(new_user).await?;

        // Log audit event
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "user.created",
                Some(user.id.as_str()),
                Some(&email),
                serde_json::json!({
                    "name": name,
                    "is_admin": is_admin,
                    "created_by": created_by,
                }),
            ))
            .await?;

        Ok(user)
    }

    /// Get a user by ID.
    pub async fn get_user(&self, id: &UserId) -> Result<Option<User>> {
        self.user_repository.get_user(id).await
    }

    /// Get a user by email.
    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let normalized_email = User::normalize_email(email);
        self.user_repository.get_user_by_email(&normalized_email).await
    }

    /// List all users with pagination.
    pub async fn list_users(&self, limit: i64, offset: i64) -> Result<Vec<User>> {
        self.user_repository.list_users(limit, offset).await
    }

    /// Count total users.
    pub async fn count_users(&self) -> Result<i64> {
        self.user_repository.count_users().await
    }

    /// Update a user's details.
    ///
    /// Only fields present in the update payload will be modified.
    pub async fn update_user(
        &self,
        id: &UserId,
        update: UpdateUser,
        updated_by: Option<String>,
    ) -> Result<User> {
        // Verify user exists
        let existing = self
            .user_repository
            .get_user(id)
            .await?
            .ok_or_else(|| Error::not_found("User", id.as_str()))?;

        // If email is being updated, check it's not already taken
        if let Some(ref new_email) = update.email {
            let normalized = User::normalize_email(new_email);
            if normalized != existing.email {
                if let Some(_other) = self.user_repository.get_user_by_email(&normalized).await? {
                    return Err(Error::validation(format!(
                        "Email '{}' is already in use",
                        new_email
                    )));
                }
            }
        }

        // Perform update
        let updated_user = self.user_repository.update_user(id, update.clone()).await?;

        // Log audit event
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "user.updated",
                Some(id.as_str()),
                Some(&updated_user.email),
                serde_json::json!({
                    "changes": {
                        "email": update.email,
                        "name": update.name,
                        "status": update.status.map(|s| s.to_string()),
                        "is_admin": update.is_admin,
                    },
                    "updated_by": updated_by,
                }),
            ))
            .await?;

        Ok(updated_user)
    }

    /// Change a user's password.
    ///
    /// This hashes the new password before storing it.
    pub async fn change_password(
        &self,
        id: &UserId,
        new_password: String,
        changed_by: Option<String>,
    ) -> Result<()> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(id)
            .await?
            .ok_or_else(|| Error::not_found("User", id.as_str()))?;

        // Hash new password
        let password_hash = hashing::hash_password(&new_password)?;

        // Update password
        self.user_repository.update_password(id, password_hash).await?;

        // Log audit event
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "user.password_changed",
                Some(id.as_str()),
                Some(&user.email),
                serde_json::json!({
                    "changed_by": changed_by,
                }),
            ))
            .await?;

        Ok(())
    }

    /// Delete a user account.
    ///
    /// This will cascade delete all team memberships for the user.
    pub async fn delete_user(&self, id: &UserId, deleted_by: Option<String>) -> Result<()> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(id)
            .await?
            .ok_or_else(|| Error::not_found("User", id.as_str()))?;

        // Delete user (cascade deletes memberships)
        self.user_repository.delete_user(id).await?;

        // Log audit event
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "user.deleted",
                Some(id.as_str()),
                Some(&user.email),
                serde_json::json!({
                    "name": user.name,
                    "deleted_by": deleted_by,
                }),
            ))
            .await?;

        Ok(())
    }

    /// Add a user to a team with specific scopes.
    pub async fn add_team_membership(
        &self,
        user_id: &UserId,
        team: String,
        scopes: Vec<String>,
        created_by: Option<String>,
    ) -> Result<UserTeamMembership> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(user_id)
            .await?
            .ok_or_else(|| Error::not_found("User", user_id.as_str()))?;

        // Check if membership already exists
        if let Some(_existing) =
            self.membership_repository.get_user_team_membership(user_id, &team).await?
        {
            return Err(Error::validation(format!(
                "User '{}' is already a member of team '{}'",
                user.email, team
            )));
        }

        // Create membership
        let membership_id = format!("utm_{}", uuid::Uuid::new_v4());
        let new_membership = NewUserTeamMembership {
            id: membership_id,
            user_id: user_id.clone(),
            team: team.clone(),
            scopes: scopes.clone(),
        };

        let membership = self.membership_repository.create_membership(new_membership).await?;

        // Log audit event
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "user.team_membership_added",
                Some(user_id.as_str()),
                Some(&user.email),
                serde_json::json!({
                    "team": team,
                    "scopes": scopes,
                    "created_by": created_by,
                }),
            ))
            .await?;

        Ok(membership)
    }

    /// Remove a user from a team.
    pub async fn remove_team_membership(
        &self,
        user_id: &UserId,
        team: &str,
        removed_by: Option<String>,
    ) -> Result<()> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(user_id)
            .await?
            .ok_or_else(|| Error::not_found("User", user_id.as_str()))?;

        // Verify membership exists
        let membership =
            self.membership_repository.get_user_team_membership(user_id, team).await?.ok_or_else(
                || Error::not_found("TeamMembership", format!("{}/{}", user_id.as_str(), team)),
            )?;

        // Delete membership
        self.membership_repository.delete_membership(&membership.id).await?;

        // Log audit event
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "user.team_membership_removed",
                Some(user_id.as_str()),
                Some(&user.email),
                serde_json::json!({
                    "team": team,
                    "removed_by": removed_by,
                }),
            ))
            .await?;

        Ok(())
    }

    /// List all team memberships for a user.
    pub async fn list_user_teams(&self, user_id: &UserId) -> Result<Vec<UserTeamMembership>> {
        // Verify user exists
        self.user_repository
            .get_user(user_id)
            .await?
            .ok_or_else(|| Error::not_found("User", user_id.as_str()))?;

        self.membership_repository.list_user_memberships(user_id).await
    }

    /// List all users in a team.
    pub async fn list_team_users(&self, team: &str) -> Result<Vec<UserTeamMembership>> {
        self.membership_repository.list_team_members(team).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::repositories::user::{SqlxTeamMembershipRepository, SqlxUserRepository};
    use crate::storage::DbPool;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_test_service() -> (UserService, DbPool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .expect("create sqlite pool");

        // Run migrations
        crate::storage::run_migrations(&pool).await.expect("run migrations");

        let user_repo = Arc::new(SqlxUserRepository::new(pool.clone()));
        let membership_repo = Arc::new(SqlxTeamMembershipRepository::new(pool.clone()));
        let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));

        let service = UserService::new(user_repo, membership_repo, audit_repo);

        (service, pool)
    }

    #[tokio::test]
    async fn test_create_user() {
        let (service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "SecurePassword123".to_string(),
                "Test User".to_string(),
                false,
                Some("admin".to_string()),
            )
            .await
            .expect("create user");

        assert_eq!(user.email, "test@example.com");
        assert_eq!(user.name, "Test User");
        assert!(!user.is_admin);
        assert_eq!(user.status, UserStatus::Active);
    }

    #[tokio::test]
    async fn test_create_duplicate_user_fails() {
        let (service, _pool) = setup_test_service().await;

        // Create first user
        service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create first user");

        // Try to create duplicate
        let result = service
            .create_user(
                "test@example.com".to_string(),
                "Password456".to_string(),
                "Another User".to_string(),
                false,
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_get_user() {
        let (service, _pool) = setup_test_service().await;

        let created = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create user");

        let fetched = service.get_user(&created.id).await.expect("get user");

        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.email, created.email);
    }

    #[tokio::test]
    async fn test_list_users() {
        let (service, _pool) = setup_test_service().await;

        // Create multiple users
        for i in 1..=3 {
            service
                .create_user(
                    format!("user{}@example.com", i),
                    "Password123".to_string(),
                    format!("User {}", i),
                    false,
                    None,
                )
                .await
                .expect("create user");
        }

        let users = service.list_users(10, 0).await.expect("list users");
        assert_eq!(users.len(), 3);
    }

    #[tokio::test]
    async fn test_update_user() {
        let (service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create user");

        let update = UpdateUser {
            name: Some("Updated Name".to_string()),
            email: None,
            status: Some(UserStatus::Inactive),
            is_admin: None,
        };

        let updated = service
            .update_user(&user.id, update, Some("admin".to_string()))
            .await
            .expect("update user");

        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.status, UserStatus::Inactive);
        assert_eq!(updated.email, user.email); // Unchanged
    }

    #[tokio::test]
    async fn test_delete_user() {
        let (service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create user");

        service.delete_user(&user.id, Some("admin".to_string())).await.expect("delete user");

        let fetched = service.get_user(&user.id).await.expect("get user");
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_add_team_membership() {
        let (service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create user");

        let membership = service
            .add_team_membership(
                &user.id,
                "team-alpha".to_string(),
                vec!["read".to_string(), "write".to_string()],
                Some("admin".to_string()),
            )
            .await
            .expect("add team membership");

        assert_eq!(membership.user_id, user.id);
        assert_eq!(membership.team, "team-alpha");
        assert_eq!(membership.scopes.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_team_membership() {
        let (service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create user");

        service
            .add_team_membership(&user.id, "team-alpha".to_string(), vec!["read".to_string()], None)
            .await
            .expect("add team membership");

        service
            .remove_team_membership(&user.id, "team-alpha", Some("admin".to_string()))
            .await
            .expect("remove team membership");

        let teams = service.list_user_teams(&user.id).await.expect("list user teams");
        assert_eq!(teams.len(), 0);
    }

    #[tokio::test]
    async fn test_list_user_teams() {
        let (service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
            )
            .await
            .expect("create user");

        // Add multiple team memberships
        for team in ["team-alpha", "team-beta", "team-gamma"] {
            service
                .add_team_membership(&user.id, team.to_string(), vec!["read".to_string()], None)
                .await
                .expect("add team membership");
        }

        let teams = service.list_user_teams(&user.id).await.expect("list user teams");
        assert_eq!(teams.len(), 3);
    }
}
