//! fpv2-wvp.3 — bootstrap-token file sink (black-box).
//!
//! Contract under test: a NON-dev, uninitialized server booted with the local-only
//! escape hatch `FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN=yes-this-is-local-only`
//! generates a one-shot bootstrap token, logs it at WARN (the existing documented
//! interface — the log record carries `bootstrap_token=<value>`), and NOW ALSO writes
//! the same token to `$HOME/.flowplane/bootstrap-token` (file mode 0600). That file
//! write is best-effort: an unwritable HOME must not stop the boot. The
//! operator-supplied path (`FLOWPLANE_BOOTSTRAP_TOKEN=<value>`) never logs and never
//! writes the token anywhere.
//!
//! SHARED-STATE RULE honored throughout: the shared test database's
//! bootstrap-initialized flag is irreversible global product state, so these tests
//! NEVER call `POST /api/v1/bootstrap/initialize` with a VALID token. The endpoint
//! liveness check (AC8b) sends a deliberately WRONG token and expects exactly 401. If the
//! shared instance is already initialized, the ack-path boot issues no token at all;
//! that condition is detected ("server up, no `bootstrap_token` in stderr, no file
//! after the deadline") and reported as a SKIP, not a failure.
//!
//! Conventions honored (same as the s1/s2 files): the built `flowplane` binary is
//! driven as a subprocess, unique temp dirs per test, no fixed TCP ports (API ports
//! allocated by binding 127.0.0.1:0 and releasing), all env goes on the child
//! `Command` only (never `std::env::set_var`), children are killed+reaped by a
//! `ChildGuard`, stderr is captured to a per-child file whose tail rides every
//! failure message, and DB-backed tests skip with a note when
//! `FLOWPLANE_TEST_DATABASE_URL` is unset. Unix-only: permission bits (0600) and the
//! 0500-HOME failure injection are unix.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the server child to write files / answer / come up.
const SERVER_DEADLINE: Duration = Duration::from_secs(90);
/// Poll interval while waiting on the child.
const POLL_STEP: Duration = Duration::from_millis(250);
/// The documented local-only escape-hatch acknowledgement value.
const ACK_ENV: &str = "FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN";
const ACK_VALUE: &str = "yes-this-is-local-only";

/// The unix permission bits of a path (e.g. `0o600`), sans file-type bits.
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

/// The sibling file capturing the child's STDOUT. Tracing log records (WARN etc.) go to
/// stdout; only the fatal `Error: …` from a failed boot lands on stderr — log-content
/// assertions must therefore look at BOTH streams.
fn sibling_stdout_log(stderr_log: &Path) -> PathBuf {
    stderr_log.with_extension("stdout.log")
}

/// The child's full captured output: stderr followed by stdout.
fn server_logs(stderr_log: &Path) -> String {
    let stderr = std::fs::read_to_string(stderr_log).unwrap_or_default();
    let stdout = std::fs::read_to_string(sibling_stdout_log(stderr_log)).unwrap_or_default();
    format!("{stderr}\n{stdout}")
}

/// Best-effort readback of the child's captured output (both streams) for failure
/// diagnostics.
fn stderr_tail(log: &Path) -> String {
    let text = server_logs(log);
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

/// Root ignores 0500 directory modes, which would make the failure-injection test
/// meaningless. Detect that with std only: if this process CAN create a file inside
/// the 0500 dir, the caller must skip (same probe as the s1/s2 files).
fn readonly_dir_is_writable(ro: &Path) -> bool {
    let probe = ro.join("write-probe");
    if std::fs::write(&probe, b"probe").is_ok() {
        let _ = std::fs::remove_file(&probe);
        return true;
    }
    false
}

/// A `flowplane serve` command WITHOUT dev mode and with a clean, fully explicit child
/// env (`env_clear` guarantees neither `FLOWPLANE_DEV_MODE` nor any other var leaks in
/// from the parent — the "env_remove where needed" is subsumed by the clear).
///
/// Deliberately sets NEITHER `FLOWPLANE_BOOTSTRAP_TOKEN` nor the ack env: each test
/// adds exactly the bootstrap-token source it exercises on the returned `Command`.
///
/// Env extras mirror the s2 file's documented non-dev recipe: `FLOWPLANE_API_INSECURE=true`
/// (no TLS material in tests) and `FLOWPLANE_XDS_ADDR=127.0.0.1:0` (the xDS listener
/// otherwise defaults to a FIXED port, breaking parallel-safety).
fn non_dev_serve_cmd(home: &Path, db_url: &str, api_addr: &str, stderr_log: &Path) -> Command {
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
    cmd.env("FLOWPLANE_API_ADDR", api_addr);
    cmd.env("FLOWPLANE_XDS_ADDR", "127.0.0.1:0");
    cmd.env("FLOWPLANE_API_INSECURE", "true");
    cmd.arg("serve");
    cmd.stdin(Stdio::null());
    // Capture BOTH streams to files (not pipes nobody drains): tracing records go to
    // stdout, fatal boot errors to stderr — assertions and diagnostics need both.
    cmd.stdout(Stdio::from(
        File::create(sibling_stdout_log(stderr_log)).expect("create server stdout log"),
    ));
    cmd.stderr(Stdio::from(
        File::create(stderr_log).expect("create server stderr log"),
    ));
    cmd
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

/// Outcome of an ack-path boot with a writable HOME.
enum AckBootOutcome {
    /// `$HOME/.flowplane/bootstrap-token` appeared non-empty; carries the RAW file
    /// contents (untrimmed — AC8a asserts byte-for-byte equality with the logged token).
    TokenFile(String),
    /// Server is up but issued NO token (no file, no `bootstrap_token` record in
    /// stderr) — the shared instance is already initialized ⇒ caller must SKIP.
    AlreadyInitialized,
}

/// Poll (≤ deadline) for `$HOME/.flowplane/bootstrap-token` to exist non-empty.
///
/// Disambiguates the shared-state cases:
///   * file appears        → `TokenFile` (the contract's file sink worked);
///   * child exits         → panic (boot must survive the ack path);
///   * deadline + a `bootstrap_token` stderr record but NO file → panic (the token was
///     logged, so this boot DID issue one — the missing file is an AC8a violation);
///   * deadline + no record + API port up → `AlreadyInitialized` (skip);
///   * deadline + no record + port down   → panic (hung boot, not an initialized DB).
fn wait_for_bootstrap_token_file(
    home: &Path,
    port: u16,
    guard: &mut ChildGuard,
    stderr_log: &Path,
) -> AckBootOutcome {
    let token_path = home.join(".flowplane").join("bootstrap-token");
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if token_path.is_file()
            && std::fs::metadata(&token_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            // RAW contents, deliberately untrimmed: AC8a asserts byte-for-byte equality
            // with the logged token, so stray leading/trailing bytes must fail the test.
            let token = std::fs::read_to_string(&token_path).expect("read bootstrap-token file");
            assert!(
                !token.is_empty(),
                "the bootstrap-token file must hold a non-empty token; stderr tail:\n{}",
                stderr_tail(stderr_log)
            );
            return AckBootOutcome::TokenFile(token);
        }
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before writing {}; stderr tail:\n{}",
                token_path.display(),
                stderr_tail(stderr_log)
            );
        }
        if Instant::now() >= deadline {
            if server_logs(stderr_log).contains("bootstrap_token") {
                panic!(
                    "the boot LOGGED a bootstrap_token record but never wrote {} within \
                     {SERVER_DEADLINE:?} — the file sink is part of the contract; \
                     stderr tail:\n{}",
                    token_path.display(),
                    stderr_tail(stderr_log)
                );
            }
            let addr: SocketAddr = format!("127.0.0.1:{port}")
                .parse()
                .expect("parse loopback addr");
            if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
                return AckBootOutcome::AlreadyInitialized;
            }
            panic!(
                "serve neither issued a bootstrap token nor accepted connections on port \
                 {port} within {SERVER_DEADLINE:?}; stderr tail:\n{}",
                stderr_tail(stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Liberal token extraction from one stderr line: locate every `bootstrap_token`
/// occurrence, skip separator characters (`"`, `:`, `=`, whitespace), and take the run
/// up to a terminator (`"`, `\`, `,`, `}`, whitespace). JSON-tracing shapes like
/// `"bootstrap_token":"<v>"` and message text like `bootstrap_token=<v>` both yield
/// `<v>`; prose like "bootstrap_token generated" yields a word that simply won't match
/// the real token, so callers compare candidates for exact equality.
fn bootstrap_token_candidates(line: &str) -> Vec<String> {
    const NEEDLE: &str = "bootstrap_token";
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find(NEEDLE) {
        let after = search_from + rel + NEEDLE.len();
        search_from = after;
        let rest = line[after..]
            .trim_start_matches(|c: char| c == '"' || c == ':' || c == '=' || c.is_whitespace());
        let value: String = rest
            .chars()
            .take_while(|c| !matches!(c, '"' | '\\' | ',' | '}') && !c.is_whitespace())
            .collect();
        if !value.is_empty() {
            out.push(value);
        }
    }
    out
}

/// All bootstrap-token candidate values across the whole captured output (both streams).
fn all_bootstrap_token_candidates(stderr_log: &Path) -> Vec<String> {
    let text = server_logs(stderr_log);
    text.lines()
        .filter(|l| l.contains("bootstrap_token"))
        .flat_map(bootstrap_token_candidates)
        .collect()
}

/// One raw HTTP/1.1 `POST /api/v1/bootstrap/initialize` with a bearer token and a VALID,
/// schema-complete body — so a rejection can only come from token validation, never from
/// body deserialization (which would short-circuit before authentication and make the
/// wrong-token assertion vacuous). Returns the parsed status code, or `None` when the
/// exchange didn't complete (caller retries under its deadline). std-only on purpose.
fn post_bootstrap_initialize(port: u16, bearer: &str) -> Option<u16> {
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    let body = r#"{"org_name":"it-ac8b-org","admin_subject":"it-ac8b-admin"}"#;
    let request = format!(
        "POST /api/v1/bootstrap/initialize HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {bearer}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut buf = Vec::new();
    // A read error after partial data still leaves the status line usable.
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let status_line = text.lines().next()?;
    // "HTTP/1.1 401 Unauthorized" → 401
    status_line.split_whitespace().nth(1)?.parse::<u16>().ok()
}

/// A token that differs from `token` in its first 4 characters but has the same length.
fn corrupt_token(token: &str) -> String {
    assert!(
        token.chars().count() >= 4,
        "test bug: bootstrap token too short to corrupt: {token:?}"
    );
    token
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i < 4 {
                if c == 'x' {
                    'y'
                } else {
                    'x'
                }
            } else {
                c
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// AC8a: file sink + log retained — a non-dev ack-path boot writes the one-shot bootstrap
// token to `$HOME/.flowplane/bootstrap-token` with mode 0600, stderr still carries the
// documented `bootstrap_token` WARN record, and the file's token equals the logged token
// byte-for-byte (the file carries the real credential without consuming it).
// ---------------------------------------------------------------------------------------------
#[test]
fn ac8a_ack_boot_writes_0600_token_file_matching_the_warn_log() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac8a_ack_boot_writes_0600_token_file_matching_the_warn_log: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir(); // stderr log outside the server HOME
    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");

    let mut cmd = non_dev_serve_cmd(&home, &db_url, &format!("127.0.0.1:{port}"), &stderr_log);
    cmd.env(ACK_ENV, ACK_VALUE);
    let child = cmd.spawn().expect("spawn flowplane serve (non-dev, ack)");
    let mut guard = ChildGuard(child);

    let file_token = match wait_for_bootstrap_token_file(&home, port, &mut guard, &stderr_log) {
        AckBootOutcome::TokenFile(token) => token,
        AckBootOutcome::AlreadyInitialized => {
            eprintln!("skipping ac8a_ack_boot_writes_0600_token_file_matching_the_warn_log: instance already initialized (server up, no bootstrap_token in stderr, no file)");
            return;
        }
    };

    let token_path = home.join(".flowplane").join("bootstrap-token");
    assert_eq!(
        mode_bits(&token_path),
        0o600,
        "$HOME/.flowplane/bootstrap-token must be written with mode 0600; stderr tail:\n{}",
        stderr_tail(&stderr_log)
    );

    // The WARN log interface is RETAINED: a `bootstrap_token` record must be present, and
    // its token value must equal the file's, byte for byte. Poll briefly — the file and
    // the log line are written around the same moment and stderr flushing may lag.
    let deadline = Instant::now() + SERVER_DEADLINE;
    let candidates = loop {
        let candidates = all_bootstrap_token_candidates(&stderr_log);
        if !candidates.is_empty() || Instant::now() >= deadline {
            break candidates;
        }
        std::thread::sleep(POLL_STEP);
    };
    assert!(
        !candidates.is_empty(),
        "stderr must retain the documented bootstrap_token WARN record; stderr tail:\n{}",
        stderr_tail(&stderr_log)
    );
    assert!(
        candidates.iter().any(|c| c == &file_token),
        "the token in $HOME/.flowplane/bootstrap-token must equal the logged bootstrap_token \
         byte-for-byte; file token: {file_token:?}, logged candidates: {candidates:?}; \
         stderr tail:\n{}",
        stderr_tail(&stderr_log)
    );
    // guard drops here: kill + wait the child on every path.
}

// ---------------------------------------------------------------------------------------------
// AC8b: E2E endpoint fail-closed, no consumption — after the token file exists, a POST to
// /api/v1/bootstrap/initialize with a valid body and a WRONG bearer (the file token with 4
// chars changed) gets exactly 401. Proves the endpoint is live and fail-closed while leaving the
// shared DB's irreversible initialized flag untouched (NEVER posts the valid token).
// ---------------------------------------------------------------------------------------------
#[test]
fn ac8b_initialize_endpoint_rejects_wrong_token_without_consuming() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac8b_initialize_endpoint_rejects_wrong_token_without_consuming: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir();
    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");

    let mut cmd = non_dev_serve_cmd(&home, &db_url, &format!("127.0.0.1:{port}"), &stderr_log);
    cmd.env(ACK_ENV, ACK_VALUE);
    let child = cmd.spawn().expect("spawn flowplane serve (non-dev, ack)");
    let mut guard = ChildGuard(child);

    let file_token = match wait_for_bootstrap_token_file(&home, port, &mut guard, &stderr_log) {
        AckBootOutcome::TokenFile(token) => token,
        AckBootOutcome::AlreadyInitialized => {
            eprintln!("skipping ac8b_initialize_endpoint_rejects_wrong_token_without_consuming: instance already initialized (server up, no bootstrap_token in stderr, no file)");
            return;
        }
    };

    // SHARED-STATE RULE: only ever POST a token that is guaranteed WRONG.
    let wrong_token = corrupt_token(&file_token);
    assert_ne!(
        wrong_token, file_token,
        "test bug: the corrupted token must differ from the real one"
    );

    // The file exists, but the API listener may still be settling: wait for the port,
    // then retry the raw HTTP exchange until a status line comes back.
    wait_for_tcp(port, &mut guard, &stderr_log);
    let deadline = Instant::now() + SERVER_DEADLINE;
    let status = loop {
        if let Some(code) = post_bootstrap_initialize(port, &wrong_token) {
            break code;
        }
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before answering the bootstrap/initialize probe; \
                 stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "POST /api/v1/bootstrap/initialize never returned a status line within \
                 {SERVER_DEADLINE:?}; stderr tail:\n{}",
                stderr_tail(&stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    };
    // Exactly 401: the body is valid and schema-complete, so the ONLY thing that can be
    // rejected here is the corrupted bearer token. A 4xx from body handling (e.g. 422)
    // would mean the token check was never reached — a vacuous test — so it must fail.
    assert_eq!(
        status,
        401,
        "POST /api/v1/bootstrap/initialize with a valid body and a WRONG bearer token \
         must be rejected by TOKEN validation (401); stderr tail:\n{}",
        stderr_tail(&stderr_log)
    );
    // guard drops here: kill + wait the child on every path.
}

// ---------------------------------------------------------------------------------------------
// AC8c: unwritable HOME is best-effort — with HOME a 0500 dir, the ack-path boot must
// KEEP RUNNING and serve (API port accepts TCP, child still alive 2s later) and no
// bootstrap-token file exists under HOME. (The token WARN may still be on stderr — that
// interface is unchanged — so its presence is deliberately NOT asserted either way.)
// ---------------------------------------------------------------------------------------------
#[test]
fn ac8c_unwritable_home_token_file_write_is_best_effort_nonfatal() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac8c_unwritable_home_token_file_write_is_best_effort_nonfatal: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let work = common::unique_tempdir();
    let home = make_readonly_dir(&work, "home");
    if readonly_dir_is_writable(&home) {
        eprintln!("skipping ac8c_unwritable_home_token_file_write_is_best_effort_nonfatal: 0500 dir is writable (running as root?)");
        return;
    }

    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");
    let mut cmd = non_dev_serve_cmd(&home, &db_url, &format!("127.0.0.1:{port}"), &stderr_log);
    cmd.env(ACK_ENV, ACK_VALUE);
    let child = cmd.spawn().expect("spawn flowplane serve (non-dev, ack)");
    let mut guard = ChildGuard(child);

    // Serving despite the failed best-effort write: the listener comes up...
    wait_for_tcp(port, &mut guard, &stderr_log);
    // ...and the child is STILL RUNNING 2s after the port accepted (not a dying gasp).
    std::thread::sleep(Duration::from_secs(2));
    let exited = guard.0.try_wait().expect("try_wait serve child");
    assert!(
        exited.is_none(),
        "serve must KEEP RUNNING when the bootstrap-token file sink is unwritable \
         (best-effort write), but it exited ({}); stderr tail:\n{}",
        exited.map(|s| s.to_string()).unwrap_or_default(),
        stderr_tail(&stderr_log)
    );

    let token_path = home.join(".flowplane").join("bootstrap-token");
    assert!(
        !token_path.exists(),
        "no bootstrap-token file may exist under an unwritable HOME, but {} exists; \
         stderr tail:\n{}",
        token_path.display(),
        stderr_tail(&stderr_log)
    );
    // guard drops here: kill + wait the child on every path.
}

// ---------------------------------------------------------------------------------------------
// AC9: operator path never logs, never writes — booted with FLOWPLANE_BOOTSTRAP_TOKEN
// carrying a distinctive marker and NO ack env, a fresh writable HOME ends the boot with
// (1) no `$HOME/.flowplane/bootstrap-token`, (2) the marker NOWHERE in the full stderr,
// and (3) `$HOME/.flowplane` never created at all.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac9_operator_token_never_logged_and_never_written() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac9_operator_token_never_logged_and_never_written: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir();
    let work = common::unique_tempdir();
    let port = alloc_port();
    let stderr_log = work.join("serve-stderr.log");

    // The suite-wide FIXED operator token (see its doc in tests/common): the server keeps
    // one live operator-seeded token hash per uninitialized instance and fails boot on any
    // DIFFERENT operator token ("divergent replica" fail-closed), so a per-test unique
    // token would poison the shared database for every later boot. The constant embeds
    // the marker no legitimate log line would contain.
    const MARKER: &str = "OPERATORSECRETMARKER";
    let operator_token = common::SHARED_OPERATOR_BOOTSTRAP_TOKEN;
    assert!(
        operator_token.len() >= 40 && operator_token.contains(MARKER),
        "test bug: shared operator token must be ≥40 chars and carry the marker"
    );

    let mut cmd = non_dev_serve_cmd(&home, &db_url, &format!("127.0.0.1:{port}"), &stderr_log);
    // Operator-supplied path ONLY: bootstrap token env, no ack env (env_clear in the
    // builder guarantees FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN is absent).
    cmd.env("FLOWPLANE_BOOTSTRAP_TOKEN", operator_token);
    let child = cmd
        .spawn()
        .expect("spawn flowplane serve (non-dev, operator token)");
    let mut guard = ChildGuard(child);

    // The server is up (boot — including any token handling — has happened)...
    wait_for_tcp(port, &mut guard, &stderr_log);

    // (1) ...no bootstrap-token file was written...
    let flowplane_dir = home.join(".flowplane");
    let token_path = flowplane_dir.join("bootstrap-token");
    assert!(
        !token_path.exists(),
        "an operator-supplied FLOWPLANE_BOOTSTRAP_TOKEN must never be written to disk, \
         but {} exists; stderr tail:\n{}",
        token_path.display(),
        stderr_tail(&stderr_log)
    );
    // (2) ...the token value appears NOWHERE in the full captured output (both streams —
    // tracing records land on stdout)...
    let full_output = server_logs(&stderr_log);
    assert!(
        !full_output.contains(MARKER),
        "an operator-supplied FLOWPLANE_BOOTSTRAP_TOKEN must never be logged, but the \
         marker {MARKER:?} appears in the captured output; tail:\n{}",
        stderr_tail(&stderr_log)
    );
    // (3) ...and the operator path created no $HOME/.flowplane directory at all.
    assert!(
        !flowplane_dir.exists(),
        "the operator-supplied path must not create $HOME/.flowplane, but {} exists; \
         stderr tail:\n{}",
        flowplane_dir.display(),
        stderr_tail(&stderr_log)
    );
    // guard drops here: kill + wait the child on every path.
}
