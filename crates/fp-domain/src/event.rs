//! Domain events (spec/10 §3.3): the closed vocabulary connecting subsystems.
//!
//! Every mutation in fp-core appends its events to the outbox in the same transaction;
//! consumers (xDS rebuilder, MCP tool generator, …) react. Variants are versioned by
//! addition — never repurposed.

use crate::id::{OrgId, TeamId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum DomainEvent {
    // Gateway resources (S3+; payloads carry ids as plain UUIDs for stable wire shape)
    #[serde(rename = "cluster.upserted", alias = "cluster_upserted")]
    ClusterUpserted { cluster_id: Uuid, name: String },
    #[serde(rename = "cluster.deleted", alias = "cluster_deleted")]
    ClusterDeleted { cluster_id: Uuid, name: String },
    #[serde(rename = "route_config.upserted", alias = "route_config_upserted")]
    RouteConfigUpserted { route_config_id: Uuid, name: String },
    #[serde(rename = "route_config.deleted", alias = "route_config_deleted")]
    RouteConfigDeleted { route_config_id: Uuid, name: String },
    #[serde(rename = "listener.upserted", alias = "listener_upserted")]
    ListenerUpserted { listener_id: Uuid, name: String },
    #[serde(rename = "listener.deleted", alias = "listener_deleted")]
    ListenerDeleted { listener_id: Uuid, name: String },
    // Identity / governance
    #[serde(rename = "team.created", alias = "team_created")]
    TeamCreated { team_id: Uuid, name: String },
    #[serde(rename = "team.deleted", alias = "team_deleted")]
    TeamDeleted { team_id: Uuid, name: String },
    // Dataplanes / mTLS certificate registry (S5.4)
    #[serde(rename = "dataplane.created", alias = "dataplane_created")]
    DataplaneCreated { dataplane_id: Uuid, name: String },
    #[serde(
        rename = "proxy_certificate.registered",
        alias = "proxy_certificate_registered"
    )]
    ProxyCertificateRegistered {
        certificate_id: Uuid,
        spiffe_uri: String,
    },
    #[serde(
        rename = "proxy_certificate.revoked",
        alias = "proxy_certificate_revoked"
    )]
    ProxyCertificateRevoked {
        certificate_id: Uuid,
        spiffe_uri: String,
    },
    #[serde(rename = "secret.upserted", alias = "secret_upserted")]
    SecretUpserted { secret_id: Uuid, name: String },
    // API lifecycle / learning config-first spine (S8)
    #[serde(rename = "api_definition.created", alias = "api_definition_created")]
    ApiDefinitionCreated {
        api_definition_id: Uuid,
        name: String,
    },
    #[serde(rename = "api_definition.deleted", alias = "api_definition_deleted")]
    ApiDefinitionDeleted {
        api_definition_id: Uuid,
        name: String,
    },
    #[serde(rename = "spec_version.created", alias = "spec_version_created")]
    SpecVersionCreated {
        spec_version_id: Uuid,
        api_definition_id: Uuid,
        version: i64,
    },
    #[serde(rename = "api_tools.generated", alias = "api_tools_generated")]
    ApiToolsGenerated {
        api_definition_id: Uuid,
        spec_version_id: Uuid,
        count: usize,
    },
    #[serde(rename = "capture_session.started", alias = "capture_session_started")]
    CaptureSessionStarted {
        capture_session_id: Uuid,
        name: String,
    },
    #[serde(rename = "capture_session.stopped", alias = "capture_session_stopped")]
    CaptureSessionStopped {
        capture_session_id: Uuid,
        name: String,
    },
    #[serde(
        rename = "capture_session.cancelled",
        alias = "capture_session_cancelled"
    )]
    CaptureSessionCancelled {
        capture_session_id: Uuid,
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
            Self::ApiDefinitionCreated { .. } => "api_definition.created",
            Self::ApiDefinitionDeleted { .. } => "api_definition.deleted",
            Self::SpecVersionCreated { .. } => "spec_version.created",
            Self::ApiToolsGenerated { .. } => "api_tools.generated",
            Self::CaptureSessionStarted { .. } => "capture_session.started",
            Self::CaptureSessionStopped { .. } => "capture_session.stopped",
            Self::CaptureSessionCancelled { .. } => "capture_session.cancelled",
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
    fn events_round_trip_with_canonical_kind_tag() {
        let uuid = Uuid::now_v7();
        let events = vec![
            DomainEvent::ClusterUpserted {
                cluster_id: uuid,
                name: "x".into(),
            },
            DomainEvent::ClusterDeleted {
                cluster_id: uuid,
                name: "x".into(),
            },
            DomainEvent::RouteConfigUpserted {
                route_config_id: uuid,
                name: "x".into(),
            },
            DomainEvent::RouteConfigDeleted {
                route_config_id: uuid,
                name: "x".into(),
            },
            DomainEvent::ListenerUpserted {
                listener_id: uuid,
                name: "x".into(),
            },
            DomainEvent::ListenerDeleted {
                listener_id: uuid,
                name: "x".into(),
            },
            DomainEvent::TeamCreated {
                team_id: uuid,
                name: "x".into(),
            },
            DomainEvent::TeamDeleted {
                team_id: uuid,
                name: "x".into(),
            },
            DomainEvent::DataplaneCreated {
                dataplane_id: uuid,
                name: "x".into(),
            },
            DomainEvent::ProxyCertificateRegistered {
                certificate_id: uuid,
                spiffe_uri: "spiffe://flowplane.local/org/o/team/t/dataplane/d".into(),
            },
            DomainEvent::ProxyCertificateRevoked {
                certificate_id: uuid,
                spiffe_uri: "spiffe://flowplane.local/org/o/team/t/dataplane/d".into(),
            },
            DomainEvent::SecretUpserted {
                secret_id: uuid,
                name: "x".into(),
            },
            DomainEvent::ApiDefinitionCreated {
                api_definition_id: uuid,
                name: "x".into(),
            },
            DomainEvent::ApiDefinitionDeleted {
                api_definition_id: uuid,
                name: "x".into(),
            },
            DomainEvent::SpecVersionCreated {
                spec_version_id: uuid,
                api_definition_id: uuid,
                version: 1,
            },
            DomainEvent::ApiToolsGenerated {
                api_definition_id: uuid,
                spec_version_id: uuid,
                count: 2,
            },
            DomainEvent::CaptureSessionStarted {
                capture_session_id: uuid,
                name: "x".into(),
            },
            DomainEvent::CaptureSessionStopped {
                capture_session_id: uuid,
                name: "x".into(),
            },
            DomainEvent::CaptureSessionCancelled {
                capture_session_id: uuid,
                name: "x".into(),
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event).expect("serialize");
            assert_eq!(json["type"], event.kind());
            let back: DomainEvent = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back, event);
        }
    }

    #[test]
    fn legacy_snake_case_event_tags_still_deserialize() {
        let event = DomainEvent::ClusterUpserted {
            cluster_id: Uuid::now_v7(),
            name: "x".into(),
        };
        let json = serde_json::to_value(&event).expect("serialize");
        assert_eq!(json["type"], "cluster.upserted");

        let legacy = serde_json::json!({
            "type": "cluster_upserted",
            "cluster_id": event_cluster_id(&event),
            "name": "x"
        });
        let back: DomainEvent = serde_json::from_value(legacy).expect("deserialize legacy tag");
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

    #[test]
    fn unknown_event_fields_fail_loud_not_silent() {
        let err = serde_json::from_value::<DomainEvent>(serde_json::json!({
            "type": "cluster.upserted",
            "cluster_id": Uuid::now_v7(),
            "name": "x",
            "unexpected": true
        }));
        assert!(
            err.is_err(),
            "consumers must surface unknown event fields, not drop them silently"
        );
    }

    fn event_cluster_id(event: &DomainEvent) -> Uuid {
        match event {
            DomainEvent::ClusterUpserted { cluster_id, .. } => *cluster_id,
            _ => unreachable!("test passes a cluster event"),
        }
    }
}
