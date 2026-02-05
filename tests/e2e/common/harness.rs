//! Test Harness orchestrator for E2E tests
//!
//! Provides a single entry point for starting all test infrastructure:
//! - Control plane (API + xDS)
//! - Envoy proxy
//! - Mock services (echo, auth, ext_authz)
//!
//! Supports two modes:
//! - **Isolated mode**: Each test gets its own CP/Envoy (slower but fully isolated)
//! - **Shared mode**: Tests share a single CP/Envoy (faster, isolation via unique teams)
//!
//! Handles proper startup ordering, health checks, and cleanup.

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
use tracing::info;

use super::control_plane::{ControlPlaneConfig, ControlPlaneHandle};
use super::envoy::{EnvoyConfig, EnvoyHandle, EnvoyXdsTlsConfig};
use super::mocks::MockServices;
use super::ports::{PortAllocator, TestPorts};
use super::shared_infra::SharedInfrastructure;
use super::timeout::{with_timeout, TestTimeout};

// ============================================================================
// mTLS Certificate Configuration
// ============================================================================

/// Certificate paths for mTLS configuration.
///
/// Stores only paths to certificate files, not owned certificate objects.
/// The underlying TempDir ownership is handled by SharedInfrastructure (shared mode)
/// or by the TestHarness _temp_dir field (isolated mode).
#[derive(Debug, Clone)]
pub struct MtlsCertPaths {
    /// CA certificate path (for Envoy trust store)
    pub ca_cert_path: PathBuf,
    /// Server certificate path (only used in isolated mode)
    pub server_cert_path: PathBuf,
    /// Server key path (only used in isolated mode)
    pub server_key_path: PathBuf,
    /// Client certificate path (for Envoy xDS connection)
    pub client_cert_path: PathBuf,
    /// Client key path (for Envoy xDS connection)
    pub client_key_path: PathBuf,
    /// SPIFFE URI embedded in client certificate
    pub spiffe_uri: String,
}

/// Test harness configuration
#[derive(Debug, Clone)]
pub struct TestHarnessConfig {
    /// Test name (used for port allocation and logging)
    pub test_name: String,
    /// Whether to start Envoy (skipped if binary not available)
    pub start_envoy: bool,
    /// Whether to start auth mock services
    pub start_auth_mocks: bool,
    /// Whether to start ext_authz mock
    pub start_ext_authz_mock: bool,
    /// Use shared infrastructure (faster, less isolation)
    pub use_shared: bool,
    /// Enable mTLS for xDS connection (requires FLOWPLANE_E2E_MTLS=1 in shared mode)
    pub enable_mtls: bool,
}

impl TestHarnessConfig {
    /// Create config for a test (defaults to shared mode for speed)
    pub fn new(test_name: impl Into<String>) -> Self {
        Self {
            test_name: test_name.into(),
            start_envoy: true,
            start_auth_mocks: true, // Shared infra always has auth
            start_ext_authz_mock: false,
            use_shared: true,   // Default to shared mode
            enable_mtls: false, // Default to no mTLS
        }
    }

    /// Use isolated infrastructure (slower but fully isolated)
    pub fn isolated(mut self) -> Self {
        self.use_shared = false;
        self
    }

    /// Enable mTLS for xDS connection.
    ///
    /// In shared mode: Requires `FLOWPLANE_E2E_MTLS=1` environment variable to be set
    /// when the test suite starts. The shared infrastructure will initialize the CA.
    ///
    /// In isolated mode: Creates a new CA and certificates for this test instance.
    ///
    /// # Example
    /// ```ignore
    /// let harness = TestHarness::start(
    ///     TestHarnessConfig::new("test_mtls").with_mtls()
    /// ).await?;
    /// assert!(harness.has_mtls());
    /// ```
    pub fn with_mtls(mut self) -> Self {
        self.enable_mtls = true;
        self
    }

    /// Enable auth mock services (Auth0/JWKS)
    /// Alias: `with_auth()`
    pub fn with_auth_mocks(mut self) -> Self {
        self.start_auth_mocks = true;
        self
    }

    /// Enable auth mock services (Auth0/JWKS)
    /// This is a convenience alias for `with_auth_mocks()`
    pub fn with_auth(self) -> Self {
        self.with_auth_mocks()
    }

    /// Enable ext_authz mock service
    pub fn with_ext_authz_mock(mut self) -> Self {
        self.start_ext_authz_mock = true;
        self
    }

    /// Skip Envoy startup (for API-only tests)
    pub fn without_envoy(mut self) -> Self {
        self.start_envoy = false;
        self
    }
}

/// Test harness that orchestrates all test infrastructure
pub struct TestHarness {
    /// Port allocations for this test
    pub ports: TestPorts,
    /// Control plane handle (owned if isolated, None if shared)
    cp_owned: Option<ControlPlaneHandle>,
    /// Reference to shared infrastructure (if using shared mode)
    shared: Option<&'static SharedInfrastructure>,
    /// Envoy handle (owned if isolated, None if shared)
    envoy_owned: Option<EnvoyHandle>,
    /// Mock services (owned if isolated)
    mocks_owned: Option<MockServices>,
    /// Temp directory for test artifacts (cleaned up on drop)
    _temp_dir: Option<TempDir>,
    /// Database path
    pub db_path: PathBuf,
    /// Whether we're using shared mode
    is_shared: bool,
    /// mTLS certificate paths (if mTLS enabled)
    mtls_cert_paths: Option<MtlsCertPaths>,
    /// mTLS CA (kept alive to prevent TempDir cleanup in isolated mode)
    #[allow(dead_code)]
    _mtls_ca: Option<crate::tls::support::TestCertificateAuthority>,
    /// mTLS server cert (kept alive to prevent TempDir cleanup in isolated mode)
    #[allow(dead_code)]
    _mtls_server_cert: Option<crate::tls::support::TestCertificateFiles>,
    /// mTLS client cert (kept alive to prevent TempDir cleanup in isolated mode)
    #[allow(dead_code)]
    _mtls_client_cert: Option<crate::tls::support::TestCertificateFiles>,
}

impl TestHarness {
    /// Get reference to mock services
    pub fn mocks(&self) -> &MockServices {
        if let Some(shared) = self.shared {
            &shared.mocks
        } else {
            self.mocks_owned.as_ref().expect("Mocks should be available in isolated mode")
        }
    }

    /// Get reference to envoy handle if available
    pub fn envoy(&self) -> Option<&EnvoyHandle> {
        if let Some(shared) = self.shared {
            shared.envoy.as_ref()
        } else {
            self.envoy_owned.as_ref()
        }
    }

    // ========================================================================
    // mTLS Helper Methods
    // ========================================================================

    /// Check if mTLS is enabled for this test.
    ///
    /// Returns true if the test was started with `.with_mtls()` and
    /// certificate generation succeeded.
    pub fn has_mtls(&self) -> bool {
        self.mtls_cert_paths.is_some()
    }

    /// Get SPIFFE URI from client certificate (if mTLS enabled).
    ///
    /// The SPIFFE URI is embedded in the client certificate's SAN extension
    /// and is used for team-based authorization in xDS.
    pub fn get_spiffe_uri(&self) -> Option<&str> {
        self.mtls_cert_paths.as_ref().map(|m| m.spiffe_uri.as_str())
    }

    /// Extract team name from SPIFFE URI.
    ///
    /// Parses the SPIFFE URI format: `spiffe://flowplane.local/team/{team}/proxy/{proxy}`
    /// and returns the team component.
    ///
    /// Returns `None` if mTLS is not enabled or the URI format is invalid.
    pub fn get_mtls_team(&self) -> Option<String> {
        self.get_spiffe_uri().and_then(|uri| {
            // Expected format: spiffe://flowplane.local/team/{team}/proxy/{proxy}
            // Parts: ["spiffe:", "", "flowplane.local", "team", "{team}", "proxy", "{proxy}"]
            let parts: Vec<&str> = uri.split('/').collect();
            if parts.len() >= 5 && parts[3] == "team" {
                Some(parts[4].to_string())
            } else {
                None
            }
        })
    }

    /// Get mTLS certificate paths (if mTLS enabled).
    ///
    /// Useful for configuring additional components that need to use
    /// the same certificates (e.g., for testing certificate validation).
    pub fn mtls_certs(&self) -> Option<&MtlsCertPaths> {
        self.mtls_cert_paths.as_ref()
    }
}

impl TestHarness {
    /// Start a new test harness with the given configuration
    ///
    /// In shared mode (default): Uses a singleton CP/Envoy, tests isolated by unique team names
    /// In isolated mode: Each test gets its own CP/Envoy (slower but fully isolated)
    pub async fn start(config: TestHarnessConfig) -> anyhow::Result<Self> {
        info!(test = %config.test_name, shared = config.use_shared, "Starting test harness");

        // Check if E2E tests are enabled
        if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
            anyhow::bail!("E2E tests disabled (set RUN_E2E=1 to enable)");
        }

        if config.use_shared {
            Self::start_shared(config).await
        } else {
            Self::start_isolated(config).await
        }
    }

    /// Start with shared infrastructure (faster)
    async fn start_shared(config: TestHarnessConfig) -> anyhow::Result<Self> {
        let shared = SharedInfrastructure::get_or_init().await?;

        // Get port numbers from shared mocks
        let echo_port = shared.mocks.echo.address().port();
        let mock_auth_port = shared.mocks.auth.as_ref().map(|s| s.address().port()).unwrap_or(0);
        let mock_ext_authz_port =
            shared.mocks.ext_authz.as_ref().map(|s| s.address().port()).unwrap_or(0);

        // Generate unique listener port for this test based on test name hash
        // This allows multiple tests to create their own listeners without port conflicts
        let listener_port = unique_listener_port(&config.test_name);
        let listener_secondary_port = listener_port + 1;

        // Create ports struct - shared CP/Envoy but unique listener ports per test
        let ports = super::ports::TestPorts {
            api: super::shared_infra::SHARED_API_PORT,
            xds: super::shared_infra::SHARED_XDS_PORT,
            envoy_admin: super::shared_infra::SHARED_ENVOY_ADMIN_PORT,
            listener: listener_port,
            listener_secondary: listener_secondary_port,
            echo: echo_port,
            mock_auth: mock_auth_port,
            mock_ext_authz: mock_ext_authz_port,
        };

        info!(
            test = %config.test_name,
            api_url = %shared.api_url(),
            "Using shared infrastructure"
        );

        // Validate mTLS configuration consistency
        if config.enable_mtls && shared.mtls_ca.is_none() {
            anyhow::bail!(
                "mTLS requested but shared infrastructure doesn't have it enabled. \
                 Set FLOWPLANE_E2E_MTLS=1 and restart test run."
            );
        }

        // Generate unique client certificate for this test (if mTLS enabled)
        let mtls_cert_paths = if config.enable_mtls {
            let ca = shared.mtls_ca.as_ref().expect("CA should exist after validation");

            // Generate unique team name for this test
            let team_name = super::shared_infra::unique_team_name(&config.test_name);
            let spiffe_uri = crate::tls::support::TestCertificateAuthority::build_spiffe_uri(
                "flowplane.local",
                &team_name,
                "test-proxy",
            )?;

            info!(
                test = %config.test_name,
                team = %team_name,
                spiffe_uri = %spiffe_uri,
                "Generating mTLS client certificate"
            );

            let client_cert =
                ca.issue_client_cert(&spiffe_uri, "test-proxy", time::Duration::days(1))?;

            Some(MtlsCertPaths {
                ca_cert_path: ca.ca_cert_path.clone(),
                server_cert_path: PathBuf::new(), // Not needed in shared mode
                server_key_path: PathBuf::new(),  // Not needed in shared mode
                client_cert_path: client_cert.cert_path.clone(),
                client_key_path: client_cert.key_path.clone(),
                spiffe_uri,
            })
        } else {
            None
        };

        Ok(Self {
            ports,
            cp_owned: None,
            shared: Some(shared),
            envoy_owned: None,
            mocks_owned: None,
            _temp_dir: None,
            db_path: shared.db_path.clone(),
            is_shared: true,
            mtls_cert_paths,
            // In shared mode, cert lifetimes are managed by SharedInfrastructure
            _mtls_ca: None,
            _mtls_server_cert: None,
            _mtls_client_cert: None,
        })
    }

    /// Start with isolated infrastructure (slower but fully isolated)
    async fn start_isolated(config: TestHarnessConfig) -> anyhow::Result<Self> {
        use crate::tls::support::TestCertificateAuthority;

        // Allocate ports
        let mut port_allocator = PortAllocator::for_test(&config.test_name);
        let ports = port_allocator.allocate_test_ports();
        info!(?ports, "Allocated ports for isolated test");

        // Create temp directory for test artifacts
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("flowplane-e2e.db");

        // Generate mTLS certificates if enabled
        let (mtls_cert_paths, xds_tls_config, mtls_ca, mtls_server_cert, mtls_client_cert) =
            if config.enable_mtls {
                info!("Initializing mTLS for isolated test");

                let ca = TestCertificateAuthority::new(
                    "Flowplane E2E Isolated Test CA",
                    time::Duration::days(1),
                )?;

                // Issue server cert for CP
                // NOTE: server_cert and client_cert must be stored in TestHarness to keep
                // their TempDirs alive - otherwise the cert files get deleted!
                let server_cert = ca.issue_server_cert(&["localhost"], time::Duration::days(1))?;

                // Issue client cert for Envoy
                let team_name = super::shared_infra::unique_team_name(&config.test_name);
                let spiffe_uri = TestCertificateAuthority::build_spiffe_uri(
                    "flowplane.local",
                    &team_name,
                    "test-proxy",
                )?;

                info!(
                    test = %config.test_name,
                    team = %team_name,
                    spiffe_uri = %spiffe_uri,
                    "Generated mTLS certificates for isolated test"
                );

                let client_cert =
                    ca.issue_client_cert(&spiffe_uri, "test-proxy", time::Duration::days(1))?;

                let cert_paths = MtlsCertPaths {
                    ca_cert_path: ca.ca_cert_path.clone(),
                    server_cert_path: server_cert.cert_path.clone(),
                    server_key_path: server_cert.key_path.clone(),
                    client_cert_path: client_cert.cert_path.clone(),
                    client_key_path: client_cert.key_path.clone(),
                    spiffe_uri,
                };

                let xds_tls = flowplane::config::XdsTlsConfig {
                    cert_path: server_cert.cert_path.to_string_lossy().to_string(),
                    key_path: server_cert.key_path.to_string_lossy().to_string(),
                    client_ca_path: Some(ca.ca_cert_path.to_string_lossy().to_string()),
                    require_client_cert: true,
                };

                (Some(cert_paths), Some(xds_tls), Some(ca), Some(server_cert), Some(client_cert))
            } else {
                (None, None, None, None, None)
            };

        // Start mock services
        let mocks = if config.start_auth_mocks && config.start_ext_authz_mock {
            MockServices::start_all().await
        } else if config.start_auth_mocks {
            MockServices::start_with_auth().await
        } else if config.start_ext_authz_mock {
            MockServices::start_with_ext_authz().await
        } else {
            MockServices::start_basic().await
        };
        info!(echo = %mocks.echo_endpoint(), "Mock services started");

        // Drop the port allocator to release the reserved ports before servers bind
        drop(port_allocator);

        // Start control plane with optional TLS
        let mut cp_config =
            ControlPlaneConfig::new(db_path.clone(), ports.api, ports.xds, ports.listener);
        if let Some(tls) = xds_tls_config {
            cp_config = cp_config.with_xds_tls(tls);
        }

        let cp = with_timeout(TestTimeout::startup("Starting control plane"), async {
            ControlPlaneHandle::start(cp_config).await
        })
        .await?;

        // Wait for CP to be ready
        with_timeout(TestTimeout::startup("Control plane ready"), async { cp.wait_ready().await })
            .await?;
        info!(api = %cp.api_addr, xds = %cp.xds_addr, "Control plane ready");

        // Start Envoy if available and requested
        let envoy = if config.start_envoy && EnvoyHandle::is_available() {
            // Configure Envoy with team metadata for xDS authorization
            // Use the shared team name so Envoy can see test resources
            let mut envoy_config =
                EnvoyConfig::new(ports.envoy_admin, ports.xds).with_metadata(serde_json::json!({
                    "team": super::shared_infra::E2E_SHARED_TEAM
                }));

            // Configure Envoy with mTLS client certificates if enabled
            if let Some(ref cert_paths) = mtls_cert_paths {
                let tls_config = EnvoyXdsTlsConfig {
                    ca_cert: cert_paths.ca_cert_path.clone(),
                    client_cert: Some(cert_paths.client_cert_path.clone()),
                    client_key: Some(cert_paths.client_key_path.clone()),
                };
                envoy_config = envoy_config.with_xds_tls(tls_config);
            }

            let envoy = EnvoyHandle::start(envoy_config)?;

            with_timeout(TestTimeout::startup("Envoy ready"), async { envoy.wait_ready().await })
                .await?;
            info!(admin_port = ports.envoy_admin, "Envoy ready");

            Some(envoy)
        } else {
            if config.start_envoy {
                info!("Envoy binary not found - skipping Envoy startup");
            }
            None
        };

        Ok(Self {
            ports,
            cp_owned: Some(cp),
            shared: None,
            envoy_owned: envoy,
            mocks_owned: Some(mocks),
            _temp_dir: Some(temp_dir),
            db_path,
            is_shared: false,
            mtls_cert_paths,
            _mtls_ca: mtls_ca,
            _mtls_server_cert: mtls_server_cert,
            _mtls_client_cert: mtls_client_cert,
        })
    }

    /// Get API URL for making requests
    pub fn api_url(&self) -> String {
        if let Some(shared) = self.shared {
            shared.api_url()
        } else {
            self.cp_owned.as_ref().expect("CP should exist in isolated mode").api_url()
        }
    }

    /// Get echo server endpoint for cluster configuration
    pub fn echo_endpoint(&self) -> String {
        self.mocks().echo_endpoint()
    }

    /// Wait for a route to be available through Envoy
    ///
    /// Returns the response body on success, error on timeout.
    pub async fn wait_for_route(
        &self,
        host: &str,
        path: &str,
        expected_status: u16,
    ) -> anyhow::Result<String> {
        let envoy = self
            .envoy()
            .ok_or_else(|| anyhow::anyhow!("Envoy not available - cannot wait for route"))?;

        envoy.wait_for_route(self.ports.listener, host, path, expected_status).await
    }

    /// Wait for a route on a specific port
    pub async fn wait_for_route_on_port(
        &self,
        port: u16,
        host: &str,
        path: &str,
        expected_status: u16,
    ) -> anyhow::Result<String> {
        let envoy = self
            .envoy()
            .ok_or_else(|| anyhow::anyhow!("Envoy not available - cannot wait for route"))?;

        envoy.wait_for_route(port, host, path, expected_status).await
    }

    /// Send a GET request through Envoy
    pub async fn proxy_get(&self, host: &str, path: &str) -> anyhow::Result<(u16, String)> {
        let envoy = self
            .envoy()
            .ok_or_else(|| anyhow::anyhow!("Envoy not available - cannot proxy request"))?;

        envoy.proxy_get(self.ports.listener, host, path).await
    }

    /// Send a GET request through a specific port
    pub async fn proxy_get_on_port(
        &self,
        port: u16,
        host: &str,
        path: &str,
    ) -> anyhow::Result<(u16, String)> {
        let envoy = self
            .envoy()
            .ok_or_else(|| anyhow::anyhow!("Envoy not available - cannot proxy request"))?;

        envoy.proxy_get(port, host, path).await
    }

    /// Get Envoy config dump
    pub async fn get_config_dump(&self) -> anyhow::Result<String> {
        let envoy = self.envoy().ok_or_else(|| anyhow::anyhow!("Envoy not available"))?;

        envoy.get_config_dump().await
    }

    /// Get Envoy stats
    pub async fn get_stats(&self) -> anyhow::Result<String> {
        let envoy = self.envoy().ok_or_else(|| anyhow::anyhow!("Envoy not available"))?;

        envoy.get_stats().await
    }

    /// Check if Envoy is available
    pub fn has_envoy(&self) -> bool {
        self.envoy().is_some()
    }

    /// Shutdown the test harness
    ///
    /// In shared mode, this is a no-op (shared infra lives for entire test run).
    /// In isolated mode, this shuts down owned components.
    pub async fn shutdown(mut self) {
        if self.is_shared {
            // Shared infrastructure is never shut down by individual tests
            info!("Test harness using shared infrastructure - no shutdown needed");
            return;
        }

        info!("Shutting down isolated test harness");

        // Shutdown in reverse order
        if let Some(mut envoy) = self.envoy_owned.take() {
            envoy.shutdown();
            // Give Envoy time to clean up
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if let Some(cp) = self.cp_owned.take() {
            cp.shutdown().await;
        }
    }
}

/// Generate a unique listener port for a test based on its name
///
/// Uses a hash of the test name to generate a port in the range 20000-29999.
/// This ensures each test gets a deterministic but unique port.
fn unique_listener_port(test_name: &str) -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    test_name.hash(&mut hasher);
    let hash = hasher.finish();

    // Port range 20000-29999 (10000 ports)
    // Leaves room for shared ports (19xxx) and isolated test ports (30xxx+)
    20000 + (hash % 10000) as u16
}

/// Quick harness startup for simple tests
pub async fn quick_harness(test_name: &str) -> anyhow::Result<TestHarness> {
    TestHarness::start(TestHarnessConfig::new(test_name)).await
}

/// Harness with auth mocks for JWT tests
pub async fn auth_harness(test_name: &str) -> anyhow::Result<TestHarness> {
    TestHarness::start(TestHarnessConfig::new(test_name).with_auth_mocks()).await
}

/// Harness with all mocks for comprehensive tests
pub async fn full_harness(test_name: &str) -> anyhow::Result<TestHarness> {
    TestHarness::start(TestHarnessConfig::new(test_name).with_auth_mocks().with_ext_authz_mock())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_config() {
        let config = TestHarnessConfig::new("test_example").with_auth_mocks().with_ext_authz_mock();

        assert_eq!(config.test_name, "test_example");
        assert!(config.start_envoy);
        assert!(config.start_auth_mocks);
        assert!(config.start_ext_authz_mock);
    }

    #[test]
    fn test_harness_config_without_envoy() {
        let config = TestHarnessConfig::new("test_api_only").without_envoy();

        assert!(!config.start_envoy);
    }
}
