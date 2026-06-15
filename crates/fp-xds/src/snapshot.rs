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
use fp_domain::{DomainError, DomainResult, SecretSpec, TeamId};
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
    /// Resources skipped during control-plane translation before Envoy ever saw them.
    translation_failures: HashMap<String, String>,
    served: Vec<NamedResource>,
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
        self.quarantine.retain(|name, q| {
            raw.iter()
                .any(|r| r.name == *name && r.any.value == q.offending)
        });
        self.recompute_served() || failures_changed
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
        out.insert(
            row.get("cluster_name"),
            translate::AiUpstreamProcessorMetadata {
                team_id: team_id.as_uuid(),
                route_config_id: RouteConfigId::from(row.get::<uuid::Uuid, _>("route_config_id"))
                    .as_uuid(),
                provider_id: AiProviderId::from(row.get::<uuid::Uuid, _>("provider_id")).as_uuid(),
                backend_position: row.get("position"),
            },
        );
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
    use envoy_types::pb::envoy::config::listener::v3 as lst;
    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3 as hcm;
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn decrypt_secret_spec_reads_retired_key_from_keyring() {
        let _guard = ENV_LOCK.lock().expect("env lock");
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
                access_logs: Vec::new(),
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
        fp_core::services::clusters::create_cluster(
            &pool,
            &ctx_a,
            team_a,
            &cluster_a,
            cluster_spec("10.0.0.1"),
            RequestId::generate(),
        )
        .await
        .expect("a cluster");
        fp_core::services::clusters::create_cluster(
            &pool,
            &ctx_b,
            team_b,
            &cluster_b,
            cluster_spec("10.0.0.2"),
            RequestId::generate(),
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
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(route_name.clone()),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
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
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(route_name),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
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
        fp_core::services::clusters::create_cluster(
            &pool,
            &ctx_a,
            team_a,
            &good_cluster,
            cluster_spec("10.0.0.1"),
            RequestId::generate(),
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
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(good_route.name.clone()),
                tls_context: None,
                http_filters: Vec::new(),
                access_logs: Vec::new(),
            },
            RequestId::generate(),
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
