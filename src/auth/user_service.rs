//! User management service for admin operations.
//!
//! This module provides business logic for user CRUD operations and team membership
//! management. All operations should be restricted to admin users only.

use std::sync::Arc;

use crate::auth::hashing;
use crate::auth::models::AuthContext;
use crate::auth::user::{
    NewUser, NewUserTeamMembership, UpdateUser, User, UserStatus, UserTeamMembership,
};
use crate::domain::UserId;
use crate::errors::{Error, Result};
use crate::storage::repositories::team::TeamRepository;
use crate::storage::repositories::user::{TeamMembershipRepository, UserRepository};
use crate::storage::repositories::{AuditEvent, AuditLogRepository};

/// Service for managing users and team memberships (admin-only operations).
#[derive(Clone)]
pub struct UserService {
    user_repository: Arc<dyn UserRepository>,
    membership_repository: Arc<dyn TeamMembershipRepository>,
    team_repository: Option<Arc<dyn TeamRepository>>,
    audit_repository: Arc<AuditLogRepository>,
}

impl UserService {
    /// Create a new UserService with the given repositories.
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        membership_repository: Arc<dyn TeamMembershipRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { user_repository, membership_repository, team_repository: None, audit_repository }
    }

    /// Create a new UserService with team validation enabled.
    pub fn with_team_validation(
        user_repository: Arc<dyn UserRepository>,
        membership_repository: Arc<dyn TeamMembershipRepository>,
        team_repository: Arc<dyn TeamRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self {
            user_repository,
            membership_repository,
            team_repository: Some(team_repository),
            audit_repository,
        }
    }

    /// Create a new user account.
    ///
    /// This will hash the password and create the user in the database.
    /// An audit log entry will be recorded.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_user(
        &self,
        email: String,
        password: String,
        name: String,
        is_admin: bool,
        org_id: Option<crate::domain::OrgId>,
        created_by: Option<String>,
        auth_context: Option<&AuthContext>,
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
        let org_id_for_audit = org_id.as_ref().map(|id| id.as_str().to_string());

        let new_user = NewUser {
            id: user_id.clone(),
            email: email.clone(),
            password_hash,
            name: name.clone(),
            status: UserStatus::Active,
            is_admin,
            org_id,
        };

        let user = self.user_repository.create_user(new_user).await?;

        // Log audit event
        let event = AuditEvent::token(
            "user.created",
            Some(user.id.as_str()),
            Some(&email),
            serde_json::json!({
                "name": name,
                "is_admin": is_admin,
                "org_id": org_id_for_audit,
                "created_by": created_by,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

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
        auth_context: Option<&AuthContext>,
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
        let event = AuditEvent::token(
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
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

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
        auth_context: Option<&AuthContext>,
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
        let event = AuditEvent::token(
            "user.password_changed",
            Some(id.as_str()),
            Some(&user.email),
            serde_json::json!({
                "changed_by": changed_by,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

        Ok(())
    }

    /// Change a user's password with current password verification.
    ///
    /// This method verifies the current password before allowing the change.
    /// This is the secure method that should be used for self-service password changes.
    pub async fn change_password_with_verification(
        &self,
        id: &UserId,
        current_password: String,
        new_password: String,
        auth_context: Option<&AuthContext>,
    ) -> Result<()> {
        use crate::errors::AuthErrorType;

        // Verify user exists and get password hash
        let user = self
            .user_repository
            .get_user(id)
            .await?
            .ok_or_else(|| Error::not_found("User", id.as_str()))?;

        // Get the user with password hash from the repository using email
        // (since get_user_with_password only takes email as parameter)
        let (_user, current_hash) = self
            .user_repository
            .get_user_with_password(&user.email)
            .await?
            .ok_or_else(|| Error::not_found("User", id.as_str()))?;

        // Verify current password
        let is_valid = hashing::verify_password(&current_password, &current_hash)?;
        if !is_valid {
            return Err(Error::auth(
                "Current password is incorrect",
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Hash new password
        let password_hash = hashing::hash_password(&new_password)?;

        // Update password
        self.user_repository.update_password(id, password_hash).await?;

        // Log audit event
        let event = AuditEvent::token(
            "user.password_changed",
            Some(id.as_str()),
            Some(&user.email),
            serde_json::json!({
                "changed_by": format!("user:{}", id.as_str()),
                "self_service": true,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

        Ok(())
    }

    /// Delete a user account.
    ///
    /// This will cascade delete all team memberships for the user.
    pub async fn delete_user(
        &self,
        id: &UserId,
        deleted_by: Option<String>,
        auth_context: Option<&AuthContext>,
    ) -> Result<()> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(id)
            .await?
            .ok_or_else(|| Error::not_found("User", id.as_str()))?;

        // Delete user (cascade deletes memberships)
        self.user_repository.delete_user(id).await?;

        // Log audit event
        let event = AuditEvent::token(
            "user.deleted",
            Some(id.as_str()),
            Some(&user.email),
            serde_json::json!({
                "name": user.name,
                "deleted_by": deleted_by,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

        Ok(())
    }

    /// Add a user to a team with specific scopes.
    pub async fn add_team_membership(
        &self,
        user_id: &UserId,
        team: String,
        scopes: Vec<String>,
        created_by: Option<String>,
        auth_context: Option<&AuthContext>,
    ) -> Result<UserTeamMembership> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(user_id)
            .await?
            .ok_or_else(|| Error::not_found("User", user_id.as_str()))?;

        // Resolve team name to UUID (FK migration: team column stores UUIDs)
        let team_id = if let Some(ref team_repo) = self.team_repository {
            let team_record = team_repo.get_team_by_name(&team).await?.ok_or_else(|| {
                Error::validation(format!(
                    "Team '{}' does not exist. Please create the team first.",
                    team
                ))
            })?;
            team_record.id.to_string()
        } else {
            // No team repo available, assume team is already a UUID
            team.clone()
        };

        // Check if membership already exists
        if let Some(_existing) =
            self.membership_repository.get_user_team_membership(user_id, &team_id).await?
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
            team: team_id.clone(),
            scopes: scopes.clone(),
        };

        let membership = self.membership_repository.create_membership(new_membership).await?;

        // Log audit event
        let event = AuditEvent::token(
            "user.team_membership_added",
            Some(user_id.as_str()),
            Some(&user.email),
            serde_json::json!({
                "team": team,
                "scopes": scopes,
                "created_by": created_by,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

        Ok(membership)
    }

    /// Remove a user from a team.
    pub async fn remove_team_membership(
        &self,
        user_id: &UserId,
        team: &str,
        removed_by: Option<String>,
        auth_context: Option<&AuthContext>,
    ) -> Result<()> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(user_id)
            .await?
            .ok_or_else(|| Error::not_found("User", user_id.as_str()))?;

        // Resolve team name to UUID for DB lookup
        let team_id = self.resolve_team_id(team).await?;

        // Verify membership exists
        let membership = self
            .membership_repository
            .get_user_team_membership(user_id, &team_id)
            .await?
            .ok_or_else(|| {
                Error::not_found("TeamMembership", format!("{}/{}", user_id.as_str(), team))
            })?;

        // Delete membership
        self.membership_repository.delete_membership(&membership.id).await?;

        // Log audit event
        let event = AuditEvent::token(
            "user.team_membership_removed",
            Some(user_id.as_str()),
            Some(&user.email),
            serde_json::json!({
                "team": team,
                "removed_by": removed_by,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

        Ok(())
    }

    /// Update scopes for an existing team membership.
    pub async fn update_team_membership_scopes(
        &self,
        user_id: &UserId,
        team: &str,
        scopes: Vec<String>,
        updated_by: Option<String>,
        auth_context: Option<&AuthContext>,
    ) -> Result<UserTeamMembership> {
        // Verify user exists
        let user = self
            .user_repository
            .get_user(user_id)
            .await?
            .ok_or_else(|| Error::not_found("User", user_id.as_str()))?;

        // Resolve team name to UUID for DB lookup
        let team_id = self.resolve_team_id(team).await?;

        // Verify membership exists
        let membership = self
            .membership_repository
            .get_user_team_membership(user_id, &team_id)
            .await?
            .ok_or_else(|| {
                Error::not_found("TeamMembership", format!("{}/{}", user_id.as_str(), team))
            })?;

        // Store old scopes for audit
        let old_scopes = membership.scopes.clone();

        // Update scopes
        let updated = self
            .membership_repository
            .update_membership_scopes(&membership.id, scopes.clone())
            .await?;

        // Log audit event
        let event = AuditEvent::token(
            "user.team_membership_scopes_updated",
            Some(user_id.as_str()),
            Some(&user.email),
            serde_json::json!({
                "team": team,
                "old_scopes": old_scopes,
                "new_scopes": scopes,
                "updated_by": updated_by,
            }),
        );
        let event = if let Some(ctx) = auth_context {
            let (user_id, client_ip, user_agent) = ctx.to_audit_context();
            event.with_user_context(user_id, client_ip, user_agent)
        } else {
            event
        };
        self.audit_repository.record_auth_event(event).await?;

        Ok(updated)
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
        let team_id = self.resolve_team_id(team).await?;
        self.membership_repository.list_team_members(&team_id).await
    }

    /// Resolve a team name to its UUID. Idempotent: UUIDs pass through.
    async fn resolve_team_id(&self, team: &str) -> Result<String> {
        // If already a UUID, pass through
        if uuid::Uuid::parse_str(team).is_ok() {
            return Ok(team.to_string());
        }
        if let Some(ref team_repo) = self.team_repository {
            let team_record = team_repo
                .get_team_by_name(team)
                .await?
                .ok_or_else(|| Error::not_found("Team", team))?;
            Ok(team_record.id.to_string())
        } else {
            Ok(team.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::repositories::user::{SqlxTeamMembershipRepository, SqlxUserRepository};
    use crate::storage::test_helpers::TestDatabase;
    use crate::storage::DbPool;

    async fn setup_test_service() -> (TestDatabase, UserService, DbPool) {
        let test_db = TestDatabase::new("user_service").await;
        let pool = test_db.pool.clone();

        let user_repo = Arc::new(SqlxUserRepository::new(pool.clone()));
        let membership_repo = Arc::new(SqlxTeamMembershipRepository::new(pool.clone()));
        let team_repo =
            Arc::new(crate::storage::repositories::team::SqlxTeamRepository::new(pool.clone()));
        let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));

        let service =
            UserService::with_team_validation(user_repo, membership_repo, team_repo, audit_repo);

        (test_db, service, pool)
    }

    /// Helper to create a test team in the database, returns team ID
    async fn create_test_team(pool: &DbPool, name: &str, display_name: &str) -> String {
        let team_id = format!("team-{}", uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, 'active') ON CONFLICT (name) DO NOTHING",
        )
        .bind(&team_id)
        .bind(name)
        .bind(display_name)
        .execute(pool)
        .await
        .expect("create test team");
        team_id
    }

    #[tokio::test]
    async fn test_create_user() {
        let (_db, service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "SecurePassword123".to_string(),
                "Test User".to_string(),
                false,
                None,
                Some("admin".to_string()),
                None,
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
        let (_db, service, _pool) = setup_test_service().await;

        // Create first user
        service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
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
                None,
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_get_user() {
        let (_db, service, _pool) = setup_test_service().await;

        let created = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
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
        let (_db, service, _pool) = setup_test_service().await;

        // Create multiple users
        for i in 1..=3 {
            service
                .create_user(
                    format!("user{}@example.com", i),
                    "Password123".to_string(),
                    format!("User {}", i),
                    false,
                    None,
                    None,
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
        let (_db, service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
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
            .update_user(&user.id, update, Some("admin".to_string()), None)
            .await
            .expect("update user");

        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.status, UserStatus::Inactive);
        assert_eq!(updated.email, user.email); // Unchanged
    }

    #[tokio::test]
    async fn test_delete_user() {
        let (_db, service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
                None,
            )
            .await
            .expect("create user");

        service.delete_user(&user.id, Some("admin".to_string()), None).await.expect("delete user");

        let fetched = service.get_user(&user.id).await.expect("get user");
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_add_team_membership() {
        let (_db, service, pool) = setup_test_service().await;

        // Create the team first
        let team_id = create_test_team(&pool, "team-alpha", "Team Alpha").await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
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
                None,
            )
            .await
            .expect("add team membership");

        assert_eq!(membership.user_id, user.id);
        assert_eq!(membership.team, team_id);
        assert_eq!(membership.scopes.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_team_membership() {
        let (_db, service, pool) = setup_test_service().await;

        // Create the team first
        create_test_team(&pool, "team-alpha", "Team Alpha").await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
                None,
            )
            .await
            .expect("create user");

        service
            .add_team_membership(
                &user.id,
                "team-alpha".to_string(),
                vec!["read".to_string()],
                None,
                None,
            )
            .await
            .expect("add team membership");

        service
            .remove_team_membership(&user.id, "team-alpha", Some("admin".to_string()), None)
            .await
            .expect("remove team membership");

        let teams = service.list_user_teams(&user.id).await.expect("list user teams");
        assert_eq!(teams.len(), 0);
    }

    #[tokio::test]
    async fn test_list_user_teams() {
        let (_db, service, pool) = setup_test_service().await;

        // Create teams first
        for (name, display) in
            [("team-alpha", "Team Alpha"), ("team-beta", "Team Beta"), ("team-gamma", "Team Gamma")]
        {
            create_test_team(&pool, name, display).await;
        }

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
                None,
            )
            .await
            .expect("create user");

        // Add multiple team memberships
        for team in ["team-alpha", "team-beta", "team-gamma"] {
            service
                .add_team_membership(
                    &user.id,
                    team.to_string(),
                    vec!["read".to_string()],
                    None,
                    None,
                )
                .await
                .expect("add team membership");
        }

        let teams = service.list_user_teams(&user.id).await.expect("list user teams");
        assert_eq!(teams.len(), 3);
    }

    #[tokio::test]
    async fn test_add_team_membership_validates_team_exists() {
        let (_db, service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
                None,
            )
            .await
            .expect("create user");

        // Try to add membership to non-existent team
        let result = service
            .add_team_membership(
                &user.id,
                "nonexistent-team".to_string(),
                vec!["read".to_string()],
                None,
                None,
            )
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("does not exist"),
            "Error message should mention team doesn't exist: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_update_team_membership_scopes() {
        let (_db, service, pool) = setup_test_service().await;

        // Create the team first
        create_test_team(&pool, "team-alpha", "Team Alpha").await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
                None,
            )
            .await
            .expect("create user");

        // Add initial membership with some scopes
        service
            .add_team_membership(
                &user.id,
                "team-alpha".to_string(),
                vec!["team:team-alpha:clusters:read".to_string()],
                None,
                None,
            )
            .await
            .expect("add team membership");

        // Update scopes
        let updated = service
            .update_team_membership_scopes(
                &user.id,
                "team-alpha",
                vec![
                    "team:team-alpha:clusters:read".to_string(),
                    "team:team-alpha:routes:write".to_string(),
                ],
                None,
                None,
            )
            .await
            .expect("update scopes");

        assert_eq!(updated.scopes.len(), 2);
        assert!(updated.scopes.contains(&"team:team-alpha:clusters:read".to_string()));
        assert!(updated.scopes.contains(&"team:team-alpha:routes:write".to_string()));
    }

    #[tokio::test]
    async fn test_update_team_membership_scopes_user_not_found() {
        let (_db, service, _pool) = setup_test_service().await;

        let fake_user_id = UserId::new();
        let result = service
            .update_team_membership_scopes(
                &fake_user_id,
                "nonexistent-team",
                vec!["clusters:read".to_string()],
                None,
                None,
            )
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found") || err_msg.contains("User"),
            "Error should indicate user not found: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_update_team_membership_scopes_membership_not_found() {
        let (_db, service, _pool) = setup_test_service().await;

        let user = service
            .create_user(
                "test@example.com".to_string(),
                "Password123".to_string(),
                "Test User".to_string(),
                false,
                None,
                None,
                None,
            )
            .await
            .expect("create user");

        // Try to update scopes for a team the user is not a member of
        let result = service
            .update_team_membership_scopes(
                &user.id,
                "nonexistent-team",
                vec!["clusters:read".to_string()],
                None,
                None,
            )
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found") || err_msg.contains("TeamMembership"),
            "Error should indicate membership not found: {}",
            err_msg
        );
    }
}
