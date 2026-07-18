//! fpv2-03m.2 — `flowplane dashboard` security conformance (black-box, adversarial).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * The command binds 127.0.0.1 on an EPHEMERAL port and prints exactly one stdout line:
//!     `Dashboard running at http://127.0.0.1:<port>/<nonce>/ (Ctrl-C to stop)` where
//!     `<nonce>` is 32 lowercase hex chars (128-bit, per-launch).
//!   * The nonce is mandatory on EVERY route class (page + embedded assets): missing or
//!     wrong nonce → 404; the real nonce → 200 with real content.
//!   * Foreign `Host` / foreign `Origin` → 403; the server's own origin → 200.
//!   * Non-GET methods on the page route → 405.
//!   * Security headers (`Cache-Control: no-store`, `Content-Security-Policy: default-src
//!     'self'`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`) on EVERY response,
//!     including 404s/405s/403s.
//!   * Two launches produce different nonces.
//!   * No resolvable team → non-zero exit, stderr says "team is required", no serving.
//!   * No off-loopback bind flag exists (`--listen` is a clap usage error).
//!   * The bearer token never appears in any response body/header or the stdout line.
//!
//! Parallel-safety (invariant 18): every test spawns its own child on an ephemeral port with
//! an isolated `HOME` temp dir; nothing binds a fixed port, so the suite runs green under
//! default nextest parallelism. Every spawned server is killed via a Drop guard in all
//! paths, including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A distinctive bearer token used across tests so any leak into responses/stdout is
/// unambiguously attributable (criterion 8).
const SECRET_TOKEN: &str = "sekret-token-do-not-leak-9f2c";

/// Kill-on-drop guard so the dashboard child never outlives a test, even on panic.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A running `flowplane dashboard` child plus the port/nonce parsed from its stdout line.
struct Dashboard {
    _guard: ChildGuard,
    port: u16,
    nonce: String,
    first_line: String,
}

impl Dashboard {
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The real-nonce URL for `path` (`""` for the page root, or `"assets/..."`).
    fn nonce_url(&self, path: &str) -> String {
        format!("{}/{}/{}", self.base(), self.nonce, path)
    }
}

/// Spawn `flowplane dashboard` with an isolated HOME and the standard env, read the single
/// stdout announcement line (30s timeout), parse out port + nonce, and validate the line's
/// exact documented shape.
fn spawn_dashboard(token: &str) -> Dashboard {
    let home = common::unique_tempdir();
    let mut cmd = common::flowplane_cmd(&home);
    cmd.env("FLOWPLANE_SERVER", "http://127.0.0.1:9") // dead port; this slice makes no upstream calls
        .env("FLOWPLANE_TOKEN", token)
        .env("FLOWPLANE_TEAM", "payments")
        .env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1")
        .arg("dashboard")
        .stdout(Stdio::piped())
        // stderr → null: the server outlives this test's reads, and an unread full pipe
        // could block the child. We assert on stderr only in the exiting-process tests.
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("spawn flowplane dashboard");
    let stdout = child.stdout.take().expect("child stdout piped");
    let guard = ChildGuard(child);

    // Read the first stdout line on a std thread with a hard timeout, so a silent child
    // fails the test instead of hanging the suite.
    let (tx, rx) = mpsc::channel::<std::io::Result<String>>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let res = reader.read_line(&mut line).map(|_| line);
        let _ = tx.send(res);
    });
    let first_line = match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(line)) => line.trim_end_matches(['\r', '\n']).to_string(),
        Ok(Err(e)) => panic!("failed reading dashboard stdout: {e}"),
        Err(_) => panic!("dashboard did not print its announcement line within 30s"),
    };

    // Exact documented shape: `Dashboard running at http://127.0.0.1:<port>/<nonce>/ (Ctrl-C to stop)`
    let prefix = "Dashboard running at http://127.0.0.1:";
    let suffix = " (Ctrl-C to stop)";
    let rest = first_line
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("stdout line must start with {prefix:?}, got: {first_line:?}"));
    let rest = rest
        .strip_suffix(suffix)
        .unwrap_or_else(|| panic!("stdout line must end with {suffix:?}, got: {first_line:?}"));
    // rest is now `<port>/<nonce>/`
    let mut parts = rest.split('/');
    let port: u16 = parts
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("cannot parse port from stdout line: {first_line:?}"));
    let nonce = parts
        .next()
        .unwrap_or_else(|| panic!("cannot parse nonce from stdout line: {first_line:?}"))
        .to_string();
    assert_eq!(
        parts.next(),
        Some(""),
        "URL must end with a trailing slash after the nonce: {first_line:?}"
    );
    assert_eq!(
        parts.next(),
        None,
        "unexpected extra path segments: {first_line:?}"
    );
    assert_eq!(
        nonce.len(),
        32,
        "nonce must be 32 hex chars (128-bit), got {:?} ({} chars)",
        nonce,
        nonce.len()
    );
    assert!(
        nonce
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "nonce must be lowercase hex, got: {nonce:?}"
    );

    Dashboard {
        _guard: guard,
        port,
        nonce,
        first_line,
    }
}

/// A 32-lowercase-hex nonce guaranteed different from `real` (every char is flipped).
fn wrong_nonce(real: &str) -> String {
    real.chars()
        .map(|c| if c == 'a' { 'b' } else { 'a' })
        .collect()
}

/// An HTTP client that follows no redirects — we assert the raw status of every URL.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client")
}

/// Criterion 4: the four security headers, with their exact documented values, on `resp`.
fn assert_security_headers(resp: &reqwest::Response, ctx: &str) {
    let expect = [
        ("cache-control", "no-store"),
        ("content-security-policy", "default-src 'self'"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
    ];
    for (name, want) in expect {
        let got = resp
            .headers()
            .get(name)
            .unwrap_or_else(|| {
                panic!(
                    "{ctx}: missing security header {name:?} (status {})",
                    resp.status()
                )
            })
            .to_str()
            .unwrap_or_else(|_| panic!("{ctx}: header {name:?} is not valid UTF-8"));
        assert_eq!(
            got, want,
            "{ctx}: header {name:?} must be exactly {want:?}, got {got:?}"
        );
    }
}

/// Send one raw HTTP/1.1 request over a fresh TCP connection so we fully control the
/// `Host` header (an HTTP client library may normalize it). Returns (status, raw head).
async fn raw_request_status(port: u16, path: &str, host: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to dashboard");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write raw request");
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .expect("read raw response");
    let text = String::from_utf8_lossy(&buf).to_string();
    let status_line = text
        .lines()
        .next()
        .unwrap_or_else(|| panic!("empty raw response"));
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("cannot parse status from {status_line:?}"));
    let head = text.split("\r\n\r\n").next().unwrap_or(&text).to_string();
    (status, head)
}

// ---------------------------------------------------------------------------------------------
// Criterion 1: the nonce is mandatory on EVERY route class — page and both embedded assets.
// Missing prefix → 404, wrong 32-hex nonce → 404, real nonce → 200 with real content.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nonce_is_mandatory_on_every_route_class() {
    let dash = spawn_dashboard(SECRET_TOKEN);
    let http = client();
    let bad = wrong_nonce(&dash.nonce);

    // (route-class suffix, is_page)
    let routes: &[(&str, bool)] = &[
        ("", true),
        ("assets/htmx.min.js", false),
        ("assets/dashboard.css", false),
    ];

    for (path, is_page) in routes {
        // Without any nonce prefix → 404.
        let no_nonce = format!("{}/{}", dash.base(), path);
        let resp = http.get(&no_nonce).send().await.expect("GET without nonce");
        assert_eq!(
            resp.status().as_u16(),
            404,
            "GET {no_nonce} (no nonce) must be 404"
        );

        // With a wrong (but well-formed 32-hex) nonce → 404, never a redirect or 200.
        let wrong = format!("{}/{}/{}", dash.base(), bad, path);
        let resp = http.get(&wrong).send().await.expect("GET wrong nonce");
        assert_eq!(
            resp.status().as_u16(),
            404,
            "GET {wrong} (wrong nonce) must be 404"
        );

        // With the real nonce → 200 and non-empty content.
        let real = dash.nonce_url(path);
        let resp = http.get(&real).send().await.expect("GET real nonce");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "GET {real} (real nonce) must be 200"
        );
        let body = resp.text().await.expect("read body");
        assert!(!body.is_empty(), "GET {real}: body must be non-empty");
        if *is_page {
            assert!(
                body.contains("Flowplane"),
                "page body must be HTML containing \"Flowplane\": got {} bytes",
                body.len()
            );
            assert!(
                body.contains('<'),
                "page body must look like HTML markup, got: {:.80}...",
                body
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 2: DNS-rebinding / cross-site defenses. A foreign Host header on the real nonce
// URL → 403 (raw TCP, so no client normalizes the header). A foreign Origin → 403. The
// server's own origin → 200.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_host_header_is_rejected() {
    let dash = spawn_dashboard(SECRET_TOKEN);
    let page_path = format!("/{}/", dash.nonce);

    // Sanity baseline over the same raw transport: the legitimate Host → 200.
    let own_host = format!("127.0.0.1:{}", dash.port);
    let (status, _) = raw_request_status(dash.port, &page_path, &own_host).await;
    assert_eq!(status, 200, "own Host header {own_host:?} must be accepted");

    // Foreign Host (DNS rebinding) → 403 even with the correct nonce.
    let (status, head) = raw_request_status(dash.port, &page_path, "evil.example").await;
    assert_eq!(
        status, 403,
        "foreign Host: evil.example on the real nonce URL must be 403; head: {head}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_origin_is_rejected_own_origin_is_accepted() {
    let dash = spawn_dashboard(SECRET_TOKEN);
    let http = client();
    let url = dash.nonce_url("");

    // Foreign Origin → 403 even with the correct nonce.
    let resp = http
        .get(&url)
        .header("Origin", "http://evil.example")
        .send()
        .await
        .expect("GET with foreign Origin");
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Origin: http://evil.example must be rejected with 403"
    );

    // The server's own origin → 200.
    let resp = http
        .get(&url)
        .header("Origin", dash.base())
        .send()
        .await
        .expect("GET with own Origin");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Origin matching the server's own origin ({}) must be accepted",
        dash.base()
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 3: the dashboard is read-only — non-GET methods on the real-nonce page route → 405.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_get_methods_are_405() {
    let dash = spawn_dashboard(SECRET_TOKEN);
    let http = client();
    let url = dash.nonce_url("");

    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let resp = http
            .request(m, &url)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{method} {url}: {e}"));
        assert_eq!(
            resp.status().as_u16(),
            405,
            "{method} on the real-nonce page route must be 405"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 4: the security headers ride on EVERY response — 200s, 404s (both no-nonce and
// wrong-nonce), 405s, and Origin-rejection 403s alike.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn security_headers_on_every_response_including_errors() {
    let dash = spawn_dashboard(SECRET_TOKEN);
    let http = client();

    // 200: page and both assets with the real nonce.
    for path in ["", "assets/htmx.min.js", "assets/dashboard.css"] {
        let url = dash.nonce_url(path);
        let resp = http.get(&url).send().await.expect("GET real nonce");
        assert_eq!(resp.status().as_u16(), 200, "sanity: {url}");
        assert_security_headers(&resp, &format!("200 {url}"));
    }

    // 404: no nonce prefix.
    let url = format!("{}/", dash.base());
    let resp = http.get(&url).send().await.expect("GET no nonce");
    assert_eq!(resp.status().as_u16(), 404, "sanity: {url}");
    assert_security_headers(&resp, &format!("404 (no nonce) {url}"));

    // 404: wrong nonce on an asset route.
    let url = format!(
        "{}/{}/assets/dashboard.css",
        dash.base(),
        wrong_nonce(&dash.nonce)
    );
    let resp = http.get(&url).send().await.expect("GET wrong nonce asset");
    assert_eq!(resp.status().as_u16(), 404, "sanity: {url}");
    assert_security_headers(&resp, &format!("404 (wrong nonce) {url}"));

    // 405: POST on the page route.
    let url = dash.nonce_url("");
    let resp = http.post(&url).send().await.expect("POST page");
    assert_eq!(resp.status().as_u16(), 405, "sanity: POST {url}");
    assert_security_headers(&resp, &format!("405 POST {url}"));

    // 403: foreign Origin rejection.
    let resp = http
        .get(&url)
        .header("Origin", "http://evil.example")
        .send()
        .await
        .expect("GET foreign origin");
    assert_eq!(resp.status().as_u16(), 403, "sanity: foreign-Origin {url}");
    assert_security_headers(&resp, &format!("403 (foreign Origin) {url}"));
}

// ---------------------------------------------------------------------------------------------
// Criterion 5: the nonce is per-launch — two separate launches must not share one.
// (Format — 32 lowercase hex — is asserted inside spawn_dashboard for both.)
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_launches_produce_different_nonces() {
    let first = spawn_dashboard(SECRET_TOKEN);
    let second = spawn_dashboard(SECRET_TOKEN);
    assert_ne!(
        first.nonce, second.nonce,
        "the nonce must be freshly generated per launch, not a fixed value"
    );
}

// ---------------------------------------------------------------------------------------------
// Criteria 6 & 7 helper: run a `flowplane` invocation that is EXPECTED to exit on its own,
// with a hard timeout — a buggy build that starts serving instead of exiting must fail the
// test, not hang the suite.
// ---------------------------------------------------------------------------------------------
fn wait_for_exit(mut cmd: Command, ctx: &str) -> std::process::Output {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("{ctx}: spawn: {e}"));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .unwrap_or_else(|e| panic!("{ctx}: collect output: {e}"));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    // Salvage stdout for the failure message (did it start serving?).
                    let mut out = String::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        let _ = stdout.read_to_string(&mut out);
                    }
                    let _ = child.wait();
                    panic!(
                        "{ctx}: process was expected to exit but was still running after 30s; \
                         stdout so far: {out:?}"
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("{ctx}: try_wait: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 6: with no resolvable team (no FLOWPLANE_TEAM, no config file), the command must
// refuse to start: non-zero exit, stderr contains "team is required", and stdout never
// prints the "Dashboard running" announcement.
// ---------------------------------------------------------------------------------------------
#[test]
fn missing_team_refuses_to_start() {
    let home = common::unique_tempdir();
    let mut cmd = common::flowplane_cmd(&home);
    cmd.env("FLOWPLANE_SERVER", "http://127.0.0.1:9")
        .env("FLOWPLANE_TOKEN", SECRET_TOKEN)
        .env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1")
        // Deliberately NO FLOWPLANE_TEAM, and FLOWPLANE_CONFIG points at a nonexistent file.
        .arg("dashboard");

    let out = wait_for_exit(cmd, "dashboard without team");
    assert!(
        !out.status.success(),
        "dashboard with no resolvable team must exit non-zero, got: {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("team is required"),
        "stderr must say \"team is required\", got: {stderr:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("Dashboard running"),
        "no server may be announced when the team preflight fails, stdout: {stdout:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The configured team value is interpolated into the two allowlisted upstream paths as one
// segment; a hostile value must be rejected BEFORE the server binds, so it can never widen
// the fixed upstream allowlist (design-review finding, fpv2-03m.3 reconcile).
// ---------------------------------------------------------------------------------------------
#[test]
fn hostile_team_value_is_rejected_before_bind() {
    for hostile in ["../admin", "a/b", "a?x=1", "a#frag", "a%2Fb"] {
        let home = common::unique_tempdir();
        let mut cmd = common::flowplane_cmd(&home);
        cmd.env("FLOWPLANE_SERVER", "http://127.0.0.1:9")
            .env("FLOWPLANE_TOKEN", SECRET_TOKEN)
            .env("FLOWPLANE_TEAM", hostile)
            .env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1")
            .arg("dashboard");
        let out = wait_for_exit(cmd, "dashboard with hostile team");
        assert!(
            !out.status.success(),
            "hostile team {hostile:?} must exit non-zero, got: {:?}",
            out.status
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("invalid team name"),
            "stderr must name the invalid team for {hostile:?}, got: {stderr:?}"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("Dashboard running"),
            "no server may be announced for hostile team {hostile:?}, stdout: {stdout:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 7 (superseded by ui-f3, fpv2-m4u.1): F1 shipped no off-loopback bind flag at
// all; the approved F3 design transfers `--listen` ownership to the container profile.
// The DEFAULT posture is what this suite still owns: with no flags the bind stays
// loopback-ephemeral. The off-loopback replacement guarantees (mandatory nonce, loopback
// URL derivation, stderr warning) live in cli_dashboard_container_profile.rs.
// ---------------------------------------------------------------------------------------------
#[test]
fn default_profile_never_binds_off_loopback() {
    // A garbage --listen value must still be a usage error (typed SocketAddr parse),
    // and must never announce a server.
    let home = common::unique_tempdir();
    let mut cmd = common::flowplane_cmd(&home);
    cmd.env("FLOWPLANE_SERVER", "http://127.0.0.1:9")
        .env("FLOWPLANE_TOKEN", SECRET_TOKEN)
        .env("FLOWPLANE_TEAM", "payments")
        .env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1")
        .args(["dashboard", "--listen", "not-an-address"]);

    let out = wait_for_exit(cmd, "dashboard --listen not-an-address");
    assert!(
        !out.status.success(),
        "a malformed --listen value must be rejected, got: {:?}",
        out.status
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("Dashboard running"),
        "the server must never start on a malformed --listen, stdout: {stdout:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 8: the bearer token must never leak — not in any response body, not in any
// response header (name or value), across the page and both assets fetched with the real
// nonce, and not in the stdout announcement line.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bearer_token_never_leaks_into_responses_or_stdout() {
    let dash = spawn_dashboard(SECRET_TOKEN);
    let http = client();

    assert!(
        !dash.first_line.contains(SECRET_TOKEN),
        "the stdout announcement line must not contain the bearer token: {:?}",
        dash.first_line
    );

    for path in ["", "assets/htmx.min.js", "assets/dashboard.css"] {
        let url = dash.nonce_url(path);
        let resp = http.get(&url).send().await.expect("GET real nonce");
        assert_eq!(resp.status().as_u16(), 200, "sanity: {url}");

        for (name, value) in resp.headers() {
            let value_str = String::from_utf8_lossy(value.as_bytes());
            assert!(
                !name.as_str().contains(SECRET_TOKEN) && !value_str.contains(SECRET_TOKEN),
                "{url}: response header {name:?} leaks the bearer token: {value_str:?}"
            );
        }

        let body = resp.bytes().await.expect("read body");
        let haystack = String::from_utf8_lossy(&body);
        assert!(
            !haystack.contains(SECRET_TOKEN),
            "{url}: response body leaks the bearer token"
        );
    }
}
