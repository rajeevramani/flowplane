//! Orphans panel (fpv2-cxw.6): the four orphan kinds, computed ONLY from complete
//! sweeps.
//!
//! Suppression rule (approved design, "Bounded acquisition contract"): orphan
//! analysis is never computed from a partial sweep — a missing page must not
//! manufacture false "unreferenced" findings. Any contributing collection that is
//! partial (budget/mid-sweep failure), unauthorized, unavailable, or contains items
//! whose specs fail typed decode (an unseen spec could hold the very reference that
//! would clear an orphan) suppresses the whole analysis with an explanation.
//!
//! Orphan kinds (design "Joins" — orphans bullet, AC 3):
//! 1. clusters unreferenced by any route action or aggregate-cluster list — extended
//!    conservatively with filter-level cluster references (jwt remote JWKS cluster,
//!    ext_authz cluster, user-supplied global_rate_limit service_cluster): those ARE
//!    references in the domain, and counting them can only prevent false claims;
//! 2. listeners with `route_config: None`;
//! 3. unattached rate-limit domains (built-in limiter attachment per the S4
//!    normalization contract; external limiters excluded from claims);
//! 4. secrets unreferenced under either used-by key (S3 contract: SDS refs by name,
//!    AI-provider credential by id).

use serde_json::Value;
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::super::client::RestClient;
use super::data::AuthExpired;
use super::ratelimits::attachment_map;
use super::resources::{sweep, Sweep, SweepFailure, SWEEP_BYTE_BUDGET};
use super::secrets_panel::build_references;
use fp_domain::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER;
use fp_domain::gateway::filters::{HttpFilterSpec, JwksSource};
use fp_domain::gateway::listener::ListenerSpec;
use fp_domain::gateway::route_config::RouteConfigSpec;
use fp_domain::gateway::ClusterSpec;

pub(super) struct OrphanFindings {
    pub(super) unreferenced_clusters: Vec<String>,
    pub(super) unbound_listeners: Vec<String>,
    pub(super) unattached_domains: Vec<String>,
    pub(super) unreferenced_secrets: Vec<String>,
}

impl OrphanFindings {
    pub(super) fn is_empty(&self) -> bool {
        self.unreferenced_clusters.is_empty()
            && self.unbound_listeners.is_empty()
            && self.unattached_domains.is_empty()
            && self.unreferenced_secrets.is_empty()
    }
}

pub(super) enum OrphansPanel {
    Findings(OrphanFindings),
    /// Analysis withheld; the reason names what was incomplete.
    Suppressed {
        reason: String,
    },
}

/// Typed items of one collection, or the reason the collection cannot support
/// orphan analysis.
fn typed_or_reason<S: serde::de::DeserializeOwned>(
    sweep: &Sweep,
    collection: &str,
) -> Result<Vec<(String, S)>, String> {
    if sweep.partial.is_some() {
        return Err(format!(
            "the {collection} sweep is partial — orphan analysis is never computed from a partial sweep"
        ));
    }
    let mut out = Vec::with_capacity(sweep.items.len());
    for item in &sweep.items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)")
            .to_string();
        match item
            .get("spec")
            .and_then(|s| serde_json::from_value::<S>(s.clone()).ok())
        {
            Some(spec) => out.push((name, spec)),
            None => {
                return Err(format!(
                    "a {collection} item ({name}) has an unparseable spec — its references cannot be proven"
                ))
            }
        }
    }
    Ok(out)
}

/// Compute the four orphan kinds from COMPLETE, fully-decoded collections.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_findings(
    listeners: &[(String, ListenerSpec)],
    route_configs: &[(String, RouteConfigSpec)],
    clusters: &[(String, ClusterSpec)],
    domain_names: &[String],
    listener_items_raw: &[Value],
    cluster_items_raw: &[Value],
    ai_provider_items_raw: &[Value],
    secret_items_raw: &[Value],
) -> OrphanFindings {
    // 1. Cluster references: route actions, aggregate lists, filter-level refs.
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for (_, rc) in route_configs {
        for vh in &rc.virtual_hosts {
            for route in &vh.routes {
                if let Some(cluster) = &route.action.cluster {
                    referenced.insert(cluster);
                }
                if let Some(weighted) = &route.action.weighted_clusters {
                    for target in weighted {
                        referenced.insert(&target.cluster);
                    }
                }
            }
        }
    }
    for (_, spec) in clusters {
        for member in &spec.aggregate_clusters {
            referenced.insert(member);
        }
    }
    for (_, spec) in listeners {
        for entry in &spec.http_filters {
            match &entry.filter {
                HttpFilterSpec::ExtAuthz(cfg) => {
                    referenced.insert(&cfg.cluster);
                }
                HttpFilterSpec::GlobalRateLimit(cfg) => {
                    if cfg.service_cluster != RESERVED_RATE_LIMIT_CLUSTER {
                        referenced.insert(&cfg.service_cluster);
                    }
                }
                HttpFilterSpec::JwtAuth(cfg) => {
                    for provider in cfg.providers.values() {
                        if let JwksSource::Remote { cluster, .. } = &provider.jwks {
                            referenced.insert(cluster);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let unreferenced_clusters = clusters
        .iter()
        .filter(|(name, _)| !referenced.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();

    // 2. Unbound listeners.
    let unbound_listeners = listeners
        .iter()
        .filter(|(_, spec)| spec.route_config.is_none())
        .map(|(name, _)| name.clone())
        .collect();

    // 3. Unattached rate-limit domains (built-in limiter only; S4 normalization).
    let attachments = attachment_map(listener_items_raw);
    let unattached_domains = domain_names
        .iter()
        .filter(|name| !attachments.contains_key(*name))
        .cloned()
        .collect();

    // 4. Unreferenced secrets (S3 contract: neither name nor id key matches).
    let refs = build_references(listener_items_raw, cluster_items_raw, ai_provider_items_raw);
    let unreferenced_secrets = secret_items_raw
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?;
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let referenced = refs.by_name.contains_key(name) || refs.by_id.contains_key(&id);
            (!referenced).then(|| name.to_string())
        })
        .collect();

    OrphanFindings {
        unreferenced_clusters,
        unbound_listeners,
        unattached_domains,
        unreferenced_secrets,
    }
}

/// Fail-safe envelope validation for the metadata collections: every domain and
/// secret must carry a readable `name` (secrets also an `id`), and every AI provider
/// a UUID-parseable `spec.credential_secret_id`. A provider whose credential id
/// cannot be read would otherwise silently drop the one reference proving its
/// secret is in use — a manufactured "unreferenced secret".
fn validate_join_fields(domains: &Sweep, secrets: &Sweep, ai_providers: &Sweep) -> Option<String> {
    // Name the offending item with whatever identifying field survives (name, else
    // id, else a stable placeholder) so the suppression reason is actionable.
    let identify = |item: &Value| {
        item.get("name")
            .and_then(Value::as_str)
            .or_else(|| item.get("id").and_then(Value::as_str))
            .unwrap_or("(unidentifiable)")
            .to_string()
    };
    for item in &domains.items {
        if item.get("name").and_then(Value::as_str).is_none() {
            return Some(format!(
                "a rate-limit-domain item ({}) has no readable name — orphan analysis cannot be proven",
                identify(item)
            ));
        }
    }
    for item in &secrets.items {
        if item.get("name").and_then(Value::as_str).is_none()
            || item.get("id").and_then(Value::as_str).is_none()
        {
            return Some(format!(
                "a secret item ({}) has no readable name/id — orphan analysis cannot be proven",
                identify(item)
            ));
        }
    }
    for item in &ai_providers.items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)");
        let credential = item
            .get("spec")
            .and_then(|s| s.get("credential_secret_id"))
            .and_then(Value::as_str)
            .and_then(|id| uuid::Uuid::parse_str(id).ok());
        if credential.is_none() {
            return Some(format!(
                "AI provider {name} has no readable credential_secret_id — its secret reference cannot be proven"
            ));
        }
    }
    None
}

/// Sweep all six contributing collections; any incompleteness suppresses analysis.
pub(super) async fn fetch_orphans(
    client: &RestClient,
    team: &str,
    _now: DateTime<Utc>,
) -> Result<OrphansPanel, AuthExpired> {
    let mut sweeps: Vec<(&'static str, Sweep)> = Vec::with_capacity(6);
    for collection in [
        "listeners",
        "route-configs",
        "clusters",
        "rate-limit-domains",
        "secrets",
        "ai/providers",
    ] {
        match sweep(client, team, collection, SWEEP_BYTE_BUDGET).await {
            Ok(s) => sweeps.push((collection, s)),
            Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
            Err(SweepFailure::Unauthorized) => {
                return Ok(OrphansPanel::Suppressed {
                    reason: format!(
                        "not authorized to read {collection} — orphan analysis needs every collection"
                    ),
                })
            }
            Err(SweepFailure::Unavailable) => {
                return Ok(OrphansPanel::Suppressed {
                    reason: format!(
                        "the {collection} collection is unavailable — orphan analysis needs every collection"
                    ),
                })
            }
        }
    }
    // Order matches the loop above.
    let ai_providers = sweeps.pop().map(|(_, s)| s).unwrap_or(Sweep {
        items: Vec::new(),
        total: 0,
        partial: None,
    });
    let secrets = sweeps.pop().map(|(_, s)| s).unwrap_or(Sweep {
        items: Vec::new(),
        total: 0,
        partial: None,
    });
    let domains = sweeps.pop().map(|(_, s)| s).unwrap_or(Sweep {
        items: Vec::new(),
        total: 0,
        partial: None,
    });
    let clusters = sweeps.pop().map(|(_, s)| s).unwrap_or(Sweep {
        items: Vec::new(),
        total: 0,
        partial: None,
    });
    let route_configs = sweeps.pop().map(|(_, s)| s).unwrap_or(Sweep {
        items: Vec::new(),
        total: 0,
        partial: None,
    });
    let listeners = sweeps.pop().map(|(_, s)| s).unwrap_or(Sweep {
        items: Vec::new(),
        total: 0,
        partial: None,
    });

    // Metadata collections (domains, secrets, ai/providers) need no full typed spec
    // decode, but their sweeps must be complete AND every item's join-relevant fields
    // must be readable — an unreadable field could hold the very reference (or name)
    // that clears an orphan, so it suppresses like any other decode failure.
    for (sweep, collection) in [
        (&domains, "rate-limit-domains"),
        (&secrets, "secrets"),
        (&ai_providers, "AI providers"),
    ] {
        if sweep.partial.is_some() {
            return Ok(OrphansPanel::Suppressed {
                reason: format!(
                    "the {collection} sweep is partial — orphan analysis is never computed from a partial sweep"
                ),
            });
        }
    }
    if let Some(reason) = validate_join_fields(&domains, &secrets, &ai_providers) {
        return Ok(OrphansPanel::Suppressed { reason });
    }

    let typed_listeners = match typed_or_reason::<ListenerSpec>(&listeners, "listeners") {
        Ok(t) => t,
        Err(reason) => return Ok(OrphansPanel::Suppressed { reason }),
    };
    let typed_route_configs =
        match typed_or_reason::<RouteConfigSpec>(&route_configs, "route-configs") {
            Ok(t) => t,
            Err(reason) => return Ok(OrphansPanel::Suppressed { reason }),
        };
    let typed_clusters = match typed_or_reason::<ClusterSpec>(&clusters, "clusters") {
        Ok(t) => t,
        Err(reason) => return Ok(OrphansPanel::Suppressed { reason }),
    };
    let domain_names: Vec<String> = domains
        .items
        .iter()
        .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();

    Ok(OrphansPanel::Findings(build_findings(
        &typed_listeners,
        &typed_route_configs,
        &typed_clusters,
        &domain_names,
        &listeners.items,
        &clusters.items,
        &ai_providers.items,
        &secrets.items,
    )))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn typed<S: serde::de::DeserializeOwned>(items: &[Value]) -> Vec<(String, S)> {
        items
            .iter()
            .map(|item| {
                (
                    item.get("name").unwrap().as_str().unwrap().to_string(),
                    serde_json::from_value::<S>(item.get("spec").unwrap().clone())
                        .expect("test fixture must decode"),
                )
            })
            .collect()
    }

    fn cluster(name: &str, aggregate: &[&str]) -> Value {
        json!({"name": name, "spec": {"endpoints": [{"host": "10.0.0.1", "port": 80}],
            "aggregate_clusters": aggregate}})
    }

    fn listener(name: &str, rc: Option<&str>, filters: Value) -> Value {
        let mut spec = json!({"address": "0.0.0.0", "port": 8080, "http_filters": filters});
        if let Some(rc) = rc {
            spec["route_config"] = json!(rc);
        }
        json!({"name": name, "spec": spec})
    }

    fn rc(name: &str, cluster: &str) -> Value {
        json!({"name": name, "spec": {"virtual_hosts": [{"name": "vh", "domains": ["e.com"],
            "routes": [{"name": "r", "match": {"prefix": {"prefix": "/"}},
                        "action": {"cluster": cluster}}]}]}})
    }

    fn findings(
        listeners: &[Value],
        route_configs: &[Value],
        clusters: &[Value],
        domains: &[&str],
        providers: &[Value],
        secrets: &[Value],
    ) -> OrphanFindings {
        build_findings(
            &typed::<ListenerSpec>(listeners),
            &typed::<RouteConfigSpec>(route_configs),
            &typed::<ClusterSpec>(clusters),
            &domains.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
            listeners,
            clusters,
            providers,
            secrets,
        )
    }

    #[test]
    fn each_orphan_kind_is_flagged_by_its_planted_defect() {
        let listeners = vec![
            listener("edge", Some("routes"), json!([])),
            // Planted defect 2: unbound listener.
            listener("spare", None, json!([])),
        ];
        let route_configs = vec![rc("routes", "checkout")];
        let clusters = vec![
            cluster("checkout", &[]),
            // Planted defect 1: nothing references it.
            cluster("stray", &[]),
        ];
        // Planted defect 3: nothing attaches this domain.
        let domains = ["idle-domain"];
        // Planted defect 4: a secret no reference names.
        let secrets = vec![json!({"id": "0198f2f3-1111-7000-8000-000000000001",
            "name": "dangling-secret", "secret_type": "generic"})];

        let out = findings(
            &listeners,
            &route_configs,
            &clusters,
            &domains,
            &[],
            &secrets,
        );
        assert_eq!(out.unreferenced_clusters, vec!["stray"]);
        assert_eq!(out.unbound_listeners, vec!["spare"]);
        assert_eq!(out.unattached_domains, vec!["idle-domain"]);
        assert_eq!(out.unreferenced_secrets, vec!["dangling-secret"]);
    }

    #[test]
    fn references_prevent_false_orphans_across_all_kinds() {
        const NS_A: &str = "8c2a7f1e-4b3d-8f6a-9c1b-2e5d7a9f3c1b";
        const NS_B: &str = "1f9e3d7c-5a2b-8e4f-a6d1-3c8b5f2e9a7d";
        let composed = format!("{NS_A}|{NS_B}|checkout-domain");
        let listeners = vec![listener(
            "edge",
            Some("routes"),
            json!([
                {"filter": {"type": "global_rate_limit", "domain": composed,
                            "service_cluster": "rate_limit_cluster"}},
                {"filter": {"type": "ext_authz", "cluster": "authz-svc"}},
                {"filter": {"type": "jwt_auth", "providers": {"main": {
                    "jwks": {"source": "remote", "uri": "https://idp/jwks",
                             "cluster": "idp-cluster", "timeout_ms": 1000}}}}}
            ]),
        )];
        let route_configs = vec![json!({"name": "routes", "spec": {"virtual_hosts": [{
        "name": "vh", "domains": ["e.com"], "routes": [
            {"name": "split", "match": {"prefix": {"prefix": "/"}},
             "action": {"weighted_clusters": [
                {"cluster": "blue", "weight": 90}, {"cluster": "green", "weight": 10}]}}
        ]}]}})];
        let clusters = vec![
            cluster("blue", &["agg-member"]),
            cluster("green", &[]),
            // Referenced only via aggregate list.
            cluster("agg-member", &[]),
            // Referenced only via filters.
            cluster("authz-svc", &[]),
            cluster("idp-cluster", &[]),
        ];
        let provider_secret = "0198f2f3-2222-7000-8000-00000000aaaa";
        let providers = vec![json!({"name": "openai", "spec": {
            "kind": "openai", "base_url": "https://api.openai.com",
            "credential_secret_id": provider_secret}})];
        let secrets = vec![json!({"id": provider_secret, "name": "credential",
            "secret_type": "generic"})];

        let out = findings(
            &listeners,
            &route_configs,
            &clusters,
            &["checkout-domain"],
            &providers,
            &secrets,
        );
        assert!(
            out.unreferenced_clusters.is_empty(),
            "weighted/aggregate/filter references must all count: {:?}",
            out.unreferenced_clusters
        );
        assert!(out.unbound_listeners.is_empty());
        assert!(
            out.unattached_domains.is_empty(),
            "the composed built-in domain attaches checkout-domain"
        );
        assert!(
            out.unreferenced_secrets.is_empty(),
            "the id-referenced credential is not an orphan"
        );
    }

    #[test]
    fn external_limiter_does_not_attach_a_domain() {
        // The external limiter names the domain verbatim but is excluded from
        // attachment claims → the domain IS unattached.
        let listeners = vec![listener(
            "edge",
            Some("routes"),
            json!([{"filter": {"type": "global_rate_limit", "domain": "checkout-domain",
                               "service_cluster": "my-external-rls"}}]),
        )];
        // The external service_cluster is itself a cluster reference though.
        let clusters = vec![cluster("my-external-rls", &[])];
        let out = findings(
            &listeners,
            &[rc("routes", "my-external-rls")],
            &clusters,
            &["checkout-domain"],
            &[],
            &[],
        );
        assert_eq!(out.unattached_domains, vec!["checkout-domain"]);
        assert!(out.unreferenced_clusters.is_empty());
    }

    #[test]
    fn malformed_ai_provider_or_metadata_envelope_suppresses_analysis() {
        let complete = |items: Vec<Value>| Sweep {
            total: items.len() as i64,
            items,
            partial: None,
        };
        // Provider without a parseable credential id → suppression, never a false
        // "unreferenced secret".
        let reason = validate_join_fields(
            &complete(vec![]),
            &complete(vec![]),
            &complete(vec![json!({"name": "broken", "spec": {"kind": "openai"}})]),
        )
        .expect("must suppress");
        assert!(reason.contains("broken") && reason.contains("credential_secret_id"));
        // Secret without an id → suppression naming the item by its surviving field.
        let reason = validate_join_fields(
            &complete(vec![]),
            &complete(vec![json!({"name": "s1"})]),
            &complete(vec![]),
        )
        .expect("must suppress");
        assert!(
            reason.contains("secret") && reason.contains("s1"),
            "{reason}"
        );
        // Secret without a name (id survives) → suppression naming the id.
        let reason = validate_join_fields(
            &complete(vec![]),
            &complete(vec![json!({"id": "0198f2f3-1111-7000-8000-00000000cccc"})]),
            &complete(vec![]),
        )
        .expect("must suppress");
        assert!(
            reason.contains("0198f2f3-1111-7000-8000-00000000cccc"),
            "{reason}"
        );
        // Domain without a name → suppression.
        let reason = validate_join_fields(
            &complete(vec![json!({"id": "x"})]),
            &complete(vec![]),
            &complete(vec![]),
        )
        .expect("must suppress");
        assert!(reason.contains("rate-limit-domain"));
        // Well-formed metadata passes.
        assert!(validate_join_fields(
            &complete(vec![json!({"name": "d"})]),
            &complete(vec![
                json!({"name": "s", "id": "0198f2f3-1111-7000-8000-000000000001"})
            ]),
            &complete(vec![json!({"name": "p", "spec": {
                "credential_secret_id": "0198f2f3-1111-7000-8000-000000000002"}})]),
        )
        .is_none());
    }

    #[test]
    fn suppression_reasons_for_partial_and_unparseable_collections() {
        use super::super::resources::PartialReason;
        let partial = Sweep {
            items: vec![],
            total: 10,
            partial: Some(PartialReason::Budget),
        };
        let err = typed_or_reason::<ListenerSpec>(&partial, "listeners").unwrap_err();
        assert!(err.contains("partial"), "{err}");

        let junk = Sweep {
            items: vec![json!({"name": "mystery", "spec": {"x": 1}})],
            total: 1,
            partial: None,
        };
        let err = typed_or_reason::<ListenerSpec>(&junk, "listeners").unwrap_err();
        assert!(err.contains("unparseable"), "{err}");
        assert!(err.contains("mystery"), "{err}");
    }
}
