//! Secrets panel (fpv2-cxw.3): metadata, expiry warnings, and used-by references.
//!
//! Security contract (approved design, "Security / isolation / auth"): secret VALUES
//! are never fetched or rendered — the upstream `secrets` list is metadata-only
//! (`value_redacted: true`) and the CP exposes no value-read route; the dashboard's
//! upstream allowlist test pins that none can be added.
//!
//! Used-by join contract (design-review pass 1): listener/cluster SDS references are
//! secret NAMES and match `SecretView.name`; AI-provider `credential_secret_id` is a
//! UUID and matches `SecretView.id`. A secret is unreferenced only when it matches
//! under neither key — but flagging UNREFERENCED secrets is the orphans panel's job
//! (S6); this panel lists the references it can prove, and declares the used-by
//! column unknown when any reference source is incomplete.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

use super::super::client::RestClient;
use super::data::{humanize_age, AuthExpired, Panel};
use super::resources::{sweep, PartialNotice, Sweep, SweepFailure, Table, SWEEP_BYTE_BUDGET};
use fp_domain::gateway::listener::ListenerSpec;
use fp_domain::gateway::ClusterSpec;

/// Expiry warning horizon (design AC 4: warn ≤ 30 days).
const EXPIRY_WARN_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExpiryState {
    None,
    /// Expires within [`EXPIRY_WARN_DAYS`].
    Warning,
    /// Already past `expires_at`.
    Expired,
    Ok,
}

/// Whether every reference source (listeners, clusters, AI providers) swept fully.
/// Anything less makes used-by an incomplete claim, so the column says unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsedByBasis {
    Full,
    Incomplete,
}

#[derive(Debug)]
pub(super) struct SecretRow {
    pub(super) name: String,
    pub(super) secret_type: String,
    pub(super) expires: Option<String>,
    pub(super) expiry: ExpiryState,
    pub(super) used_by: Vec<String>,
    pub(super) revision: i64,
    pub(super) updated: String,
}

impl SecretRow {
    pub(super) fn expiry_warning(&self) -> bool {
        self.expiry == ExpiryState::Warning
    }
    pub(super) fn expiry_expired(&self) -> bool {
        self.expiry == ExpiryState::Expired
    }
}

pub(super) struct SecretsData {
    pub(super) secrets: Table<SecretRow>,
    pub(super) basis: UsedByBasis,
}

impl SecretsData {
    pub(super) fn basis_full(&self) -> bool {
        self.basis == UsedByBasis::Full
    }
}

/// References keyed by secret NAME (SDS refs) and by secret ID (AI providers).
pub(super) struct References {
    pub(super) by_name: BTreeMap<String, Vec<String>>,
    pub(super) by_id: BTreeMap<String, Vec<String>>,
}

pub(super) fn build_references(
    listener_items: &[Value],
    cluster_items: &[Value],
    ai_provider_items: &[Value],
) -> References {
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for item in listener_items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(spec) = item
            .get("spec")
            .and_then(|s| serde_json::from_value::<ListenerSpec>(s.clone()).ok())
        else {
            continue;
        };
        if let Some(tls) = &spec.tls_context {
            if let Some(sds) = &tls.tls_certificate_sds_secret_name {
                by_name
                    .entry(sds.clone())
                    .or_default()
                    .push(format!("listener {name} (TLS certificate)"));
            }
            if let Some(sds) = &tls.validation_context_sds_secret_name {
                by_name
                    .entry(sds.clone())
                    .or_default()
                    .push(format!("listener {name} (client-CA validation)"));
            }
        }
    }

    for item in cluster_items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(spec) = item
            .get("spec")
            .and_then(|s| serde_json::from_value::<ClusterSpec>(s.clone()).ok())
        else {
            continue;
        };
        if let Some(tls) = &spec.upstream_tls {
            if let Some(sds) = &tls.validation_context_sds_secret_name {
                by_name
                    .entry(sds.clone())
                    .or_default()
                    .push(format!("cluster {name} (upstream CA validation)"));
            }
        }
    }

    for item in ai_provider_items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        // Only the credential id is needed — read it directly so unrelated AI-spec
        // evolution cannot invalidate the join.
        let Some(id) = item
            .get("spec")
            .and_then(|s| s.get("credential_secret_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        by_id
            .entry(id.to_ascii_lowercase())
            .or_default()
            .push(format!("AI provider {name} (credential)"));
    }

    References { by_name, by_id }
}

fn expiry_state(expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> ExpiryState {
    match expires_at {
        None => ExpiryState::None,
        Some(ts) if ts <= now => ExpiryState::Expired,
        Some(ts) if ts <= now + chrono::Duration::days(EXPIRY_WARN_DAYS) => ExpiryState::Warning,
        Some(_) => ExpiryState::Ok,
    }
}

pub(super) fn build_rows(
    secret_items: Vec<Value>,
    refs: &References,
    now: DateTime<Utc>,
) -> Vec<SecretRow> {
    secret_items
        .into_iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string();
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let secret_type = item
                .get("secret_type")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let revision = item.get("revision").and_then(Value::as_i64).unwrap_or(0);
            let updated = item
                .get("updated_at")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .map(|ts| humanize_age(now, ts))
                .unwrap_or_else(|| "?".into());
            let expires_at = item
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            let mut used_by: Vec<String> = Vec::new();
            if let Some(names) = refs.by_name.get(&name) {
                used_by.extend(names.iter().cloned());
            }
            if let Some(ids) = refs.by_id.get(&id) {
                used_by.extend(ids.iter().cloned());
            }
            SecretRow {
                name,
                secret_type,
                expires: expires_at.map(|ts| ts.to_rfc3339()),
                expiry: expiry_state(expires_at, now),
                used_by,
                revision,
                updated,
            }
        })
        .collect()
}

/// Fetch secrets metadata + the three reference sources. 401 anywhere → global
/// re-login. 403/unavailable on SECRETS → panel state; on any reference source →
/// secrets still render with used-by declared incomplete.
pub(super) async fn fetch_secrets(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<SecretsData>, AuthExpired> {
    let secrets = match sweep(client, team, "secrets", SWEEP_BYTE_BUDGET).await {
        Ok(s) => s,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(SweepFailure::Unauthorized) => return Ok(Panel::Unauthorized),
        Err(SweepFailure::Unavailable) => return Ok(Panel::Unavailable),
    };

    let mut basis = UsedByBasis::Full;
    let mut source = |result: Result<Sweep, SweepFailure>| -> Result<Vec<Value>, AuthExpired> {
        match result {
            Ok(s) => {
                if s.partial.is_some() {
                    basis = UsedByBasis::Incomplete;
                }
                Ok(s.items)
            }
            Err(SweepFailure::AuthExpired) => Err(AuthExpired),
            Err(SweepFailure::Unauthorized) | Err(SweepFailure::Unavailable) => {
                basis = UsedByBasis::Incomplete;
                Ok(Vec::new())
            }
        }
    };
    let listeners = source(sweep(client, team, "listeners", SWEEP_BYTE_BUDGET).await)?;
    let clusters = source(sweep(client, team, "clusters", SWEEP_BYTE_BUDGET).await)?;
    let providers = source(sweep(client, team, "ai/providers", SWEEP_BYTE_BUDGET).await)?;

    let refs = build_references(&listeners, &clusters, &providers);
    let partial = secrets.partial.map(|reason| PartialNotice {
        shown: secrets.items.len(),
        total: secrets.total,
        reason,
        collection: "secrets",
    });
    let total = secrets.total;
    let rows = build_rows(secrets.items, &refs, now);
    Ok(Panel::Data(SecretsData {
        secrets: Table {
            rows,
            total,
            partial,
        },
        basis,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        "2026-07-18T00:00:00Z".parse().expect("ts")
    }

    fn secret(name: &str, id: &str, expires_at: Option<&str>) -> Value {
        json!({
            "id": id,
            "team_id": "00000000-0000-7000-8000-000000000001",
            "name": name,
            "description": "",
            "secret_type": "tls_certificate",
            "revision": 1,
            "encryption_key_id": "k1",
            "expires_at": expires_at,
            "created_at": "2026-07-01T00:00:00Z",
            "updated_at": "2026-07-17T23:59:00Z",
            "value_redacted": true
        })
    }

    #[test]
    fn used_by_joins_names_for_sds_and_ids_for_ai_providers() {
        let listener = json!({"name": "edge", "spec": {
            "address": "0.0.0.0", "port": 8443,
            "tls_context": {
                "tls_certificate_sds_secret_name": "edge-cert",
                "validation_context_sds_secret_name": "client-ca"
            }
        }});
        let cluster = json!({"name": "checkout", "spec": {
            "endpoints": [{"host": "10.0.0.1", "port": 443}],
            "upstream_tls": {"validation_context_sds_secret_name": "upstream-ca"}
        }});
        let provider_secret_id = "0198f2f3-2222-7000-8000-00000000aaaa";
        let provider = json!({"name": "openai", "spec": {
            "kind": "openai", "base_url": "https://api.openai.com",
            "credential_secret_id": provider_secret_id
        }});
        let refs = build_references(&[listener], &[cluster], &[provider]);

        let rows = build_rows(
            vec![
                secret("edge-cert", "0198f2f3-1111-7000-8000-000000000001", None),
                secret("client-ca", "0198f2f3-1111-7000-8000-000000000002", None),
                secret("upstream-ca", "0198f2f3-1111-7000-8000-000000000003", None),
                // Referenced by UUID even though its NAME matches nothing.
                secret("openai-credential", provider_secret_id, None),
                secret("idle", "0198f2f3-1111-7000-8000-000000000005", None),
            ],
            &refs,
            now(),
        );
        assert_eq!(rows[0].used_by, vec!["listener edge (TLS certificate)"]);
        assert_eq!(
            rows[1].used_by,
            vec!["listener edge (client-CA validation)"]
        );
        assert_eq!(
            rows[2].used_by,
            vec!["cluster checkout (upstream CA validation)"]
        );
        assert_eq!(rows[3].used_by, vec!["AI provider openai (credential)"]);
        assert!(rows[4].used_by.is_empty());
    }

    #[test]
    fn ai_provider_id_join_is_case_insensitive_and_never_name_based() {
        let provider_secret_id = "0198F2F3-2222-7000-8000-00000000AAAA";
        let provider = json!({"name": "openai", "spec": {
            "kind": "openai", "base_url": "https://api.openai.com",
            "credential_secret_id": provider_secret_id
        }});
        let refs = build_references(&[], &[], &[provider]);
        let rows = build_rows(
            vec![
                // Name equals the UUID string of ANOTHER secret — must not match.
                secret(
                    "0198f2f3-2222-7000-8000-00000000aaaa",
                    "0198f2f3-1111-7000-8000-00000000bbbb",
                    None,
                ),
                secret(
                    "real-credential",
                    "0198f2f3-2222-7000-8000-00000000aaaa",
                    None,
                ),
            ],
            &refs,
            now(),
        );
        assert!(
            rows[0].used_by.is_empty(),
            "id joins must never match a NAME: {:?}",
            rows[0].used_by
        );
        assert_eq!(rows[1].used_by, vec!["AI provider openai (credential)"]);
    }

    #[test]
    fn expiry_states_warn_at_30_days_and_flag_expired() {
        let rows = build_rows(
            vec![
                secret("none", "0198f2f3-1111-7000-8000-000000000001", None),
                // 29 days out → warning.
                secret(
                    "soon",
                    "0198f2f3-1111-7000-8000-000000000002",
                    Some("2026-08-16T00:00:00Z"),
                ),
                // Exactly 30 days out → warning (inclusive).
                secret(
                    "edge30",
                    "0198f2f3-1111-7000-8000-000000000003",
                    Some("2026-08-17T00:00:00Z"),
                ),
                // 31 days out → ok.
                secret(
                    "later",
                    "0198f2f3-1111-7000-8000-000000000004",
                    Some("2026-08-18T00:00:01Z"),
                ),
                // In the past → expired.
                secret(
                    "gone",
                    "0198f2f3-1111-7000-8000-000000000005",
                    Some("2026-07-01T00:00:00Z"),
                ),
            ],
            &References {
                by_name: BTreeMap::new(),
                by_id: BTreeMap::new(),
            },
            now(),
        );
        assert_eq!(rows[0].expiry, ExpiryState::None);
        assert_eq!(rows[1].expiry, ExpiryState::Warning);
        assert_eq!(rows[2].expiry, ExpiryState::Warning);
        assert_eq!(rows[3].expiry, ExpiryState::Ok);
        assert_eq!(rows[4].expiry, ExpiryState::Expired);
    }

    #[test]
    fn undecodable_reference_sources_contribute_nothing_but_do_not_panic() {
        let refs = build_references(
            &[json!({"name": "junk", "spec": {"x": 1}})],
            &[json!({"name": "junk", "spec": {"x": 1}})],
            &[json!({"name": "junk", "spec": {"x": 1}})],
        );
        assert!(refs.by_name.is_empty());
        assert!(refs.by_id.is_empty());
    }
}
