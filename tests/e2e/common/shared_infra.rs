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
    /// Dev mode: no Zitadel, JWTs minted by an in-process mock OIDC server. The
    /// CP's `authenticate` middleware validates mock-signed tokens via the same
    /// Zitadel validator used in prod.
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
    /// Mock OIDC issuer kept alive for the duration of the dev-mode test run.
    /// None in prod mode. Gated behind `dev-oidc` since the type only exists
    /// when that feature is on.
    #[cfg(feature = "dev-oidc")]
    #[allow(dead_code)]
    mock_oidc: Option<Arc<flowplane::dev::oidc_server::MockOidcServer>>,
}

// ---------------------------------------------------------------------------
// boot_cp — unified CP boot path (Task 5b Part 2)
//
// `boot_cp` is the ONE function that knows how to boot a flowplane control
// plane for an e2e test. Previously this logic lived in four parallel
// functions (`SharedInfrastructure::initialize_dev`,
// `SharedInfrastructure::initialize_prod_zitadel`, the `initialize`
// dispatcher, and `TestHarness::start_isolated`) — each one drifted relative
// to the others, which produced the fp-6yj (rustls provider not installed)
// and fp-4n5 (two mock OIDC servers) bugs. All startup invariants now live
// here.
//
// The ONLY branching point inside `boot_cp` is the `match cfg.auth_mode`
// that selects the identity issuer (dev mock OIDC vs real Zitadel). Every
// other dimension — port allocation, mTLS opt-in, envoy team metadata — is
// driven by the `CpBootConfig` passed in by the caller.
// ---------------------------------------------------------------------------

/// Fixed bind ports for a control plane boot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CpBootPorts {
    pub api: u16,
    pub xds: u16,
    pub envoy_admin: u16,
    pub listener: u16,
}

/// Mocks bundle flavor requested by the caller.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MocksFlavor {
    /// All mocks (echo + auth + ext_authz). Used by shared infra.
    All,
    /// Echo + auth (JWKS) only.
    Auth,
    /// Echo + ext_authz only.
    ExtAuthz,
    /// Basic echo only.
    Basic,
}

/// Configuration for a single CP boot.
pub(crate) struct CpBootConfig {
    pub auth_mode: E2eAuthMode,
    pub test_name: String,
    pub ports: CpBootPorts,
    pub enable_mtls: bool,
    pub enable_envoy: bool,
    /// Override SPIFFE team component in the mTLS client cert.
    pub mtls_team: Option<String>,
    /// Override SPIFFE proxy_id component in the mTLS client cert.
    pub mtls_proxy_id: Option<String>,
    /// Envoy node metadata `team` value. For shared-dev use `"default"`; for
    /// shared-prod use `E2E_SHARED_TEAM`; isolated callers may override.
    pub envoy_team: String,
    /// Which mock services bundle to start.
    pub mocks_flavor: MocksFlavor,
    /// Update `teams.envoy_admin_port` to the envoy admin port after envoy
    /// starts — used by shared infra so the stats API can reach envoy on
    /// the test-assigned port. Isolated callers should pass `false`.
    pub update_team_admin_port: bool,
}

/// Everything a caller needs after a successful CP boot. The caller decides
/// where the containers + state live (OnceLock statics for shared infra,
/// struct fields for isolated).
pub(crate) struct BootedCp {
    pub cp: ControlPlaneHandle,
    pub envoy: Option<EnvoyHandle>,
    pub mocks: MockServices,
    pub db_url: String,
    pub auth_mode: E2eAuthMode,
    pub auth_token: String,
    pub team: String,
    pub org: String,
    pub auth_config: E2eAuthConfig,
    pub zitadel_config: ZitadelTestConfig,
    pub mtls_ca: Option<Arc<TestCertificateAuthority>>,
    pub mtls_server_cert: Option<crate::tls::support::TestCertificateFiles>,
    pub mtls_envoy_client_cert: Option<crate::tls::support::TestCertificateFiles>,
    /// SPIFFE URI embedded in the envoy client cert, if mTLS was enabled.
    /// Callers that need to expose `MtlsCertPaths` rebuild the path bundle
    /// from `mtls_ca` + `mtls_server_cert` + `mtls_envoy_client_cert` + this URI.
    pub mtls_spiffe_uri: Option<String>,
    #[cfg(feature = "dev-oidc")]
    pub mock_oidc: Option<Arc<flowplane::dev::oidc_server::MockOidcServer>>,
    pub pg_container: ContainerAsync<Postgres>,
    pub zitadel_container: Option<ContainerAsync<GenericImage>>,
}

/// Boot a flowplane control plane for an e2e test.
///
/// See the module-level comment above the CpBootConfig definition for the
/// rationale behind having a single function. Hard rule: the only branch
/// inside this function is the identity-issuer match.
pub(crate) async fn boot_cp(cfg: CpBootConfig) -> anyhow::Result<BootedCp> {
    // Rustls process-level crypto provider — required before any TlsAcceptor
    // is constructed. Historical bug (fp-6yj): the isolated path didn't
    // install this, so every mTLS test panicked at TLS acceptor construction.
    // Installing here unconditionally guarantees every code path that boots
    // a CP passes through this line first.
    use rustls::crypto::{ring, CryptoProvider};
    if CryptoProvider::get_default().is_none() {
        let _ = ring::default_provider().install_default();
    }

    info!(
        auth_mode = ?cfg.auth_mode,
        test = %cfg.test_name,
        api_port = cfg.ports.api,
        xds_port = cfg.ports.xds,
        enable_mtls = cfg.enable_mtls,
        enable_envoy = cfg.enable_envoy,
        "boot_cp: starting control plane"
    );

    // Start PostgreSQL container (common to both modes).
    info!("Starting PostgreSQL container...");
    let pg_container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start PostgreSQL container: {}", e))?;

    let pg_host = pg_container
        .get_host()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get PG host: {}", e))?
        .to_string();
    let pg_port = pg_container
        .get_host_port_ipv4(5432)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get PG port: {}", e))?;
    let db_url = format!("postgresql://postgres:postgres@{}:{}/postgres", pg_host, pg_port);
    info!(db_url = %db_url, "PostgreSQL container ready");

    match cfg.auth_mode {
        // ---------------------------------------------------------------
        // Dev mode: mock OIDC issuer, seeded default user/team/dataplane,
        // bearer-token auth via issuer-minted JWT.
        // ---------------------------------------------------------------
        E2eAuthMode::Dev => {
            info!("boot_cp: dev mode — using mock OIDC issuer");

            // Auth-mode selector vars (CP runtime side).
            std::env::set_var("FLOWPLANE_AUTH_MODE", "dev");
            std::env::set_var("FLOWPLANE_COOKIE_SECURE", "false");
            std::env::set_var("FLOWPLANE_BASE_URL", format!("http://localhost:{}", cfg.ports.api));

            // Start mock OIDC issuer BEFORE the CP so we can wire it into
            // ControlPlaneConfig via `with_dev_oidc_mock` — the CP derives
            // its ZitadelAuthState from the same mock instance rather than
            // spawning its own. One mock, one signing key (fp-4n5 Part 1).
            #[cfg(feature = "dev-oidc")]
            let (mock_oidc, dev_token): (
                Option<Arc<flowplane::dev::oidc_server::MockOidcServer>>,
                String,
            ) = {
                use flowplane::dev::oidc_server::{MockOidcConfig, MockOidcServer};
                let mock = MockOidcServer::start(MockOidcConfig::default())
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to start mock OIDC server: {e}"))?;
                let mock = Arc::new(mock);
                let token = mock
                    .issue_token_for_sub(flowplane::auth::dev_token::DEV_USER_SUB)
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to issue mock OIDC token: {e}"))?;
                (Some(mock), token)
            };
            #[cfg(not(feature = "dev-oidc"))]
            let dev_token: String = {
                return Err(anyhow::anyhow!(
                    "Dev mode E2E tests require the `dev-oidc` cargo feature"
                ));
            };

            // mTLS for dev is rare but supported in isolated mode. Generate
            // per-boot CA + server cert + client cert if requested.
            let MtlsMaterial {
                tls_config: xds_tls_config,
                ca: mtls_ca,
                server_cert: mtls_server_cert,
                client_cert: envoy_client_cert,
                spiffe_uri: mtls_spiffe_uri,
            } = issue_mtls_material(&cfg)?;

            // Mock services for the test.
            let mocks = start_mocks(cfg.mocks_flavor).await;
            info!(echo = %mocks.echo_endpoint(), "Mock services started");

            // Build ControlPlaneConfig.
            #[allow(unused_mut)]
            let mut cp_config = ControlPlaneConfig::new(
                db_url.clone(),
                cfg.ports.api,
                cfg.ports.xds,
                cfg.ports.listener,
            );
            if let Some(tls) = xds_tls_config {
                cp_config = cp_config.with_xds_tls(tls);
            }
            #[cfg(feature = "dev-oidc")]
            if let Some(ref mock) = mock_oidc {
                cp_config = cp_config.with_dev_oidc_mock(mock.clone());
            }

            let cp = with_timeout(TestTimeout::startup("Starting control plane"), async {
                ControlPlaneHandle::start(cp_config).await
            })
            .await?;
            with_timeout(TestTimeout::startup("Control plane ready"), async {
                cp.wait_ready().await
            })
            .await?;
            info!(api = %cp.api_addr, xds = %cp.xds_addr, "Control plane ready (dev mode)");

            // Seed dev org/team/user/dataplane so the mock-issued JWT
            // (sub=DEV_USER_SUB) resolves to a real user row with memberships.
            {
                use flowplane::storage::create_pool;
                let db_cfg = flowplane::config::DatabaseConfig {
                    url: db_url.clone(),
                    auto_migrate: false,
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

            // Start envoy if requested and available.
            let envoy = start_envoy_if_available(
                &cfg,
                &cfg.envoy_team,
                envoy_client_cert.as_ref(),
                mtls_ca.as_deref(),
                &db_url,
            )
            .await?;

            // Dummy ZitadelTestConfig for backward compat.
            let dummy_zitadel_config = ZitadelTestConfig {
                base_url: String::new(),
                admin_pat: String::new(),
                project_id: String::new(),
                spa_client_id: String::new(),
            };

            Ok(BootedCp {
                cp,
                envoy,
                mocks,
                db_url,
                auth_mode: E2eAuthMode::Dev,
                auth_token: dev_token.clone(),
                team: "default".to_string(),
                org: "dev-org".to_string(),
                auth_config: E2eAuthConfig::Dev { token: dev_token },
                zitadel_config: dummy_zitadel_config,
                mtls_ca,
                mtls_server_cert,
                mtls_envoy_client_cert: envoy_client_cert,
                mtls_spiffe_uri,
                #[cfg(feature = "dev-oidc")]
                mock_oidc,
                pg_container,
                zitadel_container: None,
            })
        }

        // ---------------------------------------------------------------
        // Prod mode: real Zitadel container + tenant org bootstrap.
        // ---------------------------------------------------------------
        E2eAuthMode::Prod => {
            info!("boot_cp: prod mode — using real Zitadel container");

            // Start Zitadel container (shares PG instance via separate DB).
            info!("Starting Zitadel container...");
            let (zitadel_container, zitadel_port, machinekey_dir) =
                zitadel::start_zitadel_container(&pg_host, pg_port).await?;
            let zitadel_base_url = format!("http://localhost:{}", zitadel_port);

            zitadel::wait_for_zitadel_ready(&zitadel_base_url).await?;

            let admin_pat = zitadel::read_admin_pat(&machinekey_dir).await?;
            zitadel::validate_pat(&zitadel_base_url, &admin_pat).await?;

            let zitadel_config =
                zitadel::bootstrap_zitadel(&zitadel_base_url, &admin_pat, zitadel_port).await?;

            // CP runtime env vars for Zitadel auth.
            zitadel::set_cp_env_vars(&zitadel_config);

            // mTLS material.
            let MtlsMaterial {
                tls_config: xds_tls_config,
                ca: mtls_ca,
                server_cert: mtls_server_cert,
                client_cert: envoy_client_cert,
                spiffe_uri: mtls_spiffe_uri,
            } = issue_mtls_material(&cfg)?;

            // Mock services.
            let mocks = start_mocks(cfg.mocks_flavor).await;
            info!(echo = %mocks.echo_endpoint(), "Mock services started");

            let mut cp_config = ControlPlaneConfig::new(
                db_url.clone(),
                cfg.ports.api,
                cfg.ports.xds,
                cfg.ports.listener,
            );
            if let Some(tls) = xds_tls_config {
                cp_config = cp_config.with_xds_tls(tls);
            }

            let cp = with_timeout(TestTimeout::startup("Starting control plane"), async {
                ControlPlaneHandle::start(cp_config).await
            })
            .await?;
            with_timeout(TestTimeout::startup("Control plane ready"), async {
                cp.wait_ready().await
            })
            .await?;
            info!(
                api = %cp.api_addr,
                xds = %cp.xds_addr,
                mtls = cfg.enable_mtls,
                "Control plane ready (prod mode)"
            );

            // Wait for superadmin seeding (CP seeds on startup).
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

            // Clean up stale dataplanes from previous runs.
            info!("Cleaning up stale dataplanes from previous runs...");
            let deleted = api.delete_all_dataplanes(&superadmin_token).await.unwrap_or(0);
            if deleted > 0 {
                info!(count = deleted, "Deleted stale dataplanes");
            }

            // Tenant org + shared team + dataplane for resource-scoped tests.
            info!("Creating tenant org and shared team for E2E tests...");
            let tenant_org =
                with_timeout(TestTimeout::default_with_label("Create e2e-tenant org"), async {
                    api.create_organization_idempotent(
                        &superadmin_token,
                        "e2e-tenant",
                        "E2E Tenant Org",
                        Some("Tenant org for E2E tests"),
                    )
                    .await
                })
                .await?;
            let tenant_org_id = tenant_org.id;
            let tenant_org_name = tenant_org.name;
            info!(org = %tenant_org_name, id = %tenant_org_id, "Tenant org ready");

            // Make superadmin an org admin of the tenant org.
            let session = api.get_auth_session(&superadmin_token).await?;
            super::api_client::ensure_org_admin_via_db(&db_url, &session.user_id, &tenant_org_id)
                .await?;
            info!(
                user_id = %session.user_id,
                org = %tenant_org_name,
                "Superadmin org admin membership set"
            );

            // Wait for permission cache (TTL=2s in E2E mode).
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let shared_team =
                with_timeout(TestTimeout::default_with_label("Create shared team"), async {
                    api.create_team_idempotent(
                        &superadmin_token,
                        E2E_SHARED_TEAM,
                        Some("Shared team for E2E tests"),
                        &tenant_org_id,
                    )
                    .await
                })
                .await?;
            info!(team = %shared_team.name, "Shared team created");

            let _dataplane =
                with_timeout(TestTimeout::default_with_label("Create shared dataplane"), async {
                    api.create_dataplane_idempotent(
                        &superadmin_token,
                        E2E_SHARED_TEAM,
                        &super::api_client::CreateDataplaneRequest {
                            name: format!("{}-dataplane", E2E_SHARED_TEAM),
                            gateway_host: Some("127.0.0.1".to_string()),
                            description: Some("Shared dataplane for E2E tests".to_string()),
                        },
                    )
                    .await
                })
                .await?;
            info!("Shared dataplane created");

            // Start envoy if requested.
            let envoy = start_envoy_if_available(
                &cfg,
                &cfg.envoy_team,
                envoy_client_cert.as_ref(),
                mtls_ca.as_deref(),
                &db_url,
            )
            .await?;

            Ok(BootedCp {
                cp,
                envoy,
                mocks,
                db_url,
                auth_mode: E2eAuthMode::Prod,
                auth_token: superadmin_token,
                team: E2E_SHARED_TEAM.to_string(),
                org: tenant_org_name,
                auth_config: E2eAuthConfig::Zitadel(zitadel_config.clone()),
                zitadel_config,
                mtls_ca,
                mtls_server_cert,
                mtls_envoy_client_cert: envoy_client_cert,
                mtls_spiffe_uri,
                #[cfg(feature = "dev-oidc")]
                mock_oidc: None,
                pg_container,
                zitadel_container: Some(zitadel_container),
            })
        }
    }
}

/// Bundle of mTLS material produced by [`issue_mtls_material`]. All fields are
/// `None` when `cfg.enable_mtls` is false.
struct MtlsMaterial {
    tls_config: Option<flowplane::config::XdsTlsConfig>,
    ca: Option<Arc<TestCertificateAuthority>>,
    server_cert: Option<crate::tls::support::TestCertificateFiles>,
    client_cert: Option<crate::tls::support::TestCertificateFiles>,
    spiffe_uri: Option<String>,
}

/// Issue the mTLS CA + server cert + envoy client cert if `cfg.enable_mtls`
/// is set. Returns an empty [`MtlsMaterial`] otherwise.
fn issue_mtls_material(cfg: &CpBootConfig) -> anyhow::Result<MtlsMaterial> {
    if !cfg.enable_mtls {
        return Ok(MtlsMaterial {
            tls_config: None,
            ca: None,
            server_cert: None,
            client_cert: None,
            spiffe_uri: None,
        });
    }

    let ca_name = format!("Flowplane E2E Test CA ({})", cfg.test_name);
    let ca = TestCertificateAuthority::new(&ca_name, time::Duration::days(1))?;
    let server_cert = ca.issue_server_cert(&["localhost"], time::Duration::days(1))?;

    let team_name = cfg.mtls_team.clone().unwrap_or_else(|| unique_team_name(&cfg.test_name));
    let proxy_id = cfg.mtls_proxy_id.clone().unwrap_or_else(|| "test-proxy".to_string());
    let spiffe_uri =
        TestCertificateAuthority::build_spiffe_uri("flowplane.local", &team_name, &proxy_id)?;
    let client_cert = ca.issue_client_cert(&spiffe_uri, &proxy_id, time::Duration::days(1))?;

    let xds_tls = flowplane::config::XdsTlsConfig {
        cert_path: server_cert.cert_path.to_string_lossy().to_string(),
        key_path: server_cert.key_path.to_string_lossy().to_string(),
        client_ca_path: Some(ca.ca_cert_path.to_string_lossy().to_string()),
        require_client_cert: true,
    };

    info!(
        ca_path = %ca.ca_cert_path.display(),
        server_cert = %server_cert.cert_path.display(),
        spiffe_uri = %spiffe_uri,
        "Generated mTLS certificates"
    );

    Ok(MtlsMaterial {
        tls_config: Some(xds_tls),
        ca: Some(Arc::new(ca)),
        server_cert: Some(server_cert),
        client_cert: Some(client_cert),
        spiffe_uri: Some(spiffe_uri),
    })
}

async fn start_mocks(flavor: MocksFlavor) -> MockServices {
    match flavor {
        MocksFlavor::All => MockServices::start_all().await,
        MocksFlavor::Auth => MockServices::start_with_auth().await,
        MocksFlavor::ExtAuthz => MockServices::start_with_ext_authz().await,
        MocksFlavor::Basic => MockServices::start_basic().await,
    }
}

async fn start_envoy_if_available(
    cfg: &CpBootConfig,
    envoy_team: &str,
    client_cert: Option<&crate::tls::support::TestCertificateFiles>,
    ca: Option<&TestCertificateAuthority>,
    db_url: &str,
) -> anyhow::Result<Option<EnvoyHandle>> {
    if !cfg.enable_envoy {
        return Ok(None);
    }
    if !EnvoyHandle::is_available() {
        info!("Envoy binary not found — tests will skip proxy verification");
        return Ok(None);
    }

    let mut envoy_config = EnvoyConfig::new(cfg.ports.envoy_admin, cfg.ports.xds)
        .with_metadata(serde_json::json!({ "team": envoy_team }));

    if let (Some(client_cert), Some(ca)) = (client_cert, ca) {
        let tls_config = EnvoyXdsTlsConfig {
            ca_cert: ca.ca_cert_path.clone(),
            client_cert: Some(client_cert.cert_path.clone()),
            client_key: Some(client_cert.key_path.clone()),
        };
        envoy_config = envoy_config.with_xds_tls(tls_config);
        info!(
            client_cert = %client_cert.cert_path.display(),
            "Configured envoy with mTLS client certificate"
        );
    }

    let envoy = EnvoyHandle::start(envoy_config)?;
    with_timeout(TestTimeout::startup("Envoy ready"), async { envoy.wait_ready().await }).await?;
    info!(admin_port = cfg.ports.envoy_admin, "Envoy ready");

    if cfg.update_team_admin_port && cfg.auth_mode == E2eAuthMode::Dev {
        use flowplane::storage::create_pool;
        use sqlx::Executor;
        let db_cfg = flowplane::config::DatabaseConfig {
            url: db_url.to_string(),
            auto_migrate: false,
            max_connections: 2,
            min_connections: 1,
            ..Default::default()
        };
        let pool = create_pool(&db_cfg).await?;
        pool.execute(
            sqlx::query("UPDATE teams SET envoy_admin_port = $1 WHERE name = 'default'")
                .bind(cfg.ports.envoy_admin as i64),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to update team envoy_admin_port: {}", e))?;
        info!(port = cfg.ports.envoy_admin, "Updated default team envoy_admin_port");
    }

    Ok(Some(envoy))
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
            let handle = std::thread::spawn(|| {
                let runtime = SHARED_RUNTIME.get_or_init(|| {
                    tokio::runtime::Builder::new_multi_thread()
                        .enable_all()
                        .thread_name("shared-infra")
                        .build()
                        .expect("Failed to create shared infrastructure runtime")
                });

                match runtime.block_on(build_shared_infra()) {
                    Ok(infra) => {
                        let _ = SHARED_INFRA.set(infra);
                        let _ = INIT_RESULT.set(Ok(()));
                    }
                    Err(e) => {
                        let _ = INIT_RESULT.set(Err(e.to_string()));
                    }
                }
            });

            let _ = handle.join();
        });

        match INIT_RESULT.get() {
            Some(Ok(())) => Ok(SHARED_INFRA.get().expect("Init succeeded but no infra")),
            Some(Err(e)) => Err(anyhow::anyhow!("Shared infrastructure init failed: {}", e)),
            None => Err(anyhow::anyhow!("Shared infrastructure init incomplete")),
        }
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

/// Shared-infra wrapper around boot_cp: performs cleanup_stale_processes,
/// builds the shared-mode CpBootConfig (fixed SHARED_* ports, FLOWPLANE_E2E_MTLS
/// opt-in), calls boot_cp, and moves the returned containers into the
/// `SHARED_PG_CONTAINER` / `SHARED_ZITADEL_CONTAINER` statics so they live
/// for the entire test run.
async fn build_shared_infra() -> anyhow::Result<SharedInfrastructure> {
    let auth_mode = e2e_auth_mode();
    info!(?auth_mode, "Initializing shared E2E infrastructure...");

    // Clean up any stale processes from previous test runs.
    cleanup_stale_processes();

    // FLOWPLANE_E2E_MTLS=1 opt-in applies only to shared prod mode (it's
    // always been a prod-only knob; shared dev doesn't use mTLS).
    let enable_mtls = auth_mode == E2eAuthMode::Prod
        && std::env::var("FLOWPLANE_E2E_MTLS").ok().as_deref() == Some("1");
    if enable_mtls {
        info!("mTLS enabled for shared infrastructure (FLOWPLANE_E2E_MTLS=1)");
    }

    let envoy_team = match auth_mode {
        E2eAuthMode::Dev => "default".to_string(),
        E2eAuthMode::Prod => E2E_SHARED_TEAM.to_string(),
    };

    let cfg = CpBootConfig {
        auth_mode,
        test_name: "shared-e2e".to_string(),
        ports: CpBootPorts {
            api: SHARED_API_PORT,
            xds: SHARED_XDS_PORT,
            envoy_admin: SHARED_ENVOY_ADMIN_PORT,
            listener: SHARED_LISTENER_PORT,
        },
        enable_mtls,
        enable_envoy: true,
        mtls_team: None,
        mtls_proxy_id: Some("shared-envoy".to_string()),
        envoy_team,
        mocks_flavor: MocksFlavor::All,
        update_team_admin_port: true,
    };

    let booted = boot_cp(cfg).await?;

    // Containers must outlive every test — stash in the dedicated statics.
    let _ = SHARED_PG_CONTAINER.set(booted.pg_container);
    if let Some(container) = booted.zitadel_container {
        let _ = SHARED_ZITADEL_CONTAINER.set(container);
    }

    Ok(SharedInfrastructure {
        cp: booted.cp,
        envoy: booted.envoy,
        mocks: booted.mocks,
        db_url: booted.db_url,
        zitadel_config: booted.zitadel_config,
        auth_mode: booted.auth_mode,
        auth_token: booted.auth_token,
        team: booted.team,
        org: booted.org,
        auth_config: booted.auth_config,
        mtls_ca: booted.mtls_ca,
        mtls_server_cert: booted.mtls_server_cert,
        mtls_envoy_client_cert: booted.mtls_envoy_client_cert,
        #[cfg(feature = "dev-oidc")]
        mock_oidc: booted.mock_oidc,
    })
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
