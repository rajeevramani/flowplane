//! S9 traffic-first discovery sessions.

use crate::error::{DomainError, DomainResult};
use crate::id::{DiscoverySessionId, TeamId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoverySession {
    pub id: DiscoverySessionId,
    pub team_id: TeamId,
    pub name: String,
    pub status: DiscoverySessionStatus,
    pub listener_port: i32,
    pub upstream_host: String,
    pub upstream_port: i32,
    pub upstream_tls: bool,
    pub validated_upstream_ip: String,
    pub validated_upstream_port: i32,
    pub cluster_name: String,
    pub route_config_name: String,
    pub listener_name: String,
    pub target_sample_count: i32,
    pub max_duration_seconds: Option<i32>,
    pub max_bytes: i64,
    pub max_distinct_paths: i32,
    pub sample_count: i64,
    pub byte_count: i64,
    pub path_count: i64,
    pub drop_count: i64,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySessionStatus {
    Capturing,
    Completed,
    Cancelled,
    Failed,
}

impl DiscoverySessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capturing => "capturing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "capturing" => Ok(Self::Capturing),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            other => Err(DomainError::validation(format!(
                "unknown discovery session status \"{other}\""
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverySessionSpec {
    pub listener_port: i32,
    pub upstream_host: String,
    pub upstream_port: i32,
    pub upstream_tls: bool,
    pub target_sample_count: i32,
    pub max_duration_seconds: Option<i32>,
    pub max_bytes: i64,
    pub max_distinct_paths: i32,
}

impl DiscoverySessionSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if !(1024..=65535).contains(&self.listener_port) {
            return Err(DomainError::validation(
                "discovery listener_port must be between 1024 and 65535",
            ));
        }
        if !(1..=65535).contains(&self.upstream_port) {
            return Err(DomainError::validation(
                "discovery upstream_port must be between 1 and 65535",
            ));
        }
        if self.upstream_host.trim().is_empty()
            || self.upstream_host.contains('/')
            || self.upstream_host.contains('@')
            || self.upstream_host.contains(':')
            || self.upstream_host == "*"
        {
            return Err(DomainError::validation(
                "discovery upstream_host must be a bare host without scheme, path, credentials, or port",
            ));
        }
        if !(1..=100_000).contains(&self.target_sample_count) {
            return Err(DomainError::validation(
                "discovery target_sample_count must be between 1 and 100000",
            ));
        }
        if let Some(max_duration) = self.max_duration_seconds {
            if !(1..=86_400).contains(&max_duration) {
                return Err(DomainError::validation(
                    "discovery max_duration_seconds must be between 1 and 86400",
                ));
            }
        }
        if !(1..=1_073_741_824).contains(&self.max_bytes) {
            return Err(DomainError::validation(
                "discovery max_bytes must be between 1 and 1073741824",
            ));
        }
        if !(1..=10_000).contains(&self.max_distinct_paths) {
            return Err(DomainError::validation(
                "discovery max_distinct_paths must be between 1 and 10000",
            ));
        }
        Ok(())
    }
}
