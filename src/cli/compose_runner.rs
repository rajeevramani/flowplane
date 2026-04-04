//! Abstraction over container runtime compose operations.
//!
//! Provides `ComposeRunner` trait so tests can verify compose commands without
//! actually running Docker/Podman, and `ProductionComposeRunner` / `MockComposeRunner`
//! implementations.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

/// Abstraction over container runtime compose operations.
///
/// Allows tests to verify compose commands without actually running Docker/Podman.
pub trait ComposeRunner: Send + Sync {
    /// Run `compose up` with the given options.
    fn compose_up(
        &self,
        compose_path: &Path,
        project_name: &str,
        profiles: &[&str],
        env_vars: &[(&str, &str)],
        force_recreate: bool,
    ) -> Result<()>;

    /// Run `compose down` with optional volume removal.
    fn compose_down(
        &self,
        compose_path: &Path,
        project_name: &str,
        remove_volumes: bool,
    ) -> Result<()>;

    /// Get the detected container runtime name ("docker" or "podman").
    fn runtime_name(&self) -> &str;
}

/// Production implementation that shells out to docker/podman compose.
pub struct ProductionComposeRunner {
    runtime: String,
}

impl ProductionComposeRunner {
    /// Detect the container runtime and create a runner.
    pub fn detect() -> Result<Self> {
        Ok(Self { runtime: super::compose::detect_runtime()? })
    }
}

impl ComposeRunner for ProductionComposeRunner {
    fn compose_up(
        &self,
        compose_path: &Path,
        project_name: &str,
        profiles: &[&str],
        env_vars: &[(&str, &str)],
        force_recreate: bool,
    ) -> Result<()> {
        let mut cmd = std::process::Command::new(&self.runtime);
        cmd.arg("compose").arg("-f").arg(compose_path).arg("-p").arg(project_name);

        for profile in profiles {
            cmd.arg("--profile").arg(profile);
        }

        cmd.arg("up").arg("-d");

        if force_recreate {
            cmd.arg("--force-recreate");
        }

        cmd.arg("--build");

        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let status = cmd.status().context("failed to run docker/podman compose")?;
        if !status.success() {
            anyhow::bail!("docker compose up failed with exit code: {}", status);
        }

        Ok(())
    }

    fn compose_down(
        &self,
        compose_path: &Path,
        project_name: &str,
        remove_volumes: bool,
    ) -> Result<()> {
        let mut cmd = std::process::Command::new(&self.runtime);
        cmd.arg("compose").arg("-f").arg(compose_path).arg("-p").arg(project_name);

        // Always pass all profiles so profiled services (envoy, httpbin) are also stopped.
        // These are no-ops if those containers weren't started.
        cmd.arg("--profile").arg("envoy");
        cmd.arg("--profile").arg("httpbin");

        cmd.arg("down");

        if remove_volumes {
            cmd.arg("--volumes");
        }

        let status = cmd.status().context("failed to run docker/podman compose down")?;
        if !status.success() {
            anyhow::bail!("docker compose down failed with exit code: {}", status);
        }

        Ok(())
    }

    fn runtime_name(&self) -> &str {
        &self.runtime
    }
}

/// Recorded compose invocation for test assertions.
#[derive(Debug, Clone)]
pub struct ComposeCall {
    pub operation: ComposeOp,
    pub compose_path: PathBuf,
    pub project_name: String,
    pub profiles: Vec<String>,
    pub env_vars: Vec<(String, String)>,
}

/// Type of compose operation that was invoked.
#[derive(Debug, Clone)]
pub enum ComposeOp {
    Up { force_recreate: bool },
    Down { remove_volumes: bool },
}

/// Mock implementation that records all calls for test assertions.
pub struct MockComposeRunner {
    pub calls: Mutex<Vec<ComposeCall>>,
    pub runtime: String,
}

impl MockComposeRunner {
    /// Create a new mock runner with the given runtime name.
    pub fn new(runtime: &str) -> Self {
        Self { calls: Mutex::new(Vec::new()), runtime: runtime.to_string() }
    }

    /// Return a snapshot of all recorded calls.
    pub fn recorded_calls(&self) -> Vec<ComposeCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Default for MockComposeRunner {
    fn default() -> Self {
        Self::new("docker")
    }
}

impl ComposeRunner for MockComposeRunner {
    fn compose_up(
        &self,
        compose_path: &Path,
        project_name: &str,
        profiles: &[&str],
        env_vars: &[(&str, &str)],
        force_recreate: bool,
    ) -> Result<()> {
        let call = ComposeCall {
            operation: ComposeOp::Up { force_recreate },
            compose_path: compose_path.to_path_buf(),
            project_name: project_name.to_string(),
            profiles: profiles.iter().map(|s| s.to_string()).collect(),
            env_vars: env_vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        };
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
        Ok(())
    }

    fn compose_down(
        &self,
        compose_path: &Path,
        project_name: &str,
        remove_volumes: bool,
    ) -> Result<()> {
        let call = ComposeCall {
            operation: ComposeOp::Down { remove_volumes },
            compose_path: compose_path.to_path_buf(),
            project_name: project_name.to_string(),
            profiles: Vec::new(),
            env_vars: Vec::new(),
        };
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
        Ok(())
    }

    fn runtime_name(&self) -> &str {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_runner_records_compose_up() {
        let runner = MockComposeRunner::default();
        let path = PathBuf::from("/tmp/compose.yml");

        runner
            .compose_up(&path, "test-project", &["envoy"], &[("KEY", "val")], true)
            .expect("mock compose_up should succeed");

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].project_name, "test-project");
        assert_eq!(calls[0].profiles, vec!["envoy"]);
        assert_eq!(calls[0].env_vars, vec![("KEY".to_string(), "val".to_string())]);
        assert!(matches!(calls[0].operation, ComposeOp::Up { force_recreate: true }));
    }

    #[test]
    fn mock_runner_records_compose_down() {
        let runner = MockComposeRunner::default();
        let path = PathBuf::from("/tmp/compose.yml");

        runner.compose_down(&path, "test-project", true).expect("mock compose_down should succeed");

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert!(matches!(calls[0].operation, ComposeOp::Down { remove_volumes: true }));
    }

    #[test]
    fn mock_runner_default_runtime_is_docker() {
        let runner = MockComposeRunner::default();
        assert_eq!(runner.runtime_name(), "docker");
    }

    #[test]
    fn mock_runner_custom_runtime() {
        let runner = MockComposeRunner::new("podman");
        assert_eq!(runner.runtime_name(), "podman");
    }

    #[test]
    fn mock_runner_records_multiple_calls() {
        let runner = MockComposeRunner::default();
        let path = PathBuf::from("/tmp/compose.yml");

        runner.compose_up(&path, "proj", &[], &[], false).expect("should succeed");
        runner.compose_down(&path, "proj", false).expect("should succeed");

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 2);
        assert!(matches!(calls[0].operation, ComposeOp::Up { .. }));
        assert!(matches!(calls[1].operation, ComposeOp::Down { .. }));
    }
}
