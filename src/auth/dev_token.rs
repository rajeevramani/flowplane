//! Dev-mode credential file management and canonical dev user constants.
//!
//! In unified auth mode, dev tokens are now real OIDC JWTs minted by the
//! embedded mock OIDC server (`src/dev/oidc_server.rs`). This module no
//! longer generates opaque bearer tokens — it only handles the credentials
//! file written by the control plane on startup and read back by the CLI.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// Credentials filename inside the flowplane directory.
const CREDENTIALS_FILE: &str = "credentials";

/// Canonical `sub` claim value for the dev-mode seeded user.
///
/// Referenced by both `src/startup.rs::seed_dev_resources` (which inserts
/// the row) and the dev-mode OIDC mock server (which must issue tokens
/// whose `sub` matches the seeded user). Keeping the constant here in
/// `src/auth/dev_token.rs` rather than `src/startup.rs` avoids a
/// dev-module → startup-module dependency edge when the OIDC mock lands.
pub const DEV_USER_SUB: &str = "dev-sub";

/// Canonical email for the dev-mode seeded user. Paired with
/// `DEV_USER_SUB` — same rationale for the location.
pub const DEV_USER_EMAIL: &str = "dev@flowplane.local";

/// Write credentials to `{home_dir}/.flowplane/credentials` using atomic write-then-rename.
///
/// Creates the `.flowplane/` directory with `0700` permissions if it does not exist.
/// The credentials file is created with `0600` permissions on Unix.
pub fn write_credentials_file(token: &str, home_dir: &Path) -> Result<()> {
    let target = home_dir.join(".flowplane").join(CREDENTIALS_FILE);
    write_credentials_to_path(token, &target)
}

/// Write credentials to an explicit file path using atomic write-then-rename.
///
/// Creates the parent directory with `0700` permissions if it does not exist.
/// The credentials file is created with `0600` permissions on Unix.
pub fn write_credentials_to_path(token: &str, path: &Path) -> Result<()> {
    let dir = path.parent().context("credentials path has no parent directory")?;
    ensure_dir_with_permissions(dir)?;

    let tmp_path = dir.join(".credentials.tmp");

    // Write to a temp file first (same filesystem guarantees atomic rename)
    {
        let mut file = create_restricted_file(&tmp_path)?;
        file.write_all(token.as_bytes())
            .with_context(|| format!("failed to write temp credentials: {}", tmp_path.display()))?;
        file.flush().with_context(|| "failed to flush temp credentials")?;
    }

    // Atomic rename
    std::fs::rename(&tmp_path, path).with_context(|| {
        format!("failed to rename {} -> {}", tmp_path.display(), path.display())
    })?;

    Ok(())
}

/// Read credentials from `{home_dir}/.flowplane/credentials`.
///
/// Returns an error if the file does not exist or contains only whitespace.
pub fn read_credentials_file(home_dir: &Path) -> Result<String> {
    let path = home_dir.join(".flowplane").join(CREDENTIALS_FILE);
    read_credentials_from_path(&path)
}

/// Read credentials from an explicit file path.
///
/// Returns an error if the file does not exist or contains only whitespace.
pub fn read_credentials_from_path(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read credentials file: {}", path.display()))?;

    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("credentials file is empty: {}", path.display());
    }

    Ok(trimmed)
}

/// Create directory with `0700` permissions on Unix. No-op if it already exists.
fn ensure_dir_with_permissions(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create directory: {}", dir.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to set permissions on {}", dir.display()))?;
        }
    }
    Ok(())
}

/// Create a file with mode `0600` on Unix.
fn create_restricted_file(path: &Path) -> Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to create file: {}", path.display()))
    }

    #[cfg(not(unix))]
    {
        std::fs::File::create(path)
            .with_context(|| format!("failed to create file: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_write_and_read_credentials() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let home = tmp.path();

        let token = "test-token-roundtrip-xyz";
        write_credentials_file(token, home).expect("write_credentials_file failed");

        let read_back = read_credentials_file(home).expect("read_credentials_file failed");
        assert_eq!(read_back, token);
    }

    #[test]
    fn test_write_credentials_atomic() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let home = tmp.path();

        write_credentials_file("atomic-test-token", home).expect("write failed");

        // Final file should exist
        let cred_path = home.join(".flowplane").join("credentials");
        assert!(cred_path.exists(), "credentials file should exist");

        // Temp file should NOT exist (rename consumed it)
        let tmp_path = home.join(".flowplane").join(".credentials.tmp");
        assert!(!tmp_path.exists(), "temp file should not remain after rename");
    }

    #[test]
    fn test_read_credentials_file_not_found() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let result = read_credentials_file(tmp.path());
        assert!(result.is_err(), "should error on missing file");
    }

    #[test]
    fn test_read_credentials_file_empty() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let dir = tmp.path().join(".flowplane");
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(dir.join("credentials"), "").expect("write empty file");

        let result = read_credentials_file(tmp.path());
        assert!(result.is_err(), "should error on empty credentials file");
    }

    #[cfg(unix)]
    #[test]
    fn test_credentials_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().expect("failed to create temp dir");
        write_credentials_file("perm-test-token", tmp.path()).expect("write failed");

        let cred_path = tmp.path().join(".flowplane").join("credentials");
        let perms = std::fs::metadata(&cred_path).expect("metadata").permissions().mode();
        assert_eq!(
            perms & 0o777,
            0o600,
            "credentials file should be 0600, got {:o}",
            perms & 0o777
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().expect("failed to create temp dir");
        write_credentials_file("dir-perm-test", tmp.path()).expect("write failed");

        let dir = tmp.path().join(".flowplane");
        let perms = std::fs::metadata(&dir).expect("metadata").permissions().mode();
        assert_eq!(perms & 0o777, 0o700, "directory should be 0700, got {:o}", perms & 0o777);
    }
}
