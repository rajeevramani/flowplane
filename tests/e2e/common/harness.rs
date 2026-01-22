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
use super::envoy::{EnvoyConfig, EnvoyHandle};
use super::mocks::MockServices;
use super::ports::{PortAllocator, TestPorts};
use super::shared_infra::{SharedInfrastructure, SHARED_LISTENER_PORT};
use super::timeout::{with_timeout, TestTimeout};

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
}

impl TestHarnessConfig {
    /// Create config for a test (defaults to shared mode for speed)
    pub fn new(test_name: impl Into<String>) -> Self {
        Self {
            test_name: test_name.into(),
            start_envoy: true,
            start_auth_mocks: true, // Shared infra always has auth
            start_ext_authz_mock: false,
            use_shared: true, // Default to shared mode
        }
    }

    /// Use isolated infrastructure (slower but fully isolated)
    pub fn isolated(mut self) -> Self {
        self.use_shared = false;
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

        // Create ports struct pointing to shared ports
        let ports = super::ports::TestPorts {
            api: super::shared_infra::SHARED_API_PORT,
            xds: super::shared_infra::SHARED_XDS_PORT,
            envoy_admin: super::shared_infra::SHARED_ENVOY_ADMIN_PORT,
            listener: SHARED_LISTENER_PORT,
            listener_secondary: SHARED_LISTENER_PORT + 1, // Not used in shared mode typically
            echo: echo_port,
            mock_auth: mock_auth_port,
            mock_ext_authz: mock_ext_authz_port,
        };

        info!(
            test = %config.test_name,
            api_url = %shared.api_url(),
            "Using shared infrastructure"
        );

        Ok(Self {
            ports,
            cp_owned: None,
            shared: Some(shared),
            envoy_owned: None,
            mocks_owned: None,
            _temp_dir: None,
            db_path: shared.db_path.clone(),
            is_shared: true,
        })
    }

    /// Start with isolated infrastructure (slower but fully isolated)
    async fn start_isolated(config: TestHarnessConfig) -> anyhow::Result<Self> {
        // Allocate ports
        let mut port_allocator = PortAllocator::for_test(&config.test_name);
        let ports = port_allocator.allocate_test_ports();
        info!(?ports, "Allocated ports for isolated test");

        // Create temp directory for test artifacts
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("flowplane-e2e.db");

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

        // Start control plane
        let cp_config =
            ControlPlaneConfig::new(db_path.clone(), ports.api, ports.xds, ports.listener);
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
            let envoy_config = EnvoyConfig::new(ports.envoy_admin, ports.xds);
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
