//! Identity & tenancy domain types (spec/05, spec/10 §4).

use crate::error::{DomainError, DomainResult};
use crate::id::{AgentId, OrgId, TeamId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Resource-name rule carried from v1 (spec/03 §7.1): lowercase, digit/hyphen segments,
/// must start with a letter, single hyphens only, 1–100 chars. One rule for every
/// human-named resource on every surface.
pub const NAME_MAX_LEN: usize = 100;

pub fn validate_name(name: &str) -> DomainResult<()> {
    if name.is_empty() || name.len() > NAME_MAX_LEN {
        return Err(DomainError::validation(format!(
            "name must be 1-{NAME_MAX_LEN} characters, got {}",
            name.len()
        ))
        .with_hint("names are lowercase letters, digits, and single hyphens, e.g. payments-api"));
    }
    let bytes = name.as_bytes();
    let valid_start = bytes[0].is_ascii_lowercase();
    let mut prev_hyphen = false;
    let mut valid_body = true;
    for &b in bytes {
        match b {
            b'a'..=b'z' | b'0'..=b'9' => prev_hyphen = false,
            b'-' => {
                if prev_hyphen {
                    valid_body = false;
                    break;
                }
                prev_hyphen = true;
            }
            _ => {
                valid_body = false;
                break;
            }
        }
    }
    if !valid_start || !valid_body || bytes.ends_with(b"-") {
        return Err(DomainError::validation(format!(
            "\"{}\" is not a valid name",
            name.chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect::<String>()
        ))
        .with_hint(
            "names start with a lowercase letter and contain only lowercase letters, digits, \
             and single hyphens between segments",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityStatus {
    Active,
    Suspended,
}

impl EntityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }
}

/// Org-level role. Ordering matters: each role includes the powers of those below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrgRole {
    Viewer,
    Member,
    Admin,
    Owner,
}

impl OrgRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "viewer" => Ok(Self::Viewer),
            "member" => Ok(Self::Member),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            other => Err(DomainError::validation(format!(
                "\"{other}\" is not an org role (viewer, member, admin, owner)"
            ))),
        }
    }

    /// Admin and owner get implicit access to every team in their org (spec/05 §3.1 2b).
    pub fn is_org_admin(self) -> bool {
        matches!(self, Self::Admin | Self::Owner)
    }
}

/// Structural partition for machine identities (spec/05 §3): decided at creation,
/// not grantable away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    /// May operate control-plane resources through grants, like a user without org roles.
    CpTool,
    /// May only execute gateway MCP tools; structurally denied all CP resources.
    GatewayTool,
    /// Data-plane consumer; structurally denied all CP resources.
    ApiConsumer,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CpTool => "cp-tool",
            Self::GatewayTool => "gateway-tool",
            Self::ApiConsumer => "api-consumer",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "cp-tool" => Ok(Self::CpTool),
            "gateway-tool" => Ok(Self::GatewayTool),
            "api-consumer" => Ok(Self::ApiConsumer),
            other => Err(DomainError::validation(format!(
                "\"{other}\" is not an agent kind (cp-tool, gateway-tool, api-consumer)"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Organization {
    pub id: OrgId,
    pub name: String,
    pub display_name: String,
    pub status: EntityStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Team {
    pub id: TeamId,
    pub org_id: OrgId,
    pub name: String,
    pub display_name: String,
    pub status: EntityStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct User {
    pub id: UserId,
    /// OIDC `sub` claim — any compliant IdP (Q-004).
    pub subject: String,
    pub email: String,
    pub name: String,
    pub status: EntityStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agent {
    pub id: AgentId,
    pub org_id: OrgId,
    pub name: String,
    pub kind: AgentKind,
    pub status: EntityStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_pass() {
        for name in ["a", "payments", "payments-api", "a1-b2-c3", "x9"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn adversarial_names_fail() {
        let too_long = "a".repeat(101);
        for name in [
            "",
            "-leading",
            "trailing-",
            "double--hyphen",
            "Capital",
            "under_score",
            "dot.name",
            "space name",
            "1starts-with-digit",
            "héllo",
            "name\0null",
            "../../etc/passwd",
            too_long.as_str(),
        ] {
            assert!(validate_name(name).is_err(), "{name:?} should be rejected");
        }
    }

    #[test]
    fn validation_error_strips_control_chars_from_message() {
        let err = validate_name("bad\x07name").expect_err("must fail");
        assert!(!err.message.contains('\x07'));
    }

    #[test]
    fn org_role_ordering_reflects_power() {
        assert!(OrgRole::Owner > OrgRole::Admin);
        assert!(OrgRole::Admin > OrgRole::Member);
        assert!(OrgRole::Member > OrgRole::Viewer);
        assert!(OrgRole::Owner.is_org_admin());
        assert!(OrgRole::Admin.is_org_admin());
        assert!(!OrgRole::Member.is_org_admin());
        assert!(!OrgRole::Viewer.is_org_admin());
    }

    #[test]
    fn role_parse_round_trips() {
        for role in [
            OrgRole::Viewer,
            OrgRole::Member,
            OrgRole::Admin,
            OrgRole::Owner,
        ] {
            assert_eq!(OrgRole::parse(role.as_str()).ok(), Some(role));
        }
        assert!(OrgRole::parse("superuser").is_err());
    }
}
