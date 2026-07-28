//! fpv2-80z.5 — `flowplane dashboard` APIs tab UI restyle — black-box, spec-driven
//! contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix. The
//! contract under test is the APIs list partial `GET /<nonce>/partials/apis/list`, the
//! per-row detail partial `GET /<nonce>/partials/apis/detail?api=<name>`, backed by a stub
//! upstream serving the API-lifecycle read model
//! (`/api/v1/teams/{team}/api-definitions…`, plus the route-configs/listeners join sweeps).
//!
//! Acceptance criteria covered (served-HTML level):
//!   1. APIs shell page `GET /<nonce>/apis`: the nav is `<nav class="tabs">` linking all 7
//!      tabs as real `<a>` anchors with the APIs anchor carrying `class="active"` (a real
//!      link, not an ARIA-tab widget); the topbar `<header class="topbar">` renders a
//!      `.brand` element and a `.ctx` chip row with a team-name chip and a `read-only` chip;
//!      no inline `<style>`, no src-less `<script>`, no inline `style=` in the shell.
//!   2. List partial: `.panel` h2 carries `<span class="count">` "N of M"; the table is
//!      CONTAINED in `.tablewrap` (the wrapper opens before `<table>` and its `</div>`
//!      closes after `</table>`); NO `class="dataplanes"` anywhere; the per-row
//!      `<details hx-get="/<nonce>/partials/apis/detail?api=<name>" hx-trigger="toggle
//!      once">` lazy wiring is preserved verbatim.
//!   3. Detail partial: the composite API-lifecycle pill `d.pill` in the `<h3>` is `pill
//!      neutral`. Review decisions are DESCRIPTIVE, NOT classified good/crit — every
//!      decision renders `<span class="pill neutral">{decision}</span>` verbatim, both in
//!      the pipeline events (`<ol class="pipeline">`) and in the Spec-lineage table's
//!      decision column. A single shared decision value is seeded into BOTH the pipeline
//!      events AND a lineage row, and each region is asserted independently (no `pill
//!      good`/`pill crit` in either), so a regression on either surface is caught on its
//!      own. A disabled tool renders `pill warn` "disabled", an enabled tool `pill neutral`
//!      "yes"; all 3 tables (lineage, chain, tools) are CONTAINED in `.tablewrap`; the
//!      legacy `decision` / `decision-*` class pattern is GONE.
//!   4. Upstream 403 on the list → the partial says "Not authorized"; 500 → "unavailable";
//!      no inline `<style>`, no src-less `<script>`, no `style=` attribute in any partial.
//!
//! Parallel-safety: every test spawns its own stub upstream and dashboard child on ephemeral
//! ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique team/API names; nothing
//! binds a fixed port. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
// The harness is copied verbatim from cli_dashboard_uistyle_resources.rs; a few helpers
// (stylesheet/CSS helpers, row_of) are unused by this slice's criteria.
#![allow(dead_code)]

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
const SECRET_TOKEN: &str = "sekret-uistyle-apis-token-do-not-leak-41c9";

const TS: &str = "2026-01-01T00:00:00Z";
const TS2: &str = "2026-01-02T03:04:05Z";

/// The 7 tabs the restyled shell nav must name, in order (shared with the shell contract).
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

/// Deterministic id with no accidental digit collisions.
fn uid(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

fn hex_hash(v: u64) -> String {
    format!("{v:064x}")
}

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving the API-lifecycle
// read-model endpoints. The api-definitions LIST sits behind a mutable Coll so one test can
// walk several upstream scenarios (200 fixtures, 403, 500) through a single spawned
// dashboard. Detail sub-resources (definition/specs/events/route-bindings/tools) and the
// route-configs/listeners join sweeps are static fixtures. Anything else 404s.
// =============================================================================================

struct Coll {
    status: u16,
    items: Vec<Value>,
    total: u64,
    /// Inject a failure at exactly this paging offset: `(offset, status)`.
    fail_at: Option<(u64, u16)>,
}

/// One API definition's canned upstream sub-resources (shapes as the real CP returns them).
struct ApiFixture {
    name: String,
    /// Enriched item as it appears in the api-definitions LIST envelope.
    list_item: Value,
    /// Body of `GET .../api-definitions/{name}` (no enrichment fields).
    definition: Value,
    /// Items of `GET .../api-definitions/{name}/specs` (newest first, as the CP returns).
    specs: Vec<Value>,
    /// Items of `GET .../api-definitions/{name}/specs/{any}/events`.
    events: Vec<Value>,
    /// Items of `GET .../api-definitions/{name}/route-bindings`.
    bindings: Vec<Value>,
    /// Items of `GET .../api-definitions/{name}/tools`.
    tools: Vec<Value>,
}

struct StubState {
    team: String,
    /// The api-definitions LIST collection (mutable status for degradation walks).
    list: Mutex<Coll>,
    apis: Vec<ApiFixture>,
    route_configs: Vec<Value>,
    listeners: Vec<Value>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    fn set_list_status(&self, status: u16) {
        self.state.list.lock().unwrap().status = status;
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

/// Slice `items` per limit/offset into the uniform `{items, total, limit, offset}` envelope.
fn paged(items: &[Value], total: u64, limit: u64, offset: u64) -> Response {
    let len = items.len() as u64;
    let start = offset.min(len);
    let end = offset.saturating_add(limit).min(len);
    Json(json!({
        "items": items[start as usize..end as usize].to_vec(),
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

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

    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();
    let api = |name: &str| state.apis.iter().find(|a| a.name == name);

    match segs.as_slice() {
        ["api-definitions"] => {
            let coll = state.list.lock().unwrap();
            if coll.status != 200 {
                return canned_error(coll.status);
            }
            if let Some((fail_offset, fail_status)) = coll.fail_at {
                if offset == fail_offset {
                    return canned_error(fail_status);
                }
            }
            paged(&coll.items, coll.total, limit, offset)
        }
        ["api-definitions", name] => match api(name) {
            Some(a) => Json(a.definition.clone()).into_response(),
            None => canned_error(404),
        },
        ["api-definitions", name, "specs"] => match api(name) {
            Some(a) => paged(&a.specs, a.specs.len() as u64, limit, offset),
            None => canned_error(404),
        },
        ["api-definitions", name, "specs", _v, "events"] => match api(name) {
            Some(a) => paged(&a.events, a.events.len() as u64, limit, offset),
            None => canned_error(404),
        },
        ["api-definitions", name, "route-bindings"] => match api(name) {
            Some(a) => paged(&a.bindings, a.bindings.len() as u64, limit, offset),
            None => canned_error(404),
        },
        ["api-definitions", name, "tools"] => match api(name) {
            Some(a) => paged(&a.tools, a.tools.len() as u64, limit, offset),
            None => canned_error(404),
        },
        ["route-configs"] => paged(
            &state.route_configs,
            state.route_configs.len() as u64,
            limit,
            offset,
        ),
        ["listeners"] => paged(
            &state.listeners,
            state.listeners.len() as u64,
            limit,
            offset,
        ),
        _ => canned_error(404),
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
// Canned payload builders (shapes as the real CP returns them).
// =============================================================================================

#[allow(clippy::too_many_arguments)] // canned-payload builder mirroring the wire shape
fn list_item(
    id: u64,
    name: &str,
    display_name: &str,
    published_spec_version_id: Value,
    tool_count: u64,
    route_binding_count: u64,
    latest_version: Value,
    published_version: Value,
) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "display_name": display_name,
        "description": "",
        "published_spec_version_id": published_spec_version_id,
        "revision": 1,
        "tool_count": tool_count,
        "enabled_tool_count": 0,
        "route_binding_count": route_binding_count,
        "latest_version": latest_version,
        "published_version": published_version,
        // Required-but-nullable on the CLI↔CP wire boundary.
        "latest_decision": null,
        "created_at": TS,
        "updated_at": TS2,
    })
}

#[allow(clippy::too_many_arguments)] // explicit lifecycle fixture facts are easier to audit
fn lifecycle_list_item(
    id: u64,
    name: &str,
    display_name: &str,
    description: &str,
    revision: u64,
    tool_count: u64,
    enabled_tool_count: u64,
    route_binding_count: u64,
    latest_version: Option<u64>,
    published_version: Option<u64>,
    latest_decision: Option<&str>,
) -> Value {
    let mut item = list_item(
        id,
        name,
        display_name,
        Value::Null,
        tool_count,
        route_binding_count,
        latest_version.map_or(Value::Null, |v| json!(v)),
        published_version.map_or(Value::Null, |v| json!(v)),
    );
    let obj = item.as_object_mut().expect("list item object");
    obj.insert("description".into(), json!(description));
    obj.insert("revision".into(), json!(revision));
    obj.insert("enabled_tool_count".into(), json!(enabled_tool_count));
    obj.insert(
        "latest_decision".into(),
        latest_decision.map_or(Value::Null, |decision| json!(decision)),
    );
    item
}

/// The definition GET body: the list item minus the enrichment fields
/// (published_spec_version_id stays present).
fn definition_from(list_item: &Value) -> Value {
    let mut def = list_item.clone();
    let obj = def.as_object_mut().expect("list item is an object");
    obj.remove("tool_count");
    obj.remove("route_binding_count");
    obj.remove("latest_version");
    obj.remove("published_version");
    def
}

fn spec_item(id: u64, version: u64, source_kind: &str, latest_decision: Option<&str>) -> Value {
    let mut v = json!({
        "id": uid(id),
        "version": version,
        "source_kind": source_kind,
        "format": "openapi3",
        "spec_hash": hex_hash(version),
        "created_at": TS,
    });
    if let Some(d) = latest_decision {
        v.as_object_mut()
            .unwrap()
            .insert("latest_decision".into(), json!(d));
    }
    v
}

fn event_item(id: u64, decision: &str, reason: &str, created_at: &str) -> Value {
    json!({
        "id": uid(id),
        "decision": decision,
        "actor_type": "user",
        "actor_id": uid(id + 5000),
        "reason": reason,
        "metadata": {},
        "created_at": created_at,
    })
}

fn binding_item(
    id: u64,
    name: &str,
    api_definition_id: u64,
    route_config_id: u64,
    listener_id: u64,
) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "api_definition_id": uid(api_definition_id),
        "route_config_id": uid(route_config_id),
        "listener_id": uid(listener_id),
        "virtual_host": "vh-1",
        "route": "route-1",
        "created_at": TS,
    })
}

fn tool_item(
    id: u64,
    name: &str,
    api_definition_id: u64,
    spec_version_id: u64,
    enabled: bool,
) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "api_definition_id": uid(api_definition_id),
        "spec_version_id": uid(spec_version_id),
        "operation_id": format!("op-{name}"),
        "method": "GET",
        "path": "/x",
        "input_schema": {},
        "output_schema": {},
        "enabled": enabled,
        "created_at": TS,
        "updated_at": TS2,
    })
}

fn infra_item(id: u64, name: &str) -> Value {
    json!({
        "id": uid(id),
        "name": name,
        "spec": { "placeholder": true },
        "revision": 1,
        "created_at": TS,
        "updated_at": TS2,
    })
}

// =============================================================================================
// Happy fixture: API-X (published v2, latest v3 rejected, one bound route, one enabled + one
// disabled tool) plus a trivially-published API-Y so the list renders two rows.
//
// Review decisions are DESCRIPTIVE (all `pill neutral`, verbatim). A single distinctive
// `decision_shared` value is seeded into BOTH a pipeline event AND a Spec-lineage row so the
// two surfaces can be asserted independently. The pipeline events also carry the ordinary
// "published" / "rejected" decisions (verbatim, still `pill neutral`) to prove the mapping is
// descriptive rather than classified.
// =============================================================================================

struct HappyFixture {
    team: String,
    api_x: String,
    api_y: String,
    /// A distinctive decision string seeded into BOTH a pipeline event and a lineage row;
    /// each region must render it verbatim inside a `pill neutral` (never good/crit).
    decision_shared: String,
}

fn happy_fixture() -> (StubState, HappyFixture) {
    let team = unique("team");
    let api_x = unique("api-x");
    let api_y = unique("api-y");
    let decision_shared = unique("reviewed");

    // API-X: v2 published (uid(102)), v3 latest rejected, v1 imported with no decision.
    // v2's lineage decision is the shared value so it appears in the lineage table; the
    // pipeline event #503 carries the same shared value so it appears in the pipeline too.
    let x_list = list_item(
        100,
        &api_x,
        "API X Display",
        json!(uid(102)),
        2,
        1,
        json!(3),
        json!(2),
    );
    let x = ApiFixture {
        name: api_x.clone(),
        definition: definition_from(&x_list),
        list_item: x_list,
        specs: vec![
            spec_item(103, 3, "learned", Some("rejected")),
            spec_item(102, 2, "learned", Some(decision_shared.as_str())),
            spec_item(101, 1, "imported", None),
        ],
        events: vec![
            event_item(501, "published", "looks-good-ship-it", TS),
            event_item(502, "rejected", "spec-violates-policy", TS2),
            event_item(503, &decision_shared, "needs-security-signoff", TS2),
        ],
        bindings: vec![binding_item(401, "bind-main", 100, 201, 301)],
        tools: vec![
            tool_item(601, "tool-on", 100, 103, true),
            tool_item(602, "tool-off", 100, 103, false),
        ],
    };

    // API-Y: published == latest == v1 → "published v1".
    let y_list = list_item(
        110,
        &api_y,
        "API Y Display",
        json!(uid(112)),
        0,
        0,
        json!(1),
        json!(1),
    );
    let y = ApiFixture {
        name: api_y.clone(),
        definition: definition_from(&y_list),
        list_item: y_list.clone(),
        specs: vec![spec_item(112, 1, "imported", Some("published"))],
        events: Vec::new(),
        bindings: Vec::new(),
        tools: Vec::new(),
    };

    let list_items = vec![x.list_item.clone(), y_list];
    let state = StubState {
        team: team.clone(),
        list: Mutex::new(Coll {
            status: 200,
            total: list_items.len() as u64,
            items: list_items,
            fail_at: None,
        }),
        apis: vec![x, y],
        route_configs: vec![infra_item(201, "rc-main"), infra_item(202, "rc-other")],
        listeners: vec![
            infra_item(301, "listener-main"),
            infra_item(302, "listener-other"),
        ],
    };

    (
        state,
        HappyFixture {
            team,
            api_x,
            api_y,
            decision_shared,
        },
    )
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

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn assert_no_inline_event_attributes(html: &str, which: &str) {
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start..=start + end];
        if !tag.starts_with("</") && !tag.starts_with("<!--") {
            for token in tag.split_ascii_whitespace().skip(1) {
                let Some((attribute_name, _)) = token.split_once('=') else {
                    continue;
                };
                let attribute_name = attribute_name.trim_end_matches('>').to_ascii_lowercase();
                assert!(
                    !attribute_name.starts_with("on"),
                    "the {which} must not use inline event handlers; offending tag: {tag:?}"
                );
            }
        }
        rest = &rest[start + end + 1..];
    }
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
/// selector part (e.g. `.cchip`, `.cchip.hl`).
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

/// Assert the inline-forbidden rules shared by every partial (criterion 3 tail):
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

/// Byte offset just past the `</div>` that matches the `<div ...>` opening at `div_open`,
/// depth-counted over nested `<div>`s. `None` if the div is never closed.
fn matching_div_close(html: &str, div_open: usize) -> Option<usize> {
    let after_open = html[div_open..].find('>')? + div_open + 1;
    let mut depth = 1usize;
    let mut pos = after_open;
    let close = "</div>";
    loop {
        let next_open = html[pos..].find("<div").map(|x| pos + x);
        let next_close = html[pos..].find(close).map(|x| pos + x);
        match (next_open, next_close) {
            (_, None) => return None,
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                pos = o + "<div".len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return Some(c + close.len());
                }
                pos = c + close.len();
            }
        }
    }
}

/// The `<div class="tablewrap" ...>` opening positions (byte offset of the `<`), in order.
fn tablewrap_div_opens(html: &str) -> Vec<usize> {
    class_positions(html, "tablewrap")
        .into_iter()
        .map(|class_at| {
            let lt = html[..class_at].rfind('<').unwrap_or_else(|| {
                panic!("a `tablewrap` class must sit on an element; body:\n{html}")
            });
            assert!(
                html[lt..].starts_with("<div"),
                "a `.tablewrap` wrapper must be a <div> so its `</div>` can prove containment; \
                 got tag starting: {:?}",
                &html[lt..(lt + 12).min(html.len())]
            );
            lt
        })
        .collect()
}

/// Every `<table>` in `html` must be truly CONTAINED in a `.tablewrap` <div>: for each table
/// a tablewrap div must open after the previous table AND before this `<table>`, and its
/// matching `</div>` must close AFTER this table's `</table>`. Asserts the exact table count.
fn assert_tables_wrapped(html: &str, which: &str, expected_tables: usize) {
    let tables: Vec<usize> = html.match_indices("<table").map(|(i, _)| i).collect();
    assert_eq!(
        tables.len(),
        expected_tables,
        "the {which} partial must render exactly {expected_tables} <table> element(s); \
         found {}; body:\n{html}",
        tables.len()
    );
    let wrap_opens = tablewrap_div_opens(html);
    let mut prev_table: Option<usize> = None;
    for &table_at in &tables {
        let table_close = html[table_at..]
            .find("</table>")
            .map(|e| table_at + e + "</table>".len())
            .unwrap_or_else(|| {
                panic!("every <table> in the {which} partial must be closed; body:\n{html}")
            });
        let contained = wrap_opens.iter().any(|&w| {
            w < table_at
                && prev_table.is_none_or(|p| w > p)
                && matching_div_close(html, w).is_some_and(|wclose| wclose > table_close)
        });
        assert!(
            contained,
            "every <table> in the {which} partial must be CONTAINED in a `.tablewrap` <div> \
             (the wrapper opens before <table> and its </div> closes after </table>); \
             uncontained table at byte {table_at}; body:\n{html}"
        );
        prev_table = Some(table_at);
    }
}

/// The generic `<ol class="ui-steps"> … </ol>` region of the detail partial.
fn pipeline_region(detail: &str) -> &str {
    let class_at = detail.find("class=\"ui-steps\"").unwrap_or_else(|| {
        panic!("the apis detail partial must render events with the generic ui-steps primitive; body:\n{detail}")
    });
    let ol_open = detail[..class_at]
        .rfind("<ol")
        .unwrap_or_else(|| panic!("the `ui-steps` class must sit on an <ol>; body:\n{detail}"));
    region(&detail[ol_open..], "<ol", "</ol>")
        .unwrap_or_else(|| panic!("the pipeline <ol> must be closed; body:\n{detail}"))
}

/// The `<table> … </table>` region that contains `needle` (used to locate the Spec-lineage
/// table by a decision value seeded into both surfaces — only tables are scanned, so the
/// pipeline `<ol>` occurrence cannot match).
fn table_containing<'a>(html: &'a str, needle: &str, which: &str) -> &'a str {
    let mut base = 0usize;
    while let Some(rel) = html[base..].find("<table") {
        let start = base + rel;
        let end = html[start..]
            .find("</table>")
            .map(|e| start + e + "</table>".len())
            .unwrap_or_else(|| panic!("an unclosed <table> in the {which} partial; body:\n{html}"));
        let table = &html[start..end];
        if table.contains(needle) {
            return table;
        }
        base = end;
    }
    panic!("the {which} partial must contain a <table> holding {needle:?}; body:\n{html}");
}

fn text_content(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct OverviewFixture {
    items: Vec<Value>,
    payments: String,
    present_null: String,
}

fn overview_fixture() -> OverviewFixture {
    let payments = unique("api-payments");
    let present_null = unique("api-stable-null-decision");
    OverviewFixture {
        items: vec![
            lifecycle_list_item(
                801,
                &unique("api-catalog"),
                "Catalog API",
                "Current published catalog",
                3,
                3,
                2,
                2,
                Some(1),
                Some(1),
                Some("published"),
            ),
            lifecycle_list_item(
                802,
                &payments,
                "Payments API",
                "Payments ingress lifecycle",
                17,
                4,
                1,
                1,
                Some(3),
                Some(2),
                Some("submitted"),
            ),
            lifecycle_list_item(
                803,
                &unique("api-reviews"),
                "Reviews API",
                "Awaiting first publication",
                8,
                2,
                2,
                0,
                Some(1),
                None,
                Some("reviewed"),
            ),
            lifecycle_list_item(
                804,
                &unique("api-orders"),
                "Orders API",
                "Rejected draft retained",
                11,
                5,
                3,
                3,
                Some(3),
                Some(2),
                Some("rejected"),
            ),
            lifecycle_list_item(
                805,
                &unique("api-empty"),
                "Empty API",
                "No specification yet",
                2,
                1,
                1,
                1,
                None,
                None,
                None,
            ),
            lifecycle_list_item(
                806,
                &present_null,
                "Stable API",
                "Published without a latest review event",
                29,
                0,
                0,
                0,
                Some(4),
                Some(4),
                None,
            ),
        ],
        payments,
        present_null,
    }
}

fn list_only_state(
    team: &str,
    items: Vec<Value>,
    total: u64,
    fail_at: Option<(u64, u16)>,
) -> StubState {
    StubState {
        team: team.to_string(),
        list: Mutex::new(Coll {
            status: 200,
            items,
            total,
            fail_at,
        }),
        apis: Vec::new(),
        route_configs: Vec::new(),
        listeners: Vec::new(),
    }
}

// =============================================================================================
// Criterion 1 (shell): GET /<nonce>/apis serves the APIs shell page — `<nav class="tabs">`
// links all 7 tabs as real `<a>` anchors with the APIs anchor `class="active"` (a real link,
// not an ARIA-tab widget); the `<header class="topbar">` renders a `.brand` element and a
// `.ctx` chip row with a team-name chip and a `read-only` chip; no inline `<style>`, no
// src-less `<script>`, no inline `style=`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apis_shell_nav_active_tab_topbar_and_no_inline() {
    let (state, fx) = happy_fixture();
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let shell = fetch_page(&http, &dash, "apis").await;

    // The nav is `<nav class="tabs">`.
    let nav_tag = opening_tag(&shell, "<nav")
        .unwrap_or_else(|| panic!("the apis shell must render a <nav> element; body:\n{shell}"));
    assert!(
        tag_has_class(nav_tag, "tabs"),
        "the nav must be `<nav class=\"tabs\">`; got tag: {nav_tag:?}"
    );
    let nav = region(&shell, "<nav", "</nav>")
        .unwrap_or_else(|| panic!("the <nav> must be closed; body:\n{shell}"));

    // All 7 tabs are named as real anchors, in order.
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

    // The APIs anchor is the active one, and it is a REAL link (href=), not an ARIA-tab widget.
    let apis_at = nav.find("APIs").expect("nav names APIs (asserted above)");
    let a_start = nav[..apis_at]
        .rfind("<a")
        .expect("APIs anchor (asserted above)");
    let apis_tag = opening_tag(&nav[a_start..], "<a").expect("APIs anchor tag");
    assert!(
        tag_has_class(apis_tag, "active"),
        "the APIs anchor must carry `class=\"active\"`; got tag: {apis_tag:?}"
    );
    assert!(
        apis_tag.contains("href="),
        "the APIs tab must be a real <a> link (carry href=), not an ARIA-tab widget; \
         got tag: {apis_tag:?}"
    );
    assert!(
        !apis_tag.to_lowercase().contains("role=\"tab\""),
        "the APIs tab must be a real link, not a role=\"tab\" widget; got tag: {apis_tag:?}"
    );

    // The topbar `<header class="topbar">` renders `.brand` and a `.ctx` chip row.
    let header_tag = opening_tag(&shell, "<header")
        .unwrap_or_else(|| panic!("the apis shell must render a <header> topbar; body:\n{shell}"));
    assert!(
        tag_has_class(header_tag, "topbar"),
        "the topbar must be `<header class=\"topbar\">`; got tag: {header_tag:?}"
    );
    let header = region(&shell, "<header", "</header>")
        .unwrap_or_else(|| panic!("the <header> must be closed; body:\n{shell}"));
    assert!(
        has_element_with_classes(header, &["brand"]),
        "the topbar must render a `.brand` element; header:\n{header}"
    );
    assert!(
        has_element_with_classes(header, &["ctx"]),
        "the topbar must render a `.ctx` chip row; header:\n{header}"
    );
    let ctx_start = header
        .find("ctx")
        .expect("the .ctx chip row (asserted above)");
    let ctx_scope = &header[ctx_start..];
    assert!(
        ctx_scope.contains(fx.team.as_str()),
        "the ctx chip row must contain a chip with the team name {:?}; ctx row:\n{ctx_scope}",
        fx.team
    );
    assert!(
        ctx_scope.to_lowercase().contains("read-only"),
        "the ctx chip row must contain a `read-only` chip; ctx row:\n{ctx_scope}"
    );

    // No inline <style>, no src-less <script>, no inline style= in the shell.
    assert_no_inline(&shell, "apis shell");
}

// =============================================================================================
// Criterion 2: list partial — `.panel` h2 with `<span class="count">` "N of M"; table
// CONTAINED in `.tablewrap`; NO `class="dataplanes"`; per-row `<details hx-get="…partials/
// apis/detail?api=…" hx-trigger="toggle once">` wiring preserved verbatim.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_partial_panel_count_tablewrap_no_dataplanes_and_detail_row_wiring() {
    let (state, fx) = happy_fixture();
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let list = fetch_page(&http, &dash, "partials/apis/list").await;

    // The partial renders as a `.panel` whose h2 carries <span class="count"> "N of M".
    assert!(
        has_element_with_classes(&list, &["panel"]),
        "the apis list partial must render as a `.panel`; body:\n{list}"
    );
    let h2 = region(&list, "<h2", "</h2>")
        .unwrap_or_else(|| panic!("the apis list panel must have an <h2>; body:\n{list}"));
    let count_texts = texts_of_elements_with_classes(h2, &["count"]);
    assert!(
        count_texts.iter().any(|t| t == "2 of 2"),
        "the apis list panel h2 must carry `<span class=\"count\">` \"2 of 2\" \
         (2 rendered rows of total 2); found count texts: {count_texts:?}; h2:\n{h2}"
    );

    // The table is wrapped in `.tablewrap`.
    assert_tables_wrapped(&list, "apis list", 1);

    // NO class="dataplanes" anywhere in the partial.
    assert!(
        !has_element_with_classes(&list, &["dataplanes"]),
        "the apis list partial must not contain class=\"dataplanes\" anywhere; body:\n{list}"
    );

    // Slice 4 transfers expansion ownership to accessible native buttons.
    let disclosures = all_opening_tags(&list, "<button");
    for name in [&fx.api_x, &fx.api_y] {
        let tag = disclosures
            .iter()
            .find(|t| t.contains(&format!("partials/apis/detail?api={name}")))
            .unwrap_or_else(|| {
                panic!(
                    "the apis list must render a native disclosure button wired to the detail \
                     partial for {name:?}; buttons: {disclosures:?}; body:\n{list}"
                )
            });
        let want_hx_get = format!("hx-get=\"/{}/partials/apis/detail?api={name}\"", dash.nonce);
        assert!(
            tag.contains(&want_hx_get),
            "the per-row button for {name:?} must carry the verbatim hx-get \
             {want_hx_get:?}; offending tag: {tag:?}"
        );
        assert!(
            tag.contains("hx-trigger=\"api-expand once\""),
            "the per-row button for {name:?} must carry hx-trigger=\"api-expand once\"; \
             offending tag: {tag:?}"
        );
    }
}

// =============================================================================================
// Criterion 3: detail partial — composite `d.pill` is `pill neutral`; review decisions are
// DESCRIPTIVE (every decision → `pill neutral` verbatim), asserted independently in the
// pipeline `<ol class="pipeline">` region AND the Spec-lineage table region, with a shared
// decision value seeded into both; NOTHING classified `pill good`/`pill crit` in either
// region; tools enabled → `pill neutral` "yes", disabled → `pill warn` "disabled"; all 3
// tables CONTAINED in `.tablewrap`; legacy `decision`/`decision-*` classes GONE.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detail_partial_pill_mappings_tablewrap_and_no_legacy_decision_classes() {
    let (state, fx) = happy_fixture();
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let detail = fetch_page(
        &http,
        &dash,
        &format!("partials/apis/detail?api={}", fx.api_x),
    )
    .await;

    // The composite API-lifecycle pill `d.pill` is `pill neutral` (published v2, latest v3
    // rejected). "published v2" is a distinctive composite string not produced elsewhere.
    let neutral_texts = texts_of_elements_with_classes(&detail, &["pill", "neutral"]);
    assert!(
        neutral_texts.iter().any(|t| t.contains("published v2")),
        "the composite API-lifecycle pill must be `pill neutral` carrying the composite state \
         (\"published v2 …\"); pill-neutral texts: {neutral_texts:?}; body:\n{detail}"
    );

    // G1(i) — pipeline region: review decisions are DESCRIPTIVE. Every `e.decision` in the
    // `<ol class="pipeline">` renders `pill neutral` verbatim, and NOTHING in the pipeline is
    // classified `pill good`/`pill crit`.
    let pipeline = pipeline_region(&detail);
    let pipe_neutral = texts_of_elements_with_classes(pipeline, &["pill", "neutral"]);
    for decision in ["published", "rejected", fx.decision_shared.as_str()] {
        assert!(
            pipe_neutral.iter().any(|t| t == decision),
            "the pipeline event decision {decision:?} must render \
             `<span class=\"pill neutral\">{decision}</span>` verbatim; \
             pipeline pill-neutral texts: {pipe_neutral:?}; pipeline:\n{pipeline}"
        );
    }
    assert!(
        texts_of_elements_with_classes(pipeline, &["pill", "good"]).is_empty()
            && texts_of_elements_with_classes(pipeline, &["pill", "crit"]).is_empty(),
        "pipeline event decisions are descriptive and must NOT be classified `pill good`/`pill \
         crit`; pipeline:\n{pipeline}"
    );

    // G1(ii) — Spec-lineage table region (located by the shared decision value, which is
    // seeded into BOTH the pipeline and a lineage row — table_containing scans only
    // <table> regions, so it lands on the lineage): the `s.decision` cell renders
    // `pill neutral` verbatim, and the table classifies NOTHING as `pill good`/`pill crit`.
    let lineage = table_containing(&detail, &fx.decision_shared, "apis detail lineage");
    let lineage_neutral = texts_of_elements_with_classes(lineage, &["pill", "neutral"]);
    for decision in ["rejected", fx.decision_shared.as_str()] {
        assert!(
            lineage_neutral.iter().any(|t| t == decision),
            "the Spec-lineage decision {decision:?} must render \
             `<span class=\"pill neutral\">{decision}</span>` verbatim; \
             lineage pill-neutral texts: {lineage_neutral:?}; lineage table:\n{lineage}"
        );
    }
    assert!(
        texts_of_elements_with_classes(lineage, &["pill", "good"]).is_empty()
            && texts_of_elements_with_classes(lineage, &["pill", "crit"]).is_empty(),
        "Spec-lineage decisions are descriptive and must NOT be classified `pill good`/`pill \
         crit`; lineage table:\n{lineage}"
    );

    // Tools: disabled → `pill warn` "disabled"; enabled → `pill neutral` "yes".
    let warn_texts = texts_of_elements_with_classes(&detail, &["pill", "warn"]);
    assert!(
        warn_texts.iter().any(|t| t == "disabled"),
        "a disabled tool must render `<span class=\"pill warn\">disabled</span>`; \
         pill-warn texts: {warn_texts:?}; body:\n{detail}"
    );
    assert!(
        neutral_texts.iter().any(|t| t == "yes"),
        "an enabled tool must render `<span class=\"pill neutral\">yes</span>`; \
         pill-neutral texts: {neutral_texts:?}; body:\n{detail}"
    );

    // All 3 tables (lineage, chain, tools) are wrapped in `.tablewrap`.
    assert_tables_wrapped(&detail, "apis detail", 3);

    // The legacy `decision` / `decision-*` class pattern is GONE.
    for tokens in class_token_sets(&detail) {
        for token in &tokens {
            assert!(
                token != "decision" && !token.starts_with("decision-"),
                "the legacy `decision`/`decision-*` class pattern must be gone from the \
                 apis detail partial; offending class token {token:?} in \
                 {tokens:?}; body:\n{detail}"
            );
        }
    }
}

// =============================================================================================
// Criterion 4: upstream 403 on the list → "Not authorized" panel; 500 → "unavailable"
// panel; no inline <style>, no src-less <script>, no style= attribute in any partial.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_403_not_authorized_500_unavailable_and_no_inline_anywhere() {
    let (state, fx) = happy_fixture();
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    // 403 on the api-definitions list → the partial says "Not authorized".
    stub.set_list_status(403);
    let partial = fetch_page(&http, &dash, "partials/apis/list").await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the api-definitions list must render \"Not authorized\" in the apis \
         list partial; body:\n{partial}"
    );

    // 500 → "unavailable".
    stub.set_list_status(500);
    let partial = fetch_page(&http, &dash, "partials/apis/list").await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a 500 from the api-definitions list must render \"unavailable\" in the apis list \
         partial; body:\n{partial}"
    );

    // Healthy again: no inline <style>, no src-less <script>, no style= in ANY partial.
    stub.set_list_status(200);
    let list = fetch_page(&http, &dash, "partials/apis/list").await;
    assert_no_inline(&list, "apis list");
    let detail = fetch_page(
        &http,
        &dash,
        &format!("partials/apis/detail?api={}", fx.api_x),
    )
    .await;
    assert_no_inline(&detail, "apis detail");
}

// =============================================================================================
// fpv2-41r.2 — independently authored served-HTML / black-box contracts.
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn six_complete_api_rows_render_exact_overview_and_lifecycle_facts() {
    let team = unique("team-overview-complete");
    let fx = overview_fixture();
    let state = list_only_state(&team, fx.items, 6, None);
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let list = fetch_page(&client(), &dash, "partials/apis/list").await;
    let text = text_content(&list);

    // Four exact cards over the six completely acquired, parseable rows.
    for expected in [
        "API definitions 6",
        "4 published",
        "2 attention",
        "API tools 15",
        "9 enabled",
        "6 disabled",
        "Route bindings 7",
        "4 APIs with bindings",
        "Latest unpublished 3",
    ] {
        assert!(
            text.contains(expected),
            "complete six-row HTML must expose exact aggregate text {expected:?}; text:\n{text}"
        );
    }
    assert!(
        !text.contains('≥') && !list.contains("&ge;"),
        "complete acquisition must not render lower-bound card values; body:\n{list}"
    );

    // One lifecycle row pins real description/revision, semantic state, enabled/total tools,
    // bindings, age, and the retained native detail interaction.
    let payments = row_of(&list, &fx.payments);
    let payments_text = text_content(payments);
    for expected in [
        "Payments ingress lifecycle",
        "published v2 · v3 submitted",
        "1 / 4",
        "1 binding",
        "ago",
    ] {
        assert!(
            payments_text.contains(expected),
            "payments row must render lifecycle fact {expected:?}; row:\n{payments}"
        );
    }
    assert!(
        payments_text.contains("revision 17") || payments_text.contains("rev 17"),
        "payments row must expose real revision 17; row:\n{payments}"
    );
    assert!(
        has_element_with_classes(payments, &["pill", "warn"]),
        "submitted newer-latest state must have semantic warn class; row:\n{payments}"
    );
    let disclosures = all_opening_tags(&list, "<button");
    let disclosure = disclosures
        .iter()
        .find(|tag| tag.contains(&format!("partials/apis/detail?api={}", fx.payments)))
        .unwrap_or_else(|| {
            panic!(
                "the slice must expose a native disclosure-button interaction for {}; body:\n{list}",
                fx.payments
            )
        });
    assert!(
        disclosure.contains("hx-trigger=\"api-expand once\""),
        "native detail must retain one-shot lazy detail wiring; tag: {disclosure:?}"
    );

    // `latest_decision` is present-null (not absent) and `enabled_tool_count` is present: the
    // row must decode normally rather than falling into version-skew presentation.
    let null_decision = row_of(&list, &fx.present_null);
    let null_text = text_content(null_decision);
    assert!(
        null_text.contains("published v4") && null_text.contains("0 / 0"),
        "present-null latest_decision and required enabled count must decode; row:\n{null_decision}"
    );
    assert!(
        !null_text.to_lowercase().contains("unparseable")
            && !null_text.to_lowercase().contains("version skew"),
        "present-null latest_decision is a real no-event value, not version skew; row:\n{null_decision}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_api_sweep_uses_visible_lower_bounds_not_exact_global_aggregates() {
    let team = unique("team-overview-partial");
    let mut fx = overview_fixture();
    // A real second-page failure requires a full first page: the production sweep correctly
    // treats a short page as natural completion even when the envelope total drifted.
    for i in 6..500_u64 {
        fx.items.push(lifecycle_list_item(
            10_000 + i,
            &unique("api-padding"),
            "Padding API",
            "Zero-valued partial-sweep padding",
            1,
            0,
            0,
            0,
            None,
            None,
            None,
        ));
    }
    // The first page is visible, but the next bounded-sweep page fails. The envelope's 600 is
    // useful only for "first N of M" acquisition wording, never for fabricated global sums.
    let state = list_only_state(&team, fx.items, 600, Some((500, 500)));
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let list = fetch_page(&client(), &dash, "partials/apis/list").await;
    let text = text_content(&list);
    let lower = text.to_lowercase();

    assert!(
        lower.contains("partial") && lower.contains("visible"),
        "bounded-sweep failure must remain an explicit visible-partial notice; text:\n{text}"
    );
    assert!(
        lower.contains("first 500 of 600"),
        "partial acquisition must state the visible subset as first 500 of 600; text:\n{text}"
    );
    let lower_bound_marks = text.matches('≥').count() + list.matches("&ge;").count();
    assert!(
        lower_bound_marks >= 4,
        "all four partial aggregate cards must use lower-bound values; body:\n{list}"
    );
    for heading in [
        "API definitions",
        "API tools",
        "Route bindings",
        "Latest unpublished",
    ] {
        assert!(
            text.contains(heading),
            "partial HTML must retain the {heading:?} card; text:\n{text}"
        );
    }
    assert!(
        !text.contains("API definitions 600")
            && !text.contains("API tools 600")
            && !text.contains("Route bindings 600")
            && !text.contains("Latest unpublished 600"),
        "partial HTML must never promote upstream total=600 into an exact global aggregate; \
         text:\n{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_required_lifecycle_keys_render_version_skew_and_are_excluded_from_cards() {
    let team = unique("team-overview-version-skew");
    let valid = lifecycle_list_item(
        901,
        &unique("api-valid"),
        "Valid API",
        "Only parseable aggregate contributor",
        6,
        5,
        3,
        2,
        Some(1),
        None,
        Some("submitted"),
    );
    let mut missing_enabled = lifecycle_list_item(
        902,
        &unique("api-missing-enabled"),
        "Missing enabled",
        "Must not fabricate enabled zero",
        7,
        99,
        99,
        99,
        Some(2),
        Some(1),
        Some("reviewed"),
    );
    missing_enabled
        .as_object_mut()
        .expect("fixture object")
        .remove("enabled_tool_count");
    let mut missing_decision = lifecycle_list_item(
        903,
        &unique("api-missing-decision"),
        "Missing decision",
        "Must not fabricate no events",
        8,
        88,
        88,
        88,
        Some(3),
        Some(2),
        Some("published"),
    );
    missing_decision
        .as_object_mut()
        .expect("fixture object")
        .remove("latest_decision");

    let state = list_only_state(
        &team,
        vec![valid, missing_enabled, missing_decision],
        3,
        None,
    );
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let list = fetch_page(&client(), &dash, "partials/apis/list").await;
    let text = text_content(&list);
    let lower = text.to_lowercase();

    assert!(
        lower.matches("unparseable").count() >= 2 && lower.matches("version skew").count() >= 2,
        "each missing required key must produce an explicit unparseable/version-skew row; \
         text:\n{text}"
    );
    assert!(
        lower.contains("unparsed rows excluded"),
        "decode failures must carry an explicit aggregate-exclusion notice; text:\n{text}"
    );
    for expected in [
        "API definitions 3",
        "0 published",
        "1 attention",
        "API tools 5",
        "3 enabled",
        "2 disabled",
        "Route bindings 2",
        "1 APIs with bindings",
        "Latest unpublished 1",
    ] {
        assert!(
            text.contains(expected),
            "cards must derive only from the one parseable row ({expected:?}); text:\n{text}"
        );
    }
    for fabricated in ["99 enabled", "88 enabled", "187 enabled", "192 bindings"] {
        assert!(
            !text.contains(fabricated),
            "unparseable rows must not leak into aggregate fact {fabricated:?}; text:\n{text}"
        );
    }
}

// =============================================================================================
// fpv2-41r.4 — independently authored accessible master/detail and DOM-only script contracts.
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_list_serves_scoped_search_pager_and_adjacent_native_disclosure_pairs() {
    let team = unique("team-interaction-contract");
    let api_name = unique("api-contract-name");
    let item = lifecycle_list_item(
        20_001,
        &api_name,
        "Display Scope Needle",
        "Description Scope Needle",
        9,
        4,
        3,
        2,
        Some(2),
        Some(1),
        Some("submitted"),
    );
    let state = list_only_state(&team, vec![item], 1, None);
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let list = fetch_page(&client(), &dash, "partials/apis/list").await;

    let search = all_opening_tags(&list, "<input")
        .into_iter()
        .find(|tag| attribute(tag, "type") == Some("search"))
        .unwrap_or_else(|| panic!("APIs list must render an input type=search; body:\n{list}"));
    assert_eq!(
        attribute(search, "placeholder"),
        Some("Search API name, display name, description, or lifecycle"),
        "search placeholder must state its exact four-field scope and must not claim tool/path search"
    );

    let buttons = all_opening_tags(&list, "<button");
    assert!(
        buttons
            .iter()
            .filter(|tag| attribute(tag, "type") == Some("button"))
            .count()
            >= 3,
        "list must render Prev, Next and disclosure as native type=button controls: {buttons:?}"
    );
    let visible_text = text_content(&list);
    for marker in ["Prev", "Next", "1–1 of 1"] {
        assert!(
            visible_text.contains(marker),
            "pager must expose marker {marker:?}; text:\n{visible_text}"
        );
    }

    let disclosure = buttons
        .iter()
        .find(|tag| tag.contains(&format!("partials/apis/detail?api={api_name}")))
        .unwrap_or_else(|| {
            panic!("primary API row must contain its detail disclosure button; body:\n{list}")
        });
    assert_eq!(attribute(disclosure, "type"), Some("button"));
    assert_eq!(attribute(disclosure, "aria-expanded"), Some("false"));
    assert_eq!(attribute(disclosure, "hx-trigger"), Some("api-expand once"));
    let controls = attribute(disclosure, "aria-controls")
        .unwrap_or_else(|| panic!("disclosure must name its adjacent detail row: {disclosure}"));
    let target = format!("#{controls}");
    assert_eq!(
        attribute(disclosure, "hx-target"),
        Some(target.as_str()),
        "htmx must swap the detail response into the controlled row"
    );

    let primary = row_of(&list, &api_name);
    let primary_at = primary.as_ptr() as usize - list.as_ptr() as usize;
    let after_primary = list[primary_at + primary.len()..].trim_start();
    assert!(
        after_primary.starts_with("<tr"),
        "the hidden detail row must be immediately adjacent to its primary row; after row:\n{after_primary}"
    );
    let detail_tag = opening_tag(after_primary, "<tr").expect("adjacent detail tr");
    assert_eq!(attribute(detail_tag, "id"), Some(controls));
    assert!(
        detail_tag
            .split_ascii_whitespace()
            .any(|part| part.trim_end_matches('>') == "hidden"),
        "adjacent detail row must initially carry the boolean hidden attribute: {detail_tag}"
    );

    let primary_tag = opening_tag(primary, "<tr").expect("primary tr");
    let lower_primary = primary_tag.to_ascii_lowercase();
    for forbidden in [
        "onclick=",
        "hx-get=",
        "hx-trigger=",
        "role=\"button\"",
        "tabindex=",
        "style=",
    ] {
        assert!(
            !lower_primary.contains(forbidden),
            "the primary <tr> itself must not be clickable or inline-styled ({forbidden}); tag: {primary_tag}"
        );
    }
    let search_text = attribute(primary_tag, "data-search")
        .unwrap_or_else(|| {
            panic!("primary row must carry normalized data-search text: {primary_tag}")
        })
        .to_ascii_lowercase();
    for needle in [
        api_name.to_ascii_lowercase(),
        "display scope needle".to_string(),
        "description scope needle".to_string(),
        "published v1 · v2 submitted".to_string(),
    ] {
        assert!(
            search_text.contains(&needle),
            "data-search must include name/display/description/lifecycle, missing {needle:?}: {search_text:?}"
        );
    }
    assert_no_inline(&list, "apis list");
    assert_no_inline_event_attributes(&list, "apis list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_ui_framework_and_api_adapter_are_dom_only_and_share_filter_paging() {
    let (state, fx) = happy_fixture();
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();
    let shell = fetch_page(&http, &dash, "apis").await;
    let expected_src = format!("src=\"/{}/assets/apis.js\"", dash.nonce);
    let expected_ui_src = format!("src=\"/{}/assets/ui.js\"", dash.nonce);
    let scripts = all_opening_tags(&shell, "<script");
    let ui_position = scripts
        .iter()
        .position(|tag| tag.contains(&expected_ui_src))
        .expect("APIs shell must load the shared UI framework");
    let adapter_position = scripts
        .iter()
        .position(|tag| tag.contains(&expected_src))
        .expect("APIs shell must load its adapter");
    assert!(
        ui_position < adapter_position,
        "shared UI framework must load before APIs adapter"
    );

    let url = dash.nonce_url("assets/apis.js");
    let resp = fetch(&http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "nonce-prefixed apis.js must be served"
    );
    let source = resp.text().await.expect("apis.js source");
    let ui_resp = fetch(&http, &dash.nonce_url("assets/ui.js")).await;
    assert_eq!(
        ui_resp.status().as_u16(),
        200,
        "shared ui.js must be served"
    );
    let ui_source = ui_resp.text().await.expect("ui.js source");
    let lower = format!("{ui_source}\n{source}").to_ascii_lowercase();
    for forbidden in [
        "fetch(",
        "eval(",
        "localstorage",
        "innerhtml",
        "insertadjacenthtml",
        "document.cookie",
        "authorization",
        "bearer",
        "credential",
        "token",
    ] {
        assert!(
            !lower.contains(forbidden),
            "apis.js is DOM-state-only and must not contain {forbidden:?}; source:\n{source}"
        );
    }

    for required in [
        "addeventlistener",
        "aria-expanded",
        "hidden",
        "dataset.search",
        "api-expand",
        "dispatchevent",
    ] {
        assert!(
            lower.contains(required),
            "apis.js must own disclosure/filter/page DOM state and contain {required:?}; source:\n{source}"
        );
    }
    for required in ["paginate", "tolowercase", "math.ceil", "selecttab"] {
        assert!(
            ui_source.to_ascii_lowercase().contains(required),
            "ui.js must own reusable tab/filter/page behavior and contain {required:?}; source:\n{ui_source}"
        );
    }
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        compact.contains("constPAGE_SIZE=25;"),
        "display paging must define an auditable 25-pair page size; source:\n{source}"
    );
    assert!(
        compact.contains("letcurrentPage=0;") || compact.contains("letpage=0;"),
        "paging state must start on page one (zero-based); source:\n{source}"
    );
    let input_listener = compact
        .find("addEventListener(\"input\"")
        .or_else(|| compact.find("addEventListener('input'"))
        .unwrap_or_else(|| panic!("filter must have an input listener; source:\n{source}"));
    let input_tail = &compact[input_listener..];
    assert!(
        input_tail.contains("currentPage=0;") || input_tail.contains("page=0;"),
        "filter input must reset display paging to page one; source:\n{source}"
    );
    assert_eq!(
        compact.matches("button.dispatchEvent(").count(),
        1,
        "the first expansion path must dispatch api-expand on the disclosure button exactly once in source"
    );
}
