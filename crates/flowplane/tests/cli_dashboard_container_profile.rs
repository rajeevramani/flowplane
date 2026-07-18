//! fpv2-m4u.1 — `flowplane dashboard` container-profile flags (black-box, adversarial).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   1. `--listen <addr:port>` binds the given address. An off-loopback bind prints a
//!      prominent warning to stderr; the per-launch URL nonce stays mandatory (requests
//!      without the nonce path prefix are rejected).
//!   2. `--no-open` suppresses browser auto-open: the dashboard runs headless and prints
//!      its URL to stdout.
//!   3. `--url-file <path>`: any pre-existing file at the path is deleted BEFORE the server
//!      starts serving; the new file is written atomically only AFTER a successful bind AND
//!      the first successful upstream (control-plane) fetch; an unwritable path is FATAL
//!      (non-zero exit); without `--url-file` no file is involved.
//!   4. URL derivation: the printed URL and the url-file contents are NEVER derived from the
//!      bind address — an off-loopback bind (`--listen 0.0.0.0:<port>`) still yields
//!      `http://127.0.0.1:<bound-port>/<nonce>/`.
//!   5. Bind failure → non-zero exit with an error naming the address.
//!   6. When up, the dashboard prints `Dashboard running at http://127.0.0.1:<port>/<nonce>/`
//!      on stdout.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports only (`127.0.0.1:0` for the stub; `--listen …:0` for the
//! dashboard, with the real bound port parsed from stdout), with an isolated `HOME` temp
//! dir and unique org/team names. Nothing binds a fixed port, so the suite runs green under
//! default nextest parallelism. Every child is killed via a Drop guard on all paths,
//! including assertion failures. No test needs `FLOWPLANE_TEST_DATABASE_URL` — the control
//! plane is a pure in-test stub.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

const TOKEN: &str = "container-profile-test-token-1a2b";

/// The exact documented stdout announcement prefix (contract clause 6).
const ANNOUNCE_PREFIX: &str = "Dashboard running at http://127.0.0.1:";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub control plane: an in-test axum server on 127.0.0.1:0 serving the two overview
// endpoints. Modes let a test make the upstream healthy, unhealthy-for-a-window (anchored at
// the FIRST request it receives), or permanently unhealthy.
// =============================================================================================

enum StubMode {
    /// Both endpoints 200 always.
    Ok,
    /// 500 for `window` starting at the first request received; 200 afterwards.
    FailFirstWindow(Duration),
    /// 500 always.
    AlwaysFail,
}

struct StubState {
    mode: StubMode,
    /// For `FailFirstWindow`: the instant failures stop, set on the first request.
    fail_until: Mutex<Option<Instant>>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    /// For `FailFirstWindow`: when the failure window ended (None until the first request).
    fn fail_window_end(&self) -> Option<Instant> {
        *self.state.fail_until.lock().unwrap()
    }
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let failing = match state.mode {
        StubMode::Ok => false,
        StubMode::AlwaysFail => true,
        StubMode::FailFirstWindow(window) => {
            let mut guard = state.fail_until.lock().unwrap();
            let until = *guard.get_or_insert_with(|| Instant::now() + window);
            Instant::now() < until
        }
    };
    if failing {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "code": "internal", "message": "upstream not ready" })),
        )
            .into_response();
    }
    let (code, body): (StatusCode, Value) = if path.ends_with("/stats/overview") {
        (
            StatusCode::OK,
            json!({
                "total_dataplanes": 1,
                "live_dataplanes": 1,
                "stale_dataplanes": 0,
                "total_requests": 5,
                "total_errors": 0,
                "warming_failures": 0
            }),
        )
    } else if path.ends_with("/xds/status") {
        (
            StatusCode::OK,
            json!({
                "health": "healthy",
                "recent_nack_count": 0,
                "connected_count": 0,
                "dataplanes": []
            }),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            json!({ "code": "not_found", "message": "no such route" }),
        )
    };
    (code, Json(body)).into_response()
}

async fn start_stub(mode: StubMode) -> StubUpstream {
    let state = Arc::new(StubState {
        mode,
        fail_until: Mutex::new(None),
    });
    let app = Router::new()
        .fallback(stub_handler)
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub upstream to an ephemeral port");
    let addr = listener.local_addr().expect("stub local addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    StubUpstream {
        base_url: format!("http://{addr}"),
        state,
        handle,
    }
}

// =============================================================================================
// Dashboard child: spawn with per-test flags, collect stdout/stderr continuously on threads
// (so a full pipe can never block the child), kill on drop in all paths.
// =============================================================================================

struct DashOpts {
    home: PathBuf,
    server: String,
    /// Arguments after the `dashboard` subcommand (`--listen`, `--no-open`, `--url-file`, …).
    extra_args: Vec<String>,
    /// `FLOWPLANE_DASHBOARD_NO_BROWSER=1` for every test EXCEPT the one where `--no-open`
    /// itself is under test (there the flag alone must suppress the browser).
    no_browser_env: bool,
}

struct DashChild {
    child: Child,
    stdout_buf: Arc<Mutex<String>>,
    stderr_buf: Arc<Mutex<String>>,
}

impl Drop for DashChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Continuously drain `stream` into a shared string buffer on a std thread.
fn collect_stream(stream: impl std::io::Read + Send + 'static) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    let sink = buf.clone();
    std::thread::spawn(move || {
        let mut stream = stream;
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&chunk[..n])),
            }
        }
    });
    buf
}

fn spawn_dashboard(opts: DashOpts) -> DashChild {
    let mut cmd = common::flowplane_cmd(&opts.home);
    cmd.env("FLOWPLANE_SERVER", &opts.server)
        .env("FLOWPLANE_TOKEN", TOKEN)
        .env("FLOWPLANE_TEAM", unique("team"))
        .env("FLOWPLANE_ORG", unique("org"))
        .arg("dashboard")
        .args(&opts.extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if opts.no_browser_env {
        cmd.env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1");
    }
    let mut child = cmd.spawn().expect("spawn flowplane dashboard");
    let stdout_buf = collect_stream(child.stdout.take().expect("child stdout piped"));
    let stderr_buf = collect_stream(child.stderr.take().expect("child stderr piped"));
    DashChild {
        child,
        stdout_buf,
        stderr_buf,
    }
}

impl DashChild {
    fn stdout(&self) -> String {
        self.stdout_buf.lock().unwrap().clone()
    }

    fn stderr(&self) -> String {
        self.stderr_buf.lock().unwrap().clone()
    }

    fn still_running(&mut self) -> bool {
        self.child.try_wait().expect("try_wait").is_none()
    }

    /// Poll stdout until the documented announcement line appears; parse (port, nonce).
    async fn wait_for_announcement(&self, ctx: &str) -> (u16, String) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(parsed) = parse_announcement(&self.stdout()) {
                return parsed;
            }
            if Instant::now() >= deadline {
                panic!(
                    "{ctx}: no `{ANNOUNCE_PREFIX}<port>/<nonce>/` announcement on stdout \
                     within 30s; stdout: {:?}; stderr: {:?}",
                    self.stdout(),
                    self.stderr()
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Poll until the child exits on its own; panic (and kill via Drop) if it outlives
    /// `timeout` — a build that starts serving instead of exiting must fail, not hang.
    async fn wait_for_exit(&mut self, timeout: Duration, ctx: &str) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    // Give the collector threads a beat to drain the final output.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    return status;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        panic!(
                            "{ctx}: process was expected to exit within {timeout:?} but is \
                             still running; stdout: {:?}; stderr: {:?}",
                            self.stdout(),
                            self.stderr()
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("{ctx}: try_wait: {e}"),
            }
        }
    }
}

/// Parse `Dashboard running at http://127.0.0.1:<port>/<nonce>/` out of collected stdout.
/// Returns None until the full URL (through the trailing slash) has been flushed.
fn parse_announcement(stdout: &str) -> Option<(u16, String)> {
    let idx = stdout.find(ANNOUNCE_PREFIX)?;
    let rest = &stdout[idx + ANNOUNCE_PREFIX.len()..];
    let mut parts = rest.splitn(3, '/');
    let port: u16 = parts.next()?.parse().ok()?;
    let nonce = parts.next()?.to_string();
    // A third split part proves the terminating slash after the nonce was flushed.
    parts.next()?;
    if nonce.is_empty() || nonce.contains(char::is_whitespace) {
        return None;
    }
    Some((port, nonce))
}

fn expected_url(port: u16, nonce: &str) -> String {
    format!("http://127.0.0.1:{port}/{nonce}/")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client")
}

/// GET with a tolerance for transport-level races only (the server may still be finishing
/// its accept loop); the FIRST HTTP response of any status is returned untouched.
async fn get_first_response(http: &reqwest::Client, url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match http.get(url).send().await {
            Ok(resp) => return resp,
            Err(e) => {
                if Instant::now() >= deadline {
                    panic!("GET {url}: unreachable after 10s: {e}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Recursively look for a file with the given name anywhere under `dir`.
fn find_file_named(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_named(&path, name) {
                return Some(found);
            }
        } else if entry.file_name() == name {
            return Some(path);
        }
    }
    None
}

// =============================================================================================
// Test a (clauses 1, 2, 3, 4, 6): off-loopback bind + --no-open + --url-file. The url-file
// appears only once ready, contains EXACTLY the loopback URL (never the 0.0.0.0 bind
// address), stderr carries the off-loopback warning, the nonce URL serves 200, and the
// nonce stays mandatory (no-nonce request → 404). `--no-open` is under test here, so the
// browser-suppression env var is deliberately NOT set: the flag alone must keep the
// dashboard headless (observable: it stays up and prints its URL).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_file_written_after_ready_and_contains_loopback_url_despite_off_loopback_bind() {
    let stub = start_stub(StubMode::Ok).await;
    let home = common::unique_tempdir();
    let url_file = home.join("dashboard-url");

    let mut dash = spawn_dashboard(DashOpts {
        home: home.clone(),
        server: stub.base_url.clone(),
        extra_args: vec![
            "--listen".into(),
            "0.0.0.0:0".into(),
            "--no-open".into(),
            "--url-file".into(),
            url_file.to_string_lossy().into_owned(),
        ],
        no_browser_env: false, // --no-open itself is under test
    });

    // The url-file must appear (poll ≤10s).
    let deadline = Instant::now() + Duration::from_secs(10);
    let content = loop {
        match std::fs::read_to_string(&url_file) {
            Ok(c) if !c.is_empty() => break c,
            _ => {}
        }
        if Instant::now() >= deadline {
            panic!(
                "url-file was not written within 10s; stdout: {:?}; stderr: {:?}",
                dash.stdout(),
                dash.stderr()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    // Clause 6 + clause 4: stdout announces the LOOPBACK URL even though the bind is 0.0.0.0.
    let (port, nonce) = dash
        .wait_for_announcement("off-loopback --url-file run")
        .await;
    let want = expected_url(port, &nonce);
    assert!(
        content == want || content == format!("{want}\n"),
        "url-file must contain exactly the loopback URL (+ optional trailing newline); \
         want {want:?}, got {content:?}"
    );
    assert!(
        !content.contains("0.0.0.0"),
        "the url-file must NEVER be derived from the bind address; got {content:?}"
    );
    assert!(
        !dash.stdout().contains("0.0.0.0"),
        "the printed URL must NEVER be derived from the bind address; stdout: {:?}",
        dash.stdout()
    );

    // Clause 1: prominent off-loopback warning on stderr, naming the bind.
    let warn_deadline = Instant::now() + Duration::from_secs(5);
    let stderr = loop {
        let s = dash.stderr();
        if s.contains("0.0.0.0") {
            break s;
        }
        if Instant::now() >= warn_deadline {
            panic!(
                "stderr must carry an off-loopback warning mentioning the 0.0.0.0 bind; \
                 stderr: {s:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        stderr.to_lowercase().contains("warn"),
        "the off-loopback notice must be a prominent WARNING; stderr: {stderr:?}"
    );

    // The URL from the file serves the dashboard page.
    let http = client();
    let resp = get_first_response(&http, content.trim_end_matches('\n')).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET of the url-file URL must be 200"
    );

    // Clause 1: the nonce stays mandatory even when bound off-loopback.
    let no_nonce = format!("http://127.0.0.1:{port}/");
    let resp = get_first_response(&http, &no_nonce).await;
    assert_eq!(
        resp.status().as_u16(),
        404,
        "GET {no_nonce} (no nonce prefix) must be rejected with 404 even off-loopback"
    );

    // Clause 2: with --no-open (and no env var) the dashboard runs headless and stays up.
    assert!(
        dash.still_running(),
        "the dashboard must keep running headless under --no-open"
    );
}

// =============================================================================================
// Test b (clause 3): a pre-existing (stale) url-file is deleted BEFORE the server starts
// serving, and the new file appears only AFTER the first successful upstream fetch. The stub
// upstream 500s for 2s starting from its first request, so there is a wide observable window
// in which the path must be ABSENT — stale content must never survive into (or reappear
// during) that window, and the final content must be the new URL.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_url_file_deleted_before_ready() {
    const STALE_MARKER: &str = "stale-nonce-must-never-be-served-0000";

    let stub = start_stub(StubMode::FailFirstWindow(Duration::from_secs(2))).await;
    let home = common::unique_tempdir();
    let url_file = home.join("dashboard-url");
    std::fs::write(&url_file, format!("http://127.0.0.1:1/{STALE_MARKER}/\n"))
        .expect("pre-create stale url-file");

    let dash = spawn_dashboard(DashOpts {
        home: home.clone(),
        server: stub.base_url.clone(),
        extra_args: vec!["--url-file".into(), url_file.to_string_lossy().into_owned()],
        no_browser_env: true,
    });

    // Observe the file's lifecycle: Stale → Absent → New. Stale must never be seen again
    // once deletion has been observed, and New must never appear without Absent first.
    let mut seen_absent = false;
    let deadline = Instant::now() + Duration::from_secs(25);
    let new_content = loop {
        match std::fs::read_to_string(&url_file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => seen_absent = true,
            Err(e) => panic!("reading url-file: {e}"),
            Ok(c) if c.contains(STALE_MARKER) => {
                assert!(
                    !seen_absent,
                    "stale url-file content reappeared after deletion: {c:?}"
                );
                // Still the pre-existing file; deletion has not happened yet.
            }
            Ok(c) if c.trim().is_empty() => panic!(
                "url-file observed existing but empty — it must be deleted before serving \
                 and later written atomically (never truncated/created empty)"
            ),
            Ok(c) => {
                assert!(
                    seen_absent,
                    "new url-file content appeared without the stale file ever being \
                     deleted first (deletion must happen BEFORE the server starts \
                     serving): {c:?}"
                );
                break c;
            }
        }
        if Instant::now() >= deadline {
            panic!(
                "url-file never reached its new content within 25s (seen_absent: \
                 {seen_absent}); stdout: {:?}; stderr: {:?}",
                dash.stdout(),
                dash.stderr()
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // The write may only follow the FIRST SUCCESSFUL upstream fetch — i.e. only after the
    // stub's failure window ended. (Observation time is necessarily >= write time.)
    let window_end = stub
        .fail_window_end()
        .expect("the dashboard must have contacted the upstream before writing the url-file");
    assert!(
        Instant::now() >= window_end,
        "the url-file was written while every upstream fetch was still failing — it must \
         wait for the first successful upstream fetch"
    );

    // Final content is exactly the new URL, never the stale nonce.
    let (port, nonce) = dash.wait_for_announcement("stale url-file run").await;
    let want = expected_url(port, &nonce);
    assert!(
        new_content == want || new_content == format!("{want}\n"),
        "final url-file content must be exactly the new URL (+ optional trailing \
         newline); want {want:?}, got {new_content:?}"
    );
    assert!(
        !new_content.contains(STALE_MARKER),
        "the stale nonce must never appear in the final url-file: {new_content:?}"
    );
}

// =============================================================================================
// Test c (clause 3): while the upstream control plane is down (permanent 500s), the url-file
// must NOT be written — but the dashboard itself is up: it announces its URL and serves its
// page, and the process keeps running.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_file_not_written_while_upstream_down() {
    let stub = start_stub(StubMode::AlwaysFail).await;
    let home = common::unique_tempdir();
    let url_file = home.join("dashboard-url");

    let mut dash = spawn_dashboard(DashOpts {
        home: home.clone(),
        server: stub.base_url.clone(),
        extra_args: vec!["--url-file".into(), url_file.to_string_lossy().into_owned()],
        no_browser_env: true,
    });

    // The dashboard itself comes up and announces (clause 6) despite the dead upstream.
    let (port, nonce) = dash.wait_for_announcement("upstream-down run").await;

    // Grace period: readiness can never be reached, so the file must not appear.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !url_file.exists(),
        "the url-file must NOT be written while the upstream has never returned success; \
         found: {:?}",
        std::fs::read_to_string(&url_file)
    );
    assert!(
        dash.still_running(),
        "a down upstream must not kill the dashboard; stderr: {:?}",
        dash.stderr()
    );

    // The page itself still renders.
    let resp = get_first_response(&client(), &expected_url(port, &nonce)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the dashboard page must still serve 200 while the upstream is down"
    );
}

// =============================================================================================
// Test d (clause 3): an unwritable --url-file path (its parent is a regular file) is FATAL —
// the process exits non-zero quickly and stderr references the url-file path problem.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unwritable_url_file_is_fatal() {
    let stub = start_stub(StubMode::Ok).await;
    let home = common::unique_tempdir();
    let blocker = home.join("blocker");
    std::fs::write(&blocker, "a regular file, not a directory").expect("create blocker file");
    let url_file = blocker.join("dashboard-url");

    let mut dash = spawn_dashboard(DashOpts {
        home: home.clone(),
        server: stub.base_url.clone(),
        extra_args: vec!["--url-file".into(), url_file.to_string_lossy().into_owned()],
        no_browser_env: true,
    });

    let status = dash
        .wait_for_exit(Duration::from_secs(10), "unwritable --url-file")
        .await;
    assert!(
        !status.success(),
        "an unwritable --url-file path must be fatal (non-zero exit), got: {status:?}; \
         stderr: {:?}",
        dash.stderr()
    );
    let stderr = dash.stderr();
    assert!(
        stderr.contains("blocker"),
        "stderr must reference the unwritable url-file path (…/blocker/dashboard-url); \
         got: {stderr:?}"
    );
}

// =============================================================================================
// Test e (clause 5): binding an already-occupied address fails the process with a non-zero
// exit and an error naming the address.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_failure_exits_nonzero_naming_address() {
    // Occupy an ephemeral loopback port for the whole test.
    let occupier = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupier");
    let addr = occupier.local_addr().expect("occupier addr").to_string();

    let stub = start_stub(StubMode::Ok).await;
    let mut dash = spawn_dashboard(DashOpts {
        home: common::unique_tempdir(),
        server: stub.base_url.clone(),
        extra_args: vec!["--listen".into(), addr.clone()],
        no_browser_env: true,
    });

    let status = dash
        .wait_for_exit(Duration::from_secs(15), "occupied --listen address")
        .await;
    assert!(
        !status.success(),
        "binding an occupied address ({addr}) must exit non-zero, got: {status:?}"
    );
    let stderr = dash.stderr();
    assert!(
        stderr.contains(&addr),
        "the bind-failure error must name the address {addr:?}; stderr: {stderr:?}"
    );
    assert!(
        !dash.stdout().contains(ANNOUNCE_PREFIX),
        "no server may be announced when the bind fails; stdout: {:?}",
        dash.stdout()
    );
    drop(occupier);
}

// =============================================================================================
// Test f (clauses 3, 6): with none of the new flags, the native profile is unchanged — the
// dashboard announces its loopback URL, serves it, and no url-file is created anywhere in
// the (isolated) HOME.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_flags_native_profile_unchanged() {
    let stub = start_stub(StubMode::Ok).await;
    let home = common::unique_tempdir();

    let mut dash = spawn_dashboard(DashOpts {
        home: home.clone(),
        server: stub.base_url.clone(),
        extra_args: vec![],
        no_browser_env: true,
    });

    let (port, nonce) = dash.wait_for_announcement("native-profile run").await;
    let resp = get_first_response(&client(), &expected_url(port, &nonce)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the announced URL must serve the dashboard page"
    );

    // Without --url-file no file may be involved.
    assert_eq!(
        find_file_named(&home, "dashboard-url"),
        None,
        "no url-file may be created anywhere under HOME when --url-file is not given"
    );
    assert!(dash.still_running(), "the dashboard must still be running");
}

// =============================================================================================
// Regression (Codex diff-review pass 1, replacing the F1 "no --listen flag exists" test's
// exposure protection): the no-flags native profile must BIND loopback-only — announcing a
// 127.0.0.1 URL is not proof, because an accidental 0.0.0.0 default bind would serve that
// URL too. If the host has a non-loopback interface address, connecting to it on the
// announced port must fail.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_profile_binds_loopback_only() {
    let stub = start_stub(StubMode::Ok).await;
    let home = common::unique_tempdir();

    let mut dash = spawn_dashboard(DashOpts {
        home: home.clone(),
        server: stub.base_url.clone(),
        extra_args: vec![],
        no_browser_env: true,
    });

    let (port, nonce) = dash.wait_for_announcement("loopback-only bind run").await;

    // Loopback reachability first, so a total bind failure cannot masquerade as success.
    let resp = get_first_response(&client(), &expected_url(port, &nonce)).await;
    assert_eq!(resp.status().as_u16(), 200, "loopback URL must serve");

    // Discover a non-loopback local interface address without sending traffic: a UDP
    // socket "connected" to a public address reports the local egress IP. Hosts with no
    // route (fully offline CI) skip the outside probe; the loopback assertion above ran.
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").expect("bind udp probe");
    if udp.connect("8.8.8.8:80").is_ok() {
        if let Ok(local) = udp.local_addr() {
            if !local.ip().is_loopback() {
                let target = std::net::SocketAddr::new(local.ip(), port);
                let attempt = tokio::time::timeout(
                    Duration::from_secs(2),
                    tokio::net::TcpStream::connect(target),
                )
                .await;
                if let Ok(Ok(_)) = attempt {
                    panic!(
                        "default profile must bind loopback-only, but {target} accepted a \
                         connection"
                    );
                }
            }
        }
    }
    assert!(dash.still_running(), "the dashboard must still be running");
}
