//! fpv2-80z.7 — `flowplane dashboard` AI surface UI restyle — black-box, spec-driven
//! contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The contract under
//! test is the AI shell `GET /<nonce>/ai`, the partials
//! `GET /<nonce>/partials/ai/{overview,traces}`, and the served stylesheet
//! `/<nonce>/assets/dashboard.css`, backed by a stub upstream serving the AI read model
//! (`/api/v1/teams/{team}/ai/{providers,routes,budgets,usage,trace}` plus the route-configs
//! list).
//!
//! Acceptance criteria covered (served-HTML level):
//!   1. overview partial: cards carry `.label` BEFORE `.value` in DOM order; budget mode is
//!      `pill crit` "enforcing" / `pill neutral` "shadow" (verbatim); the budget meter is a
//!      native `<progress class="meter" value="{pct}" max="100">` with NO `style=` attribute
//!      and NO `.meter-fill` element anywhere (the slice's headline AC); the served
//!      stylesheet has a `progress.meter` rule; a ≥80% budget's meter carries the `warn`
//!      class; the near-limit `.banner` + chips are preserved.
//!   2. routes status pills: `pill good` "active" / `pill warn` "stale" / `pill neutral`
//!      fallback; every table is wrapped in `.tablewrap`.
//!   3. traces partial: trace status is DESCRIPTIVE (the request's HTTP status code as
//!      digits) → `pill neutral` with verbatim text, never folded into good/warn/crit
//!      (design descriptive carve-out); the failure signal is the `.trace-failure` marker
//!      plus the per-hop `.hop.failed` class; `<details class="trace-row">` and the
//!      `.hop-timeline` structure are intact; the Load-older hx-get wiring
//!      (`/partials/ai/traces?before=`) is preserved when a cursor page is full.
//!   4. upstream 403 → "Not authorized"; 500 → "unavailable"; no inline <style>, no inline
//!      <script> (every script tag carries src=), and no `style=` attribute anywhere in the
//!      shell or partials — the whole point of the slice.
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
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-ai-token-do-not-leak-41c9";

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
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving the AI read model
// (providers / routes / budgets / usage / trace) plus the route-configs list. Mutable status
// levers on the providers and trace endpoints let one test walk the degradation scenarios
// (200 fixtures, 403, 500) through a single spawned dashboard. Anything else 404s.
// =============================================================================================

struct StubState {
    providers_status: Mutex<u16>,
    trace_status: Mutex<u16>,
    providers: Vec<Value>,
    routes: Vec<Value>,
    budgets: Vec<Value>,
    usage: Vec<Value>,
    route_configs: Vec<Value>,
    traces: Vec<Value>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    fn set_providers_status(&self, status: u16) {
        *self.state.providers_status.lock().unwrap() = status;
    }

    fn set_trace_status(&self, status: u16) {
        *self.state.trace_status.lock().unwrap() = status;
    }
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
    let start = (offset as usize).min(items.len());
    let end = start.saturating_add(limit as usize).min(items.len());
    Json(json!({
        "items": items[start..end].to_vec(),
        "total": items.len(),
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    if path.ends_with("/ai/providers") {
        let status = *state.providers_status.lock().unwrap();
        if status != 200 {
            return canned_error(status);
        }
        paged(&state.providers, &query)
    } else if path.ends_with("/ai/routes") {
        paged(&state.routes, &query)
    } else if path.ends_with("/ai/budgets") {
        paged(&state.budgets, &query)
    } else if path.ends_with("/ai/usage") {
        paged(&state.usage, &query)
    } else if path.ends_with("/ai/trace") {
        let status = *state.trace_status.lock().unwrap();
        if status != 200 {
            return canned_error(status);
        }
        Json(json!({ "traces": state.traces })).into_response()
    } else if path.ends_with("/route-configs") {
        paged(&state.route_configs, &query)
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "no such route" })),
        )
            .into_response()
    }
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
// Canned payload builders (shapes as the real CP returns them; extra fields are harmless,
// the stub only needs what the dashboard reads).
// =============================================================================================

fn provider_item(id: u64, name: &str, kind: &str) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "spec": {
            "kind": kind,
            "base_url": "https://llm.example.invalid/v1",
            "models": ["model-alpha", "model-beta"],
        },
        "revision": 1,
        "created_at": TS,
        "updated_at": TS2,
    })
}

fn backend(provider_id: u64, priority: u64) -> Value {
    json!({
        "provider_id": uid(provider_id),
        "priority": priority,
        "weight": 1,
        "models": [],
    })
}

fn route_item(id: u64, name: &str, status: &str, backends: Vec<Value>) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "status": status,
        "spec": { "backends": backends },
        "materialized": {},
        "revision": 1,
        "created_at": TS,
        "updated_at": TS2,
    })
}

/// `spec.limit_units` deliberately DIVERGES from `state.limit_units`: the rendered pct (and
/// the ≥ 80% warn class) must read the STATE numbers.
fn budget_item(
    id: u64,
    name: &str,
    mode: &str,
    spec_limit: u64,
    used_units: u64,
    state_limit: u64,
) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "spec": {
            "mode": mode,
            "limit_units": spec_limit,
            "window_seconds": 86400,
        },
        "state": {
            "used_units": used_units,
            "window_start": TS,
            "limit_units": state_limit,
            "window_seconds": 86400,
        },
        "revision": 1,
        "created_at": TS,
        "updated_at": TS2,
    })
}

/// A usage row with every column explicit.
fn usage_row(route_cfg: u64, provider: u64, prompt: u64, completion: u64, events: u64) -> Value {
    json!({
        "route_config_id": uid(route_cfg),
        "provider_id": uid(provider),
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "event_count": events,
    })
}

fn route_config_item(id: u64, name: &str) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "revision": 1,
        "created_at": TS,
        "updated_at": TS2,
    })
}

/// RFC3339 with microsecond precision, `mins` minutes in the past (negative = future).
fn ts_minutes_ago(mins: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(mins))
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn hop_entry(name: &str, outcome: &str, failed: bool) -> Value {
    json!({
        "hop": name,
        "started_at": TS,
        "ended_at": TS2,
        "outcome": outcome,
        "origin": "gateway",
        "failed": failed,
        "detail": null,
    })
}

#[allow(clippy::too_many_arguments)]
fn trace_item(
    id: u64,
    request_id: &str,
    trace_id: Option<&str>,
    model: Option<&str>,
    status_code: Option<i64>,
    failure_hop: Option<&str>,
    hops: Vec<Value>,
    created_at: &str,
) -> Value {
    json!({
        "id": uid(id),
        "request_id": request_id,
        "trace_id": trace_id,
        "route_config_id": uid(300),
        "listener_id": null,
        "provider_id": uid(200),
        "model": model,
        "status_code": status_code,
        "failure_hop": failure_hop,
        "hops": hops,
        "created_at": created_at,
        "expires_at": ts_minutes_ago(-7 * 24 * 60),
    })
}

// =============================================================================================
// The happy-path fixture shared by most tests:
//   * providers prov-alpha (openai) + prov-beta (openai-compatible)
//   * routes: route-active (status "active" → pill good), route-stale ("stale" → pill warn),
//     route-pending ("pending" → pill neutral fallback)
//   * budgets: budget-enforce (mode enforcing, 90/100 → 90% ≥ 80% → meter warn + near-limit
//     banner), budget-shadow (mode shadow, 40/100 → 40% → plain meter)
//   * one usage row against route-config rc-alpha / provider prov-alpha (usage table + the
//     route-configs mapping fetch)
//   * traces: none (the traces partial renders its empty state; the traces test builds its
//     own 50-row fixture)
// =============================================================================================
const PROV_ALPHA: &str = "prov-alpha-uistyle";
const PROV_BETA: &str = "prov-beta-uistyle";
const ROUTE_ACTIVE: &str = "route-active-uistyle";
const ROUTE_STALE: &str = "route-stale-uistyle";
const ROUTE_PENDING: &str = "route-pending-uistyle";
const BUDGET_ENFORCE: &str = "budget-enforce-uistyle";
const BUDGET_SHADOW: &str = "budget-shadow-uistyle";

async fn start_happy_stub() -> StubUpstream {
    start_stub(StubState {
        providers_status: Mutex::new(200),
        trace_status: Mutex::new(200),
        providers: vec![
            provider_item(200, PROV_ALPHA, "openai"),
            provider_item(201, PROV_BETA, "openai-compatible"),
        ],
        routes: vec![
            route_item(100, ROUTE_ACTIVE, "active", vec![backend(200, 0)]),
            route_item(101, ROUTE_STALE, "stale", vec![backend(201, 0)]),
            route_item(102, ROUTE_PENDING, "pending", vec![]),
        ],
        budgets: vec![
            budget_item(400, BUDGET_ENFORCE, "enforcing", 999, 90, 100),
            budget_item(401, BUDGET_SHADOW, "shadow", 999, 40, 100),
        ],
        usage: vec![usage_row(300, 200, 10, 20, 3)],
        route_configs: vec![route_config_item(300, "rc-alpha-uistyle")],
        traces: vec![],
    })
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

/// The rule bodies of every CSS rule whose selector list includes `token` as a whole
/// selector part (e.g. `progress.meter`, `progress.meter.warn`).
fn css_rule_bodies<'a>(css: &'a str, token: &str) -> Vec<&'a str> {
    let mut bodies = Vec::new();
    for block in css.split('}') {
        let Some((selector, body)) = block.split_once('{') else {
            continue;
        };
        let matches = selector.split([',', ' ', '\n', '\t', '>']).any(|part| {
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

/// Assert the inline-forbidden rules shared by every partial (criterion 4 tail):
/// no inline `<style>` element, every `<script>` carries src=, no inline `style=`.
fn assert_no_inline(partial: &str, which: &str) {
    let lower = partial.to_lowercase();
    assert!(
        !lower.contains("<style"),
        "the {which} page must not contain an inline <style> element; body:\n{partial}"
    );
    for tag in all_opening_tags(&lower, "<script") {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden in the {which} page; every script tag \
             must carry src=; offending tag: {tag:?}"
        );
    }
    assert!(
        !lower.contains(" style="),
        "the {which} page must not contain any inline style= attribute; body:\n{partial}"
    );
}

/// The `<tr>...</tr>` region containing `needle` (row-scoped assertions). Scans table rows
/// rather than the first textual occurrence: a value (e.g. a budget name) can also appear
/// outside the table, such as in the near-limit banner chips, which precede the table.
fn row_of<'a>(partial: &'a str, needle: &str) -> &'a str {
    let mut base = 0usize;
    while let Some(rel) = partial[base..].find("<tr") {
        let tr_start = base + rel;
        let close_rel = partial[tr_start..].find("</tr>").unwrap_or_else(|| {
            panic!("a <tr> starting at byte {tr_start} must be closed; body:\n{partial}")
        });
        let tr_end = tr_start + close_rel + "</tr>".len();
        let row = &partial[tr_start..tr_end];
        if row.contains(needle) {
            return row;
        }
        base = tr_end;
    }
    panic!("no <tr> row contains {needle:?}; body:\n{partial}")
}

/// The value of the first `attr="..."` in `tag`.
fn attr_value<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let pat = format!("{attr}=\"");
    let start = tag.find(&pat)? + pat.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Assert every `<table` in `partial` is wrapped: a `.tablewrap` element must appear between
/// the previous table (or the start of the partial) and this table. Returns the table count.
fn assert_all_tables_tablewrap(partial: &str, which: &str) -> usize {
    let wrap_positions = class_positions(partial, "tablewrap");
    let mut prev_boundary = 0usize;
    let mut count = 0usize;
    let mut rest = partial;
    let mut base = 0usize;
    while let Some(rel) = rest.find("<table") {
        let table_at = base + rel;
        assert!(
            wrap_positions
                .iter()
                .any(|&w| prev_boundary <= w && w < table_at),
            "every <table> in the {which} partial must be wrapped in `.tablewrap`; \
             table at byte {table_at} has no preceding .tablewrap; body:\n{partial}"
        );
        count += 1;
        prev_boundary = table_at;
        base = table_at + "<table".len();
        rest = &partial[base..];
    }
    count
}

// =============================================================================================
// Criterion 1: overview partial — cards carry `.label` BEFORE `.value` in DOM order; budget
// mode is `pill crit` "enforcing" / `pill neutral` "shadow" (verbatim); the meter is a
// native `<progress class="meter" value="{pct}" max="100">` with NO style= attribute and NO
// `.meter-fill` element anywhere (the slice's headline AC); the served stylesheet has a
// `progress.meter` rule; the ≥80% budget's meter carries `warn`; the near-limit `.banner` +
// chips are preserved.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_cards_label_before_value_and_native_progress_meter() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/ai/overview").await;

    // Cards: within every `.card`, the `.label` element precedes the `.value` element.
    let card_positions = class_positions(&partial, "card");
    assert!(
        !card_positions.is_empty(),
        "the overview partial must render `.card` elements; body:\n{partial}"
    );
    for (i, &start) in card_positions.iter().enumerate() {
        let end = card_positions.get(i + 1).copied().unwrap_or(partial.len());
        let card = &partial[start..end];
        if !has_element_with_classes(card, &["value"]) {
            continue; // not a stat card (e.g. a wrapper carrying the class)
        }
        let label_at = class_positions(card, "label")
            .first()
            .copied()
            .unwrap_or_else(|| panic!("a stat card must carry a `.label` element; card:\n{card}"));
        let value_at = class_positions(card, "value")
            .first()
            .copied()
            .expect("checked above");
        assert!(
            label_at < value_at,
            "within a stat card the `.label` element must precede the `.value` element \
             in DOM order; card:\n{card}"
        );
    }

    // Budget rows: mode pills verbatim + the native progress meter.
    let enforce_row = row_of(&partial, BUDGET_ENFORCE);
    let crit_texts = texts_of_elements_with_classes(enforce_row, &["pill", "crit"]);
    assert!(
        crit_texts.iter().any(|t| t == "enforcing"),
        "an enforcing budget row must show `<span class=\"pill crit\">enforcing</span>`; \
         found pill-crit texts: {crit_texts:?}; row:\n{enforce_row}"
    );
    let shadow_row = row_of(&partial, BUDGET_SHADOW);
    let neutral_texts = texts_of_elements_with_classes(shadow_row, &["pill", "neutral"]);
    assert!(
        neutral_texts.iter().any(|t| t == "shadow"),
        "a shadow budget row must show `<span class=\"pill neutral\">shadow</span>`; \
         found pill-neutral texts: {neutral_texts:?}; row:\n{shadow_row}"
    );

    // HEADLINE AC: the budget meter is a native <progress class="meter" value="{pct}"
    // max="100"> with NO style= attribute and NO .meter-fill element anywhere in the partial.
    let progress_tags = all_opening_tags(&partial, "<progress");
    assert!(
        !progress_tags.is_empty(),
        "the budget meter must be a native <progress> element; body:\n{partial}"
    );
    for tag in &progress_tags {
        assert!(
            tag_has_class(tag, "meter"),
            "every budget meter <progress> must carry class=\"meter\"; offending tag: {tag:?}"
        );
        assert!(
            tag.contains("max=\"100\""),
            "every budget meter <progress> must carry max=\"100\"; offending tag: {tag:?}"
        );
        assert!(
            attr_value(tag, "value").is_some(),
            "every budget meter <progress> must carry a value= attribute; \
             offending tag: {tag:?}"
        );
        assert!(
            !tag.contains("style="),
            "the budget meter <progress> must not carry a style= attribute (inline styles \
             are forbidden); offending tag: {tag:?}"
        );
    }
    assert!(
        !partial.contains("meter-fill"),
        "the legacy `.meter-fill` div must be GONE from the overview partial (the meter is \
         a native <progress> now); body:\n{partial}"
    );

    // The enforcing budget's row: meter value ≈ 90 (state numbers, not the divergent spec
    // limit) and the warn class; the shadow budget's row: value ≈ 40 and NO warn class.
    let enforce_progress = all_opening_tags(enforce_row, "<progress")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("the enforcing budget row must render a <progress> meter; row:\n{enforce_row}")
        });
    let pct: f64 = attr_value(enforce_progress, "value")
        .expect("checked above")
        .parse()
        .unwrap_or_else(|_| panic!("the meter value= must be numeric; tag: {enforce_progress:?}"));
    assert!(
        (pct - 90.0).abs() < 0.51,
        "the enforcing budget meter must read ≈90 (90/100 from state); tag: {enforce_progress:?}"
    );
    assert!(
        tag_has_class(enforce_progress, "warn"),
        "a ≥80% budget's meter must carry the `warn` class (`progress class=\"meter warn\"`); \
         tag: {enforce_progress:?}"
    );
    let shadow_progress = all_opening_tags(shadow_row, "<progress")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("the shadow budget row must render a <progress> meter; row:\n{shadow_row}")
        });
    let pct: f64 = attr_value(shadow_progress, "value")
        .expect("checked above")
        .parse()
        .unwrap_or_else(|_| panic!("the meter value= must be numeric; tag: {shadow_progress:?}"));
    assert!(
        (pct - 40.0).abs() < 0.51,
        "the shadow budget meter must read ≈40 (40/100 from state); tag: {shadow_progress:?}"
    );
    assert!(
        !tag_has_class(shadow_progress, "warn"),
        "a sub-80% budget's meter must not carry the `warn` class; tag: {shadow_progress:?}"
    );

    // The near-limit banner + chips are preserved, naming the ≥80% budget.
    assert!(
        has_element_with_classes(&partial, &["banner"]),
        "the near-limit banner must render as a `.banner` element; body:\n{partial}"
    );
    let banner_texts = texts_of_elements_with_classes(&partial, &["banner"]);
    assert!(
        banner_texts.iter().any(|t| t.contains(BUDGET_ENFORCE)),
        "the near-limit banner must name the ≥80% budget {BUDGET_ENFORCE:?}; \
         banner texts: {banner_texts:?}"
    );
    assert!(
        has_element_with_classes(&partial, &["chip"]),
        "the near-limit banner must keep its chips; body:\n{partial}"
    );

    // The served stylesheet has a `progress.meter` rule.
    let css = fetch_stylesheet(&http, &dash).await;
    let meter_bodies = css_rule_bodies(&css, "progress.meter");
    assert!(
        !meter_bodies.is_empty(),
        "the stylesheet must contain a `progress.meter` rule (native meter styling); \
         css:\n{css}"
    );
}

// =============================================================================================
// Criterion 2: routes status pills per the map (`pill good` "active" / `pill warn` "stale" /
// `pill neutral` fallback) and every table in the overview partial wrapped in `.tablewrap`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_route_status_pills_and_all_tables_tablewrap() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/ai/overview").await;

    // Route rows: status pill per the documented map, verbatim text.
    let active_row = row_of(&partial, ROUTE_ACTIVE);
    let good_texts = texts_of_elements_with_classes(active_row, &["pill", "good"]);
    assert!(
        good_texts.iter().any(|t| t == "active"),
        "an active route row must show `<span class=\"pill good\">active</span>`; \
         found pill-good texts: {good_texts:?}; row:\n{active_row}"
    );
    let stale_row = row_of(&partial, ROUTE_STALE);
    let warn_texts = texts_of_elements_with_classes(stale_row, &["pill", "warn"]);
    assert!(
        warn_texts.iter().any(|t| t == "stale"),
        "a stale route row must show `<span class=\"pill warn\">stale</span>`; \
         found pill-warn texts: {warn_texts:?}; row:\n{stale_row}"
    );
    let pending_row = row_of(&partial, ROUTE_PENDING);
    let neutral_texts = texts_of_elements_with_classes(pending_row, &["pill", "neutral"]);
    assert!(
        neutral_texts.iter().any(|t| !t.is_empty()),
        "an unrecognized route status must fall back to `<span class=\"pill neutral\">…</span>`; \
         found pill-neutral texts: {neutral_texts:?}; row:\n{pending_row}"
    );

    // Every table is wrapped in `.tablewrap`; the migrated overview renders its four
    // collections (providers / routes / budgets / usage) as tables.
    let table_count = assert_all_tables_tablewrap(&partial, "overview");
    assert_eq!(
        table_count, 4,
        "the overview partial must render its four collections as tables (providers, \
         routes, budgets, usage); found {table_count} <table> elements; body:\n{partial}"
    );

    // The migrated panels' h2s carry `.count` spans.
    assert!(
        has_element_with_classes(&partial, &["count"]),
        "the overview partial's panel h2s must carry `<span class=\"count\">`; \
         body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 3: traces partial — trace status is descriptive (the request's HTTP status code)
// and renders `pill neutral` verbatim, never folded into good/warn/crit; the failure signal is
// the `.trace-failure` marker + per-hop `.hop.failed`; the `<details class="trace-row">` /
// `.hop-timeline` drill-down structure is intact, and the Load-older hx-get wiring
// (`/partials/ai/traces?before=`) is preserved on a full cursor page.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn traces_partial_status_pills_failure_marker_structure_and_load_older() {
    // A full cursor page (exactly 50 rows) so the Load-older control renders. Row 0 is a
    // failed request (status 429, failure_hop budget, a failed hop); the rest are healthy.
    let mut traces = vec![trace_item(
        1,
        "req-failed-uistyle",
        Some("trace-failed"),
        Some("model-alpha"),
        Some(429),
        Some("budget"),
        vec![
            hop_entry("auth", "ok", false),
            hop_entry("budget", "rejected", true),
        ],
        &ts_minutes_ago(1),
    )];
    for i in 2..=50u64 {
        traces.push(trace_item(
            i,
            &format!("req-ok-{i:03}"),
            Some("trace-ok"),
            Some("model-beta"),
            Some(200),
            None,
            vec![hop_entry("auth", "ok", false)],
            &ts_minutes_ago(i64::try_from(i).unwrap()),
        ));
    }

    let stub = start_stub(StubState {
        providers_status: Mutex::new(200),
        trace_status: Mutex::new(200),
        providers: vec![provider_item(200, PROV_ALPHA, "openai")],
        routes: vec![route_item(
            100,
            ROUTE_ACTIVE,
            "active",
            vec![backend(200, 0)],
        )],
        budgets: vec![],
        usage: vec![],
        route_configs: vec![route_config_item(300, "rc-alpha-uistyle")],
        traces,
    })
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/ai/traces").await;

    // Trace status is DESCRIPTIVE — the request's HTTP status code rendered as digits
    // (200, 429), not a fixed ok/failed enum — so per the design's descriptive carve-out
    // it stays `pill neutral` with its verbatim text and is NEVER folded into the
    // good/warn/crit status map. The failure signal is `.trace-failure` + `.hop.failed`.
    let neutral_texts = texts_of_elements_with_classes(&partial, &["pill", "neutral"]);
    assert!(
        neutral_texts.iter().any(|t| t == "200"),
        "healthy trace rows must render their HTTP status code (200) as a `pill neutral` \
         (descriptive carve-out); found neutral pill texts: {neutral_texts:?}; body:\n{partial}"
    );
    assert!(
        neutral_texts.iter().any(|t| t == "429"),
        "the failed trace row must render its HTTP status code (429) as a `pill neutral` — \
         the failure signal is `.trace-failure`/`.hop.failed`, not a crit status pill; \
         found neutral pill texts: {neutral_texts:?}; body:\n{partial}"
    );
    // The descriptive carve-out forbids folding trace status into good/warn/crit pills.
    assert!(
        texts_of_elements_with_classes(&partial, &["pill", "good"]).is_empty(),
        "trace status is descriptive and must NOT render any `pill good` (design carve-out); \
         body:\n{partial}"
    );
    assert!(
        texts_of_elements_with_classes(&partial, &["pill", "crit"]).is_empty(),
        "trace status is descriptive and must NOT render any `pill crit`; the failed row's \
         signal is `.trace-failure`/`.hop.failed`; body:\n{partial}"
    );
    // The failing hop carries the `.hop.failed` class (per-hop boolean render path).
    assert!(
        has_element_with_classes(&partial, &["hop", "failed"]),
        "the failing hop must add the `.hop.failed` class; body:\n{partial}"
    );

    // The `.trace-failure` marker is preserved on the failed row.
    assert!(
        has_element_with_classes(&partial, &["trace-failure"]),
        "a row with failure_hop set must keep the `.trace-failure` marker; body:\n{partial}"
    );

    // The details/summary drill-down structure is intact: `<details class="trace-row">`
    // rows with the `.hop-timeline` inside.
    let trace_row_details: Vec<&str> = all_opening_tags(&partial, "<details")
        .into_iter()
        .filter(|tag| tag_has_class(tag, "trace-row"))
        .collect();
    assert!(
        !trace_row_details.is_empty(),
        "the traces partial must render `<details class=\"trace-row\">` drill-down rows; \
         body:\n{partial}"
    );
    assert!(
        has_element_with_classes(&partial, &["hop-timeline"]),
        "the traces drill-down must keep the `.hop-timeline` structure; body:\n{partial}"
    );

    // A full 50-row page keeps the Load-older control whose hx-get targets
    // `/partials/ai/traces?before=<cursor>`.
    let load_older: Vec<&str> = all_opening_tags(&partial, "<button")
        .into_iter()
        .chain(all_opening_tags(&partial, "<a"))
        .filter(|tag| tag.contains("hx-get") && tag.contains("partials/ai/traces?before="))
        .collect();
    assert!(
        !load_older.is_empty(),
        "a full 50-row traces page must render the Load-older control with hx-get to \
         `/partials/ai/traces?before=<cursor>`; body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 4: upstream 403 → "Not authorized"; 500 → "unavailable"; and NO inline
// <style>/<script>/style= anywhere in the AI shell or partials — the whole point of the
// slice.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn degradation_403_not_authorized_500_unavailable_and_no_inline_anywhere() {
    let stub = start_happy_stub().await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // 403 on the providers collection → the overview partial says "Not authorized".
    stub.set_providers_status(403);
    let partial = fetch_page(&http, &dash, "partials/ai/overview").await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the providers collection must render \"Not authorized\" in the \
         overview partial; body:\n{partial}"
    );

    // First-page 500 → "unavailable".
    stub.set_providers_status(500);
    let partial = fetch_page(&http, &dash, "partials/ai/overview").await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a 500 from the providers collection must render \"unavailable\" in the overview \
         partial; body:\n{partial}"
    );
    stub.set_providers_status(200);

    // 403 / 500 on the trace endpoint → the traces partial degrades the same way.
    stub.set_trace_status(403);
    let partial = fetch_page(&http, &dash, "partials/ai/traces").await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the trace endpoint must render \"Not authorized\" in the traces \
         partial; body:\n{partial}"
    );
    stub.set_trace_status(500);
    let partial = fetch_page(&http, &dash, "partials/ai/traces").await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a 500 from the trace endpoint must render \"unavailable\" in the traces partial; \
         body:\n{partial}"
    );
    stub.set_trace_status(200);

    // Healthy again: no inline <style>, no inline <script>, no style= in the shell or ANY
    // partial — the whole point of the slice.
    for path in ["ai", "partials/ai/overview", "partials/ai/traces"] {
        let page = fetch_page(&http, &dash, path).await;
        assert_no_inline(&page, path);
    }
}
