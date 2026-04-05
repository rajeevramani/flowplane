//! Logs CLI command
//!
//! Thin wrapper over `docker/podman compose logs` for the local dev stack.

use anyhow::Result;
use std::path::Path;

use super::compose;
use super::expose::is_loopback;

/// Build a compose logs command without spawning it.
///
/// This is separated from `handle_logs_command` for testability.
pub fn compose_logs_command(
    runtime: &str,
    compose_file: &Path,
    follow: bool,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(runtime);
    cmd.arg("compose")
        .arg("-f")
        .arg(compose_file)
        .arg("--profile")
        .arg("envoy")
        .arg("--profile")
        .arg("httpbin")
        .arg("logs");

    if follow {
        cmd.arg("-f");
    }

    cmd
}

/// Handle `flowplane logs [--follow]`
pub async fn handle_logs_command(base_url: &str, follow: bool) -> Result<()> {
    if !is_loopback(base_url) {
        println!("Logs are only available for local dev stacks started with `flowplane init`.");
        return Ok(());
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| anyhow::anyhow!("Unable to determine home directory"))?;

    let compose_file =
        std::path::PathBuf::from(home).join(".flowplane").join("docker-compose-dev.yml");

    if !compose_file.exists() {
        anyhow::bail!(
            "Compose file not found at {}. Run 'flowplane init' first.",
            compose_file.display()
        );
    }

    let runtime = compose::detect_runtime()?;

    let mut child = compose_logs_command(&runtime, &compose_file, follow)
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run {runtime} compose: {e}"))?;

    let status =
        child.wait().map_err(|e| anyhow::anyhow!("Failed to wait for {runtime} compose: {e}"))?;

    if !status.success() {
        anyhow::bail!("{runtime} compose logs exited with status {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn compose_logs_command_no_follow() {
        let path = PathBuf::from("/tmp/docker-compose-dev.yml");
        let cmd = compose_logs_command("docker", &path, false);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(cmd.get_program().to_string_lossy(), "docker");
        assert_eq!(
            args,
            vec![
                "compose",
                "-f",
                "/tmp/docker-compose-dev.yml",
                "--profile",
                "envoy",
                "--profile",
                "httpbin",
                "logs"
            ]
        );
    }

    #[test]
    fn compose_logs_command_with_follow() {
        let path = PathBuf::from("/home/user/.flowplane/docker-compose-dev.yml");
        let cmd = compose_logs_command("podman", &path, true);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(cmd.get_program().to_string_lossy(), "podman");
        assert_eq!(
            args,
            vec![
                "compose",
                "-f",
                "/home/user/.flowplane/docker-compose-dev.yml",
                "--profile",
                "envoy",
                "--profile",
                "httpbin",
                "logs",
                "-f"
            ]
        );
    }
}
