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
    storage::{self, repository::AuditLogRepository, DbPool},
    xds::XdsState,
};
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Test server instance
pub struct TestServer {
    pub addr: SocketAddr,
    #[allow(dead_code)]
    pub pool: DbPool,
    pub token_service: TokenService,
    _handle: JoinHandle<()>,
}

impl TestServer {
    /// Start a test server on a random available port
    pub async fn start() -> Self {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:?cache=shared")
            .await
            .expect("create sqlite pool");

        storage::run_migrations(&pool).await.expect("run migrations for CLI integration tests");

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

        TestServer { addr, pool, token_service, _handle: handle }
    }

    /// Issue a test token with specified scopes
    pub async fn issue_token(&self, name: &str, scopes: &[&str]) -> TokenSecretResponse {
        self.token_service
            .create_token(CreateTokenRequest {
                name: name.to_string(),
                description: None,
                expires_at: None,
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                created_by: Some("cli-integration-tests".into()),
            })
            .await
            .expect("create token")
    }

    /// Get the base URL for this test server
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

/// Get or build the CLI binary path (cached after first build)
fn get_cli_binary_path() -> &'static PathBuf {
    use std::process::Command;
    use std::sync::OnceLock;

    static CLI_PATH: OnceLock<PathBuf> = OnceLock::new();

    CLI_PATH.get_or_init(|| {
        // Build the CLI binary once
        let build_output = Command::new("cargo")
            .args(["build", "--bin", "flowplane-cli"])
            .output()
            .expect("failed to build CLI binary");

        if !build_output.status.success() {
            panic!(
                "CLI binary build failed: {}",
                String::from_utf8_lossy(&build_output.stderr)
            );
        }

        // Determine the binary path based on build profile
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR not set");
        let mut binary_path = PathBuf::from(manifest_dir);
        binary_path.push("target");
        binary_path.push("debug"); // Tests use debug builds
        binary_path.push("flowplane-cli");

        if !binary_path.exists() {
            panic!("CLI binary not found at {:?}", binary_path);
        }

        binary_path
    })
}

/// Run a CLI command and capture output
pub async fn run_cli_command(args: &[&str]) -> Result<String, String> {
    use std::process::Command;

    // Capture environment BEFORE spawning thread to ensure test env is captured
    let current_env: Vec<(String, String)> = std::env::vars().collect();
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let cli_path = get_cli_binary_path().clone();

    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(cli_path);
        cmd.args(&args_owned);

        // Clear environment and set from captured vars to ensure test env is used
        cmd.env_clear();
        for (key, value) in current_env {
            cmd.env(key, value);
        }

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
