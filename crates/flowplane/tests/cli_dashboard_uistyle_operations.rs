//! fpv2-80z.9 — `flowplane dashboard` Operations tab UI restyle — black-box, spec-driven
//! contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix. The
//! Operations tab serves an HTML shell page `GET /<nonce>/operations` carrying one htmx-lazy
//! panel and a NACK partial `GET /<nonce>/partials/operations/nacks`, backed by a stub
//! upstream serving the xDS NACK read model
//! (`GET /api/v1/teams/{team}/xds/nacks?since=<rfc3339>&limit=50`).
//!
//! Acceptance criteria covered (served-HTML level), per bead fpv2-80z.9 and the authoritative
//! status→pill map:
//!   1. A quarantined NACK entry renders as `pill crit` with the quarantined resource's
//!      verbatim status text inside the pill; the assertion is scoped to the row that actually
//!      represents the quarantined entry (a non-quarantined row carries NO crit pill), proving
//!      the crit pill is specific to quarantine.
//!   2. Every `<table>` in each Operations partial is wrapped in a `.tablewrap` element (a
//!      `.tablewrap` opens before the `<table>`).
//!   3. No inline `<style>` element, no src-less `<script>` (every `<script>` carries `src=`),
//!      and no inline ` style=` attribute anywhere in any served Operations page/partial.
//!   4. The CSP response header on the shell is still exactly `default-src 'self'`.
//!
//! Parallel-safety: every test spawns its own stub upstream and dashboard child on ephemeral
//! ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique team name; nothing binds a
//! fixed port. Every spawned server is killed via a Drop guard in all paths, including
//! assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-operations-token-do-not-leak-9f3a";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// Distinctive per-field markers so row-scoped assertions never collide with unrelated markup.
const NODE_A: &str = "node-alpha-77";
const NODE_B: &str = "node-beta-88";
const TYPE_A: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
const TYPE_B: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
const VER_A: &str = "ver-rej-5551";
const VER_B: &str = "ver-rej-6662";
const ERR_A: &str = "cluster-rejected-boom";
const ERR_B: &str = "listener-rejected-bang";
/// The quarantined resource name — its verbatim text must appear inside the `pill crit`.
const QUARANTINED_A: &str = "orders-cluster-Q";

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving the xds/nacks read model.
// A full request journal (auth header) lets the leak/bearer negatives see every request the
// dashboard makes. Anything else 404s.
// =============================================================================================

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    authorization: Option<String>,
}

struct StubState {
    team: String,
    /// The windowed total the header renders.
    window_total: i64,
    /// The NACK items (each an object the dashboard renders as a row).
    items: Vec<Value>,
    requests: Mutex<Vec<Recorded>>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    fn recorded(&self) -> Vec<Recorded> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn canned_error(status: u16) -> Response {
    let body = json!({ "code": "not_found", "message": "not found" });
    (
        StatusCode::from_u16(status).expect("valid canned status"),
        Json(body),
    )
        .into_response()
}

fn route_request(state: &StubState, path: &str) -> Response {
    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();

    match segs.as_slice() {
        ["xds", "nacks"] => Json(json!({
            "items": state.items,
            "window_total": state.window_total,
            "next_cursor": Value::Null,
        }))
        .into_response(),
        _ => canned_error(404),
    }
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let authorization = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let response = route_request(&state, &path);
    state.requests.lock().unwrap().push(Recorded {
        path,
        authorization,
    });
    response
}

async fn start_stub(state: StubState) -> StubUpstream {
    let state = Arc::new(state);
    let app = Router::new()
        .fallback(stub_handler)
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub upstream to an ephemeral port");
    let addr = listener.local_addr().expect("stub local addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    StubUpstream {
        base_url: format!("http://{addr}"),
        state,
        handle,
    }
}

// =============================================================================================
// Canned payload builder (shapes as the real CP returns them; reused from the Operations tab's
// non-uistyle contract suite).
// =============================================================================================

fn nack_item(
    id: &str,
    node_id: &str,
    type_url: &str,
    version_rejected: &str,
    error_message: &str,
    quarantined: &[&str],
    created_at: &str,
) -> Value {
    json!({
        "id": id,
        "node_id": node_id,
        "type_url": type_url,
        "version_rejected": version_rejected,
        "error_message": error_message,
        "quarantined_resources": quarantined,
        "created_at": created_at,
    })
}

/// Two items: item A carries a quarantined resource, item B carries none — so we can prove the
/// crit pill is specific to quarantine.
fn nack_items() -> Vec<Value> {
    vec![
        nack_item(
            "019f9999-0000-7000-8000-00000000000a",
            NODE_A,
            TYPE_A,
            VER_A,
            ERR_A,
            &[QUARANTINED_A],
            "2026-07-20T00:00:00Z",
        ),
        nack_item(
            "019f9999-0000-7000-8000-00000000000b",
            NODE_B,
            TYPE_B,
            VER_B,
            ERR_B,
            &[],
            "2026-07-21T00:00:00Z",
        ),
    ]
}

struct NacksFixture {
    stub_state: StubState,
    team: String,
}

fn nacks_fixture() -> NacksFixture {
    let team = unique("team");
    NacksFixture {
        stub_state: StubState {
            team: team.clone(),
            window_total: 7,
            items: nack_items(),
            requests: Mutex::new(Vec::new()),
        },
        team,
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

/// GET a dashboard page/partial at `path` and assert the 200/HTML/no-token-leak baseline.
async fn fetch_page(http: &reqwest::Client, dash: &Dashboard, path: &str) -> String {
    let url = dash.nonce_url(path);
    let resp = fetch(http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/{path} must serve 200"
    );
    let body = resp.text().await.expect("response body");
    assert!(
        body.contains('<'),
        "GET /<nonce>/{path} must be HTML; body:\n{body}"
    );
    assert!(
        !body.contains(SECRET_TOKEN),
        "the response must never leak the bearer token; body:\n{body}"
    );
    body
}

// =============================================================================================
// Markup assertion helpers (spec-level, tolerant of whitespace / extra classes). Copied verbatim
// from the APIs uistyle harness; only the helpers this slice's criteria need are kept.
// =============================================================================================

/// The opening tag (e.g. `<table ...>`) starting at the first occurrence of `tag_start`
/// (e.g. `<table`), up to and including its closing `>`.
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

/// The segment of `html` from the start of the first `<open ...>` tag through the matching
/// `</close>` (inclusive), for scoping assertions to one region.
fn region<'a>(html: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = html.find(open)?;
    let end = html[start..].find(close)? + start + close.len();
    Some(&html[start..end])
}

/// The text nodes of every element carrying ALL of `classes`: for each matching `class=`
/// attribute, the text between the end of its opening tag and the matching close tag, with depth
/// counting so nested same-tag elements don't truncate the region, inner tags stripped.
fn texts_of_elements_with_classes(html: &str, classes: &[&str]) -> Vec<String> {
    let mut texts = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("class=\"") {
        let after = &rest[i + "class=\"".len()..];
        let Some(end) = after.find('"') else { break };
        let tokens: Vec<&str> = after[..end].split_whitespace().collect();
        if classes.iter().all(|c| tokens.contains(c)) {
            let tag_name = rest[..i]
                .rfind('<')
                .map(|lt| {
                    rest[lt + 1..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
                .unwrap_or_default();
            if let Some(gt) = after[end..].find('>') {
                let text_start = end + gt + 1;
                let open_pat = format!("<{tag_name}");
                let close = format!("</{tag_name}>");
                let mut region_end = after.len() - text_start;
                if !tag_name.is_empty() {
                    let mut depth = 1usize;
                    let mut pos = text_start;
                    while pos < after.len() {
                        let next_open = after[pos..].find(&open_pat).map(|x| pos + x);
                        let next_close = after[pos..].find(&close).map(|x| pos + x);
                        match (next_open, next_close) {
                            (_, None) => break,
                            (Some(o), Some(c)) if o < c => {
                                depth += 1;
                                pos = o + open_pat.len();
                            }
                            (Some(_), Some(c)) | (None, Some(c)) => {
                                depth -= 1;
                                if depth == 0 {
                                    region_end = c - text_start;
                                    break;
                                }
                                pos = c + close.len();
                            }
                        }
                    }
                } else {
                    region_end = after[text_start..]
                        .find('<')
                        .unwrap_or(after.len() - text_start);
                }
                let raw = &after[text_start..text_start + region_end];
                let mut out = String::with_capacity(raw.len());
                let mut r2 = raw;
                while let Some(lt) = r2.find('<') {
                    out.push_str(&r2[..lt]);
                    let Some(gt2) = r2[lt..].find('>') else { break };
                    r2 = &r2[lt + gt2 + 1..];
                }
                out.push_str(r2);
                texts.push(out.trim().to_string());
            }
        }
        rest = &after[end..];
    }
    texts
}

/// The positions (byte offsets) of every element whose class tokens contain `class`.
fn class_positions(html: &str, class: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut rest = html;
    let mut offset = 0usize;
    while let Some(i) = rest.find("class=\"") {
        let after = &rest[i + "class=\"".len()..];
        let Some(end) = after.find('"') else { break };
        if after[..end].split_whitespace().any(|t| t == class) {
            positions.push(offset + i);
        }
        offset += i + "class=\"".len() + end;
        rest = &after[end..];
    }
    positions
}

/// Assert the inline-forbidden rules shared by every partial/page:
/// no inline `<style>` element, every `<script>` carries src=, no inline `style=`.
fn assert_no_inline(partial: &str, which: &str) {
    let lower = partial.to_lowercase();
    assert!(
        !lower.contains("<style"),
        "the {which} must not contain an inline <style> element; body:\n{partial}"
    );
    for tag in all_opening_tags(&lower, "<script") {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden in the {which}; every script tag \
             must carry src=; offending tag: {tag:?}"
        );
    }
    assert!(
        !lower.contains(" style="),
        "the {which} must not contain any inline style= attribute; body:\n{partial}"
    );
}

/// The `<tr>...</tr>` region containing `needle` (row-scoped assertions). Scans back from the
/// needle to the enclosing `<tr` — never first-textual-occurrence matching.
fn row_of<'a>(partial: &'a str, needle: &str) -> &'a str {
    let at = partial
        .find(needle)
        .unwrap_or_else(|| panic!("row for {needle:?} must exist; body:\n{partial}"));
    let tr_open = partial[..at]
        .rfind("<tr")
        .unwrap_or_else(|| panic!("row for {needle:?} must be a <tr>; body:\n{partial}"));
    region(&partial[tr_open..], "<tr", "</tr>")
        .unwrap_or_else(|| panic!("row for {needle:?} must be closed; body:\n{partial}"))
}

/// Every `<table>` in `html` must sit inside a `.tablewrap` wrapper: for each table, a
/// tablewrap-classed element must open after the previous table and before this one. Asserts
/// the exact table count too.
fn assert_tables_wrapped(html: &str, which: &str, expected_tables: usize) {
    let tables: Vec<usize> = html.match_indices("<table").map(|(i, _)| i).collect();
    assert_eq!(
        tables.len(),
        expected_tables,
        "the {which} must render exactly {expected_tables} <table> element(s); found {}; \
         body:\n{html}",
        tables.len()
    );
    let wraps = class_positions(html, "tablewrap");
    let mut prev_table: Option<usize> = None;
    for table_at in &tables {
        let wrapped = wraps
            .iter()
            .any(|&w| w < *table_at && prev_table.is_none_or(|p| w > p));
        assert!(
            wrapped,
            "every <table> in the {which} must be wrapped in `.tablewrap`; unwrapped table at \
             byte {table_at}; body:\n{html}"
        );
        prev_table = Some(*table_at);
    }
}

/// Every recorded upstream request carried the bearer token.
fn assert_bearer(recorded: &[Recorded]) {
    let want_auth = format!("Bearer {SECRET_TOKEN}");
    for req in recorded {
        assert_eq!(
            req.authorization.as_deref(),
            Some(want_auth.as_str()),
            "upstream request to {} must carry `Authorization: Bearer <token>`; got {:?}",
            req.path,
            req.authorization
        );
    }
}

// =============================================================================================
// Criterion 1: a quarantined NACK entry renders as `pill crit` carrying the quarantined
// resource's verbatim status text — scoped to the row that represents the quarantined entry.
// A non-quarantined row carries NO crit pill (crit is specific to quarantine).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quarantined_nack_row_renders_pill_crit_and_non_quarantined_row_does_not() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let body = fetch_page(&http, &dash, "partials/operations/nacks").await;

    // The quarantined row (identified by its node_id) carries a `pill crit` whose text shows
    // the quarantined resource verbatim.
    let quarantined_row = row_of(&body, NODE_A);
    let crit_texts = texts_of_elements_with_classes(quarantined_row, &["pill", "crit"]);
    assert!(
        crit_texts.iter().any(|t| t.contains(QUARANTINED_A)),
        "the quarantined NACK entry (node {NODE_A:?}) must render a `<… class=\"pill crit\">` \
         carrying the quarantined resource {QUARANTINED_A:?} verbatim; pill-crit texts in the \
         row: {crit_texts:?}; row:\n{quarantined_row}"
    );

    // The non-quarantined row (identified by its node_id) carries NO crit pill — proving the
    // crit pill is specific to quarantine, not applied to every NACK.
    let plain_row = row_of(&body, NODE_B);
    let plain_crit = texts_of_elements_with_classes(plain_row, &["pill", "crit"]);
    assert!(
        plain_crit.is_empty(),
        "a NACK entry with NO quarantined resources (node {NODE_B:?}) must NOT render a \
         `pill crit`; found pill-crit texts: {plain_crit:?}; row:\n{plain_row}"
    );

    assert_bearer(&stub.recorded());
}

// =============================================================================================
// Criterion 2: every `<table>` in each Operations partial is wrapped in `.tablewrap`.
// The Operations tab has a single data partial (nacks) rendering one table.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_nacks_table_is_wrapped_in_tablewrap() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let body = fetch_page(&http, &dash, "partials/operations/nacks").await;
    assert_tables_wrapped(&body, "operations nacks partial", 1);

    assert_bearer(&stub.recorded());
}

// =============================================================================================
// Criterion 3: no inline <style>, no src-less <script>, no inline style= attribute anywhere in
// any served Operations page/partial (the shell page and the nacks partial).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_pages_have_no_inline_style_script_or_style_attribute() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let shell = fetch_page(&http, &dash, "operations").await;
    assert_no_inline(&shell, "operations shell page");

    let nacks = fetch_page(&http, &dash, "partials/operations/nacks").await;
    assert_no_inline(&nacks, "operations nacks partial");

    assert_bearer(&stub.recorded());
}

// =============================================================================================
// Criterion 4: the CSP response header on the shell is still exactly `default-src 'self'`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_shell_csp_header_is_default_src_self() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let url = dash.nonce_url("operations");
    let resp = fetch(&http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/operations must serve the Operations shell page"
    );
    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("GET {url} must carry a Content-Security-Policy header"));
    assert_eq!(
        csp, "default-src 'self'",
        "the Operations shell CSP header must still be `default-src 'self'`"
    );

    let shell = resp.text().await.expect("shell body");
    assert!(
        !shell.contains(SECRET_TOKEN),
        "the shell must never leak the bearer token; body:\n{shell}"
    );
}
