//! fpv2-cuu — `flowplane dashboard` Learning tab — black-box, spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * New page `GET /<nonce>/learning` (nav: Overview / Resources / APIs / Learning) with a
//!     "Learning sessions" panel that lazy-loads `GET /<nonce>/partials/learning/sessions`,
//!     and a "Spec viewer" section targeted by htmx.
//!   * The sessions partial fetches
//!     `GET /api/v1/teams/{team}/learning-sessions?limit=500&offset=0` (Page envelope). When
//!     any session has `api_definition_id` it also fetches `GET .../api-definitions` to map
//!     api ids to names. For each DISTINCT `api_definition_id` among COMPLETED sessions
//!     (`completed_at` set) it fetches `GET .../api-definitions/{name}/specs` and collects
//!     the versions whose `source_kind == "learned"` as "produced spec versions" — rendered
//!     as viewer links labeled "<api> v<version>" wired to
//!     `/<nonce>/partials/learning/content?api=<name>&version=<n>`. Running (uncompleted)
//!     sessions show no produced links. Sessions with no api show "—".
//!   * The content viewer partial `/<nonce>/partials/learning/content?api=..&version=..`
//!     fetches `GET .../api-definitions/{api}/specs/{version}/content` (THE ONLY content
//!     fetch in the whole dashboard) and renders the JSON document pretty-printed inside a
//!     `<pre>` (read-only). The browser response carries the dashboard's global no-store
//!     headers. ETag revalidation: repeat fetches of the same (api, version) send
//!     `If-None-Match` with the first response's ETag; on upstream 304 the cached document
//!     renders and is marked revalidated ("revalidated" appears in the HTML).
//!   * Degradation per the dashboard conventions: viewer upstream 403 → not-authorized
//!     section mentioning the grants; 500 → "unavailable"; sessions upstream 401 → HTTP 286
//!     naming `flowplane auth login`.
//!   * CRITICAL negative: no secret-value path is ever fetched, and the ONLY `/content`
//!     fetches come from the viewer partial (never the sessions partial or a shell page).
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
const SECRET_TOKEN: &str = "sekret-learning-token-do-not-leak-4c9a";

/// The documented sessions-list page size.
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
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the learning-sessions +
// api-definitions read model, a spec-content endpoint with real If-None-Match/304 handling,
// canned failures, and a full request journal (path + query + auth + If-None-Match + the
// status the stub answered). Unknown paths are recorded too and answered 404, so allowlist /
// negative assertions see every request the dashboard makes.
// =============================================================================================

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Raw query string (empty when absent).
    query: String,
    authorization: Option<String>,
    if_none_match: Option<String>,
    /// The HTTP status the stub answered with.
    responded: u16,
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
    /// Items of `GET .../api-definitions/{name}/specs` (newest first, as the CP returns).
    specs: Vec<Value>,
    /// Canned bodies of `GET .../specs/{version}/content`: (version, document, etag).
    contents: Vec<(u64, Value, String)>,
}

struct StubState {
    team: String,
    /// Status for the learning-sessions LIST endpoint (200 = healthy).
    sessions_status: u16,
    /// Status for the spec-content endpoint (200 = healthy → real ETag/304 handling).
    content_status: u16,
    sessions: Vec<Value>,
    apis: Vec<ApiFixture>,
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

fn route_request(
    state: &StubState,
    path: &str,
    page: PageQuery,
    if_none_match: Option<&str>,
) -> Response {
    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();
    let api = |name: &str| state.apis.iter().find(|a| a.name == name);

    match segs.as_slice() {
        ["learning-sessions"] => {
            if state.sessions_status != 200 {
                return canned_error(state.sessions_status);
            }
            paged(&state.sessions, page)
        }
        ["api-definitions"] => {
            let items: Vec<Value> = state.apis.iter().map(|a| a.list_item.clone()).collect();
            paged(&items, page)
        }
        ["api-definitions", name] => match api(name) {
            Some(a) => {
                // The definition GET body: the list item works fine for name/id mapping.
                Json(a.list_item.clone()).into_response()
            }
            None => canned_error(404),
        },
        ["api-definitions", name, "specs"] => match api(name) {
            Some(a) => paged(&a.specs, page),
            None => canned_error(404),
        },
        ["api-definitions", name, "specs", v, "content"] => {
            if state.content_status != 200 {
                return canned_error(state.content_status);
            }
            let Some(a) = api(name) else {
                return canned_error(404);
            };
            let Ok(version) = v.parse::<u64>() else {
                return canned_error(404);
            };
            let Some((_, doc, etag)) = a.contents.iter().find(|(cv, _, _)| *cv == version) else {
                return canned_error(404);
            };
            // Real conditional-GET semantics: matching If-None-Match → 304, no body.
            if if_none_match == Some(etag.as_str()) {
                return Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header("etag", etag)
                    .body(axum::body::Body::empty())
                    .expect("build 304");
            }
            (StatusCode::OK, [("etag", etag.clone())], Json(doc.clone())).into_response()
        }
        _ => canned_error(404),
    }
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let header = |key: &str| {
        req.headers()
            .get(key)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let authorization = header("authorization");
    let if_none_match = header("if-none-match");

    let page: PageQuery = Query::<PageQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();

    let response = route_request(&state, &path, page, if_none_match.as_deref());
    state.requests.lock().unwrap().push(Recorded {
        path,
        query,
        authorization,
        if_none_match,
        responded: response.status().as_u16(),
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

/// `spec_item` plus the learned version's capture-session provenance (the wire field the
/// CP extracts from the document's learning-source stamp; ui-f4 S8 contract).
fn learned_spec_item(
    id: u64,
    version: u64,
    latest_decision: Option<&str>,
    session_id: u64,
) -> Value {
    let mut v = spec_item(id, version, "learned", latest_decision);
    v.as_object_mut()
        .unwrap()
        .insert("capture_session_id".into(), json!(uid(session_id)));
    v
}

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

/// A minimal-but-real OpenAPI document with a distinctive `info.title` marker.
fn openapi_doc(marker: &str) -> Value {
    json!({
        "openapi": "3.0.3",
        "info": { "title": marker, "version": "2.0.0" },
        "paths": {
            "/widgets": {
                "get": {
                    "operationId": "listWidgets",
                    "responses": { "200": { "description": "ok" } }
                }
            }
        }
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
    fn page_url(&self, page: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{}", self.port, self.nonce, page)
    }

    fn learning_shell_url(&self) -> String {
        self.page_url("learning")
    }

    fn sessions_partial_url(&self) -> String {
        self.page_url("partials/learning/sessions")
    }

    fn content_partial_url(&self, api: &str, version: u64) -> String {
        format!(
            "{}?api={api}&version={version}",
            self.page_url("partials/learning/content")
        )
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

/// CRITICAL negative: no upstream request ever targets a secret/value route.
fn assert_no_secret_paths(recorded: &[Recorded]) {
    for req in recorded {
        let full = format!("{}?{}", req.path, req.query).to_lowercase();
        assert!(
            !full.contains("secret") && !full.contains("value"),
            "upstream request must never target a secret/value route: {full:?}"
        );
    }
}

/// The upstream `/content` fetches recorded so far.
fn content_fetches(recorded: &[Recorded]) -> Vec<Recorded> {
    recorded
        .iter()
        .filter(|r| r.path.contains("/content"))
        .cloned()
        .collect()
}

/// Every recorded upstream request carried the bearer token — and never leaked into `bodies`.
fn assert_bearer_and_no_leak(recorded: &[Recorded], bodies: &[&str]) {
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
    for body in bodies {
        assert!(
            !body.contains(SECRET_TOKEN),
            "a dashboard response body leaks the bearer token; body:\n{body}"
        );
    }
}

// =============================================================================================
// Fixture: api-learn (learned v2 published + imported v1 → only "v2" is a produced link;
// v2 has a content document with a marker title), api-cap (learned v1, referenced only by a
// RUNNING session → must never surface as a produced link), and three sessions: completed
// (→ api-learn), capturing (→ api-cap, no completed_at), and one with no api at all.
// =============================================================================================

struct LearningFixture {
    stub_state: StubState,
    team: String,
    api_learn: String,
    api_cap: String,
    session_done: String,
    session_running: String,
    session_no_api: String,
    marker: String,
    etag_v2: String,
}

fn learning_fixture() -> LearningFixture {
    let team = unique("team");
    let api_learn = unique("api-learn");
    let api_cap = unique("api-cap");
    let session_done = unique("sess-done");
    let session_running = unique("sess-run");
    let session_no_api = unique("sess-noapi");
    let marker = unique("MARKER-openapi-title");
    let etag_v2 = format!("\"{}\"", hex_hash(2));

    // api-learn (id uid(100)): learned v2 (published) + imported v1 → produced link is v2 only.
    let learn = ApiFixture {
        name: api_learn.clone(),
        list_item: list_item(
            100,
            &api_learn,
            "API Learn Display",
            json!(uid(102)),
            0,
            0,
            json!(2),
            json!(2),
        ),
        specs: vec![
            learned_spec_item(102, 2, Some("published"), 500),
            spec_item(101, 1, "imported", None),
        ],
        contents: vec![(2, openapi_doc(&marker), etag_v2.clone())],
    };

    // api-cap (id uid(110)): has a learned v1 — but is only referenced by a RUNNING session,
    // so it must never surface as a produced link.
    let cap = ApiFixture {
        name: api_cap.clone(),
        list_item: list_item(
            110,
            &api_cap,
            "API Cap Display",
            Value::Null,
            0,
            0,
            json!(1),
            Value::Null,
        ),
        specs: vec![learned_spec_item(111, 1, None, 501)],
        contents: vec![(
            1,
            openapi_doc("cap-doc-should-not-render"),
            format!("\"{}\"", hex_hash(11)),
        )],
    };

    let stub_state = StubState {
        team: team.clone(),
        sessions_status: 200,
        content_status: 200,
        sessions: vec![
            session_item(500, &session_done, "completed", json!(uid(100)), json!(TS2)),
            session_item(
                501,
                &session_running,
                "capturing",
                json!(uid(110)),
                Value::Null,
            ),
            session_item(502, &session_no_api, "completed", Value::Null, json!(TS2)),
        ],
        apis: vec![learn, cap],
        requests: Mutex::new(Vec::new()),
    };

    LearningFixture {
        stub_state,
        team,
        api_learn,
        api_cap,
        session_done,
        session_running,
        session_no_api,
        marker,
        etag_v2,
    }
}

// =============================================================================================
// Test 1: SESSIONS PANEL — shell + nav, session rows (name/status/api name), produced-spec
// links for learned versions of COMPLETED sessions only, "—" for api-less sessions, and the
// journal contract (sessions limit=500&offset=0; api-definitions + specs fetched; NO content
// fetch from the sessions partial; no secret path; bearer auth everywhere).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn learning_sessions_panel_renders_sessions_and_produced_spec_links() {
    let fx = learning_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    // --- Shell page: 200 HTML with the Learning nav entry, a lazy "Learning sessions"
    // panel, and a "Spec viewer" section.
    let resp = fetch(&http, &dash.learning_shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/learning must serve the Learning shell page"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        shell.contains('<'),
        "the shell must be HTML; body:\n{shell}"
    );
    assert!(
        shell.contains("Learning sessions"),
        "the shell must contain the \"Learning sessions\" panel; body:\n{shell}"
    );
    assert!(
        shell.contains("partials/learning/sessions"),
        "the panel must lazy-load /partials/learning/sessions; body:\n{shell}"
    );
    for tab in ["Overview", "Resources", "APIs", "Learning"] {
        assert!(
            shell.contains(tab),
            "the nav must name the {tab:?} tab; body:\n{shell}"
        );
    }
    assert!(
        shell.contains("Spec viewer"),
        "the shell must contain the \"Spec viewer\" section; body:\n{shell}"
    );

    // --- Sessions partial: rows for all three sessions.
    let resp = fetch(&http, &dash.sessions_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the sessions partial must be 200"
    );
    let sessions = resp.text().await.expect("sessions partial body");
    for name in [&fx.session_done, &fx.session_running, &fx.session_no_api] {
        assert!(
            sessions.contains(name.as_str()),
            "the sessions panel must render a row for session {name:?}; body:\n{sessions}"
        );
    }
    // Statuses render.
    assert!(
        sessions.contains("completed") && sessions.contains("capturing"),
        "session statuses (completed, capturing) must render; body:\n{sessions}"
    );
    // The completed session's api renders by NAME (mapped via the api-definitions list).
    assert!(
        sessions.contains(fx.api_learn.as_str()),
        "the completed session must show its API by NAME {:?}; body:\n{sessions}",
        fx.api_learn
    );
    // Produced spec versions: only the learned v2 of the completed session's api is a viewer
    // link, labeled "<api> v2" and wired to the content partial.
    let produced_v2 = format!("{} v2", fx.api_learn);
    assert!(
        sessions.contains(&produced_v2),
        "the completed session's learned v2 must render as a produced-spec link labeled \
         {produced_v2:?}; body:\n{sessions}"
    );
    assert!(
        sessions.contains("partials/learning/content"),
        "produced-spec links must be wired to /partials/learning/content; body:\n{sessions}"
    );
    let produced_v1 = format!("{} v1", fx.api_learn);
    assert!(
        !sessions.contains(&produced_v1),
        "v1 is IMPORTED (not learned) and must NOT be a produced-spec link \
         ({produced_v1:?} must be absent); body:\n{sessions}"
    );
    // The RUNNING session shows no produced links: its api's learned v1 must not surface.
    let cap_link = format!("{} v", fx.api_cap);
    assert!(
        !sessions.contains(&cap_link),
        "a running (uncompleted) session must show NO produced-spec links — \
         {cap_link:?}* must be absent; body:\n{sessions}"
    );
    // The api-less session shows "—".
    assert!(
        sessions.contains('—'),
        "a session without an api must show \"—\"; body:\n{sessions}"
    );

    // --- Journal contract.
    let recorded = stub.recorded();
    let base = format!("/api/v1/teams/{}", fx.team);

    // learning-sessions fetched with limit=500&offset=0.
    let sessions_path = format!("{base}/learning-sessions");
    let session_reqs: Vec<&Recorded> = recorded
        .iter()
        .filter(|r| r.path == sessions_path)
        .collect();
    assert!(
        !session_reqs.is_empty(),
        "the sessions partial must fetch the learning-sessions list; recorded: {recorded:?}"
    );
    for req in &session_reqs {
        let page = req.page();
        assert_eq!(
            page.limit,
            Some(PAGE_LIMIT),
            "the learning-sessions fetch must carry limit=500; got query {:?}",
            req.query
        );
    }
    assert!(
        session_reqs
            .iter()
            .any(|r| r.page().offset.unwrap_or(0) == 0),
        "the learning-sessions fetch must start at offset=0; recorded: {recorded:?}"
    );

    // api-definitions fetched (id → name mapping) and api-learn's specs fetched.
    assert!(
        recorded
            .iter()
            .any(|r| r.path == format!("{base}/api-definitions")),
        "sessions reference api ids, so the api-definitions list must be fetched; recorded \
         paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert!(
        recorded
            .iter()
            .any(|r| r.path == format!("{base}/api-definitions/{}/specs", fx.api_learn)),
        "the completed session's api must have its specs fetched (produced-versions \
         collection); recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );

    // CRITICAL negatives: the sessions partial (and shell) must trigger NO /content fetch,
    // and never a secret/value path.
    let content = content_fetches(&recorded);
    assert!(
        content.is_empty(),
        "the sessions partial must NEVER fetch a spec /content path — only the viewer \
         partial may; recorded content fetches: {content:?}"
    );
    assert_no_secret_paths(&recorded);
    assert_bearer_and_no_leak(&recorded, &[&shell, &sessions]);
}

// =============================================================================================
// Test 2: SPEC VIEWER — the content partial fetches the upstream content endpoint exactly
// once (bearer auth, no If-None-Match on the first fetch), renders the document
// pretty-printed in a <pre> with no-store headers; a repeat fetch revalidates with
// If-None-Match, the stub answers 304, and the cached document renders marked "revalidated".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_viewer_renders_content_and_revalidates_with_etag() {
    let fx = learning_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    // Load the sessions partial first: proves the later /content fetches are attributable
    // to the viewer partial ONLY.
    let sessions_body = fetch(&http, &dash.sessions_partial_url())
        .await
        .text()
        .await
        .expect("sessions body");
    assert!(
        content_fetches(&stub.recorded()).is_empty(),
        "no /content fetch may happen before the viewer partial is requested"
    );

    // --- First viewer fetch: 200, marker inside a <pre>, no-store headers.
    let url = dash.content_partial_url(&fx.api_learn, 2);
    let resp = fetch(&http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the content viewer partial must be 200"
    );
    let cache_control = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(
        cache_control, "no-store",
        "the viewer response must carry the dashboard's global `Cache-Control: no-store`"
    );
    let first = resp.text().await.expect("viewer body");
    assert!(
        first.contains("<pre"),
        "the spec document must render inside a <pre>; body:\n{first}"
    );
    assert!(
        first.contains(fx.marker.as_str()),
        "the pretty-printed document must contain the info.title marker {:?}; body:\n{first}",
        fx.marker
    );
    assert!(
        first.contains("/widgets"),
        "the whole document (paths included) must render; body:\n{first}"
    );

    // Journal: exactly ONE upstream content fetch, to the right path, bearer-authed, and
    // WITHOUT If-None-Match (nothing is cached yet).
    let content_path = format!(
        "/api/v1/teams/{}/api-definitions/{}/specs/2/content",
        fx.team, fx.api_learn
    );
    let content = content_fetches(&stub.recorded());
    assert_eq!(
        content.len(),
        1,
        "exactly one upstream /content fetch after one viewer request; got: {content:?}"
    );
    assert_eq!(
        content[0].path, content_path,
        "the viewer must fetch the spec content endpoint"
    );
    assert_eq!(
        content[0].authorization.as_deref(),
        Some(format!("Bearer {SECRET_TOKEN}").as_str()),
        "the content fetch must carry bearer auth"
    );
    assert!(
        content[0].if_none_match.is_none(),
        "the FIRST content fetch has nothing cached, so it must not send If-None-Match; \
         got {:?}",
        content[0].if_none_match
    );
    assert_eq!(
        content[0].responded, 200,
        "stub sanity: first fetch answered 200"
    );

    // --- Second viewer fetch of the SAME (api, version): ETag revalidation.
    let resp = fetch(&http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the revalidated viewer partial must still be 200"
    );
    let second = resp.text().await.expect("second viewer body");

    let content = content_fetches(&stub.recorded());
    assert_eq!(
        content.len(),
        2,
        "the second viewer request must revalidate upstream (2 content fetches total); \
         got: {content:?}"
    );
    assert_eq!(
        content[1].if_none_match.as_deref(),
        Some(fx.etag_v2.as_str()),
        "the second content fetch must carry If-None-Match with the first response's ETag"
    );
    assert_eq!(
        content[1].responded, 304,
        "the stub must have answered the conditional fetch with 304 Not Modified"
    );
    assert!(
        second.contains(fx.marker.as_str()),
        "on upstream 304 the CACHED document must render (marker present); body:\n{second}"
    );
    assert!(
        second.to_lowercase().contains("revalidated"),
        "the 304 path must mark the document as revalidated (\"revalidated\" in the HTML); \
         body:\n{second}"
    );

    assert_no_secret_paths(&stub.recorded());
    assert_bearer_and_no_leak(&stub.recorded(), &[&sessions_body, &first, &second]);
}

// =============================================================================================
// Test 3: DEGRADATION — viewer content upstream 403 → not-authorized section mentioning the
// grants; 500 → "unavailable"; sessions upstream 401 → HTTP 286 naming `flowplane auth
// login` (same conventions as every other tab).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_and_sessions_partials_degrade_per_dashboard_conventions() {
    let http = client();

    // Content upstream 403 → HTTP 200 partial with a not-authorized section naming the grants.
    {
        let fx = learning_fixture();
        let api = fx.api_learn.clone();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.content_status = 403;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.content_partial_url(&api, 2)).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 403 must not fail the viewer partial itself"
        );
        let body = resp.text().await.expect("body");
        let lower = body.to_lowercase();
        assert!(
            lower.contains("not authorized"),
            "the viewer must say not authorized on upstream 403; body:\n{body}"
        );
        assert!(
            lower.contains("grant"),
            "the not-authorized section must mention the required grants; body:\n{body}"
        );
        assert!(
            !body.contains("MARKER-openapi-title"),
            "no document content may render on 403; body:\n{body}"
        );
    }

    // Content upstream 500 → HTTP 200 partial with an "unavailable" state.
    {
        let fx = learning_fixture();
        let api = fx.api_learn.clone();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.content_status = 500;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.content_partial_url(&api, 2)).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 500 must not fail the viewer partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "the viewer must render an \"unavailable\" state on upstream 500; body:\n{body}"
        );
    }

    // Sessions upstream 401 → HTTP 286 (htmx stop-polling) naming `flowplane auth login`.
    {
        let fx = learning_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.sessions_status = 401;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.sessions_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            286,
            "upstream 401 on the sessions list must yield the htmx stop-polling status 286"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.contains("flowplane auth login"),
            "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
        );
    }
}

// =============================================================================================
// Test 4: SESSIONS DEGRADATION (403/500) — the sessions partial degrades exactly like the
// other tabs: 403 → 200 not-authorized, 500 → 200 unavailable; in both cases no /content
// and no secret path is ever fetched.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_partial_403_and_500_degrade_without_content_fetches() {
    let http = client();

    for (status, needle) in [(403u16, "not authorized"), (500u16, "unavailable")] {
        let fx = learning_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.sessions_status = status;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.sessions_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream {status} must not fail the sessions partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains(needle),
            "the sessions partial must render {needle:?} on upstream {status}; body:\n{body}"
        );
        let recorded = stub.recorded();
        assert!(
            content_fetches(&recorded).is_empty(),
            "a degraded sessions partial must never fetch /content; recorded: {recorded:?}"
        );
        assert_no_secret_paths(&recorded);
    }
}

// =============================================================================================
// Implementer-authored regression test (Codex S8 review finding 1): produced-spec links
// must follow each version's capture-session provenance — two completed sessions on the
// SAME API must not claim each other's versions.
// =============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_completed_sessions_on_one_api_attribute_versions_by_provenance() {
    let team = unique("team");
    let api = unique("api-shared");
    let sess_a = unique("sess-a");
    let sess_b = unique("sess-b");
    let shared = ApiFixture {
        name: api.clone(),
        list_item: list_item(
            700,
            &api,
            "Shared",
            Value::Null,
            0,
            0,
            json!(3),
            Value::Null,
        ),
        specs: vec![
            // v3 produced by session B, v2 by session A, v1 learned with NO provenance.
            learned_spec_item(703, 3, None, 801),
            learned_spec_item(702, 2, None, 800),
            spec_item(701, 1, "learned", None),
        ],
        contents: Vec::new(),
    };
    let stub_state = StubState {
        team: team.clone(),
        sessions_status: 200,
        content_status: 200,
        sessions: vec![
            session_item(800, &sess_a, "completed", json!(uid(700)), json!(TS2)),
            session_item(801, &sess_b, "completed", json!(uid(700)), json!(TS2)),
        ],
        apis: vec![shared],
        requests: Mutex::new(Vec::new()),
    };
    let stub = start_stub(stub_state).await;
    let home = common::unique_tempdir();
    let dash = spawn_dashboard(home, &stub.base_url, &team);
    let http = client();

    let body = fetch(&http, &dash.sessions_partial_url())
        .await
        .text()
        .await
        .expect("sessions body");

    // Row A carries exactly v2; row B exactly v3; the provenance-less v1 links nowhere.
    let row_a = body.split(&sess_a).nth(1).expect("session A row");
    let row_a = row_a.split("</tr>").next().expect("row A cell");
    assert!(row_a.contains("v2"), "session A owns v2: {row_a}");
    assert!(
        !row_a.contains("v3"),
        "session A must not claim v3: {row_a}"
    );
    let row_b = body.split(&sess_b).nth(1).expect("session B row");
    let row_b = row_b.split("</tr>").next().expect("row B cell");
    assert!(row_b.contains("v3"), "session B owns v3: {row_b}");
    assert!(
        !row_b.contains("v2"),
        "session B must not claim v2: {row_b}"
    );
    assert!(
        !body.contains(&format!("{api} v1")),
        "a learned version without provenance is attributed to no session"
    );
}
