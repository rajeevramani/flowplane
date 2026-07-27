//! fpv2-80z.6 — `flowplane dashboard` LEARNING tab UI restyle — black-box, spec-driven
//! contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix. The
//! contract under test is the learning sessions partial `GET /<nonce>/partials/learning/
//! sessions`, backed by a stub upstream serving the learning-sessions list
//! (`/api/v1/teams/{team}/learning-sessions?limit=500&offset=0`), the additive
//! discovery-session list, the api-definitions id→name list, and per-API spec-metadata
//! lists (`/api/v1/teams/{team}/api-definitions/{name}/specs`). No spec `/content` is ever
//! fetched by this panel.
//!
//! Acceptance criteria covered (served-HTML level):
//!   1. Sessions partial: renders as a `.panel` whose h2 carries `<span class="count">`
//!      "N of M"; the sessions table is wrapped in `.tablewrap`; NO `class="dataplanes"`;
//!      status pills per the map — completed → `pill good` "completed", failed →
//!      `pill crit` "failed", any other status string → `pill neutral` with the status
//!      text verbatim; produced-spec links keep their
//!      `hx-get="...partials/learning/content?api=..."` wiring.
//!   2. The unparsed-specs banner and partial-sweep notice banners still render their text.
//!   3. Sessions upstream 403 → "Not authorized"; 500 → "unavailable"; no inline
//!      <style>/<script>/style= in the partial or the shell.
//!
//! Parallel-safety: every test spawns its own stub upstream and dashboard child on ephemeral
//! ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique team/API/session names;
//! nothing binds a fixed port. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-learning-token-do-not-leak-91e2";

const TS: &str = "2026-01-01T00:00:00Z";
const TS2: &str = "2026-01-02T03:04:05Z";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Deterministic id with no accidental digit collisions.
fn uid(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving the learning-sessions
// list (mutable status), an empty discovery-session list, the api-definitions id→name list,
// and per-API spec-metadata lists (per-API mutable status). Anything else 404s.
// =============================================================================================

struct ApiFixture {
    id: u64,
    name: String,
    /// Status for this API's spec-metadata list (200 = healthy).
    specs_status: u16,
    specs: Vec<Value>,
}

struct StubState {
    team: String,
    sessions_status: u16,
    sessions: Vec<Value>,
    apis: Vec<ApiFixture>,
}

struct StubUpstream {
    base_url: String,
    handle: JoinHandle<()>,
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn canned_error(status: u16) -> Response {
    let body = match status {
        401 => json!({ "code": "unauthorized", "message": "missing or invalid token" }),
        403 => json!({ "code": "forbidden", "message": "access denied" }),
        404 => json!({ "code": "not_found", "message": "not found" }),
        _ => json!({ "code": "internal", "message": "boom" }),
    };
    (
        StatusCode::from_u16(status).expect("valid canned status"),
        Json(body),
    )
        .into_response()
}

/// Slice `items` per limit/offset into the uniform Page envelope.
fn paged(items: &[Value], query: &str) -> Response {
    let mut limit: u64 = 500;
    let mut offset: u64 = 0;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "limit" => limit = v.parse().unwrap_or(limit),
                "offset" => offset = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    let len = items.len() as u64;
    let start = offset.min(len);
    let end = offset.saturating_add(limit).min(len);
    Json(json!({
        "items": items[start as usize..end as usize].to_vec(),
        "total": len,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

async fn stub_handler(State(state): State<std::sync::Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();

    match segs.as_slice() {
        ["learning-sessions"] => {
            if state.sessions_status != 200 {
                return canned_error(state.sessions_status);
            }
            paged(&state.sessions, &query)
        }
        // fpv2-g87: the Learning tab also sweeps the discovery-session metadata list; these
        // tests do not exercise discovery rows, so a clean 200-empty keeps sweeps healthy.
        ["learning-discovery-sessions"] => paged(&[], &query),
        ["api-definitions"] => {
            let items: Vec<Value> = state
                .apis
                .iter()
                .map(|a| json!({ "id": uid(a.id), "name": a.name }))
                .collect();
            paged(&items, &query)
        }
        ["api-definitions", name, "specs"] => match state.apis.iter().find(|a| a.name == *name) {
            Some(a) => {
                if a.specs_status != 200 {
                    return canned_error(a.specs_status);
                }
                paged(&a.specs, &query)
            }
            None => canned_error(404),
        },
        _ => canned_error(404),
    }
}

async fn start_stub(state: StubState) -> StubUpstream {
    let state = std::sync::Arc::new(state);
    let app = Router::new().fallback(stub_handler).with_state(state);
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
// Canned payload builders (wire shapes as the real CP returns them).
// =============================================================================================

/// A learning-sessions LIST item, wire shape.
fn session_item(
    id: u64,
    name: &str,
    status: &str,
    api_definition_id: Value,
    completed_at: Value,
) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "status": status,
        "api_definition_id": api_definition_id,
        "route_config_id": uid(id + 9000),
        "target_sample_count": 100,
        "max_bytes": 1_000_000,
        "max_distinct_paths": 100,
        "sample_count": 42,
        "byte_count": 2048,
        "path_count": 3,
        "drop_count": 0,
        "started_at": TS,
        "completed_at": completed_at,
        "updated_at": TS2,
        "created_at": TS,
    })
}

/// A spec-metadata LIST item with the learned version's capture-session provenance (the
/// wire field the CP extracts from the document's learning-source stamp).
fn learned_spec_item(id: u64, version: u64, session_id: u64) -> Value {
    json!({
        "id": uid(id),
        "version": version,
        "source_kind": "learned",
        "format": "openapi3",
        "spec_hash": format!("{version:064x}"),
        "created_at": TS,
        "capture_session_id": uid(session_id),
    })
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

/// The segment of `html` from the start of the first `<open ...>` tag through the matching
/// `</close>` (inclusive), for scoping assertions to one region.
fn region<'a>(html: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = html.find(open)?;
    let end = html[start..].find(close)? + start + close.len();
    Some(&html[start..end])
}

/// The byte offset (in `html`) just past the `</tag>` that closes the `<tag ...>` opening at
/// `open_at`, matched with depth counting so nested same-name elements don't close it early.
/// `open_at` must point at the `<` of the opening tag. Returns `None` if never balanced.
fn matching_close_end(html: &str, open_at: usize, tag: &str) -> Option<usize> {
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}>");
    let mut depth = 0usize;
    let mut pos = open_at;
    loop {
        let next_open = html[pos..].find(&open_pat).map(|x| pos + x);
        let next_close = html[pos..].find(&close_pat).map(|x| pos + x);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                pos = o + open_pat.len();
            }
            (_, Some(c)) => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(c + close_pat.len());
                }
                pos = c + close_pat.len();
            }
            (_, None) => return None,
        }
    }
}

/// The tokens of every `class="..."` attribute in `html`, in document order.
fn class_token_sets(html: &str) -> Vec<Vec<String>> {
    let mut sets = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("class=\"") {
        let after = &rest[i + "class=\"".len()..];
        let Some(end) = after.find('"') else { break };
        sets.push(
            after[..end]
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        );
        rest = &after[end..];
    }
    sets
}

/// Does any element in `html` carry ALL of `classes` as whole class tokens?
fn has_element_with_classes(html: &str, classes: &[&str]) -> bool {
    class_token_sets(html)
        .iter()
        .any(|tokens| classes.iter().all(|c| tokens.iter().any(|t| t == c)))
}

/// The text nodes of every element carrying ALL of `classes`: for each matching `class=`
/// attribute, the text between the end of its opening tag and the next `<`, with depth
/// counting so nested same-tag elements don't truncate the region.
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

/// Assert the inline-forbidden rules shared by every partial: no inline `<style>` element,
/// every `<script>` carries src=, no inline `style=`.
fn assert_no_inline(partial: &str, which: &str) {
    let lower = partial.to_lowercase();
    assert!(
        !lower.contains("<style"),
        "the {which} partial must not contain an inline <style> element; body:\n{partial}"
    );
    for tag in all_opening_tags(&lower, "<script") {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden in the {which} partial; every script tag \
             must carry src=; offending tag: {tag:?}"
        );
    }
    assert!(
        !lower.contains(" style="),
        "the {which} partial must not contain any inline style= attribute; body:\n{partial}"
    );
}

/// The `<tr>...</tr>` region containing `needle` (row-scoped assertions).
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

// =============================================================================================
// Fixture: one API (api-learn) whose learned v2 was produced by the COMPLETED session, and
// FIVE capture sessions exercising the COMPLETE status-pill map: completed (→ api-learn,
// produced v2 link) → `pill good`; failed → `pill crit`; and three distinct
// "everything else" cases that must ALL collapse to `pill neutral` with verbatim text —
// capturing (a known running state), cancelled (a known terminal state), and an
// unknown/future status string with no explicit map arm. No discovery sessions.
// =============================================================================================

struct LearningFixture {
    state: StubState,
    team: String,
    api_learn: String,
    sess_done: String,
    sess_failed: String,
    sess_running: String,
    sess_cancelled: String,
    sess_unknown: String,
    /// The verbatim, non-standard status string of the unknown/future-status session — the
    /// panel must echo it inside a `pill neutral`, never remap or drop it.
    unknown_status: String,
}

fn learning_fixture() -> LearningFixture {
    let team = unique("team");
    let api_learn = unique("api-learn");
    let sess_done = unique("sess-done");
    let sess_failed = unique("sess-failed");
    let sess_running = unique("sess-run");
    let sess_cancelled = unique("sess-cancelled");
    let sess_unknown = unique("sess-unknown");
    // A status the pill map has no explicit arm for — proves the neutral fallback is truly
    // "everything else", not an enumerated allow-list.
    let unknown_status = "reticulating".to_string();

    let state = StubState {
        team: team.clone(),
        sessions_status: 200,
        sessions: vec![
            session_item(500, &sess_done, "completed", json!(uid(100)), json!(TS2)),
            session_item(501, &sess_failed, "failed", Value::Null, json!(TS2)),
            session_item(502, &sess_running, "capturing", Value::Null, Value::Null),
            session_item(503, &sess_cancelled, "cancelled", Value::Null, json!(TS2)),
            session_item(
                504,
                &sess_unknown,
                &unknown_status,
                Value::Null,
                Value::Null,
            ),
        ],
        apis: vec![ApiFixture {
            id: 100,
            name: api_learn.clone(),
            specs_status: 200,
            // v2 learned by session 500 (produced link); v1 imported (never a produced link).
            specs: vec![
                learned_spec_item(102, 2, 500),
                json!({
                    "id": uid(101),
                    "version": 1,
                    "source_kind": "imported",
                    "format": "openapi3",
                    "spec_hash": format!("{:064x}", 1),
                    "created_at": TS,
                }),
            ],
        }],
    };

    LearningFixture {
        state,
        team,
        api_learn,
        sess_done,
        sess_failed,
        sess_running,
        sess_cancelled,
        sess_unknown,
        unknown_status,
    }
}

// =============================================================================================
// Criterion 1: sessions partial — `.panel` h2 with `<span class="count">` "N of M"; the
// sessions table wrapped in `.tablewrap`; NO class="dataplanes"; status pills per the map
// (completed → pill good "completed", failed → pill crit "failed", other status → pill
// neutral verbatim); produced-spec links keep their hx-get wiring to the content partial.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_partial_panel_count_tablewrap_status_pills_and_spec_link_wiring() {
    let fx = learning_fixture();
    let stub = start_stub(fx.state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/learning/sessions").await;

    // The partial renders as a `.panel` whose h2 carries `<span class="count">` "N of M"
    // (5 rows shown of an upstream total of 5).
    assert!(
        has_element_with_classes(&partial, &["panel"]),
        "the sessions partial must render as a `.panel`; body:\n{partial}"
    );
    let h2 = region(&partial, "<h2", "</h2>")
        .unwrap_or_else(|| panic!("the sessions panel must have an <h2>; body:\n{partial}"));
    let count_texts = texts_of_elements_with_classes(h2, &["count"]);
    assert!(
        count_texts.iter().any(|t| t == "5 of 5"),
        "the sessions panel h2 must carry `<span class=\"count\">` with \"5 of 5\"; \
         found count texts: {count_texts:?}; h2:\n{h2}"
    );

    // The sessions <table> is CONTAINED inside the `.tablewrap` element: the wrapper opens
    // before the table, and — matching its opening `<div>` to the balanced `</div>` — closes
    // AFTER the table ends. Position-before alone would pass even if the wrapper closed before
    // the table began (siblings, not nesting); this proves true containment.
    let wrap_class_at = class_positions(&partial, "tablewrap")
        .first()
        .copied()
        .unwrap_or_else(|| {
            panic!("the sessions table must be wrapped in `.tablewrap`; body:\n{partial}")
        });
    // Back up from the class attribute to the `<` of the wrapper's opening tag, and read its
    // element name (expected `div`, but derived so the assertion follows the markup).
    let wrap_open = partial[..wrap_class_at]
        .rfind('<')
        .unwrap_or_else(|| panic!("the `.tablewrap` must be an element; body:\n{partial}"));
    let wrap_tag_name: String = partial[wrap_open + 1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    assert!(
        !wrap_tag_name.is_empty(),
        "could not read the `.tablewrap` element name; body:\n{partial}"
    );
    let table_at = partial
        .find("<table")
        .unwrap_or_else(|| panic!("the sessions partial must render a table; body:\n{partial}"));
    let table_close_end = partial
        .find("</table>")
        .map(|c| c + "</table>".len())
        .unwrap_or_else(|| panic!("the sessions <table> must be closed; body:\n{partial}"));
    let wrap_close_end = matching_close_end(&partial, wrap_open, &wrap_tag_name)
        .unwrap_or_else(|| panic!("the `.tablewrap` element must be closed; body:\n{partial}"));
    assert!(
        wrap_open < table_at,
        "the `.tablewrap` wrapper must OPEN before the <table>; wrap_open={wrap_open}, \
         table_at={table_at}; body:\n{partial}"
    );
    assert!(
        wrap_close_end > table_close_end,
        "the `.tablewrap` wrapper must CLOSE (</{wrap_tag_name}> at {wrap_close_end}) after the \
         </table> ends ({table_close_end}) — the table must be nested inside the wrapper, not a \
         sibling; body:\n{partial}"
    );

    // NO class="dataplanes" anywhere in the partial.
    assert!(
        !has_element_with_classes(&partial, &["dataplanes"]),
        "the sessions partial must not contain class=\"dataplanes\" anywhere; body:\n{partial}"
    );

    // Status pills per the map, row-scoped.
    let done_row = row_of(&partial, &fx.sess_done);
    let good_texts = texts_of_elements_with_classes(done_row, &["pill", "good"]);
    assert!(
        good_texts.iter().any(|t| t == "completed"),
        "a completed session must show `<span class=\"pill good\">completed</span>`; \
         found pill-good texts: {good_texts:?}; row:\n{done_row}"
    );

    let failed_row = row_of(&partial, &fx.sess_failed);
    let crit_texts = texts_of_elements_with_classes(failed_row, &["pill", "crit"]);
    assert!(
        crit_texts.iter().any(|t| t == "failed"),
        "a failed session must show `<span class=\"pill crit\">failed</span>`; \
         found pill-crit texts: {crit_texts:?}; row:\n{failed_row}"
    );

    // The neutral fallback is "EVERYTHING else", not an enumerated allow-list. Assert three
    // distinct non-good/non-crit statuses each collapse to `pill neutral` with verbatim text,
    // scoping to each session's own row: a known running state (capturing), a known terminal
    // state (cancelled), and an unknown/future status string with no explicit map arm.
    let running_row = row_of(&partial, &fx.sess_running);
    let neutral_texts = texts_of_elements_with_classes(running_row, &["pill", "neutral"]);
    assert!(
        neutral_texts.iter().any(|t| t == "capturing"),
        "a `capturing` session must fall back to `<span class=\"pill neutral\">` with the \
         status text verbatim (\"capturing\"); found pill-neutral texts: {neutral_texts:?}; \
         row:\n{running_row}"
    );

    let cancelled_row = row_of(&partial, &fx.sess_cancelled);
    let cancelled_neutral = texts_of_elements_with_classes(cancelled_row, &["pill", "neutral"]);
    assert!(
        cancelled_neutral.iter().any(|t| t == "cancelled"),
        "a `cancelled` session must fall back to `<span class=\"pill neutral\">cancelled</span>` \
         (only completed→good and failed→crit are special-cased); found pill-neutral texts: \
         {cancelled_neutral:?}; row:\n{cancelled_row}"
    );
    // The cancelled status must NOT be mistaken for a crit/good state.
    assert!(
        texts_of_elements_with_classes(cancelled_row, &["pill", "crit"]).is_empty()
            && texts_of_elements_with_classes(cancelled_row, &["pill", "good"]).is_empty(),
        "a `cancelled` session must render ONLY a neutral pill, never good/crit; row:\n\
         {cancelled_row}"
    );

    let unknown_row = row_of(&partial, &fx.sess_unknown);
    let unknown_neutral = texts_of_elements_with_classes(unknown_row, &["pill", "neutral"]);
    assert!(
        unknown_neutral.contains(&fx.unknown_status),
        "an unknown/future status string ({:?}) must fall back to `<span class=\"pill \
         neutral\">` with the status echoed VERBATIM — never remapped or dropped; found \
         pill-neutral texts: {unknown_neutral:?}; row:\n{unknown_row}",
        fx.unknown_status
    );
    assert!(
        texts_of_elements_with_classes(unknown_row, &["pill", "crit"]).is_empty()
            && texts_of_elements_with_classes(unknown_row, &["pill", "good"]).is_empty(),
        "an unknown-status session must render ONLY a neutral pill, never good/crit; row:\n\
         {unknown_row}"
    );

    // The completed session's learned v2 renders as a produced-spec link labeled
    // "<api> v2", wired with hx-get to the content partial.
    let produced_v2 = format!("{} v2", fx.api_learn);
    assert!(
        partial.contains(&produced_v2),
        "the completed session's learned v2 must render as a produced-spec link labeled \
         {produced_v2:?}; body:\n{partial}"
    );
    let link_tags: Vec<&str> = ["<button", "<a"]
        .into_iter()
        .flat_map(|start| all_opening_tags(&partial, start))
        .filter(|tag| tag.contains("hx-get"))
        .collect();
    assert!(
        link_tags.iter().any(|tag| {
            tag.contains(&format!("partials/learning/content?api={}", fx.api_learn))
                && tag.contains("version=2")
        }),
        "the produced-spec link must keep its `hx-get=\"...partials/learning/content?api=...\"\
         ` wiring; hx-get tags: {link_tags:?}"
    );
}

// =============================================================================================
// Criterion 2: the TWO distinct sweep-degradation banners each render as their own element,
// asserted independently by their semantic text (not "some banner mentions specs"). Fixture:
// two completed sessions on two distinct APIs — api-a's spec list carries one malformed item
// (typed-decode failure → the unparsed-specs / version-skew banner), api-b's spec list fails
// 500 (→ the partial-data / "collection is incomplete" budget banner) — while both session
// rows still render beneath them.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unparsed_specs_and_partial_notice_banners_render_text() {
    let team = unique("team");
    let api_a = unique("api-skewed");
    let api_b = unique("api-broken");
    let sess_a = unique("sess-a");
    let sess_b = unique("sess-b");

    let state = StubState {
        team: team.clone(),
        sessions_status: 200,
        sessions: vec![
            session_item(600, &sess_a, "completed", json!(uid(200)), json!(TS2)),
            session_item(601, &sess_b, "completed", json!(uid(210)), json!(TS2)),
        ],
        apis: vec![
            ApiFixture {
                id: 200,
                name: api_a.clone(),
                specs_status: 200,
                specs: vec![
                    learned_spec_item(202, 2, 600),
                    // Version-skew item: fails typed decode (version must be an integer) —
                    // must surface as the unparsed-specs banner, never silently dropped.
                    json!({ "id": uid(201), "version": "not-a-number" }),
                ],
            },
            ApiFixture {
                id: 210,
                name: api_b.clone(),
                specs_status: 500, // → the partial-data notice for "spec versions"
                specs: vec![],
            },
        ],
    };
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/learning/sessions").await;

    // The rows still render beneath the banners.
    assert!(
        partial.contains(&sess_a) && partial.contains(&sess_b),
        "session rows must still render alongside the banners; body:\n{partial}"
    );

    // Collect every `.banner` element's text, so each message can be asserted as its OWN
    // banner (not merely "some banner mentions specs").
    assert!(
        has_element_with_classes(&partial, &["banner"]),
        "the unparsed-specs and partial-sweep notices must render as `.banner` elements; \
         body:\n{partial}"
    );
    let banner_texts: Vec<String> = texts_of_elements_with_classes(&partial, &["banner"])
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();

    // Banner 1 — the partial-data / budget notice: the spec-versions sweep for api-b returned
    // 500, so the collection is incomplete. It must render as its OWN banner and name that the
    // spec-versions data is partial/incomplete.
    let partial_data_banner = banner_texts.iter().find(|t| {
        let lc = t.to_lowercase();
        lc.contains("partial") && lc.contains("incomplete") && lc.contains("spec")
    });
    assert!(
        partial_data_banner.is_some(),
        "a distinct partial-data banner must announce that the spec-versions collection is \
         incomplete (the api-b spec sweep failed 500); banner texts: {banner_texts:?}; \
         body:\n{partial}"
    );

    // Banner 2 — the unparsed-spec-rows / version-skew notice: api-a's spec list carried one
    // row that fails typed decode. It must render as its OWN, DISTINCT banner naming that
    // spec-version row(s) could not be parsed (version skew) — never silently dropped.
    let unparsed_banner = banner_texts.iter().find(|t| {
        let lc = t.to_lowercase();
        lc.contains("could not be parsed") && (lc.contains("version skew") || lc.contains("skew"))
    });
    assert!(
        unparsed_banner.is_some(),
        "a distinct unparsed-specs banner must announce that spec-version row(s) could not be \
         parsed (version skew); banner texts: {banner_texts:?}; body:\n{partial}"
    );

    // The two messages are genuinely SEPARATE banner elements, not one banner matching both
    // needles.
    assert_ne!(
        partial_data_banner, unparsed_banner,
        "the partial-data notice and the unparsed-specs notice must be two DISTINCT banners, \
         not a single combined one; banner texts: {banner_texts:?}"
    );
}

// =============================================================================================
// Criterion 3: sessions upstream 403 → "Not authorized"; 500 → "unavailable"; no inline
// <style>/<script>/style= in the sessions partial or the learning shell.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_partial_403_not_authorized_500_unavailable_and_no_inline() {
    let http = client();

    // 403 on the learning-sessions list → the partial says "Not authorized".
    {
        let fx = learning_fixture();
        let team = fx.team.clone();
        let mut state = fx.state;
        state.sessions_status = 403;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let partial = fetch_page(&http, &dash, "partials/learning/sessions").await;
        assert!(
            partial.to_lowercase().contains("not authorized"),
            "a 403 from the learning-sessions list must render \"Not authorized\" in the \
             partial; body:\n{partial}"
        );
    }

    // First-page 500 → "unavailable".
    {
        let fx = learning_fixture();
        let team = fx.team.clone();
        let mut state = fx.state;
        state.sessions_status = 500;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let partial = fetch_page(&http, &dash, "partials/learning/sessions").await;
        assert!(
            partial.to_lowercase().contains("unavailable"),
            "a first-page 500 from the learning-sessions list must render \"unavailable\" in \
             the partial; body:\n{partial}"
        );
    }

    // Healthy: no inline <style>, no inline <script>, no style= in the sessions partial or
    // the learning shell.
    {
        let fx = learning_fixture();
        let stub = start_stub(fx.state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
        let partial = fetch_page(&http, &dash, "partials/learning/sessions").await;
        assert_no_inline(&partial, "learning sessions");
        let shell = fetch_page(&http, &dash, "learning").await;
        assert_no_inline(&shell, "learning shell");
    }
}
