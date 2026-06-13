//! Per-team xDS snapshot cache (spec/10 §5).
//!
//! Rebuilds are driven by outbox events — never by polling (kills spec/08 §2). Each team's
//! snapshot carries an independent version PER resource type, bumped only when the encoded
//! bytes actually change; ADS streams respond from this cache and never re-query the
//! database per request (kills v1's reconnect-storm fan-out, spec/04 §8.7).

use crate::translate;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use envoy_types::pb::google::protobuf::Any;
use fp_domain::{DomainError, DomainResult, SecretSpec, TeamId};
use prost::Message;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

pub const CLUSTER_TYPE_URL: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
pub const ROUTE_TYPE_URL: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
pub const LISTENER_TYPE_URL: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
pub const ENDPOINT_TYPE_URL: &str =
    "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment";
pub const SECRET_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret";

/// One resource type's serving state for one team (the external, per-stream view).
#[derive(Debug, Clone, Default)]
pub struct ResourceSet {
    /// Monotonic per-type version; bumps only when the served bytes change.
    pub version: u64,
    /// Encoded resources, sorted by name (deterministic responses, post-quarantine).
    pub resources: Vec<Any>,
}

#[derive(Debug, Clone, Default)]
pub struct TeamSnapshot {
    pub clusters: ResourceSet,
    pub endpoints: ResourceSet,
    pub routes: ResourceSet,
    pub secrets: ResourceSet,
    pub listeners: ResourceSet,
}

impl TeamSnapshot {
    pub fn for_type_url(&self, type_url: &str) -> Option<&ResourceSet> {
        match type_url {
            CLUSTER_TYPE_URL => Some(&self.clusters),
            ENDPOINT_TYPE_URL => Some(&self.endpoints),
            ROUTE_TYPE_URL => Some(&self.routes),
            SECRET_TYPE_URL => Some(&self.secrets),
            LISTENER_TYPE_URL => Some(&self.listeners),
            _ => None,
        }
    }
}

/// A translated resource with its name retained (NACK attribution + quarantine keying).
#[derive(Debug, Clone, PartialEq)]
struct NamedResource {
    name: String,
    any: Any,
}

/// A quarantined resource: its rejected bytes and the last bytes a dataplane accepted.
#[derive(Debug, Clone)]
struct Quarantined {
    /// The encoded bytes that were NACKed; serving resumes only when they change.
    offending: Vec<u8>,
    /// Last-known-good encoding, served in place of the offending one (spec/10 §5).
    /// `None` when the resource was new — then it is excluded entirely.
    last_good: Option<Any>,
    error: String,
}

/// Internal per-type state: latest two raw generations (for NACK attribution), the
/// quarantine map, and the served (post-quarantine) set.
#[derive(Debug, Clone, Default)]
struct TypeInternal {
    version: u64,
    /// Latest translation from the database (pre-quarantine).
    raw: Vec<NamedResource>,
    /// Previous raw generation — what a NACKing dataplane last accepted, best effort.
    raw_prev: Vec<NamedResource>,
    quarantine: HashMap<String, Quarantined>,
    served: Vec<NamedResource>,
}

impl TypeInternal {
    /// Install a fresh raw generation, prune stale quarantine entries (resource deleted,
    /// or its bytes changed = operator pushed a fix), and recompute the served set.
    /// Returns true when the served bytes changed.
    fn install_raw(&mut self, fresh: Vec<NamedResource>) -> bool {
        if fresh != self.raw {
            self.raw_prev = std::mem::replace(&mut self.raw, fresh);
        }
        let raw = &self.raw;
        self.quarantine.retain(|name, q| {
            raw.iter()
                .any(|r| r.name == *name && r.any.value == q.offending)
        });
        self.recompute_served()
    }

    fn recompute_served(&mut self) -> bool {
        let served: Vec<NamedResource> = self
            .raw
            .iter()
            .filter_map(|r| match self.quarantine.get(&r.name) {
                Some(q) if r.any.value == q.offending => {
                    q.last_good.as_ref().map(|good| NamedResource {
                        name: r.name.clone(),
                        any: good.clone(),
                    })
                }
                _ => Some(r.clone()),
            })
            .collect();
        if served != self.served {
            self.served = served;
            self.version += 1;
            true
        } else {
            false
        }
    }

    /// Quarantine what changed between the two raw generations. With no previous
    /// generation (first push or post-restart prime) attribution is impossible — persist
    /// only, quarantine nothing (wait-for-fix, never blanket-quarantine a whole type).
    fn apply_nack(&mut self, error: &str) -> (Vec<String>, bool) {
        if self.raw_prev.is_empty() {
            return (Vec::new(), false);
        }
        let mut named = Vec::new();
        for resource in &self.raw {
            let previous = self.raw_prev.iter().find(|p| p.name == resource.name);
            let changed = match previous {
                Some(p) => p.any.value != resource.any.value,
                None => true, // newly added
            };
            let already = self
                .quarantine
                .get(&resource.name)
                .is_some_and(|q| q.offending == resource.any.value);
            if changed && !already {
                self.quarantine.insert(
                    resource.name.clone(),
                    Quarantined {
                        offending: resource.any.value.clone(),
                        last_good: previous.map(|p| p.any.clone()),
                        error: error.to_string(),
                    },
                );
                named.push(resource.name.clone());
            }
        }
        let served_changed = if named.is_empty() {
            false
        } else {
            self.recompute_served()
        };
        (named, served_changed)
    }

    fn to_set(&self) -> ResourceSet {
        ResourceSet {
            version: self.version,
            resources: self.served.iter().map(|r| r.any.clone()).collect(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TeamInternal {
    clusters: TypeInternal,
    endpoints: TypeInternal,
    routes: TypeInternal,
    secrets: TypeInternal,
    listeners: TypeInternal,
}

impl TeamInternal {
    fn for_type_mut(&mut self, type_url: &str) -> Option<&mut TypeInternal> {
        match type_url {
            CLUSTER_TYPE_URL => Some(&mut self.clusters),
            ENDPOINT_TYPE_URL => Some(&mut self.endpoints),
            ROUTE_TYPE_URL => Some(&mut self.routes),
            SECRET_TYPE_URL => Some(&mut self.secrets),
            LISTENER_TYPE_URL => Some(&mut self.listeners),
            _ => None,
        }
    }
}

/// One quarantined (degraded) resource, as surfaced to status queries.
#[derive(Debug, Clone)]
pub struct DegradedResource {
    pub type_url: String,
    pub name: String,
    pub error: String,
}

/// The cache: team → snapshot, plus a change signal streams can await.
pub struct SnapshotCache {
    snapshots: RwLock<HashMap<TeamId, TeamInternal>>,
    /// Bumped on every snapshot change; payload is the team that changed.
    change_tx: watch::Sender<(u64, Option<TeamId>)>,
    change_seq: std::sync::atomic::AtomicU64,
}

impl Default for SnapshotCache {
    fn default() -> Self {
        let (change_tx, _) = watch::channel((0, None));
        Self {
            snapshots: RwLock::new(HashMap::new()),
            change_tx,
            change_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl SnapshotCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn team(&self, team_id: TeamId) -> TeamSnapshot {
        self.snapshots
            .read()
            .await
            .get(&team_id)
            .map(|internal| TeamSnapshot {
                clusters: internal.clusters.to_set(),
                endpoints: internal.endpoints.to_set(),
                routes: internal.routes.to_set(),
                secrets: internal.secrets.to_set(),
                listeners: internal.listeners.to_set(),
            })
            .unwrap_or_default()
    }

    /// Currently quarantined (degraded) resources for a team, all types.
    pub async fn degraded(&self, team_id: TeamId) -> Vec<DegradedResource> {
        let snapshots = self.snapshots.read().await;
        let Some(internal) = snapshots.get(&team_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (type_url, state) in [
            (CLUSTER_TYPE_URL, &internal.clusters),
            (ENDPOINT_TYPE_URL, &internal.endpoints),
            (ROUTE_TYPE_URL, &internal.routes),
            (SECRET_TYPE_URL, &internal.secrets),
            (LISTENER_TYPE_URL, &internal.listeners),
        ] {
            for (name, q) in &state.quarantine {
                out.push(DegradedResource {
                    type_url: type_url.to_string(),
                    name: name.clone(),
                    error: q.error.clone(),
                });
            }
        }
        out.sort_by(|a, b| (&a.type_url, &a.name).cmp(&(&b.type_url, &b.name)));
        out
    }

    /// A dataplane NACKed `type_url` for this team: quarantine the resources that changed
    /// since the previous generation (served set falls back to their last-good bytes) and
    /// return their names for persistence. Streams are notified when serving changed.
    pub async fn apply_nack(&self, team_id: TeamId, type_url: &str, error: &str) -> Vec<String> {
        let (named, changed) = {
            let mut snapshots = self.snapshots.write().await;
            let Some(state) = snapshots
                .get_mut(&team_id)
                .and_then(|t| t.for_type_mut(type_url))
            else {
                return Vec::new();
            };
            state.apply_nack(error)
        };
        if !named.is_empty() {
            metrics::counter!("fp_xds_quarantined_resources_total").increment(named.len() as u64);
            tracing::warn!(team = %team_id, type_url, resources = ?named,
                "quarantined NACKed resources; serving last-good bytes");
        }
        if changed {
            self.notify(team_id);
        }
        named
    }

    fn notify(&self, team_id: TeamId) {
        let seq = self
            .change_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let _ = self.change_tx.send((seq, Some(team_id)));
    }

    /// Subscribe to change notifications (streams re-check their team on wake).
    pub fn watch(&self) -> watch::Receiver<(u64, Option<TeamId>)> {
        self.change_tx.subscribe()
    }

    /// Prime the cache from the database at startup: rebuild every team that owns gateway
    /// resources. Without this, a restarted control plane serves EMPTY snapshots to
    /// reconnecting dataplanes (the outbox cursor is durable, so old events never replay)
    /// and state-of-the-world delivery would wipe their config.
    pub async fn prime_all(&self, pool: &PgPool) -> DomainResult<usize> {
        let teams = fp_storage::repos::gateway::teams_with_gateway_resources(pool).await?;
        let count = teams.len();
        for team_id in teams {
            self.rebuild_team(pool, team_id).await?;
        }
        Ok(count)
    }

    /// Rebuild one team's snapshot from the database. Loads, translates, and swaps in the
    /// new sets, bumping each type's version only when its bytes changed.
    pub async fn rebuild_team(&self, pool: &PgPool, team_id: TeamId) -> DomainResult<()> {
        // Load everything the team owns. The 500-row repo cap is the current ceiling per
        // type; quotas (50/25/100) keep real teams far below it.
        let (clusters, _) = fp_storage::repos::clusters::list(
            pool,
            fp_storage::scope::TeamScope::Team(team_id),
            500,
            0,
        )
        .await?;
        let (route_configs, _) =
            fp_storage::repos::gateway::list_route_configs(pool, team_id, 500, 0).await?;
        let (listeners, _) =
            fp_storage::repos::gateway::list_listeners(pool, team_id, 500, 0).await?;
        let secrets = fp_storage::repos::secrets::list_encrypted_secrets(pool, team_id).await?;

        let mut cluster_named = Vec::with_capacity(clusters.len());
        let mut endpoint_named = Vec::new();
        for cluster in &clusters {
            let proto = translate::cluster_to_proto(&cluster.name, &cluster.spec)?;
            cluster_named.push(NamedResource {
                name: cluster.name.clone(),
                any: Any {
                    type_url: CLUSTER_TYPE_URL.to_string(),
                    value: proto.encode_to_vec(),
                },
            });
            // EDS clusters get their assignment as a separate resource: endpoint churn
            // bumps only the endpoints version, never the cluster bytes (spec/10 §5).
            if translate::cluster_uses_eds(&cluster.spec) {
                let cla = translate::endpoints_to_proto(&cluster.name, &cluster.spec);
                endpoint_named.push(NamedResource {
                    name: cluster.name.clone(),
                    any: Any {
                        type_url: ENDPOINT_TYPE_URL.to_string(),
                        value: cla.encode_to_vec(),
                    },
                });
            }
        }
        let mut route_named = Vec::with_capacity(route_configs.len());
        for rc in &route_configs {
            let proto = translate::route_config_to_proto(&rc.name, &rc.spec)?;
            route_named.push(NamedResource {
                name: rc.name.clone(),
                any: Any {
                    type_url: ROUTE_TYPE_URL.to_string(),
                    value: proto.encode_to_vec(),
                },
            });
        }
        let mut secret_named = Vec::with_capacity(secrets.len());
        for secret in &secrets {
            let spec = decrypt_secret_spec(&secret.ciphertext, &secret.nonce)?;
            let proto = translate::secret_to_proto(&secret.metadata.name, &spec)?;
            secret_named.push(NamedResource {
                name: secret.metadata.name.clone(),
                any: Any {
                    type_url: SECRET_TYPE_URL.to_string(),
                    value: proto.encode_to_vec(),
                },
            });
        }
        let mut listener_named = Vec::with_capacity(listeners.len());
        for listener in &listeners {
            // Listeners without a bound route config cannot serve; they stay out of the
            // snapshot rather than producing a NACK-able resource.
            if listener.spec.route_config.is_none() {
                tracing::debug!(team = %team_id, listener = %listener.name,
                    "skipping unbound listener in snapshot");
                continue;
            }
            let proto = translate::listener_to_proto(&listener.name, &listener.spec)?;
            listener_named.push(NamedResource {
                name: listener.name.clone(),
                any: Any {
                    type_url: LISTENER_TYPE_URL.to_string(),
                    value: proto.encode_to_vec(),
                },
            });
        }

        let mut changed = false;
        {
            let mut snapshots = self.snapshots.write().await;
            let entry = snapshots.entry(team_id).or_default();
            for (state, fresh) in [
                (&mut entry.clusters, cluster_named),
                (&mut entry.endpoints, endpoint_named),
                (&mut entry.routes, route_named),
                (&mut entry.secrets, secret_named),
                (&mut entry.listeners, listener_named),
            ] {
                changed |= state.install_raw(fresh);
            }
        }

        if changed {
            let seq = self
                .change_seq
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            let _ = self.change_tx.send((seq, Some(team_id)));
            metrics::counter!("fp_xds_snapshot_rebuilds_total").increment(1);
            tracing::info!(team = %team_id, "xDS snapshot rebuilt");
        }
        Ok(())
    }
}

fn decrypt_secret_spec(ciphertext: &[u8], nonce: &[u8]) -> DomainResult<SecretSpec> {
    let key = secret_key()?;
    let nonce = <[u8; 12]>::try_from(nonce)
        .map_err(|_| DomainError::internal("secret nonce must be 12 bytes"))?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| DomainError::invalid_config("FLOWPLANE_SECRET_ENCRYPTION_KEY is invalid"))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext)
        .map_err(|_| DomainError::internal("decrypt secret spec"))?;
    serde_json::from_slice(&plaintext)
        .map_err(|e| DomainError::internal(format!("parse decrypted secret spec: {e}")))
}

fn secret_key() -> DomainResult<[u8; 32]> {
    let raw = std::env::var("FLOWPLANE_SECRET_ENCRYPTION_KEY").map_err(|_| {
        DomainError::unavailable("secret encryption key is not configured")
            .with_hint("set FLOWPLANE_SECRET_ENCRYPTION_KEY to a 32-byte or base64-encoded key")
    })?;
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    <[u8; 32]>::try_from(raw.as_bytes()).map_err(|_| {
        DomainError::invalid_config(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY must be exactly 32 bytes after decoding",
        )
    })
}

/// The outbox consumer wiring: rebuild each team touched by the batch, once.
pub async fn handle_events(
    cache: &SnapshotCache,
    pool: &PgPool,
    events: Vec<fp_storage::outbox::StoredEvent>,
) -> DomainResult<()> {
    let mut teams: Vec<TeamId> = events.iter().filter_map(|e| e.scope.team_id).collect();
    teams.sort();
    teams.dedup();
    for team_id in teams {
        cache.rebuild_team(pool, team_id).await?;
    }
    Ok(())
}

pub const XDS_CONSUMER: &str = "xds-snapshot";

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_core::{GrantSet, PrincipalCtx};
    use fp_domain::authz::TeamRef;
    use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
    use fp_domain::gateway::listener::ListenerSpec;
    use fp_domain::gateway::route_config::{
        PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
    };
    use fp_domain::{OrgRole, RequestId};
    use fp_storage::repos::identity;

    fn unique(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            &uuid::Uuid::now_v7().simple().to_string()[20..]
        )
    }

    fn cluster_spec(host: &str) -> ClusterSpec {
        ClusterSpec {
            endpoints: vec![Endpoint {
                host: host.into(),
                port: 8080,
                weight: None,
            }],
            lb_policy: LbPolicy::RoundRobin,
            least_request: None,
            ring_hash: None,
            maglev: None,
            dns_lookup_family: None,
            connect_timeout_secs: 5,
            use_tls: false,
            upstream_tls: None,
            protocol: None,
            health_checks: None,
            circuit_breakers: None,
            outlier_detection: None,
        }
    }

    fn rc_spec(cluster: &str) -> RouteConfigSpec {
        RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "all".into(),
                    matcher: PathMatch::Prefix { prefix: "/".into() },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: RouteAction {
                        cluster: Some(cluster.into()),
                        weighted_clusters: None,
                        redirect: None,
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                        retry_policy: None,
                        rate_limits: Vec::new(),
                    },
                    filter_overrides: Vec::new(),
                }],
                rate_limits: Vec::new(),
                filter_overrides: Vec::new(),
            }],
        }
    }

    async fn world() -> Option<(PgPool, TeamRef, TeamRef, PrincipalCtx, PrincipalCtx)> {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return None;
        };
        let pool = fp_storage::connect(&url, 8).await.expect("connect");
        fp_storage::migrate(&pool).await.expect("migrate");
        let mut out = Vec::new();
        for _ in 0..2 {
            let org = identity::create_org(&pool, &unique("org"), "")
                .await
                .expect("org");
            let team = identity::create_team(&pool, org.id, &unique("team"), "")
                .await
                .expect("t");
            let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "x@x.test", "X")
                .await
                .expect("u");
            identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
                .await
                .expect("m");
            out.push((
                TeamRef {
                    id: team.id,
                    org_id: org.id,
                },
                PrincipalCtx::User {
                    user_id: user,
                    platform_admin: false,
                    org_selector_required: false,
                    org: Some((org.id, OrgRole::Admin)),
                    grants: GrantSet::default(),
                },
            ));
        }
        let (team_b, ctx_b) = out.pop().expect("b");
        let (team_a, ctx_a) = out.pop().expect("a");
        Some((pool, team_a, team_b, ctx_a, ctx_b))
    }

    #[tokio::test]
    async fn events_drive_rebuilds_with_per_type_versions_and_team_isolation() {
        let Some((pool, team_a, team_b, ctx_a, ctx_b)) = world().await else {
            return;
        };
        let cache = SnapshotCache::new();
        let consumer = format!("xds-test-{}", unique("c"));
        fp_storage::outbox::register_consumer(&pool, &consumer)
            .await
            .expect("register");
        // Fast-forward past unrelated events from parallel tests.
        let _ = fp_storage::outbox::process_batch(&pool, &consumer, 100_000, |_| async { Ok(()) })
            .await;

        // Team A: cluster + bound route config + listener. Team B: one cluster only.
        let upstream = unique("upstream");
        fp_core::services::clusters::create_cluster(
            &pool,
            &ctx_a,
            team_a,
            &upstream,
            cluster_spec("10.0.0.1"),
            RequestId::generate(),
        )
        .await
        .expect("a cluster");
        let rc = unique("routes");
        fp_core::services::gateway::create_route_config(
            &pool,
            &ctx_a,
            team_a,
            &rc,
            rc_spec(&upstream),
            RequestId::generate(),
        )
        .await
        .expect("a rc");
        fp_core::services::gateway::create_listener(
            &pool,
            &ctx_a,
            team_a,
            &unique("edge"),
            ListenerSpec {
                address: "0.0.0.0".into(),
                port: 19200,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(rc.clone()),
                tls_context: None,
                http_filters: Vec::new(),
            },
            RequestId::generate(),
        )
        .await
        .expect("a listener");
        fp_core::services::clusters::create_cluster(
            &pool,
            &ctx_b,
            team_b,
            &unique("other"),
            cluster_spec("10.9.9.9"),
            RequestId::generate(),
        )
        .await
        .expect("b cluster");

        // Drain the outbox through the snapshot handler (as the live consumer would).
        while fp_storage::outbox::process_batch(&pool, &consumer, 100, |events| {
            let cache = cache.clone();
            let pool = pool.clone();
            async move { handle_events(&cache, &pool, events).await }
        })
        .await
        .expect("process")
            > 0
        {}

        let snap_a = cache.team(team_a.id).await;
        assert_eq!(snap_a.clusters.resources.len(), 1);
        assert_eq!(snap_a.routes.resources.len(), 1);
        assert_eq!(snap_a.listeners.resources.len(), 1);
        assert!(snap_a.clusters.version >= 1);

        // Team isolation: B's snapshot contains ONLY B's cluster, no routes/listeners.
        let snap_b = cache.team(team_b.id).await;
        assert_eq!(snap_b.clusters.resources.len(), 1);
        assert!(snap_b.routes.resources.is_empty());
        assert!(snap_b.listeners.resources.is_empty());
        assert_ne!(
            snap_a.clusters.resources[0].value, snap_b.clusters.resources[0].value,
            "different teams, different bytes"
        );

        // No-change rebuild: versions must NOT bump (byte-diff suppression).
        let cluster_version = snap_a.clusters.version;
        cache.rebuild_team(&pool, team_a.id).await.expect("rebuild");
        let again = cache.team(team_a.id).await;
        assert_eq!(
            again.clusters.version, cluster_version,
            "unchanged bytes, unchanged version"
        );

        // Endpoint churn is EDS-only: the endpoints version bumps, the cluster bytes (an
        // EDS reference for IP endpoints) and routes stay untouched (spec/10 §5).
        let routes_version = again.routes.version;
        let endpoints_version = again.endpoints.version;
        fp_core::services::clusters::update_cluster(
            &pool,
            &ctx_a,
            team_a,
            &upstream,
            cluster_spec("10.0.0.2"),
            1,
            RequestId::generate(),
        )
        .await
        .expect("update");
        while fp_storage::outbox::process_batch(&pool, &consumer, 100, |events| {
            let cache = cache.clone();
            let pool = pool.clone();
            async move { handle_events(&cache, &pool, events).await }
        })
        .await
        .expect("process")
            > 0
        {}
        let after = cache.team(team_a.id).await;
        assert_eq!(
            after.clusters.version, cluster_version,
            "endpoint churn must not rebuild the cluster"
        );
        assert_eq!(
            after.endpoints.version,
            endpoints_version + 1,
            "endpoints version bumped"
        );
        assert_eq!(
            after.routes.version, routes_version,
            "route version untouched"
        );

        // Restart safety: a brand-new cache (fresh process, durable outbox cursor, so no
        // event replay) primed from the DB must serve the same resources — never empty
        // snapshots that would wipe a reconnecting dataplane.
        let fresh = SnapshotCache::new();
        fresh.prime_all(&pool).await.expect("prime");
        let primed = fresh.team(team_a.id).await;
        assert_eq!(primed.clusters.resources.len(), 1);
        assert_eq!(primed.routes.resources.len(), 1);
        assert_eq!(primed.listeners.resources.len(), 1);
        assert_eq!(
            primed.clusters.resources, after.clusters.resources,
            "primed snapshot matches the event-driven one byte for byte"
        );
    }

    #[tokio::test]
    async fn nack_quarantine_serves_last_good_until_a_fix_arrives() {
        let Some((pool, team, _, ctx, _)) = world().await else {
            return;
        };
        let cache = SnapshotCache::new();
        let upstream = unique("upstream");
        fp_core::services::clusters::create_cluster(
            &pool,
            &ctx,
            team,
            &upstream,
            cluster_spec("10.0.0.1"),
            RequestId::generate(),
        )
        .await
        .expect("cluster");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let initial = cache.team(team.id).await.endpoints;

        // A NACK on the FIRST generation cannot be attributed: nothing is quarantined
        // (never blanket-quarantine a whole type).
        let quarantined = cache
            .apply_nack(team.id, ENDPOINT_TYPE_URL, "first push rejected")
            .await;
        assert!(quarantined.is_empty(), "no previous generation, no blame");
        assert!(cache.degraded(team.id).await.is_empty());

        // Update the cluster, then a dataplane NACKs the new bytes: the changed resource
        // is quarantined and serving falls back to the last-good bytes.
        fp_core::services::clusters::update_cluster(
            &pool,
            &ctx,
            team,
            &upstream,
            cluster_spec("10.0.0.2"),
            1,
            RequestId::generate(),
        )
        .await
        .expect("update");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let updated = cache.team(team.id).await.endpoints;
        assert_ne!(updated.resources, initial.resources);

        let quarantined = cache
            .apply_nack(
                team.id,
                ENDPOINT_TYPE_URL,
                "Proto constraint validation failed",
            )
            .await;
        assert_eq!(quarantined, vec![upstream.clone()]);
        let degraded = cache.degraded(team.id).await;
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].name, upstream);
        let rolled_back = cache.team(team.id).await.endpoints;
        assert!(
            rolled_back.version > updated.version,
            "quarantine produces a new pushable version"
        );
        assert_eq!(
            rolled_back.resources, initial.resources,
            "served bytes are the last-good generation"
        );

        // Re-NACKing the corrected set must not loop: nothing new to quarantine.
        let version_after_rollback = rolled_back.version;
        let again = cache
            .apply_nack(team.id, ENDPOINT_TYPE_URL, "still unhappy")
            .await;
        assert!(
            again.is_empty(),
            "rollback bytes match last-good: no new blame"
        );
        assert_eq!(
            cache.team(team.id).await.endpoints.version,
            version_after_rollback
        );

        // An operator fix (any byte change) clears the quarantine and serves fresh.
        fp_core::services::clusters::update_cluster(
            &pool,
            &ctx,
            team,
            &upstream,
            cluster_spec("10.0.0.3"),
            2,
            RequestId::generate(),
        )
        .await
        .expect("fix");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        assert!(
            cache.degraded(team.id).await.is_empty(),
            "fix clears quarantine"
        );
        let fixed = cache.team(team.id).await.endpoints;
        assert_ne!(fixed.resources, initial.resources);
        assert_ne!(fixed.resources, updated.resources);
    }
}
