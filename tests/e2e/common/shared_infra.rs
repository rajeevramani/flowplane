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
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use tracing::info;

use super::api_client::ApiClient;
use super::control_plane::{ControlPlaneConfig, ControlPlaneHandle};
use super::envoy::{EnvoyConfig, EnvoyHandle, EnvoyXdsTlsConfig};
use super::mocks::MockServices;
use super::timeout::{with_timeout, TestTimeout};
use super::zitadel::{self, ZitadelTestConfig};
use crate::tls::support::TestCertificateAuthority;

// ---------------------------------------------------------------------------
// E2E auth mode
// ---------------------------------------------------------------------------

/// E2E test auth mode — determines how SharedInfrastructure initializes auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2eAuthMode {
    /// Dev mode: no Zitadel, bearer token auth via FLOWPLANE_DEV_TOKEN
    Dev,
    /// Prod mode with real Zitadel container
    Prod,
}

/// Auth configuration produced during initialization.
#[derive(Debug, Clone)]
pub enum E2eAuthConfig {
    Dev { token: String },
    Zitadel(ZitadelTestConfig),
}

/// Read `FLOWPLANE_E2E_AUTH_MODE` and return the corresponding enum variant.
///
/// - `"dev"` → `E2eAuthMode::Dev`
/// - `"prod"` or unset → `E2eAuthMode::Prod`
pub fn e2e_auth_mode() -> E2eAuthMode {
    match std::env::var("FLOWPLANE_E2E_AUTH_MODE").as_deref() {
        Ok("dev") => E2eAuthMode::Dev,
        _ => E2eAuthMode::Prod,
    }
}

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

/// Zitadel container for shared auth - must live for entire test run
static SHARED_ZITADEL_CONTAINER: OnceLock<ContainerAsync<GenericImage>> = OnceLock::new();

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
    /// Zitadel configuration for OIDC token acquisition (kept for backward compat)
    pub zitadel_config: ZitadelTestConfig,
    /// Auth mode used for this shared instance
    pub auth_mode: E2eAuthMode,
    /// Auth token for tests to use (dev token or JWT)
    pub auth_token: String,
    /// Team name for test resources
    pub team: String,
    /// Organization name
    pub org: String,
    /// General auth config (replaces zitadel_config conceptually)
    pub auth_config: E2eAuthConfig,
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

    /// Internal initialization - runs in the dedicated shared runtime.
    ///
    /// Branches on `FLOWPLANE_E2E_AUTH_MODE`:
    /// - `dev`  → skip Zitadel, use bearer dev token
    /// - `prod` / unset → full Zitadel container (existing path)
    async fn initialize() -> anyhow::Result<SharedInfrastructure> {
        let mode = e2e_auth_mode();
        info!(?mode, "Initializing shared E2E infrastructure...");

        // Clean up any stale processes from previous test runs
        cleanup_stale_processes();

        // Install Rustls crypto provider (required for TLS to work)
        use rustls::crypto::{ring, CryptoProvider};
        if CryptoProvider::get_default().is_none() {
            ring::default_provider()
                .install_default()
                .expect("install ring crypto provider for E2E tests");
        }

        match mode {
            E2eAuthMode::Dev => Self::initialize_dev().await,
            E2eAuthMode::Prod => Self::initialize_prod_zitadel().await,
        }
    }

    /// Dev mode initialization: no Zitadel, bearer token auth.
    async fn initialize_dev() -> anyhow::Result<SharedInfrastructure> {
        info!("Dev mode: skipping Zitadel, using bearer token auth");

        // Start PostgreSQL container
        info!("Starting PostgreSQL container for shared E2E database...");
        let container = Postgres::default()
            .with_tag("17-alpine")
            .start()
            .await
            .expect("Failed to start PostgreSQL container for shared E2E");

        let pg_host = container
            .get_host()
            .await
            .expect("Failed to get PostgreSQL container host")
            .to_string();
        let pg_port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Failed to get PostgreSQL container port");
        let db_url = format!("postgresql://postgres:postgres@{}:{}/postgres", pg_host, pg_port);
        info!(db_url = %db_url, "PostgreSQL container ready for shared E2E");

        let _ = SHARED_PG_CONTAINER.set(container);

        // Generate dev token and set env vars BEFORE starting CP
        let dev_token = flowplane::auth::dev_token::generate_dev_token();
        std::env::set_var("FLOWPLANE_AUTH_MODE", "dev");
        std::env::set_var("FLOWPLANE_DEV_TOKEN", &dev_token);
        std::env::set_var("FLOWPLANE_COOKIE_SECURE", "false");
        std::env::set_var("FLOWPLANE_BASE_URL", format!("http://localhost:{}", SHARED_API_PORT));

        // Start mock services
        let mocks = MockServices::start_all().await;
        info!(echo = %mocks.echo_endpoint(), "Mock services started");

        // Start control plane (no mTLS in dev mode for simplicity)
        let cp_config = ControlPlaneConfig::new(
            db_url.clone(),
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
        info!(api = %cp.api_addr, xds = %cp.xds_addr, "Shared control plane ready (dev mode)");

        // Seed dev resources (org, team, user, dataplane)
        // The CP startup path seeds these when auth_mode=dev, but we also call
        // seed_dev_resources explicitly via the DB pool to be sure.
        {
            use flowplane::storage::create_pool;
            let db_cfg = flowplane::config::DatabaseConfig {
                url: db_url.clone(),
                auto_migrate: false, // CP already migrated
                max_connections: 2,
                min_connections: 1,
                ..Default::default()
            };
            let pool = create_pool(&db_cfg).await?;
            flowplane::startup::seed_dev_resources(&pool)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to seed dev resources: {}", e))?;
            info!("Dev resources seeded");
        }

        // Start Envoy if available.
        // Dev mode uses team "default" (matching seed_dev_resources) so Envoy
        // can see resources created via the expose API under the default team.
        let envoy = if EnvoyHandle::is_available() {
            let envoy_config = EnvoyConfig::new(SHARED_ENVOY_ADMIN_PORT, SHARED_XDS_PORT)
                .with_metadata(serde_json::json!({
                    "team": "default"
                }));

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

        // Dummy ZitadelTestConfig for backward compat (unused in dev mode)
        let dummy_zitadel_config = ZitadelTestConfig {
            base_url: String::new(),
            admin_pat: String::new(),
            project_id: String::new(),
            spa_client_id: String::new(),
        };

        Ok(SharedInfrastructure {
            cp,
            envoy,
            mocks,
            db_url,
            zitadel_config: dummy_zitadel_config,
            auth_mode: E2eAuthMode::Dev,
            auth_token: dev_token.clone(),
            team: "default".to_string(),
            org: "dev-org".to_string(),
            auth_config: E2eAuthConfig::Dev { token: dev_token },
            mtls_ca: None,
            mtls_server_cert: None,
            mtls_envoy_client_cert: None,
        })
    }

    /// Prod initialization: full Zitadel container.
    async fn initialize_prod_zitadel() -> anyhow::Result<SharedInfrastructure> {
        // Check if mTLS is requested via environment variable
        let enable_mtls = std::env::var("FLOWPLANE_E2E_MTLS").ok().as_deref() == Some("1");

        if enable_mtls {
            info!("mTLS enabled for shared infrastructure (FLOWPLANE_E2E_MTLS=1)");
        }

        // Start PostgreSQL container for shared E2E database
        // Use PG 17 to match docker-compose and satisfy Zitadel v4+ requirements
        info!("Starting PostgreSQL container for shared E2E database...");
        let container = Postgres::default()
            .with_tag("17-alpine")
            .start()
            .await
            .expect("Failed to start PostgreSQL container for shared E2E");

        let pg_host_obj =
            container.get_host().await.expect("Failed to get PostgreSQL container host");
        let pg_host = pg_host_obj.to_string();
        let pg_port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Failed to get PostgreSQL container port");
        let db_url = format!("postgresql://postgres:postgres@{}:{}/postgres", pg_host, pg_port);
        info!(db_url = %db_url, "PostgreSQL container ready for shared E2E");

        // Store container in static to keep it alive for entire test run
        let _ = SHARED_PG_CONTAINER.set(container);

        // Start Zitadel container (shares the PostgreSQL instance via separate DB)
        info!("Starting Zitadel container...");
        let (zitadel_container, zitadel_port, machinekey_dir) =
            zitadel::start_zitadel_container(&pg_host, pg_port).await?;
        let zitadel_base_url = format!("http://localhost:{}", zitadel_port);

        // Wait for Zitadel to be ready
        zitadel::wait_for_zitadel_ready(&zitadel_base_url).await?;

        // Read admin PAT from host-side machinekey directory
        let admin_pat = zitadel::read_admin_pat(&machinekey_dir).await?;
        zitadel::validate_pat(&zitadel_base_url, &admin_pat).await?;

        // Bootstrap Zitadel (create project, SPA app, email action)
        let zitadel_config =
            zitadel::bootstrap_zitadel(&zitadel_base_url, &admin_pat, zitadel_port).await?;

        // Set CP environment variables for Zitadel auth before starting CP
        zitadel::set_cp_env_vars(&zitadel_config);

        // Store Zitadel container in static to keep it alive
        let _ = SHARED_ZITADEL_CONTAINER.set(zitadel_container);

        // Initialize mTLS CA if enabled
        let (mtls_ca, mtls_server_cert, xds_tls_config) = if enable_mtls {
            let ca = TestCertificateAuthority::new(
                "Flowplane E2E Shared Test CA",
                time::Duration::days(1),
            )?;

            // Issue server cert for CP xDS
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

        // Wait for superadmin to be seeded (CP seeds it on startup)
        let api_url = format!("http://{}", cp.api_addr);
        let api = ApiClient::new(&api_url);

        info!("Waiting for superadmin to be seeded by CP...");
        let superadmin_token = {
            let mut attempts = 0;
            loop {
                match zitadel::obtain_human_token(
                    &zitadel_config,
                    zitadel::SUPERADMIN_EMAIL,
                    zitadel::SUPERADMIN_PASSWORD,
                )
                .await
                {
                    Ok(token) => break token,
                    Err(e) => {
                        attempts += 1;
                        if attempts >= 30 {
                            anyhow::bail!(
                                "Failed to obtain superadmin token after 30 attempts: {}",
                                e
                            );
                        }
                        if attempts % 5 == 0 {
                            info!(attempt = attempts, "Superadmin not ready yet, retrying...");
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        };
        info!("Superadmin JWT token obtained");

        // Clean up stale dataplanes from previous test runs
        info!("Cleaning up stale dataplanes from previous runs...");
        let deleted = api.delete_all_dataplanes(&superadmin_token).await.unwrap_or(0);
        if deleted > 0 {
            info!(count = deleted, "Deleted stale dataplanes");
        }

        // Start Envoy if available
        let (envoy, mtls_envoy_client_cert) = if EnvoyHandle::is_available() {
            let mut envoy_config = EnvoyConfig::new(SHARED_ENVOY_ADMIN_PORT, SHARED_XDS_PORT)
                .with_metadata(serde_json::json!({
                    "team": E2E_SHARED_TEAM
                }));

            let envoy_client_cert = if let Some(ref ca) = mtls_ca {
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
            auth_mode: E2eAuthMode::Prod,
            auth_token: superadmin_token,
            team: E2E_SHARED_TEAM.to_string(),
            org: "platform".to_string(),
            auth_config: E2eAuthConfig::Zitadel(zitadel_config.clone()),
            zitadel_config,
            mtls_ca,
            mtls_server_cert,
            mtls_envoy_client_cert,
        })
    }

    /// Get a token for the admin/superuser persona.
    /// Dev: returns the dev bearer token
    /// Prod: calls obtain_human_token with superadmin creds
    pub async fn get_admin_token(&self) -> anyhow::Result<String> {
        match &self.auth_mode {
            E2eAuthMode::Dev => Ok(self.auth_token.clone()),
            E2eAuthMode::Prod => {
                zitadel::obtain_human_token(
                    &self.zitadel_config,
                    zitadel::SUPERADMIN_EMAIL,
                    zitadel::SUPERADMIN_PASSWORD,
                )
                .await
            }
        }
    }

    /// Get a token for a specific user persona.
    /// Dev: returns the default auth_token (single-user mode)
    /// Prod: calls obtain_human_token with provided creds
    pub async fn get_user_token(&self, email: &str, password: &str) -> anyhow::Result<String> {
        match &self.auth_mode {
            E2eAuthMode::Dev => Ok(self.auth_token.clone()),
            E2eAuthMode::Prod => {
                zitadel::obtain_human_token(&self.zitadel_config, email, password).await
            }
        }
    }

    /// Get a token for a machine/agent user.
    /// Dev: returns the default auth_token
    /// Prod: calls obtain_agent_token with client credentials
    pub async fn get_agent_token(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> anyhow::Result<String> {
        match &self.auth_mode {
            E2eAuthMode::Dev => Ok(self.auth_token.clone()),
            E2eAuthMode::Prod => {
                zitadel::obtain_agent_token(&self.zitadel_config, client_id, client_secret).await
            }
        }
    }

    /// Create a test user in the auth provider.
    /// Dev: no-op (returns a fake user ID)
    /// Prod: calls create_human_user
    pub async fn create_test_user(
        &self,
        email: &str,
        first_name: &str,
        last_name: &str,
        password: &str,
    ) -> anyhow::Result<String> {
        match &self.auth_mode {
            E2eAuthMode::Dev => {
                // Return a fake user ID — user doesn't exist in auth provider
                Ok(format!("mock-user-{}", email))
            }
            E2eAuthMode::Prod => {
                zitadel::create_human_user(
                    &self.zitadel_config.base_url,
                    &self.zitadel_config.admin_pat,
                    email,
                    first_name,
                    last_name,
                    password,
                )
                .await
            }
        }
    }

    /// Check if running in a mode that supports multi-user testing
    pub fn supports_multi_user(&self) -> bool {
        matches!(self.auth_mode, E2eAuthMode::Prod)
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
