//! Shared infrastructure for E2E tests
//!
//! Provides a single control plane and Envoy instance that's reused across all tests.
//! Each test gets isolation via unique team names based on test name.
//!
//! This approach:
//! - Speeds up tests significantly (no startup overhead per test)
//! - Avoids port conflicts from orphaned processes
//! - Is more realistic (closer to production behavior)
//!
//! IMPORTANT: The shared infrastructure runs in a dedicated tokio runtime that persists
//! across all tests. This is necessary because each `#[tokio::test]` has its own runtime
//! that gets shut down when the test completes, which would kill any tasks spawned on it.

use std::sync::{Arc, OnceLock};

use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use tracing::info;

use super::api_client::{ApiClient, TEST_EMAIL, TEST_NAME, TEST_PASSWORD};
use super::control_plane::{ControlPlaneConfig, ControlPlaneHandle};
use super::envoy::{EnvoyConfig, EnvoyHandle, EnvoyXdsTlsConfig};
use super::mocks::MockServices;
use super::timeout::{with_timeout, TestTimeout};
use crate::tls::support::TestCertificateAuthority;

/// Fixed ports for shared infrastructure
pub const SHARED_API_PORT: u16 = 19080;
pub const SHARED_XDS_PORT: u16 = 19010;
pub const SHARED_ENVOY_ADMIN_PORT: u16 = 19901;
pub const SHARED_LISTENER_PORT: u16 = 19000;

/// Clean up stale processes from previous test runs.
///
/// This function kills any processes listening on the shared infrastructure ports.
/// This is necessary because if tests crash or timeout, the Drop handlers may not
/// run properly, leaving orphaned processes that block subsequent test runs.
fn cleanup_stale_processes() {
    use std::process::Command;

    // All ports that shared infrastructure uses
    let ports = [SHARED_API_PORT, SHARED_XDS_PORT, SHARED_ENVOY_ADMIN_PORT, SHARED_LISTENER_PORT];

    for port in ports {
        // Use lsof to find PIDs listening on the port
        let output = Command::new("lsof").args(["-ti", &format!(":{}", port)]).output();

        if let Ok(output) = output {
            if output.status.success() {
                let pids = String::from_utf8_lossy(&output.stdout);
                for pid_str in pids.lines() {
                    if let Ok(pid) = pid_str.trim().parse::<i32>() {
                        // Don't kill ourselves
                        let our_pid = std::process::id() as i32;
                        if pid != our_pid {
                            info!(port, pid, "Killing stale process on shared port");
                            // Use SIGKILL to ensure process termination
                            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
                        }
                    }
                }
            }
        }
    }

    // Also specifically kill any stale Envoy processes
    // This is a safety net for Envoy processes that might be on different ports
    let output = Command::new("pgrep").arg("-f").arg("envoy.*e2e").output();
    if let Ok(output) = output {
        if output.status.success() {
            let pids = String::from_utf8_lossy(&output.stdout);
            for pid_str in pids.lines() {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    let our_pid = std::process::id() as i32;
                    if pid != our_pid {
                        info!(pid, "Killing stale Envoy process");
                        let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
                    }
                }
            }
        }
    }

    // Clean up stale testcontainer PostgreSQL containers from previous runs.
    // ContainerAsync::Drop is async but Rust's Drop is sync, so containers
    // are never properly cleaned up when the tokio runtime shuts down.
    {
        use std::process::Command;

        let output = Command::new("docker")
            .args([
                "ps",
                "-q",
                "--filter",
                "label=org.testcontainers.managed-by=testcontainers",
                "--filter",
                "ancestor=postgres",
            ])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let ids_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !ids_str.is_empty() {
                    let ids: Vec<&str> = ids_str.lines().collect();
                    info!(
                        count = ids.len(),
                        "Cleaning up stale testcontainer PostgreSQL containers"
                    );

                    let mut stop_args = vec!["stop", "--time", "5"];
                    stop_args.extend(&ids);
                    let _ = Command::new("docker").args(&stop_args).output();

                    let mut rm_args = vec!["rm", "-f"];
                    rm_args.extend(&ids);
                    let _ = Command::new("docker").args(&rm_args).output();
                }
            }
        }
    }

    // Give processes time to fully terminate and release ports
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// Shared team name for e2e tests that need Envoy routing
/// Tests that create resources expecting Envoy to route to them MUST use this team
/// This is configured in Envoy's node.metadata.team for xDS authorization
pub const E2E_SHARED_TEAM: &str = "e2e-shared";

/// Shared infrastructure singleton - initialized lazily with dedicated runtime
static SHARED_INFRA: OnceLock<SharedInfrastructure> = OnceLock::new();

/// Initialization result for waiting threads
static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

/// PostgreSQL container for shared DB - must live for entire test run
static SHARED_PG_CONTAINER: OnceLock<ContainerAsync<Postgres>> = OnceLock::new();

/// Dedicated runtime for shared infrastructure - persists across all tests
static SHARED_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Initialization lock to prevent concurrent initialization attempts
static INIT_ONCE: std::sync::Once = std::sync::Once::new();

/// Shared infrastructure that's reused across all tests
pub struct SharedInfrastructure {
    /// Control plane handle
    pub cp: ControlPlaneHandle,
    /// Envoy handle (optional)
    pub envoy: Option<EnvoyHandle>,
    /// Mock services
    pub mocks: MockServices,
    /// Database URL
    pub db_url: String,
    /// mTLS Certificate Authority (if FLOWPLANE_E2E_MTLS=1 was set)
    ///
    /// When present, the CP is configured with xDS TLS and tests can
    /// request mTLS certificates via `TestHarnessConfig::with_mtls()`.
    pub mtls_ca: Option<Arc<TestCertificateAuthority>>,
    /// Server certificate for xDS TLS (kept alive to prevent TempDir cleanup)
    #[allow(dead_code)]
    mtls_server_cert: Option<crate::tls::support::TestCertificateFiles>,
    /// Envoy client certificate for mTLS (kept alive to prevent TempDir cleanup)
    #[allow(dead_code)]
    mtls_envoy_client_cert: Option<crate::tls::support::TestCertificateFiles>,
}

impl SharedInfrastructure {
    /// Get or create the shared infrastructure.
    ///
    /// This function ensures the shared infrastructure is initialized in a dedicated
    /// tokio runtime that persists across all tests. Individual test runtimes shut down
    /// when their test completes, which would kill any tasks spawned on them.
    pub async fn get_or_init() -> anyhow::Result<&'static SharedInfrastructure> {
        // Fast path: already initialized
        if let Some(infra) = SHARED_INFRA.get() {
            return Ok(infra);
        }

        // Use std::sync::Once to ensure initialization happens exactly once
        // Other threads will block here until initialization completes
        INIT_ONCE.call_once(|| {
            // Spawn a separate thread to avoid "cannot start runtime within runtime" errors
            let handle = std::thread::spawn(|| {
                // Create a dedicated runtime for the shared infrastructure
                let runtime = SHARED_RUNTIME.get_or_init(|| {
                    tokio::runtime::Builder::new_multi_thread()
                        .enable_all()
                        .thread_name("shared-infra")
                        .build()
                        .expect("Failed to create shared infrastructure runtime")
                });

                // Run initialization in the dedicated runtime
                match runtime.block_on(Self::initialize()) {
                    Ok(infra) => {
                        let _ = SHARED_INFRA.set(infra);
                        let _ = INIT_RESULT.set(Ok(()));
                    }
                    Err(e) => {
                        let _ = INIT_RESULT.set(Err(e.to_string()));
                    }
                }
            });

            // Wait for the initialization thread to complete
            let _ = handle.join();
        });

        // Check if initialization succeeded
        match INIT_RESULT.get() {
            Some(Ok(())) => Ok(SHARED_INFRA.get().expect("Init succeeded but no infra")),
            Some(Err(e)) => Err(anyhow::anyhow!("Shared infrastructure init failed: {}", e)),
            None => Err(anyhow::anyhow!("Shared infrastructure init incomplete")),
        }
    }

    /// Internal initialization - runs in the dedicated shared runtime
    async fn initialize() -> anyhow::Result<SharedInfrastructure> {
        info!("Initializing shared E2E infrastructure...");

        // Clean up any stale processes from previous test runs
        // This prevents port conflicts and test hangs
        cleanup_stale_processes();

        // Install Rustls crypto provider (required for TLS to work)
        // This must be done before any TLS operations
        use rustls::crypto::{ring, CryptoProvider};
        if CryptoProvider::get_default().is_none() {
            ring::default_provider()
                .install_default()
                .expect("install ring crypto provider for E2E tests");
        }

        // Check if mTLS is requested via environment variable
        let enable_mtls = std::env::var("FLOWPLANE_E2E_MTLS").ok().as_deref() == Some("1");

        if enable_mtls {
            info!("mTLS enabled for shared infrastructure (FLOWPLANE_E2E_MTLS=1)");
        }

        // Start PostgreSQL container for shared E2E database
        info!("Starting PostgreSQL container for shared E2E database...");
        let container = Postgres::default()
            .start()
            .await
            .expect("Failed to start PostgreSQL container for shared E2E");

        let pg_host = container.get_host().await.expect("Failed to get PostgreSQL container host");
        let pg_port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Failed to get PostgreSQL container port");
        let db_url = format!("postgresql://postgres:postgres@{}:{}/postgres", pg_host, pg_port);
        info!(db_url = %db_url, "PostgreSQL container ready for shared E2E");

        // Store container in static to keep it alive for entire test run
        let _ = SHARED_PG_CONTAINER.set(container);

        // Initialize mTLS CA if enabled
        let (mtls_ca, mtls_server_cert, xds_tls_config) = if enable_mtls {
            let ca = TestCertificateAuthority::new(
                "Flowplane E2E Shared Test CA",
                time::Duration::days(1),
            )?;

            // Issue server cert for CP xDS
            // NOTE: server_cert must be stored in SharedInfrastructure to keep
            // its TempDir alive - otherwise the cert files get deleted!
            let server_cert = ca.issue_server_cert(&["localhost"], time::Duration::days(1))?;

            let xds_tls = flowplane::config::XdsTlsConfig {
                cert_path: server_cert.cert_path.to_string_lossy().to_string(),
                key_path: server_cert.key_path.to_string_lossy().to_string(),
                client_ca_path: Some(ca.ca_cert_path.to_string_lossy().to_string()),
                require_client_cert: true,
            };

            info!(
                ca_path = %ca.ca_cert_path.display(),
                server_cert = %server_cert.cert_path.display(),
                "Generated mTLS certificates for shared infrastructure"
            );

            (Some(Arc::new(ca)), Some(server_cert), Some(xds_tls))
        } else {
            (None, None, None)
        };

        // Start mock services with full support (auth + ext_authz)
        let mocks = MockServices::start_all().await;
        info!(echo = %mocks.echo_endpoint(), "Mock services started");

        // Start control plane with optional TLS
        let mut cp_config = ControlPlaneConfig::new(
            db_url.clone(),
            SHARED_API_PORT,
            SHARED_XDS_PORT,
            SHARED_LISTENER_PORT,
        );

        if let Some(tls) = xds_tls_config {
            cp_config = cp_config.with_xds_tls(tls);
        }

        let cp = with_timeout(TestTimeout::startup("Starting shared control plane"), async {
            ControlPlaneHandle::start(cp_config).await
        })
        .await?;

        with_timeout(TestTimeout::startup("Shared control plane ready"), async {
            cp.wait_ready().await
        })
        .await?;
        info!(api = %cp.api_addr, xds = %cp.xds_addr, mtls = enable_mtls, "Shared control plane ready");

        // Bootstrap the system with standard test credentials
        // This ensures all tests can rely on a bootstrapped system
        let api_url = format!("http://{}", cp.api_addr);
        let api = ApiClient::new(&api_url);

        let needs_bootstrap = api.needs_bootstrap().await.unwrap_or(true);
        if needs_bootstrap {
            info!("Bootstrapping shared infrastructure...");
            api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME)
                .await
                .expect("Shared infrastructure bootstrap should succeed");
            info!("Shared infrastructure bootstrap complete");
        }

        // Clean up stale dataplanes from previous test runs
        // This ensures a clean slate for tests that create dataplanes
        info!("Cleaning up stale dataplanes from previous runs...");
        let session =
            api.login(TEST_EMAIL, TEST_PASSWORD).await.expect("Login should succeed for cleanup");
        let token_resp = api
            .create_token(&session, "cleanup-token", vec!["admin:all".to_string()])
            .await
            .expect("Token creation should succeed for cleanup");
        let deleted = api.delete_all_dataplanes(&token_resp.token).await.unwrap_or(0);
        if deleted > 0 {
            info!(count = deleted, "Deleted stale dataplanes");
        }

        // Start Envoy if available
        // Note: In shared mode, Envoy does NOT use mTLS client certs by default
        // Tests that need mTLS should use isolated mode or the harness generates
        // per-test client certs (but shared Envoy still connects without them)
        let (envoy, mtls_envoy_client_cert) = if EnvoyHandle::is_available() {
            // Configure Envoy with team metadata for xDS authorization
            // Tests that need Envoy routing must create resources under E2E_SHARED_TEAM
            let mut envoy_config = EnvoyConfig::new(SHARED_ENVOY_ADMIN_PORT, SHARED_XDS_PORT)
                .with_metadata(serde_json::json!({
                    "team": E2E_SHARED_TEAM
                }));

            // If mTLS is enabled, configure Envoy with a default client cert
            // This allows the shared Envoy to connect to the mTLS-enabled CP
            // NOTE: client_cert must be stored in SharedInfrastructure to keep
            // its TempDir alive - otherwise the cert files get deleted!
            let envoy_client_cert = if let Some(ref ca) = mtls_ca {
                // Generate a shared Envoy client cert
                let spiffe_uri = TestCertificateAuthority::build_spiffe_uri(
                    "flowplane.local",
                    E2E_SHARED_TEAM,
                    "shared-envoy",
                )?;
                let client_cert =
                    ca.issue_client_cert(&spiffe_uri, "shared-envoy", time::Duration::days(1))?;

                let tls_config = EnvoyXdsTlsConfig {
                    ca_cert: ca.ca_cert_path.clone(),
                    client_cert: Some(client_cert.cert_path.clone()),
                    client_key: Some(client_cert.key_path.clone()),
                };
                envoy_config = envoy_config.with_xds_tls(tls_config);
                info!(
                    client_cert = %client_cert.cert_path.display(),
                    "Configured shared Envoy with mTLS client certificate"
                );
                Some(client_cert)
            } else {
                None
            };

            let envoy = EnvoyHandle::start(envoy_config)?;

            with_timeout(TestTimeout::startup("Shared Envoy ready"), async {
                envoy.wait_ready().await
            })
            .await?;
            info!(admin_port = SHARED_ENVOY_ADMIN_PORT, "Shared Envoy ready");

            (Some(envoy), envoy_client_cert)
        } else {
            info!("Envoy binary not found - tests will skip proxy verification");
            (None, None)
        };

        Ok(SharedInfrastructure {
            cp,
            envoy,
            mocks,
            db_url,
            mtls_ca,
            mtls_server_cert,
            mtls_envoy_client_cert,
        })
    }

    /// Get API URL
    pub fn api_url(&self) -> String {
        self.cp.api_url()
    }

    /// Get echo server endpoint
    pub fn echo_endpoint(&self) -> String {
        self.mocks.echo_endpoint()
    }

    /// Check if Envoy is available
    pub fn has_envoy(&self) -> bool {
        self.envoy.is_some()
    }

    /// Wait for a route through Envoy
    pub async fn wait_for_route(
        &self,
        host: &str,
        path: &str,
        expected_status: u16,
    ) -> anyhow::Result<String> {
        let envoy = self.envoy.as_ref().ok_or_else(|| anyhow::anyhow!("Envoy not available"))?;

        envoy.wait_for_route(SHARED_LISTENER_PORT, host, path, expected_status).await
    }
}

/// Generate a unique team name for a test
pub fn unique_team_name(test_name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    test_name.hash(&mut hasher);
    let hash = hasher.finish();

    format!("team-{:08x}", hash as u32)
}

/// Generate a unique resource name for a test
pub fn unique_name(test_name: &str, resource: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    test_name.hash(&mut hasher);
    let hash = hasher.finish();

    format!("{}-{:08x}", resource, hash as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unique_team_name() {
        let name1 = unique_team_name("test_foo");
        let name2 = unique_team_name("test_bar");
        let name1_again = unique_team_name("test_foo");

        assert_ne!(name1, name2);
        assert_eq!(name1, name1_again);
        assert!(name1.starts_with("team-"));
    }

    #[test]
    fn test_unique_name() {
        let name = unique_name("test_foo", "cluster");
        assert!(name.starts_with("cluster-"));
    }
}
