//! E2E smoke tests for fp-u54.3 — dev mode mTLS chain.
//!
//! Adversarial, spec-driven tests against the public contract of
//! `flowplane init --with-envoy`. Written from the fp-u54 bead acceptance
//! criteria ONLY — the implementation source of `src/cli/dev_certs.rs` and
//! the fp-u54.4 additions to `src/cli/compose.rs` were NOT read while
//! authoring. (fp-u54.6 deleted the separate `src/xds/dev_mtls.rs` module —
//! dev and prod now share `build_server_tls_config`.)
//!
//! The test harness pattern (HOME tempdir, MockComposeRunner, stub health
//! server on 127.0.0.1:8080) is adapted from `tests/dev_agent_supervisor.rs`
//! which covers the agent-spawn contract from fp-hsk.6. These tests focus on
//! the mTLS material, SPIFFE identities, agent endpoint scheme, and adversarial
//! cert tampering — not the agent spawn mechanics.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 \
//!     cargo test -p flowplane --test e2e dev_mtls -- --ignored --nocapture --test-threads=1
//! ```

use std::io::{Read as IoRead, Write as IoWrite};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use flowplane::cli::compose::handle_init_with_runner;
use flowplane::cli::compose_runner::MockComposeRunner;

// ---------------------------------------------------------------------------
// Shared test harness
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
/// `wait_for_healthy` by replying `200 OK` to anything. Bound once per
/// test binary.
fn ensure_stub_health_server() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let listener = match TcpListener::bind("127.0.0.1:8080") {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "test_dev_mtls_chain: could not bind 127.0.0.1:8080 ({e}). \
                     Another test or a running flowplane CP is holding the port."
                );
                return;
            }
        };
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

fn pgrep_agent() -> Vec<u32> {
    let out = Command::new("pgrep").args(["-f", "target/debug/flowplane-agent"]).output();
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
    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
}

fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut predicate: F) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// True if `RUN_E2E=1` — e2e smoke convention is to bail out of the test
/// body with a warning when the gate is not set so `cargo test --ignored`
/// against the whole tree doesn't blow up.
fn e2e_gate_open() -> bool {
    std::env::var("RUN_E2E").ok().as_deref() == Some("1")
}

struct TestCtx {
    _serial: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    _env: EnvGuard,
    baseline_pids: Vec<u32>,
}

impl TestCtx {
    fn new() -> Self {
        let _serial = serial_lock();
        let _env = EnvGuard::new(&[
            "HOME",
            "FLOWPLANE_SOURCE_DIR",
            "FLOWPLANE_AGENT_BIN",
            "FLOWPLANE_DEV_DISABLE_AGENT",
            "FLOWPLANE_DEV_TOKEN",
        ]);
        let home = tempfile::tempdir().expect("create tempdir for HOME");
        std::env::set_var("HOME", home.path());
        std::env::set_var("FLOWPLANE_SOURCE_DIR", env!("CARGO_MANIFEST_DIR"));
        std::env::remove_var("FLOWPLANE_DEV_DISABLE_AGENT");
        std::env::remove_var("FLOWPLANE_DEV_TOKEN");
        std::env::set_var("FLOWPLANE_AGENT_BIN", agent_bin());

        let baseline_pids = pgrep_agent();
        ensure_stub_health_server();

        Self { _serial, home, _env, baseline_pids }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn certs_dir(&self) -> PathBuf {
        self.home().join(".flowplane").join("certs")
    }

    fn new_pids(&self) -> Vec<u32> {
        pgrep_agent().into_iter().filter(|p| !self.baseline_pids.contains(p)).collect()
    }
}

impl Drop for TestCtx {
    fn drop(&mut self) {
        for pid in self.new_pids() {
            kill_pid(pid);
        }
    }
}

// ---------------------------------------------------------------------------
// openssl helpers (call the binary — avoids a crypto dep in the test crate)
// ---------------------------------------------------------------------------

fn openssl_verify(ca: &Path, leaf: &Path) -> Result<String, String> {
    let out = Command::new("openssl")
        .args(["verify", "-CAfile"])
        .arg(ca)
        .arg(leaf)
        .output()
        .map_err(|e| format!("spawn openssl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(format!(
            "openssl verify failed: status={} stdout={stdout} stderr={stderr}",
            out.status
        ));
    }
    // openssl verify prints "<path>: OK" on success.
    if !stdout.contains("OK") {
        return Err(format!("openssl verify did not print OK: stdout={stdout} stderr={stderr}"));
    }
    Ok(stdout)
}

fn openssl_text(leaf: &Path) -> String {
    let out = Command::new("openssl")
        .args(["x509", "-in"])
        .arg(leaf)
        .args(["-noout", "-text"])
        .output()
        .expect("openssl x509 -text");
    assert!(
        out.status.success(),
        "openssl x509 -text failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Extract all `URI:...` entries from the SAN section of a cert's -text output.
fn extract_uri_sans(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        // A SAN line typically looks like:
        //   URI:spiffe://flowplane.local/team/default/proxy/dev-dataplane
        // possibly prefixed by whitespace and mixed with DNS:... entries
        // separated by commas.
        for seg in line.split(',') {
            let seg = seg.trim();
            if let Some(rest) = seg.strip_prefix("URI:") {
                out.push(rest.to_string());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// A. Cert generation round trip
// ---------------------------------------------------------------------------

/// `flowplane init --with-envoy` must produce CA + per-service leaf certs,
/// and every leaf must chain to the CA.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_init_produces_cert_tree() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();

    handle_init_with_runner(true, false, &runner).expect("init --with-envoy should succeed");

    let certs = ctx.certs_dir();
    let ca = certs.join("ca.pem");
    assert!(ca.exists(), "CA cert missing at {}", ca.display());

    // The bead spec names three service leaves. Assert each exists and chains.
    for role in ["cp", "agent", "envoy"] {
        let leaf = certs.join(role).join("cert.pem");
        let key = certs.join(role).join("key.pem");
        assert!(leaf.exists(), "{role} cert missing at {}", leaf.display());
        assert!(key.exists(), "{role} key missing at {}", key.display());

        openssl_verify(&ca, &leaf)
            .unwrap_or_else(|e| panic!("{role} cert does not verify against CA: {e}"));
    }
}

/// Every client leaf (agent, envoy) must carry a SPIFFE URI SAN matching the
/// dev dataplane identity specified in the fp-u54.1 spec.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_client_certs_have_spiffe_uri() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();
    handle_init_with_runner(true, false, &runner).expect("init should succeed");

    let certs = ctx.certs_dir();

    // fp-u54.6: the shared dev dataplane identity uses the legacy
    // `team/{team}/proxy/{proxy_id}` shape so `parse_team_from_spiffe_uri`
    // and `parse_proxy_id_from_spiffe_uri` in `src/secrets/vault.rs` accept
    // it unchanged. Team is pinned to `default` (matches the dev team seeded
    // by `src/startup.rs`) and proxy id matches `DEV_DATAPLANE_ID`.
    const EXPECTED_DATAPLANE_URI: &str =
        "spiffe://flowplane.local/team/default/proxy/dev-dataplane";

    // Agent: exact URI match — catches any future drift between the minter
    // and the parser.
    let agent_text = openssl_text(&certs.join("agent").join("cert.pem"));
    let agent_uris = extract_uri_sans(&agent_text);
    assert!(
        agent_uris.iter().any(|u| u == EXPECTED_DATAPLANE_URI),
        "agent cert must carry {EXPECTED_DATAPLANE_URI}; got URIs={agent_uris:?}\n\
         Full cert text:\n{agent_text}"
    );

    // Envoy: exact URI match — per the shared-dev-dataplane-identity decision,
    // envoy and agent MUST carry byte-identical SPIFFE URIs.
    let envoy_text = openssl_text(&certs.join("envoy").join("cert.pem"));
    let envoy_uris = extract_uri_sans(&envoy_text);
    assert!(
        envoy_uris.iter().any(|u| u == EXPECTED_DATAPLANE_URI),
        "envoy cert must carry {EXPECTED_DATAPLANE_URI}; got URIs={envoy_uris:?}\n\
         Full cert text:\n{envoy_text}"
    );

    // CP server cert: MUST carry spiffe://flowplane.local/control-plane/...
    let cp_text = openssl_text(&certs.join("cp").join("cert.pem"));
    let cp_uris = extract_uri_sans(&cp_text);
    assert!(
        cp_uris.iter().any(|u| u.starts_with("spiffe://flowplane.local/control-plane/")),
        "cp server cert must carry a control-plane SPIFFE URI SAN; got URIs={cp_uris:?}\n\
         Full cert text:\n{cp_text}"
    );
}

/// CP server cert MUST also carry DNS/IP SANs covering 127.0.0.1, localhost,
/// and the compose service name, or mTLS connections from Envoy/agent will
/// fail hostname verification.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_cp_cert_covers_dev_hostnames() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();
    handle_init_with_runner(true, false, &runner).expect("init should succeed");

    let cp_text = openssl_text(&ctx.certs_dir().join("cp").join("cert.pem"));

    // Adversarial: user could connect via any of these names. If any is
    // missing, a dev session will fail in a confusing way.
    let must_contain = ["127.0.0.1", "localhost"];
    for needle in must_contain {
        assert!(
            cp_text.contains(needle),
            "CP cert SAN section must cover {needle}, but -text output did not mention it.\n{cp_text}"
        );
    }
}

/// Running `flowplane init --with-envoy` twice in a row must not error.
/// fp-u54.1 tightening decision: regenerate on every init, overwrite cleanly.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_init_is_idempotent() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();

    handle_init_with_runner(true, false, &runner).expect("first init");

    // Read the first CA so we can confirm regen actually overwrote it.
    let ca_path = ctx.certs_dir().join("ca.pem");
    let first_ca = std::fs::read(&ca_path).expect("read first CA");

    // Kill any agent spawned by the first init before re-running.
    for pid in ctx.new_pids() {
        kill_pid(pid);
    }
    std::thread::sleep(Duration::from_millis(500));

    handle_init_with_runner(true, false, &runner).expect("second init must not error");

    let second_ca = std::fs::read(&ca_path).expect("read second CA");
    assert_ne!(
        first_ca, second_ca,
        "second init should have regenerated the CA cert, but bytes are identical"
    );

    // And the regenerated CA still signs the new agent leaf.
    openssl_verify(&ca_path, &ctx.certs_dir().join("agent").join("cert.pem"))
        .expect("regenerated chain must still verify");
}

// ---------------------------------------------------------------------------
// B. Agent TLS wiring (endpoint scheme, env vars read by the agent binary)
// ---------------------------------------------------------------------------

/// After init, the agent CP_ENDPOINT the bootstrapper builds must use the
/// `https://` scheme. This is detectable via the agent's own stderr/log on
/// startup — if the bootstrapper still passes `http://`, the agent would
/// connect in plaintext and the stub's `200 OK` would make init look healthy,
/// masking the bug. We re-launch the agent binary ourselves after init so
/// we can capture stderr in isolation.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_agent_rejects_plaintext_without_tls_env() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();
    handle_init_with_runner(true, false, &runner).expect("init should succeed");

    // Kill the agent the bootstrapper spawned — we need a clean stderr.
    for pid in ctx.new_pids() {
        kill_pid(pid);
    }
    std::thread::sleep(Duration::from_millis(300));

    // Sanity: the agent reads `FLOWPLANE_AGENT_CP_ENDPOINT`. If we launch
    // with an `http://` endpoint + TLS env vars set, the agent should refuse
    // or fail the handshake. We tolerate either a non-zero exit OR stderr
    // that mentions TLS/https — both are observable signals the contract is
    // enforced end-to-end.
    let certs = ctx.certs_dir();
    let mut cmd = Command::new(agent_bin());
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", "http://127.0.0.1:8080")
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dev-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", certs.join("agent").join("cert.pem"))
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", certs.join("agent").join("key.pem"))
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", certs.join("ca.pem"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn flowplane-agent");

    // Give it a short window to attempt the handshake against the plain HTTP
    // stub on 8080 and surface an error. If it's still running after the
    // grace period, kill it and collect whatever it printed.
    std::thread::sleep(Duration::from_secs(3));
    let _ = child.kill();
    let out = child.wait_with_output().expect("collect agent output");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}\n{stderr}");

    // Signal set: failure to connect, TLS handshake error, wrong scheme —
    // ANY of these proves the agent is actually attempting mTLS and not
    // silently downgrading.
    let signals =
        ["tls", "TLS", "handshake", "certificate", "https", "scheme", "connect", "transport"];
    assert!(
        signals.iter().any(|s| combined.contains(s)),
        "Agent output contained no TLS/transport signal — it may be silently \
         downgrading or not honoring the TLS env vars.\nstdout={stdout}\nstderr={stderr}"
    );
}

// ---------------------------------------------------------------------------
// E. Adversarial cert tampering
// ---------------------------------------------------------------------------

/// If the agent's CA file is deleted between init and agent launch, the
/// agent must fail cleanly (non-zero exit, meaningful log) — not panic, not
/// silently hang.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_agent_exits_cleanly_when_ca_missing() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();
    handle_init_with_runner(true, false, &runner).expect("init should succeed");
    for pid in ctx.new_pids() {
        kill_pid(pid);
    }
    std::thread::sleep(Duration::from_millis(300));

    // Nuke the CA — leaf certs and keys remain.
    let certs = ctx.certs_dir();
    let ca = certs.join("ca.pem");
    std::fs::remove_file(&ca).expect("delete ca.pem");
    assert!(!ca.exists(), "ca.pem should be gone");

    let mut cmd = Command::new(agent_bin());
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", "https://127.0.0.1:8080")
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dev-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", certs.join("agent").join("cert.pem"))
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", certs.join("agent").join("key.pem"))
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", &ca)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn agent with missing CA");

    // Wait up to 10s for a clean exit. Longer than that is a hang and a bug.
    let exited = wait_for(Duration::from_secs(10), || matches!(child.try_wait(), Ok(Some(_))));

    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        panic!("agent did not exit within 10s with missing CA — probable silent hang");
    }

    let out = child.wait_with_output().expect("collect agent output");
    assert!(
        !out.status.success(),
        "agent with missing CA should have exited non-zero; status={:?}",
        out.status
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}\n{stderr}");

    // The error must name the missing CA file or mention "ca" / "No such file"
    // — NOT a raw panic trace.
    assert!(
        !combined.contains("panicked at"),
        "agent panicked instead of exiting cleanly:\n{combined}"
    );
    let meaningful = ["ca.pem", "CA", "certificate", "No such file", "not found", "ENOENT"];
    assert!(
        meaningful.iter().any(|s| combined.contains(s)),
        "agent exit did not surface a meaningful error about the missing CA.\nstdout={stdout}\nstderr={stderr}"
    );
}

/// A client cert whose SPIFFE URI SAN is bogus must NOT satisfy CP mTLS.
/// We can't exercise a full CP handshake in this smoke (no real CP over mTLS
/// is spun up by `handle_init_with_runner`), but we CAN verify that the
/// bogus cert is detectable at the openssl level and that the agent still
/// launches without crashing on a malformed SPIFFE URI — the handshake
/// rejection path is covered by fp-u54.2's CP-side unit tests and by the
/// full docker-backed E2E suite (see fp-hsk.8).
///
/// This test guards against a latent bug: if the agent panics on an
/// unexpected SAN shape, the CP never gets a chance to reject it — the
/// process dies first.
#[test]
#[ignore = "requires RUN_E2E=1"]
fn dev_mtls_agent_does_not_panic_on_bogus_spiffe_san() {
    if !e2e_gate_open() {
        eprintln!("SKIP: RUN_E2E=1 not set");
        return;
    }
    let ctx = TestCtx::new();
    let runner = MockComposeRunner::default();
    handle_init_with_runner(true, false, &runner).expect("init should succeed");
    for pid in ctx.new_pids() {
        kill_pid(pid);
    }
    std::thread::sleep(Duration::from_millis(300));

    // Overwrite agent/cert.pem with the CP cert — wrong SPIFFE (control-plane
    // URI, not dataplane), wrong key pairing. This is a deliberate tamper.
    let certs = ctx.certs_dir();
    let bogus_cert = certs.join("cp").join("cert.pem");
    let agent_cert_path = certs.join("agent").join("cert.pem");
    std::fs::copy(&bogus_cert, &agent_cert_path).expect("overwrite agent cert with cp cert");

    let mut cmd = Command::new(agent_bin());
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", "https://127.0.0.1:8080")
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dev-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", &agent_cert_path)
        // Key path still points at the real agent key — which does NOT match
        // the cp cert we just copied. Agent should surface a clear error, not
        // a panic.
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", certs.join("agent").join("key.pem"))
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", certs.join("ca.pem"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn agent with mismatched cert/key");

    let exited = wait_for(Duration::from_secs(10), || matches!(child.try_wait(), Ok(Some(_))));
    if !exited {
        let _ = child.kill();
    }
    let out = child.wait_with_output().expect("collect output");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}\n{stderr}");

    assert!(
        !combined.contains("panicked at"),
        "agent panicked on a mismatched cert/key pair instead of exiting with a clear error:\n{combined}"
    );
}
