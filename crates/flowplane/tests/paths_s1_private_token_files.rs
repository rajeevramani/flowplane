//! fpv2-wvp.1 — private token/config file hardening (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only the
//! externally observable contract for how the binary persists private files:
//!
//!   * A. CLI config persistence: `config set-context` at the default location creates
//!     `$HOME/.flowplane` with mode `0700` and writes `config.toml` with mode `0600`,
//!     and the written config round-trips through a follow-up read command.
//!   * B. Server dev-token explicit path: with `FLOWPLANE_DEV_TOKEN_PATH` pointing at a
//!     not-yet-existing nested path, `flowplane serve` in dev mode creates the missing
//!     parent directory and writes a non-empty token file with mode `0600`.
//!   * C. Server dev-token explicit-path write failure is FATAL: when the token path is
//!     inside an unwritable (`0500`) directory, the server exits on its own with a
//!     non-zero status instead of running without the requested token file.
//!
//! Conventions honored: unique temp dirs per test, no fixed TCP ports, all env goes on
//! the child `Command` only (never `std::env::set_var`), and the server tests skip with
//! a note when `FLOWPLANE_TEST_DATABASE_URL` is unset (shared-PG convention). The whole
//! file is unix-only because every assertion here is about unix permission bits.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the server child to produce/deny the dev-token file.
const SERVER_DEADLINE: Duration = Duration::from_secs(90);
/// Poll interval while waiting on the child.
const POLL_STEP: Duration = Duration::from_millis(250);

/// The unix permission bits of a path (e.g. `0o700`), sans file-type bits.
fn mode_bits(path: &Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

/// Kills (and reaps) the child on drop, so a panicking assertion never leaks a
/// running `flowplane serve` process.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The shared test database URL, or `None` (⇒ skip) per this repo's DB-test convention.
fn test_database_url() -> Option<String> {
    std::env::var("FLOWPLANE_TEST_DATABASE_URL").ok()
}

/// Best-effort readback of the child's captured stderr for failure diagnostics.
fn stderr_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(30).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// A `flowplane serve` command with a clean, fully explicit child environment.
///
/// Mirrors the acceptance-criteria env list; two additions come straight from the
/// documented configuration contract (`docs/reference/configuration.md`):
///   * `FLOWPLANE_API_INSECURE=true` — required when the API listener has no TLS
///     material, otherwise startup fails (footnote ², D-008);
///   * `FLOWPLANE_XDS_ADDR=127.0.0.1:0` — the xDS listener otherwise defaults to a
///     FIXED port (18000), which would break parallel-safety;
///   * `FLOWPLANE_DEV_MODE_ACK` — a no-op in debug builds, but release builds refuse
///     dev mode without it (footnote ⁴).
fn serve_cmd(home: &Path, db_url: &str, dev_token_path: &Path, stderr_log: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home);
    cmd.env("FLOWPLANE_DATABASE_URL", db_url);
    // Pass through the shared-PG encryption key if the test process has one (convention).
    if let Some(key) = std::env::var_os("FLOWPLANE_SECRET_ENCRYPTION_KEY") {
        cmd.env("FLOWPLANE_SECRET_ENCRYPTION_KEY", key);
    }
    cmd.env("FLOWPLANE_DEV_MODE", "true");
    cmd.env("FLOWPLANE_DEV_MODE_ACK", "yes-this-is-not-production");
    cmd.env("FLOWPLANE_API_ADDR", "127.0.0.1:0");
    cmd.env("FLOWPLANE_XDS_ADDR", "127.0.0.1:0");
    cmd.env("FLOWPLANE_API_INSECURE", "true");
    cmd.env("FLOWPLANE_DEV_TOKEN_PATH", dev_token_path);
    cmd.arg("serve");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    // Capture stderr to a file (not a pipe nobody drains) purely for failure diagnostics.
    cmd.stderr(Stdio::from(
        File::create(stderr_log).expect("create server stderr log"),
    ));
    cmd
}

/// A directory pre-created with mode `0500` (r-x, not writable) inside `base`.
fn make_readonly_dir(base: &Path) -> PathBuf {
    let ro = base.join("ro");
    std::fs::create_dir(&ro).expect("create ro dir");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500))
        .expect("chmod 0500 ro dir");
    ro
}

// ---------------------------------------------------------------------------------------------
// Criterion A: CLI private files — `config set-context` at the DEFAULT config location
// (`$HOME/.flowplane/config.toml`, FLOWPLANE_CONFIG unset) yields a 0700 directory and a
// 0600 file, and the config round-trips through `config get-contexts`.
// ---------------------------------------------------------------------------------------------
#[test]
fn cli_config_dir_0700_file_0600_and_round_trip() {
    let home = common::unique_tempdir();

    // `flowplane_cmd` isolates HOME but pins FLOWPLANE_CONFIG; the criterion requires the
    // DEFAULT `$HOME/.flowplane/config.toml` location, so unset it on the child.
    let mut cmd = common::flowplane_cmd(&home);
    cmd.env_remove("FLOWPLANE_CONFIG");
    let out = cmd
        .args([
            "config",
            "set-context",
            "prod",
            "--server",
            "http://127.0.0.1:9",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "config set-context must exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );

    let dir = home.join(".flowplane");
    let file = dir.join("config.toml");
    assert!(
        dir.is_dir(),
        "set-context must create the default config dir {}",
        dir.display()
    );
    assert!(
        file.is_file(),
        "set-context must write the default config file {}",
        file.display()
    );
    assert_eq!(
        mode_bits(&dir),
        0o700,
        "$HOME/.flowplane must be created with mode 0700"
    );
    assert_eq!(
        mode_bits(&file),
        0o600,
        "$HOME/.flowplane/config.toml must be written with mode 0600"
    );

    // Round-trip: a follow-up read against the same HOME must still work and must see the
    // context that was just persisted (`config get-contexts` is the documented lister).
    let mut cmd = common::flowplane_cmd(&home);
    cmd.env_remove("FLOWPLANE_CONFIG");
    let out = cmd.args(["config", "get-contexts"]).output().unwrap();
    assert!(
        out.status.success(),
        "config get-contexts must exit 0 against the freshly written config, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("prod"),
        "config get-contexts must list the persisted context 'prod': {stdout:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion B: server dev-token explicit path — `serve` with FLOWPLANE_DEV_TOKEN_PATH at a
// nested, not-yet-existing path creates the parent dir and a non-empty 0600 token file.
// ---------------------------------------------------------------------------------------------
#[test]
fn serve_dev_token_explicit_path_creates_parent_and_0600_file() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping serve_dev_token_explicit_path_creates_parent_and_0600_file: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let token_base = common::unique_tempdir();
    let nested = token_base.join("nested");
    let token_path = nested.join("dev-token");
    assert!(
        !nested.exists(),
        "precondition: the 'nested' parent dir must NOT exist before boot"
    );

    let stderr_log = home.join("serve-stderr.log");
    let child = serve_cmd(&home, &db_url, &token_path, &stderr_log)
        .spawn()
        .expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    // Poll for the token file; fail fast if the server exits before producing it.
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if token_path.is_file() && std::fs::metadata(&token_path).unwrap().len() > 0 {
            break;
        }
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before writing the dev-token file; stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "dev-token file did not appear at {} within {SERVER_DEADLINE:?}; stderr tail:\n{}",
                token_path.display(),
                stderr_tail(&stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }

    // The previously-missing parent directory must have been created by the server.
    assert!(
        nested.is_dir(),
        "serve must create the missing 'nested' parent directory for the dev-token path"
    );
    let meta = std::fs::metadata(&token_path).expect("stat dev-token");
    assert!(
        meta.len() > 0,
        "the dev-token file must be non-empty (it holds the per-boot dev bearer token)"
    );
    assert_eq!(
        mode_bits(&token_path),
        0o600,
        "the dev-token file must be written with mode 0600"
    );
    // guard drops here: kill + wait the child on every path.
}

// ---------------------------------------------------------------------------------------------
// Criterion C: server dev-token explicit-path write FAILURE is fatal — with the token path
// inside a 0500 (unwritable) directory, `serve` exits on its own with a non-zero status.
// ---------------------------------------------------------------------------------------------
#[test]
fn serve_dev_token_unwritable_path_is_fatal() {
    let Some(db_url) = test_database_url() else {
        eprintln!(
            "skipping serve_dev_token_unwritable_path_is_fatal: FLOWPLANE_TEST_DATABASE_URL not set"
        );
        return;
    };

    let home = common::unique_tempdir();
    let token_base = common::unique_tempdir();
    let ro = make_readonly_dir(&token_base);

    // Root ignores 0500 directory modes, which would make this test meaningless. Detect
    // that with std only: if this process CAN create a file inside the 0500 dir, skip.
    let probe = ro.join("write-probe");
    if std::fs::write(&probe, b"probe").is_ok() {
        let _ = std::fs::remove_file(&probe);
        eprintln!("skipping serve_dev_token_unwritable_path_is_fatal: 0500 dir is writable (running as root?)");
        return;
    }

    let token_path = ro.join("dev-token");
    let stderr_log = home.join("serve-stderr.log");
    let child = serve_cmd(&home, &db_url, &token_path, &stderr_log)
        .spawn()
        .expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    // The server must exit ON ITS OWN with a non-zero status within the deadline.
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            assert!(
                !status.success(),
                "serve must exit NON-ZERO when the dev-token path is unwritable, got {status}; \
                 stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
            // The exit must be caused by THIS failure, not some unrelated startup error
            // (DB connect, migrations, ...): the fatal error carries the write context.
            let stderr = stderr_tail(&stderr_log);
            assert!(
                stderr.contains("failed to write dev token"),
                "serve must fail BECAUSE of the dev-token write; stderr tail:\n{stderr}"
            );
            assert!(
                !token_path.exists(),
                "no dev-token file may exist inside the unwritable directory"
            );
            return;
        }
        if Instant::now() >= deadline {
            // guard's Drop kills the leaked child after the panic unwinds this frame.
            panic!(
                "serve was still running {SERVER_DEADLINE:?} after boot despite an unwritable \
                 dev-token path — the write failure must be FATAL; stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}
