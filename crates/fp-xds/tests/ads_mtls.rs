//! mTLS xDS integration (S5.4): real TLS handshakes against `serve_mtls`, with team
//! identity bound through the certificate registry — never the SAN's team segment or the
//! node id. Certificates are minted with the `openssl` CLI (present on dev/CI images).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use envoy_types::pb::envoy::config::core::v3::Node;
use envoy_types::pb::envoy::service::discovery::v3::aggregated_discovery_service_client::AggregatedDiscoveryServiceClient;
use envoy_types::pb::envoy::service::discovery::v3::DiscoveryRequest;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::{OrgRole, RequestId};
use fp_storage::repos::identity;
use fp_xds::ads::{publish_revocations, CertRegistryResolver};
use fp_xds::server::{serve_mtls, XdsTlsPaths};
use fp_xds::snapshot::{handle_events, SnapshotCache, CLUSTER_TYPE_URL};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Run openssl, panicking with its stderr on failure (test fixture only).
fn openssl(dir: &Path, args: &[&str]) {
    let out = Command::new("openssl")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("openssl CLI must be available for mTLS tests");
    assert!(
        out.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct TestPki {
    dir: PathBuf,
}

impl TestPki {
    /// One CA + a server certificate for localhost/127.0.0.1.
    fn new() -> Self {
        let dir = std::env::temp_dir().join(unique("fp-mtls"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        openssl(
            &dir,
            &[
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-keyout",
                "ca.key",
                "-out",
                "ca.crt",
                "-days",
                "2",
                "-nodes",
                "-subj",
                "/CN=fp-test-ca",
            ],
        );
        std::fs::write(
            dir.join("server-san.cnf"),
            "subjectAltName=DNS:localhost,IP:127.0.0.1\n",
        )
        .expect("write san");
        openssl(
            &dir,
            &[
                "req",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-nodes",
                "-subj",
                "/CN=fp-xds-server",
            ],
        );
        openssl(
            &dir,
            &[
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                "server.crt",
                "-days",
                "2",
                "-extfile",
                "server-san.cnf",
            ],
        );
        Self { dir }
    }

    fn tls_paths(&self) -> XdsTlsPaths {
        XdsTlsPaths {
            cert_path: self.dir.join("server.crt"),
            key_path: self.dir.join("server.key"),
            client_ca_path: self.dir.join("ca.crt"),
        }
    }

    /// CA-signed client certificate carrying `spiffe_uri` as a URI SAN. Returns the
    /// identity for the tonic client.
    fn client_identity(&self, name: &str, spiffe_uri: &str) -> Identity {
        std::fs::write(
            self.dir.join(format!("{name}-san.cnf")),
            format!("subjectAltName=URI:{spiffe_uri}\n"),
        )
        .expect("write client san");
        openssl(
            &self.dir,
            &[
                "req",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-keyout",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.csr"),
                "-nodes",
                "-subj",
                &format!("/CN={name}"),
            ],
        );
        openssl(
            &self.dir,
            &[
                "x509",
                "-req",
                "-in",
                &format!("{name}.csr"),
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                &format!("{name}.crt"),
                "-days",
                "2",
                "-extfile",
                &format!("{name}-san.cnf"),
            ],
        );
        Identity::from_pem(
            std::fs::read(self.dir.join(format!("{name}.crt"))).expect("crt"),
            std::fs::read(self.dir.join(format!("{name}.key"))).expect("key"),
        )
    }

    fn ca_pem(&self) -> Vec<u8> {
        std::fs::read(self.dir.join("ca.crt")).expect("ca")
    }
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

struct World {
    pool: sqlx::PgPool,
    team: TeamRef,
    ctx: PrincipalCtx,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
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
    Some(World {
        pool,
        team,
        ctx: PrincipalCtx::User {
            user_id: user,
            platform_admin: false,
            memberships: vec![(org.id, OrgRole::Admin)],
            org: Some((org.id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
    })
}

/// Boot `serve_mtls` on an ephemeral port; returns (addr, revocation bus sender).
async fn start_server(
    pki: &TestPki,
    cache: Arc<SnapshotCache>,
    pool: sqlx::PgPool,
) -> (
    std::net::SocketAddr,
    tokio::sync::broadcast::Sender<uuid::Uuid>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let (revocations, _) = tokio::sync::broadcast::channel::<uuid::Uuid>(16);
    let tls = pki.tls_paths();
    let bus = revocations.clone();
    tokio::spawn(async move {
        serve_mtls(
            addr,
            cache,
            Arc::new(CertRegistryResolver::new(pool.clone())),
            bus,
            pool,
            &tls,
            std::future::pending(),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    (addr, revocations)
}

async fn tls_channel(
    pki: &TestPki,
    addr: std::net::SocketAddr,
    identity: Option<Identity>,
) -> Result<Channel, tonic::transport::Error> {
    let mut tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(pki.ca_pem()))
        .domain_name("localhost");
    if let Some(identity) = identity {
        tls = tls.identity(identity);
    }
    Channel::from_shared(format!("https://localhost:{}", addr.port()))
        .expect("uri")
        .tls_config(tls)
        .expect("tls")
        .connect()
        .await
}

fn cds_subscribe(node_id: &str) -> DiscoveryRequest {
    DiscoveryRequest {
        node: Some(Node {
            id: node_id.to_string(),
            ..Default::default()
        }),
        type_url: CLUSTER_TYPE_URL.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn registry_binds_team_and_revocation_kills_live_stream() {
    let Some(w) = world().await else { return };
    let pki = TestPki::new();

    // Team A owns one cluster; cache primed.
    fp_core::services::clusters::create_cluster(
        &w.pool,
        &w.ctx,
        w.team,
        &unique("upstream"),
        cluster_spec("10.1.0.1"),
        RequestId::generate(),
    )
    .await
    .expect("cluster");
    let cache = SnapshotCache::new();
    cache.rebuild_team(&w.pool, w.team.id).await.expect("prime");

    // Dataplane + registered certificate. The SPIFFE URI deliberately claims a DIFFERENT
    // team name in its path — the registry row, not the SAN text, decides the tenant.
    let dp = unique("dp");
    fp_core::services::dataplanes::create_dataplane(
        &w.pool,
        &w.ctx,
        w.team,
        &dp,
        "",
        RequestId::generate(),
    )
    .await
    .expect("dataplane");
    let spiffe = format!("spiffe://flowplane.test/team/some-other-team/proxy/{dp}");
    let serial = unique("serial");
    fp_core::services::dataplanes::register_certificate(
        &w.pool,
        &w.ctx,
        w.team,
        fp_core::services::dataplanes::CertificateRegistration {
            dataplane: &dp,
            spiffe_uri: &spiffe,
            serial_number: &serial,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        },
        RequestId::generate(),
    )
    .await
    .expect("register");

    let (addr, revocations) = start_server(&pki, cache.clone(), w.pool.clone()).await;
    let identity = pki.client_identity("dp-good", &spiffe);
    let channel = tls_channel(&pki, addr, Some(identity))
        .await
        .expect("mTLS connect");
    let mut client = AggregatedDiscoveryServiceClient::new(channel);

    // node.id claims yet another team; it must be ignored (attribution only).
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(8);
    req_tx
        .send(cds_subscribe(
            "team=ffffffff-ffff-ffff-ffff-ffffffffffff/dp-lies",
        ))
        .await
        .expect("send");
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
    assert_eq!(first.type_url, CLUSTER_TYPE_URL);
    assert_eq!(
        first.resources.len(),
        1,
        "the registry team's snapshot is served regardless of SAN/node-id claims"
    );

    // Revoke; drive the event through the consumer path (publish + snapshot handler).
    fp_core::services::dataplanes::revoke_certificate(
        &w.pool,
        &w.ctx,
        w.team,
        &serial,
        "test revocation",
        RequestId::generate(),
    )
    .await
    .expect("revoke");
    let consumer = unique("mtls-consumer");
    fp_storage::outbox::register_consumer(&w.pool, &consumer)
        .await
        .expect("register consumer");
    while fp_storage::outbox::process_batch(&w.pool, &consumer, 1000, |events| {
        let cache = cache.clone();
        let pool = w.pool.clone();
        let revocations = revocations.clone();
        async move {
            publish_revocations(&revocations, &events);
            handle_events(&cache, &pool, events).await
        }
    })
    .await
    .expect("process")
        > 0
    {}

    // The live stream must terminate with PERMISSION_DENIED.
    let outcome = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely");
    match outcome {
        Err(status) => assert_eq!(status.code(), tonic::Code::PermissionDenied),
        Ok(other) => panic!("expected stream kill after revocation, got {other:?}"),
    }

    // Reconnect with the same (now revoked) certificate: rejected at the registry.
    let identity = pki.client_identity("dp-good-2", &spiffe);
    let channel = tls_channel(&pki, addr, Some(identity))
        .await
        .expect("TLS still succeeds; authz happens at the registry");
    let mut client = AggregatedDiscoveryServiceClient::new(channel);
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(2);
    req_tx.send(cds_subscribe("dp-e2e")).await.expect("send");
    let mut responses = client
        .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(req_rx))
        .await
        .expect("stream")
        .into_inner();
    let outcome = tokio::time::timeout(Duration::from_secs(5), responses.message())
        .await
        .expect("timely");
    match outcome {
        Err(status) => assert_eq!(status.code(), tonic::Code::Unauthenticated),
        Ok(other) => panic!("revoked certificate must not authenticate, got {other:?}"),
    }
}

#[tokio::test]
async fn unregistered_and_expired_certificates_are_rejected() {
    let Some(w) = world().await else { return };
    let pki = TestPki::new();
    let cache = SnapshotCache::new();
    let (addr, _revocations) = start_server(&pki, cache, w.pool.clone()).await;

    // CA-signed but never registered.
    let spiffe = format!("spiffe://flowplane.test/team/x/proxy/{}", unique("ghost"));
    let identity = pki.client_identity("dp-ghost", &spiffe);
    let channel = tls_channel(&pki, addr, Some(identity))
        .await
        .expect("TLS handshake passes (valid CA); registry must still reject");
    let mut client = AggregatedDiscoveryServiceClient::new(channel);
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(2);
    req_tx.send(cds_subscribe("dp-ghost")).await.expect("send");
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
        "unregistered certificate must be rejected, got {outcome:?}"
    );

    // Registered but expired (inserted at the storage layer; the service refuses past
    // expiries by construction).
    let dp = unique("dp");
    fp_core::services::dataplanes::create_dataplane(
        &w.pool,
        &w.ctx,
        w.team,
        &dp,
        "",
        RequestId::generate(),
    )
    .await
    .expect("dataplane");
    let dataplane = fp_storage::repos::dataplanes::get_dataplane(&w.pool, w.team.id, &dp)
        .await
        .expect("get")
        .expect("exists");
    let spiffe = format!("spiffe://flowplane.test/team/x/proxy/{dp}");
    let mut tx = w.pool.begin().await.expect("tx");
    fp_storage::repos::dataplanes::register_certificate(
        &mut tx,
        w.team.id,
        dataplane.id,
        &spiffe,
        &unique("serial"),
        chrono::Utc::now() - chrono::Duration::hours(1),
        None,
    )
    .await
    .expect("insert expired");
    tx.commit().await.expect("commit");

    let identity = pki.client_identity("dp-expired", &spiffe);
    let channel = tls_channel(&pki, addr, Some(identity))
        .await
        .expect("connect");
    let mut client = AggregatedDiscoveryServiceClient::new(channel);
    let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(2);
    req_tx
        .send(cds_subscribe("dp-expired"))
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
        "expired certificate must be rejected, got {outcome:?}"
    );
}

#[tokio::test]
async fn connection_without_client_certificate_fails() {
    let Some(w) = world().await else { return };
    let pki = TestPki::new();
    let cache = SnapshotCache::new();
    let (addr, _revocations) = start_server(&pki, cache, w.pool.clone()).await;

    // No client identity: the handshake (or first use of the channel) must fail — there
    // is no anonymous mode on the mTLS listener.
    match tls_channel(&pki, addr, None).await {
        Err(_) => {} // rejected at connect
        Ok(channel) => {
            let mut client = AggregatedDiscoveryServiceClient::new(channel);
            let (req_tx, req_rx) = tokio::sync::mpsc::channel::<DiscoveryRequest>(2);
            req_tx.send(cds_subscribe("dp-anon")).await.expect("send");
            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                client.stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(
                    req_rx,
                )),
            )
            .await
            .expect("timely");
            assert!(
                outcome.is_err(),
                "anonymous TLS must not reach the ADS service"
            );
        }
    }
}
