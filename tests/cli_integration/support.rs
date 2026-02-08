// NOTE: Requires PostgreSQL - disabled until Phase 4
#![cfg(feature = "postgres_tests")]

//! Support utilities for CLI integration tests
//!
//! Provides test fixtures, server setup, and helper functions for testing
//! CLI commands against a running Flowplane server.

use flowplane::{
    auth::{
        token_service::{TokenSecretResponse, TokenService},
        validation::CreateTokenRequest,
    },
    config::SimpleXdsConfig,
    storage::{repository::AuditLogRepository, DbPool},
    xds::XdsState,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;

#[path = "../common/mod.rs"]
mod common;
use common::test_db::TestDatabase;

/// Test server instance
pub struct TestServer {
    pub addr: SocketAddr,
    #[allow(dead_code)]
    pub pool: DbPool,
    pub token_service: TokenService,
    _handle: JoinHandle<()>,
    #[allow(dead_code)]
    _test_db: TestDatabase,
}

impl TestServer {
    /// Start a test server on a random available port
    pub async fn start() -> Self {
        let test_db = TestDatabase::new("cli_integration").await;
        let pool = test_db.pool().clone();

        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));
        let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
        let token_service = TokenService::with_sqlx(pool.clone(), audit_repo);

        let router = flowplane::api::routes::build_router(state.clone());

        // Bind to a random port
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind to random port");
        let addr = listener.local_addr().expect("get local addr");

        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("server error");
        });

        // Give the server time to start and be ready to accept connections
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Verify server is responding
        for _ in 0..10 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        TestServer { addr, pool, token_service, _handle: handle, _test_db: test_db }
    }

    /// Issue a test token with specified scopes
    pub async fn issue_token(&self, name: &str, scopes: &[&str]) -> TokenSecretResponse {
        self.token_service
            .create_token(
                CreateTokenRequest::without_user(
                    name.to_string(),
                    None,
                    None,
                    scopes.iter().map(|s| s.to_string()).collect(),
                    Some("cli-integration-tests".into()),
                ),
                None,
            )
            .await
            .expect("create token")
    }

    /// Issue a test token with admin:all + specified scopes.
    /// Use this for tests that need global resource scope access (the security fix
    /// restricts global resource scopes to platform admins only).
    pub async fn issue_admin_token(&self, name: &str, scopes: &[&str]) -> TokenSecretResponse {
        let mut all_scopes = vec!["admin:all".to_string()];
        all_scopes.extend(scopes.iter().map(|s| s.to_string()));
        self.token_service
            .create_token(
                CreateTokenRequest::without_user(
                    name.to_string(),
                    None,
                    None,
                    all_scopes,
                    Some("cli-integration-tests".into()),
                ),
                None,
            )
            .await
            .expect("create token")
    }

    /// Get the base URL for this test server
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Create a team via direct database access (for test setup)
    #[allow(dead_code)]
    pub async fn create_team(&self, team_name: &str) {
        use flowplane::auth::CreateTeamRequest;
        use flowplane::domain::OrgId;
        use flowplane::storage::repositories::{SqlxTeamRepository, TeamRepository};

        let team_repo = SqlxTeamRepository::new(self.pool.clone());

        // Ignore errors if team already exists
        let _ = team_repo
            .create_team(CreateTeamRequest {
                name: team_name.to_string(),
                display_name: format!("Test Team {}", team_name),
                description: Some("Team for CLI integration tests".to_string()),
                owner_user_id: None,
                org_id: OrgId::from_str_unchecked(common::test_db::TEST_ORG_ID),
                settings: None,
            })
            .await;
    }
}

/// Get the CLI binary path (already built by cargo test)
fn get_cli_binary_path() -> &'static PathBuf {
    use std::sync::OnceLock;

    static CLI_PATH: OnceLock<PathBuf> = OnceLock::new();

    CLI_PATH.get_or_init(|| {
        // cargo test already builds all binaries before running tests
        // We just need to find the pre-built binary
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        let mut binary_path = PathBuf::from(manifest_dir);
        binary_path.push("target");
        binary_path.push("debug"); // Tests use debug builds
        binary_path.push("flowplane-cli");

        if !binary_path.exists() {
            panic!(
                "CLI binary not found at {:?}. Run 'cargo build --bin flowplane-cli' first.",
                binary_path
            );
        }

        binary_path
    })
}

/// Run a CLI command and capture output
///
/// # Arguments
/// * `args` - Command line arguments to pass to the CLI
/// * `env_overrides` - Optional environment variables to set for this command (e.g., HOME)
pub async fn run_cli_command_with_env(
    args: &[&str],
    env_overrides: Option<&[(&str, &str)]>,
) -> Result<String, String> {
    use std::process::Command;

    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let cli_path = get_cli_binary_path().clone();
    let env_overrides_owned: Option<Vec<(String, String)>> = env_overrides
        .map(|overrides| overrides.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());

    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(cli_path);
        cmd.args(&args_owned);

        // Apply environment overrides if provided
        if let Some(overrides) = env_overrides_owned {
            // Clear inherited environment to ensure only specified vars are set
            cmd.env_clear();

            // Restore only PATH (essential for finding binaries)
            if let Ok(path) = std::env::var("PATH") {
                cmd.env("PATH", path);
            }

            // Apply the requested environment overrides
            for (key, value) in overrides {
                cmd.env(key, value);
            }

            // Note: We intentionally do NOT restore HOME/USERPROFILE automatically
            // Tests that need these variables should pass them explicitly
            // This prevents config file interference in auth tests
        }
        // If no overrides specified, inherit parent environment (default behavior)

        let output = cmd.output().expect("failed to execute CLI command");

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    })
    .await
    .expect("task join error")
}

/// Run a CLI command and capture output (without environment overrides)
///
/// This is a convenience wrapper around run_cli_command_with_env for backwards compatibility.
pub async fn run_cli_command(args: &[&str]) -> Result<String, String> {
    run_cli_command_with_env(args, None).await
}

/// Create a temporary config file for testing
pub struct TempConfig {
    pub path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

impl TempConfig {
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        // CLI expects config at $HOME/.flowplane/config.toml
        let flowplane_dir = temp_dir.path().join(".flowplane");
        std::fs::create_dir_all(&flowplane_dir).expect("create .flowplane directory");
        let path = flowplane_dir.join("config.toml");
        TempConfig { path, _temp_dir: temp_dir }
    }

    /// Get the home directory path to use for HOME env var
    pub fn home_dir(&self) -> &std::path::Path {
        self._temp_dir.path()
    }

    pub fn write_config(&self, token: &str, base_url: &str) {
        use std::fs;
        let content = format!(
            r#"token = "{}"
base_url = "{}"
timeout = 30
"#,
            token, base_url
        );
        fs::write(&self.path, content).expect("write config file");
    }
}

/// Create a temporary token file for testing
pub struct TempTokenFile {
    pub path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

impl TempTokenFile {
    pub fn new(token: &str) -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("token.txt");
        std::fs::write(&path, token).expect("write token file");
        TempTokenFile { path, _temp_dir: temp_dir }
    }

    pub fn path_str(&self) -> String {
        self.path.to_str().unwrap().to_string()
    }
}

/// Create a temporary OpenAPI spec file for testing
pub struct TempOpenApiFile {
    pub path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

impl TempOpenApiFile {
    pub fn new(content: &str) -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("openapi.yaml");
        std::fs::write(&path, content).expect("write openapi file");
        TempOpenApiFile { path, _temp_dir: temp_dir }
    }

    pub fn path_str(&self) -> String {
        self.path.to_str().unwrap().to_string()
    }
}
