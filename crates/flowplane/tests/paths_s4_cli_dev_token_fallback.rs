//! fpv2-wvp.4 — CLI dev-token fallback (black-box).
//!
//! Contract under test: the CLI gains a LOWEST-precedence bearer-token source — the
//! contents of `$HOME/.flowplane/dev-token` (trimmed) — applied ONLY when the effective
//! server URL's host is literally loopback (`localhost`, `127.0.0.0/8`, `::1`; no DNS
//! resolution). When the fallback is used, the CLI prints exactly one stderr note:
//! `using dev token from ~/.flowplane/dev-token (dev mode)` — and stdout stays clean.
//! On a 401 with the fallback token, the error output additionally includes a
//! stale-token hint naming `~/.flowplane/dev-token`. EVERY other token source
//! (`--token` flag, `FLOWPLANE_TOKEN` env, context token, config-file token,
//! credentials file) beats the dev file.
//!
//! Conventions honored (same as the s2/s3 files): the built `flowplane` binary is
//! driven as a subprocess; unique temp dirs per test; no fixed TCP ports (allocated by
//! binding 127.0.0.1:0 and releasing); all env goes on the child `Command` only (never
//! `std::env::set_var` — `flowplane_cmd` env-clears, so a parent `FLOWPLANE_TOKEN`
//! cannot leak); real-server children are killed+reaped by a `ChildGuard` with BOTH
//! stdout and stderr captured to files whose tails ride every failure message (tracing
//! records go to stdout); DB-backed tests skip with a note when
//! `FLOWPLANE_TEST_DATABASE_URL` is unset. Mock-backed tests need no database.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// The one-line stderr note the CLI must print when (and ONLY when) the fallback is used.
const DEV_NOTE: &str = "using dev token from ~/.flowplane/dev-token (dev mode)";

/// How long to wait for the server child to write files / answer.
const SERVER_DEADLINE: Duration = Duration::from_secs(90);
/// Poll interval while waiting on the child.
const POLL_STEP: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// Kills (and reaps) the child on drop, so a panicking assertion never leaks a running
/// `flowplane serve` process.
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

/// Allocate a unique loopback port: bind 127.0.0.1:0, read the port, release it.
fn alloc_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// A per-test unique token value (embeds pid + nanos so parallel tests never collide).
fn unique_token(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// Plant a dev-token file at the HOME-based path the fallback reads:
/// `<home>/.flowplane/dev-token`. Written with a trailing newline so the tests also
/// prove the CLI trims the file contents.
fn plant_dev_token(home: &Path, token: &str) {
    let dir = home.join(".flowplane");
    std::fs::create_dir_all(&dir).expect("create <home>/.flowplane");
    std::fs::write(dir.join("dev-token"), format!("{token}\n")).expect("write dev-token file");
}

/// Write the credentials file that lives next to the config path as `<config dir>/credentials`.
/// With `FLOWPLANE_CONFIG=<home>/config.toml` (set by `flowplane_cmd`), that is
/// `<home>/credentials`.
fn write_credentials(home: &Path, token: &str) {
    std::fs::write(home.join("credentials"), token).expect("write credentials");
}

/// Number of times `needle` occurs in `haystack`.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// Best-effort readback of the server child's captured stdout+stderr for diagnostics.
fn server_tail(stdout_log: &Path, stderr_log: &Path) -> String {
    let tail = |path: &Path| {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let tail: Vec<&str> = text.lines().rev().take(30).collect();
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    format!(
        "--- server stdout tail ---\n{}\n--- server stderr tail ---\n{}",
        tail(stdout_log),
        tail(stderr_log)
    )
}

/// A `flowplane serve` command in DEV mode with a clean, fully explicit child env
/// (the s2 recipe). No dev-token path is configured, so the server writes its per-boot
/// token to the default sink `$HOME/.flowplane/dev-token`. BOTH stdout and stderr are
/// captured to files (tracing records go to stdout).
fn dev_serve_cmd(
    home: &Path,
    db_url: &str,
    api_addr: &str,
    stdout_log: &Path,
    stderr_log: &Path,
) -> Command {
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
    cmd.stdout(Stdio::from(
        File::create(stdout_log).expect("create server stdout log"),
    ));
    cmd.stderr(Stdio::from(
        File::create(stderr_log).expect("create server stderr log"),
    ));
    cmd
}

/// Poll until `path` exists as a NON-EMPTY file; fail fast (with the server output tail)
/// if the server exits first or the deadline passes.
fn wait_for_nonempty_file(
    path: &Path,
    guard: &mut ChildGuard,
    stdout_log: &Path,
    stderr_log: &Path,
) {
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
                "serve exited ({status}) before writing {};\n{}",
                path.display(),
                server_tail(stdout_log, stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "token file did not appear at {} within {SERVER_DEADLINE:?};\n{}",
                path.display(),
                server_tail(stdout_log, stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Poll until the dev-token file's content differs from `old_token` (non-empty after
/// trimming). Returns the fresh token. Fails fast on server exit / deadline.
fn wait_for_token_change(
    path: &Path,
    old_token: &str,
    guard: &mut ChildGuard,
    stdout_log: &Path,
    stderr_log: &Path,
) -> String {
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let trimmed = text.trim();
            if !trimmed.is_empty() && trimmed != old_token {
                return trimmed.to_string();
            }
        }
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "restarted serve exited ({status}) before rewriting {};\n{}",
                path.display(),
                server_tail(stdout_log, stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "dev-token file content never changed from the previous boot's token within \
                 {SERVER_DEADLINE:?} — restart must mint a FRESH token;\n{}",
                server_tail(stdout_log, stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Retry a CLI invocation (with the isolated `flowplane_cmd` env: HOME=`home`, NO token
/// configured anywhere) until it exits 0, returning that successful `Output`. Never a
/// blind sleep. On server exit or deadline, panic with the last CLI stderr plus the
/// server's captured output tail.
fn wait_for_cli_ok(
    home: &Path,
    args: &[&str],
    guard: &mut ChildGuard,
    stdout_log: &Path,
    stderr_log: &Path,
) -> Output {
    let deadline = Instant::now() + SERVER_DEADLINE;
    loop {
        let out = common::flowplane_cmd(home)
            .args(args)
            .output()
            .expect("run flowplane CLI");
        if out.status.success() {
            return out;
        }
        let last_stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if let Some(status) = guard.0.try_wait().expect("try_wait serve child") {
            panic!(
                "serve exited ({status}) before `flowplane {}` succeeded; last CLI stderr: \
                 {last_stderr:?};\n{}",
                args.join(" "),
                server_tail(stdout_log, stderr_log)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "`flowplane {}` never exited 0 within {SERVER_DEADLINE:?}; last CLI stderr: \
                 {last_stderr:?};\n{}",
                args.join(" "),
                server_tail(stdout_log, stderr_log)
            );
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Parse the probe success envelope `{schemaVersion,kind,data}` from stdout, asserting
/// exit 0. Parsing the WHOLE stdout also proves the fallback note never leaks to stdout.
fn parse_probe(out: &Output, ctx: &str) -> Value {
    assert!(
        out.status.success(),
        "{ctx}: probe must exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout must be a JSON envelope and nothing else ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The `Bearer …` authorization header the CLI actually sent, as echoed by the probe.
fn received_authorization(out: &Output, ctx: &str) -> String {
    let v = parse_probe(out, ctx);
    v["data"]["received_authorization"]
        .as_str()
        .unwrap_or_else(|| panic!("{ctx}: probe must echo received_authorization: {v}"))
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// AC4 + AC7 (E2E, zero token configuration): boot a REAL dev CP sharing HOME with the
// CLI. With NO token configured anywhere (no --token, no FLOWPLANE_TOKEN, no config
// file, no credentials):
//   (a) `auth whoami --server http://127.0.0.1:<port>` exits 0 and stderr carries the
//       one-line fallback note exactly once (stdout stays clean of it);
//   (b) `cluster list --team default` exits 0;
//   (c) `auth token` exits 0, stdout is EXACTLY the file token + one trailing newline,
//       and stderr carries the note.
// AC7 (restart freshness): kill the server, boot a NEW one on a NEW port with the same
// HOME, wait for the dev-token file's CONTENT to change, then `auth whoami` against the
// new port exits 0 — the CLI transparently picks up the fresh token.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac4_ac7_zero_config_e2e_uses_dev_file_and_survives_restart() {
    let Some(db_url) = test_database_url() else {
        eprintln!("skipping ac4_ac7_zero_config_e2e_uses_dev_file_and_survives_restart: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    let home = common::unique_tempdir(); // SHARED by the server and the CLI
    let work = common::unique_tempdir(); // server logs, outside HOME
    let port = alloc_port();
    let url = format!("http://127.0.0.1:{port}");
    let stdout_log = work.join("serve1-stdout.log");
    let stderr_log = work.join("serve1-stderr.log");

    let child = dev_serve_cmd(
        &home,
        &db_url,
        &format!("127.0.0.1:{port}"),
        &stdout_log,
        &stderr_log,
    )
    .spawn()
    .expect("spawn flowplane serve");
    let mut guard = ChildGuard(child);

    let token_path = home.join(".flowplane").join("dev-token");
    wait_for_nonempty_file(&token_path, &mut guard, &stdout_log, &stderr_log);
    let token1 = std::fs::read_to_string(&token_path)
        .expect("read dev-token file")
        .trim()
        .to_string();
    assert!(
        !token1.is_empty(),
        "the dev-token file must hold a non-empty bearer token after trimming"
    );

    // (a) whoami with ZERO token configuration — retried until the API answers.
    let whoami = wait_for_cli_ok(
        &home,
        &["auth", "whoami", "--server", &url],
        &mut guard,
        &stdout_log,
        &stderr_log,
    );
    let whoami_stderr = String::from_utf8_lossy(&whoami.stderr).into_owned();
    assert_eq!(
        count_occurrences(&whoami_stderr, DEV_NOTE),
        1,
        "whoami using the fallback must print the note EXACTLY ONCE on stderr, got: \
         {whoami_stderr:?}"
    );
    let whoami_stdout = String::from_utf8_lossy(&whoami.stdout).into_owned();
    assert!(
        !whoami_stdout.contains("using dev token"),
        "the fallback note must go to stderr, never stdout: {whoami_stdout:?}"
    );

    // (b) a resource command works with zero token configuration.
    let list = common::flowplane_cmd(&home)
        .args(["cluster", "list", "--team", "default", "--server", &url])
        .output()
        .expect("run flowplane cluster list");
    assert!(
        list.status.success(),
        "`cluster list --team default` must exit 0 on the fallback token, got {:?}; CLI \
         stderr: {:?};\n{}",
        list.status.code(),
        String::from_utf8_lossy(&list.stderr),
        server_tail(&stdout_log, &stderr_log)
    );

    // (c) `auth token` prints EXACTLY the file token + one trailing newline on stdout.
    let tok = common::flowplane_cmd(&home)
        .args(["auth", "token", "--server", &url])
        .output()
        .expect("run flowplane auth token");
    assert!(
        tok.status.success(),
        "`auth token` must exit 0, got {:?}; stderr: {:?}",
        tok.status.code(),
        String::from_utf8_lossy(&tok.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&tok.stdout),
        format!("{token1}\n"),
        "`auth token` stdout must be EXACTLY the dev-token file contents (trimmed) plus one \
         trailing newline — nothing else"
    );
    let tok_stderr = String::from_utf8_lossy(&tok.stderr).into_owned();
    assert_eq!(
        count_occurrences(&tok_stderr, DEV_NOTE),
        1,
        "`auth token` using the fallback must print the note exactly once on stderr, got: \
         {tok_stderr:?}"
    );

    // ----- AC7: restart freshness -----
    drop(guard); // kill + reap server 1

    let port2 = alloc_port();
    let url2 = format!("http://127.0.0.1:{port2}");
    let stdout_log2 = work.join("serve2-stdout.log");
    let stderr_log2 = work.join("serve2-stderr.log");
    let child2 = dev_serve_cmd(
        &home,
        &db_url,
        &format!("127.0.0.1:{port2}"),
        &stdout_log2,
        &stderr_log2,
    )
    .spawn()
    .expect("spawn restarted flowplane serve");
    let mut guard2 = ChildGuard(child2);

    // The restarted server must mint a FRESH per-boot token into the same default sink.
    let token2 = wait_for_token_change(
        &token_path,
        &token1,
        &mut guard2,
        &stdout_log2,
        &stderr_log2,
    );
    assert_ne!(
        token2, token1,
        "test bug: wait_for_token_change returned the old token"
    );

    // The CLI — still with ZERO token configuration — transparently picks up the fresh token.
    let whoami2 = wait_for_cli_ok(
        &home,
        &["auth", "whoami", "--server", &url2],
        &mut guard2,
        &stdout_log2,
        &stderr_log2,
    );
    let whoami2_stderr = String::from_utf8_lossy(&whoami2.stderr).into_owned();
    assert!(
        whoami2_stderr.contains(DEV_NOTE),
        "after restart, whoami must still report the fallback note on stderr, got: \
         {whoami2_stderr:?}"
    );
    // guard2 drops here: kill + wait the child on every path.
}

// ---------------------------------------------------------------------------------------------
// AC5(a): precedence — FLOWPLANE_TOKEN (env) beats the planted dev file. The probe echoes
// `Bearer env-token`, and stderr carries NO fallback note (the note appears only when the
// fallback is actually used).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac5a_env_token_beats_dev_file_and_prints_no_note() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let dev_token = unique_token("dev-file-token");
    plant_dev_token(&home, &dev_token);

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_TOKEN", "env-token")
        .args([
            "cluster",
            "get",
            "probe",
            "--team",
            "payments",
            "--server",
            mock.base_url(),
            "-o",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(
        received_authorization(&out, "AC5a env beats dev file"),
        "Bearer env-token",
        "FLOWPLANE_TOKEN must beat the ~/.flowplane/dev-token fallback"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(DEV_NOTE),
        "the fallback note must NOT be printed when the env token wins: {stderr:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC5(b): precedence — the credentials file (`<config dir>/credentials`) beats the planted
// dev file. The probe echoes `Bearer cred-token` and stderr carries no fallback note.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac5b_credentials_file_beats_dev_file() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let dev_token = unique_token("dev-file-token");
    plant_dev_token(&home, &dev_token);
    write_credentials(&home, "cred-token");

    let out = common::flowplane_cmd(&home)
        .args([
            "cluster",
            "get",
            "probe",
            "--team",
            "payments",
            "--server",
            mock.base_url(),
            "-o",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(
        received_authorization(&out, "AC5b credentials beat dev file"),
        "Bearer cred-token",
        "the credentials file must beat the ~/.flowplane/dev-token fallback"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(DEV_NOTE),
        "the fallback note must NOT be printed when the credentials token wins: {stderr:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC5(c): with NO other token source, the dev file IS the effective token on a loopback
// server — the probe echoes `Bearer dev-file-token-<unique>` (proving the file was read and
// TRIMMED: it was planted with a trailing newline) and stderr carries the note EXACTLY once.
// `parse_probe` also proves stdout is the JSON envelope and nothing else (note not on stdout).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac5c_dev_file_is_lowest_rung_with_exactly_one_note() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let dev_token = unique_token("dev-file-token");
    plant_dev_token(&home, &dev_token);

    let out = common::flowplane_cmd(&home)
        .args([
            "cluster",
            "get",
            "probe",
            "--team",
            "payments",
            "--server",
            mock.base_url(),
            "-o",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(
        received_authorization(&out, "AC5c dev file only"),
        format!("Bearer {dev_token}"),
        "with no other source, the trimmed ~/.flowplane/dev-token contents must be the bearer"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        count_occurrences(&stderr, DEV_NOTE),
        1,
        "the fallback note must appear EXACTLY once on stderr when the dev file is used: \
         {stderr:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC6 (loopback gate, no network): with a planted dev file and a NON-loopback server
// (`http://cp.example.invalid:9`, unresolvable TLD — nothing is ever reached):
//   * `auth token` exits 0 and prints an EMPTY line to stdout — the dev file must not be
//     read into the effective config — and stderr carries no fallback note;
//   * `auth whoami` FAILS (non-zero) with a transport-style error, again with no note,
//     and the dev token value never appears in its output.
// ---------------------------------------------------------------------------------------------
#[test]
fn ac6_non_loopback_host_never_reads_dev_file() {
    let home = common::unique_tempdir();
    let dev_token = unique_token("dev-file-token");
    plant_dev_token(&home, &dev_token);
    let server = "http://cp.example.invalid:9";

    // `auth token`: no token resolved ⇒ exactly one empty line on stdout.
    let tok = common::flowplane_cmd(&home)
        .args(["auth", "token", "--server", server])
        .output()
        .unwrap();
    assert!(
        tok.status.success(),
        "`auth token` must exit 0 even with no resolvable token, got {:?}; stderr: {:?}",
        tok.status.code(),
        String::from_utf8_lossy(&tok.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&tok.stdout),
        "\n",
        "`auth token` against a NON-loopback host must print an EMPTY line — the dev file \
         must not be read into the effective config"
    );
    let tok_stderr = String::from_utf8_lossy(&tok.stderr).into_owned();
    assert!(
        !tok_stderr.contains(DEV_NOTE),
        "no fallback note may be printed for a non-loopback host: {tok_stderr:?}"
    );
    assert!(
        !tok_stderr.contains(&dev_token),
        "the dev token value must never leak to stderr: {tok_stderr:?}"
    );

    // `auth whoami`: no token AND an unreachable host ⇒ non-zero transport-style failure.
    let who = common::flowplane_cmd(&home)
        .args(["auth", "whoami", "--server", server])
        .output()
        .unwrap();
    assert!(
        !who.status.success(),
        "`auth whoami` against an unresolvable non-loopback host must fail; stdout: {:?}",
        String::from_utf8_lossy(&who.stdout),
    );
    let who_stderr = String::from_utf8_lossy(&who.stderr).into_owned();
    assert!(
        !who_stderr.trim().is_empty(),
        "the whoami failure must report a transport-style error on stderr"
    );
    assert!(
        !who_stderr.contains(DEV_NOTE),
        "no fallback note may be printed for a non-loopback host: {who_stderr:?}"
    );
    assert!(
        !who_stderr.contains(&dev_token)
            && !String::from_utf8_lossy(&who.stdout).contains(&dev_token),
        "the dev token value must never appear in whoami output for a non-loopback host"
    );
}

/// A minimal always-401 loopback server (the shared mock has no 401 route for
/// `auth whoami`). Returns its base URL and the serve task's handle (abort when done).
async fn start_always_401_mock() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().fallback(|| async {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "code": "unauthorized",
                "message": "missing or invalid token"
            })),
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 401 mock to an ephemeral port");
    let base_url = format!("http://{}", listener.local_addr().expect("401 mock addr"));
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base_url, handle)
}

// ---------------------------------------------------------------------------------------------
// AC12 (stale-token hint): a loopback mock that answers 401 to everything, with ONLY the
// planted dev file as a token source. `auth whoami` must exit non-zero and its stderr must
// carry BOTH the fallback note AND — beyond the note itself — a stale-token hint naming
// `~/.flowplane/dev-token` with the word "stale".
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac12_401_with_fallback_token_adds_stale_dev_token_hint() {
    let (base_url, handle) = start_always_401_mock().await;

    let home = common::unique_tempdir();
    let dev_token = unique_token("dev-file-token");
    plant_dev_token(&home, &dev_token);

    let out = common::flowplane_cmd(&home)
        .args(["auth", "whoami", "--server", &base_url])
        .output()
        .unwrap();
    handle.abort();

    assert!(
        !out.status.success(),
        "`auth whoami` must exit non-zero on a 401; stdout: {:?}",
        String::from_utf8_lossy(&out.stdout),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains(DEV_NOTE),
        "the fallback note must be printed (the dev file WAS the token source): {stderr:?}"
    );
    // The hint is ADDITIONAL to the note: strip the (single) note, then the remainder must
    // still name the dev-token file and say "stale".
    let without_note = stderr.replacen(DEV_NOTE, "", 1);
    assert!(
        without_note.to_lowercase().contains("stale"),
        "a 401 with the fallback token must add a STALE-token hint on stderr: {stderr:?}"
    );
    assert!(
        without_note.contains("~/.flowplane/dev-token"),
        "the stale-token hint must name ~/.flowplane/dev-token (beyond the note itself): \
         {stderr:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC14 (shadowed-credential hint): a 401 where the rejected token came from a PERSISTENT
// store (here: the credentials file) while the dev fallback was actually available
// (non-empty $HOME/.flowplane/dev-token AND a loopback server) must add a stderr hint that
// a stored credential may be SHADOWING the local dev token — mentioning
// `~/.flowplane/credentials` and the word "shadowing". It must NOT print the fallback note
// (the dev file was not used) and must NOT print the AC-12 "may be stale" dev-file hint.
// Negative case: the --token/FLOWPLANE_TOKEN tier must NEVER trigger the shadow hint —
// same 401 + planted dev file, but the rejected token comes from FLOWPLANE_TOKEN.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac14_401_from_stale_stored_credential_hints_at_shadowed_dev_token() {
    let (base_url, handle) = start_always_401_mock().await;

    // Positive case: credentials file (persistent store) is the rejected source, and the
    // dev fallback was available (non-empty dev file + loopback server).
    {
        let home = common::unique_tempdir();
        let dev_token = unique_token("dev-file-token");
        plant_dev_token(&home, &dev_token);
        write_credentials(&home, "stale-cred-token");

        let out = common::flowplane_cmd(&home)
            .args(["auth", "whoami", "--server", &base_url])
            .output()
            .unwrap();

        assert!(
            !out.status.success(),
            "`auth whoami` must exit non-zero on a 401; stdout: {:?}",
            String::from_utf8_lossy(&out.stdout),
        );
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            stderr.contains("shadowing"),
            "a 401 on a stored credential with a usable dev fallback must warn that the \
             stored credential may be SHADOWING the dev token: {stderr:?}"
        );
        assert!(
            stderr.contains("~/.flowplane/credentials"),
            "the shadow hint must name ~/.flowplane/credentials: {stderr:?}"
        );
        assert!(
            !stderr.contains("using dev token from"),
            "the fallback note must NOT be printed — the dev file was NOT the token source: \
             {stderr:?}"
        );
        assert!(
            !stderr.contains("may be stale"),
            "the AC-12 stale dev-file hint must NOT be printed — the dev file was not the \
             rejected token: {stderr:?}"
        );
    }

    // Negative case: same 401 + planted dev file, but the rejected token comes from the
    // FLOWPLANE_TOKEN tier — that tier must NEVER trigger the shadow hint.
    {
        let home = common::unique_tempdir();
        let dev_token = unique_token("dev-file-token");
        plant_dev_token(&home, &dev_token);

        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_TOKEN", "stale-env-token")
            .args(["auth", "whoami", "--server", &base_url])
            .output()
            .unwrap();

        assert!(
            !out.status.success(),
            "`auth whoami` must exit non-zero on a 401; stdout: {:?}",
            String::from_utf8_lossy(&out.stdout),
        );
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stderr.contains("shadowing"),
            "an explicitly provided FLOWPLANE_TOKEN must NEVER trigger the shadow hint: \
             {stderr:?}"
        );
        assert!(
            !stderr.contains("using dev token from"),
            "the fallback note must NOT be printed — the env token was the source: {stderr:?}"
        );
    }

    handle.abort();
}
