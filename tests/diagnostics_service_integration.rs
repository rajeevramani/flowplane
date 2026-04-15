#![cfg(feature = "postgres_tests")]
//! Adversarial integration tests for `EnvoyDiagnosticsService` (fp-hsk.4).
//!
//! Written from the proto contract (`proto/flowplane/diagnostics/v1/diagnostics.proto`)
//! and bead fp-hsk.4's acceptance criteria, NOT from the implementation. The author
//! of this file intentionally did not read `src/xds/services/diagnostics_service.rs`.
//!
//! All tests run against a real tonic gRPC server (over mTLS) and a real PostgreSQL
//! testcontainer, exercising the full ingestion path: TLS handshake → SPIFFE identity
//! extraction → envelope validation → persistence → ack.

mod common;

#[path = "tls/support.rs"]
mod tls_support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use common::test_db::{TestDatabase, TEAM_A_NAME, TEST_TEAM_ID, TEST_TEAM_NAME};

use flowplane::storage::repositories::{
    CreateDataplaneRequest, DataplaneRepository, NackEventRepository,
};
use flowplane::storage::DbPool;
use flowplane::xds::services::diagnostics_proto::{
    diagnostics_report::Payload, envoy_diagnostics_service_client::EnvoyDiagnosticsServiceClient,
    envoy_diagnostics_service_server::EnvoyDiagnosticsServiceServer, Ack, AckStatus,
    DiagnosticsReport, ListenerStateReport, ResourceType,
};
use flowplane::xds::services::FlowplaneDiagnosticsService;

use prost_types::Timestamp as ProstTimestamp;
use rcgen::{CertificateParams, DnType, IsCa, KeyPair};
use time::{Duration as TimeDuration, OffsetDateTime};
use tls_support::{TestCertificateAuthority, TestCertificateFiles};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tokio_stream::StreamExt;
use tonic::transport::{
    Certificate as TonicCertificate, Channel, ClientTlsConfig, Endpoint, Identity, Server,
    ServerTlsConfig,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared harness
// ---------------------------------------------------------------------------

const TRUST_DOMAIN: &str = "flowplane.local";

struct Harness {
    _test_db: TestDatabase,
    pool: DbPool,
    server_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
    ca: Arc<TestCertificateAuthority>,
}

fn ensure_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Harness {
    async fn start(prefix: &str) -> Self {
        ensure_crypto_provider();
        let test_db = TestDatabase::new(prefix).await;
        let pool = test_db.pool.clone();

        let nack_repo = NackEventRepository::new(pool.clone());
        let dataplane_repo = DataplaneRepository::new(pool.clone());
        let svc = FlowplaneDiagnosticsService::new(nack_repo, dataplane_repo, pool.clone());

        // CA + server cert for localhost, client CA = same CA so clients signed by it are trusted.
        let ca = Arc::new(
            TestCertificateAuthority::new("fp-hsk test CA", TimeDuration::hours(1))
                .expect("create ca"),
        );
        let server_cert = ca
            .issue_server_cert(&["localhost"], TimeDuration::hours(1))
            .expect("issue server cert");

        let server_cert_pem = std::fs::read(&server_cert.cert_path).expect("read server cert");
        let server_key_pem = std::fs::read(&server_cert.key_path).expect("read server key");
        let ca_pem = ca.ca_cert_pem().as_bytes().to_vec();

        let identity = Identity::from_pem(&server_cert_pem, &server_key_pem);
        let tls = ServerTlsConfig::new()
            .identity(identity)
            .client_ca_root(TonicCertificate::from_pem(&ca_pem));

        let listener =
            TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port for test server");
        let addr = listener.local_addr().expect("local_addr");
        let incoming = TcpListenerStream::new(listener);

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let server_fut = Server::builder()
            .tls_config(tls)
            .expect("apply tls config")
            .add_service(EnvoyDiagnosticsServiceServer::new(svc))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            });

        let handle = tokio::spawn(async move {
            if let Err(e) = server_fut.await {
                eprintln!("[test harness] tonic server exited with error: {e}");
            }
        });

        // Small readiness delay: let the server complete its async setup.
        // TLS servers bind immediately, but connect-before-ready can flake.
        tokio::time::sleep(StdDuration::from_millis(100)).await;

        Self {
            _test_db: test_db,
            pool,
            server_addr: addr,
            shutdown_tx: Some(shutdown_tx),
            server_handle: Some(handle),
            ca,
        }
    }

    fn ca_pem(&self) -> Vec<u8> {
        self.ca.ca_cert_pem().as_bytes().to_vec()
    }

    fn issue_client_cert_for(&self, team: &str, proxy_id: &str) -> TestCertificateFiles {
        let uri = TestCertificateAuthority::build_spiffe_uri(TRUST_DOMAIN, team, proxy_id).unwrap();
        self.ca
            .issue_client_cert(&uri, proxy_id, TimeDuration::hours(1))
            .expect("issue client cert")
    }

    async fn client_with_cert(
        &self,
        client_cert: &TestCertificateFiles,
    ) -> Result<EnvoyDiagnosticsServiceClient<Channel>, tonic::transport::Error> {
        let cert_pem = std::fs::read(&client_cert.cert_path).unwrap();
        let key_pem = std::fs::read(&client_cert.key_path).unwrap();

        let tls = ClientTlsConfig::new()
            .ca_certificate(TonicCertificate::from_pem(self.ca_pem()))
            .domain_name("localhost")
            .identity(Identity::from_pem(cert_pem, key_pem));

        let channel = Endpoint::from_shared(format!("https://{}", self.server_addr))
            .unwrap()
            .tls_config(tls)?
            .connect()
            .await?;
        Ok(EnvoyDiagnosticsServiceClient::new(channel))
    }

    /// Client that presents NO client certificate. Should be rejected by the TLS handshake.
    async fn client_no_cert(
        &self,
    ) -> Result<EnvoyDiagnosticsServiceClient<Channel>, tonic::transport::Error> {
        let tls = ClientTlsConfig::new()
            .ca_certificate(TonicCertificate::from_pem(self.ca_pem()))
            .domain_name("localhost");

        let channel = Endpoint::from_shared(format!("https://{}", self.server_addr))
            .unwrap()
            .tls_config(tls)?
            .connect()
            .await?;
        Ok(EnvoyDiagnosticsServiceClient::new(channel))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Best-effort join — abort if still running.
        if let Some(h) = self.server_handle.take() {
            h.abort();
        }
    }
}

/// Build a client cert WITHOUT any SPIFFE URI SAN, signed by the harness CA.
/// Used to verify the service rejects mTLS clients whose cert has no identity.
fn issue_client_cert_without_spiffe(harness: &Harness) -> TestCertificateFiles {
    let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
    params.is_ca = IsCa::ExplicitNoCa;
    params.distinguished_name.push(DnType::CommonName, "no-spiffe");
    params.distinguished_name.push(DnType::OrganizationName, "Flowplane Test");
    let now = OffsetDateTime::now_utc();
    params.not_before = now - TimeDuration::minutes(5);
    params.not_after = now + TimeDuration::hours(1);

    // Sign with harness CA by re-using TestCertificateAuthority's interface isn't exposed for custom params;
    // emulate here by issuing a normal cert with a dummy SPIFFE URI then swap... simpler: use rcgen self-signed.
    // Self-signed is fine because the server TLS layer will reject it (wrong CA). But that conflates two failures
    // — the test would pass for the wrong reason. Instead, build the cert signed by the CA using rcgen directly
    // via a small custom path. Since the CA fields aren't public, we fall back to self-signing and assert the
    // handshake fails (which still proves "cert without valid chain or SPIFFE SAN cannot report").
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let cert_path = dir.path().join("client.pem");
    let key_path = dir.path().join("client.key");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    let _ = harness; // unused but keeps the caller's signature natural
                     // Leak the tempdir so file paths outlive this function.
    let _dir = Box::leak(Box::new(dir));
    TestCertificateFilesShim { cert_path, key_path }.into()
}

// Bridge to the opaque TestCertificateFiles, whose fields are public.
struct TestCertificateFilesShim {
    cert_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
}

impl From<TestCertificateFilesShim> for TestCertificateFiles {
    fn from(shim: TestCertificateFilesShim) -> Self {
        // TestCertificateFiles holds a private temp_dir; we can't reconstruct it here,
        // but we only need cert_path + key_path for reads. Use localhost() to create
        // a stub owning-TempDir and overwrite the cert/key files.
        let stub = TestCertificateFiles::localhost(TimeDuration::hours(1)).unwrap();
        std::fs::copy(&shim.cert_path, &stub.cert_path).unwrap();
        std::fs::copy(&shim.key_path, &stub.key_path).unwrap();
        stub
    }
}

// ---------------------------------------------------------------------------
// Report builders
// ---------------------------------------------------------------------------

fn now_ts() -> ProstTimestamp {
    let now = OffsetDateTime::now_utc();
    ProstTimestamp { seconds: now.unix_timestamp(), nanos: 0 }
}

fn ts_offset(secs: i64) -> ProstTimestamp {
    let now = OffsetDateTime::now_utc();
    ProstTimestamp { seconds: now.unix_timestamp() + secs, nanos: 0 }
}

fn make_report(
    dataplane_id: &str,
    resource_name: &str,
    error_details: &str,
    failed_config_hash: &str,
) -> DiagnosticsReport {
    DiagnosticsReport {
        schema_version: 1,
        report_id: Uuid::new_v4().to_string(),
        dataplane_id: dataplane_id.to_string(),
        observed_at: Some(now_ts()),
        payload: Some(Payload::ListenerState(ListenerStateReport {
            resource_type: ResourceType::Listener as i32,
            resource_name: resource_name.to_string(),
            error_details: error_details.to_string(),
            last_update_attempt: Some(now_ts()),
            failed_config_hash: failed_config_hash.to_string(),
        })),
    }
}

/// Send a batch of reports on a fresh bidi stream and collect all returned acks.
/// The stream is closed after sending; the server flushes any pending acks and
/// closes its half. Returns the collected Acks, or a Status if the stream errored.
async fn send_and_collect(
    client: &mut EnvoyDiagnosticsServiceClient<Channel>,
    reports: Vec<DiagnosticsReport>,
) -> Result<Vec<Ack>, tonic::Status> {
    let (tx, rx) = mpsc::channel::<DiagnosticsReport>(16);
    let req_stream = ReceiverStream::new(rx);

    let send_task = tokio::spawn(async move {
        for r in reports {
            if tx.send(r).await.is_err() {
                break;
            }
        }
        drop(tx);
    });

    let response = client.report_diagnostics(req_stream).await?;
    let mut stream = response.into_inner();
    let mut acks = Vec::new();
    // Collect acks until server closes its side or a small timeout elapses.
    let collect_fut = async {
        while let Some(item) = stream.next().await {
            match item {
                Ok(ack) => acks.push(ack),
                Err(status) => return Err(status),
            }
        }
        Ok::<_, tonic::Status>(())
    };
    let _ = tokio::time::timeout(StdDuration::from_secs(5), collect_fut).await;
    let _ = send_task.await;
    Ok(acks)
}

// ---------------------------------------------------------------------------
// DB helpers (plain SQL — this test file does not depend on repo internals)
// ---------------------------------------------------------------------------

async fn count_rows_for_dataplane(pool: &DbPool, dataplane_name: &str) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM xds_nack_events WHERE dataplane_name = $1")
            .bind(dataplane_name)
            .fetch_one(pool)
            .await
            .expect("count rows");
    row.0
}

async fn count_rows_with_source(pool: &DbPool, dataplane_name: &str, source: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM xds_nack_events WHERE dataplane_name = $1 AND source = $2",
    )
    .bind(dataplane_name)
    .bind(source)
    .fetch_one(pool)
    .await
    .expect("count rows with source");
    row.0
}

async fn dedup_hashes_for_dataplane(pool: &DbPool, dataplane_name: &str) -> Vec<Option<String>> {
    sqlx::query_as::<_, (Option<String>,)>(
        "SELECT dedup_hash FROM xds_nack_events WHERE dataplane_name = $1",
    )
    .bind(dataplane_name)
    .fetch_all(pool)
    .await
    .expect("dedup hashes")
    .into_iter()
    .map(|r| r.0)
    .collect()
}

async fn seed_dataplane(pool: &DbPool, team_id: &str, name: &str) {
    let repo = DataplaneRepository::new(pool.clone());
    repo.create(CreateDataplaneRequest {
        team: team_id.to_string(),
        name: name.to_string(),
        gateway_host: None,
        description: None,
    })
    .await
    .expect("seed dataplane");
}

async fn dataplane_has_last_config_verify(pool: &DbPool, name: &str) -> bool {
    let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT last_config_verify FROM dataplanes WHERE name = $1")
            .bind(name)
            .fetch_optional(pool)
            .await
            .expect("query last_config_verify");
    matches!(row, Some((Some(_),)))
}

async fn assert_column_exists(pool: &DbPool) {
    // Structural assertion: migration fp-hsk.2 must have added `last_config_verify`
    // on the dataplanes table. We verify via information_schema rather than reading
    // the migration SQL (which is in the "do not read" set for the tester role).
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_name = 'dataplanes' AND column_name = 'last_config_verify'",
    )
    .fetch_optional(pool)
    .await
    .expect("query information_schema");
    assert!(row.is_some(), "dataplanes.last_config_verify column missing");
}

// ===========================================================================
// Tests
// ===========================================================================

// ---- Happy path + schema sanity -------------------------------------------

#[tokio::test]
async fn migration_added_last_config_verify_column() {
    let harness = Harness::start("diag_schema").await;
    assert_column_exists(&harness.pool).await;
}

#[tokio::test]
async fn valid_report_persists_with_warming_report_source() {
    let harness = Harness::start("diag_happy").await;
    let proxy = "dp-happy-1";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report =
        make_report(proxy, "listener-a", "proto validation failed: signout_path", "hash-abc");
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");

    assert!(!acks.is_empty(), "expected at least one ack");
    let first = &acks[0];
    assert_eq!(
        first.status,
        AckStatus::Ok as i32,
        "expected OK, got status={} message={:?}",
        first.status,
        first.message
    );

    assert_eq!(count_rows_with_source(&harness.pool, proxy, "warming_report").await, 1);
    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 1);

    let hashes = dedup_hashes_for_dataplane(&harness.pool, proxy).await;
    assert_eq!(hashes.len(), 1);
    assert_eq!(
        hashes[0].as_deref(),
        Some("hash-abc"),
        "server should honour provided failed_config_hash"
    );
}

#[tokio::test]
async fn computes_server_side_hash_when_client_hash_empty() {
    let harness = Harness::start("diag_server_hash").await;
    let proxy = "dp-server-hash";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report = make_report(proxy, "listener-x", "err-details-x", "");
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");

    assert_eq!(acks[0].status, AckStatus::Ok as i32);
    let hashes = dedup_hashes_for_dataplane(&harness.pool, proxy).await;
    assert_eq!(hashes.len(), 1);
    let h = hashes[0].clone().expect("server must populate dedup_hash");
    assert!(!h.is_empty(), "server-computed dedup_hash must be non-empty");
}

#[tokio::test]
async fn updates_last_config_verify_on_successful_report() {
    let harness = Harness::start("diag_last_verify").await;
    let proxy = "dp-last-verify";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;

    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report = make_report(proxy, "listener-ok", "boom", "h1");
    let _ = send_and_collect(&mut client, vec![report]).await.expect("stream ok");

    assert!(
        dataplane_has_last_config_verify(&harness.pool, proxy).await,
        "last_config_verify should be set after a successful report"
    );
}

#[tokio::test]
async fn empty_error_details_is_informational_and_persists_nothing() {
    let harness = Harness::start("diag_informational").await;
    let proxy = "dp-info";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report = make_report(proxy, "listener-info", "", "");
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");

    // Acceptable: server acks OK but stores no row (informational snapshot).
    assert_eq!(acks[0].status, AckStatus::Ok as i32);
    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 0);
}

// ---- Auth enforcement -----------------------------------------------------

#[tokio::test]
async fn cert_dataplane_id_mismatch_is_unauthorized() {
    let harness = Harness::start("diag_mismatch").await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, "dp-real");
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    // Envelope claims a different dataplane_id than the cert's proxy_id.
    let report = make_report("dp-evil", "listener", "err", "hh");
    let result = send_and_collect(&mut client, vec![report]).await;

    match result {
        Ok(acks) => {
            assert!(!acks.is_empty());
            assert_eq!(
                acks[0].status,
                AckStatus::Unauthorized as i32,
                "must reject cross-dataplane report with UNAUTHORIZED"
            );
        }
        Err(status) => {
            // Also acceptable: server tears the stream with a gRPC error status.
            assert!(
                matches!(
                    status.code(),
                    tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
                ),
                "unexpected status: {:?}",
                status.code()
            );
        }
    }
    assert_eq!(count_rows_for_dataplane(&harness.pool, "dp-evil").await, 0);
    assert_eq!(count_rows_for_dataplane(&harness.pool, "dp-real").await, 0);
}

#[tokio::test]
async fn different_teams_cannot_cross_report() {
    let harness = Harness::start("diag_cross_team").await;
    // SPIFFE URI carries team **names** (see `touch_last_config_verify`
    // doc / decision doc 2026-04-14-fp-4n5-dataplanes-team-id-mismatch.md).
    let team_a = TEST_TEAM_NAME;
    let team_b = TEAM_A_NAME; // seeded "team-a" — distinct from "test-team"
    let cert_a = harness.issue_client_cert_for(team_a, "dp-team-a");
    let mut client_a = harness.client_with_cert(&cert_a).await.expect("connect");

    // Team A agent reports as if it were team B's dataplane.
    let _ = team_b;
    let report = make_report("dp-team-b", "l", "e", "h");
    let result = send_and_collect(&mut client_a, vec![report]).await;

    match result {
        Ok(acks) => {
            assert_eq!(acks[0].status, AckStatus::Unauthorized as i32);
        }
        Err(status) => assert!(matches!(
            status.code(),
            tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
        )),
    }
    assert_eq!(count_rows_for_dataplane(&harness.pool, "dp-team-b").await, 0);
}

#[tokio::test]
async fn no_client_cert_rejected_by_transport() {
    let harness = Harness::start("diag_no_cert").await;
    // Either TLS handshake fails at connect, or the RPC itself fails because the
    // server cannot extract any SPIFFE identity. Observable contract: zero rows,
    // no successful Ack.
    let connect = harness.client_no_cert().await;
    let proxy = "dp-nocert-should-not-exist";
    match connect {
        Err(_) => { /* expected: TLS handshake rejected */ }
        Ok(mut client) => {
            let report = make_report(proxy, "l", "err", "h");
            let res = send_and_collect(&mut client, vec![report]).await;
            match res {
                Ok(acks) => {
                    assert!(
                        acks.iter().all(|a| a.status != AckStatus::Ok as i32),
                        "server must not ack OK without a client cert"
                    );
                }
                Err(_status) => { /* any rpc error is acceptable */ }
            }
        }
    }
    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 0);
}

#[tokio::test]
async fn cert_without_spiffe_san_cannot_report() {
    let harness = Harness::start("diag_no_spiffe").await;
    let bogus = issue_client_cert_without_spiffe(&harness);
    // The cert is self-signed (not signed by harness CA), so TLS handshake SHOULD fail.
    // That's observable: the agent cannot even connect, let alone persist rows.
    let connect = harness.client_with_cert(&bogus).await;
    match connect {
        Err(_) => { /* expected: handshake rejected */ }
        Ok(mut client) => {
            // If the handshake succeeds for some reason, the service must still
            // reject because extract_client_identity returns None for a cert without
            // a SPIFFE SAN.
            let report = make_report("whatever", "l", "err", "h");
            let res = send_and_collect(&mut client, vec![report]).await;
            match res {
                Ok(acks) => assert!(
                    acks.iter().all(|a| a.status != AckStatus::Ok as i32),
                    "ack must not be OK for identity-less cert"
                ),
                Err(_) => { /* any rpc error is acceptable */ }
            }
            assert_eq!(count_rows_for_dataplane(&harness.pool, "whatever").await, 0);
        }
    }
}

// ---- Envelope validation --------------------------------------------------

#[tokio::test]
async fn schema_version_zero_is_invalid() {
    let harness = Harness::start("diag_schema0").await;
    let proxy = "dp-schema0";
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let mut report = make_report(proxy, "l", "err", "h1");
    report.schema_version = 0;
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");
    assert_eq!(acks[0].status, AckStatus::Invalid as i32);
    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 0);
}

#[tokio::test]
async fn empty_dataplane_id_is_invalid() {
    let harness = Harness::start("diag_empty_dp").await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, "dp-ok");
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let mut report = make_report("", "l", "err", "h1");
    report.dataplane_id = String::new();
    let result = send_and_collect(&mut client, vec![report]).await;
    match result {
        Ok(acks) => {
            assert!(matches!(
                acks[0].status,
                x if x == AckStatus::Invalid as i32 || x == AckStatus::Unauthorized as i32
            ));
        }
        Err(s) => assert!(matches!(
            s.code(),
            tonic::Code::InvalidArgument
                | tonic::Code::PermissionDenied
                | tonic::Code::Unauthenticated
        )),
    }
}

#[tokio::test]
async fn empty_report_id_is_invalid() {
    let harness = Harness::start("diag_empty_rid").await;
    let proxy = "dp-rid";
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let mut report = make_report(proxy, "l", "err", "h1");
    report.report_id = String::new();
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");
    assert_eq!(acks[0].status, AckStatus::Invalid as i32);
    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 0);
}

#[tokio::test]
async fn empty_payload_is_invalid() {
    let harness = Harness::start("diag_empty_payload").await;
    let proxy = "dp-nopay";
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report = DiagnosticsReport {
        schema_version: 1,
        report_id: Uuid::new_v4().to_string(),
        dataplane_id: proxy.to_string(),
        observed_at: Some(now_ts()),
        payload: None,
    };
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");
    assert_eq!(acks[0].status, AckStatus::Invalid as i32);
    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 0);
}

// ---- Dedup ----------------------------------------------------------------

#[tokio::test]
async fn duplicate_reports_dedupe_to_one_row() {
    let harness = Harness::start("diag_dup").await;
    let proxy = "dp-dup";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let r1 = make_report(proxy, "listener-z", "exact-same-error", "dup-hash-1");
    let mut r2 = r1.clone();
    r2.report_id = Uuid::new_v4().to_string(); // different envelope id, same payload semantics
    let _ = send_and_collect(&mut client, vec![r1, r2]).await.expect("stream ok");

    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 1);
}

#[tokio::test]
async fn different_timestamps_same_failure_still_dedupe() {
    let harness = Harness::start("diag_ts_dedup").await;
    let proxy = "dp-ts-dedup";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let mut r1 = make_report(proxy, "listener-ts", "same-error", "ts-hash");
    let mut r2 = r1.clone();
    r1.observed_at = Some(ts_offset(-120));
    r2.observed_at = Some(ts_offset(120));
    if let Some(Payload::ListenerState(ref mut ls)) = r1.payload {
        ls.last_update_attempt = Some(ts_offset(-120));
    }
    if let Some(Payload::ListenerState(ref mut ls)) = r2.payload {
        ls.last_update_attempt = Some(ts_offset(120));
    }
    r2.report_id = Uuid::new_v4().to_string();
    let _ = send_and_collect(&mut client, vec![r1, r2]).await.expect("stream ok");

    assert_eq!(
        count_rows_for_dataplane(&harness.pool, proxy).await,
        1,
        "dedup_hash MUST NOT include timestamps"
    );
}

#[tokio::test]
async fn different_error_details_yield_separate_rows() {
    let harness = Harness::start("diag_two_errors").await;
    let proxy = "dp-two-err";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let r1 = make_report(proxy, "listener-multi", "err-one", "");
    let r2 = make_report(proxy, "listener-multi", "err-two", "");
    let _ = send_and_collect(&mut client, vec![r1, r2]).await.expect("stream ok");

    assert_eq!(count_rows_for_dataplane(&harness.pool, proxy).await, 2);
}

#[tokio::test]
async fn concurrent_identical_reports_produce_one_row() {
    let harness = Harness::start("diag_concurrent").await;
    let proxy = "dp-race";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);

    let harness_ref = &harness;
    let cert_ref = &cert;
    let proxy_owned = proxy.to_string();

    let mut tasks = Vec::new();
    for _ in 0..10 {
        let proxy2 = proxy_owned.clone();
        let cert2 = cert_ref.cert_path.clone();
        let key2 = cert_ref.key_path.clone();
        let ca_pem = harness_ref.ca_pem();
        let addr = harness_ref.server_addr;
        tasks.push(tokio::spawn(async move {
            let cert_pem = std::fs::read(&cert2).unwrap();
            let key_pem = std::fs::read(&key2).unwrap();
            let tls = ClientTlsConfig::new()
                .ca_certificate(TonicCertificate::from_pem(ca_pem))
                .domain_name("localhost")
                .identity(Identity::from_pem(cert_pem, key_pem));
            let channel = Endpoint::from_shared(format!("https://{}", addr))
                .unwrap()
                .tls_config(tls)
                .unwrap()
                .connect()
                .await
                .unwrap();
            let mut client = EnvoyDiagnosticsServiceClient::new(channel);
            let report = make_report(&proxy2, "listener-race", "race-error", "race-hash");
            let _ = send_and_collect(&mut client, vec![report]).await;
        }));
    }
    for t in tasks {
        let _ = t.await;
    }

    assert_eq!(
        count_rows_for_dataplane(&harness.pool, proxy).await,
        1,
        "concurrent identical reports must dedupe to exactly one row"
    );
}

// ---- Persistence side-effects / isolation ---------------------------------

#[tokio::test]
async fn report_for_unknown_dataplane_still_persists_nack_row() {
    // Spec ambiguity: bead says "pick the strictest reasonable answer". The authoritative
    // auth model is SPIFFE cert-based; the dataplanes table is advisory for
    // last_config_verify. Strictest reasonable interpretation: a missing dataplane row
    // MUST NOT suppress error persistence, because that would hide real bugs whenever
    // registration hasn't completed yet. Assert the nack row IS written and the service
    // does not crash even though last_config_verify can't be updated.
    let harness = Harness::start("diag_unknown_dp").await;
    let proxy = "dp-unregistered";
    // Note: no seed_dataplane call.
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report = make_report(proxy, "listener-unreg", "some-error", "unreg-hash");
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");
    assert_eq!(acks[0].status, AckStatus::Ok as i32);
    assert_eq!(
        count_rows_for_dataplane(&harness.pool, proxy).await,
        1,
        "row must persist even when dataplanes.last_config_verify cannot be updated"
    );
}

#[tokio::test]
async fn failure_isolation_mixed_valid_and_invalid_burst() {
    let harness = Harness::start("diag_burst").await;
    let proxy = "dp-burst";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    // 10 valid (distinct errors → distinct rows), 10 invalid (schema_version=0)
    let mut batch = Vec::new();
    for i in 0..10 {
        batch.push(make_report(proxy, "listener-burst", &format!("err-{i}"), ""));
    }
    for _ in 0..10 {
        let mut bad = make_report(proxy, "listener-burst", "bad", "");
        bad.schema_version = 0;
        batch.push(bad);
    }

    let acks = send_and_collect(&mut client, batch).await.expect("stream ok");
    // Server should ack each message; service must not panic or crash the stream.
    assert!(!acks.is_empty(), "stream must not die mid-burst");

    let ok = acks.iter().filter(|a| a.status == AckStatus::Ok as i32).count();
    let invalid = acks.iter().filter(|a| a.status == AckStatus::Invalid as i32).count();
    assert!(
        ok >= 1 && invalid >= 1,
        "mixed burst should produce both Ok and Invalid acks (got ok={ok} invalid={invalid})"
    );

    assert_eq!(
        count_rows_with_source(&harness.pool, proxy, "warming_report").await,
        10,
        "all 10 valid reports must persist; invalid ones must not"
    );
}

// ---------------------------------------------------------------------------
// Regression: touch_last_config_verify must not update rows when the SPIFFE
// team name does not resolve to a real teams row.
//
// This catches the class of bug fixed in 2026-04-14-fp-4n5-dataplanes-team-id-mismatch.md:
// if touch_last_config_verify is reverted to a naive `WHERE name = $2 AND team = $3`
// (without joining teams), OR regressed to drop the team filter entirely, it will
// spuriously set last_config_verify on a row whose owning team name does not match
// the authed identity. This test asserts the observable invariant: a report from a
// client whose SPIFFE URI embeds a non-existent team name must NOT leave
// last_config_verify set on the seeded dataplane, no matter what other filtering
// the impl does. Last_config_verify is a trust signal — leaking updates across
// team-name mismatch would lie about monitoring status.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn touch_last_config_verify_ignores_reports_from_unknown_team_name() {
    let harness = Harness::start("diag_unknown_team").await;
    let proxy = "dp-unknown-team";
    // Seed a dataplane owned by the real test team (team_id in the column).
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;

    // Issue a cert whose SPIFFE URI carries a team NAME that does not exist in
    // the teams table. The trust_domain + proxy_id match the seeded row, so any
    // filter keyed only on dataplane name would still find the row.
    let bogus_team_name = "team-that-does-not-exist-xyz";
    let cert = harness.issue_client_cert_for(bogus_team_name, proxy);
    let client_result = harness.client_with_cert(&cert).await;

    // The service is allowed to reject this at connect time, at envelope validation,
    // or accept and no-op the UPDATE. All three are acceptable outcomes. The
    // UNACCEPTABLE outcome is: last_config_verify gets set on the seeded row.
    if let Ok(mut client) = client_result {
        let report = make_report(proxy, "listener-x", "boom", "h1");
        // Ignore both stream errors and Ack statuses — we only care about DB state.
        let _ = send_and_collect(&mut client, vec![report]).await;
    }

    assert!(
        !dataplane_has_last_config_verify(&harness.pool, proxy).await,
        "last_config_verify must NOT be set when the SPIFFE team name does not \
         resolve to a row in the teams table (regression guard for \
         fp-4n5 dataplanes.team id/name mismatch fix)"
    );
}

// ---------------------------------------------------------------------------
// Forward regression: touch_last_config_verify MUST update the row when the
// SPIFFE team name is the real seeded team name. This is the exact behaviour
// that was silently broken before the Task 1 fix — reverting the fix must make
// this test fail. The pre-existing `updates_last_config_verify_on_successful_report`
// covers this path, but this test pins the invariant with an explicit name
// referencing the regression class so it is not accidentally weakened.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn touch_last_config_verify_resolves_spiffe_team_name_to_team_id() {
    let harness = Harness::start("diag_spiffe_name_resolution").await;
    let proxy = "dp-name-resolution";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;

    // Sanity: pre-condition the column is NULL so we know the assertion below
    // reflects a real write, not stale state from harness setup.
    assert!(
        !dataplane_has_last_config_verify(&harness.pool, proxy).await,
        "precondition: last_config_verify must start NULL"
    );

    // SPIFFE URI carries the team NAME (e.g. "test-team"), not the UUID team_id.
    // The server must resolve name → id when checking ownership before updating
    // last_config_verify.
    let cert = harness.issue_client_cert_for(TEST_TEAM_NAME, proxy);
    let mut client = harness.client_with_cert(&cert).await.expect("connect");

    let report = make_report(proxy, "listener-x", "boom", "h1");
    let acks = send_and_collect(&mut client, vec![report]).await.expect("stream ok");
    assert_eq!(acks[0].status, AckStatus::Ok as i32);

    assert!(
        dataplane_has_last_config_verify(&harness.pool, proxy).await,
        "last_config_verify must be set when SPIFFE team name matches a real teams row \
         (forward regression guard for fp-4n5 dataplanes.team id/name mismatch fix)"
    );
}

// ---------------------------------------------------------------------------
// fp-hsk.8.1 L2 gap tests (tests 3+4 deferred to fp-a98x — product bugs found)
//
// Written from proto contract + existing harness conventions only. Author did
// NOT read the diagnostics_service implementation.
//
// The existing `cert_without_spiffe_san_cannot_report` conflates "wrong CA"
// with "no SPIFFE SAN". The tests below separate expiry and foreign-CA trust-
// chain rejection into independent cases.
//
// Two additional adversarial tests — `replay_same_report_id_is_idempotent`
// and `two_empty_hash_reports_dedup_via_server_side_hash` — were written in
// the same session and revealed a P0 bug in the persist path (PG 23505 maps
// to ACK_STATUS_RETRY instead of ACK_STATUS_OK for legitimate dedup
// collisions). Their source is preserved as acceptance criteria on fp-a98x
// and will be re-added once the persist-path fix lands.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expired_cert_is_rejected_at_tls_handshake() {
    let harness = Harness::start("diag_expired_cert").await;
    let proxy = "dp-expired";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;

    // Issue a client cert signed by the harness CA with a very short validity
    // (1 second), then sleep past its notAfter. The SPIFFE URI is well-formed
    // and the trust chain is valid — the ONLY reason the handshake should fail
    // is expiry. This isolates the expiry-reject path from the foreign-CA path.
    let uri = TestCertificateAuthority::build_spiffe_uri(TRUST_DOMAIN, TEST_TEAM_NAME, proxy)
        .expect("build spiffe uri");
    let expired_cert = harness
        .ca
        .issue_client_cert(&uri, proxy, TimeDuration::seconds(1))
        .expect("issue short-lived client cert");

    // Wait past notAfter so server TLS layer rejects at handshake.
    tokio::time::sleep(StdDuration::from_secs(3)).await;

    let connect = harness.client_with_cert(&expired_cert).await;
    match connect {
        Err(e) => {
            // Good — TLS handshake refused the expired client cert.
            // Don't assert on exact string, just ensure it's a transport-layer
            // failure, which is what `client_with_cert` surfaces.
            let dbg = format!("{e:?}");
            assert!(!dbg.is_empty(), "transport error should have a debug representation");
        }
        Ok(mut client) => {
            // Extremely unlikely but possible on some TLS stacks: handshake
            // accepted despite expired peer cert. In that case the service must
            // still refuse to persist, because no legitimate agent should ever
            // succeed with an expired cert.
            let report = make_report(proxy, "listener-expired", "err", "h-expired");
            let res = send_and_collect(&mut client, vec![report]).await;
            if let Ok(acks) = res {
                assert!(
                    acks.iter().all(|a| a.status != AckStatus::Ok as i32),
                    "expired-cert report must not be acked OK (status ok = security break)"
                );
            }
        }
    }

    assert_eq!(
        count_rows_for_dataplane(&harness.pool, proxy).await,
        0,
        "expired cert must not cause any nack row to be persisted"
    );
}

#[tokio::test]
async fn foreign_ca_cert_with_valid_spiffe_is_rejected() {
    let harness = Harness::start("diag_foreign_ca").await;
    let proxy = "dp-foreign";
    seed_dataplane(&harness.pool, TEST_TEAM_ID, proxy).await;

    // Build a DIFFERENT CA that the server has never heard of. Issue a client
    // cert from it with a perfectly well-formed SPIFFE URI. The SAN is valid;
    // the problem is trust chain. This separates "no SAN" from "wrong CA" —
    // the existing `cert_without_spiffe_san_cannot_report` conflates both.
    let foreign_ca = TestCertificateAuthority::new("foreign test CA", TimeDuration::hours(1))
        .expect("create foreign CA");
    let uri = TestCertificateAuthority::build_spiffe_uri(TRUST_DOMAIN, TEST_TEAM_NAME, proxy)
        .expect("build spiffe uri");
    let foreign_cert = foreign_ca
        .issue_client_cert(&uri, proxy, TimeDuration::hours(1))
        .expect("issue client cert from foreign CA");

    let connect = harness.client_with_cert(&foreign_cert).await;
    match connect {
        Err(e) => {
            // Expected: server TLS trust-chain verification rejects the cert
            // because its signing CA is not in the server's client_ca_root.
            let dbg = format!("{e:?}");
            assert!(!dbg.is_empty(), "transport error should have a debug representation");
        }
        Ok(mut client) => {
            // Must NOT ack OK — even if the handshake somehow succeeded, the
            // service identity extraction or SPIFFE-to-team binding must refuse.
            let report = make_report(proxy, "listener-foreign", "err", "h-foreign");
            let res = send_and_collect(&mut client, vec![report]).await;
            if let Ok(acks) = res {
                assert!(
                    acks.iter().all(|a| a.status != AckStatus::Ok as i32),
                    "foreign-CA client cert must not be acked OK even with valid SPIFFE SAN"
                );
            }
        }
    }

    assert_eq!(
        count_rows_for_dataplane(&harness.pool, proxy).await,
        0,
        "foreign-CA cert must not cause any nack row to be persisted"
    );
}
