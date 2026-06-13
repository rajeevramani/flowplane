//! Domain events (spec/10 §3.3): the closed vocabulary connecting subsystems.
//!
//! Every mutation in fp-core appends its events to the outbox in the same transaction;
//! consumers (xDS rebuilder, MCP tool generator, …) react. Variants are versioned by
//! addition — never repurposed.

use crate::id::{OrgId, TeamId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    // Gateway resources (S3+; payloads carry ids as plain UUIDs for stable wire shape)
    ClusterUpserted {
        cluster_id: Uuid,
        name: String,
    },
    ClusterDeleted {
        cluster_id: Uuid,
        name: String,
    },
    RouteConfigUpserted {
        route_config_id: Uuid,
        name: String,
    },
    RouteConfigDeleted {
        route_config_id: Uuid,
        name: String,
    },
    ListenerUpserted {
        listener_id: Uuid,
        name: String,
    },
    ListenerDeleted {
        listener_id: Uuid,
        name: String,
    },
    // Identity / governance
    TeamCreated {
        team_id: Uuid,
        name: String,
    },
    TeamDeleted {
        team_id: Uuid,
        name: String,
    },
    // Dataplanes / mTLS certificate registry (S5.4)
    DataplaneCreated {
        dataplane_id: Uuid,
        name: String,
    },
    ProxyCertificateRegistered {
        certificate_id: Uuid,
        spiffe_uri: String,
    },
    ProxyCertificateRevoked {
        certificate_id: Uuid,
        spiffe_uri: String,
    },
    SecretUpserted {
        secret_id: Uuid,
        name: String,
    },
}

impl DomainEvent {
    /// Stable wire string stored in `events.event_type` (used for consumer filtering).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ClusterUpserted { .. } => "cluster.upserted",
            Self::ClusterDeleted { .. } => "cluster.deleted",
            Self::RouteConfigUpserted { .. } => "route_config.upserted",
            Self::RouteConfigDeleted { .. } => "route_config.deleted",
            Self::ListenerUpserted { .. } => "listener.upserted",
            Self::ListenerDeleted { .. } => "listener.deleted",
            Self::TeamCreated { .. } => "team.created",
            Self::TeamDeleted { .. } => "team.deleted",
            Self::DataplaneCreated { .. } => "dataplane.created",
            Self::ProxyCertificateRegistered { .. } => "proxy_certificate.registered",
            Self::ProxyCertificateRevoked { .. } => "proxy_certificate.revoked",
            Self::SecretUpserted { .. } => "secret.upserted",
        }
    }
}

/// Tenancy scope attached to every event row (consumers rebuild per team).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventScope {
    pub org_id: Option<OrgId>,
    pub team_id: Option<TeamId>,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_with_stable_tag() {
        let event = DomainEvent::ClusterUpserted {
            cluster_id: Uuid::now_v7(),
            name: "x".into(),
        };
        let json = serde_json::to_value(&event).expect("serialize");
        assert_eq!(json["type"], "cluster_upserted");
        let back: DomainEvent = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, event);
    }

    #[test]
    fn unknown_event_types_fail_loud_not_silent() {
        let err = serde_json::from_value::<DomainEvent>(
            serde_json::json!({"type": "from_the_future", "x": 1}),
        );
        assert!(
            err.is_err(),
            "consumers must surface unknown events, not drop them silently"
        );
    }
}
