//! Compose wrapper for `flowplane init` and `flowplane down`.
//!
//! Embeds the dev-mode `docker-compose-dev.yml` via `include_str!` and writes
//! it to a temporary directory. Container runtime (Docker/Podman) is detected
//! automatically.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Write Envoy bootstrap YAML to `~/.flowplane/envoy/envoy.yaml`.
///
/// The bootstrap configures Envoy to connect to the control plane's xDS server
/// at `control-plane:18000` (the Docker network address).
pub fn write_envoy_bootstrap() -> Result<PathBuf> {
    let home = home_dir()?;
    let envoy_dir = home.join(".flowplane").join("envoy");
    std::fs::create_dir_all(&envoy_dir)
        .with_context(|| format!("failed to create directory: {}", envoy_dir.display()))?;

    let bootstrap_path = envoy_dir.join("envoy.yaml");

    let bootstrap = r#"# Auto-generated by `flowplane init --with-envoy`
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
    ads: {}
  lds_config:
    ads: {}
static_resources:
  clusters:
    - name: xds_cluster
      connect_timeout: 5s
      type: LOGICAL_DNS
      dns_lookup_family: V4_ONLY
      http2_protocol_options: {}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: control-plane
                      port_value: 18000
"#;

    std::fs::write(&bootstrap_path, bootstrap).with_context(|| {
        format!("failed to write envoy bootstrap: {}", bootstrap_path.display())
    })?;

    Ok(bootstrap_path)
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
pub fn handle_init(with_envoy: bool) -> Result<()> {
    use super::compose_runner::ProductionComposeRunner;
    let runner = ProductionComposeRunner::detect()?;
    handle_init_with_runner(with_envoy, &runner)
}

/// Testable variant of `handle_init` that accepts an injected `ComposeRunner`.
pub fn handle_init_with_runner(
    with_envoy: bool,
    runner: &dyn super::compose_runner::ComposeRunner,
) -> Result<()> {
    loopback_guard()?;

    let runtime = runner.runtime_name();
    eprintln!("Using container runtime: {runtime}");

    let source_dir = resolve_source_dir()?;
    eprintln!("Source directory: {}", source_dir.display());

    // Resolve or generate dev token (sets FLOWPLANE_DEV_TOKEN env var)
    let token = crate::auth::dev_token::resolve_or_generate_dev_token();

    // Write compose file
    let compose_path = write_compose_file(&source_dir)?;
    eprintln!("Compose file: {}", compose_path.display());

    // Write Envoy bootstrap if requested
    if with_envoy {
        let bootstrap_path = write_envoy_bootstrap()?;
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
    let profiles: Vec<&str> = if with_envoy { vec!["envoy"] } else { vec![] };

    // Pass the dev token as an env var for the compose file's ${FLOWPLANE_DEV_TOKEN}
    let env_vars: Vec<(&str, &str)> = vec![("FLOWPLANE_DEV_TOKEN", token.as_str())];

    eprintln!("Starting services...");
    runner.compose_up(&compose_path, "flowplane", &profiles, &env_vars, true)?;

    // Wait for the control plane to become healthy
    wait_for_healthy(60)?;

    // Write credentials
    let home = home_dir()?;
    crate::auth::dev_token::write_credentials_file(&token, &home)?;

    // Update config.toml with base_url, team, and org pointing to the compose stack
    let mut config = crate::cli::config::CliConfig::load().unwrap_or_default();
    config.base_url = Some("http://localhost:8080".to_string());
    config.team = Some("default".to_string());
    config.org = Some("dev-org".to_string());
    config.save()?;

    eprintln!();
    eprintln!("Flowplane is running!");
    eprintln!("  API:   http://localhost:8080");
    eprintln!("  xDS:   localhost:18000");
    if with_envoy {
        eprintln!("  Envoy: localhost:10000 (admin: localhost:9901)");
    }
    eprintln!();
    eprintln!("Token saved to ~/.flowplane/credentials");
    eprintln!("Run `flowplane down` to stop all services.");

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

    eprintln!("Stopping services...");
    runner.compose_down(&compose_path, "flowplane", volumes)?;

    eprintln!("Flowplane services stopped.");
    if volumes {
        eprintln!("Volumes removed — database data has been deleted.");
    }

    Ok(())
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
    fn test_write_envoy_bootstrap() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);

        let result = write_envoy_bootstrap();

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
    }
}
