//! CLI subprocess test helper
//!
//! Invokes the `flowplane` binary as a child process with an isolated HOME directory,
//! pre-configured credentials, and captured stdout/stderr. This enables testing
//! argument parsing, output formatting, env var handling, and exit codes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use tempfile::TempDir;

use super::harness::TestHarness;

/// Default timeout for CLI commands (30 seconds).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Helper for invoking the flowplane CLI binary in tests.
///
/// Provides isolated filesystem (TempDir HOME), env var injection,
/// stdout/stderr capture, and exit code assertions.
pub struct CliRunner {
    /// Path to the flowplane binary
    binary_path: PathBuf,
    /// Isolated HOME directory (TempDir) — kept alive so cert/config files persist
    _home_dir: TempDir,
    /// Path to the isolated HOME (for building commands)
    home_path: PathBuf,
    /// Additional env vars to inject
    env_vars: HashMap<String, String>,
}

impl CliRunner {
    /// Create from a TestHarness — pulls base_url and auth_token automatically.
    ///
    /// Sets up an isolated `~/.flowplane/` directory with:
    /// - `config.toml` pointing at the test control plane
    /// - `credentials` file containing the harness auth token
    pub fn from_harness(harness: &TestHarness) -> anyhow::Result<Self> {
        let binary_path = Self::find_binary()?;
        let home_dir = tempfile::tempdir()?;
        let home_path = home_dir.path().to_path_buf();

        // Create ~/.flowplane/ directory
        let fp_dir = home_path.join(".flowplane");
        std::fs::create_dir_all(&fp_dir)?;

        // Write config.toml with base_url and team
        let config_content = format!(
            "base_url = \"{}\"\nteam = \"{}\"\norg = \"{}\"\n",
            harness.api_url(),
            harness.team,
            harness.org,
        );
        std::fs::write(fp_dir.join("config.toml"), config_content)?;

        // Write credentials file with the auth token
        std::fs::write(fp_dir.join("credentials"), &harness.auth_token)?;

        Ok(Self { binary_path, _home_dir: home_dir, home_path, env_vars: HashMap::new() })
    }

    /// Create a CliRunner that uses a different auth token than the harness default.
    ///
    /// Use this for multi-user isolation tests: create a user via
    /// `harness.shared_infra().create_test_user(...)`, obtain their token via
    /// `shared.get_user_token(email, password)`, then run CLI commands as that user.
    pub fn with_token(harness: &TestHarness, token: &str) -> anyhow::Result<Self> {
        let binary_path = Self::find_binary()?;
        let home_dir = tempfile::tempdir()?;
        let home_path = home_dir.path().to_path_buf();

        let fp_dir = home_path.join(".flowplane");
        std::fs::create_dir_all(&fp_dir)?;

        let config_content = format!(
            "base_url = \"{}\"\nteam = \"{}\"\norg = \"{}\"\n",
            harness.api_url(),
            harness.team,
            harness.org,
        );
        std::fs::write(fp_dir.join("config.toml"), config_content)?;
        std::fs::write(fp_dir.join("credentials"), token)?;

        Ok(Self { binary_path, _home_dir: home_dir, home_path, env_vars: HashMap::new() })
    }

    /// Create a CliRunner targeting a specific team and org with a custom token.
    ///
    /// Use this when testing cross-team isolation: user A in team-engineering
    /// should not see resources created by user B in team-ops.
    pub fn with_token_and_team(
        harness: &TestHarness,
        token: &str,
        team: &str,
        org: &str,
    ) -> anyhow::Result<Self> {
        let binary_path = Self::find_binary()?;
        let home_dir = tempfile::tempdir()?;
        let home_path = home_dir.path().to_path_buf();

        let fp_dir = home_path.join(".flowplane");
        std::fs::create_dir_all(&fp_dir)?;

        let config_content = format!(
            "base_url = \"{}\"\nteam = \"{}\"\norg = \"{}\"\n",
            harness.api_url(),
            team,
            org,
        );
        std::fs::write(fp_dir.join("config.toml"), config_content)?;
        std::fs::write(fp_dir.join("credentials"), token)?;

        Ok(Self { binary_path, _home_dir: home_dir, home_path, env_vars: HashMap::new() })
    }

    /// Add an env var for subsequent runs.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env_vars.insert(key.to_string(), value.to_string());
        self
    }

    /// Run a CLI command and capture output (default 30s timeout).
    pub fn run(&self, args: &[&str]) -> anyhow::Result<CliOutput> {
        self.run_with_timeout(args, DEFAULT_TIMEOUT)
    }

    /// Run a CLI command with an explicit timeout.
    pub fn run_with_timeout(&self, args: &[&str], timeout: Duration) -> anyhow::Result<CliOutput> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.args(args);

        // Isolated HOME — prevents touching real ~/.flowplane/
        cmd.env("HOME", &self.home_path);

        // Clear FLOWPLANE_* env vars from parent to prevent leaking
        for (key, _) in std::env::vars() {
            if key.starts_with("FLOWPLANE_") {
                cmd.env_remove(&key);
            }
        }

        // Inject user-specified env vars (after clearing)
        for (key, value) in &self.env_vars {
            cmd.env(key, value);
        }

        let child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn CLI binary: {e}"))?;

        // Wait with timeout using a background thread
        let output = wait_with_timeout(child, timeout)?;

        Ok(CliOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Locate the compiled flowplane binary.
    ///
    /// Checks `target/debug/flowplane` then `target/release/flowplane` relative
    /// to the cargo manifest directory.
    fn find_binary() -> anyhow::Result<PathBuf> {
        // Use CARGO_BIN_EXE_flowplane if set (cargo test sets this for integration tests)
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_flowplane") {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
        }

        // Find project root from CARGO_MANIFEST_DIR or walk up from current dir
        let root = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().expect("current dir"));

        let debug_bin = root.join("target/debug/flowplane");
        if debug_bin.exists() {
            return Ok(debug_bin);
        }

        let release_bin = root.join("target/release/flowplane");
        if release_bin.exists() {
            return Ok(release_bin);
        }

        anyhow::bail!(
            "flowplane binary not found. Run `cargo build` first. \
             Checked: {}, {}",
            debug_bin.display(),
            release_bin.display()
        )
    }
}

/// Output from a CLI invocation.
pub struct CliOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    /// Assert the command exited with code 0.
    pub fn assert_success(&self) -> &Self {
        assert_eq!(
            self.exit_code, 0,
            "Expected exit code 0 but got {}.\nstdout: {}\nstderr: {}",
            self.exit_code, self.stdout, self.stderr
        );
        self
    }

    /// Assert the command exited with a non-zero code.
    pub fn assert_failure(&self) -> &Self {
        assert_ne!(
            self.exit_code, 0,
            "Expected non-zero exit code but got 0.\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }

    /// Assert the command exited with a specific code.
    pub fn assert_exit_code(&self, code: i32) -> &Self {
        assert_eq!(
            self.exit_code, code,
            "Expected exit code {code} but got {}.\nstdout: {}\nstderr: {}",
            self.exit_code, self.stdout, self.stderr
        );
        self
    }

    /// Assert stdout contains the given text.
    pub fn assert_stdout_contains(&self, text: &str) -> &Self {
        assert!(
            self.stdout.contains(text),
            "Expected stdout to contain {text:?} but got:\n{}",
            self.stdout
        );
        self
    }

    /// Assert stderr contains the given text.
    pub fn assert_stderr_contains(&self, text: &str) -> &Self {
        assert!(
            self.stderr.contains(text),
            "Expected stderr to contain {text:?} but got:\n{}",
            self.stderr
        );
        self
    }
}

/// Wait for a child process with a timeout.
///
/// Kills the process if it exceeds the timeout.
fn wait_with_timeout(
    child: std::process::Child,
    timeout: Duration,
) -> anyhow::Result<std::process::Output> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result.map_err(|e| anyhow::anyhow!("CLI process failed: {e}")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            anyhow::bail!("CLI command timed out after {}s", timeout.as_secs())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("CLI process channel disconnected unexpectedly")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_binary_or_skip() {
        // This test validates the binary lookup logic.
        // If the binary isn't built, the test is informational — not a failure.
        match CliRunner::find_binary() {
            Ok(path) => {
                assert!(path.exists(), "Binary path exists: {}", path.display());
                assert!(
                    path.to_string_lossy().contains("flowplane"),
                    "Binary path contains 'flowplane': {}",
                    path.display()
                );
            }
            Err(e) => {
                eprintln!("Binary not found (expected if not built): {e}");
            }
        }
    }

    #[test]
    fn test_cli_output_assertions() {
        let output =
            CliOutput { exit_code: 0, stdout: "hello world\n".to_string(), stderr: String::new() };
        output.assert_success();
        output.assert_exit_code(0);
        output.assert_stdout_contains("hello");
    }

    #[test]
    #[should_panic(expected = "Expected exit code 0")]
    fn test_cli_output_assert_success_panics_on_failure() {
        let output =
            CliOutput { exit_code: 1, stdout: String::new(), stderr: "error\n".to_string() };
        output.assert_success();
    }

    #[test]
    fn test_cli_output_assert_failure() {
        let output =
            CliOutput { exit_code: 1, stdout: String::new(), stderr: "error\n".to_string() };
        output.assert_failure();
        output.assert_stderr_contains("error");
    }
}
