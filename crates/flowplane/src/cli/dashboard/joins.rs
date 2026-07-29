//! Topology join layer (fpv2-cxw.2): client-side joins over the swept gateway specs,
//! in the dashboard process (never the browser).
//!
//! Chain: `ListenerSpec.route_config` (name, same team) → `RouteConfigSpec.
//! virtual_hosts[].routes[]` → `RouteAction.cluster` / `weighted_clusters` →
//! `ClusterSpec`. Dangling references render as "unresolved reference" — never
//! silently dropped. The rendered topology is a best-effort, NON-transactional
//! snapshot (pages fetched sequentially can interleave with writes); past the node
//! budget it degrades to the tables view with an explicit notice.

use serde_json::Value;
use std::collections::BTreeMap;

use fp_domain::gateway::listener::ListenerSpec;
use fp_domain::gateway::route_config::{RouteAction, RouteConfigSpec};
use fp_domain::gateway::ClusterSpec;

/// Rendered-node ceiling (design "Scale guardrails": ~10,000 rendered nodes, measured,
/// adjustable). Nodes = listeners + resolved route configs + vhosts + routes + cluster
/// targets. Beyond it the topology partial renders the tables instead, with a notice.
pub(super) const MAX_TOPOLOGY_NODES: usize = 10_000;

pub(super) struct ClusterChip {
    pub(super) endpoints: usize,
    pub(super) tls: bool,
    pub(super) lb_policy: String,
}

/// One route-action target. `chip: None` means the named cluster was not found in the
/// swept clusters — an unresolved reference (rendered as such).
pub(super) struct ClusterTarget {
    pub(super) name: String,
    /// Weighted-cluster weight; None for a plain single-cluster action.
    pub(super) weight: Option<u32>,
    pub(super) chip: Option<ClusterChip>,
}

pub(super) struct RouteNode {
    pub(super) name: String,
    /// "cluster" | "weighted" | "redirect" | "direct" | "none".
    pub(super) action: &'static str,
    pub(super) targets: Vec<ClusterTarget>,
}

pub(super) struct VhostNode {
    pub(super) name: String,
    pub(super) domains: Vec<String>,
    pub(super) routes: Vec<RouteNode>,
}

pub(super) struct RouteConfigNode {
    pub(super) name: String,
    pub(super) vhosts: Vec<VhostNode>,
}

/// A listener's route-config edge: bound + resolved, bound + dangling, or unbound.
pub(super) enum RouteConfigEdge {
    Resolved(RouteConfigNode),
    /// The listener names a route config the sweep did not return.
    Unresolved(String),
    Unbound,
}

pub(super) struct ListenerChain {
    pub(super) name: String,
    pub(super) bind: String,
    pub(super) route_config: RouteConfigEdge,
    /// True when the listener item's spec failed typed decode (version skew): the
    /// chain renders an "unparseable spec" marker instead of disappearing.
    pub(super) unparsed: bool,
}

pub(super) struct Topology {
    pub(super) chains: Vec<ListenerChain>,
    /// Rendered-node count (listeners + resolved rcs + vhosts + routes + targets).
    pub(super) nodes: usize,
    /// True when `nodes` crossed [`MAX_TOPOLOGY_NODES`] — the caller renders tables.
    pub(super) degraded: bool,
}

fn typed<S: serde::de::DeserializeOwned>(items: &[Value]) -> Vec<(String, S)> {
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let spec = serde_json::from_value::<S>(item.get("spec")?.clone()).ok()?;
            Some((name, spec))
        })
        .collect()
}

fn enum_str<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => "?".into(),
    }
}

fn chip(spec: &ClusterSpec) -> ClusterChip {
    ClusterChip {
        endpoints: spec.endpoints.len(),
        tls: spec.use_tls || spec.upstream_tls.is_some(),
        lb_policy: enum_str(&spec.lb_policy),
    }
}

fn targets(action: &RouteAction, clusters: &BTreeMap<&str, &ClusterSpec>) -> RouteNode {
    let resolve = |name: &str, weight: Option<u32>| ClusterTarget {
        name: name.to_string(),
        weight,
        chip: clusters.get(name).map(|spec| chip(spec)),
    };
    if let Some(cluster) = &action.cluster {
        RouteNode {
            name: String::new(),
            action: "cluster",
            targets: vec![resolve(cluster, None)],
        }
    } else if let Some(weighted) = &action.weighted_clusters {
        RouteNode {
            name: String::new(),
            action: "weighted",
            targets: weighted
                .iter()
                .map(|w| resolve(&w.cluster, Some(w.weight)))
                .collect(),
        }
    } else if action.redirect.is_some() {
        RouteNode {
            name: String::new(),
            action: "redirect",
            targets: Vec::new(),
        }
    } else if action.direct_response.is_some() {
        RouteNode {
            name: String::new(),
            action: "direct",
            targets: Vec::new(),
        }
    } else {
        RouteNode {
            name: String::new(),
            action: "none",
            targets: Vec::new(),
        }
    }
}

/// Build the topology from the three sweeps' raw items. Items whose spec fails typed
/// decode are excluded from joins (their names then surface as unresolved references
/// wherever something points at them — visible, not hidden).
pub(super) fn build_topology(
    listeners: &[Value],
    route_configs: &[Value],
    clusters: &[Value],
) -> Topology {
    // Listeners are the chain ROOTS: an undecodable listener would otherwise vanish
    // (nothing points at it), so decode failures are kept as unparsed chains. For
    // route configs and clusters a decode failure already stays visible — whatever
    // references the name renders it as an unresolved reference.
    let listeners: Vec<(String, Option<ListenerSpec>)> = listeners
        .iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string();
            let spec = item
                .get("spec")
                .and_then(|spec| serde_json::from_value::<ListenerSpec>(spec.clone()).ok());
            (name, spec)
        })
        .collect();
    let route_configs: Vec<(String, RouteConfigSpec)> = typed(route_configs);
    let clusters: Vec<(String, ClusterSpec)> = typed(clusters);

    let rc_by_name: BTreeMap<&str, &RouteConfigSpec> = route_configs
        .iter()
        .map(|(name, spec)| (name.as_str(), spec))
        .collect();
    let cluster_by_name: BTreeMap<&str, &ClusterSpec> = clusters
        .iter()
        .map(|(name, spec)| (name.as_str(), spec))
        .collect();

    let mut nodes = 0usize;
    let mut chains = Vec::with_capacity(listeners.len());
    for (name, spec) in &listeners {
        nodes += 1;
        let Some(spec) = spec else {
            chains.push(ListenerChain {
                name: name.clone(),
                bind: "?".into(),
                route_config: RouteConfigEdge::Unbound,
                unparsed: true,
            });
            continue;
        };
        let route_config = match &spec.route_config {
            None => RouteConfigEdge::Unbound,
            Some(rc_name) => match rc_by_name.get(rc_name.as_str()) {
                None => RouteConfigEdge::Unresolved(rc_name.clone()),
                Some(rc) => {
                    nodes += 1;
                    let vhosts = rc
                        .virtual_hosts
                        .iter()
                        .map(|vh| {
                            nodes += 1;
                            VhostNode {
                                name: vh.name.clone(),
                                domains: vh.domains.clone(),
                                routes: vh
                                    .routes
                                    .iter()
                                    .map(|route| {
                                        nodes += 1;
                                        let mut node = targets(&route.action, &cluster_by_name);
                                        nodes += node.targets.len();
                                        node.name = route.name.clone();
                                        node
                                    })
                                    .collect(),
                            }
                        })
                        .collect();
                    RouteConfigEdge::Resolved(RouteConfigNode {
                        name: rc_name.clone(),
                        vhosts,
                    })
                }
            },
        };
        chains.push(ListenerChain {
            name: name.clone(),
            bind: format!("{}:{}", spec.address, spec.port),
            route_config,
            unparsed: false,
        });
    }

    Topology {
        chains,
        nodes,
        degraded: nodes > MAX_TOPOLOGY_NODES,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn listener(name: &str, rc: Option<&str>) -> Value {
        json!({
            "name": name,
            "spec": {
                "address": "0.0.0.0", "port": 8080,
                "route_config": rc
            }
        })
    }

    fn cluster(name: &str, endpoints: usize, tls: bool) -> Value {
        let eps: Vec<Value> = (0..endpoints)
            .map(|i| json!({"host": format!("10.0.0.{i}"), "port": 8080}))
            .collect();
        json!({"name": name, "spec": {"endpoints": eps, "use_tls": tls, "lb_policy": "least-request"}})
    }

    fn rc_single(name: &str, route: &str, cluster: &str) -> Value {
        json!({
            "name": name,
            "spec": {"virtual_hosts": [{"name": "vh", "domains": ["example.com"], "routes": [
                {"name": route, "match": {"prefix": {"prefix": "/"}}, "action": {"cluster": cluster}}
            ]}]}
        })
    }

    #[test]
    fn resolved_chain_with_cluster_chip() {
        let topo = build_topology(
            &[listener("edge", Some("routes"))],
            &[rc_single("routes", "r1", "checkout")],
            &[cluster("checkout", 3, true)],
        );
        assert_eq!(topo.chains.len(), 1);
        let RouteConfigEdge::Resolved(rc) = &topo.chains[0].route_config else {
            panic!("route config must resolve");
        };
        assert_eq!(rc.name, "routes");
        let target = &rc.vhosts[0].routes[0].targets[0];
        assert_eq!(target.name, "checkout");
        let chip = target.chip.as_ref().expect("cluster chip resolves");
        assert_eq!(chip.endpoints, 3);
        assert!(chip.tls);
        assert_eq!(chip.lb_policy, "least-request");
        // Nodes: listener + rc + vhost + route + target = 5.
        assert_eq!(topo.nodes, 5);
        assert!(!topo.degraded);
    }

    #[test]
    fn weighted_cluster_fan_out_resolves_each_target() {
        let rc = json!({
            "name": "routes",
            "spec": {"virtual_hosts": [{"name": "vh", "domains": ["e.com"], "routes": [
                {"name": "split", "match": {"prefix": {"prefix": "/"}},
                 "action": {"weighted_clusters": [
                    {"cluster": "blue", "weight": 90},
                    {"cluster": "green", "weight": 10}
                 ]}}
            ]}]}
        });
        let topo = build_topology(
            &[listener("edge", Some("routes"))],
            &[rc],
            &[cluster("blue", 2, false), cluster("green", 1, false)],
        );
        let RouteConfigEdge::Resolved(rc) = &topo.chains[0].route_config else {
            panic!("resolve");
        };
        let route = &rc.vhosts[0].routes[0];
        assert_eq!(route.action, "weighted");
        assert_eq!(route.targets.len(), 2);
        assert_eq!(route.targets[0].weight, Some(90));
        assert!(route.targets[0].chip.is_some());
        assert_eq!(route.targets[1].name, "green");
    }

    #[test]
    fn dangling_references_surface_as_unresolved_never_dropped() {
        // Listener → missing route config.
        let topo = build_topology(&[listener("edge", Some("ghost"))], &[], &[]);
        match &topo.chains[0].route_config {
            RouteConfigEdge::Unresolved(name) => assert_eq!(name, "ghost"),
            _ => panic!("dangling route config must render as unresolved"),
        }
        // Route → missing cluster: target present, chip absent.
        let topo = build_topology(
            &[listener("edge", Some("routes"))],
            &[rc_single("routes", "r1", "ghost-cluster")],
            &[],
        );
        let RouteConfigEdge::Resolved(rc) = &topo.chains[0].route_config else {
            panic!("resolve");
        };
        let target = &rc.vhosts[0].routes[0].targets[0];
        assert_eq!(target.name, "ghost-cluster");
        assert!(target.chip.is_none(), "unresolved cluster keeps its name");
    }

    #[test]
    fn unbound_listener_and_redirect_direct_actions() {
        let rc = json!({
            "name": "routes",
            "spec": {"virtual_hosts": [{"name": "vh", "domains": ["e.com"], "routes": [
                {"name": "jump", "match": {"prefix": {"prefix": "/old"}},
                 "action": {"redirect": {"host_redirect": "new.example.com"}}},
                {"name": "static", "match": {"prefix": {"prefix": "/ping"}},
                 "action": {"direct_response": {"status": 200, "body": "ok"}}}
            ]}]}
        });
        let topo = build_topology(
            &[listener("edge", Some("routes")), listener("spare", None)],
            &[rc],
            &[],
        );
        assert!(matches!(
            topo.chains[1].route_config,
            RouteConfigEdge::Unbound
        ));
        let RouteConfigEdge::Resolved(rc) = &topo.chains[0].route_config else {
            panic!("resolve");
        };
        assert_eq!(rc.vhosts[0].routes[0].action, "redirect");
        assert_eq!(rc.vhosts[0].routes[1].action, "direct");
        assert!(rc.vhosts[0].routes[0].targets.is_empty());
    }

    #[test]
    fn node_budget_marks_degraded() {
        // One rc with 50 vhosts × 200 routes = 10,000 routes (the legal single-config
        // maximum) → nodes exceed MAX_TOPOLOGY_NODES.
        let vhosts: Vec<Value> = (0..50)
            .map(|v| {
                let routes: Vec<Value> = (0..200)
                    .map(|r| {
                        json!({"name": format!("r-{v}-{r}"),
                               "match": {"prefix": {"prefix": format!("/{v}/{r}")}},
                               "action": {"cluster": "c"}})
                    })
                    .collect();
                json!({"name": format!("vh-{v}"), "domains": [format!("v{v}.e.com")], "routes": routes})
            })
            .collect();
        let rc = json!({"name": "big", "spec": {"virtual_hosts": vhosts}});
        let topo = build_topology(
            &[listener("edge", Some("big"))],
            &[rc],
            &[cluster("c", 1, false)],
        );
        assert!(topo.nodes > MAX_TOPOLOGY_NODES, "nodes: {}", topo.nodes);
        assert!(topo.degraded);
    }

    #[test]
    fn undecodable_spec_is_excluded_from_joins_and_referenced_names_stay_visible() {
        // The cluster item is junk → route target renders unresolved (visible).
        let junk_cluster = json!({"name": "checkout", "spec": {"nonsense": true}});
        let topo = build_topology(
            &[listener("edge", Some("routes"))],
            &[rc_single("routes", "r1", "checkout")],
            &[junk_cluster],
        );
        let RouteConfigEdge::Resolved(rc) = &topo.chains[0].route_config else {
            panic!("resolve");
        };
        assert!(rc.vhosts[0].routes[0].targets[0].chip.is_none());
    }

    #[test]
    fn undecodable_listener_renders_as_unparsed_chain_not_nothing() {
        let junk_listener = json!({"name": "mystery-edge", "spec": {"garbage": 1}});
        let topo = build_topology(&[junk_listener], &[], &[]);
        assert_eq!(topo.chains.len(), 1, "the chain root must stay visible");
        assert!(topo.chains[0].unparsed);
        assert_eq!(topo.chains[0].name, "mystery-edge");
    }
}
