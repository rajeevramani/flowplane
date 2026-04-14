//! Adversarial integration tests for fp-hsk.6: bootstrapper agent supervisor.
//!
//! Black-box against the public bootstrapper entry points
//! `handle_init_with_runner` / `handle_down_with_runner`, using a
//! `MockComposeRunner` so we don't need real Docker. We satisfy
//! `wait_for_healthy` with a stub TCP server on 127.0.0.1:8080, redirect
//! `$HOME` to a `TempDir`, and observe side-effects: process table (pgrep),
//! `data/logs/flowplane-agent-*.log`, init/down idempotence.
//!
//! The spawn/kill wiring in `src/cli/compose.rs` and the entire
//! `src/cli/agent_supervisor.rs` module were deliberately NOT read while
//! authoring these tests. Inputs are derived from the bead spec only:
//!   - flag: `flowplane init --with-envoy` must spawn flowplane-agent
//!   - env:  FLOWPLANE_DEV_DISABLE_AGENT=1 must skip the spawn
//!   - env:  FLOWPLANE_AGENT_BIN points at the agent binary
//!   - logs: `data/logs/flowplane-agent-*.log`
//!   - down: must terminate the agent before compose-down
//!
//! Tests share global state (HOME, port 8080, env vars), so they serialize
//! through a process-wide mutex. Each test cleans up any flowplane-agent
//! process it spawned even on failure (Drop on `TestCtx`).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use flowplane::cli::compose::{handle_down_with_runner, handle_init_with_runner};
use flowplane::cli::compose_runner::MockComposeRunner;

// ---------------------------------------------------------------------------
// Test harness primitives
// ---------------------------------------------------------------------------

fn serial_lock() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn new(keys: &[&'static str]) -> Self {
        let saved = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

/// Long-lived stub HTTP server on 127.0.0.1:8080 that satisfies
/// `wait_for_healthy` by replying `200 OK` to anything. Started once for the
/// lifetime of the test process — sharing across tests avoids TIME_WAIT and
/// rebind churn.
fn ensure_stub_health_server() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let listener = TcpListener::bind("127.0.0.1:8080").expect(
            "tests/dev_agent_supervisor: port 8080 must be free — stop any local Flowplane CP first",
        );
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut s, _)) => {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                }
                Err(_) => return,
            }
        });
    });
}

fn agent_bin() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let p = Path::new(manifest).join("target/debug/flowplane-agent");
    assert!(
        p.exists(),
        "flowplane-agent binary not built. Run: cargo build --bin flowplane-agent (looked at {})",
        p.display()
    );
    p
}

/// Return PIDs whose argv contains the path `target/debug/flowplane-agent`.
/// Filters by the binary path so we don't accidentally collide with other
/// flowplane-* processes the developer may have running.
fn pgrep_agent() -> Vec<u32> {
    let out =
        std::process::Command::new("pgrep").args(["-f", "target/debug/flowplane-agent"]).output();
    let out = match out {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("kill").arg("-9").arg(pid.to_string()).status();
}

fn agent_log_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join("logs");
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("flowplane-agent"))
                .unwrap_or(false)
        })
        .collect()
}

fn wait_for<F: Fn() -> bool>(timeout: Duration, predicate: F) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Per-test fixture: holds the serial lock, a `$HOME` tempdir, env restoration,
/// the stub health server, and a baseline of pre-existing agent PIDs so we
/// only assert on processes spawned by this test.
struct TestCtx {
    _serial: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    _env: EnvGuard,
    baseline_pids: Vec<u32>,
    pre_log_files: Vec<PathBuf>,
}

impl TestCtx {
    fn new() -> Self {
        let _serial = serial_lock();
        let _env = EnvGuard::new(&[
            "HOME",
            "FLOWPLANE_SOURCE_DIR",
            "FLOWPLANE_AGENT_BIN",
            "FLOWPLANE_DEV_DISABLE_AGENT",
        ]);
        let home = tempfile::tempdir().expect("create tempdir for HOME");
        std::env::set_var("HOME", home.path());
        std::env::set_var("FLOWPLANE_SOURCE_DIR", env!("CARGO_MANIFEST_DIR"));
        std::env::remove_var("FLOWPLANE_DEV_DISABLE_AGENT");
        std::env::set_var("FLOWPLANE_AGENT_BIN", agent_bin());

        // Seed a dummy credentials file at the path compose::init falls back to
        // when FLOWPLANE_CREDENTIALS_PATH is unset (HOME/.flowplane/credentials).
        // A real CP writes this during startup; MockComposeRunner is a no-op,
        // so we stand in for that side effect here.
        let cred_path = home.path().join(".flowplane").join("credentials");
        std::fs::create_dir_all(cred_path.parent().expect("credentials path has parent"))
            .expect("create .flowplane dir in tempdir HOME");
        std::fs::write(&cred_path, b"ZmFrZS1oZWFkZXI.ZmFrZS1wYXlsb2Fk.ZmFrZS1zaWc")
            .expect("seed dummy credentials file");

        let baseline_pids = pgrep_agent();
        let pre_log_files = agent_log_files();

        ensure_stub_health_server();

        Self { _serial, home, _env, baseline_pids, pre_log_files }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn new_pids(&self) -> Vec<u32> {
        pgrep_agent().into_iter().filter(|p| !self.baseline_pids.contains(p)).collect()
    }

    fn new_log_files(&self) -> Vec<PathBuf> {
        agent_log_files().into_iter().filter(|p| !self.pre_log_files.contains(p)).collect()
    }
}

impl Drop for TestCtx {
    fn drop(&mut self) {
        // Kill any agent process this test introduced.
        for pid in self.new_pids() {
            kill_pid(pid);
        }
        // Remove agent log files this test created (best-effort).
        for p in self.new_log_files() {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn mock_runner() -> MockComposeRunner {
    MockComposeRunner::default()
}

// ---------------------------------------------------------------------------
// Spawn behaviour
// ---------------------------------------------------------------------------

#[test]
fn init_with_envoy_spawns_flowplane_agent_subprocess() {
    let ctx = TestCtx::new();
    let runner = mock_runner();

    handle_init_with_runner(true, false, &runner)
        .expect("init --with-envoy should succeed against mock runner + stub health");

    assert!(
        wait_for(Duration::from_secs(15), || !ctx.new_pids().is_empty()),
        "expected at least one flowplane-agent process running after init --with-envoy"
    );

    let pids = ctx.new_pids();
    assert_eq!(
        pids.len(),
        1,
        "init --with-envoy should spawn exactly one agent process; got {pids:?}"
    );

    // The home tempdir is the source of truth for any state files; just check
    // that the test fixture is wired up correctly.
    assert!(ctx.home().join(".flowplane").exists(), ".flowplane dir should exist after init");
}

#[test]
fn init_writes_non_empty_agent_log_file() {
    let ctx = TestCtx::new();
    let runner = mock_runner();

    handle_init_with_runner(true, false, &runner).expect("init should succeed");

    assert!(
        wait_for(Duration::from_secs(15), || !ctx.new_log_files().is_empty()),
        "expected data/logs/flowplane-agent-*.log to be created within 15s"
    );

    let logs = ctx.new_log_files();
    let log = logs.first().expect("at least one agent log file");

    // Agent must actually start and write at least one line — proves the
    // subprocess didn't immediately crash silently.
    assert!(
        wait_for(Duration::from_secs(15), || {
            std::fs::metadata(log).map(|m| m.len() > 0).unwrap_or(false)
        }),
        "agent log file {log:?} stayed empty for 15s — subprocess likely died on launch"
    );
}

// ---------------------------------------------------------------------------
// Resilience / opt-out
// ---------------------------------------------------------------------------

#[test]
fn disable_env_skips_agent_spawn_but_init_succeeds() {
    let ctx = TestCtx::new();
    std::env::set_var("FLOWPLANE_DEV_DISABLE_AGENT", "1");
    let runner = mock_runner();

    handle_init_with_runner(true, false, &runner)
        .expect("init must still succeed when agent is disabled");

    // Give any potential async spawn a moment to misbehave.
    std::thread::sleep(Duration::from_millis(750));

    let stray = ctx.new_pids();
    assert!(
        stray.is_empty(),
        "FLOWPLANE_DEV_DISABLE_AGENT=1 must NOT spawn a flowplane-agent subprocess; saw {stray:?}"
    );
    let stray_logs = ctx.new_log_files();
    assert!(
        stray_logs.is_empty(),
        "FLOWPLANE_DEV_DISABLE_AGENT=1 must NOT create an agent log file; saw {stray_logs:?}"
    );
}

#[test]
fn missing_agent_binary_warns_but_init_succeeds() {
    let ctx = TestCtx::new();
    std::env::set_var("FLOWPLANE_AGENT_BIN", "/nonexistent/flowplane-agent-doesnt-exist-xyz");
    let runner = mock_runner();

    let result = handle_init_with_runner(true, false, &runner);
    assert!(
        result.is_ok(),
        "init must succeed (WARN only) when FLOWPLANE_AGENT_BIN points at a missing file: {result:?}"
    );

    std::thread::sleep(Duration::from_millis(500));
    let stray = ctx.new_pids();
    assert!(
        stray.is_empty(),
        "no flowplane-agent process should be running when the binary is missing; saw {stray:?}"
    );
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn second_init_replaces_prior_agent_process() {
    let ctx = TestCtx::new();
    let runner = mock_runner();

    handle_init_with_runner(true, false, &runner).expect("first init");
    assert!(
        wait_for(Duration::from_secs(15), || ctx.new_pids().len() == 1),
        "first init should yield exactly one new agent pid; got {:?}",
        ctx.new_pids()
    );
    let first_pid = ctx.new_pids()[0];

    handle_init_with_runner(true, false, &runner).expect("second init");

    let replaced = wait_for(Duration::from_secs(15), || {
        let pids = ctx.new_pids();
        pids.len() == 1 && pids[0] != first_pid
    });
    assert!(
        replaced,
        "second init must reap the prior agent and spawn a fresh one; \
         pids after second init = {:?}, first_pid was {first_pid}",
        ctx.new_pids()
    );
}

#[test]
fn down_kills_agent_subprocess() {
    let ctx = TestCtx::new();
    let runner = mock_runner();

    handle_init_with_runner(true, false, &runner).expect("init");
    assert!(
        wait_for(Duration::from_secs(15), || !ctx.new_pids().is_empty()),
        "expected agent to be running before down"
    );

    handle_down_with_runner(false, &runner).expect("down should succeed");

    assert!(
        wait_for(Duration::from_secs(10), || ctx.new_pids().is_empty()),
        "flowplane down must terminate the agent subprocess; still alive: {:?}",
        ctx.new_pids()
    );
}

#[test]
fn corrupt_pid_file_does_not_block_init() {
    let ctx = TestCtx::new();
    // Pre-create .flowplane and a few plausible "stale agent state" files
    // before init runs. The bead does not pin a specific PID file path, so
    // we cover the most likely candidates.
    let fp_dir = ctx.home().join(".flowplane");
    std::fs::create_dir_all(&fp_dir).expect("mk .flowplane");
    for name in ["agent.pid", "flowplane-agent.pid", "agent-supervisor.pid"] {
        std::fs::write(fp_dir.join(name), b"NOT-A-PID\n").expect("write garbage pid");
    }

    let runner = mock_runner();

    let result = handle_init_with_runner(true, false, &runner);
    assert!(
        result.is_ok(),
        "init must tolerate a corrupt pid file from a prior crashed run: {result:?}"
    );

    // After init, a fresh agent should be running regardless of the prior
    // garbage state.
    assert!(
        wait_for(Duration::from_secs(15), || !ctx.new_pids().is_empty()),
        "init should still spawn an agent even when stale pid files exist"
    );
}
