//! S9 traffic-first discovery sessions.

use crate::api_lifecycle::RawObservation;
use crate::error::{DomainError, DomainResult};
use crate::id::{DiscoverySessionId, ListenerId, TeamId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DiscoveryObservationKey {
    pub observed_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_sni: Option<String>,
    pub forwarded_upstream_host: String,
    pub forwarded_upstream_port: i32,
    pub forwarded_upstream_tls: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryObservationProvenance {
    pub discovery_session_id: DiscoverySessionId,
    pub discovery_listener_id: ListenerId,
    pub observed_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_sni: Option<String>,
    pub route_matched: bool,
    pub forwarded_upstream_host: String,
    pub forwarded_upstream_port: i32,
    pub forwarded_upstream_ip: String,
    pub forwarded_upstream_tls: bool,
}

impl DiscoveryObservationProvenance {
    pub fn key(&self) -> DiscoveryObservationKey {
        DiscoveryObservationKey {
            observed_host: self.observed_host.clone(),
            observed_sni: self.observed_sni.clone(),
            forwarded_upstream_host: self.forwarded_upstream_host.clone(),
            forwarded_upstream_port: self.forwarded_upstream_port,
            forwarded_upstream_tls: self.forwarded_upstream_tls,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryObservation {
    pub raw: RawObservation,
    pub provenance: DiscoveryObservationProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryCandidateCluster {
    pub key: DiscoveryObservationKey,
    pub discovery_session_id: DiscoverySessionId,
    pub observations: Vec<RawObservation>,
}

pub fn cluster_discovery_observations(
    observations: Vec<DiscoveryObservation>,
) -> DomainResult<Vec<DiscoveryCandidateCluster>> {
    let Some(first) = observations.first() else {
        return Ok(Vec::new());
    };
    let team_id = first.raw.team_id;
    let discovery_session_id = first.provenance.discovery_session_id;
    let mut grouped: BTreeMap<DiscoveryObservationKey, Vec<RawObservation>> = BTreeMap::new();
    for observation in observations {
        if observation.raw.team_id != team_id
            || observation.provenance.discovery_session_id != discovery_session_id
        {
            return Err(DomainError::validation(
                "discovery clustering requires one team and discovery session",
            ));
        }
        if observation.raw.capture_session_id.is_some() {
            return Err(DomainError::validation(
                "discovery observations must not reference capture sessions",
            ));
        }
        grouped
            .entry(observation.provenance.key())
            .or_default()
            .push(observation.raw);
    }
    Ok(grouped
        .into_iter()
        .map(|(key, observations)| DiscoveryCandidateCluster {
            key,
            discovery_session_id,
            observations,
        })
        .collect())
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::api_lifecycle::RawObservation;
    use crate::{DiscoverySessionId, ListenerId, RawObservationId, TeamId};
    use chrono::Utc;

    #[test]
    fn discovery_clustering_splits_same_path_by_host() {
        let team_id = TeamId::generate();
        let session_id = DiscoverySessionId::generate();
        let listener_id = ListenerId::generate();
        let rows = vec![
            observation(team_id, session_id, listener_id, "api-a.example.test"),
            observation(team_id, session_id, listener_id, "api-b.example.test"),
        ];

        let clusters = cluster_discovery_observations(rows).expect("clusters");

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].key.observed_host, "api-a.example.test");
        assert_eq!(clusters[1].key.observed_host, "api-b.example.test");
        assert!(clusters
            .iter()
            .all(|cluster| cluster.observations.len() == 1));
    }

    #[test]
    fn discovery_clustering_rejects_cross_team_rows() {
        let session_id = DiscoverySessionId::generate();
        let listener_id = ListenerId::generate();
        let rows = vec![
            observation(
                TeamId::generate(),
                session_id,
                listener_id,
                "api-a.example.test",
            ),
            observation(
                TeamId::generate(),
                session_id,
                listener_id,
                "api-b.example.test",
            ),
        ];

        let err = cluster_discovery_observations(rows).expect_err("cross-team rejected");

        assert!(err.message.contains("one team"));
    }

    fn observation(
        team_id: TeamId,
        session_id: DiscoverySessionId,
        listener_id: ListenerId,
        host: &str,
    ) -> DiscoveryObservation {
        DiscoveryObservation {
            raw: RawObservation {
                id: RawObservationId::generate(),
                team_id,
                capture_session_id: None,
                request_id: host.into(),
                method: "GET".into(),
                path: "/v1/items".into(),
                response_status: Some(200),
                request_headers: serde_json::json!({"host": host}),
                response_headers: serde_json::json!({}),
                request_body: None,
                response_body: None,
                request_body_truncated: false,
                response_body_truncated: false,
                request_body_bytes: 0,
                response_body_bytes: 0,
                metadata_seen: true,
                body_seen: false,
                observed_at: Utc::now(),
                updated_at: Utc::now(),
                created_at: Utc::now(),
            },
            provenance: DiscoveryObservationProvenance {
                discovery_session_id: session_id,
                discovery_listener_id: listener_id,
                observed_host: host.into(),
                observed_sni: None,
                route_matched: false,
                forwarded_upstream_host: "upstream.example.test".into(),
                forwarded_upstream_port: 443,
                forwarded_upstream_ip: "93.184.216.34".into(),
                forwarded_upstream_tls: true,
            },
        }
    }
}
