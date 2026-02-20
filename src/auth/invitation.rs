//! Invitation domain models and types for invite-only registration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use utoipa::ToSchema;
use validator::Validate;

use crate::auth::organization::OrgRole;
use crate::auth::user_validation::{validate_email, validate_password, validate_user_name};
use crate::domain::{InvitationId, OrgId, UserId};

/// Status of an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Expired,
    Revoked,
}

impl InvitationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvitationStatus::Pending => "pending",
            InvitationStatus::Accepted => "accepted",
            InvitationStatus::Expired => "expired",
            InvitationStatus::Revoked => "revoked",
        }
    }
}

impl Display for InvitationStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for InvitationStatus {
    type Err = InvitationStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(InvitationStatus::Pending),
            "accepted" => Ok(InvitationStatus::Accepted),
            "expired" => Ok(InvitationStatus::Expired),
            "revoked" => Ok(InvitationStatus::Revoked),
            other => Err(InvitationStatusParseError(other.to_string())),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid invitation status: {0}")]
pub struct InvitationStatusParseError(pub String);

/// Database invitation record.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    pub id: InvitationId,
    pub org_id: OrgId,
    pub email: String,
    pub role: OrgRole,
    pub status: InvitationStatus,
    pub invited_by: Option<UserId>,
    pub accepted_by: Option<UserId>,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub accepted_at: Option<DateTime<Utc>>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
}

/// Request to create an invitation (from HTTP handler).
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateInvitationRequest {
    #[validate(custom(function = "validate_email"))]
    pub email: String,
    pub role: OrgRole,
}

/// Response after creating an invitation (includes plaintext invite URL — one-time display).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateInvitationResponse {
    pub id: String,
    pub email: String,
    pub role: OrgRole,
    pub org_name: String,
    pub invite_url: String,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
}

/// Invitation list item response (for admin listing — no token info).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationResponse {
    pub id: String,
    pub email: String,
    pub role: OrgRole,
    pub status: InvitationStatus,
    pub invited_by: Option<String>,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
}

/// Public token validation response — reveals only what the registering user needs.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteTokenInfo {
    pub org_name: String,
    pub org_display_name: String,
    pub email: String,
    pub role: OrgRole,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
}

/// Request to accept an invitation and register a new user.
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcceptInvitationRequest {
    pub token: String,
    #[validate(custom(function = "validate_user_name"))]
    pub name: String,
    #[validate(custom(function = "validate_password"))]
    pub password: String,
}

/// Paginated invitation listing response.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedInvitations {
    pub invitations: Vec<InvitationResponse>,
    pub total: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invitation_status_roundtrip() {
        for status in [
            InvitationStatus::Pending,
            InvitationStatus::Accepted,
            InvitationStatus::Expired,
            InvitationStatus::Revoked,
        ] {
            let s = status.as_str();
            let parsed: InvitationStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_invitation_status_invalid() {
        let result: Result<InvitationStatus, _> = "invalid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_invitation_status_display() {
        assert_eq!(InvitationStatus::Pending.to_string(), "pending");
        assert_eq!(InvitationStatus::Accepted.to_string(), "accepted");
        assert_eq!(InvitationStatus::Expired.to_string(), "expired");
        assert_eq!(InvitationStatus::Revoked.to_string(), "revoked");
    }

    #[test]
    fn test_invitation_status_as_str_matches_display() {
        for status in [
            InvitationStatus::Pending,
            InvitationStatus::Accepted,
            InvitationStatus::Expired,
            InvitationStatus::Revoked,
        ] {
            assert_eq!(status.as_str(), status.to_string());
        }
    }

    #[test]
    fn test_create_invitation_request_validation() {
        let valid = CreateInvitationRequest {
            email: "user@example.com".to_string(),
            role: OrgRole::Member,
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn test_create_invitation_request_invalid_email() {
        let invalid =
            CreateInvitationRequest { email: "not-an-email".to_string(), role: OrgRole::Member };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_create_invitation_request_empty_email() {
        let empty = CreateInvitationRequest { email: "".to_string(), role: OrgRole::Member };
        assert!(empty.validate().is_err());
    }

    #[test]
    fn test_accept_invitation_request_validation() {
        let valid = AcceptInvitationRequest {
            token: "fp_invite_abc.secret123".to_string(),
            name: "John Doe".to_string(),
            password: "SecureP@ss123!".to_string(),
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn test_accept_invitation_request_empty_name() {
        let invalid = AcceptInvitationRequest {
            token: "fp_invite_abc.secret123".to_string(),
            name: "".to_string(),
            password: "SecureP@ss123!".to_string(),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_accept_invitation_request_empty_password() {
        let invalid = AcceptInvitationRequest {
            token: "fp_invite_abc.secret123".to_string(),
            name: "John Doe".to_string(),
            password: "".to_string(),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_accept_invitation_request_weak_password() {
        let invalid = AcceptInvitationRequest {
            token: "fp_invite_abc.secret123".to_string(),
            name: "John Doe".to_string(),
            password: "weak".to_string(),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_invitation_status_from_str_case_sensitive() {
        // Should reject uppercase variants
        assert!("Pending".parse::<InvitationStatus>().is_err());
        assert!("ACCEPTED".parse::<InvitationStatus>().is_err());
        assert!("Expired".parse::<InvitationStatus>().is_err());
        assert!("REVOKED".parse::<InvitationStatus>().is_err());
    }
}
