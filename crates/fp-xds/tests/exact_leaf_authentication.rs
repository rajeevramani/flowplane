//! RED contract for fpv2-7f3.4: exact verified-leaf authentication on every xDS-family service.
//!
//! Authored from the approved design/plan as a black-box integration target. This file uses a
//! real PostgreSQL registry and a real root -> intermediate -> leaf PKI. It intentionally does not
//! inspect fp-xds/fp-storage production implementation. Every shared-database fixture is unique,
//! listeners use ephemeral ports, waits are bounded, and no test deletes shared rows.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use envoy_types::pb::envoy::config::core::v3::Node;
use envoy_types::pb::envoy::service::accesslog::v3::{
    access_log_service_client::AccessLogServiceClient, stream_access_logs_message,
    StreamAccessLogsMessage,
};
use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_client::AggregatedDiscoveryServiceClient, DiscoveryRequest,
};
use envoy_types::pb::envoy::service::ext_proc::v3::{
    external_processor_client::ExternalProcessorClient, ProcessingRequest,
};
use fp_domain::{DataplaneId, TeamId};
use fp_xds::ads::{CertRegistryResolver, PeerIdentity, TeamResolver};
use fp_xds::diagnostics::{
    diagnostics_report, DiagnosticsReport, EnvoyDiagnosticsServiceClient, HeartbeatReport,
};
use fp_xds::server::{serve_mtls, XdsTlsPaths};
use fp_xds::snapshot::{SnapshotCache, CLUSTER_TYPE_URL};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

const DEADLINE: Duration = Duration::from_secs(5);

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

fn openssl(dir: &Path, args: &[&str]) {
    let output = Command::new("openssl")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("openssl CLI must be available for real-mTLS tests");
    assert!(
        output.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Clone)]
struct Leaf {
    cert_chain_pem: Vec<u8>,
    key_pem: Vec<u8>,
    der: Vec<u8>,
    serial: String,
    fingerprint: String,
}

impl Leaf {
    fn identity(&self) -> Identity {
        Identity::from_pem(self.cert_chain_pem.clone(), self.key_pem.clone())
    }
}

struct TestPki {
    dir: PathBuf,
}

impl TestPki {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(unique("fpv2-7f3-4-pki"));
        std::fs::create_dir_all(&dir).expect("create PKI fixture directory");

        std::fs::write(
            dir.join("root.cnf"),
            "[req]\ndistinguished_name=dn\nx509_extensions=v3_ca\nprompt=no\n[dn]\nCN=Flowplane test root\n[v3_ca]\nbasicConstraints=critical,CA:true,pathlen:1\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always\n",
        )
        .expect("root config");
        openssl(
            &dir,
            &[
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-keyout",
                "root.key", "-out", "root.crt", "-config", "root.cnf",
            ],
        );

        std::fs::write(
            dir.join("intermediate.cnf"),
            "basicConstraints=critical,CA:true,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n",
        )
        .expect("intermediate config");
        openssl(
            &dir,
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "intermediate.key",
                "-out",
                "intermediate.csr",
                "-subj",
                "/CN=Flowplane test intermediate",
            ],
        );
        openssl(
            &dir,
            &[
                "x509",
                "-req",
                "-in",
                "intermediate.csr",
                "-CA",
                "root.crt",
                "-CAkey",
                "root.key",
                "-CAcreateserial",
                "-days",
                "2",
                "-out",
                "intermediate.crt",
                "-extfile",
                "intermediate.cnf",
            ],
        );

        std::fs::write(
            dir.join("server.cnf"),
            "basicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n",
        )
        .expect("server config");
        openssl(
            &dir,
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-subj",
                "/CN=localhost",
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
                "intermediate.crt",
                "-CAkey",
                "intermediate.key",
                "-CAcreateserial",
                "-days",
                "2",
                "-out",
                "server-leaf.crt",
                "-extfile",
                "server.cnf",
            ],
        );
        openssl(
            &dir,
            &[
                "pkcs8",
                "-topk8",
                "-nocrypt",
                "-in",
                "server.key",
                "-out",
                "server.pk8.key",
            ],
        );
        let mut server_chain = std::fs::read(dir.join("server-leaf.crt")).expect("server leaf");
        server_chain.extend(std::fs::read(dir.join("intermediate.crt")).expect("intermediate"));
        std::fs::write(dir.join("server-chain.crt"), server_chain).expect("server chain");

        Self { dir }
    }

    fn issue_client(&self, name: &str, spiffe_uri: &str, serial: &str) -> Leaf {
        let ext = format!(
            "basicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=clientAuth\nsubjectAltName=URI:{spiffe_uri}\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n"
        );
        std::fs::write(self.dir.join(format!("{name}.cnf")), ext).expect("client config");
        openssl(
            &self.dir,
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.csr"),
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
                "intermediate.crt",
                "-CAkey",
                "intermediate.key",
                "-set_serial",
                &format!("0x00{serial}"),
                "-days",
                "2",
                "-out",
                &format!("{name}.crt"),
                "-extfile",
                &format!("{name}.cnf"),
            ],
        );
        openssl(
            &self.dir,
            &[
                "pkcs8",
                "-topk8",
                "-nocrypt",
                "-in",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.pk8.key"),
            ],
        );
        openssl(
            &self.dir,
            &[
                "x509",
                "-in",
                &format!("{name}.crt"),
                "-outform",
                "DER",
                "-out",
                &format!("{name}.der"),
            ],
        );
        let der = std::fs::read(self.dir.join(format!("{name}.der"))).expect("leaf DER");
        let fingerprint = format!("{:x}", Sha256::digest(&der));
        let mut chain = std::fs::read(self.dir.join(format!("{name}.crt"))).expect("leaf cert");
        chain.extend(std::fs::read(self.dir.join("intermediate.crt")).expect("intermediate"));
        Leaf {
            cert_chain_pem: chain,
            key_pem: std::fs::read(self.dir.join(format!("{name}.pk8.key"))).expect("client key"),
            der,
            serial: canonical_hex(serial),
            fingerprint,
        }
    }

    fn tls_paths(&self) -> XdsTlsPaths {
        XdsTlsPaths {
            cert_path: self.dir.join("server-chain.crt"),
            key_path: self.dir.join("server.pk8.key"),
            client_ca_path: self.dir.join("root.crt"),
        }
    }

    fn root_pem(&self) -> Vec<u8> {
        std::fs::read(self.dir.join("root.crt")).expect("root cert")
    }
}

impl Drop for TestPki {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn canonical_hex(value: &str) -> String {
    let trimmed = value.trim_start_matches('0').to_ascii_lowercase();
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed
    }
}

#[test]
fn derives_exact_identity_from_leaf_der_and_fails_closed_on_malformed_der() {
    let pki = TestPki::new();
    let spiffe_uri = "spiffe://flowplane.test/dataplane/der-unit";
    let leaf = pki.issue_client("der-unit", spiffe_uri, "00000000000000a1");
    let identity =
        fp_xds::server::certificate_identity_from_der(&leaf.der).expect("valid leaf identity");
    assert_eq!(identity.spiffe_uri, spiffe_uri);
    assert_eq!(identity.serial_number, "a1");
    assert_eq!(identity.fingerprint_sha256, leaf.fingerprint);
    assert!(fp_xds::server::certificate_identity_from_der(b"not DER").is_none());
}

struct World {
    pool: PgPool,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 12)
        .await
        .expect("connect PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");
    let org_id = uuid::Uuid::now_v7();
    let team_id = uuid::Uuid::now_v7();
    let dataplane_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(unique("exact-leaf-org"))
        .execute(&pool)
        .await
        .expect("organization fixture");
    sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(org_id)
        .bind(unique("exact-leaf-team"))
        .execute(&pool)
        .await
        .expect("team fixture");
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(dataplane_id)
        .bind(team_id)
        .bind(org_id)
        .bind(unique("exact-leaf-dataplane"))
        .execute(&pool)
        .await
        .expect("dataplane fixture");
    Some(World {
        pool,
        team_id,
        dataplane_id,
    })
}

#[derive(Clone, Copy)]
enum RegistryState<'a> {
    Exact(&'a Leaf),
    Legacy,
    WrongFingerprint,
    Expired,
    Revoked,
}

async fn insert_registry_row(
    world: &World,
    spiffe_uri: &str,
    leaf: &Leaf,
    state: RegistryState<'_>,
) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    let fingerprint = match state {
        RegistryState::Exact(exact) => Some(exact.fingerprint.clone()),
        RegistryState::Legacy => None,
        RegistryState::WrongFingerprint => Some(format!("{:x}", Sha256::digest(id.as_bytes()))),
        RegistryState::Expired | RegistryState::Revoked => Some(leaf.fingerprint.clone()),
    };
    let serial = leaf.serial.clone();
    sqlx::query(
        "INSERT INTO proxy_certificates \
         (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at, \
          revoked_at, revoked_reason) \
         VALUES ($1, $2, $3, $4, $5, $6, \
          CASE WHEN $7 THEN now() - interval '1 second' ELSE now() + interval '1 hour' END, \
          CASE WHEN $8 THEN now() ELSE NULL END, \
          CASE WHEN $8 THEN 'fpv2-7f3.4 fixture' ELSE NULL END)",
    )
    .bind(id)
    .bind(world.team_id)
    .bind(world.dataplane_id)
    .bind(spiffe_uri)
    .bind(serial)
    .bind(fingerprint)
    .bind(matches!(state, RegistryState::Expired))
    .bind(matches!(state, RegistryState::Revoked))
    .execute(&world.pool)
    .await
    .expect("certificate registry fixture");
    id
}

struct ServerGuard {
    addr: std::net::SocketAddr,
    revocations: tokio::sync::broadcast::Sender<uuid::Uuid>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<fp_domain::DomainResult<()>>,
}

struct TestLegacyPinner(PgPool);

#[fp_xds::async_trait]
impl fp_xds::ads::LegacyCertificateFingerprintPinner for TestLegacyPinner {
    async fn pin(
        &self,
        spiffe_uri: &str,
        serial_number: &str,
        fingerprint_sha256: &str,
        request_id: fp_domain::RequestId,
    ) -> fp_domain::DomainResult<fp_domain::ProxyCertificate> {
        fp_core::services::dataplanes::pin_legacy_certificate_fingerprint(
            &self.0,
            spiffe_uri,
            serial_number,
            fingerprint_sha256,
            request_id,
        )
        .await
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}

async fn start_server(pki: &TestPki, resolver_pool: PgPool, service_pool: PgPool) -> ServerGuard {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve listener");
    let addr = listener.local_addr().expect("ephemeral address");
    drop(listener);
    let tls = pki.tls_paths();
    let (revocations, _) = tokio::sync::broadcast::channel(16);
    let server_revocations = revocations.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let legacy_pinner = Arc::new(TestLegacyPinner(resolver_pool.clone()));
    let task = tokio::spawn(async move {
        serve_mtls(
            addr,
            SnapshotCache::new(),
            Arc::new(CertRegistryResolver::new(resolver_pool, legacy_pinner)),
            server_revocations,
            service_pool,
            &tls,
            async move {
                let _ = stopped.await;
            },
        )
        .await
    });
    let guard = ServerGuard {
        addr,
        revocations,
        stop: Some(stop),
        task,
    };
    for _ in 0..40 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return guard;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("mTLS server did not bind within one second");
}

async fn channel(
    pki: &TestPki,
    addr: std::net::SocketAddr,
    leaf: &Leaf,
) -> Result<Channel, tonic::transport::Error> {
    Channel::from_shared(format!("https://localhost:{}", addr.port()))
        .expect("channel URI")
        .tls_config(
            ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(pki.root_pem()))
                .domain_name("localhost")
                .identity(leaf.identity()),
        )
        .expect("client TLS config")
        .connect()
        .await
}

#[derive(Clone, Copy, Debug)]
enum Service {
    Ads,
    Diagnostics,
    AccessLog,
    ExtProc,
    ExtProcCapture,
    ExtProcNoContext,
}

const SERVICES: [Service; 5] = [
    Service::Ads,
    Service::Diagnostics,
    Service::AccessLog,
    Service::ExtProc,
    Service::ExtProcCapture,
];

async fn service_result(
    channel: Channel,
    service: Service,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
) -> Result<(), tonic::Status> {
    match service {
        Service::Ads => {
            let mut client = AggregatedDiscoveryServiceClient::new(channel);
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            tx.send(DiscoveryRequest {
                node: Some(Node {
                    id: format!("dataplane/{dataplane_id}"),
                    ..Default::default()
                }),
                type_url: CLUSTER_TYPE_URL.to_owned(),
                ..Default::default()
            })
            .await
            .expect("ADS request");
            drop(tx);
            let mut responses = client
                .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(rx))
                .await?
                .into_inner();
            responses.message().await?.ok_or_else(|| {
                tonic::Status::unknown("ADS closed without authenticating and responding")
            })?;
            Ok(())
        }
        Service::Diagnostics => {
            let report_id = uuid::Uuid::now_v7().to_string();
            let report = DiagnosticsReport {
                schema_version: 1,
                report_id: report_id.clone(),
                dataplane_id: dataplane_id.to_string(),
                observed_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                payload: Some(diagnostics_report::Payload::Heartbeat(HeartbeatReport {
                    requests_delta: 0,
                    errors_delta: 0,
                    warming_failures_delta: 0,
                    config_verified: true,
                })),
            };
            let mut client = EnvoyDiagnosticsServiceClient::new(channel);
            let mut responses = client
                .report_diagnostics(tokio_stream::iter([report]))
                .await?
                .into_inner();
            let ack = responses.message().await?.ok_or_else(|| {
                tonic::Status::unknown("diagnostics closed without an acknowledgement")
            })?;
            if !ack.report_ids.iter().any(|id| id == &report_id) {
                return Err(tonic::Status::unknown(
                    "diagnostics acknowledgement omitted the report id",
                ));
            }
            Ok(())
        }
        Service::AccessLog => {
            let mut client = AccessLogServiceClient::new(channel);
            let message = StreamAccessLogsMessage {
                identifier: Some(stream_access_logs_message::Identifier {
                    node: Some(Node {
                        id: format!("dataplane/{dataplane_id}"),
                        ..Default::default()
                    }),
                    log_name: "fpv2-7f3.4-auth-boundary".to_owned(),
                }),
                log_entries: Some(stream_access_logs_message::LogEntries::HttpLogs(
                    stream_access_logs_message::HttpAccessLogEntries {
                        log_entry: Vec::new(),
                    },
                )),
            };
            let mut request = tonic::Request::new(tokio_stream::iter([message]));
            request.metadata_mut().insert(
                "x-flowplane-team-id",
                team_id.to_string().parse().expect("team metadata"),
            );
            request.metadata_mut().insert(
                "x-flowplane-capture-session-id",
                uuid::Uuid::now_v7()
                    .to_string()
                    .parse()
                    .expect("capture metadata"),
            );
            request.metadata_mut().insert(
                "x-flowplane-route-config-id",
                uuid::Uuid::now_v7()
                    .to_string()
                    .parse()
                    .expect("route metadata"),
            );
            client.stream_access_logs(request).await?;
            Ok(())
        }
        Service::ExtProc => {
            let mut client = ExternalProcessorClient::new(channel);
            let (_tx, rx) = tokio::sync::mpsc::channel::<ProcessingRequest>(1);
            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            let metadata = request.metadata_mut();
            metadata.insert("x-flowplane-ai-processor", "true".parse().unwrap());
            metadata.insert(
                "x-flowplane-team-id",
                team_id.to_string().parse().expect("team metadata"),
            );
            metadata.insert(
                "x-flowplane-route-config-id",
                uuid::Uuid::now_v7().to_string().parse().unwrap(),
            );
            metadata.insert(
                "x-flowplane-ai-provider-id",
                uuid::Uuid::now_v7().to_string().parse().unwrap(),
            );
            metadata.insert("x-flowplane-ai-backend-position", "0".parse().unwrap());
            client.process(request).await?;
            Ok(())
        }
        Service::ExtProcCapture => {
            let mut client = ExternalProcessorClient::new(channel);
            let (_tx, rx) = tokio::sync::mpsc::channel::<ProcessingRequest>(1);
            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            let metadata = request.metadata_mut();
            metadata.insert(
                "x-flowplane-team-id",
                team_id.to_string().parse().expect("team metadata"),
            );
            metadata.insert(
                "x-flowplane-capture-session-id",
                uuid::Uuid::now_v7().to_string().parse().unwrap(),
            );
            metadata.insert(
                "x-flowplane-route-config-id",
                uuid::Uuid::now_v7().to_string().parse().unwrap(),
            );
            client.process(request).await?;
            Ok(())
        }
        Service::ExtProcNoContext => {
            let mut client = ExternalProcessorClient::new(channel);
            let (_tx, rx) = tokio::sync::mpsc::channel::<ProcessingRequest>(1);
            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            request
                .metadata_mut()
                .insert("x-flowplane-ai-processor", "true".parse().unwrap());
            client.process(request).await?;
            Ok(())
        }
    }
}

async fn bounded_service_result(
    channel: Channel,
    service: Service,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
) -> Result<(), tonic::Status> {
    tokio::time::timeout(
        DEADLINE,
        service_result(channel, service, team_id, dataplane_id),
    )
    .await
    .unwrap_or_else(|_| Err(tonic::Status::deadline_exceeded("service auth timed out")))
}

#[tokio::test]
async fn registered_exact_leaf_succeeds_on_all_services() {
    let Some(world) = world().await else { return };
    let pki = TestPki::new();
    let spiffe_uri = format!("spiffe://flowplane.test/dataplane/{}", world.dataplane_id);
    let leaf = pki.issue_client("registered", &spiffe_uri, "00000000000000a1");
    let certificate_id =
        insert_registry_row(&world, &spiffe_uri, &leaf, RegistryState::Exact(&leaf)).await;
    let presented =
        fp_xds::server::certificate_identity_from_der(&leaf.der).expect("presented leaf identity");
    let resolver = CertRegistryResolver::new(
        world.pool.clone(),
        Arc::new(TestLegacyPinner(world.pool.clone())),
    );
    let resolved = resolver
        .resolve("all-service-entrypoints", Some(&presented))
        .await
        .expect("exact registry resolution");
    assert_eq!(resolved.certificate_id, Some(certificate_id));
    let server = start_server(&pki, world.pool.clone(), world.pool.clone()).await;

    for service in SERVICES {
        let result = bounded_service_result(
            channel(&pki, server.addr, &leaf)
                .await
                .expect("registered leaf TLS handshake"),
            service,
            world.team_id,
            world.dataplane_id,
        )
        .await;
        assert!(
            result.is_ok(),
            "registered exact leaf {certificate_id} must authenticate on {service:?}: {result:?}"
        );
    }
}

#[tokio::test]
async fn trusted_unregistered_copied_spiffe_leaf_is_unauthenticated_on_every_service() {
    let Some(world) = world().await else { return };
    let pki = TestPki::new();
    let spiffe_uri = format!("spiffe://flowplane.test/dataplane/{}", world.dataplane_id);
    let registered = pki.issue_client("registered-copy-control", &spiffe_uri, "a2");
    let copied = pki.issue_client("unregistered-copy", &spiffe_uri, "a3");
    assert_ne!(registered.fingerprint, copied.fingerprint);
    insert_registry_row(
        &world,
        &spiffe_uri,
        &registered,
        RegistryState::Exact(&registered),
    )
    .await;
    let server = start_server(&pki, world.pool.clone(), world.pool.clone()).await;

    for service in SERVICES {
        let status = bounded_service_result(
            channel(&pki, server.addr, &copied)
                .await
                .expect("copied leaf is trusted by TLS"),
            service,
            world.team_id,
            world.dataplane_id,
        )
        .await
        .expect_err("trusted but unregistered exact leaf must fail closed");
        assert_eq!(
            status.code(),
            tonic::Code::Unauthenticated,
            "{service:?} must expose the same closed authentication result"
        );
    }
}

#[tokio::test]
async fn wrong_fingerprint_serial_expired_revoked_and_zero_are_indistinguishable_and_closed() {
    for (case, state, register) in [
        (
            "wrong-fingerprint",
            Some(RegistryState::WrongFingerprint),
            true,
        ),
        ("wrong-serial", Some(RegistryState::Legacy), true),
        ("expired", Some(RegistryState::Expired), true),
        ("revoked", Some(RegistryState::Revoked), true),
        ("zero", None, false),
    ] {
        let Some(world) = world().await else { return };
        let pki = TestPki::new();
        let spiffe_uri = format!(
            "spiffe://flowplane.test/dataplane/{}/{}",
            world.dataplane_id, case
        );
        let leaf = pki.issue_client(case, &spiffe_uri, "00b1");
        if register {
            let selected = state.expect("registered case has a registry state");
            if case == "wrong-serial" {
                let mut wrong_serial_leaf = leaf.clone();
                wrong_serial_leaf.serial = "b2".to_owned();
                insert_registry_row(&world, &spiffe_uri, &wrong_serial_leaf, selected).await;
            } else {
                insert_registry_row(&world, &spiffe_uri, &leaf, selected).await;
            }
        }
        let server = start_server(&pki, world.pool.clone(), world.pool.clone()).await;
        let status = bounded_service_result(
            channel(&pki, server.addr, &leaf)
                .await
                .expect("negative registry state still has a trusted TLS leaf"),
            Service::Ads,
            world.team_id,
            world.dataplane_id,
        )
        .await
        .expect_err("all invalid registry states fail closed");
        assert_eq!(
            (status.code(), status.message()),
            (
                tonic::Code::Unauthenticated,
                "dataplane authentication failed"
            ),
            "{case} must be indistinguishable from every other authentication failure"
        );
    }
}

#[tokio::test]
async fn closed_registry_pool_is_indistinguishable_and_closed() {
    let Some(world) = world().await else { return };
    let pki = TestPki::new();
    let spiffe_uri = format!(
        "spiffe://flowplane.test/dataplane/{}/db",
        world.dataplane_id
    );
    let leaf = pki.issue_client("db-failure", &spiffe_uri, "b3");
    insert_registry_row(&world, &spiffe_uri, &leaf, RegistryState::Exact(&leaf)).await;
    let dead_pool = world.pool.clone();
    dead_pool.close().await;
    let server = start_server(&pki, dead_pool, world.pool.clone()).await;
    let status = bounded_service_result(
        channel(&pki, server.addr, &leaf)
            .await
            .expect("TLS succeeds while registry is unavailable"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await
    .expect_err("database failure must fail closed");
    assert_eq!(
        (status.code(), status.message()),
        (
            tonic::Code::Unauthenticated,
            "dataplane authentication failed"
        )
    );
}

#[tokio::test]
async fn legacy_row_pins_only_after_verified_uri_and_numeric_serial_match_then_requires_exact_leaf()
{
    let Some(world) = world().await else { return };
    let pki = TestPki::new();
    let spiffe_uri = format!(
        "spiffe://flowplane.test/dataplane/{}/legacy",
        world.dataplane_id
    );
    let leaf = pki.issue_client("legacy-match", &spiffe_uri, "000000c1");
    let copied = pki.issue_client("legacy-copy", &spiffe_uri, "c2");
    let certificate_id =
        insert_registry_row(&world, &spiffe_uri, &leaf, RegistryState::Legacy).await;
    let server = start_server(&pki, world.pool.clone(), world.pool.clone()).await;

    bounded_service_result(
        channel(&pki, server.addr, &leaf)
            .await
            .expect("legacy exact leaf TLS"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await
    .expect("verified URI plus numeric serial pins and authenticates");

    let stored: Option<String> =
        sqlx::query_scalar("SELECT fingerprint_sha256 FROM proxy_certificates WHERE id = $1")
            .bind(certificate_id)
            .fetch_one(&world.pool)
            .await
            .expect("pinned legacy row");
    assert_eq!(stored.as_deref(), Some(leaf.fingerprint.as_str()));

    let status = bounded_service_result(
        channel(&pki, server.addr, &copied)
            .await
            .expect("copied legacy URI TLS"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await
    .expect_err("after pinning, another trusted leaf with the URI must fail");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn ext_proc_authenticates_before_the_no_ai_context_branch() {
    let Some(world) = world().await else { return };
    let pki = TestPki::new();
    let spiffe_uri = format!(
        "spiffe://flowplane.test/dataplane/{}/extproc",
        world.dataplane_id
    );
    let registered = pki.issue_client("extproc-registered", &spiffe_uri, "d1");
    let copied = pki.issue_client("extproc-copy", &spiffe_uri, "d2");
    insert_registry_row(
        &world,
        &spiffe_uri,
        &registered,
        RegistryState::Exact(&registered),
    )
    .await;
    let server = start_server(&pki, world.pool.clone(), world.pool.clone()).await;

    let status = bounded_service_result(
        channel(&pki, server.addr, &copied)
            .await
            .expect("copied leaf TLS"),
        Service::ExtProcNoContext,
        world.team_id,
        world.dataplane_id,
    )
    .await
    .expect_err("ExtProc no-context branch must not bypass exact authentication");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

struct LiveStream {
    _inbound: LiveInbound,
    outcome: tokio::task::JoinHandle<Result<(), tonic::Status>>,
}

enum LiveInbound {
    Ads(tokio::sync::mpsc::Sender<DiscoveryRequest>),
    Diagnostics(tokio::sync::mpsc::Sender<DiagnosticsReport>),
    AccessLog(tokio::sync::mpsc::Sender<StreamAccessLogsMessage>),
    ExtProc(tokio::sync::mpsc::Sender<ProcessingRequest>),
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        match &self._inbound {
            LiveInbound::Ads(tx) => drop(tx.clone()),
            LiveInbound::Diagnostics(tx) => drop(tx.clone()),
            LiveInbound::AccessLog(tx) => drop(tx.clone()),
            LiveInbound::ExtProc(tx) => drop(tx.clone()),
        }
        self.outcome.abort();
    }
}

async fn drain_responses<M>(mut responses: tonic::Streaming<M>) -> Result<(), tonic::Status>
where
    M: prost::Message + Default,
{
    loop {
        if responses.message().await?.is_none() {
            return Err(tonic::Status::unknown(
                "certificate-bound stream closed without a terminal status",
            ));
        }
    }
}

async fn open_live_stream(
    channel: Channel,
    service: Service,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
) -> LiveStream {
    match service {
        Service::Ads => {
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(DiscoveryRequest {
                node: Some(Node {
                    id: format!("dataplane/{dataplane_id}"),
                    ..Default::default()
                }),
                type_url: CLUSTER_TYPE_URL.to_owned(),
                ..Default::default()
            })
            .await
            .expect("prime ADS stream");
            let responses = tokio::time::timeout(DEADLINE, async move {
                AggregatedDiscoveryServiceClient::new(channel)
                    .stream_aggregated_resources(tokio_stream::wrappers::ReceiverStream::new(rx))
                    .await
            })
            .await
            .expect("ADS must open within the deadline")
            .expect("ADS exact leaf must authenticate")
            .into_inner();
            LiveStream {
                _inbound: LiveInbound::Ads(tx),
                outcome: tokio::spawn(drain_responses(responses)),
            }
        }
        Service::Diagnostics => {
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(DiagnosticsReport {
                schema_version: 1,
                report_id: uuid::Uuid::now_v7().to_string(),
                dataplane_id: dataplane_id.to_string(),
                observed_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                payload: Some(diagnostics_report::Payload::Heartbeat(HeartbeatReport {
                    requests_delta: 0,
                    errors_delta: 0,
                    warming_failures_delta: 0,
                    config_verified: true,
                })),
            })
            .await
            .expect("prime diagnostics stream");
            let responses = tokio::time::timeout(DEADLINE, async move {
                EnvoyDiagnosticsServiceClient::new(channel)
                    .report_diagnostics(tokio_stream::wrappers::ReceiverStream::new(rx))
                    .await
            })
            .await
            .expect("diagnostics must open within the deadline")
            .expect("diagnostics exact leaf must authenticate")
            .into_inner();
            LiveStream {
                _inbound: LiveInbound::Diagnostics(tx),
                outcome: tokio::spawn(drain_responses(responses)),
            }
        }
        Service::AccessLog => {
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(StreamAccessLogsMessage {
                identifier: Some(stream_access_logs_message::Identifier {
                    node: Some(Node {
                        id: format!("dataplane/{dataplane_id}"),
                        ..Default::default()
                    }),
                    log_name: "fpv2-7f3.5-selective-revocation".to_owned(),
                }),
                log_entries: Some(stream_access_logs_message::LogEntries::HttpLogs(
                    stream_access_logs_message::HttpAccessLogEntries {
                        log_entry: Vec::new(),
                    },
                )),
            })
            .await
            .expect("prime access-log stream");
            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            request.metadata_mut().insert(
                "x-flowplane-team-id",
                team_id.to_string().parse().expect("team metadata"),
            );
            request.metadata_mut().insert(
                "x-flowplane-capture-session-id",
                uuid::Uuid::now_v7()
                    .to_string()
                    .parse()
                    .expect("capture metadata"),
            );
            request.metadata_mut().insert(
                "x-flowplane-route-config-id",
                uuid::Uuid::now_v7()
                    .to_string()
                    .parse()
                    .expect("route metadata"),
            );
            let outcome = tokio::spawn(async move {
                match AccessLogServiceClient::new(channel)
                    .stream_access_logs(request)
                    .await
                {
                    Ok(_) => Err(tonic::Status::unknown(
                        "access-log stream closed without a terminal status",
                    )),
                    Err(status) => Err(status),
                }
            });
            // ALS is client-streaming and has no open acknowledgement. A bounded settling
            // window lets the first message cross the real mTLS/registry boundary.
            tokio::time::sleep(Duration::from_millis(250)).await;
            assert!(!outcome.is_finished(), "access-log stream must be live");
            LiveStream {
                _inbound: LiveInbound::AccessLog(tx),
                outcome,
            }
        }
        Service::ExtProc | Service::ExtProcCapture => {
            let (tx, rx) = tokio::sync::mpsc::channel::<ProcessingRequest>(2);
            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            let metadata = request.metadata_mut();
            metadata.insert(
                "x-flowplane-team-id",
                team_id.to_string().parse().expect("team metadata"),
            );
            if matches!(service, Service::ExtProc) {
                metadata.insert("x-flowplane-ai-processor", "true".parse().unwrap());
                metadata.insert(
                    "x-flowplane-ai-provider-id",
                    uuid::Uuid::now_v7().to_string().parse().unwrap(),
                );
                metadata.insert("x-flowplane-ai-backend-position", "0".parse().unwrap());
            } else {
                metadata.insert(
                    "x-flowplane-capture-session-id",
                    uuid::Uuid::now_v7().to_string().parse().unwrap(),
                );
            }
            metadata.insert(
                "x-flowplane-route-config-id",
                uuid::Uuid::now_v7().to_string().parse().unwrap(),
            );
            let responses = tokio::time::timeout(DEADLINE, async move {
                ExternalProcessorClient::new(channel).process(request).await
            })
            .await
            .expect("ExtProc must open within the deadline")
            .expect("ExtProc exact leaf must authenticate")
            .into_inner();
            LiveStream {
                _inbound: LiveInbound::ExtProc(tx),
                outcome: tokio::spawn(drain_responses(responses)),
            }
        }
        Service::ExtProcNoContext => unreachable!("not a certificate-lifecycle service variant"),
    }
}

async fn assert_stream_stays_live(stream: &mut LiveStream, service: Service, phase: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut stream.outcome)
            .await
            .is_err(),
        "{service:?} stream terminated during {phase}"
    );
}

async fn assert_permission_denied(mut stream: LiveStream, service: Service) {
    let status = tokio::time::timeout(DEADLINE, &mut stream.outcome)
        .await
        .unwrap_or_else(|_| {
            panic!("{service:?} did not terminate after exact-certificate revocation")
        })
        .expect("stream drain task")
        .expect_err("revoked stream must terminate with a status");
    assert_eq!(
        status.code(),
        tonic::Code::PermissionDenied,
        "{service:?} must fail closed with permission denied"
    );
}

async fn assert_two_revocation_subscribers(
    revocations: &tokio::sync::broadcast::Sender<uuid::Uuid>,
    service: Service,
) {
    for _ in 0..40 {
        if revocations.receiver_count() >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "{service:?} did not subscribe both certificate-bound streams to the revocation bus within one second"
    );
}

#[tokio::test]
async fn revoking_one_exact_certificate_selectively_terminates_all_mtls_service_paths() {
    let Some(old_world) = world().await else {
        return;
    };
    let Some(foreign_world) = world().await else {
        return;
    };
    let pki = TestPki::new();
    let spiffe_uri = format!(
        "spiffe://flowplane.test/dataplane/{}/rotation",
        old_world.dataplane_id
    );
    let foreign_spiffe_uri = format!(
        "spiffe://flowplane.test/dataplane/{}/foreign",
        foreign_world.dataplane_id
    );
    let old_leaf = pki.issue_client("revocation-old", &spiffe_uri, "e1");
    let foreign_leaf = pki.issue_client("revocation-foreign", &foreign_spiffe_uri, "e2");
    let old_certificate_id = insert_registry_row(
        &old_world,
        &spiffe_uri,
        &old_leaf,
        RegistryState::Exact(&old_leaf),
    )
    .await;
    let foreign_certificate_id = insert_registry_row(
        &foreign_world,
        &foreign_spiffe_uri,
        &foreign_leaf,
        RegistryState::Exact(&foreign_leaf),
    )
    .await;
    let server = start_server(&pki, old_world.pool.clone(), old_world.pool.clone()).await;

    for service in SERVICES {
        let mut old_stream = open_live_stream(
            channel(&pki, server.addr, &old_leaf)
                .await
                .expect("old leaf TLS handshake"),
            service,
            old_world.team_id,
            old_world.dataplane_id,
        )
        .await;
        let mut foreign_stream = open_live_stream(
            channel(&pki, server.addr, &foreign_leaf)
                .await
                .expect("foreign leaf TLS handshake"),
            service,
            foreign_world.team_id,
            foreign_world.dataplane_id,
        )
        .await;

        assert_two_revocation_subscribers(&server.revocations, service).await;
        server
            .revocations
            .send(uuid::Uuid::now_v7())
            .expect("broadcast unrelated certificate id");
        assert_stream_stays_live(&mut old_stream, service, "unrelated revocation").await;
        assert_stream_stays_live(&mut foreign_stream, service, "unrelated revocation").await;

        server
            .revocations
            .send(old_certificate_id)
            .expect("broadcast old certificate id");
        assert_permission_denied(old_stream, service).await;
        assert_stream_stays_live(&mut foreign_stream, service, "old-certificate revocation").await;

        server
            .revocations
            .send(foreign_certificate_id)
            .expect("bounded cleanup of foreign stream");
        assert_permission_denied(foreign_stream, service).await;
    }
}

#[tokio::test]
async fn restart_and_rotation_keep_both_exact_leaves_until_old_is_revoked() {
    let Some(world) = world().await else { return };
    let pki = TestPki::new();
    let spiffe_uri = format!(
        "spiffe://flowplane.test/dataplane/{}/bounded-rotation",
        world.dataplane_id
    );
    let old_leaf = pki.issue_client("bounded-rotation-old", &spiffe_uri, "f621");
    let replacement_leaf = pki.issue_client("bounded-rotation-replacement", &spiffe_uri, "f622");
    let old_certificate_id = insert_registry_row(
        &world,
        &spiffe_uri,
        &old_leaf,
        RegistryState::Exact(&old_leaf),
    )
    .await;
    let replacement_certificate_id = insert_registry_row(
        &world,
        &spiffe_uri,
        &replacement_leaf,
        RegistryState::Exact(&replacement_leaf),
    )
    .await;
    assert_ne!(old_certificate_id, replacement_certificate_id);

    // The old exact credential survives a real listener restart without issuing a new identity.
    let first_server = start_server(&pki, world.pool.clone(), world.pool.clone()).await;
    bounded_service_result(
        channel(&pki, first_server.addr, &old_leaf)
            .await
            .expect("old leaf TLS before restart"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await
    .expect("old exact leaf authenticates before restart");
    drop(first_server);

    let restarted = start_server(&pki, world.pool.clone(), world.pool.clone()).await;
    let mut old_stream = open_live_stream(
        channel(&pki, restarted.addr, &old_leaf)
            .await
            .expect("old leaf TLS after restart"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await;
    let mut replacement_stream = open_live_stream(
        channel(&pki, restarted.addr, &replacement_leaf)
            .await
            .expect("replacement leaf TLS after restart"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await;
    assert_two_revocation_subscribers(&restarted.revocations, Service::Ads).await;
    assert_stream_stays_live(&mut old_stream, Service::Ads, "overlap before revocation").await;
    assert_stream_stays_live(
        &mut replacement_stream,
        Service::Ads,
        "overlap before revocation",
    )
    .await;

    restarted
        .revocations
        .send(old_certificate_id)
        .expect("broadcast old exact credential revocation");
    assert_permission_denied(old_stream, Service::Ads).await;
    assert_stream_stays_live(
        &mut replacement_stream,
        Service::Ads,
        "old exact credential revocation",
    )
    .await;

    // The replacement remains an independently resolvable exact credential on a new connection.
    bounded_service_result(
        channel(&pki, restarted.addr, &replacement_leaf)
            .await
            .expect("replacement TLS reconnect after old revocation"),
        Service::Ads,
        world.team_id,
        world.dataplane_id,
    )
    .await
    .expect("replacement exact leaf reconnects after old revocation");

    restarted
        .revocations
        .send(replacement_certificate_id)
        .expect("bounded replacement stream cleanup");
    assert_permission_denied(replacement_stream, Service::Ads).await;
}

#[derive(Clone)]
struct InjectedIdentityResolver {
    identity: PeerIdentity,
}

#[fp_xds::async_trait]
impl TeamResolver for InjectedIdentityResolver {
    async fn resolve(
        &self,
        _node_id: &str,
        _peer_certificate: Option<&fp_xds::server::PresentedCertificateIdentity>,
    ) -> Result<PeerIdentity, tonic::Status> {
        Ok(self.identity)
    }
}

#[tokio::test]
async fn test_injection_retains_the_complete_peer_identity_shape() {
    let expected = PeerIdentity {
        team_id: TeamId::from(uuid::Uuid::now_v7()),
        dataplane_id: Some(DataplaneId::from(uuid::Uuid::now_v7())),
        certificate_id: Some(uuid::Uuid::now_v7()),
    };
    let resolved = InjectedIdentityResolver { identity: expected }
        .resolve("attribution-only", None)
        .await
        .expect("test resolver identity");
    assert_eq!(resolved.team_id, expected.team_id);
    assert_eq!(resolved.dataplane_id, expected.dataplane_id);
    assert_eq!(resolved.certificate_id, expected.certificate_id);
}
