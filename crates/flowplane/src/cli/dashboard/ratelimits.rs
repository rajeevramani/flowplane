//! Rate limits panel (fpv2-cxw.4): domains, lazy per-domain policies, and the
//! listener-attachment join.
//!
//! Attachment semantics (approved design, "Joins" — rate-limit bullet): listeners
//! attach a DOMAIN, never a policy; the panel says "domain attached by <listeners>".
//! The join distinguishes the BUILT-IN limiter (`service_cluster == rate_limit_cluster`,
//! whose persisted Envoy domain the CP rewrote to `{ns_uuid(org)}|{ns_uuid(team)}|{base}`)
//! from a user-supplied EXTERNAL limiter cluster, which carries external semantics and
//! is EXCLUDED from attachment claims entirely.
//!
//! Composed-domain normalization contract: strip exactly one leading
//! `<36-char UUID>|<36-char UUID>|` prefix (both segments must parse as UUIDs) and keep
//! the ENTIRE remaining suffix — never split on `|`, because `|` is legal in a base
//! domain and the dashboard cannot recompute `namespace_uuid`. Absent prefix → the
//! value is used as-is.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

use super::super::client::RestClient;
use super::data::{humanize_age, AuthExpired, Panel};
use super::resources::{
    encode_segment, policies_sub_path, sweep, PartialNotice, Sweep, SweepFailure, Table,
    SWEEP_BYTE_BUDGET,
};
use fp_domain::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER;
use fp_domain::gateway::filters::HttpFilterSpec;
use fp_domain::gateway::listener::ListenerSpec;

/// Structurally strip the CP-composed `{uuid}|{uuid}|` prefix. Returns the base domain
/// when the prefix shape is present, else `None` (caller uses the value as-is).
pub(super) fn strip_composed_prefix(domain: &str) -> Option<&str> {
    let bytes = domain.as_bytes();
    if bytes.len() < 74 || bytes[36] != b'|' || bytes[73] != b'|' {
        return None;
    }
    let (a, b) = (&domain[..36], &domain[37..73]);
    if uuid::Uuid::parse_str(a).is_ok() && uuid::Uuid::parse_str(b).is_ok() {
        Some(&domain[74..])
    } else {
        None
    }
}

/// How complete the listener data behind the attachment join is. Anything but `Full`
/// makes "unattached" an unverifiable claim, so badges are withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttachmentBasis {
    Full,
    /// Listener sweep was partial (budget or mid-sweep failure).
    Partial,
    /// Listener collection was 403/unavailable — attachment is unknown.
    Unknown,
}

/// base domain → listener names attaching it via the BUILT-IN limiter.
pub(super) fn attachment_map(listener_items: &[Value]) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
        for entry in &spec.http_filters {
            let HttpFilterSpec::GlobalRateLimit(cfg) = &entry.filter else {
                continue;
            };
            if cfg.service_cluster != RESERVED_RATE_LIMIT_CLUSTER {
                // External limiter cluster: excluded from attachment claims.
                continue;
            }
            let base = strip_composed_prefix(&cfg.domain).unwrap_or(&cfg.domain);
            map.entry(base.to_string())
                .or_default()
                .push(name.to_string());
        }
    }
    for names in map.values_mut() {
        names.sort();
        names.dedup();
    }
    map
}

#[derive(Debug)]
pub(super) struct DomainRow {
    pub(super) name: String,
    /// Percent-encoded name for the lazy policies partial's query string.
    pub(super) encoded: String,
    /// Listeners attaching this domain via the built-in limiter (empty may mean
    /// unattached OR unknown — see `basis`).
    pub(super) attached_by: Vec<String>,
    pub(super) revision: i64,
    pub(super) updated: String,
}

pub(super) struct RateLimitsData {
    pub(super) domains: Table<DomainRow>,
    pub(super) basis: AttachmentBasis,
}

impl RateLimitsData {
    pub(super) fn basis_full(&self) -> bool {
        self.basis == AttachmentBasis::Full
    }
}

/// Fetch the domains sweep + the listeners sweep (attachment source) and join.
/// 401 anywhere → global re-login. 403/unavailable on DOMAINS → panel state; on
/// LISTENERS → domains still render, attachment basis Unknown.
pub(super) async fn fetch_rate_limits(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<RateLimitsData>, AuthExpired> {
    let domains = match sweep(client, team, "rate-limit-domains", SWEEP_BYTE_BUDGET).await {
        Ok(s) => s,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(SweepFailure::Unauthorized) => return Ok(Panel::Unauthorized),
        Err(SweepFailure::Unavailable) => return Ok(Panel::Unavailable),
    };
    let (attachments, basis) = match sweep(client, team, "listeners", SWEEP_BYTE_BUDGET).await {
        Ok(listeners) => {
            let basis = if listeners.partial.is_some() {
                AttachmentBasis::Partial
            } else {
                AttachmentBasis::Full
            };
            (attachment_map(&listeners.items), basis)
        }
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(SweepFailure::Unauthorized) | Err(SweepFailure::Unavailable) => {
            (BTreeMap::new(), AttachmentBasis::Unknown)
        }
    };

    let partial = domains_notice(&domains);
    let total = domains.total;
    let rows = domains
        .items
        .into_iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string();
            let revision = item.get("revision").and_then(Value::as_i64).unwrap_or(0);
            let updated = item
                .get("updated_at")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .map(|ts| humanize_age(now, ts))
                .unwrap_or_else(|| "?".into());
            DomainRow {
                encoded: encode_segment(&name),
                attached_by: attachments.get(&name).cloned().unwrap_or_default(),
                name,
                revision,
                updated,
            }
        })
        .collect();
    Ok(Panel::Data(RateLimitsData {
        domains: Table {
            rows,
            total,
            partial,
        },
        basis,
    }))
}

fn domains_notice(sweep: &Sweep) -> Option<PartialNotice> {
    sweep.partial.map(|reason| PartialNotice {
        shown: sweep.items.len(),
        total: sweep.total,
        reason,
        collection: "rate-limit domains",
    })
}

// ---------------------------------------------------------------------------------------------
// Per-domain policies (lazy, fetched when a domain row is first expanded).
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub(super) struct PolicyRow {
    pub(super) name: String,
    pub(super) requests_per_unit: u64,
    pub(super) unit: String,
    pub(super) descriptors: usize,
    pub(super) revision: i64,
    pub(super) unparsed: bool,
}

pub(super) async fn fetch_policies(
    client: &RestClient,
    team: &str,
    domain: &str,
) -> Result<Panel<Table<PolicyRow>>, AuthExpired> {
    let sub = policies_sub_path(domain);
    let swept = match sweep(client, team, &sub, SWEEP_BYTE_BUDGET).await {
        Ok(s) => s,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(SweepFailure::Unauthorized) => return Ok(Panel::Unauthorized),
        Err(SweepFailure::Unavailable) => return Ok(Panel::Unavailable),
    };
    let partial = swept.partial.map(|reason| PartialNotice {
        shown: swept.items.len(),
        total: swept.total,
        reason,
        collection: "policies",
    });
    let total = swept.total;
    let rows = swept
        .items
        .into_iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string();
            let revision = item.get("revision").and_then(Value::as_i64).unwrap_or(0);
            let spec = item.get("spec");
            let requests_per_unit = spec
                .and_then(|s| s.get("requests_per_unit"))
                .and_then(Value::as_u64);
            let unit = spec
                .and_then(|s| s.get("unit"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let descriptors = spec
                .and_then(|s| s.get("descriptors"))
                .and_then(Value::as_object)
                .map(|m| m.len());
            match (requests_per_unit, unit, descriptors) {
                (Some(rpu), Some(unit), Some(descriptors)) => PolicyRow {
                    name,
                    requests_per_unit: rpu,
                    unit,
                    descriptors,
                    revision,
                    unparsed: false,
                },
                _ => PolicyRow {
                    name,
                    requests_per_unit: 0,
                    unit: "?".into(),
                    descriptors: 0,
                    revision,
                    unparsed: true,
                },
            }
        })
        .collect();
    Ok(Panel::Data(Table {
        rows,
        total,
        partial,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    const NS_A: &str = "8c2a7f1e-4b3d-8f6a-9c1b-2e5d7a9f3c1b";
    const NS_B: &str = "1f9e3d7c-5a2b-8e4f-a6d1-3c8b5f2e9a7d";

    #[test]
    fn structural_prefix_strip_keeps_entire_suffix() {
        // Base domain containing '|' — the adversarial case the design pins: never
        // split on '|', keep the whole suffix.
        let base = "check|out|api";
        let composed = format!("{NS_A}|{NS_B}|{base}");
        assert_eq!(strip_composed_prefix(&composed), Some(base));
        // Plain base.
        let composed = format!("{NS_A}|{NS_B}|checkout");
        assert_eq!(strip_composed_prefix(&composed), Some("checkout"));
        // Empty base is structurally valid.
        let composed = format!("{NS_A}|{NS_B}|");
        assert_eq!(strip_composed_prefix(&composed), Some(""));
    }

    #[test]
    fn non_composed_values_pass_through_untouched() {
        for raw in [
            "checkout",
            "a|b", // pipes but not UUID-shaped
            "not-a-uuid-xxxxxxxxxxxxxxxxxxxxxxxx|also-not-a-uuid-yyyyyyyyyyyyyyyyyyyy|base",
            "",
        ] {
            assert_eq!(strip_composed_prefix(raw), None, "{raw:?}");
        }
        // One valid UUID then junk.
        let half = format!("{NS_A}|definitely-not-a-uuid-here-padding00|base");
        assert_eq!(strip_composed_prefix(&half), None);
    }

    fn listener_with_grl(name: &str, domain: &str, service_cluster: &str) -> Value {
        json!({
            "name": name,
            "spec": {
                "address": "0.0.0.0", "port": 8080,
                "http_filters": [
                    {"filter": {"type": "global_rate_limit",
                        "domain": domain,
                        "service_cluster": service_cluster}}
                ]
            }
        })
    }

    #[test]
    fn attachment_join_built_in_normalized_external_excluded() {
        let composed = format!("{NS_A}|{NS_B}|checkout");
        let items = vec![
            listener_with_grl("edge-a", &composed, RESERVED_RATE_LIMIT_CLUSTER),
            listener_with_grl("edge-b", &composed, RESERVED_RATE_LIMIT_CLUSTER),
            // External limiter: same-looking domain but a user cluster — EXCLUDED.
            listener_with_grl("edge-ext", "checkout", "my-external-rls"),
        ];
        let map = attachment_map(&items);
        assert_eq!(
            map.get("checkout").map(Vec::as_slice),
            Some(["edge-a".to_string(), "edge-b".to_string()].as_slice()),
            "built-in attachments match the normalized base domain"
        );
        assert_eq!(map.len(), 1, "the external limiter must claim nothing");
    }

    #[test]
    fn attachment_join_adversarial_pipe_base_domain() {
        let base = "multi|part|domain";
        let composed = format!("{NS_A}|{NS_B}|{base}");
        let map = attachment_map(&[listener_with_grl(
            "edge",
            &composed,
            RESERVED_RATE_LIMIT_CLUSTER,
        )]);
        assert_eq!(map.get(base).map(Vec::len), Some(1));
        assert!(
            !map.contains_key("multi"),
            "never split the base on '|': {map:?}"
        );
    }

    #[test]
    fn uncomposed_built_in_domain_used_as_is() {
        // Absent prefix (e.g. dev data predating composition) → value as-is.
        let map = attachment_map(&[listener_with_grl(
            "edge",
            "raw-domain",
            RESERVED_RATE_LIMIT_CLUSTER,
        )]);
        assert_eq!(map.get("raw-domain").map(Vec::len), Some(1));
    }
}
