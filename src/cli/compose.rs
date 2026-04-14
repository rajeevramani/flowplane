//! Compose wrapper for `flowplane init` and `flowplane down`.
//!
//! Embeds the dev-mode `docker-compose-dev.yml` via `include_str!` and writes
//! it to a temporary directory. Container runtime (Docker/Podman) is detected
//! automatically.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

use super::agent_supervisor::{
    agent_log_path, agent_pid_path, find_agent_binary, kill_agent_from_pid_file,
    spawn_agent_detached, wait_for_envoy_admin, AgentSpawnSpec, DEV_DATAPLANE_ID,
    DISABLE_AGENT_ENV,
};
use super::dev_certs::{generate_dev_certs, DevCertPaths};

/// Embedded dev-mode compose file (checked into the repo root).
const DEV_COMPOSE_YAML: &str = include_str!("../../docker-compose-dev.yml");

/// Detect whether `docker` or `podman` is available.
///
/// H15 priority order:
/// 1. `DOCKER_HOST` env var (explicit override — could be tcp:// or unix://)
/// 2. `/var/run/docker.sock` (standard Docker)
/// 3. `~/.orbstack/run/docker.sock` (OrbStack on macOS)
/// 4. `~/.rd/run/docker.sock` (Rancher Desktop)
/// 5. `$XDG_RUNTIME_DIR/podman/podman.sock` (rootless Podman)
/// 6. Fallback: `docker` or `podman` on PATH
pub(crate) fn detect_runtime() -> Result<String> {
    // 1. DOCKER_HOST env var — explicit override
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if !host.is_empty() {
            // DOCKER_HOST is set — use docker (or podman if it's a podman socket)
            if host.contains("podman") {
                return Ok("podman".to_string());
            }
            return Ok("docker".to_string());
        }
    }

    // 2-4. Well-known Docker sockets (standard, OrbStack, Rancher Desktop)
    let home = std::env::var("HOME").unwrap_or_default();
    let docker_sockets: Vec<PathBuf> = vec![
        PathBuf::from("/var/run/docker.sock"),
        PathBuf::from(&home).join(".orbstack/run/docker.sock"),
        PathBuf::from(&home).join(".rd/run/docker.sock"),
    ];

    for sock in &docker_sockets {
        if sock.exists() {
            // Socket exists but verify the binary is actually on PATH
            // (Podman creates a Docker-compat socket but `docker` may be a shell alias)
            if command_exists("docker") {
                return Ok("docker".to_string());
            }
            if command_exists("podman") {
                return Ok("podman".to_string());
            }
        }
    }

    // 5. XDG_RUNTIME_DIR podman socket (rootless Podman)
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let podman_sock = PathBuf::from(&xdg).join("podman/podman.sock");
        if podman_sock.exists() && command_exists("podman") {
            return Ok("podman".to_string());
        }
    }

    // 6. Fallback: check PATH
    if command_exists("podman") {
        return Ok("podman".to_string());
    }
    if command_exists("docker") {
        return Ok("docker".to_string());
    }

    anyhow::bail!(
        "No container runtime found. Install Docker or Podman and ensure the daemon is running."
    )
}

/// Returns true if `name` is found on PATH and exits successfully with `--version`.
fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Resolve the project source directory for the Docker build context.
///
/// Priority:
/// 1. `FLOWPLANE_SOURCE_DIR` env var (explicit override)
/// 2. Directory containing the current executable (works for `cargo run`)
/// 3. Current working directory (last resort)
fn resolve_source_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("FLOWPLANE_SOURCE_DIR") {
        let p = PathBuf::from(&dir);
        if p.join("Cargo.toml").exists() {
            return Ok(p);
        }
        anyhow::bail!(
            "FLOWPLANE_SOURCE_DIR={dir} does not contain Cargo.toml — \
             point it at the Flowplane repo root"
        );
    }

    // Try the directory of the current executable
    if let Ok(exe) = std::env::current_exe() {
        // Walk up from target/debug or target/release to the repo root
        let mut candidate = exe.as_path();
        for _ in 0..5 {
            if let Some(parent) = candidate.parent() {
                candidate = parent;
                if candidate.join("Cargo.toml").exists() {
                    return Ok(candidate.to_path_buf());
                }
            }
        }
    }

    // Fallback: cwd
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    if cwd.join("Cargo.toml").exists() {
        return Ok(cwd);
    }

    anyhow::bail!(
        "Cannot locate Flowplane source directory. Set FLOWPLANE_SOURCE_DIR to the repo root."
    )
}

/// Get the home directory (HOME on Unix, USERPROFILE on Windows).
fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .context("Unable to determine home directory")
}

/// Loopback guard: fail fast if we detect we are running inside the
/// container we are about to launch.
pub fn loopback_guard() -> Result<()> {
    // Inside Docker/Podman containers, /.dockerenv exists or cgroup contains "docker"/"podman"
    if Path::new("/.dockerenv").exists() {
        anyhow::bail!(
            "Loopback detected: `flowplane init` cannot be run inside a container. \
             Run it from your host machine."
        );
    }
    Ok(())
}

/// Get or generate a dev-mode secret encryption key.
///
/// On first run, generates a random 32-byte key and saves it to
/// `~/.flowplane/encryption.key`. On subsequent runs, reuses the
/// existing key so secrets survive restarts.
fn get_or_create_dev_encryption_key(fp_dir: &Path) -> Result<String> {
    use base64::Engine;
    use rand::RngCore;

    let key_path = fp_dir.join("encryption.key");

    if let Ok(existing) = std::fs::read_to_string(&key_path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);

    std::fs::write(&key_path, &encoded)
        .with_context(|| format!("failed to write encryption key: {}", key_path.display()))?;

    Ok(encoded)
}

/// Write the embedded compose YAML to `~/.flowplane/docker-compose-dev.yml`.
///
/// The file is written with a comment header pointing back to the source and
/// the build context is patched to the resolved source directory.
pub fn write_compose_file(source_dir: &Path) -> Result<PathBuf> {
    let home = home_dir()?;
    let fp_dir = home.join(".flowplane");
    std::fs::create_dir_all(&fp_dir)
        .with_context(|| format!("failed to create directory: {}", fp_dir.display()))?;

    let compose_path = fp_dir.join("docker-compose-dev.yml");

    // Patch the build context to point at the source directory
    let patched = DEV_COMPOSE_YAML
        .replace("      context: .", &format!("      context: {}", source_dir.display()));

    // Get or create a persistent encryption key for dev-mode secrets
    let encryption_key = get_or_create_dev_encryption_key(&fp_dir)?;
    let patched = patched.replace("__FLOWPLANE_DEV_ENCRYPTION_KEY__", &encryption_key);

    // fp-u54.6: replace dev mTLS cert-path tokens with the in-container paths
    // where `~/.flowplane/certs/` is bind-mounted. Same substitution pattern
    // as the encryption key above.
    let patched = patched
        .replace("__FLOWPLANE_XDS_TLS_CERT_PATH__", CP_CERT_PATH_IN_CONTAINER)
        .replace("__FLOWPLANE_XDS_TLS_KEY_PATH__", CP_KEY_PATH_IN_CONTAINER)
        .replace("__FLOWPLANE_XDS_TLS_CLIENT_CA_PATH__", CA_PATH_IN_CONTAINER);

    std::fs::write(&compose_path, patched)
        .with_context(|| format!("failed to write compose file: {}", compose_path.display()))?;

    Ok(compose_path)
}

/// Poll the health endpoint until it responds 200 or we time out.
///
/// Uses a raw TCP connection + minimal HTTP/1.1 request to avoid pulling in
/// an additional HTTP client dependency.
fn wait_for_healthy(timeout_secs: u64) -> Result<()> {
    use std::io::{Read, Write as IoWrite};
    use std::net::TcpStream;

    let addr = "127.0.0.1:8080";
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let connect_timeout = std::time::Duration::from_secs(2);

    eprintln!("Waiting for control plane to become healthy...");

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Control plane did not become healthy within {timeout_secs}s. \
                 Check `docker compose logs control-plane` for errors."
            );
        }

        if let Ok(mut stream) =
            TcpStream::connect_timeout(&addr.parse().expect("valid socket addr"), connect_timeout)
        {
            let _ = stream.set_read_timeout(Some(connect_timeout));
            let _ = stream.set_write_timeout(Some(connect_timeout));

            let request = "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(request.as_bytes()).is_ok() {
                let mut response = String::new();
                if stream.read_to_string(&mut response).is_ok() && response.contains("200") {
                    eprintln!("Control plane is healthy.");
                    return Ok(());
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Container paths where dev-mode mTLS material is mounted into the Envoy
/// container. These live under `/etc/flowplane/certs/` via the bind mount in
/// `docker-compose-dev.yml`. The host path is `~/.flowplane/certs/` (see
/// [`crate::cli::dev_certs`]).
const ENVOY_CERT_PATH_IN_CONTAINER: &str = "/etc/flowplane/certs/envoy/cert.pem";
const ENVOY_KEY_PATH_IN_CONTAINER: &str = "/etc/flowplane/certs/envoy/key.pem";
const CA_PATH_IN_CONTAINER: &str = "/etc/flowplane/certs/ca.pem";

/// Control-plane-side cert paths inside the `control-plane` container.
///
/// These are the values `write_compose_file` substitutes into the
/// `__FLOWPLANE_XDS_TLS_*` tokens in `docker-compose-dev.yml`, which the CP
/// then reads via `FLOWPLANE_XDS_TLS_CERT_PATH` / `_KEY_PATH` /
/// `_CLIENT_CA_PATH` on startup — the same env vars prod uses.
const CP_CERT_PATH_IN_CONTAINER: &str = "/etc/flowplane/certs/cp/cert.pem";
const CP_KEY_PATH_IN_CONTAINER: &str = "/etc/flowplane/certs/cp/key.pem";

/// Build the Envoy bootstrap YAML, optionally injecting an mTLS
/// `transport_socket` on the `xds_cluster`.
///
/// Pure function — no IO, takes `mtls: bool` so it's trivially unit-testable
/// via `serde_yaml::from_str`.
///
/// When `mtls = true` the xds_cluster gets a static `tls_certificates` entry
/// (NOT SDS — fp-u54 is deliberately bootstrap-only) referring to the
/// in-container paths where the host `~/.flowplane/certs/` is bind-mounted.
pub fn build_envoy_bootstrap_yaml(mtls: bool) -> String {
    let transport_socket = if mtls {
        format!(
            "      transport_socket:
        name: envoy.transport_sockets.tls
        typed_config:
          \"@type\": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
          common_tls_context:
            tls_certificates:
              - certificate_chain:
                  filename: {cert}
                private_key:
                  filename: {key}
            validation_context:
              trusted_ca:
                filename: {ca}
",
            cert = ENVOY_CERT_PATH_IN_CONTAINER,
            key = ENVOY_KEY_PATH_IN_CONTAINER,
            ca = CA_PATH_IN_CONTAINER,
        )
    } else {
        String::new()
    };

    format!(
        r#"# Auto-generated by `flowplane init --with-envoy`
# Connects to the Flowplane xDS control plane via ADS (gRPC)
admin:
  access_log_path: /dev/null
  address:
    socket_address:
      address: 0.0.0.0
      port_value: 9901
node:
  cluster: dev-cluster
  id: team=default/dp-dev
  metadata:
    dataplane_name: dev-dataplane
    gateway_host: envoy
    team: default
dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
      - envoy_grpc:
          cluster_name: xds_cluster
  cds_config:
    ads: {{}}
  lds_config:
    ads: {{}}
static_resources:
  clusters:
    - name: xds_cluster
      connect_timeout: 5s
      type: LOGICAL_DNS
      dns_lookup_family: V4_ONLY
      http2_protocol_options: {{}}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: control-plane
                      port_value: 18000
{transport_socket}"#,
        transport_socket = transport_socket,
    )
}

/// Write Envoy bootstrap YAML to `~/.flowplane/envoy/envoy.yaml`.
///
/// When `mtls = true`, the xds_cluster is configured with a static
/// `transport_socket` referencing the container paths where dev certs are
/// bind-mounted.
pub fn write_envoy_bootstrap(mtls: bool) -> Result<PathBuf> {
    let home = home_dir()?;
    let envoy_dir = home.join(".flowplane").join("envoy");
    std::fs::create_dir_all(&envoy_dir)
        .with_context(|| format!("failed to create directory: {}", envoy_dir.display()))?;

    let bootstrap_path = envoy_dir.join("envoy.yaml");
    let bootstrap = build_envoy_bootstrap_yaml(mtls);

    std::fs::write(&bootstrap_path, bootstrap).with_context(|| {
        format!("failed to write envoy bootstrap: {}", bootstrap_path.display())
    })?;

    Ok(bootstrap_path)
}

/// Generate (or refresh) the dev-mode SPIFFE PKI under `~/.flowplane/certs/`.
///
/// Runs unconditionally on every `flowplane init` — even without `--with-envoy`
/// — so the CP's dev mTLS loader has material to find. Cert generation is
/// idempotent: re-running rotates the PKI (see `src/cli/dev_certs.rs`).
pub fn ensure_dev_mtls_material() -> Result<DevCertPaths> {
    let home = home_dir()?;
    let cert_root = home.join(".flowplane").join("certs");
    ensure_dev_mtls_material_at(&cert_root)
}

/// Test-friendly variant that takes an explicit cert root — avoids HOME
/// mutation in unit tests (HOME is process-global and racy under parallel
/// `cargo test`).
pub fn ensure_dev_mtls_material_at(cert_root: &Path) -> Result<DevCertPaths> {
    let paths = generate_dev_certs(cert_root).with_context(|| {
        format!("failed to generate dev mTLS certs under {}", cert_root.display())
    })?;
    info!(cert_root = %cert_root.display(), "generated dev mTLS material");
    Ok(paths)
}

/// `flowplane init` — one-command dev environment bootstrap.
///
/// 1. Loopback guard
/// 2. Detect container runtime
/// 3. Resolve or generate dev token
/// 4. Write compose file with patched build context
/// 5. Write Envoy bootstrap if `--with-envoy`
/// 6. Run `docker compose up -d --force-recreate`
/// 7. Wait for /health
/// 8. Write token to ~/.flowplane/credentials and config.toml
pub fn handle_init(with_envoy: bool, with_httpbin: bool) -> Result<()> {
    use super::compose_runner::ProductionComposeRunner;
    let runner = ProductionComposeRunner::detect()?;
    handle_init_with_runner(with_envoy, with_httpbin, &runner)
}

/// Testable variant of `handle_init` that accepts an injected `ComposeRunner`.
pub fn handle_init_with_runner(
    with_envoy: bool,
    with_httpbin: bool,
    runner: &dyn super::compose_runner::ComposeRunner,
) -> Result<()> {
    loopback_guard()?;

    let runtime = runner.runtime_name();
    eprintln!("Using container runtime: {runtime}");

    let source_dir = resolve_source_dir()?;
    eprintln!("Source directory: {}", source_dir.display());

    // Generate dev mTLS material BEFORE starting compose — the CP's dev mTLS
    // loader looks for these files on startup, and they must exist even when
    // `--with-envoy` is not set (the CP still runs, just without a data plane).
    let dev_certs = ensure_dev_mtls_material()?;
    eprintln!("Dev mTLS certs: {}", dev_certs.root.display());

    // Write compose file
    let compose_path = write_compose_file(&source_dir)?;
    eprintln!("Compose file: {}", compose_path.display());

    // Write Envoy bootstrap if requested. Always mtls=true in dev mode — the
    // certs we just generated are mounted into the Envoy container.
    if with_envoy {
        let bootstrap_path = write_envoy_bootstrap(true)?;
        eprintln!("Envoy bootstrap: {}", bootstrap_path.display());
    }

    // Remove stale orphan network if it exists (created outside compose).
    // Compose requires it to have correct labels; easiest to let compose recreate it.
    let _ = Command::new(runtime)
        .args(["network", "rm", "flowplane-network"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Build profiles list
    let mut profiles: Vec<&str> = Vec::new();
    if with_envoy {
        profiles.push("envoy");
    }
    if with_httpbin {
        profiles.push("httpbin");
    }

    // Unified-auth dev mode: the CP mints its own JWT via the embedded mock OIDC
    // server on startup and writes the credentials file to a bind-mounted path.
    // No dev token is generated here — the CLI waits for the CP to become healthy,
    // then reads the credentials file the CP wrote.
    let env_vars: Vec<(&str, &str)> = Vec::new();

    eprintln!("Starting services...");
    runner.compose_up(&compose_path, "flowplane", &profiles, &env_vars, true)?;

    // Wait for the control plane to become healthy
    wait_for_healthy(60)?;

    // Spawn the dataplane diagnostics agent alongside Envoy. Best-effort:
    // failures here must NOT abort the dev harness — stream NACKs still work
    // without warming-failure reporting.
    if with_envoy {
        try_spawn_dev_agent(&source_dir, &dev_certs);
    }

    // Credentials are written by the control plane itself on startup via the
    // `~/.flowplane` bind mount declared in docker-compose-dev.yml. Verify the
    // file is present — surface a clear error if the CP failed to write it.
    let home = home_dir()?;
    let cred_path = home.join(".flowplane").join("credentials");
    if !cred_path.exists() {
        anyhow::bail!(
            "control plane did not write credentials file at {}: check CP logs",
            cred_path.display()
        );
    }

    // Update config.toml with base_url, team, and org pointing to the compose stack
    let mut config = crate::cli::config::CliConfig::load().unwrap_or_default();
    config.base_url = Some("http://localhost:8080".to_string());
    config.team = Some("default".to_string());
    config.org = Some("dev-org".to_string());
    config.save()?;

    eprintln!();
    eprintln!("Flowplane is running!");
    eprintln!();
    eprintln!("  API:     http://localhost:8080");
    eprintln!("  xDS:     localhost:18000");
    if with_envoy {
        eprintln!("  Envoy:   localhost:10000 (admin: localhost:9901)");
    }
    if with_httpbin {
        eprintln!("  httpbin: http://localhost:8000");
    }
    eprintln!();
    eprintln!("Token saved to ~/.flowplane/credentials");
    eprintln!();
    eprintln!("What's next?");
    if with_envoy && with_httpbin {
        eprintln!(
            "  flowplane expose http://httpbin:80 --name demo   Expose httpbin through Envoy"
        );
        eprintln!("  curl http://localhost:10001/get                  Verify traffic flows");
    }
    eprintln!("  flowplane list                                   See exposed services");
    eprintln!("  flowplane status                                 System health");
    eprintln!("  flowplane logs -f                                Stream logs");
    eprintln!("  flowplane down                                   Stop all services");

    Ok(())
}

/// `flowplane down` — stop and optionally remove volumes.
pub fn handle_down(volumes: bool) -> Result<()> {
    use super::compose_runner::ProductionComposeRunner;
    let runner = ProductionComposeRunner::detect()?;
    handle_down_with_runner(volumes, &runner)
}

/// Testable variant of `handle_down` that accepts an injected `ComposeRunner`.
pub fn handle_down_with_runner(
    volumes: bool,
    runner: &dyn super::compose_runner::ComposeRunner,
) -> Result<()> {
    let home = home_dir()?;
    let compose_path = home.join(".flowplane").join("docker-compose-dev.yml");

    if !compose_path.exists() {
        anyhow::bail!(
            "No compose file found at {}. Did you run `flowplane init` first?",
            compose_path.display()
        );
    }

    // Best-effort: terminate the dev-mode flowplane-agent before tearing down
    // the compose stack so it doesn't spin trying to reach a dying CP.
    let pid_path = agent_pid_path(&home.join(".flowplane"));
    if let Err(err) = kill_agent_from_pid_file(&pid_path) {
        warn!(error = %err, "failed to terminate dev-mode flowplane-agent — ignoring");
    }

    eprintln!("Stopping services...");
    runner.compose_down(&compose_path, "flowplane", volumes)?;

    eprintln!("Flowplane services stopped.");
    if volumes {
        eprintln!("Volumes removed — database data has been deleted.");
    }

    Ok(())
}

/// Spawn `flowplane-agent` as a detached host subprocess attached to the dev
/// Envoy admin port. All failures are logged and swallowed.
fn try_spawn_dev_agent(source_dir: &Path, dev_certs: &DevCertPaths) {
    if std::env::var(DISABLE_AGENT_ENV).is_ok() {
        info!(opt_out = DISABLE_AGENT_ENV, "skipping flowplane-agent spawn (opt-out set)");
        eprintln!("flowplane-agent: skipped ({} is set)", DISABLE_AGENT_ENV);
        return;
    }

    let binary = match find_agent_binary() {
        Some(b) => b,
        None => {
            warn!(
                "flowplane-agent binary not found — warming failure detection disabled. \
                 Build it with `cargo build -p flowplane-agent` (or set FLOWPLANE_AGENT_BIN \
                 to an explicit path) and re-run `flowplane init`."
            );
            eprintln!(
                "flowplane-agent: binary not found — skipping warming failure detection.\n  \
                 Build with: cargo build -p flowplane-agent"
            );
            return;
        }
    };

    // Wait briefly for the Envoy admin port to come up so the agent's first
    // poll has something to read. We don't fail if it never appears.
    if let Err(err) = wait_for_envoy_admin("127.0.0.1:9901", 30) {
        warn!(error = %err, "wait_for_envoy_admin failed — spawning agent anyway");
    }

    let home = match home_dir() {
        Ok(h) => h,
        Err(err) => {
            warn!(error = %err, "could not resolve home dir for agent pid file — skipping agent spawn");
            return;
        }
    };
    let fp_dir = home.join(".flowplane");
    let pid_path = agent_pid_path(&fp_dir);

    // Clean up any stale agent from a previous run before spawning a new one.
    if let Err(err) = kill_agent_from_pid_file(&pid_path) {
        warn!(error = %err, "could not clean up stale agent pid file — continuing");
    }

    let logs_dir = source_dir.join("data").join("logs");
    let spec = AgentSpawnSpec::dev_defaults().with_dev_mtls(dev_certs);

    match spawn_agent_detached(&binary, &spec, &logs_dir, &pid_path) {
        Ok(pid) => {
            let log = agent_log_path(&logs_dir, DEV_DATAPLANE_ID);
            info!(pid, log = %log.display(), "spawned flowplane-agent");
            eprintln!("flowplane-agent: started (pid {}, log {})", pid, log.display());
        }
        Err(err) => {
            warn!(
                error = %err,
                "failed to spawn flowplane-agent — warming failure detection disabled"
            );
            eprintln!("flowplane-agent: failed to start — {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_compose_yaml_is_valid() {
        // Verify the embedded YAML is non-empty and contains expected services
        assert!(!DEV_COMPOSE_YAML.is_empty());
        assert!(DEV_COMPOSE_YAML.contains("flowplane-pg"));
        assert!(DEV_COMPOSE_YAML.contains("control-plane"));
        assert!(DEV_COMPOSE_YAML.contains("FLOWPLANE_AUTH_MODE: dev"));
    }

    #[test]
    fn test_embedded_compose_yaml_contains_envoy_profile() {
        assert!(DEV_COMPOSE_YAML.contains("profiles:"));
        assert!(DEV_COMPOSE_YAML.contains("- envoy"));
    }

    #[test]
    fn test_embedded_compose_yaml_contains_httpbin_profile() {
        assert!(DEV_COMPOSE_YAML.contains("- httpbin"));
        assert!(DEV_COMPOSE_YAML.contains("flowplane-httpbin"));
    }

    #[test]
    fn test_detect_runtime_with_docker_host_env() {
        let original = std::env::var("DOCKER_HOST").ok();

        std::env::set_var("DOCKER_HOST", "unix:///var/run/docker.sock");
        let result = detect_runtime();

        match original {
            Some(v) => std::env::set_var("DOCKER_HOST", v),
            None => std::env::remove_var("DOCKER_HOST"),
        }

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "docker");
    }

    #[test]
    fn test_detect_runtime_podman_docker_host() {
        let original = std::env::var("DOCKER_HOST").ok();

        std::env::set_var("DOCKER_HOST", "unix:///run/podman/podman.sock");
        let result = detect_runtime();

        match original {
            Some(v) => std::env::set_var("DOCKER_HOST", v),
            None => std::env::remove_var("DOCKER_HOST"),
        }

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "podman");
    }

    #[test]
    fn test_loopback_guard_on_host() {
        // On the host machine (not in a container), this should pass
        if !Path::new("/.dockerenv").exists() {
            assert!(loopback_guard().is_ok());
        }
    }

    #[test]
    fn test_write_compose_file_patches_context() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();

        // Temporarily override HOME so write_compose_file writes to our temp dir
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);

        let source_dir = PathBuf::from("/some/source/dir");
        let result = write_compose_file(&source_dir);

        // Restore HOME
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        assert!(result.is_ok());
        let compose_path = result.unwrap();
        let content = std::fs::read_to_string(&compose_path).unwrap();
        assert!(content.contains("context: /some/source/dir"));
        assert!(!content.contains("context: ."));
    }

    #[test]
    fn test_resolve_source_dir_with_env_var() {
        let original = std::env::var("FLOWPLANE_SOURCE_DIR").ok();

        // Point at the actual repo root (this test runs from within the repo)
        let cwd = std::env::current_dir().unwrap();
        // Walk up to find Cargo.toml
        let mut root = cwd.as_path();
        while !root.join("Cargo.toml").exists() {
            root = root.parent().unwrap();
        }

        std::env::set_var("FLOWPLANE_SOURCE_DIR", root);
        let result = resolve_source_dir();

        match original {
            Some(v) => std::env::set_var("FLOWPLANE_SOURCE_DIR", v),
            None => std::env::remove_var("FLOWPLANE_SOURCE_DIR"),
        }

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), root);
    }

    #[test]
    fn test_resolve_source_dir_invalid_env_var() {
        let original = std::env::var("FLOWPLANE_SOURCE_DIR").ok();

        std::env::set_var("FLOWPLANE_SOURCE_DIR", "/nonexistent/path/that/has/no/cargo");
        let result = resolve_source_dir();

        match original {
            Some(v) => std::env::set_var("FLOWPLANE_SOURCE_DIR", v),
            None => std::env::remove_var("FLOWPLANE_SOURCE_DIR"),
        }

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cargo.toml"));
    }

    #[test]
    fn test_write_envoy_bootstrap_mtls() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);

        let result = write_envoy_bootstrap(true);

        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        assert!(result.is_ok());
        let bootstrap_path = result.unwrap();
        assert_eq!(bootstrap_path, home.join(".flowplane/envoy/envoy.yaml"));

        let content = std::fs::read_to_string(&bootstrap_path).unwrap();
        assert!(content.contains("xds_cluster"));
        assert!(content.contains("control-plane"));
        assert!(content.contains("port_value: 18000"));
        assert!(content.contains("port_value: 9901"));
        assert!(content.contains("team: default"));
        assert!(content.contains("transport_socket"));
    }

    /// Parse the bootstrap YAML and walk to the xds_cluster's transport_socket
    /// to verify the mTLS block is structurally correct — not just a substring
    /// match. Catches typos and misplaced keys a string search would miss.
    #[test]
    fn build_envoy_bootstrap_yaml_mtls_has_valid_transport_socket() {
        let yaml = build_envoy_bootstrap_yaml(true);
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&yaml).expect("mtls bootstrap must be valid YAML");

        let clusters = parsed
            .get("static_resources")
            .and_then(|v| v.get("clusters"))
            .and_then(|v| v.as_sequence())
            .expect("static_resources.clusters sequence");
        assert_eq!(clusters.len(), 1, "exactly one bootstrap cluster");

        let cluster = &clusters[0];
        assert_eq!(cluster.get("name").and_then(|v| v.as_str()), Some("xds_cluster"));

        let ts = cluster
            .get("transport_socket")
            .expect("xds_cluster must have a transport_socket block when mtls=true");
        assert_eq!(ts.get("name").and_then(|v| v.as_str()), Some("envoy.transport_sockets.tls"));

        let typed = ts.get("typed_config").expect("typed_config present");
        let type_url = typed.get("@type").and_then(|v| v.as_str()).expect("@type present");
        assert!(
            type_url.ends_with("UpstreamTlsContext"),
            "expected UpstreamTlsContext @type, got {type_url}"
        );

        let tls_certs = typed
            .get("common_tls_context")
            .and_then(|v| v.get("tls_certificates"))
            .and_then(|v| v.as_sequence())
            .expect("tls_certificates sequence — must be static, not SDS");
        assert_eq!(tls_certs.len(), 1);

        let cert_filename = tls_certs[0]
            .get("certificate_chain")
            .and_then(|v| v.get("filename"))
            .and_then(|v| v.as_str())
            .expect("certificate_chain.filename");
        assert!(
            cert_filename.starts_with("/etc/flowplane/certs/"),
            "cert path must be in-container, got {cert_filename}"
        );

        let key_filename = tls_certs[0]
            .get("private_key")
            .and_then(|v| v.get("filename"))
            .and_then(|v| v.as_str())
            .expect("private_key.filename");
        assert!(key_filename.starts_with("/etc/flowplane/certs/"));

        let trusted_ca = typed
            .get("common_tls_context")
            .and_then(|v| v.get("validation_context"))
            .and_then(|v| v.get("trusted_ca"))
            .and_then(|v| v.get("filename"))
            .and_then(|v| v.as_str())
            .expect("validation_context.trusted_ca.filename");
        assert_eq!(trusted_ca, "/etc/flowplane/certs/ca.pem");

        // Adversarial: SDS must NOT appear anywhere in the rendered YAML. A
        // stray `sds_config` would silently switch Envoy to a different cert
        // delivery mechanism and defeat fp-u54's static-bootstrap contract.
        assert!(!yaml.contains("sds_config"), "static mTLS bootstrap must not reference SDS");
    }

    #[test]
    fn build_envoy_bootstrap_yaml_without_mtls_has_no_transport_socket() {
        let yaml = build_envoy_bootstrap_yaml(false);
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&yaml).expect("plaintext bootstrap must be valid YAML");

        let cluster = &parsed["static_resources"]["clusters"][0];
        assert_eq!(cluster.get("name").and_then(|v| v.as_str()), Some("xds_cluster"));
        assert!(
            cluster.get("transport_socket").is_none(),
            "plaintext bootstrap must NOT contain a transport_socket block"
        );
        assert!(!yaml.contains("transport_socket"));
        assert!(!yaml.contains("tls_certificates"));
    }

    #[test]
    fn ensure_dev_mtls_material_at_generates_pki_under_explicit_path() {
        // No HOME mutation — HOME is process-global and racy under parallel
        // test runs. The explicit-path variant exists precisely so this test
        // can exercise the production cert-generation path without touching
        // shared state.
        let tmp = tempfile::TempDir::new().unwrap();
        // Canonicalize before comparing: `generate_dev_certs` absolutizes its
        // input (e.g. /var/folders → /private/var/folders on macOS), so the
        // caller-side path must be canonicalized too for a meaningful equality.
        let cert_root = std::fs::canonicalize(tmp.path()).unwrap().join(".flowplane").join("certs");

        let paths =
            ensure_dev_mtls_material_at(&cert_root).expect("ensure_dev_mtls_material_at succeeds");

        assert_eq!(paths.root, cert_root, "returned root must match explicit input");
        assert!(paths.ca_cert.exists(), "ca.pem must exist on disk");
        assert!(paths.cp_cert.exists(), "cp cert must exist on disk");
        assert!(paths.cp_key.exists(), "cp key must exist on disk");
        assert!(paths.envoy_cert.exists(), "envoy cert must exist on disk");
        assert!(paths.agent_cert.exists(), "agent cert must exist on disk");
        // Every returned path lives under the explicit root — catches a bug
        // where generate_dev_certs silently writes somewhere else.
        assert!(paths.ca_cert.starts_with(&cert_root));
        assert!(paths.cp_cert.starts_with(&cert_root));
        assert!(paths.envoy_cert.starts_with(&cert_root));
        assert!(paths.agent_cert.starts_with(&cert_root));
    }
}
