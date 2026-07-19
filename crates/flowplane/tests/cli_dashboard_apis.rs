//! fpv2-cuu — `flowplane dashboard` APIs tab (API lifecycle read model) — black-box,
//! spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * New page `GET /<nonce>/apis` (HTML shell, linked from the nav as "APIs") with an
//!     "API definitions" panel that lazy-loads `GET /<nonce>/partials/apis/list`.
//!   * The list partial fetches `GET /api/v1/teams/{team}/api-definitions?limit=500&offset=0`
//!     (uniform `{items, total, limit, offset}` envelope) and renders one row per API:
//!     name, display name, a versions summary derived from the enrichment fields
//!     (`published_version`/`latest_version`: "published vN" when equal, "published vN ·
//!     latest vM" when different, "latest vM (unpublished)" when nothing published,
//!     "no spec" when neither), tool_count, route_binding_count.
//!   * Each row embeds a lazy detail `GET /<nonce>/partials/apis/detail?api=<name>` which
//!     fetches from upstream: the definition GET, the paged specs list, the latest
//!     version's paged events (only when versions exist), paged route-bindings, paged
//!     tools, and — ONLY when bindings are non-empty — the paged route-configs and
//!     listeners sweeps to join binding IDs to names. It renders a state pill (for
//!     published v2 + latest v3 rejected the pill text is exactly
//!     "published v2 · v3 rejected"), the event history verbatim in order, a lineage
//!     table (one row per version, newest first, source_kind + latest_decision), an Envoy
//!     chain table (binding → route-config NAME → listener NAME), and a tools table where
//!     a disabled tool is marked disabled.
//!   * CRITICAL negative: the dashboard NEVER fetches any `/specs/{v}/content` path and
//!     never any secret-value path.
//!   * Degradation per the dashboard conventions: list upstream 403 → HTTP 200 partial
//!     saying not authorized; 500 → HTTP 200 "unavailable"; 401 → HTTP 286 (htmx
//!     stop-polling) naming `flowplane auth login`.
//!   * All five review decisions (submitted, reviewed, rejected, published, unpublished)
//!     in an events payload render without error.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique
//! team/API names; nothing binds a fixed port. Every spawned server is killed via a Drop
//! guard in all paths, including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a partial response is unambiguous.
const SECRET_TOKEN: &str = "sekret-apis-token-do-not-leak-7b2e";

/// The documented list page size.
const PAGE_LIMIT: u64 = 500;

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

fn hex_hash(v: u64) -> String {
    format!("{v:064x}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the API-lifecycle read-model
// endpoints with real limit/offset paging, canned failures, and a full request journal
// (path + query + auth header). Unknown paths are recorded too and answered 404, so
// allowlist / negative assertions see every request the dashboard makes.
// =============================================================================================

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Raw query string (empty when absent).
    query: String,
    authorization: Option<String>,
}

impl Recorded {
    fn page(&self) -> PageQuery {
        let uri: axum::http::Uri = format!("/q?{}", self.query)
            .parse()
            .expect("recorded query");
        Query::<PageQuery>::try_from_uri(&uri)
            .map(|q| q.0)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
struct PageQuery {
    limit: Option<u64>,
    offset: Option<u64>,
}

/// One API definition's canned upstream sub-resources.
#[derive(Clone)]
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
    /// Status for the api-definitions LIST endpoint (200 = healthy).
    list_status: u16,
    /// Status for spec review-EVENTS endpoints (200 = healthy). Implementer-added
    /// regression knob (Codex S7 review): failed event fetches must render an explicit
    /// unavailable notice, never an empty history.
    events_status: u16,
    /// Status for the route-configs/listeners join sweeps (200 = healthy). Same review:
    /// failed joins must render an explicit notice, not silently fall back to raw IDs.
    infra_status: u16,
    apis: Vec<ApiFixture>,
    route_configs: Vec<Value>,
    listeners: Vec<Value>,
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
fn paged(items: &[Value], page: PageQuery) -> Response {
    let limit = page.limit.unwrap_or(50) as usize;
    let offset = page.offset.unwrap_or(0) as usize;
    let start = offset.min(items.len());
    let end = start.saturating_add(limit).min(items.len());
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
    let authorization = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    state.requests.lock().unwrap().push(Recorded {
        path: path.clone(),
        query,
        authorization,
    });

    let page: PageQuery = Query::<PageQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();

    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();
    let api = |name: &str| state.apis.iter().find(|a| a.name == name);

    match segs.as_slice() {
        ["api-definitions"] => {
            if state.list_status != 200 {
                return canned_error(state.list_status);
            }
            let items: Vec<Value> = state.apis.iter().map(|a| a.list_item.clone()).collect();
            paged(&items, page)
        }
        ["api-definitions", name] => match api(name) {
            Some(a) => Json(a.definition.clone()).into_response(),
            None => canned_error(404),
        },
        ["api-definitions", name, "specs"] => match api(name) {
            Some(a) => paged(&a.specs, page),
            None => canned_error(404),
        },
        ["api-definitions", name, "specs", _v, "events"] => {
            if state.events_status != 200 {
                return canned_error(state.events_status);
            }
            match api(name) {
                Some(a) => paged(&a.events, page),
                None => canned_error(404),
            }
        }
        ["api-definitions", name, "route-bindings"] => match api(name) {
            Some(a) => paged(&a.bindings, page),
            None => canned_error(404),
        },
        ["api-definitions", name, "tools"] => match api(name) {
            Some(a) => paged(&a.tools, page),
            None => canned_error(404),
        },
        ["route-configs"] => {
            if state.infra_status != 200 {
                return canned_error(state.infra_status);
            }
            paged(&state.route_configs, page)
        }
        ["listeners"] => {
            if state.infra_status != 200 {
                return canned_error(state.infra_status);
            }
            paged(&state.listeners, page)
        }
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
        "route_binding_count": route_binding_count,
        "latest_version": latest_version,
        "published_version": published_version,
        "created_at": TS,
        "updated_at": TS2,
    })
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

fn empty_stub(team: &str, list_status: u16) -> StubState {
    StubState {
        team: team.to_string(),
        list_status,
        events_status: 200,
        infra_status: 200,
        apis: Vec::new(),
        route_configs: Vec::new(),
        listeners: Vec::new(),
        requests: Mutex::new(Vec::new()),
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
    fn page_url(&self, page: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{}", self.port, self.nonce, page)
    }

    fn apis_shell_url(&self) -> String {
        self.page_url("apis")
    }

    fn list_partial_url(&self) -> String {
        self.page_url("partials/apis/list")
    }

    fn detail_partial_url(&self, api: &str) -> String {
        format!("{}?api={api}", self.page_url("partials/apis/detail"))
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

/// Fetch a dashboard URL with a startup tolerance: retry on transport errors and 5xx until
/// a non-5xx response arrives or 15s elapse. Terminal statuses (200/286/4xx) return.
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
// Shared assertion helpers.
// =============================================================================================

fn idx_of(body: &str, needle: &str, what: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("{what}: expected {needle:?} in body:\n{body}"))
}

/// The CRITICAL negatives: no /content path, no secret/value path — over the whole journal.
fn assert_no_content_or_secret_paths(recorded: &[Recorded]) {
    for req in recorded {
        assert!(
            !req.path.contains("/content"),
            "the dashboard must NEVER fetch a spec /content path (spec content stays out of \
             the APIs tab); recorded: {:?}",
            req.path
        );
        let full = format!("{}?{}", req.path, req.query).to_lowercase();
        assert!(
            !full.contains("secret") && !full.contains("value"),
            "upstream request must never target a secret/value route: {full:?}"
        );
    }
}

// =============================================================================================
// Fixture: the happy-path API-X (published v2, latest v3 rejected, one bound route, one
// enabled + one disabled tool) plus a trivially-published API-Y.
// =============================================================================================

struct HappyFixture {
    stub_state: StubState,
    team: String,
    api_x: String,
    api_y: String,
    binding_name: String,
    tool_on: String,
    tool_off: String,
    reason_first: String,
    reason_second: String,
}

fn happy_fixture() -> HappyFixture {
    let team = unique("team");
    let api_x = unique("api-x");
    let api_y = unique("api-y");
    let binding_name = unique("bind-main");
    let tool_on = unique("tool-on");
    let tool_off = unique("tool-off");
    let reason_first = unique("reason-alpha-first");
    let reason_second = unique("reason-beta-second");

    // API-X: v2 published (id uid(102)), v3 latest rejected, v1 imported with no decision.
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
            spec_item(102, 2, "learned", Some("published")),
            spec_item(101, 1, "imported", None),
        ],
        events: vec![
            event_item(501, "submitted", &reason_first, TS),
            event_item(502, "rejected", &reason_second, TS2),
        ],
        bindings: vec![binding_item(401, &binding_name, 100, 201, 301)],
        tools: vec![
            tool_item(601, &tool_on, 100, 103, true),
            tool_item(602, &tool_off, 100, 103, false),
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
        list_item: y_list,
        specs: vec![spec_item(112, 1, "imported", Some("published"))],
        events: Vec::new(),
        bindings: Vec::new(),
        tools: Vec::new(),
    };

    let stub_state = StubState {
        team: team.clone(),
        list_status: 200,
        events_status: 200,
        infra_status: 200,
        apis: vec![x, y],
        route_configs: vec![infra_item(201, "rc-main"), infra_item(202, "rc-other")],
        listeners: vec![
            infra_item(301, "listener-main"),
            infra_item(302, "listener-other"),
        ],
        requests: Mutex::new(Vec::new()),
    };

    HappyFixture {
        stub_state,
        team,
        api_x,
        api_y,
        binding_name,
        tool_on,
        tool_off,
        reason_first,
        reason_second,
    }
}

// =============================================================================================
// Test 1: HAPPY PATH — shell + nav, list summaries, detail pill/events/lineage/chain/tools,
// and the request-journal contract (no /content, no secret path, joins fetched).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apis_happy_path_list_and_detail_render_lifecycle_read_model() {
    let fx = happy_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    // --- Shell page: 200 HTML with the APIs nav entry and a lazy "API definitions" panel.
    let resp = fetch(&http, &dash.apis_shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/apis must serve the APIs shell page"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        shell.contains('<'),
        "the shell must be HTML; body:\n{shell}"
    );
    assert!(
        shell.contains("API definitions"),
        "the shell must contain the \"API definitions\" panel; body:\n{shell}"
    );
    assert!(
        shell.contains("partials/apis/list"),
        "the panel must lazy-load /partials/apis/list; body:\n{shell}"
    );
    assert!(
        shell.contains("APIs"),
        "the nav must name the tab \"APIs\"; body:\n{shell}"
    );

    // The APIs page is linked from the existing nav: the Resources shell must link to it.
    let resp = fetch(&http, &dash.page_url("resources")).await;
    assert_eq!(resp.status().as_u16(), 200, "resources shell must be 200");
    let resources_shell = resp.text().await.expect("resources shell body");
    assert!(
        resources_shell.contains("APIs") && resources_shell.contains("/apis"),
        "the Resources nav must link to the APIs page as \"APIs\"; body:\n{resources_shell}"
    );

    // --- List partial: both API names + derived versions summaries + counts.
    let resp = fetch(&http, &dash.list_partial_url()).await;
    assert_eq!(resp.status().as_u16(), 200, "the list partial must be 200");
    let list = resp.text().await.expect("list partial body");
    for name in [&fx.api_x, &fx.api_y] {
        assert!(
            list.contains(name.as_str()),
            "the list must render a row for {name:?}; body:\n{list}"
        );
    }
    assert!(
        list.contains("API X Display"),
        "the list must render the display name; body:\n{list}"
    );
    assert!(
        list.contains("published v2 · latest v3"),
        "API-X (published_version=2, latest_version=3) must summarize as \
         \"published v2 · latest v3\"; body:\n{list}"
    );
    assert!(
        list.contains("published v1"),
        "API-Y (published_version=latest_version=1) must summarize as \"published v1\"; \
         body:\n{list}"
    );

    // List request contract: limit=500&offset=0 to the api-definitions list path.
    let list_path = format!("/api/v1/teams/{}/api-definitions", fx.team);
    let recorded = stub.recorded();
    let list_reqs: Vec<&Recorded> = recorded.iter().filter(|r| r.path == list_path).collect();
    assert!(
        !list_reqs.is_empty(),
        "the list partial must fetch the api-definitions list; recorded: {recorded:?}"
    );
    for req in &list_reqs {
        let page = req.page();
        assert_eq!(
            page.limit,
            Some(PAGE_LIMIT),
            "the api-definitions list fetch must carry limit=500; got query {:?}",
            req.query
        );
    }
    assert!(
        list_reqs.iter().any(|r| r.page().offset.unwrap_or(0) == 0),
        "the api-definitions list fetch must start at offset=0; recorded: {recorded:?}"
    );

    // --- Detail partial for API-X.
    let resp = fetch(&http, &dash.detail_partial_url(&fx.api_x)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the detail partial must be 200"
    );
    let detail = resp.text().await.expect("detail partial body");

    // State pill: exact text for published v2 + latest v3 rejected.
    assert!(
        detail.contains("published v2 · v3 rejected"),
        "the state pill for published v2 + rejected latest v3 must be exactly \
         \"published v2 · v3 rejected\"; body:\n{detail}"
    );

    // Event history verbatim, in payload order (distinctive reasons pin the order).
    assert!(
        detail.contains("submitted") && detail.contains("rejected"),
        "both event decisions (submitted, rejected) must render; body:\n{detail}"
    );
    let first = idx_of(&detail, &fx.reason_first, "event history");
    let second = idx_of(&detail, &fx.reason_second, "event history");
    assert!(
        first < second,
        "events must render verbatim IN ORDER (the submitted event's reason \
         {:?} must precede the rejected event's reason {:?}); body:\n{detail}",
        fx.reason_first,
        fx.reason_second
    );

    // Lineage: one row per version with its source kind, newest first.
    for needle in ["v3", "v2", "v1", "learned", "imported"] {
        assert!(
            detail.contains(needle),
            "the lineage table must contain {needle:?}; body:\n{detail}"
        );
    }
    let last_learned = detail.rfind("learned").expect("learned present");
    let imported = idx_of(&detail, "imported", "lineage");
    assert!(
        last_learned < imported,
        "lineage must be newest first: both \"learned\" rows (v3, v2) must precede the \
         \"imported\" row (v1); body:\n{detail}"
    );

    // Envoy chain: binding name → route-config NAME → listener NAME (joined from sweeps).
    assert!(
        detail.contains(fx.binding_name.as_str()),
        "the chain must render the binding name {:?}; body:\n{detail}",
        fx.binding_name
    );
    assert!(
        detail.contains("rc-main"),
        "the chain must resolve route_config_id to the route-config NAME \"rc-main\"; \
         body:\n{detail}"
    );
    assert!(
        detail.contains("listener-main"),
        "the chain must resolve listener_id to the listener NAME \"listener-main\"; \
         body:\n{detail}"
    );

    // Tools: both rows render; the disabled one is marked disabled.
    assert!(
        detail.contains(fx.tool_on.as_str()) && detail.contains(fx.tool_off.as_str()),
        "both tool rows must render; body:\n{detail}"
    );
    assert!(
        detail.to_lowercase().contains("disabled"),
        "the disabled tool row must be marked disabled; body:\n{detail}"
    );

    // --- Journal contract.
    let recorded = stub.recorded();
    assert_no_content_or_secret_paths(&recorded);

    let base = format!("/api/v1/teams/{}", fx.team);
    // Bindings are non-empty → the join sweeps MUST have run.
    for suffix in ["/route-configs", "/listeners"] {
        let path = format!("{base}{suffix}");
        assert!(
            recorded.iter().any(|r| r.path == path),
            "bindings are non-empty, so the dashboard must sweep {path:?} to join IDs to \
             names; recorded paths: {:?}",
            recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
        );
    }
    // Versions exist → the latest version's events were fetched.
    assert!(
        recorded.iter().any(|r| r
            .path
            .starts_with(&format!("{base}/api-definitions/{}/specs/", fx.api_x))
            && r.path.ends_with("/events")),
        "versions exist, so the latest version's events must be fetched; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    // Every upstream call carried the bearer token.
    let want_auth = format!("Bearer {SECRET_TOKEN}");
    for req in &recorded {
        assert_eq!(
            req.authorization.as_deref(),
            Some(want_auth.as_str()),
            "upstream request to {} must carry `Authorization: Bearer <token>`; got {:?}",
            req.path,
            req.authorization
        );
    }
    // Token non-disclosure in what we already fetched.
    for body in [&shell, &list, &detail] {
        assert!(
            !body.contains(SECRET_TOKEN),
            "a dashboard response body leaks the bearer token; body:\n{body}"
        );
    }
}

// =============================================================================================
// Test 2: FIVE-DECISION DEFENSIVE RENDERING — an events payload containing all five review
// decisions renders each decision string without erroring.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_five_review_decisions_render_in_detail() {
    let team = unique("team");
    let api = unique("api-five");
    let a_list = list_item(120, &api, "Five", Value::Null, 0, 0, json!(1), Value::Null);
    let fixture = ApiFixture {
        name: api.clone(),
        definition: definition_from(&a_list),
        list_item: a_list,
        specs: vec![spec_item(121, 1, "manual", Some("unpublished"))],
        events: vec![
            event_item(701, "submitted", "r-submitted", TS),
            event_item(702, "reviewed", "r-reviewed", TS),
            event_item(703, "rejected", "r-rejected", TS),
            event_item(704, "published", "r-published", TS),
            event_item(705, "unpublished", "r-unpublished", TS2),
        ],
        bindings: Vec::new(),
        tools: Vec::new(),
    };
    let mut state = empty_stub(&team, 200);
    state.apis = vec![fixture];
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.detail_partial_url(&api)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a payload with all five decisions must not error the detail partial"
    );
    let body = resp.text().await.expect("detail body");
    for decision in [
        "submitted",
        "reviewed",
        "rejected",
        "published",
        "unpublished",
    ] {
        assert!(
            body.contains(decision),
            "the event decision {decision:?} must render in the detail HTML; body:\n{body}"
        );
    }
    assert_no_content_or_secret_paths(&stub.recorded());
}

// =============================================================================================
// Test 3: DEGRADATION — list upstream 403 → 200 "not authorized"; 500 → 200 "unavailable";
// 401 → HTTP 286 naming `flowplane auth login` (same contract as the resources tabs).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_partial_degrades_per_dashboard_conventions() {
    let http = client();

    // Upstream 403 → HTTP 200 partial with a not-authorized state.
    {
        let team = unique("team");
        let stub = start_stub(empty_stub(&team, 403)).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.list_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 403 must not fail the partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("not authorized"),
            "the APIs list partial must say not authorized on upstream 403; body:\n{body}"
        );
    }

    // Upstream 500 → HTTP 200 partial with an "unavailable" state.
    {
        let team = unique("team");
        let stub = start_stub(empty_stub(&team, 500)).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.list_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 500 must not fail the partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "the APIs list partial must render an \"unavailable\" state on upstream 500; \
             body:\n{body}"
        );
    }

    // Upstream 401 → HTTP 286 (htmx stop-polling) naming `flowplane auth login`.
    {
        let team = unique("team");
        let stub = start_stub(empty_stub(&team, 401)).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.list_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            286,
            "upstream 401 must yield the htmx stop-polling status 286"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.contains("flowplane auth login"),
            "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
        );
    }
}

// =============================================================================================
// Test 4: NO VERSIONS — published_spec_version_id null + empty specs → "no spec" pill, and
// the journal shows NO /events fetch (and no join sweeps, since bindings are empty).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_without_versions_renders_no_spec_and_skips_events_fetch() {
    let team = unique("team");
    let api = unique("api-none");
    let a_list = list_item(
        130,
        &api,
        "Empty API",
        Value::Null,
        0,
        0,
        Value::Null,
        Value::Null,
    );
    let fixture = ApiFixture {
        name: api.clone(),
        definition: definition_from(&a_list),
        list_item: a_list,
        specs: Vec::new(),
        events: Vec::new(),
        bindings: Vec::new(),
        tools: Vec::new(),
    };
    let mut state = empty_stub(&team, 200);
    state.apis = vec![fixture];
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.detail_partial_url(&api)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the detail partial for a version-less API must still be 200"
    );
    let body = resp.text().await.expect("detail body");
    assert!(
        body.to_lowercase().contains("no spec"),
        "an API with no versions and nothing published must render the \"no spec\" pill; \
         body:\n{body}"
    );

    // Grace period so even an asynchronously-fired upstream fetch would be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let recorded = stub.recorded();
    let paths: Vec<String> = recorded.iter().map(|r| r.path.clone()).collect();
    assert!(
        !recorded.iter().any(|r| r.path.ends_with("/events")),
        "no versions exist, so NO /events path may be fetched; recorded paths: {paths:?}"
    );
    // Bindings are empty → the route-configs/listeners join sweeps must be skipped.
    assert!(
        !recorded
            .iter()
            .any(|r| r.path.ends_with("/route-configs") || r.path.ends_with("/listeners")),
        "bindings are empty, so the route-configs/listeners join sweeps must be skipped; \
         recorded paths: {paths:?}"
    );
    assert_no_content_or_secret_paths(&recorded);
}

// =============================================================================================
// Implementer-authored regression tests (Codex S7 review findings): failed sub-fetches
// must surface explicitly in the detail partial, never silently degrade.
// =============================================================================================

/// Events endpoint 500s: the detail must say the history is unavailable — NOT render the
/// "No review events" empty state (finding 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_events_fetch_renders_unavailable_not_empty_history() {
    let fixture = happy_fixture();
    let team = fixture.team.clone();
    let api_name = fixture.api_x.clone();
    let mut stub_state = fixture.stub_state;
    stub_state.events_status = 500;
    let stub = start_stub(stub_state).await;
    let home = common::unique_tempdir();
    let dash = spawn_dashboard(home, &stub.base_url, &team);
    let http = client();

    let body = fetch(&http, &dash.detail_partial_url(&api_name))
        .await
        .text()
        .await
        .expect("detail body");
    assert!(
        body.contains("Review events are currently unavailable"),
        "failed events fetch must surface explicitly, got: {body}"
    );
    assert!(
        !body.contains("No review events"),
        "a failed fetch must not masquerade as an empty history"
    );
}

/// Join sweeps (route-configs/listeners) 500 while bindings exist: the chain must carry
/// an explicit names-unavailable notice instead of silently showing raw IDs (finding 3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_join_sweeps_render_explicit_notice() {
    let fixture = happy_fixture();
    let team = fixture.team.clone();
    let api_name = fixture.api_x.clone();
    let mut stub_state = fixture.stub_state;
    stub_state.infra_status = 500;
    let stub = start_stub(stub_state).await;
    let home = common::unique_tempdir();
    let dash = spawn_dashboard(home, &stub.base_url, &team);
    let http = client();

    let body = fetch(&http, &dash.detail_partial_url(&api_name))
        .await
        .text()
        .await
        .expect("detail body");
    assert!(
        body.contains("Gateway resource names are currently unavailable"),
        "failed join sweeps must surface explicitly, got: {body}"
    );
    assert!(
        !body.contains("rc-main"),
        "names cannot come from a failed sweep"
    );
}

/// Implementer regression (found live in the eval container): the list panel is rendered
/// `<details open>`, and an already-open details element never fires a `toggle` event —
/// so the lazy fetch MUST be wired to htmx's `load` trigger or the panel shows
/// "Loading…" forever in a real browser (stub tests fetch partials directly and cannot
/// catch this).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_list_panel_fetches_on_load_not_toggle() {
    let fixture = happy_fixture();
    let team = fixture.team.clone();
    let stub = start_stub(fixture.stub_state).await;
    let home = common::unique_tempdir();
    let dash = spawn_dashboard(home, &stub.base_url, &team);
    let http = client();
    let shell = fetch(&http, &dash.apis_shell_url())
        .await
        .text()
        .await
        .expect("shell");
    let panel = shell
        .split("partials/apis/list")
        .next()
        .expect("panel prefix");
    let panel = &panel[panel.rfind("<details").expect("details tag")..];
    assert!(panel.contains("open"), "list panel renders open: {panel}");
    let after = &shell[shell.find("partials/apis/list").expect("wiring")..];
    let attrs = after.split('>').next().expect("attrs");
    let full = format!("{panel}{after}");
    assert!(
        full.contains("hx-trigger=\"load once\""),
        "open panel must fetch on load, not toggle: {attrs}"
    );
}
