//! User domain models and data structures.
//!
//! This module defines the core user entities, including user accounts,
//! team memberships, and their associated request/response DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use utoipa::ToSchema;

use crate::domain::UserId;

/// User account status lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UserStatus {
    Active,
    Inactive,
    Suspended,
}

impl UserStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserStatus::Active => "active",
            UserStatus::Inactive => "inactive",
            UserStatus::Suspended => "suspended",
        }
    }
}

impl Display for UserStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UserStatus {
    type Err = UserStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(UserStatus::Active),
            "inactive" => Ok(UserStatus::Inactive),
            "suspended" => Ok(UserStatus::Suspended),
            other => Err(UserStatusParseError(other.to_string())),
        }
    }
}

/// Error returned when user status parsing fails.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid user status: {0}")]
pub struct UserStatusParseError(pub String);

/// Stored representation of a user account.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub name: String,
    pub status: UserStatus,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    /// Check if the user is active and can perform operations.
    pub fn is_active(&self) -> bool {
        matches!(self.status, UserStatus::Active)
    }

    /// Normalize email to lowercase for consistent storage and comparison.
    pub fn normalize_email(email: &str) -> String {
        email.trim().to_lowercase()
    }
}

/// New user creation payload (does not include password_hash - that's added separately).
#[derive(Debug, Clone)]
pub struct NewUser {
    pub id: UserId,
    pub email: String,
    pub password_hash: String,
    pub name: String,
    pub status: UserStatus,
    pub is_admin: bool,
}

/// Update payload for an existing user.
#[derive(Debug, Clone, Default)]
pub struct UpdateUser {
    pub email: Option<String>,
    pub name: Option<String>,
    pub status: Option<UserStatus>,
    pub is_admin: Option<bool>,
}

/// User team membership representing access to a specific team.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserTeamMembership {
    pub id: String,
    pub user_id: UserId,
    pub team: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl UserTeamMembership {
    /// Check if this membership grants a specific scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Check if this membership grants any of the specified scopes.
    pub fn has_any_scope(&self, scopes: &[&str]) -> bool {
        scopes.iter().any(|scope| self.has_scope(scope))
    }

    /// Check if this membership grants all of the specified scopes.
    pub fn has_all_scopes(&self, scopes: &[&str]) -> bool {
        scopes.iter().all(|scope| self.has_scope(scope))
    }
}

/// New team membership creation payload.
#[derive(Debug, Clone)]
pub struct NewUserTeamMembership {
    pub id: String,
    pub user_id: UserId,
    pub team: String,
    pub scopes: Vec<String>,
}

/// Request to create a new user account.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
    pub name: String,
    #[serde(default)]
    pub is_admin: bool,
}

/// Request to update an existing user account.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub name: Option<String>,
    pub status: Option<UserStatus>,
    pub is_admin: Option<bool>,
}

/// Request to change a user's password.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Request to add a user to a team with specific scopes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamMembershipRequest {
    pub user_id: UserId,
    pub team: String,
    pub scopes: Vec<String>,
}

/// Request to update team membership scopes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamMembershipRequest {
    pub scopes: Vec<String>,
}

/// User authentication credentials.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Response after successful user creation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub id: UserId,
    pub email: String,
    pub name: String,
    pub status: UserStatus,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            name: user.name,
            status: user.status,
            is_admin: user.is_admin,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

/// Response containing user with their team memberships.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserWithTeamsResponse {
    #[serde(flatten)]
    pub user: UserResponse,
    pub teams: Vec<UserTeamMembership>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_status_round_trip() {
        for (input, expected) in [
            ("active", UserStatus::Active),
            ("inactive", UserStatus::Inactive),
            ("suspended", UserStatus::Suspended),
        ] {
            let parsed = input.parse::<UserStatus>().unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), input);
        }

        let err = "invalid".parse::<UserStatus>().unwrap_err();
        assert_eq!(err.0, "invalid");
    }

    #[test]
    fn user_is_active() {
        let active_user = User {
            id: UserId::new(),
            email: "test@example.com".to_string(),
            name: "Test User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let inactive_user = User { status: UserStatus::Inactive, ..active_user.clone() };

        assert!(active_user.is_active());
        assert!(!inactive_user.is_active());
    }

    #[test]
    fn email_normalization() {
        assert_eq!(User::normalize_email("Test@Example.COM"), "test@example.com");
        assert_eq!(User::normalize_email("  user@HOST.com  "), "user@host.com");
    }

    #[test]
    fn team_membership_has_scope() {
        let membership = UserTeamMembership {
            id: "membership-1".to_string(),
            user_id: UserId::new(),
            team: "team-a".to_string(),
            scopes: vec!["read".to_string(), "write".to_string(), "admin".to_string()],
            created_at: Utc::now(),
        };

        assert!(membership.has_scope("read"));
        assert!(membership.has_scope("write"));
        assert!(!membership.has_scope("delete"));
    }

    #[test]
    fn team_membership_has_any_scope() {
        let membership = UserTeamMembership {
            id: "membership-1".to_string(),
            user_id: UserId::new(),
            team: "team-a".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
            created_at: Utc::now(),
        };

        assert!(membership.has_any_scope(&["read", "admin"]));
        assert!(membership.has_any_scope(&["write"]));
        assert!(!membership.has_any_scope(&["admin", "delete"]));
    }

    #[test]
    fn team_membership_has_all_scopes() {
        let membership = UserTeamMembership {
            id: "membership-1".to_string(),
            user_id: UserId::new(),
            team: "team-a".to_string(),
            scopes: vec!["read".to_string(), "write".to_string(), "admin".to_string()],
            created_at: Utc::now(),
        };

        assert!(membership.has_all_scopes(&["read", "write"]));
        assert!(membership.has_all_scopes(&["read"]));
        assert!(!membership.has_all_scopes(&["read", "write", "delete"]));
    }

    #[test]
    fn user_response_conversion() {
        let user = User {
            id: UserId::new(),
            email: "test@example.com".to_string(),
            name: "Test User".to_string(),
            status: UserStatus::Active,
            is_admin: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let response: UserResponse = user.clone().into();

        assert_eq!(response.id, user.id);
        assert_eq!(response.email, user.email);
        assert_eq!(response.name, user.name);
        assert_eq!(response.status, user.status);
        assert_eq!(response.is_admin, user.is_admin);
    }

    #[test]
    fn create_user_request_deserialization() {
        let json = r#"{
            "email": "test@example.com",
            "password": "SecureP@ssw0rd",
            "name": "Test User",
            "isAdmin": true
        }"#;

        let request: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.email, "test@example.com");
        assert_eq!(request.password, "SecureP@ssw0rd");
        assert_eq!(request.name, "Test User");
        assert!(request.is_admin);
    }

    #[test]
    fn create_user_request_defaults_is_admin() {
        let json = r#"{
            "email": "test@example.com",
            "password": "SecureP@ssw0rd",
            "name": "Test User"
        }"#;

        let request: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert!(!request.is_admin); // Should default to false
    }

    #[test]
    fn update_user_request_partial() {
        let json = r#"{
            "name": "Updated Name"
        }"#;

        let request: UpdateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, Some("Updated Name".to_string()));
        assert!(request.email.is_none());
        assert!(request.status.is_none());
        assert!(request.is_admin.is_none());
    }

    #[test]
    fn team_membership_request_serialization() {
        let request = CreateTeamMembershipRequest {
            user_id: UserId::new(),
            team: "team-a".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("userId"));
        assert!(json.contains("team"));
        assert!(json.contains("scopes"));
    }
}
