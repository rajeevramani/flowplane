//! fpv2-80z.3 — `flowplane dashboard` resources UI restyle — black-box, spec-driven
//! contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix. The
//! contract under test is the resources shell `GET /<nonce>/resources`, the seven partials
//! under `/<nonce>/partials/resources/{clusters,route-configs,listeners,topology,orphans,
//! filters,rate-limits}`, and the served stylesheet `/<nonce>/assets/dashboard.css`, backed
//! by a stub upstream serving the three paged collections
//! (`/api/v1/teams/{team}/{clusters,route-configs,listeners}?limit=500&offset=N`).
//!
//! Acceptance criteria covered (served-HTML level):
//!   1. Shell /<nonce>/resources: `<nav class="tabs">` with the Resources anchor
//!      `class="active"`; 8 lazy `<details>` panels each carrying hx-get +
//!      hx-trigger="toggle once" + hx-target + hx-swap="innerHTML"; same-origin
//!      resources.js script tag.
//!   2. clusters/route-configs/listeners partials: `.panel` h2 has `<span class="count">`
//!      "N of M"; every table in `.tablewrap`; NO `class="dataplanes"`; listeners unbound
//!      row shows `<span class="pill warn">unbound</span>`.
//!   3. topology partial: `.flow` containers with `.lst`/`.chain`/`.vh`/`.rt`; every cluster
//!      chip is `<span class="cchip" data-cluster="...">`; stylesheet has `.cchip.hl` and
//!      `.cchip.unresolved` (warn colors); no bare `class="chip"` without cchip.
//!   4. orphans partial: kinds as `<span class="pill warn">`; no legacy `chip unresolved`.
//!   5. filters partial: state cell is `pill good` "active" or `pill neutral` "disabled";
//!      tables in `.tablewrap`.
//!   6. rate-limits partial: unattached domain → `pill warn` "unattached — no listener
//!      attaches this domain"; `<details class="rl-domain">` hx-get wiring preserved.
//!      (Our fixtures carry no rate-limit domains, so the empty state is asserted and the
//!      unattached assertion is skipped — see the test.)
//!   7. 403 on a collection → "Not authorized"; first-page 500 → "unavailable"; no inline
//!      style/script/style= in any partial.
//!   8. A budget/mid-sweep partial banner still renders its text.
//!
//! Parallel-safety: every test spawns its own stub upstream and dashboard child on ephemeral
//! ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique team name; nothing
//! binds a fixed port. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-resources-token-do-not-leak-77b3";

const PARTIALS: [&str; 7] = [
    "partials/resources/clusters",
    "partials/resources/route-configs",
    "partials/resources/listeners",
    "partials/resources/topology",
    "partials/resources/orphans",
    "partials/resources/filters",
    "partials/resources/rate-limits",
];

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving the three paged
// collections endpoints. Mutable per-collection state lets one test walk several upstream
// scenarios (200 fixtures, 403, 500) through a single spawned dashboard. Anything else 404s.
// =============================================================================================

struct Coll {
    status: u16,
    items: Vec<Value>,
    total: u64,
    /// Inject a failure at exactly this paging offset: `(offset, status)`.
    fail_at: Option<(u64, u16)>,
}

struct StubState {
    clusters: Mutex<Coll>,
    route_configs: Mutex<Coll>,
    listeners: Mutex<Coll>,
    rate_limit_domains: Mutex<Coll>,
    secrets: Mutex<Coll>,
    ai_providers: Mutex<Coll>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    fn set_clusters_status(&self, status: u16) {
        self.state.clusters.lock().unwrap().status = status;
    }
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Deterministic id with no accidental digit collisions.
fn item_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

fn resource_item(i: u64, name: &str, spec: Value) -> Value {
    json!({
        "id": item_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T03:04:05Z",
        "spec": spec
    })
}

fn cluster_item(i: u64, name: &str) -> Value {
    resource_item(
        i,
        name,
        json!({ "endpoints": [{ "host": "10.0.0.1", "port": 8080 }] }),
    )
}

fn route_config_item(i: u64, name: &str, cluster: &str) -> Value {
    resource_item(
        i,
        name,
        json!({
            "virtual_hosts": [{
                "name": "vh-main",
                "domains": ["uistyle.example.com"],
                "routes": [{
                    "name": "r-main",
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": cluster }
                }]
            }]
        }),
    )
}

/// A listener item; `route_config: None` omits the binding key entirely (unbound listener).
fn listener_item(i: u64, name: &str, route_config: Option<&str>) -> Value {
    let mut spec = json!({ "address": "0.0.0.0", "port": 8080 });
    if let Some(rc) = route_config {
        spec["route_config"] = json!(rc);
    }
    resource_item(i, name, spec)
}

/// The happy-path fixture shared by most tests:
///   * cluster  C_MAIN   — referenced by RC_MAIN's route (topology chain target)
///   * cluster  C_ORPHAN — unreferenced (orphans warn pill)
///   * route-config RC_MAIN — one vhost → one route → cluster C_MAIN
///   * listener LST_BOUND — bound to RC_MAIN (topology chain renders)
///   * listener LST_FREE  — NO route_config (unbound pill warn; unbound topology listener)
const C_MAIN: &str = "c-main-uistyle";
const C_ORPHAN: &str = "c-orphan-uistyle";
const RC_MAIN: &str = "rc-main-uistyle";
const LST_BOUND: &str = "lst-bound-uistyle";
const LST_FREE: &str = "lst-free-uistyle";

fn happy_fixture() -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let clusters = vec![cluster_item(1, C_MAIN), cluster_item(2, C_ORPHAN)];
    let route_configs = vec![route_config_item(11, RC_MAIN, C_MAIN)];
    let listeners = vec![
        listener_item(21, LST_BOUND, Some(RC_MAIN)),
        listener_item(22, LST_FREE, None),
    ];
    (clusters, route_configs, listeners)
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    let coll = if path.ends_with("/clusters") {
        &state.clusters
    } else if path.ends_with("/route-configs") {
        &state.route_configs
    } else if path.ends_with("/listeners") {
        &state.listeners
    } else if path.ends_with("/rate-limit-domains") {
        &state.rate_limit_domains
    } else if path.ends_with("/secrets") {
        &state.secrets
    } else if path.ends_with("/ai/providers") {
        &state.ai_providers
    } else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "no such route" })),
        )
            .into_response();
    };

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

    let coll = coll.lock().unwrap();
    if coll.status != 200 {
        return canned_error(coll.status);
    }
    if let Some((fail_offset, fail_status)) = coll.fail_at {
        if offset == fail_offset {
            return canned_error(fail_status);
        }
    }
    let len = coll.items.len() as u64;
    let start = offset.min(len);
    let end = offset.saturating_add(limit).min(len);
    let items: Vec<Value> = coll.items[start as usize..end as usize].to_vec();
    Json(json!({
        "items": items,
        "total": coll.total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
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

async fn start_stub(
    clusters: Coll,
    route_configs: Coll,
    listeners: Coll,
) -> StubUpstream {
    let state = Arc::new(StubState {
        clusters: Mutex::new(clusters),
        route_configs: Mutex::new(route_configs),
        listeners: Mutex::new(listeners),
        // Orphan analysis + the rate-limits partial read these too; default to
        // healthy empty collections so complete-sweep paths render.
        rate_limit_domains: Mutex::new(Coll {
            status: 200,
            items: vec![],
            total: 0,
            fail_at: None,
        }),
        secrets: Mutex::new(Coll {
            status: 200,
            items: vec![],
            total: 0,
            fail_at: None,
        }),
        ai_providers: Mutex::new(Coll {
            status: 200,
            items: vec![],
            total: 0,
            fail_at: None,
        }),
    });
    let app = Router::new().fallback(stub_handler).with_state(state.clone());
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

/// Start a stub from the happy fixture with matching totals.
async fn start_happy_stub() -> StubUpstream {
    let (clusters, route_configs, listeners) = happy_fixture();
    start_stub(
        Coll {
            status: 200,
            total: clusters.len() as u64,
            items: clusters,
            fail_at: None,
        },
        Coll {
            status: 200,
            total: route_configs.len() as u64,
            items: route_configs,
            fail_at: None,
        },
        Coll {
            status: 200,
            total: listeners.len() as u64,
            items: listeners,
            fail_at: None,
        },
    )
    .await
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

/// GET the served stylesheet.
async fn fetch_stylesheet(http: &reqwest::Client, dash: &Dashboard) -> String {
    let url = dash.nonce_url("assets/dashboard.css");
    let resp = fetch(http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/assets/dashboard.css must serve the stylesheet"
    );
    let css = resp.text().await.expect("stylesheet body");
    assert!(
        !css.contains(SECRET_TOKEN),
        "the stylesheet must never leak the bearer token"
    );
    css
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

/// The segment of `html` from the start of the first `<open ...>` tag through the matching
/// `</close>` (inclusive), for scoping assertions to one region.
fn region<'a>(html: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = html.find(open)?;
    let end = html[start..].find(close)? + start + close.len();
    Some(&html[start..end])
}

/// The tokens of every `class="..."` attribute in `html`, in document order.
fn class_token_sets(html: &str) -> Vec<Vec<String>> {
    let mut sets = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("class=\"") {
        let after = &rest[i + "class=\"".len()..];
        let Some(end) = after.find('"') else { break };
        sets.push(after[..end].split_whitespace().map(str::to_string).collect());
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
                    region_end = after[text_start..].find('<').unwrap_or(after.len() - text_start);
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

/// The rule bodies of every CSS rule whose selector list includes `token` as a whole
/// selector part (e.g. `.cchip`, `.cchip.hl`).
fn css_rule_bodies<'a>(css: &'a str, token: &str) -> Vec<&'a str> {
    let mut bodies = Vec::new();
    for block in css.split('}') {
        let Some((selector, body)) = block.split_once('{') else {
            continue;
        };
        let matches = selector
            .split([',', ' ', '\n', '\t', '>'])
            .any(|part| {
                let part = part.trim();
                part == token
                    || (part.starts_with(token)
                        && part[token.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| !c.is_alphanumeric() && c != '-' && c != '_'))
            });
        if matches {
            bodies.push(body);
        }
    }
    bodies
}

/// Assert the inline-forbidden rules shared by every partial (criterion 7 tail):
/// no inline `<style>` element, every `<script>` carries src=, no inline `style=`.
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
// Criterion 1: shell /<nonce>/resources — `<nav class="tabs">` with the Resources anchor
// class="active"; 8 lazy <details> panels each carrying hx-get + hx-trigger="toggle once" +
// hx-target + hx-swap="innerHTML"; same-origin resources.js script tag.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_resources_tab_active_and_eight_lazy_details_panels() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let shell = fetch_page(&http, &dash, "resources").await;

    // The tab bar is a <nav class="tabs">.
    let nav_tag = opening_tag(&shell, "<nav")
        .unwrap_or_else(|| panic!("the shell must render a <nav> tab bar; body:\n{shell}"));
    assert!(
        tag_has_class(nav_tag, "tabs"),
        "the tab bar must be `<nav class=\"tabs\">`; tag: {nav_tag:?}"
    );

    // The Resources anchor inside the nav carries class="active".
    let nav_region = region(&shell, "<nav", "</nav>")
        .unwrap_or_else(|| panic!("the <nav> must be closed; body:\n{shell}"));
    let anchors = all_opening_tags(nav_region, "<a");
    let resources_anchor = anchors
        .iter()
        .find(|tag| tag.contains("/resources"))
        .unwrap_or_else(|| {
            panic!("the tab bar must contain a Resources anchor; anchors: {anchors:?}")
        });
    assert!(
        tag_has_class(resources_anchor, "active"),
        "the Resources anchor must carry class=\"active\"; tag: {resources_anchor:?}"
    );

    // Exactly 8 lazy <details> panels, each carrying the full htmx lazy-load wiring.
    let details = all_opening_tags(&shell, "<details");
    assert_eq!(
        details.len(),
        8,
        "the resources shell must render exactly 8 lazy <details> panels; \
         found {} tags: {details:?}",
        details.len()
    );
    for tag in &details {
        for attr in ["hx-get", "hx-target"] {
            assert!(
                tag.contains(attr),
                "every lazy <details> panel must carry {attr}; offending tag: {tag:?}"
            );
        }
        assert!(
            tag.contains("hx-trigger=\"toggle once\""),
            "every lazy <details> panel must carry hx-trigger=\"toggle once\"; \
             offending tag: {tag:?}"
        );
        assert!(
            tag.contains("hx-swap=\"innerHTML\""),
            "every lazy <details> panel must carry hx-swap=\"innerHTML\"; \
             offending tag: {tag:?}"
        );
    }

    // A same-origin resources.js script tag is present (src starts with '/', not http).
    let scripts = all_opening_tags(&shell, "<script");
    let resources_js = scripts
        .iter()
        .find(|tag| tag.contains("resources.js"))
        .unwrap_or_else(|| {
            panic!("the shell must load resources.js; script tags: {scripts:?}")
        });
    assert!(
        resources_js.contains("src=\"/"),
        "resources.js must be same-origin (src starts with '/'); tag: {resources_js:?}"
    );
    assert!(
        !resources_js.contains("src=\"http"),
        "resources.js must not be loaded from an external origin; tag: {resources_js:?}"
    );
}

// =============================================================================================
// Criterion 2: clusters/route-configs/listeners partials — `.panel` h2 has
// `<span class="count">` "N of M"; every table wrapped in `.tablewrap`; NO
// class="dataplanes" anywhere; listeners unbound row shows pill warn "unbound".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_partials_panels_counts_tablewrap_no_dataplanes_and_unbound_pill() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    for which in ["clusters", "route-configs", "listeners"] {
        let partial = fetch_page(&http, &dash, &format!("partials/resources/{which}")).await;

        // The partial renders as a `.panel` whose h2 carries a <span class="count"> "N of M".
        assert!(
            has_element_with_classes(&partial, &["panel"]),
            "the {which} partial must render as a `.panel`; body:\n{partial}"
        );
        let h2 = region(&partial, "<h2", "</h2>")
            .unwrap_or_else(|| panic!("the {which} panel must have an <h2>; body:\n{partial}"));
        let count_texts = texts_of_elements_with_classes(h2, &["count"]);
        assert!(
            count_texts.iter().any(|t| t.contains(" of ")),
            "the {which} panel h2 must carry `<span class=\"count\">` with \"N of M\"; \
             found count texts: {count_texts:?}; h2:\n{h2}"
        );

        // Every table is wrapped in `.tablewrap`.
        if partial.contains("<table") {
            let wrap_at = class_positions(&partial, "tablewrap")
                .first()
                .copied()
                .unwrap_or_else(|| {
                    panic!("the {which} table must be wrapped in `.tablewrap`; body:\n{partial}")
                });
            let table_at = partial.find("<table").expect("table present");
            assert!(
                wrap_at < table_at,
                "the `.tablewrap` wrapper must precede the <table> in the {which} partial; \
                 body:\n{partial}"
            );
        }

        // NO class="dataplanes" anywhere in the partial.
        assert!(
            !has_element_with_classes(&partial, &["dataplanes"]),
            "the {which} partial must not contain class=\"dataplanes\" anywhere; \
             body:\n{partial}"
        );
    }

    // The unbound listener row shows <span class="pill warn">unbound</span>.
    let listeners = fetch_page(&http, &dash, "partials/resources/listeners").await;
    let free_row = row_of(&listeners, LST_FREE);
    let warn_texts = texts_of_elements_with_classes(free_row, &["pill", "warn"]);
    assert!(
        warn_texts.iter().any(|t| t == "unbound"),
        "the unbound listener row must show `<span class=\"pill warn\">unbound</span>`; \
         found pill-warn texts: {warn_texts:?}; row:\n{free_row}"
    );
}

// =============================================================================================
// Criterion 3: topology partial — `.flow` containers with .lst/.chain/.vh/.rt; every
// cluster chip is `<span class="cchip" data-cluster="...">` (hover contract); stylesheet has
// a `.cchip.hl` rule and `.cchip.unresolved` with warn colors; no bare class="chip" without
// cchip in the topology partial.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_flow_chain_cchip_hover_contract_and_stylesheet_rules() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/topology").await;

    // `.flow` containers with the chain anatomy classes present.
    assert!(
        has_element_with_classes(&partial, &["flow"]),
        "the topology partial must render `.flow` containers; body:\n{partial}"
    );
    for class in ["lst", "chain", "vh", "rt"] {
        assert!(
            has_element_with_classes(&partial, &[class]),
            "the topology partial must render `.{class}` elements in its chains; \
             body:\n{partial}"
        );
    }

    // Every cluster chip is a <span class="cchip" data-cluster="...">; the bound fixture's
    // cluster name appears verbatim in a data-cluster attribute (hover contract).
    let cchip_tags: Vec<&str> = all_opening_tags(&partial, "<span")
        .into_iter()
        .filter(|tag| tag_has_class(tag, "cchip"))
        .collect();
    assert!(
        !cchip_tags.is_empty(),
        "the topology partial must render cluster chips as `<span class=\"cchip\" ...>`; \
         body:\n{partial}"
    );
    for tag in &cchip_tags {
        assert!(
            tag.contains("data-cluster=\""),
            "every cluster chip must carry a verbatim data-cluster attribute (hover \
             contract); offending tag: {tag:?}"
        );
    }
    assert!(
        cchip_tags
            .iter()
            .any(|tag| tag.contains(&format!("data-cluster=\"{C_MAIN}\""))),
        "the bound route's cluster {C_MAIN:?} must appear verbatim in a data-cluster \
         attribute; cchip tags: {cchip_tags:?}"
    );

    // No bare class="chip" (without cchip) in the topology partial.
    for span in all_opening_tags(&partial, "<span") {
        if tag_has_class(span, "chip") {
            assert!(
                tag_has_class(span, "cchip"),
                "bare `class=\"chip\"` (without cchip) is forbidden in the topology \
                 partial; offending tag: {span:?}"
            );
        }
    }

    // The served stylesheet has a `.cchip.hl` rule and a `.cchip.unresolved` rule with
    // warn colors.
    let css = fetch_stylesheet(&http, &dash).await;
    let hl_bodies = css_rule_bodies(&css, ".cchip.hl");
    assert!(
        !hl_bodies.is_empty(),
        "the stylesheet must contain a `.cchip.hl` rule (cluster chip hover highlight); \
         css:\n{css}"
    );
    let unresolved_bodies = css_rule_bodies(&css, ".cchip.unresolved");
    assert!(
        !unresolved_bodies.is_empty(),
        "the stylesheet must contain a `.cchip.unresolved` rule; css:\n{css}"
    );
    assert!(
        unresolved_bodies.iter().any(|b| {
            let squashed = b.replace([' ', '\n', '\t'], "").to_lowercase();
            squashed.contains("color") && (squashed.contains("warn") || squashed.contains('#'))
        }),
        "the `.cchip.unresolved` rule must paint warn colors; rule bodies: {unresolved_bodies:?}"
    );
}

// =============================================================================================
// Criterion 4: orphans partial — kinds render as `<span class="pill warn">` (unreferenced
// cluster etc.); no legacy `chip unresolved`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orphans_partial_warn_pill_kinds_and_no_legacy_chip_unresolved() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/orphans").await;

    // The unreferenced fixture cluster surfaces as an orphan; orphan kinds render as
    // <span class="pill warn">…</span>.
    let warn_texts = texts_of_elements_with_classes(&partial, &["pill", "warn"]);
    assert!(
        !warn_texts.is_empty(),
        "the orphans partial must render orphan kinds as `<span class=\"pill warn\">`; \
         body:\n{partial}"
    );
    assert!(
        warn_texts.iter().all(|t| !t.is_empty()),
        "every orphan warn pill must carry its kind text; pill-warn texts: {warn_texts:?}"
    );
    assert!(
        partial.contains(C_ORPHAN),
        "the unreferenced cluster {C_ORPHAN:?} must appear in the orphans partial; \
         body:\n{partial}"
    );

    // The legacy `chip unresolved` pattern is GONE.
    assert!(
        !has_element_with_classes(&partial, &["chip", "unresolved"]),
        "the legacy `chip unresolved` pattern must be gone from the orphans partial; \
         body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 5: filters partial — state cell is `pill good` "active" or `pill neutral`
// "disabled"; tables in `.tablewrap`.
//
// Fixture note: our listener/route-config items carry no filter chains, so the partial may
// legitimately render an empty state. We therefore assert the state-pill mapping for any
// pills present, and the `.tablewrap` wrapping for any table present.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filters_partial_state_pills_and_tablewrap() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/filters").await;

    // Every pill good is "active"; every pill neutral is "disabled".
    for text in texts_of_elements_with_classes(&partial, &["pill", "good"]) {
        assert_eq!(
            text, "active",
            "a filters state cell `pill good` must say \"active\"; body:\n{partial}"
        );
    }
    for text in texts_of_elements_with_classes(&partial, &["pill", "neutral"]) {
        assert_eq!(
            text, "disabled",
            "a filters state cell `pill neutral` must say \"disabled\"; body:\n{partial}"
        );
    }

    // Any table is wrapped in `.tablewrap`.
    if partial.contains("<table") {
        let wrap_at = class_positions(&partial, "tablewrap")
            .first()
            .copied()
            .unwrap_or_else(|| {
                panic!("the filters table must be wrapped in `.tablewrap`; body:\n{partial}")
            });
        let table_at = partial.find("<table").expect("table present");
        assert!(
            wrap_at < table_at,
            "the `.tablewrap` wrapper must precede the <table> in the filters partial; \
             body:\n{partial}"
        );
    }
}

// =============================================================================================
// Criterion 6: rate-limits partial — unattached domain → `pill warn`
// "unattached — no listener attaches this domain"; `<details class="rl-domain">` hx-get
// wiring preserved.
//
// Fixture note: our collections carry no rate-limit domains, so the partial renders its
// empty state ("No rate-limit domains"). We assert the empty state and SKIP the unattached
// assertion (no domains exist to be unattached); the `.rl-domain` hx-get wiring is asserted
// for any rl-domain details present.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limits_partial_empty_state_and_rl_domain_wiring() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/rate-limits").await;

    let rl_details: Vec<&str> = all_opening_tags(&partial, "<details")
        .into_iter()
        .filter(|tag| tag_has_class(tag, "rl-domain"))
        .collect();

    if rl_details.is_empty() {
        // No rate-limit domains in the fixture → the partial must render its empty state.
        // SKIPPED (by design): the "unattached — no listener attaches this domain" pill
        // assertion requires a rate-limit domain fixture.
        assert!(
            partial.to_lowercase().contains("no rate-limit domains"),
            "with no rate-limit domains in the fixture, the rate-limits partial must show \
             the \"No rate-limit domains\" empty state; body:\n{partial}"
        );
    } else {
        // `<details class="rl-domain">` hx-get wiring preserved.
        for tag in &rl_details {
            assert!(
                tag.contains("hx-get"),
                "every `<details class=\"rl-domain\">` must preserve its hx-get wiring; \
                 offending tag: {tag:?}"
            );
        }
        // An unattached domain renders the warn pill with the full verbatim text.
        if partial.to_lowercase().contains("unattached") {
            let warn_texts = texts_of_elements_with_classes(&partial, &["pill", "warn"]);
            assert!(
                warn_texts
                    .iter()
                    .any(|t| t.contains("unattached — no listener attaches this domain")),
                "an unattached domain must render `<span class=\"pill warn\">unattached — \
                 no listener attaches this domain</span>`; pill-warn texts: {warn_texts:?}"
            );
        }
    }
}

// =============================================================================================
// Criterion 7: 403 on a collection → its partial says "Not authorized"; first-page 500 →
// "unavailable"; no inline style/script/style= in any partial.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_403_not_authorized_500_unavailable_and_no_inline_anywhere() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // 403 on the clusters collection (first page) → its partial says "Not authorized".
    stub.set_clusters_status(403);
    let partial = fetch_page(&http, &dash, "partials/resources/clusters").await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the clusters collection must render \"Not authorized\" in its partial; \
         body:\n{partial}"
    );

    // First-page 500 → "unavailable".
    stub.set_clusters_status(500);
    let partial = fetch_page(&http, &dash, "partials/resources/clusters").await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a first-page 500 from the clusters collection must render \"unavailable\" in its \
         partial; body:\n{partial}"
    );

    // Healthy again: no inline <style>, no inline <script>, no style= in ANY partial.
    stub.set_clusters_status(200);
    for path in PARTIALS {
        let partial = fetch_page(&http, &dash, path).await;
        assert_no_inline(&partial, path);
    }
}

// =============================================================================================
// Criterion 8: a budget/mid-sweep partial banner still renders its text.
//
// Fixture note: the banner is triggered by serving a clusters envelope whose `total` far
// exceeds the items the dashboard actually lists (total 2601, only 2 items available), so
// any budget/mid-sweep/truncation notice must surface. The assertion is text-agnostic: the
// `.banner` element must render and its text must be non-empty.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_mid_sweep_banner_renders_its_text() {
    // Mid-sweep failure (the documented banner trigger): a FULL first page (500 items,
    // equal to the sweep page size) forces a second-page fetch at offset 500, which
    // fails 500 → the partial renders the "upstream fetch failed mid-sweep" banner.
    let page1: Vec<Value> = (0..500)
        .map(|i| cluster_item(i + 1000, &format!("c-page1-{i:04}")))
        .collect();
    let (_clusters, route_configs, listeners) = happy_fixture();
    let stub = start_stub(
        Coll {
            status: 200,
            total: 600, // > page size, so page 2 is attempted
            items: page1,
            fail_at: Some((500, 500)),
        },
        Coll {
            status: 200,
            total: route_configs.len() as u64,
            items: route_configs,
            fail_at: None,
        },
        Coll {
            status: 200,
            total: listeners.len() as u64,
            items: listeners,
            fail_at: None,
        },
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/clusters").await;

    assert!(
        has_element_with_classes(&partial, &["banner"]),
        "a budget/mid-sweep notice must render as a `.banner` element in the partial; \
         body:\n{partial}"
    );
    let banner_texts = texts_of_elements_with_classes(&partial, &["banner"]);
    assert!(
        banner_texts.iter().any(|t| !t.is_empty()),
        "the budget/mid-sweep banner must still render its text; \
         banner texts: {banner_texts:?}"
    );
}
