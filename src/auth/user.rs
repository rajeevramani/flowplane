//! User and team membership domain types.
//!
//! With Zitadel as the sole auth provider, users are managed externally.
//! These types are retained for the repository layer (used by org handlers)
//! until the database migration (Task 2.5) drops the users table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

use crate::domain::UserId;

/// User status values stored in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Inactive,
    Suspended,
}

impl fmt::Display for UserStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Inactive => write!(f, "inactive"),
            Self::Suspended => write!(f, "suspended"),
        }
    }
}

impl FromStr for UserStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            "suspended" => Ok(Self::Suspended),
            other => Err(format!("unknown user status: {other}")),
        }
    }
}

/// User record from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub name: String,
    pub status: UserStatus,
    pub is_admin: bool,
    /// Zitadel subject identifier (`sub` claim from JWT).
    /// Bridges Zitadel identity to Flowplane permissions.
    pub zitadel_sub: Option<String>,
    /// User type: "human" (default) or "machine" (API agent).
    pub user_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// New user creation request.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub id: UserId,
    pub email: String,
    pub password_hash: String,
    pub name: String,
    pub status: UserStatus,
    pub is_admin: bool,
}

/// User update request.
#[derive(Debug, Clone, Default)]
pub struct UpdateUser {
    pub email: Option<String>,
    pub name: Option<String>,
    pub status: Option<UserStatus>,
    pub is_admin: Option<bool>,
}

/// Team membership record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTeamMembership {
    pub id: String,
    pub user_id: UserId,
    pub team: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// New team membership creation request.
#[derive(Debug, Clone)]
pub struct NewUserTeamMembership {
    pub id: String,
    pub user_id: UserId,
    pub team: String,
    pub scopes: Vec<String>,
}
