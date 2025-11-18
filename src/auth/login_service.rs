//! Login service for email/password authentication.

use std::sync::Arc;

use tracing::{info, instrument, warn};

use crate::auth::{hashing, user::UserStatus, LoginRequest, User, UserTeamMembership};
use crate::errors::{AuthErrorType, Error, Result};
use crate::observability::metrics;
use crate::storage::repositories::{
    AuditEvent, AuditLogRepository, SqlxTeamMembershipRepository, SqlxUserRepository,
    TeamMembershipRepository, UserRepository,
};

/// Service for handling email/password authentication.
#[derive(Clone)]
pub struct LoginService {
    user_repository: Arc<dyn UserRepository>,
    membership_repository: Arc<dyn TeamMembershipRepository>,
    audit_repository: Arc<AuditLogRepository>,
}

impl LoginService {
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        membership_repository: Arc<dyn TeamMembershipRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { user_repository, membership_repository, audit_repository }
    }

    pub fn with_sqlx(pool: crate::storage::DbPool) -> Self {
        let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
        Self::new(
            Arc::new(SqlxUserRepository::new(pool.clone())),
            Arc::new(SqlxTeamMembershipRepository::new(pool.clone())),
            audit_repository,
        )
    }

    /// Authenticate user with email and password, returning user and computed scopes.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - User not found
    /// - Password is incorrect
    /// - User account is not active
    /// - User account is suspended
    #[instrument(skip(self, request), fields(email = %request.email))]
    pub async fn login(&self, request: &LoginRequest) -> Result<(User, Vec<String>)> {
        // Normalize email
        let email = User::normalize_email(&request.email);

        // Fetch user with password hash
        let user_result = self.user_repository.get_user_with_password(&email).await?;
        if user_result.is_none() {
            warn!(email = %email, "login attempt for non-existent user");
            metrics::record_authentication("invalid_credentials").await;
            return Err(Error::auth(
                "Invalid email or password",
                AuthErrorType::InvalidCredentials,
            ));
        }
        let (user, password_hash) = user_result.unwrap();

        // Verify password
        let password_matches = hashing::verify_password(&request.password, &password_hash)?;
        if !password_matches {
            warn!(user_id = %user.id, email = %email, "login attempt with incorrect password");
            metrics::record_authentication("invalid_credentials").await;

            // Audit failed login
            self.audit_repository
                .record_auth_event(AuditEvent::token(
                    "auth.login.failed",
                    Some(user.id.as_str()),
                    Some(&user.email),
                    serde_json::json!({ "reason": "invalid_password" }),
                ))
                .await?;

            return Err(Error::auth(
                "Invalid email or password",
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Check user status
        if user.status != UserStatus::Active {
            warn!(user_id = %user.id, status = ?user.status, "login attempt for non-active user");
            metrics::record_authentication("account_not_active").await;

            let status_str = match user.status {
                UserStatus::Inactive => "inactive",
                UserStatus::Suspended => "suspended",
                UserStatus::Active => unreachable!(),
            };

            // Audit failed login
            self.audit_repository
                .record_auth_event(AuditEvent::token(
                    "auth.login.failed",
                    Some(user.id.as_str()),
                    Some(&user.email),
                    serde_json::json!({ "reason": "account_not_active", "status": status_str }),
                ))
                .await?;

            return Err(Error::auth(
                format!("Account is {}", status_str),
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Fetch team memberships
        let memberships = self.membership_repository.list_user_memberships(&user.id).await?;

        // Compute scopes from team memberships
        let scopes = compute_scopes_from_memberships(&user, &memberships);

        // Audit successful login
        self.audit_repository
            .record_auth_event(AuditEvent::token(
                "auth.login.success",
                Some(user.id.as_str()),
                Some(&user.email),
                serde_json::json!({
                    "teams": memberships.iter().map(|m| &m.team).collect::<Vec<_>>(),
                    "scope_count": scopes.len(),
                }),
            ))
            .await?;

        metrics::record_authentication("success").await;
        info!(user_id = %user.id, email = %user.email, "user logged in successfully");

        Ok((user, scopes))
    }
}

/// Compute scopes from user's team memberships.
///
/// If user is an admin, grants `admin:all` scope.
/// Otherwise, returns all scopes from all team memberships.
pub fn compute_scopes_from_memberships(
    user: &User,
    memberships: &[UserTeamMembership],
) -> Vec<String> {
    if user.is_admin {
        return vec!["admin:all".to_string()];
    }

    // Collect all unique scopes from all team memberships
    let mut scopes: Vec<String> =
        memberships.iter().flat_map(|m| m.scopes.iter()).map(|s| s.to_string()).collect();

    // Remove duplicates
    scopes.sort();
    scopes.dedup();

    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::UserId;
    use chrono::Utc;

    #[test]
    fn compute_scopes_admin_user() {
        let user = User {
            id: UserId::new(),
            email: "admin@example.com".to_string(),
            name: "Admin User".to_string(),
            status: UserStatus::Active,
            is_admin: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![UserTeamMembership {
            id: "m1".to_string(),
            user_id: user.id.clone(),
            team: "team-a".to_string(),
            scopes: vec!["clusters:read".to_string()],
            created_at: Utc::now(),
        }];

        let scopes = compute_scopes_from_memberships(&user, &memberships);

        assert_eq!(scopes, vec!["admin:all"]);
    }

    #[test]
    fn compute_scopes_regular_user_single_team() {
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Regular User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![UserTeamMembership {
            id: "m1".to_string(),
            user_id: user.id.clone(),
            team: "team-a".to_string(),
            scopes: vec!["clusters:read".to_string(), "clusters:write".to_string()],
            created_at: Utc::now(),
        }];

        let scopes = compute_scopes_from_memberships(&user, &memberships);

        assert_eq!(scopes, vec!["clusters:read", "clusters:write"]);
    }

    #[test]
    fn compute_scopes_regular_user_multiple_teams() {
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Regular User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![
            UserTeamMembership {
                id: "m1".to_string(),
                user_id: user.id.clone(),
                team: "team-a".to_string(),
                scopes: vec!["clusters:read".to_string(), "routes:write".to_string()],
                created_at: Utc::now(),
            },
            UserTeamMembership {
                id: "m2".to_string(),
                user_id: user.id.clone(),
                team: "team-b".to_string(),
                scopes: vec!["listeners:read".to_string(), "clusters:read".to_string()],
                created_at: Utc::now(),
            },
        ];

        let scopes = compute_scopes_from_memberships(&user, &memberships);

        // Should deduplicate clusters:read
        assert_eq!(scopes, vec!["clusters:read", "listeners:read", "routes:write"]);
    }

    #[test]
    fn compute_scopes_user_no_teams() {
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Regular User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![];

        let scopes = compute_scopes_from_memberships(&user, &memberships);

        assert_eq!(scopes, Vec::<String>::new());
    }
}
