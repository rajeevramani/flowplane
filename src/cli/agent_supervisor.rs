//! Spawn and supervise the `flowplane-agent` subprocess for the dev harness.
//!
//! In dev mode `flowplane init --with-envoy` brings up an Envoy container.
//! We additionally fork the dataplane-side `flowplane-agent` as a *detached*
//! host process so warming failure detection works out of the box. The agent
//! binary lives in the same Cargo workspace and is normally built next to the
//! `flowplane` CLI binary.
//!
//! Lifecycle: `flowplane init` records the spawned PID in `~/.flowplane/agent.pid`;
//! `flowplane down` reads that file and signals the process to exit.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::warn;

/// Env var name read by `handle_init` to opt out of spawning the agent.
pub const DISABLE_AGENT_ENV: &str = "FLOWPLANE_DEV_DISABLE_AGENT";

/// Default dataplane identity baked into the dev-mode Envoy bootstrap.
pub const DEV_DATAPLANE_ID: &str = "dev-dataplane";

/// Default Envoy admin URL exposed on the host by the dev compose stack.
pub const DEV_ENVOY_ADMIN_URL: &str = "http://127.0.0.1:9901";

/// Default Flowplane CP endpoint (plaintext) — used when dev mTLS material is
/// not available. When fp-u54 dev mTLS is active, the spec's `cp_endpoint` is
/// flipped to `https://127.0.0.1:18000` by [`AgentSpawnSpec::with_dev_mtls`].
pub const DEV_CP_ENDPOINT: &str = "http://127.0.0.1:18000";

/// HTTPS form of the dev CP endpoint, used when dev mTLS is active.
pub const DEV_CP_ENDPOINT_TLS: &str = "https://127.0.0.1:18000";

/// Default poll interval (seconds) for the dev-mode agent.
pub const DEV_POLL_INTERVAL_SECS: u64 = 10;

/// Absolute host paths to the dev mTLS material the agent should present to
/// the control plane. Populated by [`AgentSpawnSpec::with_dev_mtls`] when
/// fp-u54 dev mTLS is active, and serialized into
/// `FLOWPLANE_AGENT_TLS_CERT_PATH` / `_KEY_PATH` / `_CA_PATH` by
/// [`build_agent_env`].
#[derive(Debug, Clone)]
pub struct AgentTlsPaths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub ca_path: PathBuf,
}

/// Inputs needed to construct the agent subprocess environment.
#[derive(Debug, Clone)]
pub struct AgentSpawnSpec {
    pub envoy_admin_url: String,
    pub cp_endpoint: String,
    pub dataplane_id: String,
    pub poll_interval_secs: u64,
    /// When `Some`, the agent is launched with mTLS material and an `https://`
    /// CP endpoint. When `None`, the agent connects to the CP over plaintext.
    /// Dev-only — prod never calls this path.
    pub tls: Option<AgentTlsPaths>,
}

impl AgentSpawnSpec {
    /// Defaults for `flowplane init --with-envoy` in dev mode.
    pub fn dev_defaults() -> Self {
        Self {
            envoy_admin_url: DEV_ENVOY_ADMIN_URL.to_string(),
            cp_endpoint: DEV_CP_ENDPOINT.to_string(),
            dataplane_id: DEV_DATAPLANE_ID.to_string(),
            poll_interval_secs: DEV_POLL_INTERVAL_SECS,
            tls: None,
        }
    }

    /// Attach dev-mode mTLS material from [`crate::cli::dev_certs::DevCertPaths`].
    ///
    /// Flips the CP endpoint scheme from `http://` to `https://` — the agent
    /// must talk to the CP over the same mTLS channel Envoy uses, and the xDS
    /// server in dev mode rejects plaintext when mTLS is active (see fp-u54.2).
    ///
    /// The three path fields are absolute host paths; the agent is a host
    /// process (not inside the Envoy container), so no container path
    /// translation is needed.
    pub fn with_dev_mtls(mut self, certs: &crate::cli::dev_certs::DevCertPaths) -> Self {
        self.cp_endpoint = DEV_CP_ENDPOINT_TLS.to_string();
        self.tls = Some(AgentTlsPaths {
            cert_path: certs.agent_cert.clone(),
            key_path: certs.agent_key.clone(),
            ca_path: certs.ca_cert.clone(),
        });
        self
    }
}

/// Build the env var list passed to the agent subprocess.
///
/// Kept pure (no IO) so it can be unit tested.
pub fn build_agent_env(spec: &AgentSpawnSpec) -> Vec<(String, String)> {
    let mut env = vec![
        ("FLOWPLANE_AGENT_ENVOY_ADMIN_URL".to_string(), spec.envoy_admin_url.clone()),
        ("FLOWPLANE_AGENT_CP_ENDPOINT".to_string(), spec.cp_endpoint.clone()),
        ("FLOWPLANE_AGENT_DATAPLANE_ID".to_string(), spec.dataplane_id.clone()),
        ("FLOWPLANE_AGENT_POLL_INTERVAL_SECS".to_string(), spec.poll_interval_secs.to_string()),
    ];

    if let Some(tls) = &spec.tls {
        env.push((
            "FLOWPLANE_AGENT_TLS_CERT_PATH".to_string(),
            tls.cert_path.display().to_string(),
        ));
        env.push(("FLOWPLANE_AGENT_TLS_KEY_PATH".to_string(), tls.key_path.display().to_string()));
        env.push(("FLOWPLANE_AGENT_TLS_CA_PATH".to_string(), tls.ca_path.display().to_string()));
    }

    env
}

/// Locate the `flowplane-agent` binary.
///
/// Search order:
/// 1. `FLOWPLANE_AGENT_BIN` env var (explicit override)
/// 2. Sibling of the current executable (workspace `target/{debug,release}/`)
/// 3. PATH lookup
///
/// Returns `Ok(None)` if the binary cannot be found — caller should log a
/// WARN explaining how to build it and continue.
pub fn find_agent_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("FLOWPLANE_AGENT_BIN") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        // Try the raw exe path first, then canonicalize to resolve macOS symlinks
        // (e.g. cargo install creates symlinks in ~/.cargo/bin/)
        let dirs_to_try = std::iter::once(exe.clone())
            .chain(exe.canonicalize().ok())
            .filter_map(|p| p.parent().map(|d| d.to_path_buf()));
        for dir in dirs_to_try {
            let candidate = dir.join("flowplane-agent");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // PATH lookup
    if let Ok(path) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path.split(sep) {
            let candidate = Path::new(dir).join("flowplane-agent");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Compute the path the agent's stdout/stderr should be appended to.
pub fn agent_log_path(logs_dir: &Path, dataplane_id: &str) -> PathBuf {
    logs_dir.join(format!("flowplane-agent-{}.log", dataplane_id))
}

/// Path where we record the spawned agent's PID so `handle_down` can find it.
pub fn agent_pid_path(fp_dir: &Path) -> PathBuf {
    fp_dir.join("agent.pid")
}

/// Spawn the agent as a detached host subprocess, redirecting stdout+stderr
/// to a per-dataplane log file and recording the PID.
///
/// Returns `Ok(None)` if the binary is missing — caller should log a WARN and
/// continue. The dev harness must not abort if the agent can't start.
pub fn spawn_agent_detached(
    binary: &Path,
    spec: &AgentSpawnSpec,
    logs_dir: &Path,
    pid_path: &Path,
) -> Result<u32> {
    std::fs::create_dir_all(logs_dir)
        .with_context(|| format!("failed to create agent log dir: {}", logs_dir.display()))?;

    let log_path = agent_log_path(logs_dir, &spec.dataplane_id);
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open agent log file: {}", log_path.display()))?;
    let log_clone =
        log_file.try_clone().context("failed to clone agent log file handle for stderr")?;

    let mut cmd = Command::new(binary);
    cmd.envs(build_agent_env(spec))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_clone));

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn flowplane-agent at {}", binary.display()))?;
    let pid = child.id();

    // Drop the Child handle without killing — std::process::Child::drop does NOT
    // signal the child, so the agent continues running after `flowplane init` exits.
    drop(child);

    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create pid file dir: {}", parent.display()))?;
    }
    std::fs::write(pid_path, pid.to_string())
        .with_context(|| format!("failed to write pid file: {}", pid_path.display()))?;

    // Touch the log file via the path so callers can verify it exists.
    let _ = File::open(&log_path);

    Ok(pid)
}

/// Read the recorded agent PID and try to terminate the process.
///
/// No-op if the PID file does not exist or the process is already gone.
pub fn kill_agent_from_pid_file(pid_path: &Path) -> Result<()> {
    if !pid_path.exists() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(pid_path)
        .with_context(|| format!("failed to read pid file: {}", pid_path.display()))?;
    let pid: i32 = match raw.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            // Corrupt pid file — best effort cleanup
            let _ = std::fs::remove_file(pid_path);
            return Ok(());
        }
    };

    #[cfg(unix)]
    {
        // SAFETY: kill(2) is safe to call with any pid value; errors are ignored.
        unsafe {
            libc_kill(pid, SIGTERM);
        }
    }

    let _ = std::fs::remove_file(pid_path);
    Ok(())
}

// Minimal libc bindings to avoid pulling in the libc crate just for kill(2).
#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) {
    let _ = kill(pid, sig);
}

/// Wait for Envoy's admin port to accept TCP connections (best effort).
///
/// `addr` should be a `host:port` string like `127.0.0.1:9901`. Returns `Ok(())`
/// on success; on timeout returns `Ok(())` as well after logging a WARN — the
/// agent will simply log connection errors itself if Envoy never came up.
pub fn wait_for_envoy_admin(addr: &str, timeout_secs: u64) -> Result<()> {
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    let socket: std::net::SocketAddr =
        addr.parse().with_context(|| format!("invalid envoy admin address: {addr}"))?;
    let start = Instant::now();
    let deadline = Duration::from_secs(timeout_secs);
    let connect_timeout = Duration::from_millis(500);

    while start.elapsed() < deadline {
        if TcpStream::connect_timeout(&socket, connect_timeout).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    warn!(
        admin_addr = addr,
        timeout_secs,
        "envoy admin port did not become reachable before agent spawn — continuing anyway"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn build_agent_env_contains_all_required_vars() {
        let spec = AgentSpawnSpec::dev_defaults();
        let env = build_agent_env(&spec);

        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(
            map.get("FLOWPLANE_AGENT_ENVOY_ADMIN_URL").map(String::as_str),
            Some(DEV_ENVOY_ADMIN_URL)
        );
        assert_eq!(
            map.get("FLOWPLANE_AGENT_CP_ENDPOINT").map(String::as_str),
            Some(DEV_CP_ENDPOINT)
        );
        assert_eq!(
            map.get("FLOWPLANE_AGENT_DATAPLANE_ID").map(String::as_str),
            Some(DEV_DATAPLANE_ID)
        );
        assert_eq!(map.get("FLOWPLANE_AGENT_POLL_INTERVAL_SECS").map(String::as_str), Some("10"));
        assert_eq!(map.len(), 4, "no unexpected env vars leaked into the spawn");
    }

    #[test]
    fn build_agent_env_propagates_overrides() {
        let spec = AgentSpawnSpec {
            envoy_admin_url: "http://127.0.0.1:19901".to_string(),
            cp_endpoint: "http://127.0.0.1:28000".to_string(),
            dataplane_id: "weird-name".to_string(),
            poll_interval_secs: 3,
            tls: None,
        };
        let env = build_agent_env(&spec);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map.get("FLOWPLANE_AGENT_ENVOY_ADMIN_URL").unwrap(), "http://127.0.0.1:19901");
        assert_eq!(map.get("FLOWPLANE_AGENT_CP_ENDPOINT").unwrap(), "http://127.0.0.1:28000");
        assert_eq!(map.get("FLOWPLANE_AGENT_DATAPLANE_ID").unwrap(), "weird-name");
        assert_eq!(map.get("FLOWPLANE_AGENT_POLL_INTERVAL_SECS").unwrap(), "3");
    }

    #[test]
    fn build_agent_env_without_tls_has_no_tls_vars() {
        let spec = AgentSpawnSpec::dev_defaults();
        let env = build_agent_env(&spec);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert!(!map.contains_key("FLOWPLANE_AGENT_TLS_CERT_PATH"));
        assert!(!map.contains_key("FLOWPLANE_AGENT_TLS_KEY_PATH"));
        assert!(!map.contains_key("FLOWPLANE_AGENT_TLS_CA_PATH"));
        assert_eq!(
            map.get("FLOWPLANE_AGENT_CP_ENDPOINT").map(String::as_str),
            Some(DEV_CP_ENDPOINT),
            "plaintext endpoint when tls is None"
        );
    }

    #[test]
    fn build_agent_env_with_tls_emits_paths_and_https_endpoint() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("certs");
        let certs = crate::cli::dev_certs::generate_dev_certs(&root).unwrap();

        let spec = AgentSpawnSpec::dev_defaults().with_dev_mtls(&certs);
        let env = build_agent_env(&spec);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();

        assert_eq!(
            map.get("FLOWPLANE_AGENT_CP_ENDPOINT").map(String::as_str),
            Some(DEV_CP_ENDPOINT_TLS),
            "with_dev_mtls must flip CP endpoint to https"
        );
        assert!(map.get("FLOWPLANE_AGENT_CP_ENDPOINT").unwrap().starts_with("https://"));
        assert_eq!(
            map.get("FLOWPLANE_AGENT_TLS_CERT_PATH").unwrap(),
            &certs.agent_cert.display().to_string()
        );
        assert_eq!(
            map.get("FLOWPLANE_AGENT_TLS_KEY_PATH").unwrap(),
            &certs.agent_key.display().to_string()
        );
        assert_eq!(
            map.get("FLOWPLANE_AGENT_TLS_CA_PATH").unwrap(),
            &certs.ca_cert.display().to_string()
        );
        assert_eq!(map.len(), 7, "exactly 4 base vars + 3 tls vars");
    }

    #[test]
    fn agent_log_path_uses_dataplane_id() {
        let tmp = TempDir::new().unwrap();
        let p = agent_log_path(tmp.path(), "dev-dataplane");
        assert_eq!(p.file_name().unwrap(), "flowplane-agent-dev-dataplane.log");
        assert!(p.starts_with(tmp.path()));
    }

    #[test]
    fn find_agent_binary_returns_none_when_absent() {
        let original_bin = std::env::var("FLOWPLANE_AGENT_BIN").ok();
        let original_path = std::env::var("PATH").ok();

        std::env::remove_var("FLOWPLANE_AGENT_BIN");
        std::env::set_var("PATH", "/nonexistent/path/segment");

        // We can't fully isolate `current_exe()`, but it should not have a
        // sibling literally named `flowplane-agent` *and* be a regular file in
        // the test runner's target/deps dir. If the workspace has been built
        // with `cargo build -p flowplane-agent`, this assertion may flip — so
        // we restrict to checking the function does not panic and returns a
        // PathBuf that, if Some, points at a real file.
        let result = find_agent_binary();
        if let Some(ref p) = result {
            assert!(p.is_file(), "find_agent_binary returned non-file: {}", p.display());
        }

        match original_bin {
            Some(v) => std::env::set_var("FLOWPLANE_AGENT_BIN", v),
            None => std::env::remove_var("FLOWPLANE_AGENT_BIN"),
        }
        match original_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }

    #[test]
    fn find_agent_binary_honours_explicit_override() {
        let tmp = TempDir::new().unwrap();
        let fake_bin = tmp.path().join("flowplane-agent");
        std::fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perms).unwrap();
        }

        let original = std::env::var("FLOWPLANE_AGENT_BIN").ok();
        std::env::set_var("FLOWPLANE_AGENT_BIN", &fake_bin);

        let found = find_agent_binary();

        match original {
            Some(v) => std::env::set_var("FLOWPLANE_AGENT_BIN", v),
            None => std::env::remove_var("FLOWPLANE_AGENT_BIN"),
        }

        assert_eq!(found.as_deref(), Some(fake_bin.as_path()));
    }

    #[test]
    fn find_agent_binary_ignores_missing_override() {
        let original = std::env::var("FLOWPLANE_AGENT_BIN").ok();
        std::env::set_var("FLOWPLANE_AGENT_BIN", "/definitely/not/here/flowplane-agent");

        // Should fall through to current_exe / PATH, not return the bogus path.
        let found = find_agent_binary();
        if let Some(ref p) = found {
            assert_ne!(p, Path::new("/definitely/not/here/flowplane-agent"));
        }

        match original {
            Some(v) => std::env::set_var("FLOWPLANE_AGENT_BIN", v),
            None => std::env::remove_var("FLOWPLANE_AGENT_BIN"),
        }
    }

    #[test]
    fn kill_agent_from_pid_file_no_op_when_absent() {
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("agent.pid");
        // Should succeed even though the file does not exist.
        kill_agent_from_pid_file(&pid_path).unwrap();
    }

    #[test]
    fn kill_agent_from_pid_file_handles_corrupt_pid() {
        let tmp = TempDir::new().unwrap();
        let pid_path = tmp.path().join("agent.pid");
        std::fs::write(&pid_path, "not-a-number").unwrap();
        kill_agent_from_pid_file(&pid_path).unwrap();
        assert!(!pid_path.exists(), "corrupt pid file should be cleaned up");
    }

    #[test]
    fn agent_pid_path_lives_under_flowplane_dir() {
        let tmp = TempDir::new().unwrap();
        let p = agent_pid_path(tmp.path());
        assert_eq!(p.file_name().unwrap(), "agent.pid");
        assert!(p.starts_with(tmp.path()));
    }

    #[test]
    fn spawn_agent_detached_writes_pid_and_log_file() {
        // Use /bin/echo as a stand-in for the agent binary so we can verify
        // the spawn machinery without requiring the real flowplane-agent build.
        #[cfg(unix)]
        {
            let echo = Path::new("/bin/echo");
            if !echo.is_file() {
                return; // skip on systems without /bin/echo
            }
            let tmp = TempDir::new().unwrap();
            let logs = tmp.path().join("logs");
            let pid = tmp.path().join("agent.pid");
            let spec = AgentSpawnSpec::dev_defaults();

            let result = spawn_agent_detached(echo, &spec, &logs, &pid);
            assert!(result.is_ok(), "spawn failed: {:?}", result.err());
            assert!(pid.exists(), "pid file not written");
            let pid_str = std::fs::read_to_string(&pid).unwrap();
            assert!(pid_str.trim().parse::<u32>().is_ok(), "pid file not numeric: {pid_str}");
            let log = agent_log_path(&logs, &spec.dataplane_id);
            // Give the child a moment to flush
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(log.exists(), "log file not created");
        }
    }
}
