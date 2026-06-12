//! Per-team xDS snapshot cache (spec/10 §5).
//!
//! Rebuilds are driven by outbox events — never by polling (kills spec/08 §2). Each team's
//! snapshot carries an independent version PER resource type, bumped only when the encoded
//! bytes actually change; ADS streams respond from this cache and never re-query the
//! database per request (kills v1's reconnect-storm fan-out, spec/04 §8.7).

use crate::translate;
use envoy_types::pb::google::protobuf::Any;
use fp_domain::{DomainResult, TeamId};
use prost::Message;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

pub const CLUSTER_TYPE_URL: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
pub const ROUTE_TYPE_URL: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
pub const LISTENER_TYPE_URL: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";

/// One resource type's serving state for one team.
#[derive(Debug, Clone, Default)]
pub struct ResourceSet {
    /// Monotonic per-type version; bumps only when `resources` bytes change.
    pub version: u64,
    /// Encoded resources, sorted by name (deterministic responses).
    pub resources: Vec<Any>,
}

#[derive(Debug, Clone, Default)]
pub struct TeamSnapshot {
    pub clusters: ResourceSet,
    pub routes: ResourceSet,
    pub listeners: ResourceSet,
}

impl TeamSnapshot {
    pub fn for_type_url(&self, type_url: &str) -> Option<&ResourceSet> {
        match type_url {
            CLUSTER_TYPE_URL => Some(&self.clusters),
            ROUTE_TYPE_URL => Some(&self.routes),
            LISTENER_TYPE_URL => Some(&self.listeners),
            _ => None,
        }
    }
}

/// The cache: team → snapshot, plus a change signal streams can await.
pub struct SnapshotCache {
    snapshots: RwLock<HashMap<TeamId, TeamSnapshot>>,
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
            .cloned()
            .unwrap_or_default()
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

        let mut cluster_any = Vec::with_capacity(clusters.len());
        for cluster in &clusters {
            let proto = translate::cluster_to_proto(&cluster.name, &cluster.spec)?;
            cluster_any.push(Any {
                type_url: CLUSTER_TYPE_URL.to_string(),
                value: proto.encode_to_vec(),
            });
        }
        let mut route_any = Vec::with_capacity(route_configs.len());
        for rc in &route_configs {
            let proto = translate::route_config_to_proto(&rc.name, &rc.spec)?;
            route_any.push(Any {
                type_url: ROUTE_TYPE_URL.to_string(),
                value: proto.encode_to_vec(),
            });
        }
        let mut listener_any = Vec::with_capacity(listeners.len());
        for listener in &listeners {
            // Listeners without a bound route config cannot serve; they stay out of the
            // snapshot rather than producing a NACK-able resource.
            if listener.spec.route_config.is_none() {
                tracing::debug!(team = %team_id, listener = %listener.name,
                    "skipping unbound listener in snapshot");
                continue;
            }
            let proto = translate::listener_to_proto(&listener.name, &listener.spec)?;
            listener_any.push(Any {
                type_url: LISTENER_TYPE_URL.to_string(),
                value: proto.encode_to_vec(),
            });
        }

        let mut changed = false;
        {
            let mut snapshots = self.snapshots.write().await;
            let entry = snapshots.entry(team_id).or_default();
            for (set, fresh) in [
                (&mut entry.clusters, cluster_any),
                (&mut entry.routes, route_any),
                (&mut entry.listeners, listener_any),
            ] {
                if set.resources != fresh {
                    set.resources = fresh;
                    set.version += 1;
                    changed = true;
                }
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
            connect_timeout_secs: 5,
            use_tls: false,
            health_check: None,
            circuit_breaker: None,
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
                    action: RouteAction {
                        cluster: cluster.into(),
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                    },
                }],
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
                route_config: Some(rc.clone()),
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

        // A real update bumps ONLY the cluster set version.
        let routes_version = again.routes.version;
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
            after.clusters.version,
            cluster_version + 1,
            "cluster version bumped"
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
}
