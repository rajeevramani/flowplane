//! Spec-driven binary regressions for fpv2-hhg.1.
//!
//! The harness treats `flowplane-agent` as a black box. Both fake services own port-0
//! listeners, while the child health address uses bounded bind(0)/release/retry setup.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::{extract::State, routing::get, Router};
use fp_xds::diagnostics::{
    diagnostics_report, AckStatus, DiagnosticsAck, DiagnosticsReport, EnvoyDiagnosticsService,
    EnvoyDiagnosticsServiceServer, ResponseStream,
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request, Response, Status, Streaming};

const POLL: Duration = Duration::from_millis(50);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
const ATTEMPT_DEADLINE_WAIT: Duration = Duration::from_secs(14);

#[derive(Debug)]
struct ReportSeen {
    stream: usize,
    report_id: String,
    requests_delta: Option<i64>,
}

impl ReportSeen {
    fn from_report(stream: usize, report: DiagnosticsReport) -> Self {
        let requests_delta = match report.payload {
            Some(diagnostics_report::Payload::Heartbeat(heartbeat)) => {
                Some(heartbeat.requests_delta)
            }
            _ => None,
        };
        Self {
            stream,
            report_id: report.report_id,
            requests_delta,
        }
    }
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn pid(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }

    fn assert_running(&mut self, context: &str) {
        let child = self.0.as_mut().unwrap();
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                pipe.read_to_string(&mut stderr).unwrap();
            }
            panic!("agent exited {status} {context}; stderr:\n{stderr}");
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct AdminFake {
    base_url: String,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for AdminFake {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn start_admin_fake() -> AdminFake {
    async fn stats() -> ([(&'static str, &'static str); 1], &'static str) {
        (
            [("content-type", "application/json")],
            include_str!("../src/testdata/envoy_1_37_stats.json"),
        )
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Envoy admin to port 0");
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(get(stats)))
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await
            .unwrap();
    });
    AdminFake {
        base_url: format!("http://{addr}"),
        stop: Some(stop_tx),
    }
}

struct MutableAdminState {
    stats: RwLock<String>,
    scrape_count: AtomicUsize,
}

struct MutableAdminFake {
    base_url: String,
    state: Arc<MutableAdminState>,
    stop: Option<oneshot::Sender<()>>,
}

impl MutableAdminFake {
    async fn set_counter(&self, name: &str, value: u64) {
        let current = self.state.stats.read().await.clone();
        let mut document: serde_json::Value = serde_json::from_str(&current).unwrap();
        let entries = document["stats"].as_array_mut().unwrap();
        let stat = entries
            .iter_mut()
            .find(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("fixture lacks mutable counter {name}"));
        stat["value"] = serde_json::Value::from(value);
        *self.state.stats.write().await = serde_json::to_string(&document).unwrap();
    }

    async fn wait_for_scrapes(&self, wanted: usize) {
        let deadline = tokio::time::Instant::now() + STATE_TIMEOUT;
        while self.state.scrape_count.load(Ordering::SeqCst) < wanted {
            assert!(
                tokio::time::Instant::now() < deadline,
                "Envoy fake never observed scrape {wanted}"
            );
            tokio::time::sleep(POLL).await;
        }
    }

    fn scrape_count(&self) -> usize {
        self.state.scrape_count.load(Ordering::SeqCst)
    }
}

impl Drop for MutableAdminFake {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn mutable_stats(
    State(state): State<Arc<MutableAdminState>>,
) -> ([(&'static str, &'static str); 1], String) {
    state.scrape_count.fetch_add(1, Ordering::SeqCst);
    (
        [("content-type", "application/json")],
        state.stats.read().await.clone(),
    )
}

async fn start_mutable_admin_fake() -> MutableAdminFake {
    let state = Arc::new(MutableAdminState {
        stats: RwLock::new(include_str!("../src/testdata/envoy_1_37_stats.json").to_string()),
        scrape_count: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mutable fake Envoy admin to port 0");
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = oneshot::channel();
    let app = Router::new()
        .fallback(get(mutable_stats))
        .with_state(Arc::clone(&state));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await
            .unwrap();
    });
    MutableAdminFake {
        base_url: format!("http://{addr}"),
        state,
        stop: Some(stop_tx),
    }
}

#[derive(Clone)]
struct SwitchableDiagnostics {
    online: watch::Receiver<bool>,
    seen: mpsc::UnboundedSender<ReportSeen>,
    stream_seq: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

impl SwitchableDiagnostics {
    fn mark_active(&self) -> ActiveStream {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        ActiveStream(Arc::clone(&self.active))
    }
}

struct ActiveStream(Arc<AtomicUsize>);

impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for SwitchableDiagnostics {
    type ReportDiagnosticsStream = ResponseStream;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        let mut online = self.online.clone();
        if !*online.borrow() {
            return Err(Status::unavailable("fake control plane offline"));
        }
        let stream = self.stream_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let active = self.mark_active();
        let seen = self.seen.clone();
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            let _active = active;
            loop {
                tokio::select! {
                    changed = online.changed() => {
                        if changed.is_err() || !*online.borrow() {
                            break;
                        }
                    }
                    next = inbound.message() => {
                        let Ok(Some(report)) = next else { break };
                        let id = report.report_id.clone();
                        let _ = seen.send(ReportSeen::from_report(stream, report));
                        if tx.send(Ok(DiagnosticsAck {
                            report_ids: vec![id],
                            status: AckStatus::Ok as i32,
                            message: "accepted by fake CP".into(),
                        })).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[derive(Clone)]
struct DelayedFirstAckDiagnostics {
    seen: mpsc::UnboundedSender<ReportSeen>,
    stream_seq: Arc<AtomicUsize>,
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for DelayedFirstAckDiagnostics {
    type ReportDiagnosticsStream = ResponseStream;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        let stream = self.stream_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let seen = self.seen.clone();
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let Ok(Some(report)) = inbound.message().await else {
                return;
            };
            let id = report.report_id.clone();
            let _ = seen.send(ReportSeen::from_report(stream, report));
            if stream == 1 {
                // The approved contract gives a 1-second poll a 10-second attempt deadline.
                // This deliberately arrives too late and must not be consumed by a later attempt.
                tokio::time::sleep(Duration::from_secs(11)).await;
            }
            let _ = tx
                .send(Ok(DiagnosticsAck {
                    report_ids: vec![id],
                    status: AckStatus::Ok as i32,
                    message: "accepted by fake CP".into(),
                }))
                .await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[derive(Clone)]
struct GateAckDiagnostics {
    acknowledgments_enabled: watch::Receiver<bool>,
    seen: mpsc::UnboundedSender<ReportSeen>,
    stream_seq: Arc<AtomicUsize>,
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for GateAckDiagnostics {
    type ReportDiagnosticsStream = ResponseStream;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        let stream = self.stream_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let seen = self.seen.clone();
        let mut acknowledgments_enabled = self.acknowledgments_enabled.clone();
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            loop {
                let Ok(Some(report)) = inbound.message().await else {
                    break;
                };
                let id = report.report_id.clone();
                let _ = seen.send(ReportSeen::from_report(stream, report));
                while !*acknowledgments_enabled.borrow() {
                    if acknowledgments_enabled.changed().await.is_err() {
                        return;
                    }
                }
                if tx
                    .send(Ok(DiagnosticsAck {
                        report_ids: vec![id],
                        status: AckStatus::Ok as i32,
                        message: "released by fake CP".into(),
                    }))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[derive(Clone)]
struct RejectingDiagnostics {
    streams: Arc<AtomicUsize>,
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for RejectingDiagnostics {
    type ReportDiagnosticsStream = ResponseStream;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        self.streams.fetch_add(1, Ordering::SeqCst);
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let Ok(Some(report)) = inbound.message().await else {
                return;
            };
            let _ = tx
                .send(Ok(DiagnosticsAck {
                    report_ids: vec![report.report_id],
                    status: AckStatus::Unauthorized as i32,
                    message: "fixture rejects dataplane identity".into(),
                }))
                .await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

struct TestCa {
    cert: rcgen::Certificate,
    key: KeyPair,
}

impl TestCa {
    fn mint(common_name: &str) -> Self {
        let key = KeyPair::generate().expect("CA key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let cert = params.self_signed(&key).expect("self-signed CA");
        Self { cert, key }
    }

    fn mint_server(&self) -> PemIdentity {
        self.mint_leaf(
            "diagnostics.test",
            vec![ExtendedKeyUsagePurpose::ServerAuth],
            vec![
                SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
                SanType::DnsName("localhost".try_into().expect("DNS SAN")),
            ],
        )
    }

    fn mint_client(&self, common_name: &str) -> PemIdentity {
        self.mint_leaf(
            common_name,
            vec![ExtendedKeyUsagePurpose::ClientAuth],
            Vec::new(),
        )
    }

    fn mint_leaf(
        &self,
        common_name: &str,
        extended_key_usages: Vec<ExtendedKeyUsagePurpose>,
        subject_alt_names: Vec<SanType>,
    ) -> PemIdentity {
        let key = KeyPair::generate().expect("leaf key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("leaf params");
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = extended_key_usages;
        params.subject_alt_names = subject_alt_names;
        let cert = params
            .signed_by(&key, &self.cert, &self.key)
            .expect("CA-signed leaf");
        PemIdentity {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            cert_der: cert.der().to_vec(),
        }
    }
}

struct PemIdentity {
    cert_pem: String,
    key_pem: String,
    cert_der: Vec<u8>,
}

struct TestCerts {
    trusted_ca_pem: String,
    server: PemIdentity,
    client_a: PemIdentity,
    client_b: PemIdentity,
    untrusted_client: PemIdentity,
}

impl TestCerts {
    fn generate() -> Self {
        let trusted = TestCa::mint("fp-agent trusted rotation CA");
        let untrusted = TestCa::mint("fp-agent untrusted rotation CA");
        Self {
            trusted_ca_pem: trusted.cert.pem(),
            server: trusted.mint_server(),
            client_a: trusted.mint_client("fp-agent-client-a"),
            client_b: trusted.mint_client("fp-agent-client-b"),
            untrusted_client: untrusted.mint_client("fp-agent-untrusted-client"),
        }
    }
}

struct TempCertDir {
    dir: PathBuf,
}

impl TempCertDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "fp-agent-rotation-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("create unique certificate directory");
        Self { dir }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, contents).expect("write certificate fixture");
        path
    }

    fn replace(&self, path: &Path, contents: &str) {
        let replacement = path.with_extension("replacement");
        std::fs::write(&replacement, contents).expect("write replacement PEM");
        std::fs::rename(replacement, path).expect("atomically replace PEM file");
    }
}

impl Drop for TempCertDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Debug)]
struct TlsConnectionSeen {
    stream: usize,
    peer_cert_der: Vec<u8>,
}

#[derive(Clone)]
struct RotatingTlsDiagnostics {
    disconnect_epoch: watch::Receiver<u64>,
    connections: mpsc::UnboundedSender<TlsConnectionSeen>,
    reports: mpsc::UnboundedSender<ReportSeen>,
    stream_seq: Arc<AtomicUsize>,
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for RotatingTlsDiagnostics {
    type ReportDiagnosticsStream = ResponseStream;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        let stream = self.stream_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let peer_cert_der = request
            .peer_certs()
            .and_then(|chain| chain.first().map(|cert| cert.as_ref().to_vec()))
            .ok_or_else(|| Status::unauthenticated("mTLS client certificate missing"))?;
        let _ = self.connections.send(TlsConnectionSeen {
            stream,
            peer_cert_der,
        });

        let mut disconnect_epoch = self.disconnect_epoch.clone();
        // A newly created stream starts at the current epoch; only a later epoch change
        // represents a disconnect request for this connection.
        disconnect_epoch.borrow_and_update();
        let reports = self.reports.clone();
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = disconnect_epoch.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        break;
                    }
                    next = inbound.message() => {
                        let Ok(Some(report)) = next else { break };
                        let id = report.report_id.clone();
                        let _ = reports.send(ReportSeen::from_report(stream, report));
                        if tx.send(Ok(DiagnosticsAck {
                            report_ids: vec![id],
                            status: AckStatus::Ok as i32,
                            message: "accepted over verified mTLS".into(),
                        })).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

struct AbruptProxy {
    endpoint: String,
    connections: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    stop: Option<oneshot::Sender<()>>,
}

impl AbruptProxy {
    fn abort_connections(&self) {
        let handles = {
            let mut connections = self.connections.lock().unwrap();
            std::mem::take(&mut *connections)
        };
        for handle in handles {
            handle.abort();
        }
    }
}

impl Drop for AbruptProxy {
    fn drop(&mut self) {
        self.abort_connections();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn start_abrupt_proxy(backend: std::net::SocketAddr) -> AbruptProxy {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind abrupt transport proxy to port 0");
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(std::sync::Mutex::new(Vec::new()));
    let task_connections = Arc::clone(&connections);
    let (stop_tx, mut stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                accepted = listener.accept() => {
                    let Ok((mut downstream, _)) = accepted else { break };
                    let handle = tokio::spawn(async move {
                        let Ok(mut upstream) = tokio::net::TcpStream::connect(backend).await else {
                            return;
                        };
                        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
                    });
                    task_connections.lock().unwrap().push(handle);
                }
            }
        }
    });
    AbruptProxy {
        endpoint: format!("http://{addr}"),
        connections,
        stop: Some(stop_tx),
    }
}

struct DiagnosticsFake {
    endpoint: String,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for DiagnosticsFake {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn start_diagnostics_fake<T>(service: T) -> DiagnosticsFake
where
    T: EnvoyDiagnosticsService + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake diagnostics CP to port 0");
    start_diagnostics_fake_with_listener(listener, service).await
}

async fn start_diagnostics_fake_with_listener<T>(
    listener: tokio::net::TcpListener,
    service: T,
) -> DiagnosticsFake
where
    T: EnvoyDiagnosticsService + Send + Sync + 'static,
{
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EnvoyDiagnosticsServiceServer::new(service))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                let _ = stop_rx.await;
            })
            .await
            .unwrap();
    });
    DiagnosticsFake {
        endpoint: format!("http://{addr}"),
        stop: Some(stop_tx),
    }
}

async fn start_mtls_diagnostics_fake<T>(
    service: T,
    trusted_ca_pem: &str,
    server: &PemIdentity,
) -> DiagnosticsFake
where
    T: EnvoyDiagnosticsService + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mTLS diagnostics CP to port 0");
    let addr = listener.local_addr().unwrap();
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(&server.cert_pem, &server.key_pem))
        .client_ca_root(Certificate::from_pem(trusted_ca_pem));
    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .expect("configure real mTLS diagnostics listener")
            .add_service(EnvoyDiagnosticsServiceServer::new(service))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                let _ = stop_rx.await;
            })
            .await
            .unwrap();
    });
    DiagnosticsFake {
        endpoint: format!("https://127.0.0.1:{}", addr.port()),
        stop: Some(stop_tx),
    }
}

async fn health_status(client: &reqwest::Client, health_url: &str) -> Option<reqwest::StatusCode> {
    client
        .get(format!("{health_url}/healthz"))
        .send()
        .await
        .ok()
        .map(|response| response.status())
}

async fn wait_for_status(
    child: &mut ChildGuard,
    client: &reqwest::Client,
    health_url: &str,
    wanted: reqwest::StatusCode,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        child.assert_running(&format!("while waiting for health {wanted}"));
        if health_status(client, health_url).await == Some(wanted) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "health never became {wanted} at {health_url}"
        );
        tokio::time::sleep(POLL).await;
    }
}

async fn spawn_agent(admin_url: &str, cp_endpoint: &str) -> (ChildGuard, String, reqwest::Client) {
    spawn_agent_config(admin_url, cp_endpoint, 4, &[]).await
}

async fn spawn_agent_config(
    admin_url: &str,
    cp_endpoint: &str,
    queue_cap: usize,
    extra_args: &[String],
) -> (ChildGuard, String, reqwest::Client) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();

    for attempt in 1..=5 {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let health_addr = reservation.local_addr().unwrap();
        drop(reservation);

        let health_addr_string = health_addr.to_string();
        let queue_cap_string = queue_cap.to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_flowplane-agent"));
        command.args([
            "--envoy-admin-url",
            admin_url,
            "--cp-endpoint",
            cp_endpoint,
            "--dataplane-id",
            "018f0000-0000-7000-8000-000000000001",
            "--poll-interval-secs",
            "1",
            "--queue-cap",
            &queue_cap_string,
            "--health-bind-addr",
            &health_addr_string,
        ]);
        command.args(extra_args);
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(Some(child));
        let health_url = format!("http://{health_addr}");
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if health_status(&client, &health_url).await.is_some() {
                return (child, health_url, client);
            }
            if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.0.as_mut().unwrap().stderr.take() {
                    pipe.read_to_string(&mut stderr).unwrap();
                }
                if stderr.to_ascii_lowercase().contains("address") && attempt < 5 {
                    break;
                }
                panic!("agent exited {status} during boot attempt {attempt}; stderr:\n{stderr}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "agent health listener did not bind at {health_url}"
            );
            tokio::time::sleep(POLL).await;
        }
    }
    unreachable!("bounded health-port retry loop either returns or panics")
}

async fn next_report(
    reports: &mut mpsc::UnboundedReceiver<ReportSeen>,
    timeout: Duration,
) -> ReportSeen {
    tokio::time::timeout(timeout, reports.recv())
        .await
        .expect("timed out waiting for a diagnostics report")
        .expect("fake diagnostics event channel closed")
}

async fn next_tls_connection(
    connections: &mut mpsc::UnboundedReceiver<TlsConnectionSeen>,
    timeout: Duration,
) -> TlsConnectionSeen {
    tokio::time::timeout(timeout, connections.recv())
        .await
        .expect("timed out waiting for an authenticated mTLS stream")
        .expect("fake mTLS connection event channel closed")
}

/// AC1: daemon mode starts its health endpoint and remains alive while the CP endpoint refuses
/// connections, then connects automatically when the CP appears at that same endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_recovers_when_control_plane_is_unavailable_at_startup() {
    let admin = start_admin_fake().await;
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let cp_addr = reservation.local_addr().unwrap();
    drop(reservation);
    let endpoint = format!("http://{cp_addr}");
    let (mut child, health_url, http) = spawn_agent(&admin.base_url, &endpoint).await;
    let original_pid = child.pid();

    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        STATE_TIMEOUT,
    )
    .await;
    child.assert_running("while the control plane was unavailable at startup");

    let listener = tokio::net::TcpListener::bind(cp_addr)
        .await
        .expect("bind recovered CP at the original endpoint");
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let (_online_tx, online_rx) = watch::channel(true);
    let diagnostics = start_diagnostics_fake_with_listener(
        listener,
        SwitchableDiagnostics {
            online: online_rx,
            seen: seen_tx,
            stream_seq: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
        },
    )
    .await;
    let _report = next_report(&mut seen_rx, Duration::from_secs(12)).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;
    assert_eq!(
        child.pid(),
        original_pid,
        "startup recovery replaced the agent"
    );
    drop(diagnostics);
}

/// AC3: `--once` intentionally preserves the old fail-fast contract and must not enter the
/// daemon reconnect loop when its one diagnostics attempt cannot connect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn once_mode_remains_fail_fast_when_control_plane_is_unavailable() {
    let admin = start_admin_fake().await;
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let cp_addr = reservation.local_addr().unwrap();
    drop(reservation);

    let mut child = Command::new(env!("CARGO_BIN_EXE_flowplane-agent"))
        .args([
            "--envoy-admin-url",
            &admin.base_url,
            "--cp-endpoint",
            &format!("http://{cp_addr}"),
            "--dataplane-id",
            "018f0000-0000-7000-8000-000000000001",
            "--once",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "--once entered a reconnect loop instead of failing fast"
        );
        tokio::time::sleep(POLL).await;
    };
    assert!(
        !status.success(),
        "--once unexpectedly succeeded without a CP"
    );
}

/// AC10: an explicit application-level authorization rejection is permanent and must terminate
/// daemon mode promptly rather than being hidden behind transport retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_ack_is_fatal_and_is_not_retried() {
    let admin = start_admin_fake().await;
    let streams = Arc::new(AtomicUsize::new(0));
    let diagnostics = start_diagnostics_fake(RejectingDiagnostics {
        streams: Arc::clone(&streams),
    })
    .await;
    let health_reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let health_addr = health_reservation.local_addr().unwrap();
    drop(health_reservation);
    let health_addr_string = health_addr.to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flowplane-agent"))
        .args([
            "--envoy-admin-url",
            &admin.base_url,
            "--cp-endpoint",
            &diagnostics.endpoint,
            "--dataplane-id",
            "018f0000-0000-7000-8000-000000000001",
            "--poll-interval-secs",
            "1",
            "--health-bind-addr",
            &health_addr_string,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + STATE_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "unauthorized acknowledgment was retried instead of terminating the daemon"
        );
        tokio::time::sleep(POLL).await;
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        !status.success(),
        "unauthorized acknowledgment unexpectedly succeeded"
    );
    assert!(
        stderr.contains("fixture rejects dataplane identity"),
        "fatal error did not preserve rejection context: {stderr}"
    );
    assert_eq!(
        streams.load(Ordering::SeqCst),
        1,
        "permanent acknowledgment rejection must not reconnect"
    );
}

/// AC2/3/5/7/9: an acknowledged daemon survives graceful stream loss, becomes unready,
/// reconnects to the same endpoint, and returns healthy without replacing the process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_agent_recovers_health_after_control_plane_stream_loss() {
    let admin = start_admin_fake().await;
    let (online_tx, online_rx) = watch::channel(true);
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let diagnostics = start_diagnostics_fake(SwitchableDiagnostics {
        online: online_rx,
        seen: seen_tx,
        stream_seq: Arc::new(AtomicUsize::new(0)),
        active: Arc::clone(&active),
        max_active: Arc::clone(&max_active),
    })
    .await;
    let (mut child, health_url, http) = spawn_agent(&admin.base_url, &diagnostics.endpoint).await;
    let original_pid = child.pid();

    let acknowledged = next_report(&mut seen_rx, STATE_TIMEOUT).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;

    online_tx.send(false).unwrap();
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        STATE_TIMEOUT,
    )
    .await;
    child.assert_running("after the diagnostics stream was dropped");
    assert_eq!(
        child.pid(),
        original_pid,
        "outage must not replace the agent"
    );

    online_tx.send(true).unwrap();
    let replay_or_next = next_report(&mut seen_rx, STATE_TIMEOUT).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;

    assert_eq!(
        child.pid(),
        original_pid,
        "recovery must preserve child PID"
    );
    assert_ne!(
        replay_or_next.report_id, acknowledged.report_id,
        "a report acknowledged before disconnect must not be replayed"
    );
    assert!(
        replay_or_next.stream > acknowledged.stream,
        "recovery must use a fresh diagnostics stream"
    );
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "reconnect must never overlap active streams"
    );
}

/// AC2/3/9: killing the gRPC server task drops the active HTTP/2 transport without graceful
/// service shutdown. Two consecutive failures must preserve PID, recover automatically, and
/// never overlap server-observed diagnostics streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_abrupt_transport_loss_recovers_without_overlapping_streams() {
    let admin = start_admin_fake().await;
    let (_online_tx, online_rx) = watch::channel(true);
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let stream_seq = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let service = || SwitchableDiagnostics {
        online: online_rx.clone(),
        seen: seen_tx.clone(),
        stream_seq: Arc::clone(&stream_seq),
        active: Arc::clone(&active),
        max_active: Arc::clone(&max_active),
    };
    let backend = start_diagnostics_fake(service()).await;
    let backend_addr: std::net::SocketAddr = backend
        .endpoint
        .strip_prefix("http://")
        .unwrap()
        .parse()
        .unwrap();
    let proxy = start_abrupt_proxy(backend_addr).await;
    let (mut child, health_url, http) = spawn_agent(&admin.base_url, &proxy.endpoint).await;
    let original_pid = child.pid();
    let mut previous = next_report(&mut seen_rx, STATE_TIMEOUT).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;

    for cycle in 1..=2 {
        proxy.abort_connections();
        wait_for_status(
            &mut child,
            &http,
            &health_url,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            STATE_TIMEOUT,
        )
        .await;
        child.assert_running(&format!("after abrupt transport loss cycle {cycle}"));
        assert_eq!(
            child.pid(),
            original_pid,
            "cycle {cycle} replaced the agent"
        );

        let recovered = next_report(&mut seen_rx, Duration::from_secs(12)).await;
        wait_for_status(
            &mut child,
            &http,
            &health_url,
            reqwest::StatusCode::OK,
            STATE_TIMEOUT,
        )
        .await;
        assert!(
            recovered.stream > previous.stream,
            "cycle {cycle} did not create a fresh diagnostics stream"
        );
        assert_ne!(
            recovered.report_id, previous.report_id,
            "cycle {cycle} replayed an already acknowledged report"
        );
        previous = recovered;
    }
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "repeated abrupt recovery overlapped diagnostics streams"
    );

    drop(child);
    proxy.abort_connections();
    drop(proxy);
    let cleanup_deadline = tokio::time::Instant::now() + STATE_TIMEOUT;
    while active.load(Ordering::SeqCst) != 0 {
        assert!(
            tokio::time::Instant::now() < cleanup_deadline,
            "abrupt fixture left an active diagnostics stream behind"
        );
        tokio::time::sleep(POLL).await;
    }
    drop(backend);
    tokio::time::sleep(POLL).await;
}

/// AC4/7: an acknowledgment arriving after the 10-second attempt deadline is ignored; the
/// retained logical report is replayed on a new stream with its exact report ID, and only the
/// matching timely acknowledgment restores readiness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_ack_past_deadline_replays_exact_report_id_and_recovers_health() {
    let admin = start_admin_fake().await;
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let diagnostics = start_diagnostics_fake(DelayedFirstAckDiagnostics {
        seen: seen_tx,
        stream_seq: Arc::new(AtomicUsize::new(0)),
    })
    .await;
    let (mut child, health_url, http) = spawn_agent(&admin.base_url, &diagnostics.endpoint).await;
    let original_pid = child.pid();

    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        STATE_TIMEOUT,
    )
    .await;
    let first = next_report(&mut seen_rx, STATE_TIMEOUT).await;
    let replay = next_report(&mut seen_rx, ATTEMPT_DEADLINE_WAIT).await;

    assert!(
        replay.stream > first.stream,
        "deadline must tear down the old stream"
    );
    assert_eq!(
        replay.report_id, first.report_id,
        "the one retained report must preserve its exact id across reconnect"
    );
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;
    assert_eq!(
        child.pid(),
        original_pid,
        "deadline recovery must not replace the agent"
    );
}

/// AC5/6: with acknowledgments blocked, the bounded source queue fills and backpressures
/// polling. Reports already sampled drain in observation order once acknowledgments resume,
/// accepted IDs never reappear, and counters advanced during the blocked poll are carried by a
/// later non-negative heartbeat delta rather than being lost or made negative.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_reports_drain_in_order_and_preserve_counters_across_backpressure() {
    let admin = start_mutable_admin_fake().await;
    let (ack_tx, ack_rx) = watch::channel(false);
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let diagnostics = start_diagnostics_fake(GateAckDiagnostics {
        acknowledgments_enabled: ack_rx,
        seen: seen_tx,
        stream_seq: Arc::new(AtomicUsize::new(0)),
    })
    .await;
    let (mut child, health_url, http) =
        spawn_agent_config(&admin.base_url, &diagnostics.endpoint, 2, &[]).await;

    let first = next_report(&mut seen_rx, STATE_TIMEOUT).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        STATE_TIMEOUT,
    )
    .await;

    // One retained report is blocked on its ack; two more fill queue_cap; the fourth
    // scrape then blocks on enqueue. Advance Envoy's cumulative request counter only after
    // that state is established, and prove the poller cannot scrape again while blocked.
    admin.wait_for_scrapes(4).await;
    let blocked_scrape_count = admin.scrape_count();
    admin
        .set_counter("http.ingress_http.downstream_rq_total", 17)
        .await;
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert_eq!(
        admin.scrape_count(),
        blocked_scrape_count,
        "the source poller must remain backpressured while its fourth report cannot enqueue"
    );
    ack_tx.send(true).unwrap();

    let mut reports = vec![first];
    while reports.len() < 5 {
        reports.push(next_report(&mut seen_rx, Duration::from_secs(4)).await);
    }
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;

    let source_ids: Vec<uuid::Uuid> = reports
        .iter()
        .map(|report| uuid::Uuid::parse_str(&report.report_id).expect("report ID must be UUIDv7"))
        .collect();
    assert!(
        source_ids.windows(2).all(|pair| pair[0] < pair[1]),
        "queued reports must drain in strict UUIDv7 source order: {source_ids:?}"
    );

    let ids: HashSet<&str> = reports
        .iter()
        .map(|report| report.report_id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        reports.len(),
        "an ID acknowledged during queue drain must never be replayed"
    );
    let deltas: Vec<i64> = reports
        .iter()
        .map(|report| report.requests_delta.expect("heartbeat report"))
        .collect();
    assert!(
        deltas.iter().all(|delta| *delta >= 0),
        "cumulative-counter accounting must never emit a negative delta: {deltas:?}"
    );
    assert!(
        deltas.iter().skip(4).any(|delta| *delta >= 7),
        "the seven requests accumulated while source enqueue was blocked were lost: {deltas:?}"
    );
}

/// AC8: the diagnostics endpoint performs a real mTLS handshake, validates the client chain,
/// and observes file-backed client identity changes only on reconstructed connections. A valid
/// replacement reconnects in the same process; a replacement signed by an untrusted CA never
/// reaches the RPC service and readiness remains degraded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mtls_client_certificate_files_reload_on_reconnect_and_fail_closed() {
    let admin = start_admin_fake().await;
    let certs = TestCerts::generate();
    let cert_dir = TempCertDir::new();
    let ca_path = cert_dir.write("server-ca.pem", &certs.trusted_ca_pem);
    let cert_path = cert_dir.write("client.pem", &certs.client_a.cert_pem);
    let key_path = cert_dir.write("client-key.pem", &certs.client_a.key_pem);

    let (disconnect_tx, disconnect_rx) = watch::channel(0_u64);
    let (connection_tx, mut connection_rx) = mpsc::unbounded_channel();
    let (report_tx, mut report_rx) = mpsc::unbounded_channel();
    let diagnostics = start_mtls_diagnostics_fake(
        RotatingTlsDiagnostics {
            disconnect_epoch: disconnect_rx,
            connections: connection_tx,
            reports: report_tx,
            stream_seq: Arc::new(AtomicUsize::new(0)),
        },
        &certs.trusted_ca_pem,
        &certs.server,
    )
    .await;
    let tls_args = vec![
        "--tls-cert-path".to_string(),
        cert_path.display().to_string(),
        "--tls-key-path".to_string(),
        key_path.display().to_string(),
        "--tls-ca-path".to_string(),
        ca_path.display().to_string(),
        "--tls-server-name".to_string(),
        "localhost".to_string(),
    ];
    let (mut child, health_url, http) =
        spawn_agent_config(&admin.base_url, &diagnostics.endpoint, 2, &tls_args).await;
    let original_pid = child.pid();

    let initial = next_tls_connection(&mut connection_rx, STATE_TIMEOUT).await;
    assert_eq!(
        initial.peer_cert_der, certs.client_a.cert_der,
        "the first TLS connection must present client A"
    );
    let _ = next_report(&mut report_rx, STATE_TIMEOUT).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;

    cert_dir.replace(&cert_path, &certs.client_b.cert_pem);
    cert_dir.replace(&key_path, &certs.client_b.key_pem);
    disconnect_tx.send(1).unwrap();
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        STATE_TIMEOUT,
    )
    .await;
    let rotated = next_tls_connection(&mut connection_rx, Duration::from_secs(12)).await;
    assert!(
        rotated.stream > initial.stream,
        "rotation requires a new stream"
    );
    assert_eq!(
        rotated.peer_cert_der, certs.client_b.cert_der,
        "the reconstructed TLS channel must reread and present client B"
    );
    let _ = next_report(&mut report_rx, STATE_TIMEOUT).await;
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::OK,
        STATE_TIMEOUT,
    )
    .await;
    assert_eq!(
        child.pid(),
        original_pid,
        "valid rotation replaced the child"
    );

    cert_dir.replace(&cert_path, &certs.untrusted_client.cert_pem);
    cert_dir.replace(&key_path, &certs.untrusted_client.key_pem);
    disconnect_tx.send(2).unwrap();
    wait_for_status(
        &mut child,
        &http,
        &health_url,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        STATE_TIMEOUT,
    )
    .await;
    assert!(
        tokio::time::timeout(Duration::from_secs(8), connection_rx.recv())
            .await
            .is_err(),
        "an untrusted replacement unexpectedly completed mTLS and reached the RPC service"
    );
    assert_eq!(
        health_status(&http, &health_url).await,
        Some(reqwest::StatusCode::SERVICE_UNAVAILABLE),
        "invalid replacement must remain unhealthy"
    );
    child.assert_running("after repeated rejection of the untrusted replacement certificate");
    assert_eq!(
        child.pid(),
        original_pid,
        "certificate rejection must not replace or terminate the daemon"
    );
}
