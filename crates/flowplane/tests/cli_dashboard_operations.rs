//! fpv2-55x.4 — `flowplane dashboard` Operations tab — black-box, spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against the
//! slice's documented contract — never the implementation. The dashboard is a read-only loopback
//! htmx server; every route lives under a per-launch nonce prefix. Contract under test:
//!
//!   * New page `GET /<nonce>/operations` — an HTML shell whose nav names the "Operations" tab
//!     (active) alongside the other tabs (Overview / Resources / APIs / Learning / AI / MCP, each
//!     a link). The shell itself performs NO upstream fetch; it carries one htmx-lazy panel
//!     (`#operations-nacks`) that hx-gets `/<nonce>/partials/operations/nacks`.
//!   * `GET /<nonce>/partials/operations/nacks` fetches the CP endpoint
//!     `GET /api/v1/teams/{team}/xds/nacks?since=<rfc3339>&limit=50` and renders a header
//!     ("<window_total> in last 24h") plus one table row per item (node_id, type_url,
//!     version_rejected, error_message, created_at). Each quarantined resource name renders as a
//!     LINK `<a href="/<nonce>/resources">`. A non-null `next_cursor` yields an "Older" control
//!     whose hx-get targets `/<nonce>/partials/operations/nacks?before=<percent-encoded cursor>`.
//!   * The dashboard sends an RFC-3339 `since` value (the computed now-24h instant), NEVER the
//!     literal shorthand "24h".
//!   * Degradation per the dashboard conventions (identical to every other tab): the upstream
//!     read 403 → a "Not authorized" panel (HTTP 200 body); 500 / malformed → an "unavailable"
//!     panel (HTTP 200 body); 401 → HTTP 286 (htmx stop-polling) naming `flowplane auth login`.
//!   * CRITICAL negatives: the bearer token never appears in any response body; every route lives
//!     under the nonce prefix and a path outside it 404s.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard child on
//! ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique team name; nothing
//! binds a fixed port. Every spawned server is killed via a Drop guard in all paths, including
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

/// A distinctive bearer token so any leak into a response body is unambiguous.
const SECRET_TOKEN: &str = "sekret-operations-tab-token-do-not-leak-7b2e";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the xds/nacks read model, canned
// failures, and a full request journal (path + query + auth + the status the stub answered).
// Unknown paths are recorded too and answered 404, so allowlist / negative assertions see every
// request the dashboard makes.
// =============================================================================================

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Raw query string (empty when absent).
    query: String,
    authorization: Option<String>,
    /// The HTTP status the stub answered with.
    #[allow(dead_code)]
    responded: u16,
}

impl Recorded {
    /// The percent-decoded `since` query param (if present).
    fn since(&self) -> Option<String> {
        query_param(&self.query, "since")
    }
}

/// Extract a single query parameter's percent-decoded value from a raw query string.
///
/// Deliberately NOT `serde_urlencoded` / axum `Query`: form decoding turns `+` into a space,
/// which would corrupt an RFC-3339 offset like `+00:00`. This decodes `%XX` escapes only and
/// leaves every other byte (including a literal `+`) untouched, faithfully recovering a
/// correctly percent-encoded timestamp.
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        if k == key {
            return Some(percent_decode(it.next().unwrap_or("")));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

struct StubState {
    team: String,
    /// Status for `xds/nacks` (200 = healthy) — the degradation lever.
    nacks_status: u16,
    /// When true, `xds/nacks` answers 200 with a NON-JSON body (the "malformed" lever).
    nacks_malformed: bool,
    /// When true, `xds/nacks` answers 200 with valid JSON that OMITS the required `items` field
    /// (a structurally-incomplete envelope) — must degrade to Unavailable, not a false "No NACKs".
    nacks_missing_items: bool,
    /// The windowed total the header renders.
    window_total: i64,
    /// The NACK items (each an object the dashboard renders as a row).
    items: Vec<Value>,
    /// `next_cursor`: a JSON string (paging available) or `Value::Null` (no more).
    next_cursor: Value,
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

fn route_request(state: &StubState, path: &str) -> Response {
    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();

    match segs.as_slice() {
        ["xds", "nacks"] => {
            if state.nacks_status != 200 {
                return canned_error(state.nacks_status);
            }
            if state.nacks_malformed {
                // A 200 with a body that is NOT valid JSON — the "malformed" degradation.
                return (StatusCode::OK, "this-is-not-json {{{").into_response();
            }
            if state.nacks_missing_items {
                // Valid JSON but the required `items` field is absent — structurally malformed.
                return Json(json!({ "window_total": state.window_total })).into_response();
            }
            Json(json!({
                "items": state.items,
                "window_total": state.window_total,
                "next_cursor": state.next_cursor,
            }))
            .into_response()
        }
        _ => canned_error(404),
    }
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let authorization = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let response = route_request(&state, &path);
    state.requests.lock().unwrap().push(Recorded {
        path,
        query,
        authorization,
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
// Canned payload builders (shapes as the real CP returns them; extra fields are harmless, the
// stub only needs what the dashboard reads).
// =============================================================================================

/// One NACK item exactly as the acceptance criteria specify the envelope element.
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

// Distinctive per-field markers so row assertions never collide with unrelated markup.
const NODE_A: &str = "node-alpha-77";
const NODE_B: &str = "node-beta-88";
const TYPE_A: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
const TYPE_B: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
const VER_A: &str = "ver-rej-5551";
const VER_B: &str = "ver-rej-6662";
const ERR_A: &str = "cluster-rejected-boom";
const ERR_B: &str = "listener-rejected-bang";
const QUARANTINED_A: &str = "orders-cluster-Q";
const DATE_A: &str = "2026-07-20";
const DATE_B: &str = "2026-07-21";

/// The two happy-path items — one carrying a quarantined resource, one with none.
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

/// A realistic keyset cursor: an RFC-3339 timestamp with a `+00:00` offset joined to a UUID by a
/// comma. Both the `+` and the `,` MUST be percent-encoded (`%2B`, `%2C`) to appear in a URL.
const NEXT_CURSOR: &str =
    "2026-07-25T00:00:00.000000000+00:00,019f9999-0000-7000-8000-000000000001";

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

    fn operations_shell_url(&self) -> String {
        self.page_url("operations")
    }

    fn nacks_partial_url(&self) -> String {
        self.page_url("partials/operations/nacks")
    }
}

/// Spawn `flowplane dashboard` with an isolated HOME and the standard env, read the single stdout
/// announcement line (30s timeout), and parse out port + nonce.
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
    // stderr → null: the server outlives this test's reads and an unread full pipe could block
    // the child.
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
// Shared assertion helpers.
// =============================================================================================

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

/// The recorded fetches of a given team-scoped path.
fn fetches_of<'a>(recorded: &'a [Recorded], team: &str, suffix: &str) -> Vec<&'a Recorded> {
    let path = format!("/api/v1/teams/{team}/{suffix}");
    recorded.iter().filter(|r| r.path == path).collect()
}

/// Assert that `name` renders inside an anchor pointing at `href`: the anchor open tag with the
/// href must appear in the ~300 chars immediately preceding the name's text. Markup-agnostic
/// about the anchor's other attributes.
fn assert_links_to(body: &str, name: &str, href: &str) {
    let idx = body
        .find(name)
        .unwrap_or_else(|| panic!("expected {name:?} to render; body:\n{body}"));
    let window_start = idx.saturating_sub(300);
    let window = &body[window_start..idx];
    assert!(
        window.contains(href),
        "the quarantined resource {name:?} must render as a LINK to {href:?}; \
         markup just before it:\n{window}"
    );
}

// =============================================================================================
// Fixture: a healthy NACK read model — window_total 7, two items (one quarantined), no paging.
// =============================================================================================

struct NacksFixture {
    stub_state: StubState,
    team: String,
}

fn nacks_fixture() -> NacksFixture {
    let team = unique("team");
    NacksFixture {
        stub_state: StubState {
            team: team.clone(),
            nacks_status: 200,
            nacks_malformed: false,
            nacks_missing_items: false,
            window_total: 7,
            items: nack_items(),
            next_cursor: Value::Null,
            requests: Mutex::new(Vec::new()),
        },
        team,
    }
}

// =============================================================================================
// Test 1: SHELL PAGE — GET /<nonce>/operations is a 200 HTML page whose nav names the
// "Operations" tab and links to the other tabs, carrying ONE lazy htmx panel wired to the nacks
// partial. CRITICAL negative: the shell itself performs NO upstream fetch and never leaks the
// token.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_shell_page_serves_nav_and_lazy_nacks_panel() {
    let fx = nacks_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let resp = fetch(&http, &dash.operations_shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/operations must serve the Operations shell page"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        shell.contains('<'),
        "the shell must be HTML; body:\n{shell}"
    );

    // Nav: the Operations tab plus every other tab.
    for tab in [
        "Overview",
        "Resources",
        "APIs",
        "Learning",
        "AI",
        "MCP",
        "Operations",
    ] {
        assert!(
            shell.contains(tab),
            "the nav must name the {tab:?} tab; body:\n{shell}"
        );
    }

    // The other tabs render as links (known page slugs prove the nav items are anchors).
    for slug in ["resources", "mcp"] {
        let href = format!("href=\"/{}/{}\"", dash.nonce, slug);
        assert!(
            shell.contains(&href),
            "the nav must render the other tabs as links (expected {href:?}); body:\n{shell}"
        );
    }

    // One lazy htmx panel wired to the nacks partial.
    assert!(
        shell.contains("id=\"operations-nacks\""),
        "the shell must carry the #operations-nacks panel; body:\n{shell}"
    );
    assert!(
        shell.contains("partials/operations/nacks"),
        "the shell must lazy-load /partials/operations/nacks; body:\n{shell}"
    );
    assert!(
        shell.contains("hx-get"),
        "the panel must load via htmx (hx-get); body:\n{shell}"
    );
    assert!(
        shell.contains("hx-trigger=\"load once\"") || shell.contains("hx-trigger=\"load\""),
        "the panel must fetch lazily on load; body:\n{shell}"
    );

    // Give any (incorrect) fire-and-forget upstream fetch a moment to land, then assert the shell
    // page triggered NONE.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let recorded = stub.recorded();
    assert!(
        fetches_of(&recorded, &fx.team, "xds/nacks").is_empty(),
        "the Operations shell page must perform NO upstream fetch — only the partial does; \
         recorded: {recorded:?}"
    );
    assert_bearer_and_no_leak(&recorded, &[&shell]);
}

// =============================================================================================
// Test 2: NACKS PARTIAL HAPPY — renders the header "<window_total> in last 24h" and one row per
// item (node_id / type_url / version_rejected / error_message / created_at). Each quarantined
// resource name renders as a link to /<nonce>/resources. Journal: the nacks endpoint fetched with
// bearer auth; no token leak.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_nacks_partial_renders_header_rows_and_quarantined_links() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.nacks_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the Operations nacks partial must be 200"
    );
    let body = resp.text().await.expect("nacks body");

    // Header: the windowed count in the fixed "<n> in last 24h" phrasing.
    assert!(
        body.contains("7 in last 24h"),
        "the header must show the windowed count as \"7 in last 24h\"; body:\n{body}"
    );

    // Both item rows render every field.
    for field in [
        NODE_A, NODE_B, TYPE_A, TYPE_B, VER_A, VER_B, ERR_A, ERR_B, DATE_A, DATE_B,
    ] {
        assert!(
            body.contains(field),
            "the nacks table must render {field:?}; body:\n{body}"
        );
    }

    // The quarantined resource renders as a LINK to the Resources tab.
    let resources_href = format!("/{}/resources", dash.nonce);
    assert_links_to(&body, QUARANTINED_A, &resources_href);

    // Journal: the nacks endpoint was fetched.
    let recorded = stub.recorded();
    assert!(
        !fetches_of(&recorded, &team, "xds/nacks").is_empty(),
        "the nacks partial must fetch xds/nacks; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert_bearer_and_no_leak(&recorded, &[&body]);
}

// =============================================================================================
// Test 3: SINCE IS RFC-3339 — the journaled xds/nacks request must carry a `since=` query param
// whose decoded value parses as an RFC-3339 timestamp — NEVER the literal shorthand "24h".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_nacks_request_since_is_rfc3339_not_shorthand() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.nacks_partial_url()).await;
    assert_eq!(resp.status().as_u16(), 200, "the nacks partial must be 200");
    let _ = resp.text().await.expect("nacks body");

    let recorded = stub.recorded();
    let nacks = fetches_of(&recorded, &team, "xds/nacks");
    assert!(
        !nacks.is_empty(),
        "the nacks partial must fetch xds/nacks; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );

    let since = nacks[0].since().unwrap_or_else(|| {
        panic!(
            "the xds/nacks request must carry a `since=` query param; query was: {:?}",
            nacks[0].query
        )
    });
    assert_ne!(
        since, "24h",
        "the dashboard must send an RFC-3339 instant as `since`, never the literal shorthand \
         \"24h\""
    );
    assert!(
        chrono::DateTime::parse_from_rfc3339(&since).is_ok(),
        "the `since` value must parse as an RFC-3339 timestamp; got {since:?}"
    );

    assert_bearer_and_no_leak(&recorded, &[]);
}

// =============================================================================================
// Test 4: NEXT_CURSOR PAGING — a non-null next_cursor yields an "Older" control whose hx-get
// targets /<nonce>/partials/operations/nacks?before=<percent-encoded cursor>; a null next_cursor
// yields no such control.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_nacks_partial_paging_control_follows_next_cursor() {
    let http = client();

    // Non-null next_cursor → an "Older" control with an encoded `before=` cursor.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.next_cursor = json!(NEXT_CURSOR);
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(resp.status().as_u16(), 200, "the nacks partial must be 200");
        let body = resp.text().await.expect("nacks body");

        assert!(
            body.contains("partials/operations/nacks?before="),
            "a non-null next_cursor must render an \"Older\" control whose hx-get targets \
             /partials/operations/nacks?before=<cursor>; body:\n{body}"
        );
        // The cursor is percent-encoded: the `+` offset and `,` separator must appear as escapes,
        // never as the raw characters (a raw `+` in a query decodes to a space).
        assert!(
            body.contains("%2B") && body.contains("%2C"),
            "the `before` cursor must be percent-encoded (expected %2B for `+` and %2C for `,`); \
             body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // Null next_cursor → no paging control.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let stub = start_stub(fx.stub_state).await; // fixture default: next_cursor is Null
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(resp.status().as_u16(), 200, "the nacks partial must be 200");
        let body = resp.text().await.expect("nacks body");

        assert!(
            !body.contains("partials/operations/nacks?before="),
            "a null next_cursor must NOT render an \"Older\" paging control; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }
}

// =============================================================================================
// Test 5: NACKS PARTIAL DEGRADATION — mirrors the dashboard's conventions exactly (as every other
// tab does): xds/nacks 403 → a "Not authorized" panel (HTTP 200); 500 → an "unavailable" panel
// (HTTP 200); a 200-but-malformed body → an "unavailable" panel (HTTP 200); xds/nacks 401 → HTTP
// 286 (htmx stop-polling) naming `flowplane auth login`. No token leak in any body.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_nacks_partial_degrades_per_dashboard_conventions() {
    let http = client();

    // xds/nacks 403 → not-authorized panel, HTTP 200.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.nacks_status = 403;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 403 must not fail the nacks partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("not authorized"),
            "the nacks partial must say \"Not authorized\" on upstream 403; body:\n{body}"
        );
        assert!(
            !body.contains("in last 24h"),
            "no nacks data may render on 403; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // xds/nacks 500 → unavailable panel, HTTP 200.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.nacks_status = 500;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 500 must not fail the nacks partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "the nacks partial must render an \"unavailable\" state on upstream 500; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // xds/nacks 200 with a malformed (non-JSON) body → unavailable panel, HTTP 200.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.nacks_malformed = true;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "a malformed upstream body must not fail the nacks partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "the nacks partial must render an \"unavailable\" state on a malformed body; \
             body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // xds/nacks 200 with valid JSON but MISSING the required `items` field → unavailable panel
    // (never a deceptive "No NACKs"). A structurally-incomplete envelope is malformed.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.nacks_missing_items = true;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "a response missing the required `items` field must render \"unavailable\", not an \
             empty \"No NACKs\" table; body:\n{body}"
        );
        assert!(
            !body.contains("No NACKs"),
            "a malformed (missing items) envelope must NOT render the honest empty state; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // xds/nacks 401 → HTTP 286 naming `flowplane auth login`.
    {
        let fx = nacks_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.nacks_status = 401;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.nacks_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            286,
            "upstream 401 on xds/nacks must yield the htmx stop-polling status 286"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.contains("flowplane auth login"),
            "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }
}

// =============================================================================================
// Test 6: NEGATIVE — every route lives under the nonce prefix; a path OUTSIDE it 404s, and no
// such response leaks the bearer token.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_routes_live_only_under_the_nonce_prefix() {
    let fx = nacks_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // Sanity: the correctly-prefixed shell IS served.
    let ok = fetch(&http, &dash.operations_shell_url()).await;
    assert_eq!(
        ok.status().as_u16(),
        200,
        "the nonce-prefixed Operations page must be served"
    );

    // Paths outside the nonce prefix must 404.
    let outside = [
        format!("http://127.0.0.1:{}/operations", dash.port),
        format!("http://127.0.0.1:{}/partials/operations/nacks", dash.port),
        format!(
            "http://127.0.0.1:{}/{}/operations",
            dash.port, "00000000000000000000000000000000"
        ),
    ];
    for url in outside {
        let resp = fetch(&http, &url).await;
        assert_eq!(
            resp.status().as_u16(),
            404,
            "a path outside the per-launch nonce prefix must 404: {url}"
        );
        let body = resp.text().await.expect("body");
        assert!(
            !body.contains(SECRET_TOKEN),
            "a 404 body must never leak the bearer token; url {url}, body:\n{body}"
        );
    }
}
