//! fpv2-5kn.5 — `flowplane dashboard` MCP tab — AGENTS panel — black-box, spec-driven suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against the
//! slice's documented contract — never the implementation. The dashboard is a read-only loopback
//! htmx server; every route lives under a per-launch nonce prefix. Contract under test (S5):
//!
//!   * `GET /<nonce>/partials/mcp/agents` renders a read-only, ORG-WIDE agents panel: a header
//!     naming it "organization-wide · requires org authority", a static credential note, and a
//!     table of agents (name / kind / status) fetched from the CP `GET /api/v1/agents` (a
//!     `Page<AgentView>` envelope — the dashboard decodes `items`). Each row carries a
//!     `<details>` expand wired (hx-get) to the agent-grants partial with that agent's id.
//!   * `GET /<nonce>/partials/mcp/agent-grants?id=<agent_id>` renders that agent's grants
//!     (team_name / resource / action) fetched from the CP `GET /api/v1/agents/{id}/grants`
//!     (a `Page<AgentGrantView>` envelope — decode `items`).
//!   * Panel degradation (the standard `Panel::{Data,Unauthorized,Unavailable}` contract):
//!     CP 403 → `Panel::Unauthorized` — the panel shows the org-authority note, NOT a silently
//!     empty table and NOT a 500; a decode/transport failure → `Panel::Unavailable`, never a
//!     silently empty table.
//!   * The MCP tab shell (`GET /<nonce>/mcp`) lazy-loads the agents section via an hx-get to
//!     `/partials/mcp/agents`.
//!   * CRITICAL negative: the bearer token never appears in any response body; the rendered
//!     agents/grants HTML contains no credential value (the mock returns none — assert none
//!     surfaces).
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard child
//! on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique team name;
//! nothing binds a fixed port. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

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
const SECRET_TOKEN: &str = "sekret-agents-panel-token-do-not-leak-4d1a";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Deterministic agent/grant id (a valid uuid) with no accidental digit collisions.
fn uid(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the ORG-WIDE agents read model
// (`/api/v1/agents`, `/api/v1/agents/{id}/grants`), canned failures, and a full request journal
// (path + query + auth + the status the stub answered). Unknown paths are recorded too and
// answered 404, so allowlist / negative assertions see every request the dashboard makes.
// =============================================================================================

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Raw query string (empty when absent).
    #[allow(dead_code)]
    query: String,
    authorization: Option<String>,
    /// The HTTP status the stub answered with.
    #[allow(dead_code)]
    responded: u16,
}

struct StubState {
    /// Status for `GET /api/v1/agents` (200 = healthy) — the agents-panel degradation lever.
    agents_status: u16,
    /// When true, `GET /api/v1/agents` answers 200 with a NON-JSON body (the "unavailable"
    /// decode-failure lever).
    agents_malformed: bool,
    /// The agent items served in the `Page` envelope.
    agents: Vec<Value>,
    /// Status for `GET /api/v1/agents/{id}/grants` (200 = healthy).
    grants_status: u16,
    /// The grant items served in the `Page` envelope.
    grants: Vec<Value>,
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

/// Wrap `items` in the uniform `Page` envelope the CP list endpoints return.
fn page(items: &[Value]) -> Response {
    Json(json!({
        "items": items.to_vec(),
        "total": items.len(),
        "limit": 500,
        "offset": 0,
    }))
    .into_response()
}

fn route_request(state: &StubState, path: &str) -> Response {
    // Strip the leading slash and split into segments.
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    match segs.as_slice() {
        // `GET /api/v1/agents` — the ORG-WIDE agents list (NOT team-scoped).
        ["api", "v1", "agents"] => {
            if state.agents_status != 200 {
                return canned_error(state.agents_status);
            }
            if state.agents_malformed {
                // A 200 with a body that is NOT valid JSON — the decode-failure degradation.
                return (StatusCode::OK, "this-is-not-json {{{").into_response();
            }
            page(&state.agents)
        }
        // `GET /api/v1/agents/{id}/grants` — one agent's grants.
        ["api", "v1", "agents", _id, "grants"] => {
            if state.grants_status != 200 {
                return canned_error(state.grants_status);
            }
            page(&state.grants)
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
// Canned payload builders (shapes as the real CP `AgentView` / `AgentGrantView` return them;
// extra fields are harmless — the stub only needs what the dashboard reads).
// =============================================================================================

fn agent_item(id: u64, name: &str, kind: &str, status: &str) -> Value {
    json!({
        "id": uid(id),
        "org_id": uid(1),
        "name": name,
        "kind": kind,
        "status": status,
    })
}

fn grant_item(id: u64, team_id: u64, team_name: &str, resource: &str, action: &str) -> Value {
    json!({
        "id": uid(id),
        "team_id": uid(team_id),
        "team_name": team_name,
        "resource": resource,
        "action": action,
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

    fn mcp_shell_url(&self) -> String {
        self.page_url("mcp")
    }

    fn agents_partial_url(&self) -> String {
        self.page_url("partials/mcp/agents")
    }

    fn agent_grants_partial_url(&self, id: &str) -> String {
        self.page_url(&format!("partials/mcp/agent-grants?id={id}"))
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

/// The org-scope header contract: the panel names itself organization-wide and flags that it
/// requires org authority. Asserted as two substrings so the exact separator glyph (a middot in
/// the spec) is not load-bearing — both halves of the header must render.
fn assert_org_scope_header(body: &str) {
    let lower = body.to_lowercase();
    assert!(
        lower.contains("organization-wide"),
        "the agents panel header must name itself \"organization-wide\"; body:\n{body}"
    );
    assert!(
        lower.contains("requires org authority"),
        "the agents panel header must flag \"requires org authority\"; body:\n{body}"
    );
}

/// The fragment of `body` that renders one agent's row: from the agent's name up to the next
/// agent name that renders after it (or the end of the body). Markup-agnostic.
fn agent_segment<'a>(body: &'a str, marker: &str, others: &[&str]) -> &'a str {
    let start = body
        .find(marker)
        .unwrap_or_else(|| panic!("expected the agent {marker:?} to render; body:\n{body}"));
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
// Fixture: two agents (distinct name / kind / status) and a two-row grant listing.
// =============================================================================================

struct AgentsFixture {
    stub_state: StubState,
    team: String,
    agent_alpha: String,
    agent_bravo: String,
    grant_team_a: String,
    grant_team_b: String,
}

// Fixed agent ids so tests can address the agent-grants partial deterministically.
const AGENT_ALPHA_ID: u64 = 100;
const AGENT_BRAVO_ID: u64 = 101;

// The two agents' kinds and statuses (asserted verbatim in the happy path).
const ALPHA_KIND: &str = "cp-tool";
const ALPHA_STATUS: &str = "active";
const BRAVO_KIND: &str = "api-consumer";
const BRAVO_STATUS: &str = "disabled";

// The two grant rows' resource/action (asserted verbatim on expand).
const GRANT_A_RESOURCE: &str = "clusters";
const GRANT_A_ACTION: &str = "read";
const GRANT_B_RESOURCE: &str = "routes";
const GRANT_B_ACTION: &str = "execute";

fn agents_fixture() -> AgentsFixture {
    let team = unique("team");
    let agent_alpha = unique("agent-alpha");
    let agent_bravo = unique("agent-bravo");
    let grant_team_a = unique("grant-team-a");
    let grant_team_b = unique("grant-team-b");

    let agents = vec![
        agent_item(AGENT_ALPHA_ID, &agent_alpha, ALPHA_KIND, ALPHA_STATUS),
        agent_item(AGENT_BRAVO_ID, &agent_bravo, BRAVO_KIND, BRAVO_STATUS),
    ];
    let grants = vec![
        grant_item(200, 300, &grant_team_a, GRANT_A_RESOURCE, GRANT_A_ACTION),
        grant_item(201, 301, &grant_team_b, GRANT_B_RESOURCE, GRANT_B_ACTION),
    ];

    AgentsFixture {
        stub_state: StubState {
            agents_status: 200,
            agents_malformed: false,
            agents,
            grants_status: 200,
            grants,
            requests: Mutex::new(Vec::new()),
        },
        team,
        agent_alpha,
        agent_bravo,
        grant_team_a,
        grant_team_b,
    }
}

// =============================================================================================
// Test 1: AGENTS PANEL HAPPY — the partial renders the org-scope header, a static credential
// note, and both agents' name / kind / status from the CP `Page<AgentView>` envelope. Each row
// wires its `<details>` expand (hx-get) to the agent-grants partial with that agent's id. The
// panel fetches ONLY `/api/v1/agents` (grants are lazy, on expand). Bearer auth on every
// upstream request; no token leak; no secret path.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_partial_renders_org_scope_header_and_agent_rows() {
    let fx = agents_fixture();
    let team = fx.team.clone();
    let alpha = fx.agent_alpha.clone();
    let bravo = fx.agent_bravo.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.agents_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the MCP agents partial must be 200"
    );
    let body = resp.text().await.expect("agents body");

    // Org-scope header.
    assert_org_scope_header(&body);

    // A static credential note (a note ABOUT agent credentials — asserted by keyword so the
    // exact prose is not load-bearing).
    assert!(
        body.to_lowercase().contains("credential"),
        "the agents panel must render a static credential note; body:\n{body}"
    );

    // Both agents render by name, and each row shows its kind + status.
    let alpha_row = agent_segment(&body, &alpha, &[&bravo]);
    assert!(
        alpha_row.contains(ALPHA_KIND),
        "agent {alpha:?} must render its kind {ALPHA_KIND:?}; row:\n{alpha_row}"
    );
    assert!(
        alpha_row.contains(ALPHA_STATUS),
        "agent {alpha:?} must render its status {ALPHA_STATUS:?}; row:\n{alpha_row}"
    );
    let bravo_row = agent_segment(&body, &bravo, &[&alpha]);
    assert!(
        bravo_row.contains(BRAVO_KIND),
        "agent {bravo:?} must render its kind {BRAVO_KIND:?}; row:\n{bravo_row}"
    );
    assert!(
        bravo_row.contains(BRAVO_STATUS),
        "agent {bravo:?} must render its status {BRAVO_STATUS:?}; row:\n{bravo_row}"
    );

    // Each row's expand is wired (hx-get) to the agent-grants partial with THAT agent's id.
    assert!(
        body.contains("partials/mcp/agent-grants"),
        "the agents panel must wire each row's expand to the agent-grants partial; body:\n{body}"
    );
    assert!(
        body.contains(&uid(AGENT_ALPHA_ID)) && body.contains(&uid(AGENT_BRAVO_ID)),
        "each row's expand must carry its own agent id ({} and {}); body:\n{body}",
        uid(AGENT_ALPHA_ID),
        uid(AGENT_BRAVO_ID)
    );
    // The alpha row specifically must reference its own id near its grants expand.
    assert!(
        alpha_row.contains(&uid(AGENT_ALPHA_ID)),
        "the {alpha:?} row's expand must carry agent id {}; row:\n{alpha_row}",
        uid(AGENT_ALPHA_ID)
    );

    // Journal: the panel fetched `/api/v1/agents` and — being lazy — fetched NO grants yet.
    let recorded = stub.recorded();
    assert!(
        recorded.iter().any(|r| r.path == "/api/v1/agents"),
        "the agents partial must fetch /api/v1/agents; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert!(
        recorded.iter().all(|r| !r.path.ends_with("/grants")),
        "grants are lazy (loaded on expand) — the agents partial must NOT fetch any grants; \
         recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert_no_secret_paths(&recorded);
    assert_bearer_and_no_leak(&recorded, &[&body]);
}

// =============================================================================================
// Test 2: GRANTS ON EXPAND — the agent-grants partial fetches `/api/v1/agents/{id}/grants` and
// renders the agent's grant rows (team_name / resource / action) from the CP
// `Page<AgentGrantView>` envelope. The forwarded id matches the requested agent. No token leak.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_grants_partial_renders_grant_rows_for_the_requested_agent() {
    let fx = agents_fixture();
    let team = fx.team.clone();
    let team_a = fx.grant_team_a.clone();
    let team_b = fx.grant_team_b.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let agent_id = uid(AGENT_ALPHA_ID);
    let resp = fetch(&http, &dash.agent_grants_partial_url(&agent_id)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the MCP agent-grants partial must be 200"
    );
    let body = resp.text().await.expect("grants body");

    // Both grant rows render: team_name, resource, action.
    assert!(
        body.contains(&team_a) && body.contains(&team_b),
        "both grants' team names must render ({team_a:?}, {team_b:?}); body:\n{body}"
    );
    let row_a = agent_segment(&body, &team_a, &[&team_b]);
    assert!(
        row_a.contains(GRANT_A_RESOURCE) && row_a.contains(GRANT_A_ACTION),
        "grant row for {team_a:?} must render resource {GRANT_A_RESOURCE:?} + action \
         {GRANT_A_ACTION:?}; row:\n{row_a}"
    );
    let row_b = agent_segment(&body, &team_b, &[&team_a]);
    assert!(
        row_b.contains(GRANT_B_RESOURCE) && row_b.contains(GRANT_B_ACTION),
        "grant row for {team_b:?} must render resource {GRANT_B_RESOURCE:?} + action \
         {GRANT_B_ACTION:?}; row:\n{row_b}"
    );

    // Journal: the grants fetch targeted THIS agent's id.
    let recorded = stub.recorded();
    let want = format!("/api/v1/agents/{agent_id}/grants");
    assert!(
        recorded.iter().any(|r| r.path == want),
        "the agent-grants partial must fetch {want}; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert_no_secret_paths(&recorded);
    assert_bearer_and_no_leak(&recorded, &[&body]);
}

// =============================================================================================
// Test 3: UNAUTHORIZED — a CP 403 on `/api/v1/agents` must render `Panel::Unauthorized`: the
// panel shows the org-authority note (HTTP 200 body), NOT a silently empty table and NOT a 500.
// No agent data renders. No token leak.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_partial_renders_unauthorized_on_cp_403() {
    let fx = agents_fixture();
    let team = fx.team.clone();
    let alpha = fx.agent_alpha.clone();
    let bravo = fx.agent_bravo.clone();
    let mut state = fx.stub_state;
    state.agents_status = 403;
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.agents_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a CP 403 must not fail the agents partial itself (Panel::Unauthorized is a 200 body), \
         and must NOT surface as a 500"
    );
    let body = resp.text().await.expect("unauthorized body");

    // The org-authority note renders (the panel does not silently blank).
    assert!(
        body.to_lowercase().contains("org authority")
            || body.to_lowercase().contains("not authorized")
            || body.to_lowercase().contains("unauthorized"),
        "the Unauthorized agents panel must show the org-authority note, not a blank panel; \
         body:\n{body}"
    );

    // No agent data may render on 403.
    assert!(
        !body.contains(&alpha) && !body.contains(&bravo),
        "no agent rows may render when the CP denies with 403 (NOT a silently empty table \
         masquerading as success); body:\n{body}"
    );

    assert_no_secret_paths(&stub.recorded());
    assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
}

// =============================================================================================
// Test 4: UNAVAILABLE — a CP 200 with an undecodable (non-JSON) body must render
// `Panel::Unavailable` (HTTP 200 body), never a silently empty success table. No token leak.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_partial_renders_unavailable_on_decode_failure() {
    let fx = agents_fixture();
    let team = fx.team.clone();
    let alpha = fx.agent_alpha.clone();
    let bravo = fx.agent_bravo.clone();
    let mut state = fx.stub_state;
    state.agents_malformed = true;
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.agents_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a malformed CP body must not fail the agents partial itself (Panel::Unavailable is a \
         200 body)"
    );
    let body = resp.text().await.expect("unavailable body");

    assert!(
        body.to_lowercase().contains("unavailable"),
        "a decode failure must render an \"unavailable\" state, never a silently empty table; \
         body:\n{body}"
    );
    assert!(
        !body.contains(&alpha) && !body.contains(&bravo),
        "no agent rows may render on a decode failure; body:\n{body}"
    );

    assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
}

// =============================================================================================
// Test 5: MCP SHELL — `GET /<nonce>/mcp` lazy-loads the agents section via an hx-get to
// `/partials/mcp/agents`. The shell itself performs no agents fetch (only the partial does).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_shell_lazy_loads_the_agents_partial() {
    let fx = agents_fixture();
    let team = fx.team.clone();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.mcp_shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/mcp must serve the MCP shell page"
    );
    let shell = resp.text().await.expect("shell body");

    assert!(
        shell.contains("partials/mcp/agents"),
        "the MCP shell must lazy-load /partials/mcp/agents; body:\n{shell}"
    );
    assert!(
        shell.contains("hx-get"),
        "the agents section must load via htmx (hx-get); body:\n{shell}"
    );

    // The shell itself must not fetch the agents list — only the partial does.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let recorded = stub.recorded();
    assert!(
        recorded.iter().all(|r| r.path != "/api/v1/agents"),
        "the MCP shell page must NOT fetch /api/v1/agents — only the partial does; \
         recorded: {recorded:?}"
    );
    assert_bearer_and_no_leak(&recorded, &[&shell]);
}
