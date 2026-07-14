//! Well-known Flowplane paths + private file writes, shared by the server and the CLI.
//!
//! The flowplane home is `$HOME/.flowplane`, resolved from `HOME` only — deliberately NOT
//! derived from `FLOWPLANE_CONFIG`: relocating the config file must not relocate token
//! discovery, or the server-written and CLI-read token paths could diverge (FP-DEC-0012).
//! `HOME` unset (or empty) means "no flowplane home": callers degrade per their own
//! contract (the server WARNs and continues; the CLI simply has no dev-token fallback).

use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Pure core of [`flowplane_home`]: `$HOME/.flowplane` from an injected `HOME` value, so
/// tests never mutate process env (constitution invariant 18).
fn flowplane_home_from(home: Option<&OsStr>) -> Option<PathBuf> {
    let home = home?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".flowplane"))
}

/// `$HOME/.flowplane`, or `None` when `HOME` is unset or empty.
pub(crate) fn flowplane_home() -> Option<PathBuf> {
    flowplane_home_from(std::env::var_os("HOME").as_deref())
}

/// Well-known dev-token sink/discovery path: `~/.flowplane/dev-token`.
pub(crate) fn dev_token_path() -> Option<PathBuf> {
    flowplane_home().map(|home| home.join("dev-token"))
}

/// Well-known local bootstrap-token sink: `~/.flowplane/bootstrap-token`.
pub(crate) fn bootstrap_token_path() -> Option<PathBuf> {
    flowplane_home().map(|home| home.join("bootstrap-token"))
}

/// Best-effort [`write_private_file`] for local-only convenience sinks: a failure is logged
/// at WARN (full error chain) and swallowed — boot must never depend on such a write.
/// Returns whether the write succeeded so the caller can log where the file landed.
pub(crate) fn write_private_file_best_effort(path: &Path, contents: impl AsRef<[u8]>) -> bool {
    match write_private_file(path, contents) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %format!("{err:#}"),
                "could not write private file; continuing without it"
            );
            false
        }
    }
}

/// Write `contents` to `path` as a private file: missing parent dirs are created, the file
/// is mode 0600. A newly created parent (or one named `.flowplane`) is tightened to 0700; a
/// pre-existing arbitrary parent keeps its permissions (e.g. a shared eval volume mounted
/// for `FLOWPLANE_DEV_TOKEN_PATH`). Off-unix the write happens without permission handling,
/// matching the credentials-store posture.
pub(crate) fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    ensure_private_parent_dir(path)?;
    write_private_file_contents(path, contents.as_ref())
}

fn ensure_private_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if parent.as_os_str().is_empty() {
            return Ok(());
        }
        let existed = parent.exists();
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        if !existed || parent.file_name().is_some_and(|name| name == ".flowplane") {
            set_private_dir_permissions(parent)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file_contents(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn write_private_file_contents(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod home_tests {
    use super::flowplane_home_from;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn home_joins_dot_flowplane() {
        assert_eq!(
            flowplane_home_from(Some(OsStr::new("/base"))),
            Some(PathBuf::from("/base/.flowplane"))
        );
    }

    #[test]
    fn unset_home_is_none() {
        assert_eq!(flowplane_home_from(None), None);
    }

    #[test]
    fn empty_home_is_none() {
        assert_eq!(flowplane_home_from(Some(OsStr::new(""))), None);
    }
}

#[cfg(test)]
#[cfg(unix)]
#[allow(clippy::expect_used)]
mod permission_tests {
    use super::write_private_file;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let seq = TEMP_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "flowplane-paths-perms-{}-{suffix}-{seq}",
            std::process::id()
        ))
    }

    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn private_file_write_creates_private_flowplane_dir_and_file() {
        let root = temp_root();
        let path = root.join(".flowplane").join("credentials");

        write_private_file(&path, "bearer-token").expect("write private file");

        assert_eq!(mode(path.parent().expect("parent")), 0o700);
        assert_eq!(mode(&path), 0o600);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn private_file_write_restricts_existing_flowplane_dir_and_file() {
        let root = temp_root();
        let parent = root.join(".flowplane");
        let path = parent.join("config.toml");
        fs::create_dir_all(&parent).expect("create parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755))
            .expect("set parent permissions");
        fs::write(&path, "token = \"old\"").expect("write existing file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("set file permissions");

        write_private_file(&path, "token = \"new\"").expect("rewrite private file");

        assert_eq!(mode(&parent), 0o700);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            fs::read_to_string(&path).expect("read file"),
            "token = \"new\""
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn preexisting_arbitrary_parent_keeps_its_permissions() {
        // An operator-named target (e.g. a shared eval volume) that already exists must not
        // be chmod'd; only the file itself is private.
        let root = temp_root();
        let parent = root.join("shared-volume");
        let path = parent.join("dev-token");
        fs::create_dir_all(&parent).expect("create parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755))
            .expect("set parent permissions");

        write_private_file(&path, "tok").expect("write private file");

        assert_eq!(mode(&parent), 0o755, "pre-existing parent untouched");
        assert_eq!(mode(&path), 0o600);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn best_effort_write_succeeds_and_reports_true() {
        let root = temp_root();
        let path = root.join(".flowplane").join("bootstrap-token");

        assert!(super::write_private_file_best_effort(&path, "tok"));
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read_to_string(&path).expect("read file"), "tok");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn best_effort_write_swallows_failure_and_reports_false() {
        // A regular file as the parent path component fails deterministically (even as
        // root) — the wrapper must swallow it, not panic or propagate.
        let root = temp_root();
        fs::create_dir_all(&root).expect("create root");
        let blocker = root.join("blocker");
        fs::write(&blocker, "not a directory").expect("create blocker file");

        assert!(!super::write_private_file_best_effort(
            &blocker.join("bootstrap-token"),
            "tok"
        ));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn missing_nested_parents_are_created_private() {
        let root = temp_root();
        let path = root.join("nested").join("deeper").join("dev-token");

        write_private_file(&path, "tok").expect("write private file");

        assert_eq!(mode(path.parent().expect("parent")), 0o700);
        assert_eq!(mode(&path), 0o600);

        fs::remove_dir_all(root).expect("cleanup");
    }
}
