//! fpv2-wvp.2 — default dev-token sink (black-box).
//!
//! Contract under test: in dev mode, when NO dev-token path is configured, the server
//! writes the minted per-boot dev bearer token to the well-known default
//! `$HOME/.flowplane/dev-token` (file 0600, dir 0700), and that token authenticates
//! against the running API. An operator-named path — env `FLOWPLANE_DEV_TOKEN_PATH` OR
//! a `dev_token_path` key in the TOML file named by `FLOWPLANE_CONFIG`, both "explicit"
//! — is written fatally (boot dies if the write fails) and suppresses the default sink.
//! A FAILED write to the DEFAULT sink, by contrast, is non-fatal: the server keeps
//! booting and serving. Non-dev boots write no dev-token file at all.
//!
//! Conventions honored (same as the s1 file): the built `flowplane` binary is driven as
//! a subprocess, unique temp dirs per test, no fixed TCP ports (API ports are allocated
//! by binding 127.0.0.1:0 and releasing), all env goes on the child `Command` only
//! (never `std::env::set_var`), children are killed+reaped by a `ChildGuard`, stderr is
//! captured to a per-child file whose tail rides every failure message, and DB-backed
//! tests skip with a note when `FLOWPLANE_TEST_DATABASE_URL` is unset. Unix-only: the
//! contract's permission bits (0600/0700) and the 0500-dir failure injection are unix.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs::File;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long to wait for the server child to write files / answer / exit.
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

/// Allocate a unique loopback port: bind 127.0.0.1:0, read the port, release it.
/// The child then gets `FLOWPLANE_API_ADDR=127.0.0.1:<port>` — never a fixed port.
fn alloc_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// A directory pre-created with mode `0500` (r-x, not writable) inside `base`.
fn make_readonly_dir(base: &Path, name: &str) -> PathBuf {
    let ro = base.join(name);
    std::fs::create_dir_all(&ro).expect("create ro dir");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500))
        .expect("chmod 0500 ro dir");
    ro
}

/// Root ignores 0500 directory modes, which would make the failure-injection tests
/// meaningless. Detect that with std only: if this process CAN create a file inside
/// the 0500 dir, the caller must skip (same probe as the s1 file).
fn readonly_dir_is_writable(ro: &Path) -> bool {
    let probe = ro.join("write-probe");
    if std::fs::write(&probe, b"probe").is_ok() {
        let _ = std::fs::remove_file(&probe);
        return true;
    }
    false
}

/// A `flowplane serve` command in DEV mode with a clean, fully explicit child env.
///
/// Deliberately sets NEITHER `FLOWPLANE_DEV_TOKEN_PATH` nor `FLOWPLANE_CONFIG`
/// (`env_clear` guarantees nothing leaks from the parent): the *default* sink is the
/// baseline, and tests that need an explicit source add it on the returned `Command`.
///
/// Env extras mirror the s1 file's documented recipe: `FLOWPLANE_API_INSECURE=true`
/// (no TLS material in tests), `FLOWPLANE_XDS_ADDR=127.0.0.1:0` (the xDS listener
/// otherwise defaults to a FIXED port), and `FLOWPLANE_DEV_MODE_ACK` (release builds
/// refuse dev mode without it).
fn dev_serve_cmd(home: &Path, db_url: &str, api_addr: &str, stderr_log: &Path) -> Command {
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
    cmd.env("FLOWPLANE_API_ADDR", api_addr);
    cmd.env("FLOWPLANE_XDS_ADDR", "127.0.0.1:0");
    cmd.env("FLOWPLANE_API_INSECURE", "true");
    cmd.arg("serve");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    // Capture stderr to a file (not a pipe nobody drains) purely for failure diagnostics.
    cmd.stderr(Stdio::from(
        File::create(stderr_log).expect("create server stderr log"),
    ));
    cmd
}

/// A `flowplane serve` command WITHOUT dev mode (no `FLOWPLANE_DEV_MODE`, no ack).
/// `FLOWPLANE_BOOTSTRAP_TOKEN` is required so an uninitialized instance can boot; on an
/// already-initialized shared test DB the server proceeds either way.
fn non_dev_serve_cmd(
    home: &Path,
    db_url: &str,
    api_addr: &str,
    bootstrap_token: &str,
    stderr_log: &Path,
) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home);
    cmd.env("FLOWPLANE_DATABASE_URL", db_url);
    if let Some(key) = std::env::var_os("FLOWPLANE_SECRET_ENCRYPTION_KEY") {
        cmd.env("FLOWPLANE_SECRET_ENCRYPTION_KEY", key);
    }
    cmd.env("FLOWPLANE_BOOTSTRAP_TOKEN", bootstrap_token);
    cmd.env("FLOWPLANE_API_ADDR", api_addr);
    cmd.env("FLOWPLANE_XDS_ADDR", "127.0.0.1:0");
    cmd.env("FLOWPLANE_API_INSECURE", "true");
    cmd.arg("serve");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::from(
        File::create(stderr_log).expect("create server stderr log"),
    ));
    cmd
}

/// Poll until `path` exists as a NON-EMPTY file; fail fast (with the stderr tail) if the
/// server exits first or the deadline passes.
fn wait_for_nonempty_file(path: &Path, guard: &mut ChildGuard, stderr_log: &Path) {
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if path.is_file()
            && std::fs::metadata(path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            return;
        }
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before writing {}; stderr tail:\n{}",
                path.display(),
                stderr_tail(stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "token file did not appear at {} within {SERVER_DEADLINE:?}; stderr tail:\n{}",
                path.display(),
                stderr_tail(stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Poll until a plain TCP connect to `127.0.0.1:<port>` succeeds (the API listener is
/// up); fail fast (with the stderr tail) if the server exits first or the deadline passes.
fn wait_for_tcp(port: u16, guard: &mut ChildGuard, stderr_log: &Path) {
    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .expect("parse loopback addr");
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
            return;
        }
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before the API port {port} accepted connections; \
                 stderr tail:\n{}",
                stderr_tail(stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "API port {port} never accepted a TCP connection within {SERVER_DEADLINE:?}; \
                 stderr tail:\n{}",
                stderr_tail(stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// One `flowplane auth whoami` invocation with a clean client env (no dependency on the
/// server child's env; `FLOWPLANE_CONFIG` deliberately absent).
fn whoami_once(client_home: &Path, server_url: &str, token: &str) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", client_home);
    cmd.args(["auth", "whoami", "--server", server_url, "--token", token]);
    cmd.output().expect("run flowplane auth whoami")
}

/// Retry `auth whoami` until it exits 0 (authenticated OK) — never a blind sleep. On
/// server exit or deadline, panic with both the last whoami stderr and the server's
/// stderr tail.
fn wait_for_whoami_ok(
    client_home: &Path,
    server_url: &str,
    token: &str,
    guard: &mut ChildGuard,
    stderr_log: &Path,
) {
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        let out = whoami_once(client_home, server_url, token);
        if out.status.success() {
            return;
        }
        let last_stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before whoami authenticated; last whoami stderr: \
                 {last_stderr:?}; server stderr tail:\n{}",
                stderr_tail(stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "auth whoami with the file token never exited 0 within {SERVER_DEADLINE:?}; \
                 last whoami stderr: {last_stderr:?}; server stderr tail:\n{}",
                stderr_tail(stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

// ---------------------------------------------------------------------------------------------
// AC1 + E2E: default sink — dev boot with NO dev-token path configured (neither env nor
// config file) writes `$HOME/.flowplane/dev-token` (non-empty, 0600, dir 0700), and the
// token READ FROM THAT FILE authenticates via `auth whoami` against the same server.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac1_default_sink_writes_home_dev_token_and_it_authenticates() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac1_default_sink_writes_home_dev_token_and_it_authenticates: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir(); // stderr log + client HOME, outside the server HOME
    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");

    let child = dev_serve_cmd(&home, &db_url, &format!("127.0.0.1:{port}"), &stderr_log)
        .spawn()
        .expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    // AC1: the default sink appears with private modes.
    let flowplane_dir = home.join(".flowplane");
    let token_path = flowplane_dir.join("dev-token");
    wait_for_nonempty_file(&token_path, &mut guard, &stderr_log);
    assert_eq!(
        mode_bits(&token_path),
        0o600,
        "$HOME/.flowplane/dev-token must be written with mode 0600"
    );
    assert_eq!(
        mode_bits(&flowplane_dir),
        0o700,
        "$HOME/.flowplane must be created with mode 0700"
    );

    // E2E: the token read from THAT FILE (trimmed) authenticates against the API.
    let token = std::fs::read_to_string(&token_path)
        .expect("read dev-token file")
        .trim()
        .to_string();
    assert!(
        !token.is_empty(),
        "the dev-token file must hold a non-empty bearer token after trimming"
    );
    wait_for_whoami_ok(
        &work,
        &format!("http://127.0.0.1:{port}"),
        &token,
        &mut guard,
        &stderr_log,
    );
    // guard drops here: kill + wait the child on every path.
}

// ---------------------------------------------------------------------------------------------
// AC2a: env-explicit suppresses the default — with FLOWPLANE_DEV_TOKEN_PATH pointing at a
// nested path in ANOTHER temp dir, that file appears (0600) and `$HOME/.flowplane/dev-token`
// is never created.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac2a_env_explicit_path_suppresses_default_sink() {
    let Some(db_url) = test_database_url() else {
        eprintln!(
            "skipping ac2a_env_explicit_path_suppresses_default_sink: FLOWPLANE_TEST_DATABASE_URL not set"
        );
        return;
    };

    let home = common::unique_tempdir();
    let other = common::unique_tempdir();
    let explicit_path = other.join("nested").join("dev-token");
    let stderr_log = other.join("serve-stderr.log");

    let mut cmd = dev_serve_cmd(&home, &db_url, "127.0.0.1:0", &stderr_log);
    cmd.env("FLOWPLANE_DEV_TOKEN_PATH", &explicit_path);
    let child = cmd.spawn().expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    wait_for_nonempty_file(&explicit_path, &mut guard, &stderr_log);
    assert_eq!(
        mode_bits(&explicit_path),
        0o600,
        "the env-explicit dev-token file must be written with mode 0600"
    );

    // The explicit sink exists ⇒ the write phase is over: the default must NOT exist.
    let default_path = home.join(".flowplane").join("dev-token");
    assert!(
        !default_path.exists(),
        "an env-explicit FLOWPLANE_DEV_TOKEN_PATH must SUPPRESS the default sink, but {} exists; \
         stderr tail:\n{}",
        default_path.display(),
        stderr_tail(&stderr_log)
    );
}

// ---------------------------------------------------------------------------------------------
// AC2b (fatal): TOML-explicit path (`dev_token_path` in the FLOWPLANE_CONFIG file, no env
// var) inside a 0500 directory — the server must exit ON ITS OWN, non-zero, with
// "failed to write dev token" on stderr, and the default sink must never be created.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac2b_toml_explicit_unwritable_path_is_fatal_and_default_stays_absent() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac2b_toml_explicit_unwritable_path_is_fatal_and_default_stays_absent: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir();
    let ro = make_readonly_dir(&work, "ro");
    if readonly_dir_is_writable(&ro) {
        eprintln!("skipping ac2b_toml_explicit_unwritable_path_is_fatal_and_default_stays_absent: 0500 dir is writable (running as root?)");
        return;
    }

    let toml_token_path = ro.join("dev-token");
    let config_path = work.join("server-config.toml");
    std::fs::write(
        &config_path,
        format!("dev_token_path = \"{}\"\n", toml_token_path.display()),
    )
    .expect("write server config toml");

    let stderr_log = work.join("serve-stderr.log");
    let mut cmd = dev_serve_cmd(&home, &db_url, "127.0.0.1:0", &stderr_log);
    // TOML is the ONLY explicit source here: env FLOWPLANE_DEV_TOKEN_PATH stays absent
    // (env_clear in dev_serve_cmd guarantees it).
    cmd.env("FLOWPLANE_CONFIG", &config_path);
    let child = cmd.spawn().expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    let default_path = home.join(".flowplane").join("dev-token");
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            assert!(
                !status.success(),
                "serve must exit NON-ZERO when the TOML-named dev-token path is unwritable, \
                 got {status}; stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
            // The exit must be caused by THIS failure, not an unrelated startup error.
            let stderr = stderr_tail(&stderr_log);
            assert!(
                stderr.contains("failed to write dev token"),
                "serve must fail BECAUSE of the dev-token write; stderr tail:\n{stderr}"
            );
            assert!(
                !default_path.exists(),
                "a TOML-explicit dev_token_path must suppress the default sink even on failure, \
                 but {} exists",
                default_path.display()
            );
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "serve was still running {SERVER_DEADLINE:?} after boot despite an unwritable \
                 TOML-explicit dev-token path — the write failure must be FATAL; stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

// ---------------------------------------------------------------------------------------------
// AC2b (happy): TOML-explicit path in a WRITABLE temp dir — the token appears at the TOML
// path with mode 0600, and the default sink stays absent.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac2b_toml_explicit_writable_path_wins_and_default_stays_absent() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac2b_toml_explicit_writable_path_wins_and_default_stays_absent: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir();
    let sink_dir = work.join("toml-sink");
    std::fs::create_dir_all(&sink_dir).expect("create toml sink dir");
    let toml_token_path = sink_dir.join("dev-token");
    let config_path = work.join("server-config.toml");
    std::fs::write(
        &config_path,
        format!("dev_token_path = \"{}\"\n", toml_token_path.display()),
    )
    .expect("write server config toml");

    let stderr_log = work.join("serve-stderr.log");
    let mut cmd = dev_serve_cmd(&home, &db_url, "127.0.0.1:0", &stderr_log);
    cmd.env("FLOWPLANE_CONFIG", &config_path);
    let child = cmd.spawn().expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    wait_for_nonempty_file(&toml_token_path, &mut guard, &stderr_log);
    assert_eq!(
        mode_bits(&toml_token_path),
        0o600,
        "the TOML-explicit dev-token file must be written with mode 0600"
    );

    let default_path = home.join(".flowplane").join("dev-token");
    assert!(
        !default_path.exists(),
        "a TOML-explicit dev_token_path must SUPPRESS the default sink, but {} exists; \
         stderr tail:\n{}",
        default_path.display(),
        stderr_tail(&stderr_log)
    );
}

// ---------------------------------------------------------------------------------------------
// AC3: unwritable HOME is NON-fatal for the DEFAULT sink — with HOME a 0500 dir (so
// `.flowplane` cannot be created) and no explicit path, the server keeps booting and
// serves: the API port accepts a TCP connection and the child is still alive 2s later.
// No token file exists (and no fishing the token out of logs).
// ---------------------------------------------------------------------------------------------
#[test]
fn ac3_unwritable_home_default_sink_failure_is_nonfatal_and_server_serves() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac3_unwritable_home_default_sink_failure_is_nonfatal_and_server_serves: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let work = common::unique_tempdir();
    let home = make_readonly_dir(&work, "home");
    if readonly_dir_is_writable(&home) {
        eprintln!("skipping ac3_unwritable_home_default_sink_failure_is_nonfatal_and_server_serves: 0500 dir is writable (running as root?)");
        return;
    }

    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");
    let child = dev_serve_cmd(&home, &db_url, &format!("127.0.0.1:{port}"), &stderr_log)
        .spawn()
        .expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    // Serving despite the failed default write: the listener comes up...
    wait_for_tcp(port, &mut guard, &stderr_log);
    // ...and the child is STILL RUNNING 2s after the port accepted (not a dying gasp).
    std::thread::sleep(Duration::from_secs(2));
    let exited = guard.0.try_wait().expect("try_wait serve child");
    assert!(
        exited.is_none(),
        "serve must KEEP RUNNING when only the DEFAULT dev-token sink is unwritable, but it \
         exited ({}); stderr tail:\n{}",
        exited.map(|s| s.to_string()).unwrap_or_default(),
        stderr_tail(&stderr_log)
    );

    let default_path = home.join(".flowplane").join("dev-token");
    assert!(
        !default_path.exists(),
        "no dev-token file may exist under an unwritable HOME, but {} exists",
        default_path.display()
    );
}

// ---------------------------------------------------------------------------------------------
// AC10: non-dev boot writes nothing — without FLOWPLANE_DEV_MODE (and without the ack),
// with a bootstrap token so an uninitialized instance can boot, the server comes up
// (TCP connect succeeds) and neither `$HOME/.flowplane/dev-token` nor `$HOME/.flowplane`
// itself is created.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac10_non_dev_boot_writes_no_dev_token_and_no_flowplane_dir() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac10_non_dev_boot_writes_no_dev_token_and_no_flowplane_dir: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir();
    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");

    // Unique, ≥32-char bootstrap token (pid + nanos make it unique per test process).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let bootstrap_token = format!("it-ac10-bootstrap-{:08}-{nanos:024}", std::process::id());
    assert!(
        bootstrap_token.len() >= 32,
        "test bug: bootstrap token must be at least 32 chars"
    );

    let child = non_dev_serve_cmd(
        &home,
        &db_url,
        &format!("127.0.0.1:{port}"),
        &bootstrap_token,
        &stderr_log,
    )
    .spawn()
    .expect("spawn flowplane serve (non-dev)");
    let mut guard = ChildGuard(child);

    // The server is up (initialized or not, it must proceed to serving)...
    wait_for_tcp(port, &mut guard, &stderr_log);

    // ...and it minted NO dev token: neither the file nor the directory may exist.
    let flowplane_dir = home.join(".flowplane");
    let default_path = flowplane_dir.join("dev-token");
    assert!(
        !default_path.exists(),
        "a NON-dev boot must not write a dev-token file, but {} exists; stderr tail:\n{}",
        default_path.display(),
        stderr_tail(&stderr_log)
    );
    assert!(
        !flowplane_dir.exists(),
        "a NON-dev boot must not create $HOME/.flowplane at all, but {} exists; stderr tail:\n{}",
        flowplane_dir.display(),
        stderr_tail(&stderr_log)
    );
    // guard drops here: kill + wait the child.
}
