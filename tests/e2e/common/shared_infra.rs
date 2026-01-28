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

use std::path::PathBuf;
use std::sync::OnceLock;

use tracing::info;

use super::api_client::{ApiClient, TEST_EMAIL, TEST_NAME, TEST_PASSWORD};
use super::control_plane::{ControlPlaneConfig, ControlPlaneHandle};
use super::envoy::{EnvoyConfig, EnvoyHandle};
use super::mocks::MockServices;
use super::timeout::{with_timeout, TestTimeout};

/// Fixed ports for shared infrastructure
pub const SHARED_API_PORT: u16 = 19080;
pub const SHARED_XDS_PORT: u16 = 19010;
pub const SHARED_ENVOY_ADMIN_PORT: u16 = 19901;
pub const SHARED_LISTENER_PORT: u16 = 19000;

/// Shared infrastructure singleton - initialized lazily with dedicated runtime
static SHARED_INFRA: OnceLock<SharedInfrastructure> = OnceLock::new();

/// Initialization result for waiting threads
static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

/// Temp directory for shared DB - must live for entire test run
static TEMP_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

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
    /// Database path
    pub db_path: PathBuf,
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

        // Create temp directory that lives for entire test run
        let temp_dir =
            TEMP_DIR.get_or_init(|| tempfile::tempdir().expect("Failed to create temp dir"));
        let db_path = temp_dir.path().join("shared-e2e.db");

        // Start mock services with full support (auth + ext_authz)
        let mocks = MockServices::start_all().await;
        info!(echo = %mocks.echo_endpoint(), "Mock services started");

        // Start control plane
        let cp_config = ControlPlaneConfig::new(
            db_path.clone(),
            SHARED_API_PORT,
            SHARED_XDS_PORT,
            SHARED_LISTENER_PORT,
        );

        let cp = with_timeout(TestTimeout::startup("Starting shared control plane"), async {
            ControlPlaneHandle::start(cp_config).await
        })
        .await?;

        with_timeout(TestTimeout::startup("Shared control plane ready"), async {
            cp.wait_ready().await
        })
        .await?;
        info!(api = %cp.api_addr, xds = %cp.xds_addr, "Shared control plane ready");

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
        let envoy = if EnvoyHandle::is_available() {
            let envoy_config = EnvoyConfig::new(SHARED_ENVOY_ADMIN_PORT, SHARED_XDS_PORT);
            let envoy = EnvoyHandle::start(envoy_config)?;

            with_timeout(TestTimeout::startup("Shared Envoy ready"), async {
                envoy.wait_ready().await
            })
            .await?;
            info!(admin_port = SHARED_ENVOY_ADMIN_PORT, "Shared Envoy ready");

            Some(envoy)
        } else {
            info!("Envoy binary not found - tests will skip proxy verification");
            None
        };

        Ok(SharedInfrastructure { cp, envoy, mocks, db_path })
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
