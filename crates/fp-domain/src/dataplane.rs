//! Dataplanes and their mTLS certificate registry (spec/04 §1.3, spec/10 §5).
//!
//! A dataplane is one Envoy instance. Its xDS identity is a client certificate whose
//! SPIFFE URI SAN is bound to a `ProxyCertificate` registry row — the row, never the SAN's
//! team segment, is the authorization source (v1 findings B4/B9). Timestamps are real
//! TIMESTAMPTZ end to end (fixes v1's ISO-8601-in-TEXT smell, spec/03 §1.2).

use crate::error::{DomainError, DomainResult};
use crate::id::{DataplaneId, ProxyCertificateId, TeamId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dataplane {
    pub id: DataplaneId,
    pub team_id: TeamId,
    pub name: String,
    pub description: String,
    pub version: i64,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub last_config_verify_at: Option<DateTime<Utc>>,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamStatsOverview {
    pub total_dataplanes: i64,
    pub live_dataplanes: i64,
    pub stale_dataplanes: i64,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
}

/// One issued client certificate. Private keys are never stored — this is the binding and
/// revocation record, keyed by the globally-unique SPIFFE URI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyCertificate {
    pub id: ProxyCertificateId,
    pub team_id: TeamId,
    pub dataplane_id: DataplaneId,
    pub spiffe_uri: String,
    pub serial_number: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Validate a SPIFFE URI for registration: scheme + non-empty trust domain + path. The
/// URI's embedded team/proxy segments are informational; binding authority is the registry
/// row this URI keys.
pub fn validate_spiffe_uri(uri: &str) -> DomainResult<()> {
    let rest = uri.strip_prefix("spiffe://").ok_or_else(|| {
        DomainError::validation("SPIFFE URI must start with \"spiffe://\"")
            .with_hint("expected shape: spiffe://<trust-domain>/org/<org>/team/<team>/proxy/<id>")
    })?;
    if uri.len() > 2048 {
        return Err(DomainError::validation(
            "SPIFFE URI exceeds 2048 characters",
        ));
    }
    if uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(DomainError::validation(
            "SPIFFE URI must not contain whitespace or control characters",
        ));
    }
    let (trust_domain, path) = rest.split_once('/').unwrap_or((rest, ""));
    if trust_domain.is_empty() || path.is_empty() {
        return Err(DomainError::validation(
            "SPIFFE URI must carry a trust domain and a workload path",
        )
        .with_hint("expected shape: spiffe://<trust-domain>/org/<org>/team/<team>/proxy/<id>"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn spiffe_uri_validation() {
        assert!(validate_spiffe_uri("spiffe://flowplane.dev/org/a/team/b/proxy/c").is_ok());
        assert!(validate_spiffe_uri("https://flowplane.dev/team/b").is_err());
        assert!(
            validate_spiffe_uri("spiffe://flowplane.dev").is_err(),
            "no path"
        );
        assert!(
            validate_spiffe_uri("spiffe:///team/b").is_err(),
            "no trust domain"
        );
        assert!(
            validate_spiffe_uri("spiffe://flowplane.dev/team b").is_err(),
            "whitespace"
        );
        let long = format!("spiffe://x/{}", "a".repeat(3000));
        assert!(validate_spiffe_uri(&long).is_err());
    }
}
