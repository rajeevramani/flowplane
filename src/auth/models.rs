//! Data models used by the Flowplane personal access token system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use utoipa::ToSchema;

use crate::domain::{OrgId, TokenId, UserId};
use crate::errors::Error;

/// Lifecycle status for a personal access token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TokenStatus {
    Active,
    Revoked,
    Expired,
}

impl TokenStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenStatus::Active => "active",
            TokenStatus::Revoked => "revoked",
            TokenStatus::Expired => "expired",
        }
    }
}

impl Display for TokenStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for TokenStatus {
    type Err = TokenStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(TokenStatus::Active),
            "revoked" => Ok(TokenStatus::Revoked),
            "expired" => Ok(TokenStatus::Expired),
            other => Err(TokenStatusParseError(other.to_string())),
        }
    }
}

/// Error returned when token status parsing fails.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid token status: {0}")]
pub struct TokenStatusParseError(pub String);

/// Stored representation of a personal access token.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalAccessToken {
    pub id: TokenId,
    pub name: String,
    pub description: Option<String>,
    pub status: TokenStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub scopes: Vec<String>,
    pub user_id: Option<crate::domain::UserId>,
    pub user_email: Option<String>,
}

impl PersonalAccessToken {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// New token database payload.
#[derive(Debug, Clone)]
pub struct NewPersonalAccessToken {
    pub id: TokenId,
    pub name: String,
    pub description: Option<String>,
    pub hashed_secret: String,
    pub status: TokenStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<String>,
    pub scopes: Vec<String>,
    pub is_setup_token: bool,
    pub max_usage_count: Option<i64>,
    pub usage_count: i64,
    pub failed_attempts: i64,
    pub locked_until: Option<DateTime<Utc>>,
    pub user_id: Option<crate::domain::UserId>,
    pub user_email: Option<String>,
}

/// Update payload for an existing token.
#[derive(Debug, Clone, Default)]
pub struct UpdatePersonalAccessToken {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<TokenStatus>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
    pub scopes: Option<Vec<String>>,
}

/// Strongly-typed scope wrapper (helps with validation & display).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenScope(pub String);

impl Display for TokenScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Request-scoped authentication context derived from a valid token or session.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub token_id: TokenId,
    pub token_name: String,
    pub user_id: Option<UserId>,
    pub user_email: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    /// Organization ID for this user (if org-scoped)
    pub org_id: Option<OrgId>,
    /// Organization name for this user (if org-scoped)
    pub org_name: Option<String>,
    scopes: HashSet<String>,
}

impl AuthContext {
    pub fn new(token_id: TokenId, token_name: String, scopes: Vec<String>) -> Self {
        Self {
            token_id,
            token_name,
            user_id: None,
            user_email: None,
            client_ip: None,
            user_agent: None,
            org_id: None,
            org_name: None,
            scopes: scopes.into_iter().collect(),
        }
    }

    pub fn with_user(
        token_id: TokenId,
        token_name: String,
        user_id: UserId,
        user_email: String,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            token_id,
            token_name,
            user_id: Some(user_id),
            user_email: Some(user_email),
            client_ip: None,
            user_agent: None,
            org_id: None,
            org_name: None,
            scopes: scopes.into_iter().collect(),
        }
    }

    /// Set the organization context for this auth context.
    pub fn with_org(mut self, org_id: OrgId, org_name: String) -> Self {
        self.org_id = Some(org_id);
        self.org_name = Some(org_name);
        self
    }

    /// Set the client IP and user agent for this context.
    pub fn with_request_context(
        mut self,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        self.client_ip = client_ip;
        self.user_agent = user_agent;
        self
    }

    /// Extract user context for audit logging.
    ///
    /// Returns a tuple of (user_id, client_ip, user_agent) suitable for
    /// use with `AuditEvent::with_user_context()`.
    pub fn to_audit_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.user_id.as_ref().map(|id| id.to_string()),
            self.client_ip.clone(),
            self.user_agent.clone(),
        )
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }

    pub fn scopes(&self) -> impl Iterator<Item = &String> {
        self.scopes.iter()
    }
}

/// Errors returned by authentication middleware/services.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("unauthorized: bearer token missing")]
    MissingBearer,
    #[error("unauthorized: malformed bearer token")]
    MalformedBearer,
    #[error("unauthorized: token not found")]
    TokenNotFound,
    #[error("unauthorized: token inactive")]
    InactiveToken,
    #[error("unauthorized: token expired")]
    ExpiredToken,
    #[error("forbidden: missing required scope")]
    Forbidden,
    #[error(transparent)]
    Persistence(#[from] Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_status_round_trip() {
        for (input, expected) in [
            ("active", TokenStatus::Active),
            ("revoked", TokenStatus::Revoked),
            ("expired", TokenStatus::Expired),
        ] {
            let parsed = input.parse::<TokenStatus>().unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), input);
        }

        let err = "bad".parse::<TokenStatus>().unwrap_err();
        assert_eq!(err.0, "bad");
    }

    #[test]
    fn auth_context_scope_checks() {
        let ctx = AuthContext::new(
            TokenId::from_string("token-1".to_string()),
            "demo".into(),
            vec!["clusters:read".into(), "clusters:write".into()],
        );

        assert!(ctx.has_scope("clusters:read"));
        assert!(!ctx.has_scope("routes:read"));
        assert_eq!(ctx.scopes().count(), 2);
    }

    #[test]
    fn personal_access_token_has_scope() {
        let token = PersonalAccessToken {
            id: TokenId::from_string("t1".to_string()),
            name: "demo".into(),
            description: None,
            status: TokenStatus::Active,
            expires_at: None,
            last_used_at: None,
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            scopes: vec!["listeners:read".into(), "listeners:write".into()],
            user_id: None,
            user_email: None,
        };

        assert!(token.has_scope("listeners:write"));
        assert!(!token.has_scope("clusters:read"));
    }
}
