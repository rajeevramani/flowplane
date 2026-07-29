//! fpv2-zl8.5 — `flowplane dashboard` MCP tab — black-box, spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. The dashboard is a read-only
//! loopback htmx server; every route lives under a per-launch nonce prefix. Contract under
//! test:
//!
//!   * New page `GET /<nonce>/mcp` — an HTML shell whose nav names the "MCP" tab (active)
//!     alongside the other tabs (Overview / Resources / APIs / Learning / AI), and which
//!     carries two lazy htmx panels that hx-get `/<nonce>/partials/mcp/status` and
//!     `/<nonce>/partials/mcp/tools`. The shell itself performs NO upstream fetch.
//!   * `GET /<nonce>/partials/mcp/status` renders the status card + connections table. It
//!     fetches the CP endpoints `GET /api/v1/teams/{team}/mcp/status` and
//!     `GET /api/v1/teams/{team}/mcp/connections`, rendering the real transport / protocol
//!     strings, the connection rows, and labelling the connections as attributed to THIS
//!     control-plane node.
//!   * `GET /<nonce>/partials/mcp/tools` renders CP-tools + API-tools panels. It fetches
//!     `GET /api/v1/teams/{team}/mcp/tools?include_disabled=true`, and on 403 falls back to
//!     `GET /api/v1/teams/{team}/mcp/tools` (enabled-only), surfacing a hint that the disabled
//!     tools require the `mcp-tools:update` grant.
//!   * Degradation per the dashboard conventions (identical to every other tab): the primary
//!     read 403 → a "Not authorized" panel (HTTP 200 body); 500 / malformed → an
//!     "unavailable" panel (HTTP 200 body); 401 → HTTP 286 (htmx stop-polling) naming
//!     `flowplane auth login`.
//!   * CRITICAL negative: the bearer token never appears in any response body; no upstream
//!     request ever targets a secret/value route.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique team
//! name; nothing binds a fixed port. Every spawned server is killed via a Drop guard in all
//! paths, including assertion failures.

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
const SECRET_TOKEN: &str = "sekret-mcp-tab-token-do-not-leak-9f3c";

/// A fixed connection id used by the happy-path fixture (spec: a uuid).
const CONNECTION_ID: &str = "5f3a9c21-0000-4000-8000-000000000abc";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the MCP read model
// (status / connections / tools), canned failures, and a full request journal (path + query +
// auth + the status the stub answered). Unknown paths are recorded too and answered 404, so
// allowlist / negative assertions see every request the dashboard makes.
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
    fn tools_query(&self) -> ToolsQuery {
        parse_query(&self.query)
    }
}

/// Parse a raw query string through axum's `Query` extractor (percent-decoding included).
fn parse_query<T: serde::de::DeserializeOwned + Default>(query: &str) -> T {
    let uri: axum::http::Uri = format!("/q?{query}").parse().expect("query string parses");
    axum::extract::Query::<T>::try_from_uri(&uri)
        .map(|q| q.0)
        .unwrap_or_default()
}

/// The tools endpoint's query contract: an optional `include_disabled` flag.
#[derive(Debug, Default, Clone, serde::Deserialize)]
struct ToolsQuery {
    include_disabled: Option<bool>,
}

struct StubState {
    team: String,
    /// Status for the `mcp/status` endpoint (200 = healthy) — the status-partial degradation
    /// lever.
    status_status: u16,
    /// When true, `mcp/status` answers 200 with a NON-JSON body (the "malformed" lever).
    status_malformed: bool,
    status_body: Value,
    connections_status: u16,
    connections: Vec<Value>,
    /// Status for `mcp/tools?include_disabled=true` (200 = healthy). 403 exercises the
    /// enabled-only fallback.
    tools_with_disabled_status: u16,
    tools_with_disabled: Vec<Value>,
    /// Status + body for `mcp/tools` WITHOUT the include_disabled flag (the fallback fetch).
    tools_enabled_status: u16,
    tools_enabled: Vec<Value>,
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

fn route_request(state: &StubState, path: &str, query: &str) -> Response {
    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();

    match segs.as_slice() {
        ["mcp", "status"] => {
            if state.status_status != 200 {
                return canned_error(state.status_status);
            }
            if state.status_malformed {
                // A 200 with a body that is NOT valid JSON — the "malformed" degradation.
                return (StatusCode::OK, "this-is-not-json {{{").into_response();
            }
            Json(state.status_body.clone()).into_response()
        }
        ["mcp", "connections"] => {
            if state.connections_status != 200 {
                return canned_error(state.connections_status);
            }
            // Spec shape: a bare array of connection objects.
            Json(Value::Array(state.connections.clone())).into_response()
        }
        ["mcp", "tools"] => {
            let tq: ToolsQuery = parse_query(query);
            if tq.include_disabled == Some(true) {
                if state.tools_with_disabled_status != 200 {
                    return canned_error(state.tools_with_disabled_status);
                }
                Json(Value::Array(state.tools_with_disabled.clone())).into_response()
            } else {
                if state.tools_enabled_status != 200 {
                    return canned_error(state.tools_enabled_status);
                }
                Json(Value::Array(state.tools_enabled.clone())).into_response()
            }
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

    let response = route_request(&state, &path, &query);
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

/// The `mcp/status` object exactly as the acceptance criteria specify it.
fn mcp_status_body() -> Value {
    json!({
        "transport": "streamable_http_post",
        "preferred_protocol_version": "2025-11-25",
        "supported_protocol_versions": ["2025-11-25", "2025-03-26"],
        "session_ttl_seconds": 3600,
        "active_sessions": 2,
        "static_tool_count": 35,
        "dynamic_enabled_tool_count": 3,
        "tools_list_changed": false,
        "sse_enabled": false,
        "resources_enabled": false,
        "prompts_enabled": false,
        "api_invocation_mode": "gateway_invocation_descriptor",
    })
}

fn connection_row(connection_id: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "principal_kind": "user",
        "transport": "streamable_http_post",
        "sse": false,
        "age_seconds": 120,
        "idle_seconds": 5,
    })
}

#[allow(clippy::too_many_arguments)]
fn tool_row(
    name: &str,
    description: &str,
    resource: &str,
    action: &str,
    risk: &str,
    kind: &str,
    enabled: bool,
    executable_by_caller: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "resource": resource,
        "action": action,
        "risk": risk,
        "kind": kind,
        "enabled": enabled,
        "executable_by_caller": executable_by_caller,
    })
}

// Fixed tool names (per the acceptance criteria).
const CP_TOOL: &str = "cp_clusters_list";
const API_TOOL_ENABLED: &str = "api_orders_list";
const API_TOOL_DISABLED: &str = "api_orders_delete";

/// The three rows served on `mcp/tools?include_disabled=true` — a static CP tool, an enabled
/// dynamic API tool, and a DISABLED dynamic API tool.
fn tools_with_disabled() -> Vec<Value> {
    vec![
        tool_row(
            CP_TOOL,
            "List clusters",
            "clusters",
            "read",
            "read",
            "static",
            true,
            true,
        ),
        tool_row(
            API_TOOL_ENABLED,
            "GET /orders",
            "mcp-tools",
            "execute",
            "mutate",
            "dynamic",
            true,
            true,
        ),
        tool_row(
            API_TOOL_DISABLED,
            "DELETE /orders",
            "mcp-tools",
            "execute",
            "mutate",
            "dynamic",
            false,
            false,
        ),
    ]
}

/// The enabled-only subset served on `mcp/tools` (no include_disabled) — the fallback body.
fn tools_enabled_only() -> Vec<Value> {
    vec![
        tool_row(
            CP_TOOL,
            "List clusters",
            "clusters",
            "read",
            "read",
            "static",
            true,
            true,
        ),
        tool_row(
            API_TOOL_ENABLED,
            "GET /orders",
            "mcp-tools",
            "execute",
            "mutate",
            "dynamic",
            true,
            true,
        ),
    ]
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

    fn mcp_shell_url(&self) -> String {
        self.page_url("mcp")
    }

    fn status_partial_url(&self) -> String {
        self.page_url("partials/mcp/status")
    }

    fn tools_partial_url(&self) -> String {
        self.page_url("partials/mcp/tools")
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
// Shared assertion helpers.
// =============================================================================================

/// CRITICAL negative: no upstream request ever targets a secret/value route.
fn assert_no_secret_paths(recorded: &[Recorded]) {
    for req in recorded {
        let full = format!("{}?{}", req.path, req.query).to_lowercase();
        assert!(
            !full.contains("secret") && !full.contains("/value"),
            "upstream request must never target a secret/value route: {full:?}"
        );
    }
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

/// The recorded fetches of a given team-scoped path.
fn fetches_of<'a>(recorded: &'a [Recorded], team: &str, suffix: &str) -> Vec<&'a Recorded> {
    let path = format!("/api/v1/teams/{team}/{suffix}");
    recorded.iter().filter(|r| r.path == path).collect()
}

/// The rendered fragment for one tool: from the tool's name up to the next tool name that
/// renders after it (or the end of the body). Markup-agnostic — works whether tools render as
/// table rows, list items, or cards, so it never assumes the panel's exact HTML shape.
fn tool_segment<'a>(body: &'a str, marker: &str, others: &[&str]) -> &'a str {
    let start = body
        .find(marker)
        .unwrap_or_else(|| panic!("expected the tool {marker:?} to render; body:\n{body}"));
    let after_marker = start + marker.len();
    let mut end = body.len();
    for other in others {
        if let Some(rel) = body[after_marker..].find(other) {
            end = end.min(after_marker + rel);
        }
    }
    &body[start..end]
}

// =============================================================================================
// Fixture: a healthy MCP read model — the full status object, one node-local connection, and
// the three-tool catalog (static CP tool + enabled/disabled dynamic API tools).
// =============================================================================================

struct McpFixture {
    stub_state: StubState,
    team: String,
}

fn mcp_fixture() -> McpFixture {
    let team = unique("team");
    McpFixture {
        stub_state: StubState {
            team: team.clone(),
            status_status: 200,
            status_malformed: false,
            status_body: mcp_status_body(),
            connections_status: 200,
            connections: vec![connection_row(CONNECTION_ID)],
            tools_with_disabled_status: 200,
            tools_with_disabled: tools_with_disabled(),
            tools_enabled_status: 200,
            tools_enabled: tools_enabled_only(),
            requests: Mutex::new(Vec::new()),
        },
        team,
    }
}

// =============================================================================================
// Test 1: SHELL PAGE — GET /<nonce>/mcp is a 200 HTML page whose nav names the "MCP" tab and
// links to the other tabs, carrying two lazy htmx panels wired to the status + tools partials.
// CRITICAL negative: the shell itself performs NO upstream fetch and never leaks the token.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_shell_page_serves_nav_and_two_lazy_partial_panels() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let resp = fetch(&http, &dash.mcp_shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/mcp must serve the MCP shell page"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        shell.contains('<'),
        "the shell must be HTML; body:\n{shell}"
    );

    // Nav: the MCP tab plus links to every other tab.
    for tab in ["Overview", "Resources", "APIs", "Learning", "AI", "MCP"] {
        assert!(
            shell.contains(tab),
            "the nav must name the {tab:?} tab; body:\n{shell}"
        );
    }

    // Two lazy htmx panels wired to the status + tools partials.
    assert!(
        shell.contains("partials/mcp/status"),
        "the shell must lazy-load /partials/mcp/status; body:\n{shell}"
    );
    assert!(
        shell.contains("partials/mcp/tools"),
        "the shell must lazy-load /partials/mcp/tools; body:\n{shell}"
    );
    assert!(
        shell.contains("hx-get"),
        "the panels must load via htmx (hx-get); body:\n{shell}"
    );
    assert!(
        shell.contains("hx-trigger=\"load once\""),
        "the panels must fetch on load, once; body:\n{shell}"
    );

    // Give any (incorrect) fire-and-forget upstream fetch a moment to land, then assert the
    // shell page triggered NONE.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let recorded = stub.recorded();
    assert!(
        recorded.is_empty(),
        "the MCP shell page must perform NO upstream fetch — only the partials do; \
         recorded: {recorded:?}"
    );
    assert_bearer_and_no_leak(&recorded, &[&shell]);
}

// =============================================================================================
// Test 2: STATUS PARTIAL HAPPY — renders the real transport + protocol strings from
// `mcp/status`, the connection row (principal "user") from `mcp/connections`, and labels the
// connections as attributed to THIS control-plane node. Journal: both endpoints fetched with
// bearer auth; no token leak; no secret path.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_status_partial_renders_transport_protocol_and_node_local_connections() {
    let fx = mcp_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.status_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the MCP status partial must be 200"
    );
    let body = resp.text().await.expect("status body");

    // The real transport string and preferred protocol version render verbatim.
    assert!(
        body.contains("streamable_http_post"),
        "the status card must render the real transport string \"streamable_http_post\"; \
         body:\n{body}"
    );
    assert!(
        body.contains("2025-11-25"),
        "the status card must render the preferred protocol version \"2025-11-25\"; \
         body:\n{body}"
    );

    // The single connection row: principal "user" renders.
    assert!(
        body.contains("user"),
        "the connections table must render the connection's principal kind \"user\"; \
         body:\n{body}"
    );

    // The connections are labelled as attributed to THIS control-plane node.
    assert!(
        body.to_lowercase().contains("node"),
        "the connections panel must label the connections as node-local (attributed to this \
         control-plane node); body:\n{body}"
    );

    // Journal: both the status and connections endpoints were fetched.
    let recorded = stub.recorded();
    assert!(
        !fetches_of(&recorded, &team, "mcp/status").is_empty(),
        "the status partial must fetch mcp/status; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert!(
        !fetches_of(&recorded, &team, "mcp/connections").is_empty(),
        "the status partial must fetch mcp/connections; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert_no_secret_paths(&recorded);
    assert_bearer_and_no_leak(&recorded, &[&body]);
}

// =============================================================================================
// Test 3: TOOLS PARTIAL HAPPY (disabled visible) — the partial requests
// mcp/tools?include_disabled=true, renders the static CP tool in the CP panel and both dynamic
// API tools in the API panel; the DISABLED row is visibly marked disabled and shows executable
// "no", while the enabled row shows executable "yes".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tools_partial_shows_disabled_tools_with_markers_and_executability() {
    let fx = mcp_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.tools_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the MCP tools partial must be 200"
    );
    let body = resp.text().await.expect("tools body");

    // CP-tools panel shows the static tool; API-tools panel shows both dynamic tools.
    assert!(
        body.contains(CP_TOOL),
        "the CP-tools panel must show {CP_TOOL:?}; body:\n{body}"
    );
    assert!(
        body.contains(API_TOOL_ENABLED) && body.contains(API_TOOL_DISABLED),
        "the API-tools panel must show both {API_TOOL_ENABLED:?} and {API_TOOL_DISABLED:?}; \
         body:\n{body}"
    );

    // The disabled row is visibly marked disabled and shows executable "no".
    let disabled_seg = tool_segment(&body, API_TOOL_DISABLED, &[CP_TOOL, API_TOOL_ENABLED]);
    assert!(
        disabled_seg.to_lowercase().contains("disabled"),
        "the disabled tool's row must be VISIBLY marked disabled; row:\n{disabled_seg}"
    );
    assert!(
        disabled_seg.to_lowercase().contains("no"),
        "the disabled tool's row must show executable \"no\"; row:\n{disabled_seg}"
    );

    // The enabled dynamic row shows executable "yes".
    let enabled_seg = tool_segment(&body, API_TOOL_ENABLED, &[CP_TOOL, API_TOOL_DISABLED]);
    assert!(
        enabled_seg.to_lowercase().contains("yes"),
        "the enabled tool's row must show executable \"yes\"; row:\n{enabled_seg}"
    );

    // Journal: the tools fetch actually carried include_disabled=true.
    let recorded = stub.recorded();
    let tools = fetches_of(&recorded, &team, "mcp/tools");
    assert!(
        tools
            .iter()
            .any(|r| r.tools_query().include_disabled == Some(true)),
        "the tools partial must request mcp/tools?include_disabled=true; recorded tool \
         queries: {:?}",
        tools.iter().map(|r| r.query.clone()).collect::<Vec<_>>()
    );
    assert_no_secret_paths(&recorded);
    assert_bearer_and_no_leak(&recorded, &[&body]);
}

// =============================================================================================
// Test 4: TOOLS PARTIAL 403 FALLBACK — mcp/tools?include_disabled=true → 403, so the partial
// falls back to mcp/tools (enabled-only). It still renders 200 with the enabled tools, NOT the
// disabled row, and surfaces a hint that disabled tools require the mcp-tools:update grant.
// This is the read-only-principal-keeps-the-catalog case.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tools_partial_falls_back_to_enabled_only_on_403() {
    let fx = mcp_fixture();
    let team = fx.team.clone();
    let mut state = fx.stub_state;
    state.tools_with_disabled_status = 403;
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.tools_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a 403 on the include_disabled fetch must NOT fail the tools partial — it falls back"
    );
    let body = resp.text().await.expect("tools body");

    // Enabled tools still render.
    assert!(
        body.contains(CP_TOOL) && body.contains(API_TOOL_ENABLED),
        "the fallback must still render the enabled tools; body:\n{body}"
    );
    // The disabled tool is NOT shown (the fallback fetch is enabled-only).
    assert!(
        !body.contains(API_TOOL_DISABLED),
        "the enabled-only fallback must NOT show the disabled tool {API_TOOL_DISABLED:?}; \
         body:\n{body}"
    );
    // A hint that disabled tools require the mcp-tools:update grant.
    let lower = body.to_lowercase();
    assert!(
        lower.contains("mcp-tools:update") || lower.contains("enabled tools only"),
        "the fallback must hint that disabled tools require the mcp-tools:update grant \
         (\"mcp-tools:update\" or \"enabled tools only\"); body:\n{body}"
    );

    // Journal: the include_disabled=true fetch got 403, and an enabled-only fetch followed.
    let recorded = stub.recorded();
    let tools = fetches_of(&recorded, &team, "mcp/tools");
    assert!(
        tools
            .iter()
            .any(|r| r.tools_query().include_disabled == Some(true) && r.responded == 403),
        "the tools partial must first attempt include_disabled=true (answered 403); \
         recorded tool fetches: {tools:?}"
    );
    assert!(
        tools
            .iter()
            .any(|r| r.tools_query().include_disabled != Some(true) && r.responded == 200),
        "the tools partial must fall back to an enabled-only fetch (no include_disabled=true); \
         recorded tool fetches: {tools:?}"
    );
    assert_no_secret_paths(&recorded);
    assert_bearer_and_no_leak(&recorded, &[&body]);
}

// =============================================================================================
// Test 5: STATUS PARTIAL DEGRADATION — mirrors the dashboard's conventions exactly (as every
// other tab does): mcp/status 403 → a "Not authorized" panel (HTTP 200); a 200-but-malformed
// body → an "unavailable" panel (HTTP 200); mcp/status 401 → HTTP 286 (htmx stop-polling)
// naming `flowplane auth login`. No token leak in any body.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_status_partial_degrades_per_dashboard_conventions() {
    let http = client();

    // mcp/status 403 → not-authorized panel, HTTP 200.
    {
        let fx = mcp_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.status_status = 403;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.status_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 403 must not fail the status partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("not authorized"),
            "the status partial must say \"Not authorized\" on upstream 403; body:\n{body}"
        );
        assert!(
            !body.contains("streamable_http_post"),
            "no status data may render on 403; body:\n{body}"
        );
        assert_no_secret_paths(&stub.recorded());
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // mcp/status 200 with a malformed (non-JSON) body → unavailable panel, HTTP 200.
    {
        let fx = mcp_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.status_malformed = true;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.status_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "a malformed upstream body must not fail the status partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "the status partial must render an \"unavailable\" state on a malformed body; \
             body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // mcp/status 401 → HTTP 286 naming `flowplane auth login`.
    {
        let fx = mcp_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.status_status = 401;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.status_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            286,
            "upstream 401 on mcp/status must yield the htmx stop-polling status 286"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.contains("flowplane auth login"),
            "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }
}
