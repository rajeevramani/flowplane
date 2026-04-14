//! Secure dev-mode token generation and credential file management.
//!
//! Generates cryptographically random bearer tokens for local development
//! (AuthMode::Dev). These are NOT JWTs — just opaque URL-safe base64 strings
//! that the dev-mode middleware validates against `FLOWPLANE_DEV_TOKEN`.
//!
//! Also handles writing/reading credentials to `~/.flowplane/credentials`
//! using atomic write-then-rename with restrictive Unix permissions.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use std::io::Write;
use std::path::Path;

/// Minimum raw byte length for generated tokens.
const TOKEN_RAW_BYTES: usize = 32;
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

/// Generate a cryptographically random URL-safe bearer token.
///
/// Returns a base64url-encoded string (no padding) of at least 32 random bytes.
pub fn generate_dev_token() -> String {
    let mut buf = [0u8; TOKEN_RAW_BYTES];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Resolve the dev token for the current process.
///
/// If `FLOWPLANE_DEV_TOKEN` is already set in the environment, returns its value.
/// Otherwise generates a new random token, sets `FLOWPLANE_DEV_TOKEN` so the
/// dev-mode middleware can read it on each request, and returns the token.
///
/// # Safety note
///
/// `std::env::set_var` is not thread-safe. This function must be called during
/// startup before spawning async tasks or additional threads.
pub fn resolve_or_generate_dev_token() -> String {
    if let Ok(existing) = std::env::var("FLOWPLANE_DEV_TOKEN") {
        if !existing.is_empty() {
            return existing;
        }
    }

    let token = generate_dev_token();

    // SAFETY: Called during single-threaded startup, before tokio runtime spawns tasks.
    // set_var is needed so dev_authenticate middleware can read it per-request (E3 fix).
    std::env::set_var("FLOWPLANE_DEV_TOKEN", &token);

    println!("Dev token: {token}");

    token
}

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
    fn test_generate_dev_token_length() {
        let t = generate_dev_token();
        assert!(!t.is_empty());
        // 32 raw bytes → 43 base64url chars (no padding)
        assert_eq!(t.len(), 43, "32 bytes base64url should be 43 chars");
    }

    #[test]
    fn test_generate_dev_token_uniqueness() {
        let t1 = generate_dev_token();
        let t2 = generate_dev_token();
        assert_ne!(t1, t2, "two successive tokens must not be equal");
    }

    /// Token must never be the legacy static string.
    #[test]
    fn token_is_not_static_legacy_value() {
        for _ in 0..50 {
            let t = generate_dev_token();
            assert_ne!(t, "fp_dev_token");
        }
    }

    /// Token contains only URL-safe base64 characters (A-Z, a-z, 0-9, -, _).
    #[test]
    fn token_url_safe_characters() {
        let t = generate_dev_token();
        assert!(
            t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token contains non-URL-safe characters: {t}"
        );
    }

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

    #[test]
    fn test_resolve_with_env_var() {
        let sentinel = "test-resolve-sentinel-99887";
        std::env::set_var("FLOWPLANE_DEV_TOKEN", sentinel);
        let result = resolve_or_generate_dev_token();
        assert_eq!(result, sentinel);
        std::env::remove_var("FLOWPLANE_DEV_TOKEN");
    }

    #[test]
    fn test_resolve_without_env_var() {
        std::env::remove_var("FLOWPLANE_DEV_TOKEN");
        let token = resolve_or_generate_dev_token();
        assert!(!token.is_empty());
        assert_eq!(std::env::var("FLOWPLANE_DEV_TOKEN").ok().as_deref(), Some(token.as_str()),);
        std::env::remove_var("FLOWPLANE_DEV_TOKEN");
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
