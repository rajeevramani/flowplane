//! Team domain models and types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use utoipa::ToSchema;
use validator::Validate;

use crate::domain::{OrgId, TeamId, UserId};

/// Status of a team in the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TeamStatus {
    /// Team is active and can be used
    Active,
    /// Team is suspended (resources are preserved but cannot be modified)
    Suspended,
    /// Team is archived (read-only, cannot be reactivated)
    Archived,
}

impl TeamStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TeamStatus::Active => "active",
            TeamStatus::Suspended => "suspended",
            TeamStatus::Archived => "archived",
        }
    }
}

impl Display for TeamStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for TeamStatus {
    type Err = TeamStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(TeamStatus::Active),
            "suspended" => Ok(TeamStatus::Suspended),
            "archived" => Ok(TeamStatus::Archived),
            other => Err(TeamStatusParseError(other.to_string())),
        }
    }
}

/// Error returned when team status parsing fails.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid team status: {0}")]
pub struct TeamStatusParseError(pub String);

/// Represents a team in the system.
///
/// Teams provide multi-tenancy isolation and resource ownership. Each team has:
/// - An immutable unique name (used in xDS metadata and FK references)
/// - A mutable display name (human-friendly)
/// - Optional owner and settings
/// - Status for lifecycle management
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: TeamId,
    /// Unique, immutable identifier (used in xDS and database FK references)
    pub name: String,
    /// Human-friendly name (mutable)
    pub display_name: String,
    /// Optional description
    pub description: Option<String>,
    /// Optional owner user ID
    pub owner_user_id: Option<UserId>,
    /// Organization this team belongs to (required — enforced at DB level)
    pub org_id: OrgId,
    /// Team-specific settings (JSON)
    pub settings: Option<serde_json::Value>,
    /// Team status
    pub status: TeamStatus,
    /// Auto-allocated Envoy admin interface port for this team's Envoy instance
    pub envoy_admin_port: Option<u16>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Team {
    /// Check if the team is active and can be used for operations.
    pub fn is_active(&self) -> bool {
        matches!(self.status, TeamStatus::Active)
    }

    /// Check if the team can be modified.
    pub fn can_be_modified(&self) -> bool {
        matches!(self.status, TeamStatus::Active | TeamStatus::Suspended)
    }
}

/// Request to create a new team.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRequest {
    /// Unique, immutable team name (lowercase, alphanumeric with hyphens)
    #[validate(length(min = 1, max = 255), regex(path = "crate::utils::TEAM_NAME_REGEX"))]
    pub name: String,
    /// Human-friendly display name
    #[validate(length(min = 1, max = 255))]
    pub display_name: String,
    /// Optional description
    #[validate(length(max = 1000))]
    pub description: Option<String>,
    /// Optional owner user ID
    pub owner_user_id: Option<UserId>,
    /// Organization this team belongs to (required for org-scoped uniqueness).
    /// When using the `/orgs/{org_name}/teams` endpoint, this is set automatically
    /// from the URL path and can be omitted from the request body.
    #[serde(default)]
    pub org_id: OrgId,
    /// Optional team-specific settings
    pub settings: Option<serde_json::Value>,
}

/// Request to update an existing team.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamRequest {
    /// Updated display name (name is immutable)
    #[validate(length(min = 1, max = 255))]
    pub display_name: Option<String>,
    /// Updated description
    #[validate(length(max = 1000))]
    pub description: Option<String>,
    /// Updated owner user ID
    pub owner_user_id: Option<UserId>,
    /// Updated settings
    pub settings: Option<serde_json::Value>,
    /// Updated status
    pub status: Option<TeamStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_team_status_parsing() {
        assert_eq!(TeamStatus::from_str("active").unwrap(), TeamStatus::Active);
        assert_eq!(TeamStatus::from_str("suspended").unwrap(), TeamStatus::Suspended);
        assert_eq!(TeamStatus::from_str("archived").unwrap(), TeamStatus::Archived);
        assert!(TeamStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_team_status_display() {
        assert_eq!(TeamStatus::Active.to_string(), "active");
        assert_eq!(TeamStatus::Suspended.to_string(), "suspended");
        assert_eq!(TeamStatus::Archived.to_string(), "archived");
    }

    #[test]
    fn test_team_is_active() {
        let active_team = Team {
            id: TeamId::from_str_unchecked("team-1"),
            name: "engineering".to_string(),
            display_name: "Engineering".to_string(),
            description: None,
            owner_user_id: None,
            org_id: OrgId::from_str_unchecked("test-org"),
            settings: None,
            status: TeamStatus::Active,
            envoy_admin_port: Some(9901),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert!(active_team.is_active());
        assert!(active_team.can_be_modified());

        let suspended_team = Team { status: TeamStatus::Suspended, ..active_team.clone() };
        assert!(!suspended_team.is_active());
        assert!(suspended_team.can_be_modified());

        let archived_team = Team { status: TeamStatus::Archived, ..active_team };
        assert!(!archived_team.is_active());
        assert!(!archived_team.can_be_modified());
    }
}
