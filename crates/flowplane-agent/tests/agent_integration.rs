//! Black-box adversarial integration tests for the `flowplane-agent` binary.
//!
//! These tests run the compiled `flowplane-agent` binary as a subprocess and
//! exercise it against:
//!   1. A fake Envoy admin HTTP server (axum) returning crafted `/config_dump`
//!      payloads.
//!   2. A fake Flowplane CP gRPC server (tonic) implementing
//!      `EnvoyDiagnosticsService` that records every received `DiagnosticsReport`.
//!
//! The tests were written WITHOUT reading the agent's source under
//! `crates/flowplane-agent/src/` (except `config.rs`'s public `AgentConfig`
//! struct and `Cargo.toml` / `build.rs`), and WITHOUT reading the proto-compiled
//! client stubs. The contract is:
//!   - Envoy admin proto `envoy.admin.v3.ConfigDump` (JSON on the wire)
//!   - `proto/flowplane/diagnostics/v1/diagnostics.proto`
//!
//! Tests target the adversarial bug-hunt checklist in bead `fp-hsk.3`.

#![allow(clippy::needless_return)]

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Instant};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

// Generated proto code — include directly from OUT_DIR so this test file does
// not depend on anything the agent crate exposes as a library (it does not have
// a lib target at all).
mod pb {
    #![allow(clippy::all, unused_qualifications, dead_code)]
    include!(concat!(env!("OUT_DIR"), "/flowplane.diagnostics.v1.rs"));
}

use pb::envoy_diagnostics_service_server::{
    EnvoyDiagnosticsService, EnvoyDiagnosticsServiceServer,
};
use pb::{Ack, AckStatus, DiagnosticsReport, ListenerStateReport, ResourceType};

// -----------------------------------------------------------------------------
// Port allocation
// -----------------------------------------------------------------------------

fn alloc_port() -> u16 {
    let l = StdTcpListener::bind("127.0.0.1:0").expect("bind 0");
    l.local_addr().unwrap().port()
}

// -----------------------------------------------------------------------------
// Fake Envoy admin server (axum)
// -----------------------------------------------------------------------------

#[derive(Clone, Default)]
struct AdminState {
    inner: Arc<Mutex<AdminInner>>,
}

#[derive(Default)]
struct AdminInner {
    /// Queue of responses to serve in order; last entry is reused forever if
    /// queue is exhausted.
    responses: Vec<AdminResponse>,
    /// Number of /config_dump requests observed.
    hits: usize,
}

#[derive(Clone)]
#[allow(dead_code)]
enum AdminResponse {
    /// Return this JSON body with HTTP 200.
    Json(Value),
    /// Return this raw string body with HTTP 200 (for malformed tests).
    Raw(String),
    /// Return HTTP status code with empty body.
    Status(u16),
    /// Close the connection mid-response (simulated via deliberate delay +
    /// truncated body).
    Truncated(String),
}

impl AdminState {
    fn new(responses: Vec<AdminResponse>) -> Self {
        Self { inner: Arc::new(Mutex::new(AdminInner { responses, hits: 0 })) }
    }

    async fn hits(&self) -> usize {
        self.inner.lock().await.hits
    }
}

async fn admin_handler(State(state): State<AdminState>) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let mut inner = state.inner.lock().await;
    inner.hits += 1;
    let idx = inner.hits.saturating_sub(1).min(inner.responses.len().saturating_sub(1));
    let resp =
        inner.responses.get(idx).cloned().unwrap_or(AdminResponse::Json(json!({"configs": []})));
    drop(inner);

    match resp {
        AdminResponse::Json(v) => {
            (StatusCode::OK, [("content-type", "application/json")], v.to_string()).into_response()
        }
        AdminResponse::Raw(s) => {
            (StatusCode::OK, [("content-type", "application/json")], s).into_response()
        }
        AdminResponse::Status(code) => {
            (StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), "")
                .into_response()
        }
        AdminResponse::Truncated(s) => {
            // Return a Content-Length larger than the actual body so clients
            // observe a truncated response. axum/hyper will compute length
            // from the actual body; emulate truncation by returning a body
            // prefix of a known-good JSON.
            (StatusCode::OK, [("content-type", "application/json")], s).into_response()
        }
    }
}

async fn spawn_admin(state: AdminState) -> u16 {
    let port = alloc_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let app = Router::new().route("/config_dump", get(admin_handler)).with_state(state);
    let listener = TcpListener::bind(addr).await.expect("bind admin");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // Give the server a moment to be ready.
    for _ in 0..20 {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    port
}

// -----------------------------------------------------------------------------
// Fake CP gRPC server
// -----------------------------------------------------------------------------

#[derive(Clone, Default)]
struct CpState {
    reports: Arc<Mutex<Vec<DiagnosticsReport>>>,
    /// If set, reply with this status code for every incoming report.
    override_status: Arc<Mutex<Option<AckStatus>>>,
    /// Count of inbound streams opened.
    streams_opened: Arc<Mutex<usize>>,
    /// Notifies all in-flight stream handlers to terminate their outbound side.
    /// The test uses this to simulate "CP died" — when notified, every active
    /// `report_diagnostics` handler drops its tx, which causes the agent's
    /// `inbound.message()` to observe Ok(None) and trigger reconnect.
    kill_signal: Arc<tokio::sync::Notify>,
}

impl CpState {
    async fn reports(&self) -> Vec<DiagnosticsReport> {
        self.reports.lock().await.clone()
    }
    async fn listener_reports(&self) -> Vec<ListenerStateReport> {
        self.reports()
            .await
            .into_iter()
            .filter_map(|r| r.payload.map(|pb::diagnostics_report::Payload::ListenerState(ls)| ls))
            .collect()
    }
    #[allow(dead_code)]
    async fn streams(&self) -> usize {
        *self.streams_opened.lock().await
    }

    /// Forcibly terminate all in-flight `report_diagnostics` streams by
    /// notifying their handler tasks.
    fn kill_streams(&self) {
        self.kill_signal.notify_waiters();
    }
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for CpState {
    type ReportDiagnosticsStream = ReceiverStream<Result<Ack, Status>>;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        {
            let mut s = self.streams_opened.lock().await;
            *s += 1;
        }
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Ack, Status>>(32);
        let reports = self.reports.clone();
        let override_status = self.override_status.clone();
        let kill_signal = self.kill_signal.clone();
        let mut inbound = request.into_inner();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = kill_signal.notified() => {
                        // Drop tx, ending the outbound side. Agent observes
                        // Ok(None) on its inbound and triggers reconnect.
                        return;
                    }
                    incoming = inbound.message() => {
                        match incoming {
                            Ok(Some(msg)) => {
                                let id = msg.report_id.clone();
                                reports.lock().await.push(msg);
                                let status =
                                    override_status.lock().await.unwrap_or(AckStatus::Ok);
                                let ack = Ack {
                                    report_id: vec![id],
                                    status: status as i32,
                                    message: String::new(),
                                };
                                if tx.send(Ok(ack)).await.is_err() {
                                    return;
                                }
                            }
                            _ => return,
                        }
                    }
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn spawn_cp() -> (u16, CpState) {
    let state = CpState::default();
    let state_clone = state.clone();
    let port = alloc_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(EnvoyDiagnosticsServiceServer::new(state_clone))
            .serve(addr)
            .await;
    });
    for _ in 0..40 {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    (port, state)
}

// Spawn a CP on a CLOSED port (no server) — returns the port only.
fn reserved_port() -> u16 {
    alloc_port()
}

// -----------------------------------------------------------------------------
// Agent subprocess helpers
// -----------------------------------------------------------------------------

struct AgentProc {
    child: Child,
    #[allow(dead_code)]
    stderr_log: Arc<Mutex<String>>,
    #[allow(dead_code)]
    stdout_log: Arc<Mutex<String>>,
}

impl AgentProc {
    async fn stderr(&self) -> String {
        self.stderr_log.lock().await.clone()
    }
}

impl Drop for AgentProc {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn agent_bin() -> &'static str {
    env!("CARGO_BIN_EXE_flowplane-agent")
}

fn spawn_agent(
    admin_port: u16,
    cp_port: u16,
    dataplane_id: &str,
    poll_interval_secs: u64,
    extra_env: &[(&str, &str)],
) -> AgentProc {
    let mut cmd = Command::new(agent_bin());
    cmd.env("FLOWPLANE_AGENT_ENVOY_ADMIN_URL", format!("http://127.0.0.1:{admin_port}"))
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", format!("http://127.0.0.1:{cp_port}"))
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", dataplane_id)
        .env("FLOWPLANE_AGENT_POLL_INTERVAL_SECS", poll_interval_secs.to_string())
        .env("RUST_LOG", "flowplane_agent=debug,info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn agent binary");

    let stderr_log = Arc::new(Mutex::new(String::new()));
    let stdout_log = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let buf = stderr_log.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = r.next_line().await {
                buf.lock().await.push_str(&line);
                buf.lock().await.push('\n');
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        let buf = stdout_log.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = r.next_line().await {
                buf.lock().await.push_str(&line);
                buf.lock().await.push('\n');
            }
        });
    }

    AgentProc { child, stderr_log, stdout_log }
}

/// Wait until the CP has received at least `n` total reports, or `max` elapsed.
async fn wait_reports(cp: &CpState, n: usize, max: Duration) -> Vec<DiagnosticsReport> {
    let deadline = Instant::now() + max;
    loop {
        let r = cp.reports().await;
        if r.len() >= n {
            return r;
        }
        if Instant::now() >= deadline {
            return r;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until admin has been hit at least `n` times.
async fn wait_admin_hits(state: &AdminState, n: usize, max: Duration) -> usize {
    let deadline = Instant::now() + max;
    loop {
        let h = state.hits().await;
        if h >= n {
            return h;
        }
        if Instant::now() >= deadline {
            return h;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

// -----------------------------------------------------------------------------
// Canned config_dump builders
// -----------------------------------------------------------------------------

fn listeners_dump(listeners: Vec<Value>) -> Value {
    json!({
        "configs": [
            {
                "@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump",
                "version_info": "1",
                "dynamic_listeners": listeners,
                "static_listeners": []
            },
            {"@type": "type.googleapis.com/envoy.admin.v3.ClustersConfigDump", "dynamic_active_clusters": []},
            {"@type": "type.googleapis.com/envoy.admin.v3.RoutesConfigDump", "dynamic_route_configs": []}
        ]
    })
}

fn clusters_dump(clusters: Vec<Value>) -> Value {
    json!({
        "configs": [
            {"@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump", "dynamic_listeners": []},
            {
                "@type": "type.googleapis.com/envoy.admin.v3.ClustersConfigDump",
                "dynamic_active_clusters": clusters
            },
            {"@type": "type.googleapis.com/envoy.admin.v3.RoutesConfigDump", "dynamic_route_configs": []}
        ]
    })
}

fn routes_dump(routes: Vec<Value>) -> Value {
    json!({
        "configs": [
            {"@type": "type.googleapis.com/envoy.admin.v3.ListenersConfigDump", "dynamic_listeners": []},
            {"@type": "type.googleapis.com/envoy.admin.v3.ClustersConfigDump", "dynamic_active_clusters": []},
            {
                "@type": "type.googleapis.com/envoy.admin.v3.RoutesConfigDump",
                "dynamic_route_configs": routes
            }
        ]
    })
}

fn listener_with_error(name: &str, details: &str, ts: &str) -> Value {
    json!({
        "name": name,
        "error_state": {
            "details": details,
            "last_update_attempt": ts,
            "failed_configuration": {
                "@type": "type.googleapis.com/envoy.config.listener.v3.Listener",
                "name": name
            }
        }
    })
}

fn listener_healthy(name: &str) -> Value {
    json!({
        "name": name,
        "active_state": {
            "version_info": "1",
            "listener": {
                "@type": "type.googleapis.com/envoy.config.listener.v3.Listener",
                "name": name
            }
        }
    })
}

fn cluster_with_error(name: &str, details: &str, ts: &str) -> Value {
    json!({
        "cluster": {
            "@type": "type.googleapis.com/envoy.admin.v3.ClustersConfigDump.DynamicCluster",
            "version_info": "1",
            "cluster": {
                "@type": "type.googleapis.com/envoy.config.cluster.v3.Cluster",
                "name": name
            }
        },
        "error_state": {
            "details": details,
            "last_update_attempt": ts
        }
    })
}

fn route_with_error(name: &str, details: &str, ts: &str) -> Value {
    json!({
        "route_config": {
            "@type": "type.googleapis.com/envoy.config.route.v3.RouteConfiguration",
            "name": name
        },
        "error_state": {
            "details": details,
            "last_update_attempt": ts
        }
    })
}

// -----------------------------------------------------------------------------
// =============================================================================
// Tests
// =============================================================================
// -----------------------------------------------------------------------------

// --- Envoy admin parsing -----------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn empty_config_dump_sends_no_reports() {
    let admin = AdminState::new(vec![AdminResponse::Json(json!({"configs": []}))]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;

    let _agent = spawn_agent(admin_port, cp_port, "dp-empty", 1, &[]);

    // Give the agent 4 seconds: it should hit admin at least twice and still
    // send zero reports.
    wait_admin_hits(&admin, 2, Duration::from_secs(6)).await;
    sleep(Duration::from_secs(1)).await;

    let reports = cp.reports().await;
    assert!(reports.is_empty(), "empty config dump must not produce reports, got {reports:?}");
    assert!(admin.hits().await >= 2, "agent should have polled at least twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn healthy_listener_sends_no_reports() {
    let dump = listeners_dump(vec![listener_healthy("lst_healthy")]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;

    let _agent = spawn_agent(admin_port, cp_port, "dp-healthy", 1, &[]);
    wait_admin_hits(&admin, 2, Duration::from_secs(6)).await;
    sleep(Duration::from_secs(1)).await;

    let reports = cp.listener_reports().await;
    assert!(reports.is_empty(), "healthy listener should not report; got {reports:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn single_listener_error_reported_verbatim() {
    let err = "Proto constraint validation failed: signout_path: value length must be at least 1 characters";
    let dump = listeners_dump(vec![listener_with_error("lst_oauth2", err, "2026-04-13T00:00:00Z")]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;

    let _agent = spawn_agent(admin_port, cp_port, "dp-oauth2", 1, &[]);

    let got = wait_reports(&cp, 1, Duration::from_secs(10)).await;
    assert!(!got.is_empty(), "expected at least one report within 10s");

    let listener_reports = cp.listener_reports().await;
    assert_eq!(
        listener_reports.len(),
        1,
        "expected exactly one listener report, got {}",
        listener_reports.len()
    );
    let r = &listener_reports[0];
    assert_eq!(r.resource_name, "lst_oauth2");
    assert_eq!(r.error_details, err, "error_details must be preserved byte-for-byte");
    assert_eq!(r.resource_type, ResourceType::Listener as i32, "resource_type should be LISTENER");

    // Envelope assertions
    let env = &got[0];
    assert_eq!(env.dataplane_id, "dp-oauth2", "envelope must carry the configured dataplane_id");
    assert!(!env.report_id.is_empty(), "envelope must assign a report_id");
    assert_eq!(env.schema_version, 1, "MVP schema_version must be 1");
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_listeners_all_reported() {
    let dump = listeners_dump(vec![
        listener_with_error("lst_a", "error A: boom", "2026-04-13T00:00:00Z"),
        listener_with_error("lst_b", "error B: kaboom", "2026-04-13T00:00:01Z"),
        listener_healthy("lst_ok"),
        listener_with_error("lst_c", "error C: splat", "2026-04-13T00:00:02Z"),
    ]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-multi", 1, &[]);

    wait_reports(&cp, 3, Duration::from_secs(10)).await;
    sleep(Duration::from_millis(500)).await;

    let ls = cp.listener_reports().await;
    let names: HashSet<String> = ls.iter().map(|r| r.resource_name.clone()).collect();
    assert!(names.contains("lst_a"), "missing lst_a in {names:?}");
    assert!(names.contains("lst_b"), "missing lst_b in {names:?}");
    assert!(names.contains("lst_c"), "missing lst_c in {names:?}");
    assert!(!names.contains("lst_ok"), "healthy listener must not be reported");

    for r in &ls {
        assert_eq!(r.resource_type, ResourceType::Listener as i32);
        match r.resource_name.as_str() {
            "lst_a" => assert_eq!(r.error_details, "error A: boom"),
            "lst_b" => assert_eq!(r.error_details, "error B: kaboom"),
            "lst_c" => assert_eq!(r.error_details, "error C: splat"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_error_message_preserved_end_to_end() {
    let err = "验证失败: signout_path 不能为空 — 🔥 invalid路径 \u{1F4A5}";
    let dump =
        listeners_dump(vec![listener_with_error("lst_unicode", err, "2026-04-13T00:00:00Z")]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-unicode", 1, &[]);

    wait_reports(&cp, 1, Duration::from_secs(10)).await;
    let ls = cp.listener_reports().await;
    assert_eq!(ls.len(), 1, "expected exactly one unicode report");
    assert_eq!(
        ls[0].error_details, err,
        "unicode must survive JSON + gRPC round-trip byte-for-byte"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cluster_error_state_reported_as_cluster() {
    let dump = clusters_dump(vec![cluster_with_error(
        "cluster_bad",
        "Cluster validation failed",
        "2026-04-13T00:00:00Z",
    )]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-cl", 1, &[]);

    wait_reports(&cp, 1, Duration::from_secs(10)).await;
    let ls = cp.listener_reports().await;
    assert_eq!(
        ls.len(),
        1,
        "expected one cluster report delivered over ListenerStateReport message"
    );
    assert_eq!(ls[0].resource_name, "cluster_bad");
    assert_eq!(
        ls[0].resource_type,
        ResourceType::Cluster as i32,
        "resource_type discriminator must be CLUSTER for cluster error_state"
    );
    assert_eq!(ls[0].error_details, "Cluster validation failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn route_config_error_state_reported_as_route_config() {
    let dump = routes_dump(vec![route_with_error(
        "routes_bad",
        "Route config invalid",
        "2026-04-13T00:00:00Z",
    )]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-rc", 1, &[]);

    wait_reports(&cp, 1, Duration::from_secs(10)).await;
    let ls = cp.listener_reports().await;
    assert_eq!(ls.len(), 1, "expected one route_config report");
    assert_eq!(ls[0].resource_name, "routes_bad");
    assert_eq!(
        ls[0].resource_type,
        ResourceType::RouteConfig as i32,
        "resource_type discriminator must be ROUTE_CONFIG for route error_state"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_json_does_not_crash_agent_retries() {
    // Sequence: malformed → healthy error → agent must recover
    let good = listeners_dump(vec![listener_with_error(
        "lst_after_recovery",
        "error post-recovery",
        "2026-04-13T00:00:00Z",
    )]);
    let admin = AdminState::new(vec![
        AdminResponse::Raw("this is not json at all !!!@@@###".to_string()),
        AdminResponse::Raw("{not-json:".to_string()),
        AdminResponse::Json(good),
    ]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-malformed", 1, &[]);

    // Wait for at least one report after recovery
    let got = wait_reports(&cp, 1, Duration::from_secs(15)).await;
    assert!(!got.is_empty(), "agent must recover from malformed responses and continue polling");
    let ls = cp.listener_reports().await;
    assert!(
        ls.iter().any(|r| r.resource_name == "lst_after_recovery"),
        "expected recovery report, got {ls:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_http_500_does_not_crash_agent() {
    let good = listeners_dump(vec![listener_with_error(
        "lst_post_500",
        "error after 500",
        "2026-04-13T00:00:00Z",
    )]);
    let admin = AdminState::new(vec![
        AdminResponse::Status(500),
        AdminResponse::Status(500),
        AdminResponse::Json(good),
    ]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-500", 1, &[]);

    wait_reports(&cp, 1, Duration::from_secs(15)).await;
    let ls = cp.listener_reports().await;
    assert!(
        ls.iter().any(|r| r.resource_name == "lst_post_500"),
        "agent must continue after HTTP 500"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_http_404_does_not_crash_agent() {
    let good = listeners_dump(vec![listener_with_error(
        "lst_post_404",
        "error after 404",
        "2026-04-13T00:00:00Z",
    )]);
    let admin = AdminState::new(vec![AdminResponse::Status(404), AdminResponse::Json(good)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-404", 1, &[]);

    wait_reports(&cp, 1, Duration::from_secs(15)).await;
    let ls = cp.listener_reports().await;
    assert!(
        ls.iter().any(|r| r.resource_name == "lst_post_404"),
        "agent must continue after HTTP 404"
    );
}

// --- Dedup -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn same_error_across_polls_deduped_to_single_report() {
    let err = "constant error across 5 polls";
    let dump = listeners_dump(vec![listener_with_error("lst_dedup", err, "2026-04-13T00:00:00Z")]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-dedup", 1, &[]);

    // Allow at least 5 poll cycles.
    wait_admin_hits(&admin, 5, Duration::from_secs(15)).await;
    sleep(Duration::from_secs(1)).await;

    let ls = cp.listener_reports().await;
    let count_for_lst = ls.iter().filter(|r| r.resource_name == "lst_dedup").count();
    assert_eq!(
        count_for_lst, 1,
        "agent dedup: expected exactly 1 report for same error across >=5 polls, got {count_for_lst} (all: {ls:?})"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dedup_ignores_last_update_attempt_timestamp() {
    // Five polls, same listener+error, but varying last_update_attempt. Dedup
    // must NOT include the timestamp in the key per the spec.
    let err = "same logical failure, changing timestamp";
    let responses = vec![
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_ts",
            err,
            "2026-04-13T00:00:00Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_ts",
            err,
            "2026-04-13T00:00:10Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_ts",
            err,
            "2026-04-13T00:00:20Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_ts",
            err,
            "2026-04-13T00:00:30Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_ts",
            err,
            "2026-04-13T00:00:40Z",
        )])),
    ];
    let admin = AdminState::new(responses);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-ts", 1, &[]);

    wait_admin_hits(&admin, 5, Duration::from_secs(15)).await;
    sleep(Duration::from_secs(1)).await;

    let ls = cp.listener_reports().await;
    let count = ls.iter().filter(|r| r.resource_name == "lst_ts").count();
    assert_eq!(
        count, 1,
        "dedup must be stable across varying last_update_attempt timestamps; got {count} reports"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn different_errors_same_listener_not_deduped() {
    let responses = vec![
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_d",
            "error_v1",
            "2026-04-13T00:00:00Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_d",
            "error_v2_different",
            "2026-04-13T00:00:10Z",
        )])),
    ];
    let admin = AdminState::new(responses);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    let _agent = spawn_agent(admin_port, cp_port, "dp-diff", 1, &[]);

    // Wait for 2 distinct reports.
    let _ = wait_reports(&cp, 2, Duration::from_secs(15)).await;

    let ls = cp.listener_reports().await;
    let reports_for_d: Vec<&ListenerStateReport> =
        ls.iter().filter(|r| r.resource_name == "lst_d").collect();
    assert_eq!(
        reports_for_d.len(),
        2,
        "different error messages for the same listener must NOT be deduped; got {reports_for_d:?}"
    );
    let details: HashSet<&str> = reports_for_d.iter().map(|r| r.error_details.as_str()).collect();
    assert!(details.contains("error_v1"));
    assert!(details.contains("error_v2_different"));
}

// --- gRPC client resilience --------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cp_unreachable_at_startup_then_agent_connects_and_flushes() {
    let err = "error while CP was offline";
    let dump =
        listeners_dump(vec![listener_with_error("lst_offline", err, "2026-04-13T00:00:00Z")]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;

    // Pre-allocate the CP port but DO NOT start the server yet.
    let cp_port = reserved_port();
    let _agent = spawn_agent(admin_port, cp_port, "dp-offline", 1, &[]);

    // Wait 4 seconds: during this time the agent polls admin repeatedly and
    // buffers reports while CP is unreachable.
    sleep(Duration::from_secs(4)).await;
    assert!(admin.hits().await >= 2, "agent should keep polling admin even when CP is unreachable");

    // Now bring up a CP on the reserved port.
    let cp = CpState::default();
    let cp_clone = cp.clone();
    let addr: SocketAddr = format!("127.0.0.1:{cp_port}").parse().unwrap();
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(EnvoyDiagnosticsServiceServer::new(cp_clone))
            .serve(addr)
            .await;
    });

    // Agent must connect and flush at least one report for lst_offline.
    let got = wait_reports(&cp, 1, Duration::from_secs(20)).await;
    assert!(!got.is_empty(), "agent must recover and send reports once CP becomes reachable");
    let ls = cp.listener_reports().await;
    assert!(
        ls.iter().any(|r| r.resource_name == "lst_offline"),
        "agent must deliver the error observed while CP was offline; got {ls:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cp_connection_reset_agent_reconnects() {
    // Start a CP, observe at least one report, then kill it and start a new
    // one on the same port. Agent must reconnect and deliver new reports.
    let err1 = "first-round error";
    let err2 = "second-round error";

    let admin = AdminState::new(vec![
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_r1",
            err1,
            "2026-04-13T00:00:00Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_r1",
            err1,
            "2026-04-13T00:00:00Z",
        )])),
        AdminResponse::Json(listeners_dump(vec![listener_with_error(
            "lst_r2",
            err2,
            "2026-04-13T00:01:00Z",
        )])),
    ]);
    let admin_port = spawn_admin(admin.clone()).await;

    let cp_port = reserved_port();
    // Start first CP. Per spec the agent reconnects on "CP connection drops"
    // — simulate a real drop by `abort()`ing the spawned server task, which
    // tears down the TCP listener immediately and closes any in-flight
    // streams. (graceful Tonic shutdown via `serve_with_shutdown` would block
    // forever on the agent's open bidi stream and is a different scenario —
    // GOAWAY-driven graceful upgrade — that is out of MVP scope.)
    let cp1 = CpState::default();
    let cp1_clone = cp1.clone();
    let addr: SocketAddr = format!("127.0.0.1:{cp_port}").parse().unwrap();
    let handle1 = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(EnvoyDiagnosticsServiceServer::new(cp1_clone))
            .serve(addr)
            .await;
    });
    // Wait for it to bind.
    for _ in 0..40 {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    let agent = spawn_agent(admin_port, cp_port, "dp-reset", 1, &[]);

    // Expect at least one round-1 report
    let got = wait_reports(&cp1, 1, Duration::from_secs(8)).await;
    assert!(!got.is_empty(), "first CP should receive round-1 report");

    // Force CP1's stream handler to terminate its outbound side. The agent
    // observes Ok(None) on inbound and triggers its reconnect loop.
    cp1.kill_streams();
    handle1.abort();
    let _ = handle1.await;
    sleep(Duration::from_millis(500)).await;

    // Start second CP on the same port
    let cp2 = CpState::default();
    let cp2_clone = cp2.clone();
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(EnvoyDiagnosticsServiceServer::new(cp2_clone))
            .serve(addr)
            .await;
    });

    let got2 = wait_reports(&cp2, 1, Duration::from_secs(20)).await;
    if got2.is_empty() {
        eprintln!("--- agent stderr at failure ---");
        eprintln!("{}", agent.stderr_log.lock().await);
        eprintln!("--- end ---");
    }
    assert!(!got2.is_empty(), "agent must reconnect to CP after reset and deliver reports");
}

#[tokio::test(flavor = "multi_thread")]
async fn bounded_queue_overflow_does_not_crash() {
    // Return a dump with 500 distinct listener errors. With default queue_cap
    // 256, not all will fit. Agent must not crash.
    let mut listeners = Vec::with_capacity(500);
    for i in 0..500 {
        listeners.push(listener_with_error(
            &format!("lst_{i}"),
            &format!("err {i}"),
            "2026-04-13T00:00:00Z",
        ));
    }
    let dump = listeners_dump(listeners);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;

    // Start with CP unreachable so the queue fills up.
    let cp_port = reserved_port();
    let agent =
        spawn_agent(admin_port, cp_port, "dp-overflow", 1, &[("FLOWPLANE_AGENT_QUEUE_CAP", "16")]);

    // Let the agent poll and fill/overflow the queue.
    sleep(Duration::from_secs(4)).await;

    // Agent must still be alive.
    // tokio::process::Child doesn't expose try_wait directly; re-check by peeking.
    // We indirectly verify liveness: stderr has at least the startup line and
    // the process did not self-terminate (if it did, admin hits would stop).
    let hits_before = admin.hits().await;
    sleep(Duration::from_secs(2)).await;
    let hits_after = admin.hits().await;
    assert!(
        hits_after > hits_before,
        "agent must remain alive under queue overflow (admin hits: {hits_before} → {hits_after})"
    );

    // Now bring CP up; agent should flush SOMETHING (up to queue cap).
    let cp = CpState::default();
    let cp_clone = cp.clone();
    let addr: SocketAddr = format!("127.0.0.1:{cp_port}").parse().unwrap();
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(EnvoyDiagnosticsServiceServer::new(cp_clone))
            .serve(addr)
            .await;
    });

    let _ = wait_reports(&cp, 1, Duration::from_secs(15)).await;
    let got = cp.reports().await;
    assert!(!got.is_empty(), "agent should deliver something after queue overflow + CP recovery");

    let err_log = agent.stderr().await;
    // We expect some kind of drop/overflow log, but don't assert exact wording.
    // Just ensure the process is alive and communicating.
    drop(err_log);
}

// --- Configuration -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn missing_dataplane_id_exits_cleanly() {
    // Do not set FLOWPLANE_AGENT_DATAPLANE_ID at all.
    let mut cmd = Command::new(agent_bin());
    cmd.env_remove("FLOWPLANE_AGENT_DATAPLANE_ID")
        .env("FLOWPLANE_AGENT_ENVOY_ADMIN_URL", "http://127.0.0.1:9901")
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", "http://127.0.0.1:50051")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn agent");
    let out = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .expect("agent must exit without hanging when required env var is missing")
        .expect("child wait ok");
    assert!(
        !out.status.success(),
        "agent with no DATAPLANE_ID must exit non-zero; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_cp_endpoint_exits_cleanly() {
    let mut cmd = Command::new(agent_bin());
    cmd.env_remove("FLOWPLANE_AGENT_CP_ENDPOINT")
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dp-x")
        .env("FLOWPLANE_AGENT_ENVOY_ADMIN_URL", "http://127.0.0.1:9901")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn agent");
    let out = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .expect("agent must exit without hanging when CP_ENDPOINT is missing")
        .expect("child wait ok");
    assert!(!out.status.success(), "agent with no CP_ENDPOINT must exit non-zero");
}

#[tokio::test(flavor = "multi_thread")]
async fn non_loopback_admin_url_still_runs_with_warning() {
    // Use a non-loopback but still local-ish address. We use a fake admin
    // bound on 0.0.0.0 (non-loopback form) and point the agent at a hostname
    // that is definitely not loopback. The agent should log a WARN but keep
    // running.
    let err = "after-non-loopback error";
    let dump = listeners_dump(vec![listener_with_error("lst_nl", err, "2026-04-13T00:00:00Z")]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);

    // Bind admin on 0.0.0.0:port instead of 127.0.0.1. Clients can still reach
    // via 127.0.0.1, but we pass a non-loopback-looking URL to the agent.
    let port = alloc_port();
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    let app = Router::new().route("/config_dump", get(admin_handler)).with_state(admin.clone());
    let listener = TcpListener::bind(addr).await.expect("bind 0.0.0.0");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    sleep(Duration::from_millis(200)).await;

    let (cp_port, cp) = spawn_cp().await;

    // Point agent at 0.0.0.0 — this is NOT classified as loopback by the
    // documented heuristic, but is reachable. The agent should WARN, not exit.
    let mut cmd = Command::new(agent_bin());
    cmd.env("FLOWPLANE_AGENT_ENVOY_ADMIN_URL", format!("http://0.0.0.0:{port}"))
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", format!("http://127.0.0.1:{cp_port}"))
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dp-nonloop")
        .env("FLOWPLANE_AGENT_POLL_INTERVAL_SECS", "1")
        .env("RUST_LOG", "flowplane_agent=debug,info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().expect("spawn agent");
    let stderr = child.stderr.take().unwrap();
    let stderr_log = Arc::new(Mutex::new(String::new()));
    {
        let buf = stderr_log.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = r.next_line().await {
                buf.lock().await.push_str(&line);
                buf.lock().await.push('\n');
            }
        });
    }

    let _got = wait_reports(&cp, 1, Duration::from_secs(15)).await;
    let ls = cp.listener_reports().await;
    assert!(
        ls.iter().any(|r| r.resource_name == "lst_nl"),
        "agent with non-loopback admin URL must still run and deliver reports"
    );

    let log = stderr_log.lock().await.clone();
    assert!(
        log.to_lowercase().contains("warn") || log.to_lowercase().contains("loopback") || log.to_lowercase().contains("non-loopback"),
        "agent should emit a WARN-level log mentioning loopback when admin URL is non-loopback; stderr was:\n{log}"
    );

    let _ = child.start_kill();
}

// --- Ack handling ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ack_status_invalid_not_retried_indefinitely() {
    // CP acks every report with INVALID. Agent must not spin on the same
    // report (dedup or some other mechanism must prevent unbounded retry
    // storms). We check that admin hits grow but report count does not
    // grow unboundedly relative to distinct errors.
    let dump = listeners_dump(vec![listener_with_error(
        "lst_invalid",
        "agent must not loop on this",
        "2026-04-13T00:00:00Z",
    )]);
    let admin = AdminState::new(vec![AdminResponse::Json(dump)]);
    let admin_port = spawn_admin(admin.clone()).await;
    let (cp_port, cp) = spawn_cp().await;
    {
        *cp.override_status.lock().await = Some(AckStatus::Invalid);
    }

    let _agent = spawn_agent(admin_port, cp_port, "dp-invalid", 1, &[]);

    sleep(Duration::from_secs(6)).await;

    let reports = cp.reports().await;
    // With dedup + INVALID handling, we should not see dozens of reports for
    // the same logical failure in 6 seconds. Allow up to 3 (initial + possible
    // reconnect duplicates) but not unbounded growth.
    assert!(
        reports.len() <= 3,
        "agent must not resend the same INVALID report repeatedly; got {} reports",
        reports.len()
    );
    assert!(!reports.is_empty(), "agent should at least try once");
}
