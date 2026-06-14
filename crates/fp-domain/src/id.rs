//! Newtype identifiers (spec/10 §3.1).
//!
//! Every entity gets its own ID type via [`domain_id!`] — UUIDv7 for index locality, with
//! serde/Display/FromStr. Passing a `TeamId` where an `OrgId` is expected is a compile error;
//! that property is the foundation of the `TeamScope` repository pattern (spec/10 §4).

/// Declare a domain identifier newtype backed by UUIDv7.
#[macro_export]
macro_rules! domain_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Generate a new time-ordered (v7) identifier.
            pub fn generate() -> Self {
                Self(uuid::Uuid::now_v7())
            }

            pub fn as_uuid(&self) -> uuid::Uuid {
                self.0
            }
        }

        impl From<uuid::Uuid> for $name {
            fn from(value: uuid::Uuid) -> Self {
                Self(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::error::DomainError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(s).map(Self).map_err(|_| {
                    $crate::error::DomainError::validation(format!(
                        concat!("invalid ", stringify!($name), ": \"{}\" is not a UUID"),
                        s
                    ))
                })
            }
        }
    };
}

domain_id!(
    /// Identifies an organization (tenancy root).
    OrgId
);
domain_id!(
    /// Identifies a team — the tenant boundary (spec/08a §1).
    TeamId
);
domain_id!(
    /// Correlates one API request across error body, logs, and traces (spec/10 §8a).
    RequestId
);
domain_id!(
    /// Identifies a human user.
    UserId
);
domain_id!(
    /// Identifies a machine identity (agent).
    AgentId
);
domain_id!(
    /// Identifies a grant row.
    GrantId
);
domain_id!(
    /// Identifies an audit-log entry.
    AuditEntryId
);
domain_id!(
    /// Identifies a membership row (org or team).
    MembershipId
);
domain_id!(
    /// Identifies a cluster (upstream backend definition).
    ClusterId
);
domain_id!(
    /// Identifies a listener.
    ListenerId
);
domain_id!(
    /// Identifies a route configuration.
    RouteConfigId
);
domain_id!(
    /// Identifies a registered dataplane (one Envoy instance).
    DataplaneId
);
domain_id!(
    /// Identifies a proxy certificate registry row (mTLS identity of a dataplane).
    ProxyCertificateId
);
domain_id!(
    /// Identifies an SDS secret.
    SecretId
);
domain_id!(
    /// Identifies an API definition, the config-first lifecycle root for learning/tools.
    ApiDefinitionId
);
domain_id!(
    /// Identifies a binding from an API definition to gateway route scope.
    ApiRouteBindingId
);
domain_id!(
    /// Identifies one immutable API spec version.
    SpecVersionId
);
domain_id!(
    /// Identifies one generated API tool row.
    ApiToolId
);
domain_id!(
    /// Identifies one SpecVersion review/publish audit event.
    SpecVersionReviewEventId
);
domain_id!(
    /// Identifies one API observation/spec retention policy.
    RetentionPolicyId
);
domain_id!(
    /// Identifies one bounded traffic capture session for learning.
    CaptureSessionId
);
domain_id!(
    /// Identifies one accepted raw observation for a learning capture session.
    RawObservationId
);

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn ids_are_distinct_types() {
        // This is a compile-time property; here we just confirm values round-trip.
        let team = TeamId::generate();
        let parsed = TeamId::from_str(&team.to_string());
        assert_eq!(parsed.ok(), Some(team));
    }

    #[test]
    fn invalid_uuid_is_a_validation_error() {
        let err = TeamId::from_str("not-a-uuid");
        assert!(err.is_err());
        if let Err(e) = err {
            assert_eq!(e.code, crate::ErrorCode::ValidationFailed);
        }
    }

    #[test]
    fn v7_ids_are_time_ordered() {
        let a = RequestId::generate();
        let b = RequestId::generate();
        assert!(
            a.as_uuid() <= b.as_uuid(),
            "v7 UUIDs must be monotonic enough to sort"
        );
    }
}
