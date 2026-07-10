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
use fp_domain::gateway::cluster::{Cluster, ClusterSpec};
use fp_domain::gateway::listener::{Listener, ListenerSpec};
use fp_domain::gateway::route_config::{RouteConfig, RouteConfigSpec};
use fp_domain::{AiProviderId, ClusterId, ListenerId, RouteConfigId};
use fp_domain::{DataplaneId, DomainError, DomainResult, SecretSpec, TeamId};
use prost::Message;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DataplaneScope(Option<DataplaneId>);

#[derive(Debug, Clone, Default)]
struct ServedState {
    version: u64,
    served: Vec<NamedResource>,
}

impl ServedState {
    fn to_set(&self) -> ResourceSet {
        ResourceSet {
            version: self.version,
            resources: self.served.iter().map(|r| r.any.clone()).collect(),
        }
    }
}

/// Internal per-type state: latest two raw generations (for NACK attribution), the
/// quarantine map, and the served (post-quarantine) set.
#[derive(Debug, Clone, Default)]
struct TypeInternal {
    /// Latest translation from the database (pre-quarantine).
    raw: Vec<NamedResource>,
    /// Previous raw generation — what a NACKing dataplane last accepted, best effort.
    raw_prev: Vec<NamedResource>,
    quarantines: HashMap<DataplaneScope, HashMap<String, Quarantined>>,
    /// Resources skipped during control-plane translation before Envoy ever saw them.
    translation_failures: HashMap<String, String>,
    unscoped_served: ServedState,
    scoped_served: HashMap<DataplaneScope, ServedState>,
}

impl TypeInternal {
    /// Install a fresh raw generation, prune stale quarantine entries (resource deleted,
    /// or its bytes changed = operator pushed a fix), and recompute the served set.
    /// Returns true when the served bytes changed.
    fn install_raw(&mut self, fresh: Vec<NamedResource>) -> bool {
        self.install_raw_with_failures(fresh, HashMap::new())
    }

    fn install_raw_with_failures(
        &mut self,
        fresh: Vec<NamedResource>,
        translation_failures: HashMap<String, String>,
    ) -> bool {
        let failures_changed = self.translation_failures != translation_failures;
        self.translation_failures = translation_failures;
        if fresh != self.raw {
            self.raw_prev = std::mem::replace(&mut self.raw, fresh);
        }
        let raw = &self.raw;
        self.quarantines.retain(|_, quarantines| {
            quarantines.retain(|name, q| {
                raw.iter()
                    .any(|r| r.name == *name && r.any.value == q.offending)
            });
            !quarantines.is_empty()
        });
        self.recompute_all_served() || failures_changed
    }

    fn served_for_raw(
        raw: &[NamedResource],
        quarantine: Option<&HashMap<String, Quarantined>>,
    ) -> Vec<NamedResource> {
        let Some(quarantine) = quarantine else {
            return raw.to_vec();
        };
        raw.iter()
            .filter_map(|r| match quarantine.get(&r.name) {
                Some(q) if r.any.value == q.offending => {
                    q.last_good.as_ref().map(|good| NamedResource {
                        name: r.name.clone(),
                        any: good.clone(),
                    })
                }
                _ => Some(r.clone()),
            })
            .collect()
    }

    fn apply_served(state: &mut ServedState, served: Vec<NamedResource>) -> bool {
        if served != state.served {
            state.served = served;
            state.version += 1;
            true
        } else {
            false
        }
    }

    fn recompute_all_served(&mut self) -> bool {
        let mut changed = Self::apply_served(
            &mut self.unscoped_served,
            Self::served_for_raw(&self.raw, None),
        );
        for (scope, state) in &mut self.scoped_served {
            let quarantine = self.quarantines.get(scope);
            changed |= Self::apply_served(state, Self::served_for_raw(&self.raw, quarantine));
        }
        changed
    }

    /// Quarantine what changed between the two raw generations. With no previous
    /// generation (first push or post-restart prime) attribution is impossible — persist
    /// only, quarantine nothing (wait-for-fix, never blanket-quarantine a whole type).
    fn apply_nack(
        &mut self,
        dataplane_id: Option<DataplaneId>,
        error: &str,
    ) -> (Vec<String>, bool) {
        if self.raw_prev.is_empty() {
            return (Vec::new(), false);
        }
        let scope = DataplaneScope(dataplane_id);
        let quarantines = self.quarantines.entry(scope).or_default();
        let mut named = Vec::new();
        for resource in &self.raw {
            let previous = self.raw_prev.iter().find(|p| p.name == resource.name);
            let changed = match previous {
                Some(p) => p.any.value != resource.any.value,
                None => true, // newly added
            };
            let already = quarantines
                .get(&resource.name)
                .is_some_and(|q| q.offending == resource.any.value);
            if changed && !already {
                quarantines.insert(
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
            let served = Self::served_for_raw(&self.raw, self.quarantines.get(&scope));
            let state = self
                .scoped_served
                .entry(scope)
                .or_insert_with(|| ServedState {
                    version: self.unscoped_served.version,
                    served: self.unscoped_served.served.clone(),
                });
            Self::apply_served(state, served)
        };
        (named, served_changed)
    }

    fn to_set(&self, dataplane_id: Option<DataplaneId>) -> ResourceSet {
        let scope = DataplaneScope(dataplane_id);
        self.scoped_served
            .get(&scope)
            .unwrap_or(&self.unscoped_served)
            .to_set()
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
    /// When set, the built-in `rate_limit_cluster` is injected into every team's CDS (S6). The
    /// endpoint is validated once at boot, so synthesis here is expected to succeed.
    rls: Option<translate::RlsClusterConfig>,
}

impl Default for SnapshotCache {
    fn default() -> Self {
        let (change_tx, _) = watch::channel((0, None));
        Self {
            snapshots: RwLock::new(HashMap::new()),
            change_tx,
            change_seq: std::sync::atomic::AtomicU64::new(0),
            rls: None,
        }
    }
}

impl SnapshotCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Construct a cache that also injects the built-in `rate_limit_cluster` (S6) when `rls` is
    /// set. Pass `None` to behave exactly like [`SnapshotCache::new`].
    pub fn with_rls(rls: Option<translate::RlsClusterConfig>) -> Arc<Self> {
        Arc::new(Self {
            rls,
            ..Self::default()
        })
    }

    pub async fn team(&self, team_id: TeamId) -> TeamSnapshot {
        self.team_for_dataplane(team_id, None).await
    }

    pub async fn team_for_dataplane(
        &self,
        team_id: TeamId,
        dataplane_id: Option<DataplaneId>,
    ) -> TeamSnapshot {
        self.snapshots
            .read()
            .await
            .get(&team_id)
            .map(|internal| TeamSnapshot {
                clusters: internal.clusters.to_set(dataplane_id),
                endpoints: internal.endpoints.to_set(dataplane_id),
                routes: internal.routes.to_set(dataplane_id),
                secrets: internal.secrets.to_set(dataplane_id),
                listeners: internal.listeners.to_set(dataplane_id),
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
            for quarantines in state.quarantines.values() {
                for (name, q) in quarantines {
                    out.push(DegradedResource {
                        type_url: type_url.to_string(),
                        name: name.clone(),
                        error: q.error.clone(),
                    });
                }
            }
            for (name, error) in &state.translation_failures {
                out.push(DegradedResource {
                    type_url: type_url.to_string(),
                    name: name.clone(),
                    error: error.clone(),
                });
            }
        }
        out.sort_by(|a, b| (&a.type_url, &a.name).cmp(&(&b.type_url, &b.name)));
        out
    }

    /// A dataplane NACKed `type_url` for this team: quarantine the resources that changed
    /// since the previous generation (served set falls back to their last-good bytes) and
    /// return their names for persistence. Streams are notified when serving changed.
    pub async fn apply_nack(
        &self,
        team_id: TeamId,
        dataplane_id: Option<DataplaneId>,
        type_url: &str,
        error: &str,
    ) -> Vec<String> {
        let (named, changed) = {
            let mut snapshots = self.snapshots.write().await;
            let Some(state) = snapshots
                .get_mut(&team_id)
                .and_then(|t| t.for_type_mut(type_url))
            else {
                return Vec::new();
            };
            state.apply_nack(dataplane_id, error)
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
            if let Err(err) = self.rebuild_team(pool, team_id).await {
                metrics::counter!("fp_xds_prime_team_failures_total").increment(1);
                tracing::error!(team = %team_id, error = %err, "skipping failed xDS prime for team");
            }
        }
        Ok(count)
    }

    /// Rebuild one team's snapshot from the database. Loads, translates, and swaps in the
    /// new sets, bumping each type's version only when its bytes changed.
    pub async fn rebuild_team(&self, pool: &PgPool, team_id: TeamId) -> DomainResult<()> {
        // Load everything the team owns. The 500-row repo cap is the current ceiling per
        // type; quotas (50/25/100) keep real teams far below it.
        let XdsResources {
            clusters,
            route_configs,
            listeners,
            mut cluster_failures,
            mut route_failures,
            mut listener_failures,
        } = load_xds_resources(pool, team_id).await?;
        let secrets = fp_storage::repos::secrets::list_encrypted_secrets(pool, team_id).await?;
        let capture_plan = learning_capture_plan(pool, team_id, &route_configs).await?;

        let mut cluster_named = Vec::with_capacity(clusters.len());
        let mut endpoint_named = Vec::new();
        for xds_cluster in &clusters {
            let cluster = &xds_cluster.cluster;
            let proto = match translate::cluster_to_proto_with_ai(
                &cluster.name,
                &cluster.spec,
                xds_cluster.ai.as_ref(),
            ) {
                Ok(proto) => proto,
                Err(err) => {
                    let error = format!("cluster translation failed: {err}");
                    skip_xds_resource(team_id, "cluster", &cluster.name, &error);
                    cluster_failures.insert(cluster.name.clone(), error);
                    continue;
                }
            };
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

        // S6: inject the CP-synthesized built-in rate-limit cluster into every team's CDS when an
        // RLS endpoint is configured. STRICT_DNS/STATIC + HTTP/2 (+ optional mTLS). The endpoint
        // was validated at boot; if synthesis still fails we skip it loudly rather than abort the
        // whole team's gateway config (the listener-reference check, S7, fails closed separately).
        if let Some(rls) = &self.rls {
            match translate::rls_cluster_to_proto(rls) {
                Ok(proto) => cluster_named.push(NamedResource {
                    name: fp_domain::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER.to_string(),
                    any: Any {
                        type_url: CLUSTER_TYPE_URL.to_string(),
                        value: proto.encode_to_vec(),
                    },
                }),
                Err(err) => {
                    metrics::counter!("fp_xds_rls_cluster_synthesis_failures_total").increment(1);
                    tracing::error!(team = %team_id, error = %err,
                        "built-in rate_limit_cluster synthesis failed; CDS omits it");
                }
            }
        }
        let mut route_named = Vec::with_capacity(route_configs.len());
        for rc in &route_configs {
            let proto = match translate::route_config_to_proto(&rc.name, &rc.spec) {
                Ok(proto) => proto,
                Err(err) => {
                    let error = format!("route-config translation failed: {err}");
                    skip_xds_resource(team_id, "route-config", &rc.name, &error);
                    route_failures.insert(rc.name.clone(), error);
                    continue;
                }
            };
            let value = match translate::encode_route_config_deterministic(&proto) {
                Ok(value) => value,
                Err(err) => {
                    let error = format!("route-config translation failed: {err}");
                    skip_xds_resource(team_id, "route-config", &rc.name, &error);
                    route_failures.insert(rc.name.clone(), error);
                    continue;
                }
            };
            route_named.push(NamedResource {
                name: rc.name.clone(),
                any: Any {
                    type_url: ROUTE_TYPE_URL.to_string(),
                    value,
                },
            });
        }
        let mut secret_named = Vec::with_capacity(secrets.len());
        let mut secret_failures = HashMap::new();
        for secret in &secrets {
            let name = &secret.metadata.name;
            let spec = match decrypt_secret_spec(
                &secret.ciphertext,
                &secret.nonce,
                &secret.metadata.encryption_key_id,
            ) {
                Ok(spec) => spec,
                Err(e) => {
                    let error = format!("secret translation failed: {e}");
                    tracing::error!(team = %team_id, secret = %name, error = %error,
                        "skipping undecryptable SDS secret during xDS rebuild");
                    metrics::counter!("fp_xds_secret_translation_failures_total").increment(1);
                    secret_failures.insert(name.clone(), error);
                    continue;
                }
            };
            let proto = match translate::secret_to_proto(name, &spec) {
                Ok(proto) => proto,
                Err(e) => {
                    let error = format!("secret translation failed: {e}");
                    tracing::error!(team = %team_id, secret = %name, error = %error,
                        "skipping invalid SDS secret during xDS rebuild");
                    metrics::counter!("fp_xds_secret_translation_failures_total").increment(1);
                    secret_failures.insert(name.clone(), error);
                    continue;
                }
            };
            secret_named.push(NamedResource {
                name: name.clone(),
                any: Any {
                    type_url: SECRET_TYPE_URL.to_string(),
                    value: proto.encode_to_vec(),
                },
            });
        }
        let mut listener_named = Vec::with_capacity(listeners.len());
        for xds_listener in &listeners {
            let listener = &xds_listener.listener;
            // Listeners without a bound route config cannot serve; they stay out of the
            // snapshot rather than producing a NACK-able resource.
            if listener.spec.route_config.is_none() {
                tracing::debug!(team = %team_id, listener = %listener.name,
                    "skipping unbound listener in snapshot");
                continue;
            }
            if listener
                .spec
                .route_config
                .as_ref()
                .is_some_and(|name| !route_configs.iter().any(|rc| rc.name == *name))
            {
                let error = "listener references an unavailable route config".to_string();
                skip_xds_resource(team_id, "listener", &listener.name, &error);
                listener_failures.insert(listener.name.clone(), error);
                continue;
            }
            let route_config_id = listener
                .spec
                .route_config
                .as_ref()
                .and_then(|name| route_configs.iter().find(|rc| rc.name == *name))
                .map(|rc| rc.id);
            let captures = route_config_id
                .map(|id| {
                    capture_plan
                        .iter()
                        .filter(|capture| {
                            capture.route_config_id == id.as_uuid()
                                && capture
                                    .listener_id
                                    .is_none_or(|scope| scope == listener.id.as_uuid())
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let ai_metadata = route_config_id
                .filter(|_| xds_listener.owner_kind == "ai")
                .map(|route_config_id| translate::AiProcessorMetadata {
                    team_id: team_id.as_uuid(),
                    listener_id: listener.id.as_uuid(),
                    route_config_id: route_config_id.as_uuid(),
                });
            let proto = match translate::listener_to_proto_with_learning_and_ai(
                &listener.name,
                &listener.spec,
                &captures,
                ai_metadata.as_ref(),
            ) {
                Ok(proto) => proto,
                Err(err) => {
                    let error = format!("listener translation failed: {err}");
                    skip_xds_resource(team_id, "listener", &listener.name, &error);
                    listener_failures.insert(listener.name.clone(), error);
                    continue;
                }
            };
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
            changed |= entry
                .clusters
                .install_raw_with_failures(cluster_named, cluster_failures);
            changed |= entry.endpoints.install_raw(endpoint_named);
            changed |= entry
                .routes
                .install_raw_with_failures(route_named, route_failures);
            changed |= entry
                .listeners
                .install_raw_with_failures(listener_named, listener_failures);
            changed |= entry
                .secrets
                .install_raw_with_failures(secret_named, secret_failures);
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

struct XdsResources {
    clusters: Vec<XdsCluster>,
    route_configs: Vec<RouteConfig>,
    listeners: Vec<XdsListener>,
    cluster_failures: HashMap<String, String>,
    route_failures: HashMap<String, String>,
    listener_failures: HashMap<String, String>,
}

struct XdsCluster {
    cluster: Cluster,
    ai: Option<translate::AiUpstreamProcessorMetadata>,
}

struct XdsListener {
    listener: Listener,
    owner_kind: String,
}

async fn load_xds_resources(pool: &PgPool, team_id: TeamId) -> DomainResult<XdsResources> {
    let ai_clusters = ai_cluster_metadata(pool, team_id).await?;
    let cluster_rows = sqlx::query(
        "SELECT id, team_id, name, spec, version, created_at, updated_at \
         FROM clusters WHERE team_id = $1 ORDER BY name LIMIT 500",
    )
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|err| DomainError::internal(format!("list xDS clusters: {err}")))?;
    let route_rows = sqlx::query(
        "SELECT id, team_id, name, spec, version, created_at, updated_at \
         FROM route_configs WHERE team_id = $1 ORDER BY name LIMIT 500",
    )
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|err| DomainError::internal(format!("list xDS route configs: {err}")))?;
    let listener_rows = sqlx::query(
        "SELECT id, team_id, name, spec, version, created_at, updated_at, owner_kind \
         FROM listeners WHERE team_id = $1 ORDER BY name LIMIT 500",
    )
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|err| DomainError::internal(format!("list xDS listeners: {err}")))?;

    let mut clusters = Vec::with_capacity(cluster_rows.len());
    let mut cluster_failures = HashMap::new();
    for row in cluster_rows {
        let name: String = row.get("name");
        match cluster_from_xds_row(&row) {
            Ok(cluster) => clusters.push(XdsCluster {
                ai: ai_clusters.get(&cluster.name).cloned(),
                cluster,
            }),
            Err(error) => {
                skip_xds_resource(team_id, "cluster", &name, &error);
                cluster_failures.insert(name, error);
            }
        }
    }

    let mut route_configs = Vec::with_capacity(route_rows.len());
    let mut route_failures = HashMap::new();
    for row in route_rows {
        let name: String = row.get("name");
        match route_config_from_xds_row(&row) {
            Ok(route_config) => route_configs.push(route_config),
            Err(error) => {
                skip_xds_resource(team_id, "route-config", &name, &error);
                route_failures.insert(name, error);
            }
        }
    }

    let mut listeners = Vec::with_capacity(listener_rows.len());
    let mut listener_failures = HashMap::new();
    for row in listener_rows {
        let name: String = row.get("name");
        match listener_from_xds_row(&row) {
            Ok(listener) => listeners.push(XdsListener {
                listener,
                owner_kind: row.get("owner_kind"),
            }),
            Err(error) => {
                skip_xds_resource(team_id, "listener", &name, &error);
                listener_failures.insert(name, error);
            }
        }
    }

    Ok(XdsResources {
        clusters,
        route_configs,
        listeners,
        cluster_failures,
        route_failures,
        listener_failures,
    })
}

async fn ai_cluster_metadata(
    pool: &PgPool,
    team_id: TeamId,
) -> DomainResult<HashMap<String, translate::AiUpstreamProcessorMetadata>> {
    let rows = sqlx::query(
        "SELECT r.cluster_names[b.position + 1] AS cluster_name, rc.id AS route_config_id, \
                b.provider_id, b.position \
         FROM ai_routes r \
         JOIN ai_route_backends b ON b.team_id = r.team_id AND b.ai_route_id = r.id \
         JOIN route_configs rc ON rc.team_id = r.team_id AND rc.name = r.route_config_name \
         WHERE r.team_id = $1",
    )
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|err| DomainError::internal(format!("list AI cluster metadata: {err}")))?;
    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        // `cluster_names[position + 1]` is NULL when a backend position has no matching
        // materialized cluster name (short/misaligned cluster_names array). That is a data
        // gap for an incompletely materialized route, not a fatal condition — skip it and
        // keep priming, mirroring the undecryptable-secret resilience elsewhere in rebuild.
        // Row::get would panic on the NULL, taking down snapshot priming (and startup).
        let cluster_name: Option<String> = row.try_get("cluster_name").map_err(|err| {
            DomainError::internal(format!("decode AI cluster metadata cluster_name: {err}"))
        })?;
        let Some(cluster_name) = cluster_name else {
            let position: i32 = row.try_get("position").unwrap_or_default();
            tracing::warn!(
                team = %team_id,
                position,
                "skipping AI cluster metadata: backend position has no materialized cluster name"
            );
            continue;
        };
        out.insert(
            cluster_name,
            translate::AiUpstreamProcessorMetadata {
                team_id: team_id.as_uuid(),
                route_config_id: RouteConfigId::from(row.get::<uuid::Uuid, _>("route_config_id"))
                    .as_uuid(),
                provider_id: AiProviderId::from(row.get::<uuid::Uuid, _>("provider_id")).as_uuid(),
                backend_position: row.get("position"),
                failover_chain: Vec::new(),
            },
        );
    }
    let aggregate_rows =
        sqlx::query("SELECT name, spec FROM clusters WHERE team_id = $1 AND owner_kind = 'ai'")
            .bind(team_id.as_uuid())
            .fetch_all(pool)
            .await
            .map_err(|err| DomainError::internal(format!("list AI aggregate clusters: {err}")))?;
    for row in aggregate_rows {
        let name: String = row.get("name");
        let spec: ClusterSpec = serde_json::from_value(row.get::<serde_json::Value, _>("spec"))
            .map_err(|err| {
                DomainError::internal(format!("AI aggregate cluster spec does not parse: {err}"))
            })?;
        if spec.aggregate_clusters.is_empty() {
            continue;
        }
        let mut chain = Vec::with_capacity(spec.aggregate_clusters.len());
        let mut route_config_id = None;
        for member in &spec.aggregate_clusters {
            let Some(metadata) = out.get(member) else {
                chain.clear();
                break;
            };
            route_config_id = Some(metadata.route_config_id);
            chain.push((metadata.provider_id, metadata.backend_position));
        }
        if let (Some(route_config_id), Some((provider_id, backend_position))) =
            (route_config_id, chain.first().copied())
        {
            out.insert(
                name,
                translate::AiUpstreamProcessorMetadata {
                    team_id: team_id.as_uuid(),
                    route_config_id,
                    provider_id,
                    backend_position,
                    failover_chain: chain,
                },
            );
        }
    }
    Ok(out)
}

fn cluster_from_xds_row(row: &sqlx::postgres::PgRow) -> Result<Cluster, String> {
    let spec = serde_json::from_value::<ClusterSpec>(row.get::<serde_json::Value, _>("spec"))
        .map_err(|err| format!("cluster spec in DB does not parse: {err}"))?;
    Ok(Cluster {
        id: ClusterId::from(row.get::<uuid::Uuid, _>("id")),
        team_id: TeamId::from(row.get::<uuid::Uuid, _>("team_id")),
        name: row.get("name"),
        spec,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn route_config_from_xds_row(row: &sqlx::postgres::PgRow) -> Result<RouteConfig, String> {
    let spec = serde_json::from_value::<RouteConfigSpec>(row.get::<serde_json::Value, _>("spec"))
        .map_err(|err| format!("route-config spec in DB does not parse: {err}"))?;
    Ok(RouteConfig {
        id: RouteConfigId::from(row.get::<uuid::Uuid, _>("id")),
        team_id: TeamId::from(row.get::<uuid::Uuid, _>("team_id")),
        name: row.get("name"),
        spec,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn listener_from_xds_row(row: &sqlx::postgres::PgRow) -> Result<Listener, String> {
    let spec = serde_json::from_value::<ListenerSpec>(row.get::<serde_json::Value, _>("spec"))
        .map_err(|err| format!("listener spec in DB does not parse: {err}"))?;
    Ok(Listener {
        id: ListenerId::from(row.get::<uuid::Uuid, _>("id")),
        team_id: TeamId::from(row.get::<uuid::Uuid, _>("team_id")),
        name: row.get("name"),
        spec,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn skip_xds_resource(team_id: TeamId, kind: &'static str, name: &str, error: &str) {
    tracing::error!(team = %team_id, resource_kind = kind, resource = %name, error = %error,
        "skipping invalid resource during xDS rebuild");
    metrics::counter!("fp_xds_resource_translation_failures_total", "resource_kind" => kind)
        .increment(1);
}

async fn learning_capture_plan(
    pool: &PgPool,
    team_id: TeamId,
    route_configs: &[fp_domain::gateway::route_config::RouteConfig],
) -> DomainResult<Vec<translate::LearningCaptureInjection>> {
    let route_config_ids = route_configs
        .iter()
        .map(|rc| (rc.id, rc.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let route_config_names = route_configs
        .iter()
        .map(|rc| (rc.name.as_str(), rc.id))
        .collect::<BTreeMap<_, _>>();
    let sessions =
        fp_storage::repos::api_lifecycle::list_capturing_capture_sessions(pool, team_id).await?;
    let mut captures = Vec::new();
    for session in sessions {
        if let Some(route_config_id) = session.route_config_id {
            if route_config_ids.contains_key(&route_config_id) {
                captures.push(translate::LearningCaptureInjection {
                    session_id: session.id.as_uuid(),
                    team_id: team_id.as_uuid(),
                    api_definition_id: session.api_definition_id.map(|id| id.as_uuid()),
                    route_config_id: route_config_id.as_uuid(),
                    listener_id: session.listener_id.map(|id| id.as_uuid()),
                    virtual_host: session.virtual_host.clone(),
                    route: session.route.clone(),
                    discovery: None,
                });
            }
            continue;
        }
        let Some(api_id) = session.api_definition_id else {
            continue;
        };
        let bindings =
            fp_storage::repos::api_lifecycle::list_route_bindings_for_api(pool, team_id, api_id)
                .await?;
        for binding in bindings {
            if route_config_ids.contains_key(&binding.route_config_id) {
                captures.push(translate::LearningCaptureInjection {
                    session_id: session.id.as_uuid(),
                    team_id: team_id.as_uuid(),
                    api_definition_id: Some(api_id.as_uuid()),
                    route_config_id: binding.route_config_id.as_uuid(),
                    listener_id: binding.listener_id.map(|id| id.as_uuid()),
                    virtual_host: binding.virtual_host.clone(),
                    route: binding.route.clone(),
                    discovery: None,
                });
            }
        }
    }
    let (discovery_sessions, _) = fp_storage::repos::discovery::list(
        pool,
        team_id,
        Some(fp_domain::discovery::DiscoverySessionStatus::Capturing),
        500,
        0,
    )
    .await?;
    for session in discovery_sessions {
        let Some(route_config_id) = route_config_names.get(session.route_config_name.as_str())
        else {
            continue;
        };
        let listener_id: Option<ListenerId> = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT id FROM listeners WHERE team_id = $1 AND name = $2 AND owner_kind = 'discovery'",
        )
        .bind(team_id.as_uuid())
        .bind(&session.listener_name)
        .fetch_optional(pool)
        .await
        .map_err(|err| DomainError::internal(format!("get discovery listener id: {err}")))?
        .map(ListenerId::from);
        captures.push(translate::LearningCaptureInjection {
            session_id: session.id.as_uuid(),
            team_id: team_id.as_uuid(),
            api_definition_id: None,
            route_config_id: route_config_id.as_uuid(),
            listener_id: listener_id.map(|id| id.as_uuid()),
            virtual_host: None,
            route: None,
            discovery: Some(translate::DiscoveryCaptureMetadata {
                forwarded_upstream_host: session.upstream_host,
                forwarded_upstream_port: session.upstream_port,
                forwarded_upstream_ip: session.validated_upstream_ip,
                forwarded_upstream_tls: session.upstream_tls,
            }),
        });
    }
    captures.sort_by(|a, b| {
        (
            a.route_config_id,
            a.listener_id,
            a.virtual_host.as_deref(),
            a.route.as_deref(),
            a.session_id,
        )
            .cmp(&(
                b.route_config_id,
                b.listener_id,
                b.virtual_host.as_deref(),
                b.route.as_deref(),
                b.session_id,
            ))
    });
    captures.dedup();
    Ok(captures)
}

pub(crate) fn decrypt_secret_spec(
    ciphertext: &[u8],
    nonce: &[u8],
    key_id: &str,
) -> DomainResult<SecretSpec> {
    let key = keyring_secret_key(key_id)?;
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

const DEFAULT_KEY_ID: &str = "default";
const ACTIVE_KEY_ID_ENV: &str = "FLOWPLANE_SECRET_ENCRYPTION_KEY_ID";
const ACTIVE_KEY_ENV: &str = "FLOWPLANE_SECRET_ENCRYPTION_KEY";
const KEYRING_ENV: &str = "FLOWPLANE_SECRET_ENCRYPTION_KEYS";

#[cfg(test)]
pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SecretKey {
    id: String,
    bytes: [u8; 32],
}

fn active_secret_key() -> DomainResult<SecretKey> {
    let id = active_key_id()?;
    let raw = std::env::var(ACTIVE_KEY_ENV).map_err(|_| {
        DomainError::unavailable("secret encryption key is not configured")
            .with_hint("set FLOWPLANE_SECRET_ENCRYPTION_KEY to a 32-byte or base64-encoded key")
    })?;
    Ok(SecretKey {
        id,
        bytes: parse_secret_key(ACTIVE_KEY_ENV, &raw)?,
    })
}

fn active_key_id() -> DomainResult<String> {
    let id = std::env::var(ACTIVE_KEY_ID_ENV).unwrap_or_else(|_| DEFAULT_KEY_ID.to_string());
    validate_key_id(&id)?;
    Ok(id)
}

fn keyring_secret_key(key_id: &str) -> DomainResult<[u8; 32]> {
    validate_key_id(key_id)?;
    let active = active_secret_key()?;
    if active.id == key_id {
        return Ok(active.bytes);
    }
    let raw = std::env::var(KEYRING_ENV).map_err(|_| {
        DomainError::unavailable(format!(
            "secret encryption key \"{key_id}\" is not configured"
        ))
        .with_hint(
            "keep retired keys in FLOWPLANE_SECRET_ENCRYPTION_KEYS until old secrets are rotated",
        )
    })?;
    let values: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).map_err(|e| {
            DomainError::invalid_config(format!("{KEYRING_ENV} must be a JSON object: {e}"))
        })?;
    let Some(value) = values.get(key_id).and_then(|value| value.as_str()) else {
        return Err(DomainError::unavailable(format!(
            "secret encryption key \"{key_id}\" is not configured"
        ))
        .with_hint(
            "add the retired key to FLOWPLANE_SECRET_ENCRYPTION_KEYS or rotate the secret",
        ));
    };
    parse_secret_key(&format!("{KEYRING_ENV}.{key_id}"), value)
}

fn validate_key_id(id: &str) -> DomainResult<()> {
    if id.is_empty() || id.len() > 128 || id.chars().any(|c| c.is_control() || c == '\0') {
        return Err(DomainError::invalid_config(format!(
            "{ACTIVE_KEY_ID_ENV} must be 1..=128 printable characters"
        )));
    }
    Ok(())
}

fn parse_secret_key(label: &str, raw: &str) -> DomainResult<[u8; 32]> {
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    <[u8; 32]>::try_from(raw.as_bytes()).map_err(|_| {
        DomainError::invalid_config(format!("{label} must be exactly 32 bytes after decoding"))
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
    use envoy_types::pb::envoy::config::common::mutation_rules::v3 as mutation_rules;
    use envoy_types::pb::envoy::config::core::v3 as envoy_core;
    use envoy_types::pb::envoy::config::listener::v3 as lst;
    use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3 as ext_proc;
    use envoy_types::pb::envoy::extensions::filters::http::router::v3::Router;
    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3 as hcm;
    use envoy_types::pb::envoy::r#type::matcher::v3 as matcher_type;
    use envoy_types::pb::google::protobuf as wkt;
    use fp_core::services::egress_policy::EgressPolicy;
    use fp_core::{GrantSet, PrincipalCtx};
    use fp_domain::api_lifecycle::{ApiDefinitionSpec, ApiRouteBindingSpec, CaptureSessionSpec};
    use fp_domain::authz::TeamRef;
    use fp_domain::event::{DomainEvent, EventScope};
    use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
    use fp_domain::gateway::listener::ListenerSpec;
    use fp_domain::gateway::route_config::{
        PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
    };
    use fp_domain::{OrgRole, RequestId, SecretSpec, SecretType};
    use fp_storage::repos::identity;
    use prost::Message;
    use std::net::{IpAddr, SocketAddr};
    fn unique(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            &uuid::Uuid::now_v7().simple().to_string()[20..]
        )
    }

    fn cluster_spec(host: &str) -> ClusterSpec {
        ClusterSpec {
            aggregate_clusters: Vec::new(),
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

    fn hermetic_cluster_policy(specs: &[ClusterSpec]) -> EgressPolicy {
        let allowed = specs
            .iter()
            .flat_map(|spec| {
                spec.endpoints.iter().filter_map(|endpoint| {
                    endpoint
                        .host
                        .parse::<IpAddr>()
                        .ok()
                        .map(|ip| SocketAddr::new(ip, endpoint.port))
                })
            })
            .collect();
        EgressPolicy::with_allowed(Vec::new(), allowed)
    }

    // ---------------- S6 built-in rate_limit_cluster injection (separate author) ----------------
    //
    // DB-backed: a `with_rls(Some(..))` cache must inject a CDS resource named
    // "rate_limit_cluster" into a team's snapshot; `new()` / `with_rls(None)` must not. This is
    // the cheap real path (create one team, rebuild_team with no gateway resources) rather than
    // the full event-driven `world()` harness — the injection happens unconditionally in
    // rebuild_team when `rls` is set, so an empty team is sufficient to observe it. Skips when
    // FLOWPLANE_TEST_DATABASE_URL is unset.

    /// Create one org+team and return (pool, team_id). None when the DB env var is unset.
    async fn one_team() -> Option<(PgPool, TeamId)> {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return None;
        };
        let pool = fp_storage::connect(&url, 8).await.expect("connect");
        fp_storage::migrate(&pool).await.expect("migrate");
        let org = identity::create_org(&pool, &unique("org"), "")
            .await
            .expect("org");
        let team = identity::create_team(&pool, org.id, &unique("team"), "")
            .await
            .expect("team");
        Some((pool, team.id))
    }

    /// Decode each cluster Any in a snapshot and return whether any is named `wanted`.
    fn snapshot_has_cluster(snap: &TeamSnapshot, wanted: &str) -> bool {
        snap.clusters.resources.iter().any(|any| {
            assert_eq!(
                any.type_url, CLUSTER_TYPE_URL,
                "cluster set holds cluster Anys"
            );
            let cluster =
                envoy_types::pb::envoy::config::cluster::v3::Cluster::decode(any.value.as_slice())
                    .expect("decode cluster proto");
            cluster.name == wanted
        })
    }

    fn rls_cfg() -> translate::RlsClusterConfig {
        translate::RlsClusterConfig {
            grpc_url: "rls.internal:8081".into(),
            tls: None,
        }
    }

    #[tokio::test]
    async fn with_rls_injects_rate_limit_cluster_into_team_cds() {
        let Some((pool, team_id)) = one_team().await else {
            return;
        };
        let cache = SnapshotCache::with_rls(Some(rls_cfg()));
        cache.rebuild_team(&pool, team_id).await.expect("rebuild");
        let snap = cache.team(team_id).await;
        assert!(
            snapshot_has_cluster(&snap, "rate_limit_cluster"),
            "with_rls(Some(..)) must inject a CDS resource named rate_limit_cluster"
        );
    }

    #[tokio::test]
    async fn new_cache_does_not_inject_rate_limit_cluster() {
        let Some((pool, team_id)) = one_team().await else {
            return;
        };
        let cache = SnapshotCache::new();
        cache.rebuild_team(&pool, team_id).await.expect("rebuild");
        let snap = cache.team(team_id).await;
        assert!(
            !snapshot_has_cluster(&snap, "rate_limit_cluster"),
            "SnapshotCache::new() must NOT inject the built-in rate_limit_cluster"
        );
    }

    #[tokio::test]
    async fn with_rls_none_does_not_inject_rate_limit_cluster() {
        let Some((pool, team_id)) = one_team().await else {
            return;
        };
        let cache = SnapshotCache::with_rls(None);
        cache.rebuild_team(&pool, team_id).await.expect("rebuild");
        let snap = cache.team(team_id).await;
        assert!(
            !snapshot_has_cluster(&snap, "rate_limit_cluster"),
            "with_rls(None) must behave like new(): no injected cluster"
        );
    }

    #[tokio::test]
    async fn decrypt_secret_spec_reads_retired_key_from_keyring() {
        let _guard = ENV_LOCK.lock().await;
        let retired_key = *b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        std::env::set_var("FLOWPLANE_SECRET_ENCRYPTION_KEY_ID", "v2");
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEYS",
            serde_json::json!({"v1": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}).to_string(),
        );

        let spec = SecretSpec::GenericSecret {
            secret: "c2VjcmV0".into(),
        };
        let nonce = [7_u8; 12];
        let plaintext = serde_json::to_vec(&spec).expect("secret json");
        let cipher = Aes256Gcm::new_from_slice(&retired_key).expect("cipher");
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
            .expect("encrypt");

        let decoded = decrypt_secret_spec(&ciphertext, &nonce, "v1").expect("decrypt");
        assert_eq!(decoded, spec);
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
                        direct_response: None,
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
        fp_storage::outbox::register_consumer_at_head(&pool, &consumer)
            .await
            .expect("register");

        // Team A: cluster + bound route config + listener. Team B: one cluster only.
        let upstream = unique("upstream");
        let upstream_spec = cluster_spec("10.0.0.1");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_a,
            team_a,
            &upstream,
            upstream_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[upstream_spec]),
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
                public_base_url: None,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(rc.clone()),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
            false,
        )
        .await
        .expect("a listener");
        let other_spec = cluster_spec("10.9.9.9");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_b,
            team_b,
            &unique("other"),
            other_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[other_spec]),
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
        let update_spec = cluster_spec("10.0.0.2");
        fp_core::services::clusters::update_cluster_with_egress_policy(
            &pool,
            &ctx_a,
            team_a,
            &upstream,
            update_spec.clone(),
            1,
            RequestId::generate(),
            &hermetic_cluster_policy(&[update_spec]),
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
        fresh.rebuild_team(&pool, team_a.id).await.expect("prime");
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
    async fn learning_capture_injection_is_scoped_to_the_session_team() {
        let Some((pool, team_a, team_b, ctx_a, ctx_b)) = world().await else {
            return;
        };
        let cluster_a = unique("upstream-a");
        let cluster_b = unique("upstream-b");
        let cluster_a_spec = cluster_spec("10.0.0.1");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_a,
            team_a,
            &cluster_a,
            cluster_a_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[cluster_a_spec]),
        )
        .await
        .expect("a cluster");
        let cluster_b_spec = cluster_spec("10.0.0.2");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_b,
            team_b,
            &cluster_b,
            cluster_b_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[cluster_b_spec]),
        )
        .await
        .expect("b cluster");
        let route_name = unique("shared-routes");
        let listener_name = unique("edge");
        let rc_a = fp_core::services::gateway::create_route_config(
            &pool,
            &ctx_a,
            team_a,
            &route_name,
            rc_spec(&cluster_a),
            RequestId::generate(),
        )
        .await
        .expect("a rc");
        fp_core::services::gateway::create_route_config(
            &pool,
            &ctx_b,
            team_b,
            &route_name,
            rc_spec(&cluster_b),
            RequestId::generate(),
        )
        .await
        .expect("b rc");
        let listener_a = fp_core::services::gateway::create_listener(
            &pool,
            &ctx_a,
            team_a,
            &listener_name,
            ListenerSpec {
                address: "0.0.0.0".into(),
                port: 19300,
                public_base_url: None,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(route_name.clone()),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
            false,
        )
        .await
        .expect("a listener");
        fp_core::services::gateway::create_listener(
            &pool,
            &ctx_b,
            team_b,
            &listener_name,
            ListenerSpec {
                address: "0.0.0.0".into(),
                port: 19301,
                public_base_url: None,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(route_name),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
            false,
        )
        .await
        .expect("b listener");

        let mut tx = pool.begin().await.expect("tx");
        let api = fp_storage::repos::api_lifecycle::create_api_definition(
            &mut tx,
            team_a,
            &unique("learn-api"),
            &ApiDefinitionSpec {
                display_name: "Learn".into(),
                description: String::new(),
            },
        )
        .await
        .expect("api");
        fp_storage::repos::api_lifecycle::create_route_binding(
            &mut tx,
            team_a,
            api.id,
            &unique("binding"),
            &ApiRouteBindingSpec {
                route_config_id: rc_a.id,
                listener_id: Some(listener_a.id),
                virtual_host: Some("default".into()),
                route: Some("all".into()),
            },
        )
        .await
        .expect("binding");
        fp_storage::repos::api_lifecycle::create_capture_session(
            &mut tx,
            team_a,
            &unique("capture"),
            &CaptureSessionSpec {
                api_definition_id: Some(api.id),
                route_config_id: None,
                listener_id: None,
                virtual_host: None,
                route: None,
                target_sample_count: 10,
                max_duration_seconds: Some(60),
                max_bytes: 4096,
                max_distinct_paths: 10,
            },
        )
        .await
        .expect("session");
        tx.commit().await.expect("commit");

        let cache = SnapshotCache::new();
        cache
            .rebuild_team(&pool, team_a.id)
            .await
            .expect("a rebuild");
        cache
            .rebuild_team(&pool, team_b.id)
            .await
            .expect("b rebuild");
        let a = cache.team(team_a.id).await;
        let b = cache.team(team_b.id).await;
        assert_eq!(a.listeners.resources.len(), 1);
        assert_eq!(b.listeners.resources.len(), 1);

        let hcm_a = decode_hcm(&a.listeners.resources[0]);
        let hcm_b = decode_hcm(&b.listeners.resources[0]);
        assert_eq!(learning_filter_count(&hcm_a), 1);
        assert_eq!(learning_filter_count(&hcm_b), 0);
        assert_eq!(hcm_a.access_log.len(), 1);
        assert!(hcm_b.access_log.is_empty());
    }

    #[tokio::test]
    async fn undecryptable_secret_degrades_only_that_secret_and_does_not_block_outbox() {
        let _guard = ENV_LOCK.lock().await;
        let Some((pool, team_a, team_b, ctx_a, ctx_b)) = world().await else {
            return;
        };
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "12345678901234567890123456789012",
        );
        let cache = SnapshotCache::new();
        let consumer = format!("xds-test-{}", unique("c"));
        fp_storage::outbox::register_consumer_at_head(&pool, &consumer)
            .await
            .expect("register");

        fp_core::services::secrets::create_secret(
            &pool,
            &ctx_a,
            team_a,
            fp_core::services::secrets::SecretWrite {
                name: "good-secret",
                description: "",
                spec: SecretSpec::GenericSecret {
                    secret: "aGVsbG8=".into(),
                },
                expires_at: None,
            },
            RequestId::generate(),
        )
        .await
        .expect("good secret");

        let bad_secret = {
            let mut tx = pool.begin().await.expect("tx");
            let secret = fp_storage::repos::secrets::create_secret(
                &mut tx,
                team_a,
                "bad-secret",
                "",
                SecretType::GenericSecret,
                b"not-a-valid-ciphertext",
                &[0; 12],
                "stale-key",
                None,
            )
            .await
            .expect("bad secret row");
            fp_storage::outbox::append(
                &mut tx,
                &DomainEvent::SecretUpserted {
                    secret_id: secret.id.as_uuid(),
                    name: secret.name.clone(),
                },
                EventScope {
                    org_id: Some(team_a.org_id),
                    team_id: Some(team_a.id),
                },
                serde_json::json!({}),
            )
            .await
            .expect("bad secret event");
            tx.commit().await.expect("commit");
            secret
        };

        let other_spec = cluster_spec("10.9.9.9");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_b,
            team_b,
            &unique("other"),
            other_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[other_spec]),
        )
        .await
        .expect("b cluster");

        let processed = fp_storage::outbox::process_batch(&pool, &consumer, 100, |events| {
            let cache = cache.clone();
            let pool = pool.clone();
            async move { handle_events(&cache, &pool, events).await }
        })
        .await
        .expect("poisoned batch must not fail");
        assert!(processed >= 3);

        let snap_a = cache.team(team_a.id).await;
        assert_eq!(snap_a.secrets.resources.len(), 1);
        let snap_b = cache.team(team_b.id).await;
        assert_eq!(snap_b.clusters.resources.len(), 1);

        let degraded = cache.degraded(team_a.id).await;
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].type_url, SECRET_TYPE_URL);
        assert_eq!(degraded[0].name, bad_secret.name);
        assert!(degraded[0].error.contains("secret translation failed"));
    }

    #[tokio::test]
    async fn malformed_route_config_degrades_without_blocking_rebuild_or_outbox() {
        let Some((pool, team_a, team_b, ctx_a, ctx_b)) = world().await else {
            return;
        };
        let cache = SnapshotCache::new();
        let consumer = format!("xds-test-{}", unique("c"));
        fp_storage::outbox::register_consumer_at_head(&pool, &consumer)
            .await
            .expect("register");

        let good_cluster = unique("good-upstream");
        let good_cluster_spec = cluster_spec("10.0.0.1");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_a,
            team_a,
            &good_cluster,
            good_cluster_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[good_cluster_spec]),
        )
        .await
        .expect("good cluster");
        let good_route = fp_core::services::gateway::create_route_config(
            &pool,
            &ctx_a,
            team_a,
            &unique("good-routes"),
            rc_spec(&good_cluster),
            RequestId::generate(),
        )
        .await
        .expect("good rc");
        fp_core::services::gateway::create_listener(
            &pool,
            &ctx_a,
            team_a,
            &unique("good-edge"),
            ListenerSpec {
                address: "0.0.0.0".into(),
                port: 19210,
                public_base_url: None,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(good_route.name.clone()),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
            false,
        )
        .await
        .expect("good listener");

        let bad_route_name = unique("bad-routes");
        {
            let mut tx = pool.begin().await.expect("tx");
            let bad_route_id = RouteConfigId::generate();
            sqlx::query(
                "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
                 VALUES ($1, $2, $3, $4, '{}'::jsonb)",
            )
            .bind(bad_route_id.as_uuid())
            .bind(team_a.id.as_uuid())
            .bind(team_a.org_id.as_uuid())
            .bind(&bad_route_name)
            .execute(&mut *tx)
            .await
            .expect("bad route row");
            fp_storage::outbox::append(
                &mut tx,
                &DomainEvent::RouteConfigUpserted {
                    route_config_id: bad_route_id.as_uuid(),
                    name: bad_route_name.clone(),
                },
                EventScope {
                    org_id: Some(team_a.org_id),
                    team_id: Some(team_a.id),
                },
                serde_json::json!({}),
            )
            .await
            .expect("bad route event");
            tx.commit().await.expect("commit");
        }

        let other_spec = cluster_spec("10.9.9.9");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx_b,
            team_b,
            &unique("other"),
            other_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[other_spec]),
        )
        .await
        .expect("b cluster");

        let processed = fp_storage::outbox::process_batch(&pool, &consumer, 100, |events| {
            let cache = cache.clone();
            let pool = pool.clone();
            async move { handle_events(&cache, &pool, events).await }
        })
        .await
        .expect("malformed route-config must not poison the batch");
        assert!(processed >= 4);

        let snap_a = cache.team(team_a.id).await;
        assert_eq!(snap_a.clusters.resources.len(), 1);
        assert_eq!(snap_a.routes.resources.len(), 1);
        assert_eq!(snap_a.listeners.resources.len(), 1);
        let snap_b = cache.team(team_b.id).await;
        assert_eq!(snap_b.clusters.resources.len(), 1);

        let degraded = cache.degraded(team_a.id).await;
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].type_url, ROUTE_TYPE_URL);
        assert_eq!(degraded[0].name, bad_route_name);
        assert!(
            degraded[0]
                .error
                .contains("route-config spec in DB does not parse"),
            "{:?}",
            degraded[0]
        );

        cache
            .rebuild_team(&pool, team_a.id)
            .await
            .expect("direct rebuild also isolates malformed route config");
    }

    fn decode_hcm(any: &Any) -> hcm::HttpConnectionManager {
        let listener = lst::Listener::decode(any.value.as_slice()).expect("listener");
        match &listener.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(any)) => {
                hcm::HttpConnectionManager::decode(any.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed hcm"),
        }
    }

    fn learning_filter_count(manager: &hcm::HttpConnectionManager) -> usize {
        manager
            .http_filters
            .iter()
            .filter(|filter| {
                filter
                    .name
                    .starts_with(translate::LEARNING_EXT_PROC_FILTER_PREFIX)
            })
            .count()
    }

    #[tokio::test]
    async fn nack_quarantine_serves_last_good_until_a_fix_arrives() {
        let Some((pool, team, _, ctx, _)) = world().await else {
            return;
        };
        let cache = SnapshotCache::new();
        let upstream = unique("upstream");
        let initial_spec = cluster_spec("10.0.0.1");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx,
            team,
            &upstream,
            initial_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[initial_spec]),
        )
        .await
        .expect("cluster");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let initial = cache.team(team.id).await.endpoints;

        // A NACK on the FIRST generation cannot be attributed: nothing is quarantined
        // (never blanket-quarantine a whole type).
        let quarantined = cache
            .apply_nack(team.id, None, ENDPOINT_TYPE_URL, "first push rejected")
            .await;
        assert!(quarantined.is_empty(), "no previous generation, no blame");
        assert!(cache.degraded(team.id).await.is_empty());

        // Update the cluster, then a dataplane NACKs the new bytes: the changed resource
        // is quarantined and serving falls back to the last-good bytes.
        let update_spec = cluster_spec("10.0.0.2");
        fp_core::services::clusters::update_cluster_with_egress_policy(
            &pool,
            &ctx,
            team,
            &upstream,
            update_spec.clone(),
            1,
            RequestId::generate(),
            &hermetic_cluster_policy(&[update_spec]),
        )
        .await
        .expect("update");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let updated = cache.team(team.id).await.endpoints;
        assert_ne!(updated.resources, initial.resources);

        let quarantined = cache
            .apply_nack(
                team.id,
                None,
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
            .apply_nack(team.id, None, ENDPOINT_TYPE_URL, "still unhappy")
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
        let fix_spec = cluster_spec("10.0.0.3");
        fp_core::services::clusters::update_cluster_with_egress_policy(
            &pool,
            &ctx,
            team,
            &upstream,
            fix_spec.clone(),
            2,
            RequestId::generate(),
            &hermetic_cluster_policy(&[fix_spec]),
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

    #[tokio::test]
    async fn nack_quarantine_is_scoped_to_authenticated_dataplane() {
        let Some((pool, team, _, ctx, _)) = world().await else {
            return;
        };
        let cache = SnapshotCache::new();
        let dataplane_a = DataplaneId::generate();
        let dataplane_b = DataplaneId::generate();
        let upstream = unique("upstream");
        let initial_spec = cluster_spec("10.0.1.1");
        fp_core::services::clusters::create_cluster_with_egress_policy(
            &pool,
            &ctx,
            team,
            &upstream,
            initial_spec.clone(),
            RequestId::generate(),
            &hermetic_cluster_policy(&[initial_spec]),
        )
        .await
        .expect("cluster");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let initial_a = cache
            .team_for_dataplane(team.id, Some(dataplane_a))
            .await
            .endpoints;
        let initial_b = cache
            .team_for_dataplane(team.id, Some(dataplane_b))
            .await
            .endpoints;
        assert_eq!(initial_a.resources, initial_b.resources);

        let update_spec = cluster_spec("10.0.1.2");
        fp_core::services::clusters::update_cluster_with_egress_policy(
            &pool,
            &ctx,
            team,
            &upstream,
            update_spec.clone(),
            1,
            RequestId::generate(),
            &hermetic_cluster_policy(&[update_spec]),
        )
        .await
        .expect("update");
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let updated_a = cache
            .team_for_dataplane(team.id, Some(dataplane_a))
            .await
            .endpoints;
        let updated_b = cache
            .team_for_dataplane(team.id, Some(dataplane_b))
            .await
            .endpoints;
        assert_eq!(updated_a.resources, updated_b.resources);
        assert_ne!(updated_a.resources, initial_a.resources);

        let quarantined = cache
            .apply_nack(
                team.id,
                Some(dataplane_a),
                ENDPOINT_TYPE_URL,
                "dataplane A rejected endpoint update",
            )
            .await;
        assert_eq!(quarantined, vec![upstream]);

        let rolled_back_a = cache
            .team_for_dataplane(team.id, Some(dataplane_a))
            .await
            .endpoints;
        let still_updated_b = cache
            .team_for_dataplane(team.id, Some(dataplane_b))
            .await
            .endpoints;
        assert_eq!(
            rolled_back_a.resources, initial_a.resources,
            "NACKing dataplane serves last-good bytes"
        );
        assert_eq!(
            still_updated_b.resources, updated_b.resources,
            "same-team peer dataplane is not quarantined"
        );
    }

    // -------- ai-gateway-e2e-trace s1: server-owned x-request-id on AI listeners --------
    //
    // DB-backed: an AI route materialized through fp-core services (secret + provider +
    // route → owner_kind 'ai' listener row) must come out of rebuild_team with the
    // request-identity fields pinned on its HCM. Skips when FLOWPLANE_TEST_DATABASE_URL
    // is unset; ENV_LOCK serializes FLOWPLANE_SECRET_ENCRYPTION_KEY (shared process env).

    #[tokio::test]
    #[allow(deprecated)]
    async fn materialized_ai_listener_snapshot_pins_server_owned_request_identity() {
        use fp_domain::{AiProviderKind, AiProviderSpec, AiRouteBackend, AiRouteSpec};

        let _guard = ENV_LOCK.lock().await;
        let Some((pool, team, _, ctx, _)) = world().await else {
            return;
        };
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "12345678901234567890123456789012",
        );

        let secret = fp_core::services::secrets::create_secret(
            &pool,
            &ctx,
            team,
            fp_core::services::secrets::SecretWrite {
                name: &unique("ai-key"),
                description: "",
                spec: SecretSpec::GenericSecret {
                    secret: "aGVsbG8=".into(),
                },
                expires_at: None,
            },
            RequestId::generate(),
        )
        .await
        .expect("secret");
        // Provider base_url "https://upstream.example" is an unresolvable fixture host; pin it
        // to a public TEST-NET IP so the SSRF egress guard admits provider + materialization.
        let egress = EgressPolicy::with_static_hosts(
            Vec::new(),
            Vec::new(),
            vec![(
                "upstream.example".into(),
                443,
                vec!["203.0.113.10".parse::<IpAddr>().unwrap()],
            )],
        );
        let provider = fp_core::services::ai::create_provider_with_egress_policy(
            &pool,
            &ctx,
            team,
            &unique("provider"),
            AiProviderSpec {
                kind: AiProviderKind::OpenaiCompatible,
                base_url: "https://upstream.example".into(),
                path_prefix: Some("/v1".into()),
                credential_secret_id: secret.id,
                models: vec!["gpt-5".into()],
                auth_header: "authorization".into(),
            },
            RequestId::generate(),
            &egress,
        )
        .await
        .expect("provider");

        // Unique per-test listener port: this is product state in the shared test
        // PostgreSQL, not a bound socket, but uniqueness keeps parallel runs disjoint.
        let port = 20000 + (uuid::Uuid::now_v7().as_u128() % 10_000) as u16;
        let route_name = unique("ai-route");
        fp_core::services::ai::create_route_with_egress_policy(
            &pool,
            &ctx,
            team,
            &route_name,
            AiRouteSpec {
                listener_port: port,
                path: "/v1/chat/completions".into(),
                backends: vec![AiRouteBackend {
                    provider_id: provider.id,
                    models: vec!["gpt-5".into()],
                    model_override: None,
                    weight: 1,
                    priority: 0,
                }],
            },
            RequestId::generate(),
            &egress,
        )
        .await
        .expect("ai route");

        let cache = SnapshotCache::new();
        cache.rebuild_team(&pool, team.id).await.expect("rebuild");
        let snap = cache.team(team.id).await;

        let listener_name = format!("ai-{route_name}-listener");
        let listener = snap
            .listeners
            .resources
            .iter()
            .map(|any| {
                assert_eq!(any.type_url, LISTENER_TYPE_URL);
                lst::Listener::decode(any.value.as_slice()).expect("decode listener")
            })
            .find(|listener| listener.name == listener_name)
            .expect("materialized AI listener in snapshot");
        let hcm_bytes = match &listener.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(any)) => any.value.clone(),
            _ => panic!("expected typed HCM"),
        };
        let manager = hcm::HttpConnectionManager::decode(hcm_bytes.as_slice()).expect("decode hcm");

        // Pinned request-identity fields (ai-gateway-e2e-trace s1), asserted
        // explicitly for a readable failure before the whole-proto compare.
        assert!(!manager.preserve_external_request_id);
        assert_eq!(
            manager.use_remote_address,
            Some(wkt::BoolValue { value: true }),
            "AI listener HCM must set use_remote_address so requests are edge requests"
        );
        assert_eq!(
            manager.internal_address_config,
            Some(hcm::http_connection_manager::InternalAddressConfig {
                unix_sockets: false,
                cidr_ranges: vec![envoy_core::CidrRange {
                    address_prefix: "240.0.0.0".to_string(),
                    prefix_len: Some(wkt::UInt32Value { value: 4 }),
                }],
            }),
            "AI listener HCM must declare no client peer internal via a non-matching CIDR"
        );
        assert!(
            manager
                .generate_request_id
                .as_ref()
                .expect("generate_request_id")
                .value
        );
        assert!(manager.always_set_request_id_in_response);

        // The ext_proc initial metadata carries the materialized row ids.
        let listener_id: uuid::Uuid =
            sqlx::query_scalar("SELECT id FROM listeners WHERE team_id = $1 AND name = $2")
                .bind(team.id.as_uuid())
                .bind(&listener_name)
                .fetch_one(&pool)
                .await
                .expect("listener row id");
        let route_config_name = format!("ai-{route_name}-routes");
        let route_config_id: uuid::Uuid =
            sqlx::query_scalar("SELECT id FROM route_configs WHERE team_id = $1 AND name = $2")
                .bind(team.id.as_uuid())
                .bind(&route_config_name)
                .fetch_one(&pool)
                .await
                .expect("route config row id");

        // Whole-proto compare: the complete HCM expected for a materialized AI
        // listener, spelled out field-by-field so drift anywhere in the HCM —
        // not just the identity pins — fails this test.
        const EXT_PROC_TYPE_NAME: &str =
            "envoy.extensions.filters.http.ext_proc.v3.ExternalProcessor";
        const ROUTER_TYPE_NAME: &str = "envoy.extensions.filters.http.router.v3.Router";
        let expected_ext_proc = ext_proc::ExternalProcessor {
            grpc_service: Some(envoy_core::GrpcService {
                timeout: Some(wkt::Duration {
                    seconds: 5,
                    nanos: 0,
                }),
                initial_metadata: vec![
                    metadata_header("x-flowplane-ai-processor", "true".into()),
                    metadata_header("x-flowplane-team-id", team.id.as_uuid().to_string()),
                    metadata_header("x-flowplane-listener-id", listener_id.to_string()),
                    metadata_header("x-flowplane-route-config-id", route_config_id.to_string()),
                ],
                target_specifier: Some(envoy_core::grpc_service::TargetSpecifier::EnvoyGrpc(
                    envoy_core::grpc_service::EnvoyGrpc {
                        cluster_name: "xds_cluster".to_string(),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }),
            failure_mode_allow: false,
            processing_mode: Some(ext_proc::ProcessingMode {
                request_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                response_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                request_body_mode: ext_proc::processing_mode::BodySendMode::Buffered as i32,
                response_body_mode: ext_proc::processing_mode::BodySendMode::BufferedPartial as i32,
                request_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                response_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
            }),
            message_timeout: Some(wkt::Duration {
                seconds: 5,
                nanos: 0,
            }),
            stat_prefix: "flowplane_ai".to_string(),
            mutation_rules: Some(mutation_rules::HeaderMutationRules {
                allow_all_routing: Some(wkt::BoolValue { value: true }),
                allow_expression: Some(matcher_type::RegexMatcher {
                    regex: "^(authorization|x-api-key|x-flowplane-ai-model|content-length|:path)$"
                        .to_string(),
                    engine_type: Some(matcher_type::regex_matcher::EngineType::GoogleRe2(
                        matcher_type::regex_matcher::GoogleRe2::default(),
                    )),
                }),
                ..Default::default()
            }),
            observability_mode: false,
            disable_immediate_response: false,
            route_cache_action: ext_proc::external_processor::RouteCacheAction::Default as i32,
            ..Default::default()
        };
        let expected = hcm::HttpConnectionManager {
            codec_type: hcm::http_connection_manager::CodecType::Http1 as i32,
            stat_prefix: listener_name.clone(),
            route_specifier: Some(hcm::http_connection_manager::RouteSpecifier::Rds(
                hcm::Rds {
                    route_config_name,
                    config_source: Some(envoy_core::ConfigSource {
                        resource_api_version: envoy_core::ApiVersion::V3 as i32,
                        config_source_specifier: Some(
                            envoy_core::config_source::ConfigSourceSpecifier::Ads(
                                envoy_core::AggregatedConfigSource {},
                            ),
                        ),
                        ..Default::default()
                    }),
                },
            )),
            http_filters: vec![
                hcm::HttpFilter {
                    name: "envoy.filters.http.ext_proc.flowplane_ai".to_string(),
                    config_type: Some(hcm::http_filter::ConfigType::TypedConfig(wkt::Any {
                        type_url: format!("type.googleapis.com/{EXT_PROC_TYPE_NAME}"),
                        value: expected_ext_proc.encode_to_vec(),
                    })),
                    ..Default::default()
                },
                hcm::HttpFilter {
                    name: "envoy.filters.http.router".to_string(),
                    config_type: Some(hcm::http_filter::ConfigType::TypedConfig(wkt::Any {
                        type_url: format!("type.googleapis.com/{ROUTER_TYPE_NAME}"),
                        value: Router::default().encode_to_vec(),
                    })),
                    ..Default::default()
                },
            ],
            generate_request_id: Some(wkt::BoolValue { value: true }),
            always_set_request_id_in_response: true,
            use_remote_address: Some(wkt::BoolValue { value: true }),
            internal_address_config: Some(hcm::http_connection_manager::InternalAddressConfig {
                unix_sockets: false,
                cidr_ranges: vec![envoy_core::CidrRange {
                    address_prefix: "240.0.0.0".to_string(),
                    prefix_len: Some(wkt::UInt32Value { value: 4 }),
                }],
            }),
            ..Default::default()
        };
        assert_eq!(
            manager, expected,
            "materialized AI listener HCM drifted from the full expected proto"
        );
        assert_eq!(
            hcm_bytes,
            expected.encode_to_vec(),
            "materialized AI listener HCM bytes drifted (deterministic encoding)"
        );
    }

    /// Regression: an AI route whose `cluster_names` array is shorter than its backend
    /// positions produces a NULL `cluster_name` in `ai_cluster_metadata`. `Row::get` used
    /// to panic on that NULL, taking down xDS snapshot priming (and `flowplane serve`
    /// startup). Priming must degrade — skip the gap and keep the aligned backends.
    #[tokio::test]
    async fn ai_cluster_metadata_skips_backend_with_no_materialized_cluster_name() {
        use fp_domain::{AiProviderKind, AiProviderSpec};

        let _guard = ENV_LOCK.lock().await;
        let Some((pool, team, _, ctx, _)) = world().await else {
            return;
        };
        std::env::set_var(
            "FLOWPLANE_SECRET_ENCRYPTION_KEY",
            "12345678901234567890123456789012",
        );

        let secret = fp_core::services::secrets::create_secret(
            &pool,
            &ctx,
            team,
            fp_core::services::secrets::SecretWrite {
                name: &unique("ai-key"),
                description: "",
                spec: SecretSpec::GenericSecret {
                    secret: "aGVsbG8=".into(),
                },
                expires_at: None,
            },
            RequestId::generate(),
        )
        .await
        .expect("secret");
        // Provider base_url "https://upstream.example" is an unresolvable fixture host; pin it
        // to a public TEST-NET IP so the SSRF egress guard admits provider creation.
        let egress = EgressPolicy::with_static_hosts(
            Vec::new(),
            Vec::new(),
            vec![(
                "upstream.example".into(),
                443,
                vec!["203.0.113.10".parse::<IpAddr>().unwrap()],
            )],
        );
        let provider = fp_core::services::ai::create_provider_with_egress_policy(
            &pool,
            &ctx,
            team,
            &unique("provider"),
            AiProviderSpec {
                kind: AiProviderKind::OpenaiCompatible,
                base_url: "https://upstream.example".into(),
                path_prefix: Some("/v1".into()),
                credential_secret_id: secret.id,
                models: vec!["gpt-5".into()],
                auth_header: "authorization".into(),
            },
            RequestId::generate(),
            &egress,
        )
        .await
        .expect("provider");

        // Malformed materialization injected via raw SQL (the create_route builder keeps
        // cluster_names aligned with backends, so it cannot produce this gap): two backends,
        // but a single-element cluster_names, so position 1 -> cluster_names[2] -> NULL.
        let route_config_name = unique("cn-routes");
        let route_id = uuid::Uuid::now_v7();
        let aligned_cluster = unique("aligned-cluster");
        sqlx::query(
            "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
             VALUES ($1, $2, $3, $4, '{}'::jsonb)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(team.id.as_uuid())
        .bind(team.org_id.as_uuid())
        .bind(&route_config_name)
        .execute(&pool)
        .await
        .expect("route config");
        sqlx::query(
            "INSERT INTO ai_routes \
             (id, team_id, org_id, name, spec, cluster_names, route_config_name, listener_name) \
             VALUES ($1, $2, $3, $4, $5, ARRAY[$6], $7, $8)",
        )
        .bind(route_id)
        .bind(team.id.as_uuid())
        .bind(team.org_id.as_uuid())
        .bind(unique("cn-route"))
        .bind(serde_json::json!({
            "listener_port": 21000, "path": "/v1/chat/completions", "backends": []
        }))
        .bind(&aligned_cluster)
        .bind(&route_config_name)
        .bind(unique("cn-listener"))
        .execute(&pool)
        .await
        .expect("ai route");
        for position in [0_i32, 1_i32] {
            sqlx::query(
                "INSERT INTO ai_route_backends (ai_route_id, team_id, provider_id, position) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(route_id)
            .bind(team.id.as_uuid())
            .bind(provider.id.as_uuid())
            .bind(position)
            .execute(&pool)
            .await
            .expect("backend");
        }

        // Must return Ok (not panic). The team is fresh, so exactly the aligned backend
        // survives and the NULL-cluster_name backend at position 1 is skipped.
        let meta = ai_cluster_metadata(&pool, team.id)
            .await
            .expect("ai_cluster_metadata must not fail on a NULL cluster_name");
        assert_eq!(meta.len(), 1, "only the aligned backend is kept: {meta:?}");
        assert!(
            meta.contains_key(&aligned_cluster),
            "position 0's materialized cluster is kept"
        );
        assert_eq!(
            meta[&aligned_cluster].backend_position, 0,
            "the surviving entry is the aligned position-0 backend"
        );
    }

    fn metadata_header(key: &str, value: String) -> envoy_core::HeaderValue {
        envoy_core::HeaderValue {
            key: key.to_string(),
            value,
            raw_value: Vec::new(),
        }
    }
}
