//! Route Hierarchy Domain Types
//!
//! This module contains domain types for the hierarchical route structure:
//! - RouteConfig contains VirtualHosts
//! - VirtualHost contains RouteRules
//!
//! Filters can be attached at each level with inheritance semantics.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{Decode, Encode, Postgres, Type};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// Attachment level for tracking where a filter was attached.
///
/// This discriminator is used in the listener_auto_filters table to track
/// at which level a filter was attached, enabling proper cleanup when
/// filters are detached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentLevel {
    /// Filter attached at RouteConfig level (applies to all vhosts/routes)
    RouteConfig,
    /// Filter attached at VirtualHost level (applies to all routes in vhost)
    VirtualHost,
    /// Filter attached at Route level (applies to specific route only)
    Route,
}

impl AttachmentLevel {
    /// Convert to database string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            AttachmentLevel::RouteConfig => "route_config",
            AttachmentLevel::VirtualHost => "virtual_host",
            AttachmentLevel::Route => "route",
        }
    }
}

impl fmt::Display for AttachmentLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for AttachmentLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "route_config" => Ok(AttachmentLevel::RouteConfig),
            "virtual_host" => Ok(AttachmentLevel::VirtualHost),
            "route" => Ok(AttachmentLevel::Route),
            _ => Err(format!("Invalid attachment level: {}", s)),
        }
    }
}

// SQLx trait implementations for database compatibility
impl Type<Postgres> for AttachmentLevel {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}

impl<'q> Encode<'q, Postgres> for AttachmentLevel {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <&str as Encode<'q, Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

impl<'r> Decode<'r, Postgres> for AttachmentLevel {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as Decode<'r, Postgres>>::decode(value)?;
        AttachmentLevel::from_str(&s).map_err(|e| e.into())
    }
}

/// Type of path matching for a route rule.
///
/// Determines how the route rule matches incoming request paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteMatchType {
    /// Path prefix matching (e.g., /api/ matches /api/users)
    Prefix,
    /// Exact path matching (e.g., /health matches only /health)
    Exact,
    /// Regular expression matching
    Regex,
    /// Path template matching (e.g., /users/{id})
    PathTemplate,
    /// HTTP CONNECT method matcher
    ConnectMatcher,
}

impl RouteMatchType {
    /// Convert to database string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteMatchType::Prefix => "prefix",
            RouteMatchType::Exact => "exact",
            RouteMatchType::Regex => "regex",
            RouteMatchType::PathTemplate => "path_template",
            RouteMatchType::ConnectMatcher => "connect_matcher",
        }
    }

    /// Generate a route rule name from a path pattern
    pub fn generate_rule_name(&self, path: &str) -> String {
        let sanitized = path
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();

        match self {
            RouteMatchType::Prefix => format!("prefix-{}", sanitized),
            RouteMatchType::Exact => format!("exact-{}", sanitized),
            RouteMatchType::Regex => {
                // Use hash for regex patterns
                let mut hasher = Sha256::new();
                hasher.update(path.as_bytes());
                let hash = format!("{:x}", hasher.finalize());
                format!("regex-{}", &hash[..8])
            }
            RouteMatchType::PathTemplate => format!("template-{}", sanitized),
            RouteMatchType::ConnectMatcher => "connect".to_string(),
        }
    }
}

impl fmt::Display for RouteMatchType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for RouteMatchType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "prefix" => Ok(RouteMatchType::Prefix),
            "exact" => Ok(RouteMatchType::Exact),
            "regex" => Ok(RouteMatchType::Regex),
            "path_template" => Ok(RouteMatchType::PathTemplate),
            "connect_matcher" => Ok(RouteMatchType::ConnectMatcher),
            _ => Err(format!("Invalid route match type: {}", s)),
        }
    }
}

// SQLx trait implementations for database compatibility
impl Type<Postgres> for RouteMatchType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}

impl<'q> Encode<'q, Postgres> for RouteMatchType {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <&str as Encode<'q, Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

impl<'r> Decode<'r, Postgres> for RouteMatchType {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as Decode<'r, Postgres>>::decode(value)?;
        RouteMatchType::from_str(&s).map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_level_string_roundtrip() {
        for level in
            [AttachmentLevel::RouteConfig, AttachmentLevel::VirtualHost, AttachmentLevel::Route]
        {
            let s = level.as_str();
            let parsed: AttachmentLevel = s.parse().expect("Failed to parse");
            assert_eq!(level, parsed);
        }
    }

    #[test]
    fn route_match_type_string_roundtrip() {
        for match_type in [
            RouteMatchType::Prefix,
            RouteMatchType::Exact,
            RouteMatchType::Regex,
            RouteMatchType::PathTemplate,
            RouteMatchType::ConnectMatcher,
        ] {
            let s = match_type.as_str();
            let parsed: RouteMatchType = s.parse().expect("Failed to parse");
            assert_eq!(match_type, parsed);
        }
    }

    #[test]
    fn generate_rule_name_prefix() {
        let name = RouteMatchType::Prefix.generate_rule_name("/api/users");
        // Leading slashes are converted to dashes, then trimmed
        assert_eq!(name, "prefix-api-users");
    }

    #[test]
    fn generate_rule_name_exact() {
        let name = RouteMatchType::Exact.generate_rule_name("/health");
        // Leading slashes are converted to dashes, then trimmed
        assert_eq!(name, "exact-health");
    }

    #[test]
    fn generate_rule_name_regex() {
        let name = RouteMatchType::Regex.generate_rule_name("^/users/[0-9]+$");
        assert!(name.starts_with("regex-"));
        assert_eq!(name.len(), 14); // "regex-" + 8 hex chars
    }

    #[test]
    fn generate_rule_name_connect() {
        let name = RouteMatchType::ConnectMatcher.generate_rule_name("");
        assert_eq!(name, "connect");
    }

    #[test]
    fn attachment_level_serialization() {
        let level = AttachmentLevel::VirtualHost;
        let json = serde_json::to_string(&level).expect("Failed to serialize");
        assert_eq!(json, "\"virtual_host\"");

        let deserialized: AttachmentLevel =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(level, deserialized);
    }

    #[test]
    fn route_match_type_serialization() {
        let match_type = RouteMatchType::PathTemplate;
        let json = serde_json::to_string(&match_type).expect("Failed to serialize");
        assert_eq!(json, "\"path_template\"");

        let deserialized: RouteMatchType =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(match_type, deserialized);
    }
}
