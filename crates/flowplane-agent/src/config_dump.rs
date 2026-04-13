//! Envoy admin `/config_dump` fetching and parsing.
//!
//! The Envoy admin `/config_dump` endpoint returns JSON that is the protojson
//! serialization of `envoy.admin.v3.ConfigDump`. The generated prost bindings
//! in `envoy-types` do not derive `serde`, so this module defines a **minimal
//! serde mirror** of the admin proto — just the fields needed for warming
//! failure extraction. Field names are the stable proto field names; unknown
//! fields are ignored via `#[serde(other)]` on the enum and `#[serde(default)]`
//! on every optional field, so new Envoy minors adding fields will NOT break
//! parsing.
//!
//! If Envoy ever renames or removes one of the fields below, the
//! source-of-truth is the admin proto — update this mirror to match.

use serde::Deserialize;

/// Which xDS resource kind an extracted error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Listener,
    Cluster,
    RouteConfig,
}

impl ResourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::Listener => "listener",
            ResourceKind::Cluster => "cluster",
            ResourceKind::RouteConfig => "route_config",
        }
    }
}

/// A normalized view of a single `error_state` entry from config_dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorEntry {
    pub kind: ResourceKind,
    pub name: String,
    pub details: String,
}

/// Minimal mirror of `envoy.admin.v3.ConfigDump`.
#[derive(Debug, Deserialize, Default)]
pub struct ConfigDump {
    #[serde(default)]
    pub configs: Vec<ConfigDumpEntry>,
}

/// Each element of `ConfigDump.configs` is an `Any` whose `@type` URL tells
/// us which concrete config-dump variant it carries.
#[derive(Debug, Deserialize)]
#[serde(tag = "@type")]
pub enum ConfigDumpEntry {
    #[serde(rename = "type.googleapis.com/envoy.admin.v3.ListenersConfigDump")]
    Listeners {
        #[serde(default)]
        dynamic_listeners: Vec<DynamicResource>,
    },
    #[serde(rename = "type.googleapis.com/envoy.admin.v3.ClustersConfigDump")]
    Clusters {
        #[serde(default)]
        dynamic_active_clusters: Vec<DynamicResource>,
        #[serde(default)]
        dynamic_warming_clusters: Vec<DynamicResource>,
    },
    #[serde(rename = "type.googleapis.com/envoy.admin.v3.RoutesConfigDump")]
    Routes {
        #[serde(default)]
        dynamic_route_configs: Vec<DynamicResource>,
    },
    /// Catch-all for BootstrapConfigDump, SecretsConfigDump, EcdsConfigDump,
    /// ScopedRoutesConfigDump, EndpointsConfigDump, and any future variants.
    /// MUST be a unit variant for serde's `#[serde(other)]` on internally
    /// tagged enums.
    #[serde(other)]
    Other,
}

/// Serde mirror of the per-resource dynamic state entry. The admin proto has
/// slightly different shapes for `DynamicListener`, `DynamicCluster`,
/// `DynamicRouteConfig`, but the fields we care about are compatible:
///
///   * `DynamicListener` has a top-level `name` string and `error_state`.
///   * `DynamicCluster` has `cluster` (an Any carrying the Cluster proto,
///     whose JSON form has a `name` field) plus `error_state`.
///   * `DynamicRouteConfig` has `route_config` (an Any with a `name` field)
///     plus `error_state`.
///
/// We deserialize all three into this single struct and use `resolved_name()`
/// to prefer the top-level `name` but fall back to the nested `cluster.name`
/// or `route_config.name` if missing.
#[derive(Debug, Deserialize, Default)]
pub struct DynamicResource {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub error_state: Option<UpdateFailureState>,
    #[serde(default)]
    pub cluster: Option<serde_json::Value>,
    #[serde(default)]
    pub route_config: Option<serde_json::Value>,
}

impl DynamicResource {
    pub fn resolved_name(&self) -> Option<String> {
        if let Some(n) = &self.name {
            if !n.is_empty() {
                return Some(n.clone());
            }
        }
        if let Some(v) = &self.cluster {
            if let Some(s) = nested_name(v, "cluster") {
                return Some(s);
            }
        }
        if let Some(v) = &self.route_config {
            if let Some(s) = nested_name(v, "route_config") {
                return Some(s);
            }
        }
        None
    }
}

/// Serde mirror of `envoy.admin.v3.UpdateFailureState`. We only read `details`
/// for error extraction; `last_update_attempt` and `failed_configuration` are
/// intentionally NOT parsed to a typed form — the former is excluded from the
/// dedup hash by design (so Envoy's own retry cadence doesn't bust dedup),
/// and the latter would bloat the agent's dep tree for no MVP benefit.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateFailureState {
    #[serde(default)]
    pub details: String,
}

/// Parse config_dump JSON into the minimal mirror struct.
pub fn parse_config_dump(body: &str) -> Result<ConfigDump, serde_json::Error> {
    serde_json::from_str(body)
}

/// Recursively look for a `name` field on a serde Value. The Envoy admin
/// `/config_dump` for clusters/routes wraps the actual resource in an Any,
/// and some shapes nest twice (e.g. `cluster.cluster.name`) — the wrapper Any
/// for `DynamicCluster` plus the Any for the inner `Cluster` proto. Try the
/// direct `name`, then descend into a same-named child key, then bail out.
fn nested_name(v: &serde_json::Value, child_key: &str) -> Option<String> {
    if let Some(s) = v.get("name").and_then(|x| x.as_str()) {
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Some(inner) = v.get(child_key) {
        if let Some(s) = inner.get("name").and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Fallback name when a dynamic resource entry has an `error_state` but no
/// resolvable identifier. The spec requires every `error_state` to be
/// reported — dropping them because Envoy's own Any was sparse would hide
/// failures. The synthesized marker is stable so dedup still works.
const UNNAMED: &str = "<unnamed>";

fn name_or_fallback(dr: &DynamicResource) -> String {
    dr.resolved_name().unwrap_or_else(|| UNNAMED.to_string())
}

/// Walk a parsed config_dump and extract every resource with a non-empty
/// `error_state`. Every `error_state` is reported — if a dynamic resource
/// entry has no resolvable name, the report uses a synthetic `<unnamed>`
/// marker rather than being dropped.
pub fn extract_error_entries(dump: &ConfigDump) -> Vec<ErrorEntry> {
    let mut out = Vec::new();
    for cfg in &dump.configs {
        match cfg {
            ConfigDumpEntry::Listeners { dynamic_listeners } => {
                for l in dynamic_listeners {
                    if let Some(err) = &l.error_state {
                        out.push(ErrorEntry {
                            kind: ResourceKind::Listener,
                            name: name_or_fallback(l),
                            details: err.details.clone(),
                        });
                    }
                }
            }
            ConfigDumpEntry::Clusters { dynamic_active_clusters, dynamic_warming_clusters } => {
                for c in dynamic_active_clusters.iter().chain(dynamic_warming_clusters.iter()) {
                    if let Some(err) = &c.error_state {
                        out.push(ErrorEntry {
                            kind: ResourceKind::Cluster,
                            name: name_or_fallback(c),
                            details: err.details.clone(),
                        });
                    }
                }
            }
            ConfigDumpEntry::Routes { dynamic_route_configs } => {
                for r in dynamic_route_configs {
                    if let Some(err) = &r.error_state {
                        out.push(ErrorEntry {
                            kind: ResourceKind::RouteConfig,
                            name: name_or_fallback(r),
                            details: err.details.clone(),
                        });
                    }
                }
            }
            ConfigDumpEntry::Other => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP_EMPTY: &str = r#"{ "configs": [] }"#;

    const DUMP_LISTENER_NO_ERROR: &str = r#"{
      "configs": [
        {
          "@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump",
          "dynamic_listeners": [
            { "name": "good-listener", "active_state": { "version_info": "1" } }
          ]
        }
      ]
    }"#;

    const DUMP_LISTENER_WITH_ERROR: &str = r#"{
      "configs": [
        {
          "@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump",
          "dynamic_listeners": [
            {
              "name": "busted-listener",
              "error_state": {
                "details": "Proto constraint validation failed (ListenerValidationError.FilterChains[0].Filters[0]: [embedded message failed validation] | caused by FilterValidationError.Name: value length must be at least 1 runes)",
                "last_update_attempt": "2026-04-13T12:34:56.789Z",
                "failed_configuration": { "@type": "type.googleapis.com/envoy.config.listener.v3.Listener", "name": "busted-listener" }
              }
            },
            { "name": "good-listener", "active_state": { "version_info": "2" } }
          ]
        }
      ]
    }"#;

    const DUMP_MIXED_CATEGORIES: &str = r#"{
      "configs": [
        { "@type": "type.googleapis.com/envoy.admin.v3.BootstrapConfigDump", "bootstrap": {} },
        {
          "@type": "type.googleapis.com/envoy.admin.v3.ClustersConfigDump",
          "dynamic_active_clusters": [
            {
              "cluster": { "@type": "type.googleapis.com/envoy.config.cluster.v3.Cluster", "name": "my-cluster" },
              "error_state": { "details": "bad cluster" }
            }
          ],
          "dynamic_warming_clusters": []
        },
        {
          "@type": "type.googleapis.com/envoy.admin.v3.RoutesConfigDump",
          "dynamic_route_configs": [
            {
              "route_config": { "@type": "type.googleapis.com/envoy.config.route.v3.RouteConfiguration", "name": "my-routes" },
              "error_state": { "details": "bad route config" }
            }
          ]
        },
        {
          "@type": "type.googleapis.com/envoy.admin.v3.SecretsConfigDump",
          "dynamic_active_secrets": []
        }
      ]
    }"#;

    #[test]
    fn parses_empty_dump() {
        let dump = parse_config_dump(DUMP_EMPTY).expect("parse");
        assert!(extract_error_entries(&dump).is_empty());
    }

    #[test]
    fn ignores_listeners_without_error_state() {
        let dump = parse_config_dump(DUMP_LISTENER_NO_ERROR).expect("parse");
        assert!(extract_error_entries(&dump).is_empty());
    }

    #[test]
    fn extracts_listener_error_state_with_exact_details() {
        let dump = parse_config_dump(DUMP_LISTENER_WITH_ERROR).expect("parse");
        let entries = extract_error_entries(&dump);
        assert_eq!(entries.len(), 1, "should extract exactly one error entry");
        let e = &entries[0];
        assert_eq!(e.kind, ResourceKind::Listener);
        assert_eq!(e.name, "busted-listener");
        assert!(e.details.contains("Proto constraint validation failed"));
        // The `good-listener` has no error_state and must NOT appear.
        assert!(entries.iter().all(|e| e.name != "good-listener"));
    }

    #[test]
    fn extracts_cluster_and_route_names_from_nested_any() {
        let dump = parse_config_dump(DUMP_MIXED_CATEGORIES).expect("parse");
        let entries = extract_error_entries(&dump);
        assert_eq!(entries.len(), 2);
        let cluster = entries.iter().find(|e| e.kind == ResourceKind::Cluster).expect("cluster");
        assert_eq!(cluster.name, "my-cluster");
        assert_eq!(cluster.details, "bad cluster");
        let route = entries.iter().find(|e| e.kind == ResourceKind::RouteConfig).expect("route");
        assert_eq!(route.name, "my-routes");
        assert_eq!(route.details, "bad route config");
    }

    #[test]
    fn unknown_config_variants_are_ignored_not_errors() {
        // Bootstrap + Secrets variants in DUMP_MIXED_CATEGORIES are not in
        // our enum; they hit the `#[serde(other)]` arm and should parse fine.
        let dump = parse_config_dump(DUMP_MIXED_CATEGORIES);
        assert!(dump.is_ok(), "parse failed: {:?}", dump.err());
    }

    #[test]
    fn unicode_in_error_details_is_preserved_verbatim() {
        let body = r#"{
          "configs": [{
            "@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump",
            "dynamic_listeners": [{
              "name": "uni",
              "error_state": { "details": "héllo 🌍 — bad" }
            }]
          }]
        }"#;
        let dump = parse_config_dump(body).expect("parse");
        let entries = extract_error_entries(&dump);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].details, "héllo 🌍 — bad");
    }

    #[test]
    fn listener_without_resolvable_name_is_reported_with_fallback() {
        // Spec: every error_state must be reported, even if the dynamic
        // resource entry has no resolvable name. The report uses a
        // synthetic `<unnamed>` marker so the error is not lost.
        let body = r#"{
          "configs": [{
            "@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump",
            "dynamic_listeners": [{ "error_state": { "details": "nameless failure" } }]
          }]
        }"#;
        let dump = parse_config_dump(body).expect("parse");
        let entries = extract_error_entries(&dump);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "<unnamed>");
        assert_eq!(entries[0].details, "nameless failure");
    }

    #[test]
    fn cluster_without_nested_any_is_reported_with_fallback() {
        // DynamicCluster proto has no top-level `name` — the name lives
        // inside the nested `cluster` Any. If a test fixture omits the
        // Any entirely, we must still report the error_state.
        let body = r#"{
          "configs": [{
            "@type": "type.googleapis.com/envoy.admin.v3.ClustersConfigDump",
            "dynamic_active_clusters": [{ "error_state": { "details": "bad cluster" } }]
          }]
        }"#;
        let dump = parse_config_dump(body).expect("parse");
        let entries = extract_error_entries(&dump);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, ResourceKind::Cluster);
        assert_eq!(entries[0].name, "<unnamed>");
        assert_eq!(entries[0].details, "bad cluster");
    }
}
