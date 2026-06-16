//! ADS stream integration: a real gRPC client subscribes, receives the snapshot, then a
//! config change flows mutation → outbox → snapshot rebuild → push on the open stream.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use envoy_types::pb::envoy::config::core::v3::Node;
use envoy_types::pb::envoy::service::discovery::v3::aggregated_discovery_service_client::AggregatedDiscoveryServiceClient;
use envoy_types::pb::envoy::service::discovery::v3::DiscoveryRequest;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::{OrgRole, RequestId};
use fp_storage::repos::identity;
use fp_xds::ads::NodeIdTeamResolver;
use fp_xds::snapshot::{handle_events, SnapshotCache, CLUSTER_TYPE_URL, ROUTE_TYPE_URL};
use std::sync::Arc;
use std::time::Duration;

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

#[tokio::test]
async fn subscribe_receive_ack_and_live_push() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Tenant with one cluster.
    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_row = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };
    let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "x@x.test", "X")
        .await
        .expect("u");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("m");
    let ctx = PrincipalCtx::User {
        user_id: user,
        platform_admin: false,
        org_selector_required: false,
        org: Some((org.id, OrgRole::Admin)),
        grants: GrantSet::default(),
    };
    // Snapshot cache primed by the outbox consumer. Start the consumer at the current head
    // so this test only drains events it creates.
    let cache = SnapshotCache::new();
    let consumer = format!("ads-test-{}", unique("c"));
    fp_storage::outbox::register_consumer_at_head(&pool, &consumer)
        .await
        .expect("register");
    let drain = |cache: Arc<SnapshotCache>, pool: sqlx::PgPool, consumer: String| async move {
        while fp_storage::outbox::process_batch(&pool, &consumer, 1000, |events| {
            let cache = cache.clone();
            let pool = pool.clone();
            async move { handle_events(&cache, &pool, events).await }
        })
        .await
        .expect("process")
            > 0
        {}
    };

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

    drain(cache.clone(), pool.clone(), consumer.clone()).await;

    // ADS server on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener); // free it for tonic to rebind (small race; fine for a test)
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server_cache = cache.clone();
    let server = tokio::spawn(async move {
        fp_xds::server::serve_plaintext(
            addr,
            server_cache,
            Arc::new(NodeIdTeamResolver),
            None,
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Real gRPC client subscribes to CDS with the team in node.id (dev resolver).
    let mut client = AggregatedDiscoveryServiceClient::connect(format!("http://{addr}"))
        .await
        .expect("client connect");
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(8);
    req_tx
        .send(DiscoveryRequest {
            node: Some(Node {
                id: format!("team={}/dp-test", team.id),
                ..Default::default()
            }),
            type_url: CLUSTER_TYPE_URL.to_string(),
            ..Default::default()
        })
        .await
        .expect("send subscribe");
    let mut responses = client
        .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(req_rx))
        .await
        .expect("stream")
        .into_inner();

    // Initial snapshot arrives.
    let first = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely")
        .expect("ok")
        .expect("response");
    assert_eq!(first.type_url, CLUSTER_TYPE_URL);
    assert_eq!(first.resources.len(), 1);
    let first_version = first.version_info.clone();

    // ACK it (echo version + nonce).
    req_tx
        .send(DiscoveryRequest {
            type_url: CLUSTER_TYPE_URL.to_string(),
            version_info: first.version_info,
            response_nonce: first.nonce,
            ..Default::default()
        })
        .await
        .expect("ack");

    // Mutate the cluster (a cluster-level field — endpoint-only churn flows over EDS);
    // drain the outbox; the open stream must receive a push.
    let mut new_spec = cluster_spec("10.0.0.99");
    new_spec.connect_timeout_secs = 9;
    fp_core::services::clusters::update_cluster(
        &pool,
        &ctx,
        team,
        &upstream,
        new_spec,
        1,
        RequestId::generate(),
    )
    .await
    .expect("update");
    drain(cache.clone(), pool.clone(), consumer.clone()).await;

    let push = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("push timely")
        .expect("ok")
        .expect("response");
    assert_eq!(push.type_url, CLUSTER_TYPE_URL);
    assert_ne!(
        push.version_info, first_version,
        "new version after the update"
    );
    assert_eq!(push.resources.len(), 1);

    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

#[tokio::test]
async fn nack_quarantines_offender_and_pushes_corrected_set() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_row = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };
    let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "x@x.test", "X")
        .await
        .expect("u");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("m");
    let ctx = PrincipalCtx::User {
        user_id: user,
        platform_admin: false,
        org_selector_required: false,
        org: Some((org.id, OrgRole::Admin)),
        grants: GrantSet::default(),
    };
    let cache = SnapshotCache::new();
    let consumer = format!("nack-test-{}", unique("c"));
    fp_storage::outbox::register_consumer_at_head(&pool, &consumer)
        .await
        .expect("register");
    let drain = |cache: Arc<SnapshotCache>, pool: sqlx::PgPool, consumer: String| async move {
        while fp_storage::outbox::process_batch(&pool, &consumer, 1000, |events| {
            let cache = cache.clone();
            let pool = pool.clone();
            async move { handle_events(&cache, &pool, events).await }
        })
        .await
        .expect("process")
            > 0
        {}
    };

    let good = unique("good");
    fp_core::services::clusters::create_cluster(
        &pool,
        &ctx,
        team,
        &good,
        cluster_spec("10.0.0.1"),
        RequestId::generate(),
    )
    .await
    .expect("cluster");

    drain(cache.clone(), pool.clone(), consumer.clone()).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server_cache = cache.clone();
    let server_pool = pool.clone();
    tokio::spawn(async move {
        fp_xds::server::serve_plaintext(
            addr,
            server_cache,
            Arc::new(NodeIdTeamResolver),
            Some(server_pool),
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut client = AggregatedDiscoveryServiceClient::connect(format!("http://{addr}"))
        .await
        .expect("client connect");
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(8);
    req_tx
        .send(DiscoveryRequest {
            node: Some(Node {
                id: format!("team={}/dp-nack", team.id),
                ..Default::default()
            }),
            type_url: CLUSTER_TYPE_URL.to_string(),
            ..Default::default()
        })
        .await
        .expect("subscribe");
    let mut responses = client
        .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(req_rx))
        .await
        .expect("stream")
        .into_inner();

    let first = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely")
        .expect("ok")
        .expect("response");
    assert_eq!(first.resources.len(), 1);
    req_tx
        .send(DiscoveryRequest {
            type_url: CLUSTER_TYPE_URL.to_string(),
            version_info: first.version_info.clone(),
            response_nonce: first.nonce,
            ..Default::default()
        })
        .await
        .expect("ack");

    // A second cluster appears and is pushed.
    let bad = unique("bad");
    fp_core::services::clusters::create_cluster(
        &pool,
        &ctx,
        team,
        &bad,
        cluster_spec("10.0.0.66"),
        RequestId::generate(),
    )
    .await
    .expect("bad cluster");
    drain(cache.clone(), pool.clone(), consumer.clone()).await;
    let push = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely")
        .expect("ok")
        .expect("response");
    assert_eq!(push.resources.len(), 2);

    // The dataplane rejects the new set (NACK = old version + new nonce + error detail).
    req_tx
        .send(DiscoveryRequest {
            type_url: CLUSTER_TYPE_URL.to_string(),
            version_info: first.version_info.clone(),
            response_nonce: push.nonce,
            error_detail: Some(envoy_types::pb::google::rpc::Status {
                code: 3,
                message: "Proto constraint validation failed".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("nack");

    // Quarantine kicks in: the corrected set (offender held out — it was new, so there
    // is no last-good for it) is pushed on the same stream.
    let corrected = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely")
        .expect("ok")
        .expect("response");
    assert_eq!(
        corrected.resources.len(),
        1,
        "offending new resource is held out of the snapshot"
    );
    assert_ne!(corrected.version_info, push.version_info);
    req_tx
        .send(DiscoveryRequest {
            type_url: CLUSTER_TYPE_URL.to_string(),
            version_info: corrected.version_info,
            response_nonce: corrected.nonce,
            ..Default::default()
        })
        .await
        .expect("ack corrected");

    // The NACK is persisted with the quarantined resource named (insert is spawned off
    // the stream path — poll briefly).
    let mut persisted = Vec::new();
    for _ in 0..20 {
        persisted = fp_storage::repos::xds_nacks::list(&pool, team.id, 10)
            .await
            .expect("list nacks");
        if !persisted.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(persisted.len(), 1, "one NACK event persisted");
    assert_eq!(persisted[0].type_url, CLUSTER_TYPE_URL);
    assert_eq!(persisted[0].quarantined_resources, vec![bad.clone()]);
    assert!(persisted[0].error_message.contains("constraint"));

    // An operator fix (changing the cluster bytes) clears the quarantine: the full set
    // flows again.
    let mut fixed_spec = cluster_spec("10.0.0.67");
    fixed_spec.connect_timeout_secs = 7;
    fp_core::services::clusters::update_cluster(
        &pool,
        &ctx,
        team,
        &bad,
        fixed_spec,
        1,
        RequestId::generate(),
    )
    .await
    .expect("fix");
    drain(cache.clone(), pool.clone(), consumer.clone()).await;
    let fixed = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely")
        .expect("ok")
        .expect("response");
    assert_eq!(
        fixed.resources.len(),
        2,
        "fixed resource rejoins the snapshot"
    );

    let _ = stop_tx.send(());
}

/// A request that echoes the last nonce but CHANGES resource_names is a subscription
/// update, not an ACK — it must be answered. (A warming listener adding an RDS name does
/// exactly this; swallowing it stalls the listener — caught by the live Envoy E2E.)
#[tokio::test]
async fn subscription_change_echoing_last_nonce_is_answered() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_row = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };
    let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "x@x.test", "X")
        .await
        .expect("u");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("m");
    let ctx = PrincipalCtx::User {
        user_id: user,
        platform_admin: false,
        org_selector_required: false,
        org: Some((org.id, OrgRole::Admin)),
        grants: GrantSet::default(),
    };
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
    let rc = unique("routes");
    fp_core::services::gateway::create_route_config(
        &pool,
        &ctx,
        team,
        &rc,
        fp_domain::gateway::route_config::RouteConfigSpec {
            virtual_hosts: vec![fp_domain::gateway::route_config::VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![fp_domain::gateway::route_config::RouteRule {
                    name: "all".into(),
                    matcher: fp_domain::gateway::route_config::PathMatch::Prefix {
                        prefix: "/".into(),
                    },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: fp_domain::gateway::route_config::RouteAction {
                        cluster: Some(upstream.clone()),
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
        },
        RequestId::generate(),
    )
    .await
    .expect("rc");
    let cache = SnapshotCache::new();
    cache.rebuild_team(&pool, team.id).await.expect("prime");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server_cache = cache.clone();
    tokio::spawn(async move {
        fp_xds::server::serve_plaintext(
            addr,
            server_cache,
            Arc::new(NodeIdTeamResolver),
            None,
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut client = AggregatedDiscoveryServiceClient::connect(format!("http://{addr}"))
        .await
        .expect("connect");
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(8);
    req_tx
        .send(DiscoveryRequest {
            node: Some(Node {
                id: format!("team={}/dp-sub", team.id),
                ..Default::default()
            }),
            type_url: ROUTE_TYPE_URL.to_string(),
            resource_names: vec![rc.clone()],
            ..Default::default()
        })
        .await
        .expect("subscribe");
    let mut responses = client
        .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(req_rx))
        .await
        .expect("stream")
        .into_inner();
    let first = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely")
        .expect("ok")
        .expect("response");
    assert_eq!(first.type_url, ROUTE_TYPE_URL);

    // Subscription update shaped exactly like Envoy's: echoes the last nonce + version,
    // no error, but the name set grew. Must be answered, never swallowed as an ACK.
    req_tx
        .send(DiscoveryRequest {
            type_url: ROUTE_TYPE_URL.to_string(),
            version_info: first.version_info.clone(),
            response_nonce: first.nonce.clone(),
            resource_names: vec![rc.clone(), format!("{rc}-second")],
            ..Default::default()
        })
        .await
        .expect("subscription update");
    let answered = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("subscription updates must be answered, not treated as ACKs")
        .expect("ok")
        .expect("response");
    assert_eq!(answered.type_url, ROUTE_TYPE_URL);

    let _ = stop_tx.send(());
}

#[tokio::test]
async fn stream_without_team_identity_is_rejected() {
    let cache = SnapshotCache::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server_cache = cache.clone();
    tokio::spawn(async move {
        fp_xds::server::serve_plaintext(
            addr,
            server_cache,
            Arc::new(NodeIdTeamResolver),
            None,
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut client = AggregatedDiscoveryServiceClient::connect(format!("http://{addr}"))
        .await
        .expect("connect");
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(2);
    req_tx
        .send(DiscoveryRequest {
            node: Some(Node {
                id: "anonymous-node".into(),
                ..Default::default()
            }),
            type_url: CLUSTER_TYPE_URL.to_string(),
            ..Default::default()
        })
        .await
        .expect("send");
    let mut responses = client
        .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(req_rx))
        .await
        .expect("stream")
        .into_inner();
    let outcome = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely");
    assert!(
        outcome.is_err(),
        "a dataplane without team identity must be rejected, got {outcome:?}"
    );
    let _ = stop_tx.send(());
}
