//! HTTP filters inventory (fpv2-cxw.5): rows from listener chains plus filter
//! overrides at BOTH scopes of the route-config specs (vhost-level and route-level).
//!
//! The chain vocabulary is the domain's closed 9-kind set; rows render domain kind
//! names (`jwt_auth`, `global_rate_limit`, …). The router filter is appended at xDS
//! translation and never stored, so it renders as a SYNTHESIZED row per listener.
//! Attribute grids are FULL per kind: the typed config's serialization overlaid on
//! the kind's complete field vocabulary ([`KIND_FIELDS`]), so omitted/defaulted
//! fields render as "(default)" instead of vanishing; long values are truncated so a
//! 64 KiB inline JWKS cannot blow up the page. The footer lists kinds IN USE only —
//! the unused-kind list was cut from F2 by the framing review.

use serde_json::Value;
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::super::client::RestClient;
use super::data::AuthExpired;
use super::resources::{sweep, PartialNotice, Sweep, SweepFailure, SWEEP_BYTE_BUDGET};
use fp_domain::gateway::filters::{FilterOverride, HttpFilterEntry};

/// FULL field vocabulary per chain filter kind (the design promises full attribute
/// grids; `skip_serializing_if` would otherwise hide defaulted/absent fields, making
/// "unset" indistinguishable from "nonexistent"). Source of truth:
/// `fp-domain/src/gateway/filters.rs` config structs; a field the domain adds later
/// still renders (extra serialized keys are appended), it just loses its "(default)"
/// placeholder until this table catches up.
const KIND_FIELDS: &[(&str, &[&str])] = &[
    (
        "cors",
        &[
            "allow_origin",
            "allow_methods",
            "allow_headers",
            "expose_headers",
            "max_age_seconds",
            "allow_credentials",
        ],
    ),
    (
        "local_rate_limit",
        &["stat_prefix", "token_bucket", "status_code"],
    ),
    (
        "header_mutation",
        &[
            "request_headers_to_add",
            "request_headers_to_remove",
            "response_headers_to_add",
            "response_headers_to_remove",
        ],
    ),
    (
        "health_check",
        &["endpoint_path", "pass_through_mode", "cache_time_ms"],
    ),
    (
        "compressor",
        &["memory_level", "window_bits", "compression_level"],
    ),
    (
        "jwt_auth",
        &[
            "providers",
            "requirement_map",
            "rules",
            "bypass_cors_preflight",
        ],
    ),
    (
        "ext_authz",
        &[
            "cluster",
            "timeout_ms",
            "failure_mode_allow",
            "include_peer_certificate",
        ],
    ),
    ("rbac", &["action", "policies"]),
    (
        "global_rate_limit",
        &[
            "domain",
            "service_cluster",
            "timeout_ms",
            "failure_mode_deny",
            "stage",
            "request_type",
            "stat_prefix",
            "enable_x_ratelimit_headers",
            "disable_x_envoy_ratelimited_header",
            "rate_limited_status",
            "status_on_error",
        ],
    ),
];

/// FULL field vocabulary per OVERRIDE tag — overrides have their own schemas
/// (`FilterOverride` variants), which for `jwt_auth` and `disable` are much narrower
/// than the chain configs; reusing the chain vocabulary would fabricate fields the
/// override schema does not have.
const OVERRIDE_FIELDS: &[(&str, &[&str])] = &[
    ("disable", &["filter_type"]),
    (
        "cors",
        &[
            "allow_origin",
            "allow_methods",
            "allow_headers",
            "expose_headers",
            "max_age_seconds",
            "allow_credentials",
        ],
    ),
    (
        "local_rate_limit",
        &["stat_prefix", "token_bucket", "status_code"],
    ),
    ("jwt_auth", &["requirement_name"]),
];

/// Truncation bound for one rendered attribute value. A jwt inline JWKS may legally
/// be 64 KiB; the grid shows a prefix plus the real length instead.
const MAX_ATTR_CHARS: usize = 120;

#[derive(Debug)]
pub(super) struct FilterRow {
    pub(super) listener: String,
    /// 1-based chain position; the synthesized router renders last (position = chain
    /// length + 1, where Envoy appends it at translation).
    pub(super) position: usize,
    pub(super) kind: String,
    pub(super) disabled: bool,
    pub(super) synthesized: bool,
    pub(super) attrs: Vec<(String, String)>,
}

#[derive(Debug)]
pub(super) struct OverrideRow {
    pub(super) route_config: String,
    /// "vhost <name>" or "route <vhost>/<route>".
    pub(super) scope: String,
    pub(super) kind: String,
    pub(super) attrs: Vec<(String, String)>,
}

pub(super) struct FiltersInventory {
    pub(super) rows: Vec<FilterRow>,
    pub(super) overrides: Vec<OverrideRow>,
    /// Sorted, deduplicated kinds present in chains + overrides (footer content).
    pub(super) kinds_in_use: Vec<String>,
    pub(super) notices: Vec<PartialNotice>,
}

pub(super) enum FiltersPanel {
    Inventory(FiltersInventory),
    Unauthorized { collection: &'static str },
    Unavailable { collection: &'static str },
}

/// Render one serialized attribute value compactly; long values keep a prefix and
/// state their true size.
fn render_value(value: &Value) -> String {
    let raw = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if raw.chars().count() > MAX_ATTR_CHARS {
        let prefix: String = raw.chars().take(MAX_ATTR_CHARS).collect();
        format!("{prefix}… ({} chars)", raw.chars().count())
    } else {
        raw
    }
}

/// Attribute grid from an internally-tagged serialized config. The grid is FULL per
/// the kind's field vocabulary: a field the serializer omitted (skip_serializing_if
/// on a defaulted/absent value) renders as "(default)" so unset is distinguishable
/// from nonexistent; serialized keys outside the vocabulary (domain drift) are still
/// appended verbatim.
fn attrs_of<T: serde::Serialize>(
    config: &T,
    vocab_table: &[(&str, &[&str])],
) -> (String, Vec<(String, String)>) {
    match serde_json::to_value(config) {
        Ok(Value::Object(map)) => {
            let kind = map
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let vocabulary = vocab_table
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, fields)| *fields)
                .unwrap_or(&[]);
            let mut attrs: Vec<(String, String)> = vocabulary
                .iter()
                .map(|field| {
                    let value = map
                        .get(*field)
                        .map(render_value)
                        .unwrap_or_else(|| "(default)".into());
                    (field.to_string(), value)
                })
                .collect();
            for (k, v) in &map {
                if k != "type" && !vocabulary.contains(&k.as_str()) {
                    attrs.push((k.clone(), render_value(v)));
                }
            }
            (kind, attrs)
        }
        _ => ("?".into(), Vec::new()),
    }
}

pub(super) fn build_inventory(
    listener_items: &[Value],
    route_config_items: &[Value],
    notices: Vec<PartialNotice>,
) -> FiltersInventory {
    let mut rows = Vec::new();
    let mut kinds: BTreeSet<String> = BTreeSet::new();

    // Entries decode INDIVIDUALLY (not through the whole ListenerSpec): one
    // malformed/legacy filter must not silently erase the listener's valid rows —
    // it renders as a visible "(unparseable)" row instead.
    for item in listener_items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)");
        let filters = item
            .get("spec")
            .and_then(|s| s.get("http_filters"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = filters.len();
        for (index, raw_entry) in filters.into_iter().enumerate() {
            match serde_json::from_value::<HttpFilterEntry>(raw_entry) {
                Ok(entry) => {
                    let (kind, attrs) = attrs_of(&entry.filter, KIND_FIELDS);
                    // Disabled entries still count as in use: they are authored chain
                    // members Envoy skips (toggle without re-order), not absent.
                    kinds.insert(kind.clone());
                    rows.push(FilterRow {
                        listener: name.to_string(),
                        position: index + 1,
                        kind,
                        disabled: entry.disabled,
                        synthesized: false,
                        attrs,
                    });
                }
                Err(_) => rows.push(FilterRow {
                    listener: name.to_string(),
                    position: index + 1,
                    kind: "(unparseable)".into(),
                    disabled: false,
                    synthesized: false,
                    attrs: Vec::new(),
                }),
            }
        }
        // The router filter is appended automatically at translation and may not be
        // authored (filters.rs chain contract) — mark it synthesized, last in chain.
        rows.push(FilterRow {
            listener: name.to_string(),
            position: count + 1,
            kind: "router".into(),
            disabled: false,
            synthesized: true,
            attrs: Vec::new(),
        });
    }

    let mut overrides = Vec::new();
    let mut push_override = |rc: &str, scope: String, raw: Value, kinds: &mut BTreeSet<String>| {
        match serde_json::from_value::<FilterOverride>(raw) {
            Ok(ov) => {
                // Overrides use their OWN schema vocabulary (a jwt_auth override has
                // only `requirement_name`, never the chain config's fields).
                let (_, attrs) = attrs_of(&ov, OVERRIDE_FIELDS);
                // The rendered kind is the TARGET domain kind: a Disable override
                // shows the chain kind it disables (e.g. `compressor`), never the
                // non-vocabulary tag `disable` — that stays visible in the attrs
                // (`filter_type`). A disable whose target is not a known kind is
                // malformed data: render it unparseable, never count it in use.
                match ov.target_kind() {
                    Ok(target) => {
                        let kind = target.to_string();
                        kinds.insert(kind.clone());
                        overrides.push(OverrideRow {
                            route_config: rc.to_string(),
                            scope,
                            kind,
                            attrs,
                        });
                    }
                    Err(_) => overrides.push(OverrideRow {
                        route_config: rc.to_string(),
                        scope,
                        kind: "(unparseable)".into(),
                        attrs,
                    }),
                }
            }
            // A malformed override renders visibly instead of erasing the row.
            Err(_) => overrides.push(OverrideRow {
                route_config: rc.to_string(),
                scope,
                kind: "(unparseable)".into(),
                attrs: Vec::new(),
            }),
        }
    };

    // Same per-entry resilience as chains: walk the raw arrays so one malformed
    // override or route cannot erase a whole route config's inventory.
    for item in route_config_items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)");
        let vhosts = item
            .get("spec")
            .and_then(|s| s.get("virtual_hosts"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for vh in vhosts {
            let vh_name = vh
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string();
            for ov in vh
                .get("filter_overrides")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                push_override(name, format!("vhost {vh_name}"), ov, &mut kinds);
            }
            for route in vh
                .get("routes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let route_name = route
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("(unknown)")
                    .to_string();
                for ov in route
                    .get("filter_overrides")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    push_override(
                        name,
                        format!("route {vh_name}/{route_name}"),
                        ov,
                        &mut kinds,
                    );
                }
            }
        }
    }

    // `router` is synthesized on every listener, not an authored kind; the in-use
    // footer lists authored vocabulary only.
    FiltersInventory {
        rows,
        overrides,
        kinds_in_use: kinds.into_iter().collect(),
        notices,
    }
}

pub(super) async fn fetch_filters(
    client: &RestClient,
    team: &str,
    _now: DateTime<Utc>,
) -> Result<FiltersPanel, AuthExpired> {
    let one = |result: Result<Sweep, SweepFailure>, collection: &'static str| match result {
        Ok(s) => Ok(s),
        Err(SweepFailure::AuthExpired) => Err(None),
        Err(SweepFailure::Unauthorized) => Err(Some(FiltersPanel::Unauthorized { collection })),
        Err(SweepFailure::Unavailable) => Err(Some(FiltersPanel::Unavailable { collection })),
    };
    let listeners = match one(
        sweep(client, team, "listeners", SWEEP_BYTE_BUDGET).await,
        "listeners",
    ) {
        Ok(s) => s,
        Err(None) => return Err(AuthExpired),
        Err(Some(panel)) => return Ok(panel),
    };
    let route_configs = match one(
        sweep(client, team, "route-configs", SWEEP_BYTE_BUDGET).await,
        "route-configs",
    ) {
        Ok(s) => s,
        Err(None) => return Err(AuthExpired),
        Err(Some(panel)) => return Ok(panel),
    };

    let notice = |sweep: &Sweep, collection: &'static str| {
        sweep.partial.map(|reason| PartialNotice {
            shown: sweep.items.len(),
            total: sweep.total,
            reason,
            collection,
        })
    };
    let notices: Vec<PartialNotice> = [
        notice(&listeners, "listeners"),
        notice(&route_configs, "route configs"),
    ]
    .into_iter()
    .flatten()
    .collect();

    Ok(FiltersPanel::Inventory(build_inventory(
        &listeners.items,
        &route_configs.items,
        notices,
    )))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn listener(name: &str, filters: Value) -> Value {
        json!({"name": name, "spec": {"address": "0.0.0.0", "port": 8080, "http_filters": filters}})
    }

    #[test]
    fn chain_rows_render_domain_kinds_with_full_attribute_grids() {
        let filters = json!([
            {"filter": {"type": "global_rate_limit", "domain": "checkout",
                        "service_cluster": "rate_limit_cluster", "timeout_ms": 25,
                        "failure_mode_deny": true}},
            {"filter": {"type": "jwt_auth",
                        "providers": {"main": {"issuer": "https://idp.example.com",
                            "jwks": {"source": "remote", "uri": "https://idp.example.com/jwks",
                                     "cluster": "idp", "timeout_ms": 1000}}}},
             "disabled": true}
        ]);
        let inv = build_inventory(&[listener("edge", filters)], &[], Vec::new());
        // Two authored rows + synthesized router.
        assert_eq!(inv.rows.len(), 3);
        let grl = &inv.rows[0];
        assert_eq!(grl.kind, "global_rate_limit");
        assert!(!grl.disabled && !grl.synthesized);
        let attr = |rows: &Vec<(String, String)>, key: &str| {
            rows.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("attribute {key} missing"))
        };
        assert_eq!(attr(&grl.attrs, "domain"), "checkout");
        assert_eq!(attr(&grl.attrs, "timeout_ms"), "25");
        assert_eq!(attr(&grl.attrs, "failure_mode_deny"), "true");
        let jwt = &inv.rows[1];
        assert_eq!(jwt.kind, "jwt_auth");
        assert!(jwt.disabled, "disabled flag surfaces");
        assert!(attr(&jwt.attrs, "providers").contains("idp.example.com"));
        let router = &inv.rows[2];
        assert_eq!(router.kind, "router");
        assert!(router.synthesized, "router is synthesized, not stored");
        assert_eq!(
            inv.kinds_in_use,
            vec!["global_rate_limit".to_string(), "jwt_auth".to_string()],
            "footer lists authored kinds in use only (no router)"
        );
    }

    #[test]
    fn overrides_at_both_scopes_feed_rows_and_kinds() {
        let rc = json!({
            "name": "routes",
            "spec": {"virtual_hosts": [{
                "name": "vh-a", "domains": ["e.com"],
                "filter_overrides": [
                    {"type": "cors", "allow_origin": [{"match": "exact", "value": "https://a.example.com"}]}
                ],
                "routes": [{
                    "name": "r1", "match": {"prefix": {"prefix": "/"}},
                    "action": {"cluster": "c"},
                    "filter_overrides": [
                        {"type": "disable", "filter_type": "compressor"},
                        {"type": "jwt_auth", "requirement_name": "strict"}
                    ]
                }]
            }]}
        });
        let inv = build_inventory(&[], &[rc], Vec::new());
        assert_eq!(inv.overrides.len(), 3);
        assert_eq!(inv.overrides[0].scope, "vhost vh-a");
        assert_eq!(inv.overrides[0].kind, "cors");
        assert_eq!(inv.overrides[1].scope, "route vh-a/r1");
        // A Disable override renders under its TARGET domain kind, never "disable".
        assert_eq!(inv.overrides[1].kind, "compressor");
        assert!(inv.overrides[1]
            .attrs
            .iter()
            .any(|(k, v)| k == "filter_type" && v == "compressor"));
        assert_eq!(inv.overrides[2].kind, "jwt_auth");
        // Kinds: cors (vhost override), compressor (disable target), jwt_auth.
        assert_eq!(
            inv.kinds_in_use,
            vec![
                "compressor".to_string(),
                "cors".to_string(),
                "jwt_auth".to_string()
            ]
        );
    }

    #[test]
    fn long_attribute_values_truncate_with_true_length() {
        let jwks = "x".repeat(64 * 1024);
        let filters = json!([
            {"filter": {"type": "jwt_auth",
                "providers": {"inline": {"issuer": "https://idp.example.com",
                    "jwks": {"source": "inline", "jwks": jwks}}}}}
        ]);
        let inv = build_inventory(&[listener("edge", filters)], &[], Vec::new());
        let providers = &inv.rows[0]
            .attrs
            .iter()
            .find(|(k, _)| k == "providers")
            .expect("providers attr")
            .1;
        assert!(
            providers.chars().count() < 200,
            "grid value must be truncated, got {} chars",
            providers.chars().count()
        );
        assert!(
            providers.contains("chars)"),
            "truncation states the true length: {providers}"
        );
    }

    #[test]
    fn malformed_entries_render_visibly_and_do_not_erase_valid_rows() {
        // One junk filter entry among valid ones: valid rows + router survive, the
        // junk renders as an "(unparseable)" row.
        let filters = json!([
            {"filter": {"type": "health_check", "endpoint_path": "/healthz"}},
            {"filter": {"type": "not_a_kind", "x": 1}},
            {"filter": {"type": "ext_authz", "cluster": "authz"}}
        ]);
        let inv = build_inventory(&[listener("edge", filters)], &[], Vec::new());
        assert_eq!(inv.rows.len(), 4, "2 valid + 1 unparseable + router");
        assert_eq!(inv.rows[0].kind, "health_check");
        assert_eq!(inv.rows[1].kind, "(unparseable)");
        assert_eq!(inv.rows[2].kind, "ext_authz");
        assert!(inv.rows[3].synthesized);
        assert_eq!(
            inv.kinds_in_use,
            vec!["ext_authz".to_string(), "health_check".to_string()]
        );

        // One junk override next to a valid one in the SAME route config.
        let rc = json!({
            "name": "routes",
            "spec": {"virtual_hosts": [{
                "name": "vh", "domains": ["e.com"],
                "filter_overrides": [
                    {"type": "wat"},
                    {"type": "jwt_auth", "requirement_name": "strict"}
                ],
                "routes": []
            }]}
        });
        let inv = build_inventory(&[], &[rc], Vec::new());
        assert_eq!(inv.overrides.len(), 2);
        assert_eq!(inv.overrides[0].kind, "(unparseable)");
        assert_eq!(inv.overrides[1].kind, "jwt_auth");
    }

    #[test]
    fn override_grids_use_the_override_schema_not_the_chain_schema() {
        let rc = json!({
            "name": "routes",
            "spec": {"virtual_hosts": [{
                "name": "vh", "domains": ["e.com"],
                "filter_overrides": [
                    {"type": "jwt_auth", "requirement_name": "strict"},
                    {"type": "disable", "filter_type": "definitely-not-a-kind"}
                ],
                "routes": []
            }]}
        });
        let inv = build_inventory(&[], &[rc], Vec::new());
        assert_eq!(inv.overrides.len(), 2);
        let jwt = &inv.overrides[0];
        assert_eq!(jwt.kind, "jwt_auth");
        assert_eq!(
            jwt.attrs,
            vec![("requirement_name".to_string(), "strict".to_string())],
            "a jwt_auth OVERRIDE has only requirement_name — no chain-config fields"
        );
        // A disable naming an unknown target is malformed data: visible, not counted.
        let bad = &inv.overrides[1];
        assert_eq!(bad.kind, "(unparseable)");
        assert!(bad
            .attrs
            .iter()
            .any(|(k, v)| k == "filter_type" && v == "definitely-not-a-kind"));
        assert!(
            inv.kinds_in_use == vec!["jwt_auth".to_string()],
            "the bad target must not enter kinds in use: {:?}",
            inv.kinds_in_use
        );
    }

    #[test]
    fn full_grid_shows_default_placeholders_for_omitted_fields() {
        // A minimal global_rate_limit: serialization omits defaulted fields; the grid
        // must still list them with "(default)" so unset is distinguishable.
        let filters = json!([
            {"filter": {"type": "global_rate_limit", "domain": "d",
                        "service_cluster": "rate_limit_cluster"}}
        ]);
        let inv = build_inventory(&[listener("edge", filters)], &[], Vec::new());
        let attrs = &inv.rows[0].attrs;
        let value = |key: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("field {key} missing from full grid"))
        };
        assert_eq!(value("domain"), "d");
        assert_eq!(value("stat_prefix"), "(default)");
        assert_eq!(value("enable_x_ratelimit_headers"), "(default)");
        assert_eq!(value("rate_limited_status"), "(default)");
        // request_type has skip_serializing_if on the default → placeholder too.
        assert_eq!(value("request_type"), "(default)");
    }
}
