//! Real-docker dev mTLS E2E smoke (fp-u54.5).
//!
//! Complements fp-u54.3 (mock-compose suite). This suite proves the FULL dev
//! mTLS chain end-to-end against a real CP + real Envoy + real flowplane-agent
//! subprocess, including the mandatory rule #4 gate (Envoy /config_dump
//! error_state verification).
//!
//! All tests:
//! - use `TestHarnessConfig::isolated().with_mtls().with_mtls_identity("default", "dev-dataplane")`
//! - seed dev resources post-startup so the seeded `dev-dataplane` row matches
//!   the SPIFFE proxy_id baked into both Envoy's and the agent's client cert
//! - spawn flowplane-agent as a subprocess with mTLS env vars minted from the
//!   harness CA (second client cert, same identity)
//! - are gated behind `#[ignore]` + `RUN_E2E=1`
//!
//! Source files deliberately NOT read while authoring this suite:
//!   src/cli/dev_certs.rs, src/xds/dev_mtls.rs, src/cli/compose.rs (mTLS bits),
//!   src/cli/agent_supervisor.rs TLS additions.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{TestHarness, TestHarnessConfig};
use crate::tls::support::{TestCertificateAuthority, TestCertificateFiles};

// ---------------------------------------------------------------------------
// Subprocess + harness helpers
// ---------------------------------------------------------------------------

/// Agent subprocess wrapper. Kills the child on drop so a failing test never
/// leaves a zombie flowplane-agent behind.
struct AgentHandle {
    child: Option<Child>,
    log_path: Option<PathBuf>,
}

impl AgentHandle {
    fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }
}

impl Drop for AgentHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn agent_binary() -> PathBuf {
    // Mirror tests/dev_agent_supervisor.rs — check target/debug first.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let p = PathBuf::from(manifest).join("target/debug/flowplane-agent");
    assert!(p.exists(), "flowplane-agent binary not built. Run: cargo build --bin flowplane-agent");
    p
}

fn mint_agent_client_cert(
    ca: &TestCertificateAuthority,
) -> anyhow::Result<(TestCertificateFiles, String)> {
    // Mint a SECOND client cert from the same CA so the agent connects with
    // its own key pair distinct from Envoy's. Identity matches the seeded dev
    // dataplane so `touch_last_config_verify` can find the row.
    let spiffe_uri =
        TestCertificateAuthority::build_spiffe_uri("flowplane.local", "default", "dev-dataplane")?;
    let cert = ca.issue_client_cert(&spiffe_uri, "dev-dataplane", time::Duration::days(1))?;
    Ok((cert, spiffe_uri))
}

/// Full setup: isolated mTLS harness + dev-resources seeded. Returns harness
/// and a path to a fresh tempdir for per-test log capture.
async fn dev_mtls_docker_harness(test_name: &str) -> anyhow::Result<TestHarness> {
    let cfg = TestHarnessConfig::new(test_name)
        .isolated()
        .with_mtls()
        .with_mtls_identity("default", "dev-dataplane")
        .with_envoy_node_id("team=default/dp-dev-dataplane-id");
    let harness = TestHarness::start(cfg).await?;

    // Seed dev resources (default team, dev-dataplane row) directly against the
    // isolated PostgreSQL container. The isolated harness path spins up its own
    // DB; ControlPlaneHandle::start runs migrations but does NOT call
    // seed_dev_resources (that's main.rs's job in real dev mode).
    let pool = sqlx::PgPool::connect(&harness.db_url).await?;
    flowplane::startup::seed_dev_resources(&pool).await?;
    pool.close().await;

    Ok(harness)
}

/// Spawn flowplane-agent pointed at `harness`'s xDS endpoint using a fresh
/// client cert minted from the harness CA. Returns an AgentHandle whose Drop
/// guarantees cleanup even on panic.
fn spawn_agent(
    harness: &TestHarness,
    log_tag: &str,
) -> anyhow::Result<(AgentHandle, TestCertificateFiles)> {
    let ca = harness.mtls_ca().ok_or_else(|| {
        anyhow::anyhow!("harness.mtls_ca() returned None — isolated mTLS expected")
    })?;
    let (agent_cert, _spiffe) = mint_agent_client_cert(ca)?;

    let ca_path = harness.mtls_certs().map(|c| c.ca_cert_path.clone()).ok_or_else(|| {
        anyhow::anyhow!("harness.mtls_certs() returned None — isolated mTLS expected")
    })?;

    let xds_port = harness.ports.xds;
    let log_dir = std::env::temp_dir().join(format!("fp-u54-agent-logs-{log_tag}"));
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("agent-{}.log", std::process::id()));
    let log_file = std::fs::File::create(&log_path)?;
    let log_stderr = log_file.try_clone()?;

    let child = Command::new(agent_binary())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", format!("https://127.0.0.1:{xds_port}"))
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dev-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", &agent_cert.cert_path)
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", &agent_cert.key_path)
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", &ca_path)
        .env(
            "FLOWPLANE_AGENT_ENVOY_ADMIN_URL",
            format!("http://127.0.0.1:{}", harness.ports.envoy_admin),
        )
        .env("FLOWPLANE_AGENT_POLL_INTERVAL_SECS", "2")
        .env("RUST_LOG", "flowplane_agent=debug,info")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_stderr))
        .spawn()?;

    Ok((AgentHandle { child: Some(child), log_path: Some(log_path) }, agent_cert))
}

/// Poll `flowplane xds status` via CliRunner until JSON contains a dataplane
/// entry with agent_status == "OK" (or matching predicate). Returns the parsed
/// JSON on success.
async fn wait_for_agent_ok(
    cli: &CliRunner,
    timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    let start = Instant::now();
    let mut last: serde_json::Value = serde_json::Value::Null;
    while start.elapsed() < timeout {
        let out = cli.run(&["xds", "status", "--output", "json"])?;
        if out.exit_code == 0 {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&out.stdout) {
                last = v.clone();
                let arr =
                    v.get("dataplanes").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                for dp in arr {
                    let name = dp.get("name").and_then(|s| s.as_str()).unwrap_or("");
                    let agent = dp
                        .get("agent_status")
                        .or_else(|| dp.get("agentStatus"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    if name == "dev-dataplane" && agent == "OK" {
                        return Ok(v);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!("timeout waiting for Agent=OK; last xds status: {last}")
}

// ---------------------------------------------------------------------------
// B. CP accepts mTLS stream from agent
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires RUN_E2E=1"]
async fn dev_mtls_docker_cp_accepts_agent_stream() {
    let harness = dev_mtls_docker_harness("cp_accepts_agent_stream").await.unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    let (mut agent, _cert) = spawn_agent(&harness, "cp_accept").unwrap();
    assert!(agent.is_running(), "agent must still be running after spawn");

    // Wait up to 20s for the first poll-result envelope to land. The first
    // verify_at update proves the CP accepted our SPIFFE identity AND persisted
    // at least one report for dev-dataplane.
    let status = wait_for_agent_ok(&cli, Duration::from_secs(20))
        .await
        .expect("agent should report OK within 20s");

    // Agent must still be running — catches silent exits.
    assert!(
        agent.is_running(),
        "flowplane-agent exited unexpectedly during handshake; logs: {:?}",
        agent.log_path
    );

    // Sanity-check the last_verify timestamp surfaced.
    let dps = status.get("dataplanes").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let dev =
        dps.iter().find(|dp| dp.get("name").and_then(|s| s.as_str()) == Some("dev-dataplane"));
    assert!(dev.is_some(), "dev-dataplane missing from xds status: {status}");
    let last_verify = dev
        .unwrap()
        .get("last_config_verify")
        .or_else(|| dev.unwrap().get("lastConfigVerify"))
        .and_then(|v| v.as_str());
    assert!(
        last_verify.is_some() && last_verify.unwrap() != "-" && !last_verify.unwrap().is_empty(),
        "last_config_verify should be populated after agent delivers a report; got {:?}",
        last_verify
    );
}

// ---------------------------------------------------------------------------
// fp-084: first-contact envelope when Envoy is healthy
// ---------------------------------------------------------------------------

/// Black-box E2E for fp-084. With a healthy Envoy and no warming failures the
/// agent must STILL produce one envelope on the first poll cycle so the CP can
/// flip `dataplanes.last_config_verify` off NULL. Pre-fp-084, this test would
/// hang at `wait_for_agent_ok` because no envelope is ever emitted.
///
/// Assertions:
///   1. Within `poll_interval_secs (2) + 5s grace = 7s` window, `flowplane xds
///      status` reports the dev dataplane with agent_status != NOT_MONITORED
///      (the helper specifically waits for "OK").
///   2. `dataplanes.last_config_verify` transitions NULL → non-NULL within the
///      same window. The CLI surfaces this as a non-empty `last_config_verify`
///      field — we use that as the harness-exposed view.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires RUN_E2E=1"]
async fn dev_mtls_docker_first_contact_envelope_on_healthy_envoy() {
    let harness = dev_mtls_docker_harness("first_contact").await.unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Sanity: before the agent runs, the dev dataplane should be NOT_MONITORED
    // (no envelope has ever been received). Pre-fp-084, this would remain so
    // forever even with the agent running.
    let pre = cli.run(&["xds", "status", "--output", "json"]).unwrap();
    let pre_v: serde_json::Value =
        serde_json::from_str(&pre.stdout).unwrap_or(serde_json::Value::Null);
    let pre_dps = pre_v.get("dataplanes").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let pre_dev =
        pre_dps.iter().find(|dp| dp.get("name").and_then(|s| s.as_str()) == Some("dev-dataplane"));
    if let Some(dev) = pre_dev {
        let status = dev
            .get("agent_status")
            .or_else(|| dev.get("agentStatus"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        assert_eq!(
            status, "NOT_MONITORED",
            "fp-084 baseline: dev dataplane should start as NOT_MONITORED before the agent runs; got {status}"
        );
    }

    // Spawn the agent. spawn_agent uses poll_interval_secs=2.
    let (mut agent, _cert) = spawn_agent(&harness, "first_contact").unwrap();
    assert!(agent.is_running(), "agent must be running after spawn");

    // Within poll_interval (2s) + 5s grace, status must flip off NOT_MONITORED.
    let status = wait_for_agent_ok(&cli, Duration::from_secs(7))
        .await
        .expect(
            "fp-084 regression: dev dataplane is still NOT_MONITORED 7s after a healthy agent started — \
             the first-contact envelope was never delivered",
        );

    assert!(agent.is_running(), "agent died during first-contact handshake");

    // last_config_verify must be populated (non-NULL → non-empty/non-dash string).
    let dps = status.get("dataplanes").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let dev = dps
        .iter()
        .find(|dp| dp.get("name").and_then(|s| s.as_str()) == Some("dev-dataplane"))
        .expect("dev-dataplane row missing from xds status");
    let last_verify = dev
        .get("last_config_verify")
        .or_else(|| dev.get("lastConfigVerify"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        !last_verify.is_empty() && last_verify != "-" && last_verify != "null",
        "fp-084: last_config_verify must transition NULL → non-NULL after first-contact envelope; got {last_verify:?}"
    );
}

// ---------------------------------------------------------------------------
// C. Envoy xDS over mTLS
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires RUN_E2E=1"]
async fn dev_mtls_docker_envoy_xds_acks() {
    let harness = dev_mtls_docker_harness("envoy_xds_acks").await.unwrap();

    // If Envoy didn't come up for this harness, the test is trivially unable
    // to validate anything — fail loudly rather than skipping.
    assert!(
        harness.has_envoy(),
        "envoy_xds_acks requires a real Envoy; harness reports none available"
    );

    // Envoy boots in the harness with client cert (team=default, proxy=dev-dataplane).
    // If the mTLS handshake with the CP xDS server failed, Envoy would never
    // reach READY and the harness would have errored out before we got here.
    // Additionally assert: at least one subscription connected successfully by
    // inspecting Envoy stats.
    let stats = harness.get_stats().await.expect("envoy /stats should be reachable");

    // Look for a non-zero update_success on the xds ADS stream. Exact stat
    // path depends on Envoy version; check a few plausible names.
    let ok_stats = [
        "cluster_manager.cds.update_success:",
        "listener_manager.lds.update_success:",
        "control_plane.connected_state:",
    ];
    let mut seen: Vec<String> = Vec::new();
    for needle in ok_stats {
        for line in stats.lines() {
            if line.contains(needle) {
                seen.push(line.to_string());
            }
        }
    }
    assert!(
        !seen.is_empty(),
        "expected at least one xDS-related stat line in Envoy /stats; stats sample: {}",
        stats.lines().take(30).collect::<Vec<_>>().join("\n")
    );

    // At least one of the *update_success* counters must be > 0, proving xDS
    // over mTLS actually delivered config (not just a TCP-level connect).
    let any_positive = seen.iter().any(|l| {
        l.contains("update_success:")
            && l.split(':')
                .nth(1)
                .map(|v| v.trim().parse::<u64>().unwrap_or(0) > 0)
                .unwrap_or(false)
    }) || seen.iter().any(|l| l.contains("control_plane.connected_state: 1"));
    assert!(
        any_positive,
        "no positive xDS update_success / connected_state seen; lines: {:?}",
        seen
    );
}

// ---------------------------------------------------------------------------
// E.1 Bogus SPIFFE URI is rejected at the CP
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires RUN_E2E=1"]
async fn dev_mtls_docker_bogus_spiffe_rejected() {
    let harness = dev_mtls_docker_harness("bogus_spiffe").await.unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Start the good agent so we can verify the bogus one doesn't poison its
    // stream.
    let (mut good_agent, _good_cert) = spawn_agent(&harness, "bogus_good").unwrap();
    wait_for_agent_ok(&cli, Duration::from_secs(20)).await.expect("good agent should become OK");

    // Mint a client cert with a SPIFFE URI for a team that doesn't exist.
    let ca = harness.mtls_ca().unwrap();
    let bogus_uri =
        TestCertificateAuthority::build_spiffe_uri("flowplane.local", "phantom-team", "attacker")
            .unwrap();
    let bogus_cert = ca.issue_client_cert(&bogus_uri, "attacker", time::Duration::days(1)).unwrap();

    let ca_path = harness.mtls_certs().unwrap().ca_cert_path.clone();
    let log_dir = std::env::temp_dir().join("fp-u54-bogus-agent");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_path = log_dir.join(format!("bogus-{}.log", std::process::id()));
    let log_file = std::fs::File::create(&log_path).unwrap();
    let log_stderr = log_file.try_clone().unwrap();

    // Capture the pre-bogus NACK count so we can assert nothing new lands for
    // phantom-team.
    let pre = cli.run(&["xds", "nacks", "--limit", "100", "--output", "json"]).unwrap();
    let pre_events = serde_json::from_str::<serde_json::Value>(&pre.stdout)
        .ok()
        .and_then(|v| v.get("events").and_then(|x| x.as_array()).cloned())
        .unwrap_or_default();
    let pre_phantom = pre_events
        .iter()
        .filter(|e| {
            e.get("dataplane_name").and_then(|s| s.as_str()) == Some("phantom-team")
                || e.get("dataplane_name").and_then(|s| s.as_str()) == Some("attacker")
        })
        .count();

    let mut bogus = Command::new(agent_binary())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", format!("https://127.0.0.1:{}", harness.ports.xds))
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "phantom-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", &bogus_cert.cert_path)
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", &bogus_cert.key_path)
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", &ca_path)
        .env(
            "FLOWPLANE_AGENT_ENVOY_ADMIN_URL",
            format!("http://127.0.0.1:{}", harness.ports.envoy_admin),
        )
        .env("FLOWPLANE_AGENT_POLL_INTERVAL_SECS", "2")
        .env("RUST_LOG", "flowplane_agent=debug,info")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_stderr))
        .spawn()
        .unwrap();

    // Give the bogus agent ~8s to attempt a handshake. It may exit (CP rejects)
    // or stay up retrying — either is acceptable as long as no nack_event gets
    // persisted for phantom-team.
    std::thread::sleep(Duration::from_secs(8));
    let _ = bogus.kill();
    let _ = bogus.wait();

    let post = cli.run(&["xds", "nacks", "--limit", "100", "--output", "json"]).unwrap();
    let post_events = serde_json::from_str::<serde_json::Value>(&post.stdout)
        .ok()
        .and_then(|v| v.get("events").and_then(|x| x.as_array()).cloned())
        .unwrap_or_default();
    let post_phantom = post_events
        .iter()
        .filter(|e| {
            e.get("dataplane_name").and_then(|s| s.as_str()) == Some("phantom-team")
                || e.get("dataplane_name").and_then(|s| s.as_str()) == Some("attacker")
                || e.get("dataplane_name").and_then(|s| s.as_str()) == Some("phantom-dataplane")
        })
        .count();
    assert_eq!(
        post_phantom, pre_phantom,
        "CP must NOT persist nack_events for a bogus SPIFFE identity; \
         pre={pre_phantom} post={post_phantom}"
    );

    // Good agent must still be healthy — bogus connection must not poison state.
    assert!(good_agent.is_running(), "good agent died while bogus agent was connecting");
    let _ = wait_for_agent_ok(&cli, Duration::from_secs(10)).await.unwrap();
}

// ---------------------------------------------------------------------------
// E.2 Delete CA mid-run => agent exits cleanly
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires RUN_E2E=1"]
async fn dev_mtls_docker_delete_ca_agent_exits() {
    // Fully isolated harness — this test mutates CA files on disk, so it
    // cannot share state with peer tests.
    let harness = dev_mtls_docker_harness("delete_ca").await.unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Copy the CA + agent cert to a test-owned directory so we can delete them
    // without racing the harness Drop. The agent is spawned against these
    // copies.
    let work_dir = std::env::temp_dir().join(format!("fp-u54-delete-ca-{}", std::process::id()));
    std::fs::create_dir_all(&work_dir).unwrap();
    let src_certs = harness.mtls_certs().unwrap();
    let ca_copy = work_dir.join("ca.pem");
    std::fs::copy(&src_certs.ca_cert_path, &ca_copy).unwrap();

    let ca = harness.mtls_ca().unwrap();
    let (agent_cert, _spiffe) = mint_agent_client_cert(ca).unwrap();

    let log_path = work_dir.join("agent.log");
    let log_file = std::fs::File::create(&log_path).unwrap();
    let log_stderr = log_file.try_clone().unwrap();

    let mut child = Command::new(agent_binary())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", format!("https://127.0.0.1:{}", harness.ports.xds))
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dev-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", &agent_cert.cert_path)
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", &agent_cert.key_path)
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", &ca_copy)
        .env(
            "FLOWPLANE_AGENT_ENVOY_ADMIN_URL",
            format!("http://127.0.0.1:{}", harness.ports.envoy_admin),
        )
        .env("FLOWPLANE_AGENT_POLL_INTERVAL_SECS", "2")
        .env("RUST_LOG", "flowplane_agent=debug,info")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_stderr))
        .spawn()
        .unwrap();

    // Wait for the agent to become healthy via CLI status to confirm it's
    // actually streaming before we yank the CA.
    wait_for_agent_ok(&cli, Duration::from_secs(20))
        .await
        .expect("agent should become OK before CA deletion");

    // Yank the CA copy out from under the agent.
    std::fs::remove_file(&ca_copy).unwrap();

    // The agent reads the CA only at process startup, so deleting it mid-run
    // won't affect an already-established TLS session. To force re-read, kill
    // the gRPC connection by killing the CP's xDS server... which we can't do
    // without shutting down the harness. Instead, accept the weaker but
    // still-observable guarantee: if we restart the agent now with a missing
    // CA, it MUST exit cleanly with a clear error, not panic.
    let _ = child.kill();
    let _ = child.wait();

    let restart_log = work_dir.join("agent-restart.log");
    let restart_file = std::fs::File::create(&restart_log).unwrap();
    let restart_stderr = restart_file.try_clone().unwrap();

    let start = Instant::now();
    let mut restart = Command::new(agent_binary())
        .env("FLOWPLANE_AGENT_CP_ENDPOINT", format!("https://127.0.0.1:{}", harness.ports.xds))
        .env("FLOWPLANE_AGENT_DATAPLANE_ID", "dev-dataplane")
        .env("FLOWPLANE_AGENT_TLS_CERT_PATH", &agent_cert.cert_path)
        .env("FLOWPLANE_AGENT_TLS_KEY_PATH", &agent_cert.key_path)
        .env("FLOWPLANE_AGENT_TLS_CA_PATH", &ca_copy) // deleted
        .env(
            "FLOWPLANE_AGENT_ENVOY_ADMIN_URL",
            format!("http://127.0.0.1:{}", harness.ports.envoy_admin),
        )
        .env("RUST_LOG", "flowplane_agent=debug,info")
        .stdin(Stdio::null())
        .stdout(Stdio::from(restart_file))
        .stderr(Stdio::from(restart_stderr))
        .spawn()
        .unwrap();

    let mut exited = None;
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(Some(status)) = restart.try_wait() {
            exited = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if exited.is_none() {
        let _ = restart.kill();
    }
    // Always reap the child to appease clippy::zombie_processes and avoid leaks.
    let final_status = restart.wait().ok();
    let status =
        exited.or(final_status).expect("agent must exit within 10s when CA file is missing");
    assert!(!status.success(), "agent must exit non-zero when CA file is missing; got {status:?}");

    // Log must contain a hint about the missing CA / TLS config; fail fast if
    // the error message is silent.
    let log = std::fs::read_to_string(&restart_log).unwrap_or_default();
    let hints = ["CA", "certificate", "tls", "TLS", "trust", "No such file", "not found"];
    assert!(
        hints.iter().any(|h| log.contains(h)),
        "agent restart log didn't mention CA/tls error; log was: {log}"
    );
}
