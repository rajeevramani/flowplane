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
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use tracing::info;

use super::control_plane::ControlPlaneHandle;
use super::envoy::{EnvoyConfig, EnvoyHandle};
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
    /// Override SPIFFE team component in mTLS client cert (isolated mode only)
    pub mtls_team: Option<String>,
    /// Override SPIFFE proxy_id component in mTLS client cert (isolated mode only)
    pub mtls_proxy_id: Option<String>,
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
            mtls_team: None,
            mtls_proxy_id: None,
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

    /// Override the SPIFFE identity (team + proxy_id) baked into the mTLS
    /// client cert. Only honored in isolated mode. When unset, isolated mode
    /// uses a hashed unique team name, which will NOT match seeded dev
    /// resources — set this to ("default", "dev-dataplane") for dev-mTLS tests
    /// that rely on the seeded team.
    pub fn with_mtls_identity(
        mut self,
        team: impl Into<String>,
        proxy_id: impl Into<String>,
    ) -> Self {
        self.mtls_team = Some(team.into());
        self.mtls_proxy_id = Some(proxy_id.into());
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
    /// Per-test Envoy handle (for envoy_harness mode: shared CP + own Envoy)
    envoy_per_test: Option<EnvoyHandle>,
    /// Mock services (owned if isolated)
    mocks_owned: Option<MockServices>,
    /// Temp directory for test artifacts (cleaned up on drop)
    _temp_dir: Option<TempDir>,
    /// Database URL
    pub db_url: String,
    /// Whether we're using shared mode
    is_shared: bool,
    /// Auth token to use for API requests (bearer token in dev, JWT in prod)
    pub auth_token: String,
    /// Team name for test resources
    pub team: String,
    /// Organization name
    pub org: String,
    /// Auth mode used
    pub auth_mode: flowplane::config::AuthMode,
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
    /// PostgreSQL container (kept alive in isolated mode)
    #[allow(dead_code)]
    _pg_container: Option<ContainerAsync<Postgres>>,
    /// Mock OIDC issuer used to mint the dev JWT in isolated mode. Kept
    /// alive for the lifetime of the harness so the issuer doesn't abort.
    #[cfg(feature = "dev-oidc")]
    #[allow(dead_code)]
    _mock_oidc: Option<std::sync::Arc<flowplane::dev::oidc_server::MockOidcServer>>,
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

    /// Get reference to envoy handle if available.
    ///
    /// Priority: per-test Envoy (envoy_harness) > owned Envoy (isolated) > shared Envoy.
    pub fn envoy(&self) -> Option<&EnvoyHandle> {
        if let Some(ref envoy) = self.envoy_per_test {
            Some(envoy)
        } else if let Some(shared) = self.shared {
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

    /// Access the isolated-mode mTLS CA so tests can mint additional client
    /// certs (e.g. for a second subprocess like flowplane-agent) with custom
    /// SPIFFE identities. Returns None in shared mode.
    pub fn mtls_ca(&self) -> Option<&crate::tls::support::TestCertificateAuthority> {
        self._mtls_ca.as_ref()
    }

    // ========================================================================
    // Authenticated Request Helpers
    // ========================================================================

    /// Make an authenticated GET request to the API.
    pub async fn authed_get(&self, path: &str) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}{}", self.api_url(), path);
        reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(Into::into)
    }

    /// Make an authenticated POST request with a JSON body.
    pub async fn authed_post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}{}", self.api_url(), path);
        reqwest::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(body)
            .send()
            .await
            .map_err(Into::into)
    }

    /// Get a reqwest client pre-configured with the auth token.
    pub fn authed_client(&self) -> reqwest::Client {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.auth_token))
                .expect("valid header value"),
        );
        reqwest::Client::builder().default_headers(headers).build().expect("build reqwest client")
    }

    /// Check if this harness is running in dev auth mode.
    pub fn is_dev_mode(&self) -> bool {
        self.auth_mode == flowplane::config::AuthMode::Dev
    }

    /// Access the shared infrastructure (if using shared mode).
    ///
    /// Returns `None` in isolated mode. Use this to access multi-user
    /// token helpers (`create_test_user`, `get_user_token`, etc.) in prod mode.
    pub fn shared_infra(&self) -> Option<&'static SharedInfrastructure> {
        self.shared
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

        let auth_mode = match shared.auth_mode {
            super::shared_infra::E2eAuthMode::Dev => flowplane::config::AuthMode::Dev,
            super::shared_infra::E2eAuthMode::Prod => flowplane::config::AuthMode::Prod,
        };

        Ok(Self {
            ports,
            cp_owned: None,
            shared: Some(shared),
            envoy_owned: None,
            envoy_per_test: None,
            mocks_owned: None,
            _temp_dir: None,
            db_url: shared.db_url.clone(),
            is_shared: true,
            auth_token: shared.auth_token.clone(),
            team: shared.team.clone(),
            org: shared.org.clone(),
            auth_mode,
            mtls_cert_paths,
            // In shared mode, cert lifetimes are managed by SharedInfrastructure
            _mtls_ca: None,
            _mtls_server_cert: None,
            _mtls_client_cert: None,
            _pg_container: None,
            #[cfg(feature = "dev-oidc")]
            _mock_oidc: None,
        })
    }

    /// Start with isolated infrastructure (slower but fully isolated).
    ///
    /// Thin wrapper over `boot_cp`: allocates per-test ports, builds a
    /// `CpBootConfig`, calls `boot_cp`, and moves the returned state into
    /// a fresh `TestHarness`. Isolated-prod is unsupported and bails early.
    async fn start_isolated(config: TestHarnessConfig) -> anyhow::Result<Self> {
        // Isolated prod requires spinning up its own Zitadel container — not
        // yet supported. Shared mode (the default) handles prod E2E.
        let e2e_mode = super::shared_infra::e2e_auth_mode();
        if matches!(e2e_mode, super::shared_infra::E2eAuthMode::Prod) {
            anyhow::bail!(
                "Isolated mode is not supported for prod auth. \
                 Use shared mode (the default) for prod E2E tests. \
                 Isolated mode requires its own Zitadel container, which is not yet implemented."
            );
        }

        // Allocate per-test ports; release the reservations before servers
        // bind so the listener sockets are free.
        let mut port_allocator = PortAllocator::for_test(&config.test_name);
        let ports = port_allocator.allocate_test_ports();
        info!(?ports, "Allocated ports for isolated test");
        drop(port_allocator);

        // Temp directory for misc per-test artifacts (kept alive for the
        // lifetime of the harness).
        let temp_dir = tempfile::tempdir()?;

        // Pick the mocks flavor from config knobs.
        let mocks_flavor = if config.start_auth_mocks && config.start_ext_authz_mock {
            super::shared_infra::MocksFlavor::All
        } else if config.start_auth_mocks {
            super::shared_infra::MocksFlavor::Auth
        } else if config.start_ext_authz_mock {
            super::shared_infra::MocksFlavor::ExtAuthz
        } else {
            super::shared_infra::MocksFlavor::Basic
        };

        // Envoy node metadata team: explicit override wins, otherwise the
        // shared-team default (preserves pre-refactor isolated behavior —
        // tests that hit the seeded "default" team pass `mtls_team` via
        // `with_mtls_identity`).
        let envoy_team = config
            .mtls_team
            .clone()
            .unwrap_or_else(|| super::shared_infra::E2E_SHARED_TEAM.to_string());

        let boot_config = super::shared_infra::CpBootConfig {
            auth_mode: e2e_mode,
            test_name: config.test_name.clone(),
            ports: super::shared_infra::CpBootPorts {
                api: ports.api,
                xds: ports.xds,
                envoy_admin: ports.envoy_admin,
                listener: ports.listener,
            },
            enable_mtls: config.enable_mtls,
            enable_envoy: config.start_envoy,
            mtls_team: config.mtls_team.clone(),
            mtls_proxy_id: config.mtls_proxy_id.clone(),
            envoy_team,
            mocks_flavor,
            update_team_admin_port: false,
        };

        let booted = super::shared_infra::boot_cp(boot_config).await?;

        // Rebuild MtlsCertPaths from the cert bundle returned by boot_cp so
        // tests can access CA / client cert paths via `TestHarness::mtls_certs()`.
        let mtls_cert_paths = match (
            booted.mtls_ca.as_deref(),
            booted.mtls_server_cert.as_ref(),
            booted.mtls_envoy_client_cert.as_ref(),
            booted.mtls_spiffe_uri.as_ref(),
        ) {
            (Some(ca), Some(server_cert), Some(client_cert), Some(spiffe_uri)) => {
                Some(MtlsCertPaths {
                    ca_cert_path: ca.ca_cert_path.clone(),
                    server_cert_path: server_cert.cert_path.clone(),
                    server_key_path: server_cert.key_path.clone(),
                    client_cert_path: client_cert.cert_path.clone(),
                    client_key_path: client_cert.key_path.clone(),
                    spiffe_uri: spiffe_uri.clone(),
                })
            }
            _ => None,
        };

        let auth_mode = match booted.auth_mode {
            super::shared_infra::E2eAuthMode::Dev => flowplane::config::AuthMode::Dev,
            super::shared_infra::E2eAuthMode::Prod => flowplane::config::AuthMode::Prod,
        };

        // `mtls_ca` is returned as `Arc<TestCertificateAuthority>` from boot_cp
        // but `TestHarness::_mtls_ca` is `Option<TestCertificateAuthority>`.
        // Unwrap the Arc back into the inner value (safe — we own the only ref).
        let mtls_ca_owned = booted.mtls_ca.and_then(|arc| Arc::try_unwrap(arc).ok());

        Ok(Self {
            ports,
            cp_owned: Some(booted.cp),
            shared: None,
            envoy_owned: booted.envoy,
            envoy_per_test: None,
            mocks_owned: Some(booted.mocks),
            _temp_dir: Some(temp_dir),
            db_url: booted.db_url,
            is_shared: false,
            auth_token: booted.auth_token,
            team: booted.team,
            org: booted.org,
            auth_mode,
            mtls_cert_paths,
            _mtls_ca: mtls_ca_owned,
            _mtls_server_cert: booted.mtls_server_cert,
            _mtls_client_cert: booted.mtls_envoy_client_cert,
            _pg_container: Some(booted.pg_container),
            #[cfg(feature = "dev-oidc")]
            _mock_oidc: booted.mock_oidc,
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
    /// Per-test Envoy (envoy_harness mode) is always shut down.
    pub async fn shutdown(mut self) {
        // Always shut down per-test Envoy (envoy_harness mode)
        if let Some(mut envoy) = self.envoy_per_test.take() {
            info!("Shutting down per-test Envoy");
            envoy.shutdown();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

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

/// Dev-mode harness (shared, API-only)
pub async fn dev_harness(test_name: &str) -> anyhow::Result<TestHarness> {
    TestHarness::start(TestHarnessConfig::new(test_name).without_envoy()).await
}

/// Per-test Envoy harness: shared CP + DB, but a fresh Envoy per test.
///
/// Each test gets its own Envoy process with unique admin/listener ports and a
/// unique team name. The Envoy connects to the shared CP via xDS. This provides:
/// - Clean config_dump (only this test's resources)
/// - No port exhaustion (per-test port allocation)
/// - No state leakage between tests
/// - Deterministic absence verification after delete
///
/// The Envoy process is killed when the harness is dropped.
pub async fn envoy_harness(test_name: &str) -> anyhow::Result<TestHarness> {
    // Start with shared CP, no shared Envoy
    let mut harness = TestHarness::start(TestHarnessConfig::new(test_name).without_envoy()).await?;

    // Bail early if Envoy binary isn't available
    if !EnvoyHandle::is_available() {
        anyhow::bail!("Envoy binary not found — envoy_harness requires Envoy on PATH");
    }

    // Allocate unique ports for this test's Envoy using bind-test to avoid collisions
    let mut port_alloc = super::ports::PortAllocator::for_test(&format!("envoy_{}", test_name));
    let admin_port = port_alloc.reserve_labeled("envoy_admin");
    let listener_port = port_alloc.reserve_labeled("envoy_listener");
    let listener_secondary = port_alloc.reserve_labeled("envoy_listener2");
    // Drop allocator to release the ports before Envoy binds them
    drop(port_alloc);

    // Use the shared team (which already has a dataplane, user membership, etc.)
    // The per-test isolation comes from having a fresh Envoy process, not a separate team.
    let team = &harness.team;

    // Spawn per-test Envoy connecting to shared xDS with the shared team's metadata
    let envoy_config = EnvoyConfig::new(admin_port, super::shared_infra::SHARED_XDS_PORT)
        .with_node_id(test_name)
        .with_metadata(serde_json::json!({ "team": team }));

    let envoy = EnvoyHandle::start(envoy_config)?;
    with_timeout(TestTimeout::startup("Per-test Envoy ready"), async { envoy.wait_ready().await })
        .await?;
    info!(
        test = %test_name,
        admin_port,
        listener_port,
        team = %team,
        "Per-test Envoy ready"
    );

    // Update harness with per-test Envoy and ports
    harness.envoy_per_test = Some(envoy);
    harness.ports.envoy_admin = admin_port;
    harness.ports.listener = listener_port;
    harness.ports.listener_secondary = listener_secondary;

    Ok(harness)
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
