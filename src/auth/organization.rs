//! Organization domain models and types.
//!
//! Organizations provide a governance layer above teams. Each organization
//! contains teams and users, with role-based access control at the org level.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use utoipa::ToSchema;
use validator::Validate;

use crate::domain::{OrgId, UserId};

/// Status of an organization in the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OrgStatus {
    /// Organization is active and operational
    Active,
    /// Organization is suspended (resources preserved but cannot be modified)
    Suspended,
    /// Organization is archived (read-only)
    Archived,
}

impl OrgStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrgStatus::Active => "active",
            OrgStatus::Suspended => "suspended",
            OrgStatus::Archived => "archived",
        }
    }
}

impl Display for OrgStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for OrgStatus {
    type Err = OrgStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(OrgStatus::Active),
            "suspended" => Ok(OrgStatus::Suspended),
            "archived" => Ok(OrgStatus::Archived),
            other => Err(OrgStatusParseError(other.to_string())),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid organization status: {0}")]
pub struct OrgStatusParseError(pub String);

/// Role of a user within an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OrgRole {
    /// Full control over the organization
    Owner,
    /// Can manage teams, users, and settings within the org
    Admin,
    /// Standard org member with team-scoped access
    Member,
    /// Read-only access to org resources
    Viewer,
}

impl OrgRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrgRole::Owner => "owner",
            OrgRole::Admin => "admin",
            OrgRole::Member => "member",
            OrgRole::Viewer => "viewer",
        }
    }

    /// Whether this role grants org admin privileges.
    pub fn is_admin(&self) -> bool {
        matches!(self, OrgRole::Owner | OrgRole::Admin)
    }
}

impl Display for OrgRole {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for OrgRole {
    type Err = OrgRoleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(OrgRole::Owner),
            "admin" => Ok(OrgRole::Admin),
            "member" => Ok(OrgRole::Member),
            "viewer" => Ok(OrgRole::Viewer),
            other => Err(OrgRoleParseError(other.to_string())),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid organization role: {0}")]
pub struct OrgRoleParseError(pub String);

/// Represents an organization in the system.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: OrgId,
    /// Unique, immutable org name (lowercase, alphanumeric with hyphens)
    pub name: String,
    /// Human-friendly display name
    pub display_name: String,
    pub description: Option<String>,
    /// Optional owner user ID
    pub owner_user_id: Option<UserId>,
    /// Org-specific settings (JSON)
    pub settings: Option<serde_json::Value>,
    pub status: OrgStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Organization {
    pub fn is_active(&self) -> bool {
        matches!(self.status, OrgStatus::Active)
    }
}

/// A user's membership in an organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationMembership {
    pub id: String,
    pub user_id: UserId,
    pub org_id: OrgId,
    /// Organization name (populated via JOIN for convenience)
    pub org_name: String,
    pub role: OrgRole,
    pub created_at: DateTime<Utc>,
}

/// Request to create a new organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrganizationRequest {
    #[validate(length(min = 1, max = 255), regex(path = "crate::utils::TEAM_NAME_REGEX"))]
    pub name: String,
    #[validate(length(min = 1, max = 255))]
    pub display_name: String,
    #[validate(length(max = 1000))]
    pub description: Option<String>,
    pub owner_user_id: Option<UserId>,
    pub settings: Option<serde_json::Value>,
}

/// Request to update an existing organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOrganizationRequest {
    #[validate(length(min = 1, max = 255))]
    pub display_name: Option<String>,
    #[validate(length(max = 1000))]
    pub description: Option<String>,
    pub owner_user_id: Option<UserId>,
    pub settings: Option<serde_json::Value>,
    pub status: Option<OrgStatus>,
}

/// Response for organization data.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationResponse {
    pub id: OrgId,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub owner_user_id: Option<UserId>,
    pub status: OrgStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Organization> for OrganizationResponse {
    fn from(org: Organization) -> Self {
        Self {
            id: org.id,
            name: org.name,
            display_name: org.display_name,
            description: org.description,
            owner_user_id: org.owner_user_id,
            status: org.status,
            created_at: org.created_at,
            updated_at: org.updated_at,
        }
    }
}

/// Response for organization membership data.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgMembershipResponse {
    pub id: String,
    pub user_id: UserId,
    pub org_id: OrgId,
    pub org_name: String,
    pub role: OrgRole,
    pub created_at: DateTime<Utc>,
}

impl From<OrganizationMembership> for OrgMembershipResponse {
    fn from(m: OrganizationMembership) -> Self {
        Self {
            id: m.id,
            user_id: m.user_id,
            org_id: m.org_id,
            org_name: m.org_name,
            role: m.role,
            created_at: m.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_status_round_trip() {
        for (input, expected) in [
            ("active", OrgStatus::Active),
            ("suspended", OrgStatus::Suspended),
            ("archived", OrgStatus::Archived),
        ] {
            let parsed = input.parse::<OrgStatus>().unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), input);
        }

        let err = "invalid".parse::<OrgStatus>().unwrap_err();
        assert_eq!(err.0, "invalid");
    }

    #[test]
    fn org_role_round_trip() {
        for (input, expected) in [
            ("owner", OrgRole::Owner),
            ("admin", OrgRole::Admin),
            ("member", OrgRole::Member),
            ("viewer", OrgRole::Viewer),
        ] {
            let parsed = input.parse::<OrgRole>().unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), input);
        }

        let err = "invalid".parse::<OrgRole>().unwrap_err();
        assert_eq!(err.0, "invalid");
    }

    #[test]
    fn org_role_is_admin() {
        assert!(OrgRole::Owner.is_admin());
        assert!(OrgRole::Admin.is_admin());
        assert!(!OrgRole::Member.is_admin());
        assert!(!OrgRole::Viewer.is_admin());
    }

    #[test]
    fn org_is_active() {
        let org = Organization {
            id: OrgId::new(),
            name: "test-org".to_string(),
            display_name: "Test Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
            status: OrgStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert!(org.is_active());

        let suspended = Organization { status: OrgStatus::Suspended, ..org.clone() };
        assert!(!suspended.is_active());
    }

    #[test]
    fn org_response_conversion() {
        let org = Organization {
            id: OrgId::new(),
            name: "test-org".to_string(),
            display_name: "Test Org".to_string(),
            description: Some("A test org".to_string()),
            owner_user_id: None,
            settings: None,
            status: OrgStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let response: OrganizationResponse = org.clone().into();
        assert_eq!(response.id, org.id);
        assert_eq!(response.name, org.name);
        assert_eq!(response.display_name, org.display_name);
    }
}
