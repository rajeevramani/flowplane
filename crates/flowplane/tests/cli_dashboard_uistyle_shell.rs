//! fpv2-80z.1 — `flowplane dashboard` overview shell restyle — black-box, spec-driven
//! contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix.
//! Contract under test, for the served shell page `GET /<nonce>/`:
//!
//!   1. HTTP 200 HTML whose nav is `<nav class="tabs">` naming all 7 tabs (Overview,
//!      Resources, APIs, Learning, AI, MCP, Operations) as anchors, with the Overview
//!      anchor carrying the `active` class.
//!   2. The topbar (`<header class="topbar">`) renders a `.brand` element and a `.ctx`
//!      chip row containing a chip with the team name and a `read-only` chip.
//!   3. No inline `<style>` element, no inline `<script>` element, and no inline `style=`
//!      attribute anywhere in the served shell page. Same-origin `<script src=...>` tags
//!      (htmx) ARE allowed and expected.
//!   4. The CSP response header is still `default-src 'self'`, and the htmx wiring is
//!      preserved: the main element carries `hx-get="/<nonce>/partials/overview"`,
//!      `hx-trigger="load, every 10s"`, `hx-swap="innerHTML"`.
//!   5. The stylesheet is served at `/<nonce>/assets/dashboard.css` (200, text/css) and
//!      contains the prototype token `--surface-2`.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique
//! team name; nothing binds a fixed port. Every spawned server is killed via a Drop guard
//! in all paths, including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-shell-token-do-not-leak-7b2d";

/// The 7 tabs the restyled nav must name, in order.
const TABS: [&str; 7] = [
    "Overview",
    "Resources",
    "APIs",
    "Learning",
    "AI",
    "MCP",
    "Operations",
];

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0. The shell page performs no
// upstream fetch, so the stub only needs to exist (the dashboard requires a server URL);
// unknown paths answer 404.
// =============================================================================================

struct StubUpstream {
    base_url: String,
    handle: JoinHandle<()>,
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn stub_handler(_req: Request) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "code": "not_found", "message": "no such route" })),
    )
        .into_response()
}

async fn start_stub() -> StubUpstream {
    let app = Router::new().fallback(stub_handler);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub upstream to an ephemeral port");
    let addr = listener.local_addr().expect("stub local addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    StubUpstream {
        base_url: format!("http://{addr}"),
        handle,
    }
}

// =============================================================================================
// Dashboard child process: spawn, parse the announcement line, kill on drop.
// =============================================================================================

/// Kill-on-drop guard so the dashboard child never outlives a test, even on panic.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Dashboard {
    _guard: ChildGuard,
    port: u16,
    nonce: String,
}

impl Dashboard {
    /// `http://127.0.0.1:<port>/<nonce>/<path>` (`path` may be empty for the shell page).
    fn nonce_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{}", self.port, self.nonce, path)
    }
}

/// Spawn `flowplane dashboard` with an isolated HOME and the standard env, read the single
/// stdout announcement line (30s timeout), and parse out port + nonce.
fn spawn_dashboard(home: PathBuf, server: &str, team: &str) -> Dashboard {
    let mut cmd = common::flowplane_cmd(&home);
    cmd.env(
        "FLOWPLANE_CONFIG",
        home.join(".flowplane").join("config.toml"),
    )
    .env("FLOWPLANE_SERVER", server)
    .env("FLOWPLANE_TEAM", team)
    .env("FLOWPLANE_TOKEN", SECRET_TOKEN)
    .env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1")
    .arg("dashboard")
    .stdout(Stdio::piped())
    // stderr → null: the server outlives this test's reads and an unread full pipe could
    // block the child.
    .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("spawn flowplane dashboard");
    let stdout = child.stdout.take().expect("child stdout piped");
    let guard = ChildGuard(child);

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

    // `Dashboard running at http://127.0.0.1:<port>/<nonce>/ (Ctrl-C to stop)`
    let prefix = "Dashboard running at http://127.0.0.1:";
    let suffix = " (Ctrl-C to stop)";
    let rest = first_line
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("stdout line must start with {prefix:?}, got: {first_line:?}"));
    let rest = rest
        .strip_suffix(suffix)
        .unwrap_or_else(|| panic!("stdout line must end with {suffix:?}, got: {first_line:?}"));
    let mut parts = rest.split('/');
    let port: u16 = parts
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("cannot parse port from stdout line: {first_line:?}"));
    let nonce = parts
        .next()
        .unwrap_or_else(|| panic!("cannot parse nonce from stdout line: {first_line:?}"))
        .to_string();
    assert_eq!(nonce.len(), 32, "nonce must be 32 hex chars: {nonce:?}");

    Dashboard {
        _guard: guard,
        port,
        nonce,
    }
}

/// Fetch a dashboard URL with a startup tolerance: retry on transport errors and 5xx until a
/// non-5xx response arrives or 15s elapse. Terminal statuses (200/286/4xx) return.
async fn fetch(http: &reqwest::Client, url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match http.get(url).send().await {
            Ok(resp) if !resp.status().is_server_error() => return resp,
            other => {
                if Instant::now() >= deadline {
                    match other {
                        Ok(resp) => panic!(
                            "GET {url}: still 5xx ({}) after 15s",
                            resp.status().as_u16()
                        ),
                        Err(e) => panic!("GET {url}: still unreachable after 15s: {e}"),
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client")
}

// =============================================================================================
// Markup assertion helpers (spec-level, tolerant of whitespace / extra classes).
// =============================================================================================

/// The opening tag (e.g. `<nav class="tabs">`) starting at the first occurrence of
/// `tag_start` (e.g. `<nav`), up to and including its closing `>`.
fn opening_tag<'a>(html: &'a str, tag_start: &str) -> Option<&'a str> {
    let start = html.find(tag_start)?;
    let end = html[start..].find('>')? + start;
    Some(&html[start..=end])
}

/// All opening tags matching `tag_start`, in document order.
fn all_opening_tags<'a>(html: &'a str, tag_start: &str) -> Vec<&'a str> {
    let mut tags = Vec::new();
    let mut rest = html;
    while let Some(tag) = opening_tag(rest, tag_start) {
        tags.push(tag);
        let consumed = tag.as_ptr() as usize - rest.as_ptr() as usize + tag.len();
        rest = &rest[consumed..];
    }
    tags
}

/// Does `tag` carry `class` as a whole class token (tolerant of multi-class attributes)?
fn tag_has_class(tag: &str, class: &str) -> bool {
    for needle in [
        format!("class=\"{class}\""),
        format!("class=\"{class} "),
        format!(" {class}\""),
        format!(" {class} "),
    ] {
        if tag.contains(&needle) {
            return true;
        }
    }
    false
}

/// Does `html` contain any element carrying `class` as a whole class token?
fn any_element_has_class(html: &str, class: &str) -> bool {
    tag_has_class(html, class)
}

/// The segment of `html` from the start of the first `<open ...>` tag through the matching
/// `</close>` (inclusive), for scoping assertions to one region (nav, header).
fn region<'a>(html: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = html.find(open)?;
    let end = html[start..].find(close)? + start + close.len();
    Some(&html[start..end])
}

/// GET the shell page and assert the 200 HTML baseline shared by every shell test.
async fn fetch_shell(http: &reqwest::Client, dash: &Dashboard) -> String {
    let resp = fetch(http, &dash.nonce_url("")).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/ must serve the shell page"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        shell.contains('<'),
        "the shell must be HTML; body:\n{shell}"
    );
    assert!(
        !shell.contains(SECRET_TOKEN),
        "the shell must never leak the bearer token; body:\n{shell}"
    );
    shell
}

// =============================================================================================
// Criterion 1: GET /<nonce>/ serves 200 HTML whose nav is `<nav class="tabs">` naming all
// 7 tabs as anchors, with the Overview anchor carrying the active class.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_serves_tabs_nav_naming_all_seven_tabs_with_active_overview() {
    let stub = start_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let shell = fetch_shell(&http, &dash).await;

    // The nav element: <nav> carrying the `tabs` class.
    let nav_tag = opening_tag(&shell, "<nav")
        .unwrap_or_else(|| panic!("the shell must render a <nav> element; body:\n{shell}"));
    assert!(
        tag_has_class(nav_tag, "tabs"),
        "the nav must be `<nav class=\"tabs\">`; got tag: {nav_tag:?}"
    );

    let nav = region(&shell, "<nav", "</nav>")
        .unwrap_or_else(|| panic!("the <nav> must be closed; body:\n{shell}"));

    // All 7 tabs are named as anchors, in order.
    let anchors = all_opening_tags(nav, "<a");
    assert!(
        anchors.len() >= 7,
        "the nav must link all 7 tabs as anchors; found {} anchors: {anchors:?}",
        anchors.len()
    );
    let mut cursor = 0usize;
    for tab in TABS {
        let at = nav[cursor..]
            .find(tab)
            .unwrap_or_else(|| panic!("the nav must name the {tab:?} tab (in order); nav:\n{nav}"));
        // The tab name must sit inside an anchor (between an `<a` and its `</a>`), not
        // merely appear as stray text.
        let abs = cursor + at;
        let a_open = nav[..abs]
            .rfind("<a")
            .unwrap_or_else(|| panic!("tab {tab:?} must be an anchor; nav:\n{nav}"));
        let a_close_before = nav[..abs].rfind("</a>");
        assert!(
            a_close_before.is_none_or(|c| c < a_open),
            "tab {tab:?} must be inside an <a> anchor; nav:\n{nav}"
        );
        cursor = abs + tab.len();
    }

    // The Overview anchor carries the active class.
    let overview_at = nav
        .find("Overview")
        .expect("nav names Overview (asserted above)");
    let a_start = nav[..overview_at]
        .rfind("<a")
        .expect("Overview anchor (asserted above)");
    let overview_tag = opening_tag(&nav[a_start..], "<a").expect("Overview anchor tag");
    assert!(
        tag_has_class(overview_tag, "active"),
        "the Overview anchor must carry `class=\"active\"`; got tag: {overview_tag:?}"
    );
}

// =============================================================================================
// Criterion 2: the topbar (`<header class="topbar">`) renders a `.brand` element and a
// `.ctx` chip row containing a chip with the team name and a `read-only` chip.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_topbar_renders_brand_and_ctx_chip_row() {
    let stub = start_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let shell = fetch_shell(&http, &dash).await;

    let header_tag = opening_tag(&shell, "<header")
        .unwrap_or_else(|| panic!("the shell must render a <header> topbar; body:\n{shell}"));
    assert!(
        tag_has_class(header_tag, "topbar"),
        "the topbar must be `<header class=\"topbar\">`; got tag: {header_tag:?}"
    );

    let header = region(&shell, "<header", "</header>")
        .unwrap_or_else(|| panic!("the <header> must be closed; body:\n{shell}"));

    assert!(
        any_element_has_class(header, "brand"),
        "the topbar must render a `.brand` element; header:\n{header}"
    );
    assert!(
        any_element_has_class(header, "ctx"),
        "the topbar must render a `.ctx` chip row; header:\n{header}"
    );

    // The ctx chip row contains a chip with the team name and a `read-only` chip.
    let ctx_start = header
        .find("ctx")
        .expect("the .ctx chip row (asserted above)");
    let ctx_scope = &header[ctx_start..];
    assert!(
        ctx_scope.contains(team.as_str()),
        "the ctx chip row must contain a chip with the team name {team:?}; ctx row:\n{ctx_scope}"
    );
    assert!(
        ctx_scope.to_lowercase().contains("read-only"),
        "the ctx chip row must contain a `read-only` chip; ctx row:\n{ctx_scope}"
    );
}

// =============================================================================================
// Criterion 3: no inline <style> element, no inline <script> element, and no inline style=
// attribute anywhere in the served shell. Same-origin <script src=...> (htmx) is allowed.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_has_no_inline_style_script_or_style_attribute() {
    let stub = start_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let shell = fetch_shell(&http, &dash).await;
    let lower = shell.to_lowercase();

    assert!(
        !lower.contains("<style"),
        "the shell must not contain an inline <style> element; body:\n{shell}"
    );

    // Every <script> tag must be a same-origin external script (carry src=); inline
    // <script> elements are forbidden. htmx via <script src=...> is expected.
    let scripts = all_opening_tags(&lower, "<script");
    assert!(
        !scripts.is_empty(),
        "the shell is expected to load htmx via a same-origin <script src=...> tag; body:\n{shell}"
    );
    for tag in &scripts {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden; every script tag must carry src=; \
             offending tag: {tag:?}"
        );
    }

    assert!(
        !lower.contains(" style="),
        "the shell must not contain any inline style= attribute; body:\n{shell}"
    );
}

// =============================================================================================
// Criterion 4: the CSP header is still `default-src 'self'`, and the htmx wiring is
// preserved on the main element: hx-get="/<nonce>/partials/overview",
// hx-trigger="load, every 10s", hx-swap="innerHTML".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_preserves_csp_header_and_htmx_overview_wiring() {
    let stub = start_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let url = dash.nonce_url("");
    let resp = fetch(&http, &url).await;
    assert_eq!(resp.status().as_u16(), 200, "GET /<nonce>/ must be 200");

    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("GET {url} must carry a Content-Security-Policy header"));
    assert_eq!(
        csp, "default-src 'self'",
        "the CSP header must still be `default-src 'self'`"
    );

    let shell = resp.text().await.expect("shell body");

    let main_tag = opening_tag(&shell, "<main")
        .unwrap_or_else(|| panic!("the shell must render a <main> element; body:\n{shell}"));
    let want_hx_get = format!("hx-get=\"/{}/partials/overview\"", dash.nonce);
    assert!(
        main_tag.contains(&want_hx_get),
        "the main element must carry {want_hx_get:?}; got tag: {main_tag:?}"
    );
    assert!(
        main_tag.contains("hx-trigger=\"load, every 10s\""),
        "the main element must carry hx-trigger=\"load, every 10s\"; got tag: {main_tag:?}"
    );
    assert!(
        main_tag.contains("hx-swap=\"innerHTML\""),
        "the main element must carry hx-swap=\"innerHTML\"; got tag: {main_tag:?}"
    );
}

// =============================================================================================
// Criterion 5: the stylesheet is served at /<nonce>/assets/dashboard.css (200, text/css)
// and contains the prototype token `--surface-2`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stylesheet_is_served_as_text_css_and_contains_surface_2_token() {
    let stub = start_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let url = dash.nonce_url("assets/dashboard.css");
    let resp = fetch(&http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/assets/dashboard.css must serve the stylesheet"
    );
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("GET {url} must carry a Content-Type header"));
    assert!(
        content_type.starts_with("text/css"),
        "the stylesheet must be served as text/css; got content-type: {content_type:?}"
    );

    let css = resp.text().await.expect("stylesheet body");
    assert!(
        css.contains("--surface-2"),
        "the stylesheet must contain the prototype token `--surface-2` (proving the new \
         design system is being served); css:\n{css}"
    );
    assert!(
        !css.contains(SECRET_TOKEN),
        "the stylesheet must never leak the bearer token"
    );
}
