//! fpv2-80z.8 — `flowplane dashboard` MCP tab UI restyle — black-box, spec-driven contract
//! suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against the
//! slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix. The
//! MCP tab serves four partials, each fetched lazily from the shell page `GET /<nonce>/mcp`:
//!
//!   * `GET /<nonce>/partials/mcp/status`       (fetches team `mcp/status` + `mcp/connections`)
//!   * `GET /<nonce>/partials/mcp/tools`        (fetches team `mcp/tools?include_disabled=true`)
//!   * `GET /<nonce>/partials/mcp/agents`       (fetches org-wide `/api/v1/agents`)
//!   * `GET /<nonce>/partials/mcp/agent-grants?id=<id>` (fetches `/api/v1/agents/{id}/grants`)
//!
//! Acceptance criteria covered (served-HTML level), per the authoritative status→pill map:
//!   1. Tools partial — the tool `risk` renders as a `pill`: `read` → `pill neutral`,
//!      `mutate` → `pill warn`, `delete` → `pill crit`, an UNKNOWN risk value → `pill neutral`
//!      (the neutral fallback), and the risk text is shown verbatim inside the pill. Applies in
//!      both the CP-tools table and the API-tools table.
//!   2. Tools partial — the `enabled` and `executable_by_caller` booleans render as `pill`s:
//!      `true` → `pill good`, `false` → `pill neutral`. The API-tools table has both cells; the
//!      CP-tools table has the `executable_by_caller` cell (asserted row-scoped for both tables).
//!   3. Agents partial — the agent `status` renders as a `pill`: `active` → `pill good`,
//!      `suspended` → `pill warn`, an UNKNOWN status → `pill neutral` (the neutral fallback),
//!      status text verbatim.
//!   4. Status partial — the MCP server capability toggles (SSE / Resources / Prompts) are
//!      DESCRIPTIVE: both enabled and disabled render `pill neutral` (only the verbatim state
//!      word "enabled"/"disabled" differs) — never `pill warn`, never `pill good`.
//!   5. Structure/security across every MCP partial (and the shell): every `<table>` is genuinely
//!      CONTAINED in a `.tablewrap` (wrapper opens before `<table>` and its `</div>` closes after
//!      `</table>`); no inline `<style>`, no src-less `<script>`, no inline `style=` attribute;
//!      and the shell's CSP response header is still exactly `default-src 'self'`.
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
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-mcp-token-do-not-leak-6e2a";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Deterministic id (a valid uuid) with no accidental digit collisions.
fn uid(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the MCP read model the four
// partials fetch — the team-scoped `mcp/status` / `mcp/connections` / `mcp/tools`, plus the
// org-wide `/api/v1/agents` and `/api/v1/agents/{id}/grants`. Every route answers 200 with a
// canned fixture (this UI-restyle slice never exercises degradation); unknown paths 404.
// =============================================================================================

struct StubState {
    team: String,
    status_body: Value,
    connections: Vec<Value>,
    /// Body of `mcp/tools?include_disabled=true` (the full catalog, disabled rows included).
    tools_full: Vec<Value>,
    /// Body of `mcp/tools` without the flag (the enabled-only fallback; unused by these tests).
    tools_enabled: Vec<Value>,
    agents: Vec<Value>,
    grants: Vec<Value>,
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

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "code": "not_found", "message": "no such route" })),
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

fn route_request(state: &StubState, path: &str, query: &str) -> Response {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segs.as_slice() {
        ["api", "v1", "teams", team, "mcp", "status"] if *team == state.team => {
            Json(state.status_body.clone()).into_response()
        }
        ["api", "v1", "teams", team, "mcp", "connections"] if *team == state.team => {
            Json(Value::Array(state.connections.clone())).into_response()
        }
        ["api", "v1", "teams", team, "mcp", "tools"] if *team == state.team => {
            if query.contains("include_disabled=true") {
                Json(Value::Array(state.tools_full.clone())).into_response()
            } else {
                Json(Value::Array(state.tools_enabled.clone())).into_response()
            }
        }
        ["api", "v1", "agents"] => page(&state.agents),
        ["api", "v1", "agents", _id, "grants"] => page(&state.grants),
        _ => not_found(),
    }
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    route_request(&state, &path, &query)
}

async fn start_stub(state: StubState) -> StubUpstream {
    let state = Arc::new(state);
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
// Canned payload builders (shapes as the real CP returns them; extra fields are harmless — the
// stub only needs what the dashboard reads).
// =============================================================================================

fn mcp_status_body() -> Value {
    json!({
        "transport": "streamable_http_post",
        "preferred_protocol_version": "2025-11-25",
        "supported_protocol_versions": ["2025-11-25", "2025-03-26"],
        "session_ttl_seconds": 3600,
        "active_sessions": 1,
        "static_tool_count": 35,
        "dynamic_enabled_tool_count": 3,
        "tools_list_changed": false,
        // A mix so the capability-toggle test exercises BOTH states: at least one enabled and at
        // least one disabled. Per the corrected contract these are descriptive `pill neutral`
        // regardless of value — only the verbatim state word differs.
        "sse_enabled": true,
        "resources_enabled": false,
        "prompts_enabled": true,
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
// Fixture: a healthy MCP read model exercising every pill state at least once.
//   Tools: a `read` tool (enabled+executable), a `mutate` tool (enabled+executable), a `delete`
//   tool (disabled + non-executable), and an UNKNOWN-risk tool (enabled + non-executable).
//   Agents: an `active` agent, a `suspended` agent, and an UNKNOWN-status agent.
// The non-risk/non-status tool fields are set to non-colliding values so a `read`/`delete`/…
// substring can only come from the risk pill itself.
// =============================================================================================

const AGENT_ACTIVE_ID: u64 = 100;
const AGENT_SUSPENDED_ID: u64 = 101;
const AGENT_UNKNOWN_ID: u64 = 102;

struct McpFixture {
    stub_state: StubState,
    team: String,
    tool_read: String,
    tool_mutate: String,
    tool_delete: String,
    tool_unknown: String,
    /// A CP (static) tool with executable_by_caller=true (its CP-table executable cell → good).
    tool_cp_exec: String,
    /// A CP (static) tool with executable_by_caller=false (its CP-table executable cell → neutral).
    tool_cp_noexec: String,
    /// The unknown risk value (must render verbatim in a `pill neutral`).
    risk_weird: String,
    agent_active: String,
    agent_suspended: String,
    agent_unknown: String,
    /// The unknown agent status value (verbatim in a `pill neutral`).
    status_weird: String,
    grant_team: String,
}

fn mcp_fixture() -> McpFixture {
    let team = unique("team");
    let tool_read = unique("tool-read");
    let tool_mutate = unique("tool-mutate");
    let tool_delete = unique("tool-delete");
    let tool_unknown = unique("tool-unknown");
    let tool_cp_exec = unique("tool-cp-exec");
    let tool_cp_noexec = unique("tool-cp-noexec");
    let risk_weird = unique("severity");
    let agent_active = unique("agent-active");
    let agent_suspended = unique("agent-suspended");
    let agent_unknown = unique("agent-unknown");
    let status_weird = unique("hibernating");
    let grant_team = unique("grant-team");

    // Non-risk fields deliberately avoid the words read/mutate/delete so those tokens can only
    // originate from the risk pill.
    let tools = vec![
        tool_row(
            &tool_read, "desc", "widget", "invoke", "read", "static", true, true,
        ),
        tool_row(
            &tool_mutate,
            "desc",
            "widget",
            "invoke",
            "mutate",
            "dynamic",
            true,
            true,
        ),
        tool_row(
            &tool_delete,
            "desc",
            "widget",
            "invoke",
            "delete",
            "dynamic",
            false,
            false,
        ),
        tool_row(
            &tool_unknown,
            "desc",
            "widget",
            "invoke",
            &risk_weird,
            "dynamic",
            true,
            false,
        ),
        // Two `static` (CP-table) tools to exercise the CP-tools table's executable cell. Their
        // risk values (warn / crit) are deliberately never `good` nor `neutral`, so a `pill good`
        // / `pill neutral` in the row can only originate from the executable cell itself.
        tool_row(
            &tool_cp_exec,
            "desc",
            "widget",
            "invoke",
            "mutate",
            "static",
            true,
            true,
        ),
        tool_row(
            &tool_cp_noexec,
            "desc",
            "widget",
            "invoke",
            "delete",
            "static",
            true,
            false,
        ),
    ];
    // The enabled-only subset (fallback body; not exercised by these tests).
    let tools_enabled: Vec<Value> = tools
        .iter()
        .filter(|t| t["enabled"] == json!(true))
        .cloned()
        .collect();

    let agents = vec![
        agent_item(AGENT_ACTIVE_ID, &agent_active, "cp-tool", "active"),
        agent_item(
            AGENT_SUSPENDED_ID,
            &agent_suspended,
            "api-consumer",
            "suspended",
        ),
        agent_item(AGENT_UNKNOWN_ID, &agent_unknown, "cp-tool", &status_weird),
    ];
    let grants = vec![grant_item(200, 300, &grant_team, "clusters", "read")];

    McpFixture {
        stub_state: StubState {
            team: team.clone(),
            status_body: mcp_status_body(),
            connections: vec![connection_row("5f3a9c21-0000-4000-8000-000000000abc")],
            tools_full: tools,
            tools_enabled,
            agents,
            grants,
        },
        team,
        tool_read,
        tool_mutate,
        tool_delete,
        tool_unknown,
        tool_cp_exec,
        tool_cp_noexec,
        risk_weird,
        agent_active,
        agent_suspended,
        agent_unknown,
        status_weird,
        grant_team,
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

/// The opening tag (e.g. `<nav class="tabs">`) starting at the first occurrence of `tag_start`
/// (e.g. `<nav`), up to and including its closing `>`.
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
/// attribute, the text between the end of its opening tag and the matching close tag, with
/// depth counting so nested same-tag elements don't truncate the region.
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

/// The byte offset of every `<div ...>` opening tag whose class tokens include `tablewrap`.
fn tablewrap_div_starts(html: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut search = 0usize;
    while let Some(rel) = html[search..].find("<div") {
        let start = search + rel;
        let Some(end_rel) = html[start..].find('>') else {
            break;
        };
        let end = start + end_rel;
        let tag = &html[start..=end];
        if let Some(ci) = tag.find("class=\"") {
            let after = &tag[ci + "class=\"".len()..];
            if let Some(q) = after.find('"') {
                if after[..q].split_whitespace().any(|t| t == "tablewrap") {
                    out.push(start);
                }
            }
        }
        search = end + 1;
    }
    out
}

/// Given the byte offset of a `<div` opening tag, the byte offset of its MATCHING `</div>`
/// (depth-counted over nested `<div>`/`</div>`), or `None` if unbalanced.
fn matching_close_div(html: &str, div_start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut pos = div_start;
    loop {
        let next_open = html[pos..].find("<div").map(|x| pos + x);
        let next_close = html[pos..].find("</div>").map(|x| pos + x);
        match (next_open, next_close) {
            (_, None) => return None,
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                pos = o + "<div".len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return Some(c);
                }
                pos = c + "</div>".len();
            }
        }
    }
}

/// The `<tr>...</tr>` region containing `needle` (row-scoped assertions). Does NOT use
/// first-textual-occurrence matching — it walks back to the `<tr>` that encloses `needle`.
fn row_of<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("row for {needle:?} must exist; body:\n{html}"));
    let tr_open = html[..at]
        .rfind("<tr")
        .unwrap_or_else(|| panic!("row for {needle:?} must be a <tr>; body:\n{html}"));
    region(&html[tr_open..], "<tr", "</tr>")
        .unwrap_or_else(|| panic!("row for {needle:?} must be closed; body:\n{html}"))
}

/// Does `scope` carry a `pill <class>` element whose text contains `needle`? Asserts the pill's
/// class and its verbatim text together.
fn pill_text_contains(scope: &str, class: &str, needle: &str) -> bool {
    texts_of_elements_with_classes(scope, &["pill", class])
        .iter()
        .any(|t| t.contains(needle))
}

/// Assert the inline-forbidden rules shared by every partial/page: no inline `<style>` element,
/// every `<script>` carries src=, no inline `style=` attribute.
fn assert_no_inline(html: &str, which: &str) {
    let lower = html.to_lowercase();
    assert!(
        !lower.contains("<style"),
        "the {which} must not contain an inline <style> element; body:\n{html}"
    );
    for tag in all_opening_tags(&lower, "<script") {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden in the {which}; every script tag must carry \
             src=; offending tag: {tag:?}"
        );
    }
    assert!(
        !lower.contains(" style="),
        "the {which} must not contain any inline style= attribute; body:\n{html}"
    );
}

/// Every `<table>` in `html` must be genuinely CONTAINED in a `.tablewrap` wrapper: some
/// `<div class="...tablewrap...">` opens before the `<table>` AND its matching `</div>` closes
/// after that table's `</table>` (proving real nesting, not merely that the class appears before
/// the table). Requires at least `min_tables` tables so the assertion is not vacuous.
fn assert_tables_wrapped(html: &str, which: &str, min_tables: usize) {
    let tables: Vec<usize> = html.match_indices("<table").map(|(i, _)| i).collect();
    assert!(
        tables.len() >= min_tables,
        "the {which} must render at least {min_tables} <table> element(s); found {}; body:\n{html}",
        tables.len()
    );
    let wrap_starts = tablewrap_div_starts(html);
    for &table_at in &tables {
        let table_end = html[table_at..]
            .find("</table>")
            .map(|x| table_at + x)
            .unwrap_or_else(|| {
                panic!("the {which} has an unclosed <table> at byte {table_at}; body:\n{html}")
            });
        let contained = wrap_starts.iter().any(|&w| {
            w < table_at && matching_close_div(html, w).is_some_and(|close_at| close_at > table_end)
        });
        assert!(
            contained,
            "every <table> in the {which} must be CONTAINED in a `.tablewrap` wrapper (the \
             wrapper's `<div>` opens before `<table>` AND its `</div>` closes after `</table>`); \
             unwrapped table at byte {table_at}; body:\n{html}"
        );
    }
}

// =============================================================================================
// Criterion 1: the tools partial maps each tool's `risk` to a pill per the vocabulary —
// read → `pill neutral`, mutate → `pill warn`, delete → `pill crit`, unknown → `pill neutral`
// (fallback) — and shows the risk text verbatim inside the pill. Row-scoped so the RIGHT pill is
// asserted for each tool.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tools_partial_risk_pills_map_per_vocabulary_with_neutral_fallback() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let tools = fetch_page(&http, &dash, "partials/mcp/tools").await;

    // read → pill neutral "read".
    let read_row = row_of(&tools, &fx.tool_read);
    assert!(
        pill_text_contains(read_row, "neutral", "read"),
        "a `read` risk must render `<span class=\"pill neutral\">read</span>`; row:\n{read_row}"
    );

    // mutate → pill warn "mutate".
    let mutate_row = row_of(&tools, &fx.tool_mutate);
    assert!(
        pill_text_contains(mutate_row, "warn", "mutate"),
        "a `mutate` risk must render `<span class=\"pill warn\">mutate</span>`; row:\n{mutate_row}"
    );

    // delete → pill crit "delete".
    let delete_row = row_of(&tools, &fx.tool_delete);
    assert!(
        pill_text_contains(delete_row, "crit", "delete"),
        "a `delete` risk must render `<span class=\"pill crit\">delete</span>`; row:\n{delete_row}"
    );

    // unknown risk → pill neutral (fallback), with the unknown value verbatim.
    let unknown_row = row_of(&tools, &fx.tool_unknown);
    assert!(
        pill_text_contains(unknown_row, "neutral", &fx.risk_weird),
        "an UNKNOWN risk value must fall back to `pill neutral` showing the value verbatim \
         ({:?}); row:\n{unknown_row}",
        fx.risk_weird
    );
}

// =============================================================================================
// Criterion 2: the tools partial maps the `enabled` and `executable_by_caller` booleans to
// pills — true → `pill good`, false → `pill neutral`. Attributed via distinct risk classes:
// the enabled+executable tool has risk `mutate` (warn, never good), so any `pill good` in its
// row is a true boolean; the disabled+non-executable tool has risk `delete` (crit, never good),
// so any `pill good` there would be a mis-mapped false boolean.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tools_partial_enabled_and_executable_booleans_map_to_good_or_neutral() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let tools = fetch_page(&http, &dash, "partials/mcp/tools").await;

    // enabled=true AND executable_by_caller=true → two `pill good`s (risk here is warn, so the
    // only source of `pill good` is the two true booleans).
    let mutate_row = row_of(&tools, &fx.tool_mutate);
    let goods = texts_of_elements_with_classes(mutate_row, &["pill", "good"]);
    assert!(
        goods.len() >= 2,
        "a tool with enabled=true AND executable_by_caller=true must render both booleans as \
         `pill good` (>=2 good pills); found {}: {goods:?}; row:\n{mutate_row}",
        goods.len()
    );

    // enabled=false AND executable_by_caller=false → NO `pill good` (false never maps to good)
    // and the booleans render as `pill neutral` (risk here is crit, so neutral pills come from
    // the two false booleans).
    let delete_row = row_of(&tools, &fx.tool_delete);
    assert!(
        !has_element_with_classes(delete_row, &["pill", "good"]),
        "a tool with enabled=false AND executable_by_caller=false must render NO `pill good`; \
         row:\n{delete_row}"
    );
    let neutrals = texts_of_elements_with_classes(delete_row, &["pill", "neutral"]);
    assert!(
        neutrals.len() >= 2,
        "a tool with enabled=false AND executable_by_caller=false must render both booleans as \
         `pill neutral` (>=2 neutral pills); found {}: {neutrals:?}; row:\n{delete_row}",
        neutrals.len()
    );
}

// =============================================================================================
// Criterion 2 (CP-tools table): the CP-tools table renders `executable_by_caller` as a pill —
// true → `pill good`, false → `pill neutral`. Attributed via distinct risk classes: the
// executable=true CP tool has risk `mutate` (warn, never good), so any `pill good` in its row is
// the executable cell; the executable=false CP tool has risk `delete` (crit, never neutral), so
// any `pill neutral` there is the executable cell (and there must be NO `pill good`).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tools_partial_cp_executable_cell_maps_to_good_or_neutral() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let tools = fetch_page(&http, &dash, "partials/mcp/tools").await;

    // CP (static) tool, executable_by_caller=true → a `pill good` in its row (risk here is warn,
    // so the only source of a `pill good` is the executable cell).
    let exec_row = row_of(&tools, &fx.tool_cp_exec);
    assert!(
        has_element_with_classes(exec_row, &["pill", "good"]),
        "a CP tool with executable_by_caller=true must render its executable cell as \
         `pill good`; row:\n{exec_row}"
    );

    // CP (static) tool, executable_by_caller=false → NO `pill good` (false never maps to good)
    // and the executable cell renders `pill neutral` (risk here is crit, so the neutral pill can
    // only be the executable cell).
    let noexec_row = row_of(&tools, &fx.tool_cp_noexec);
    assert!(
        !has_element_with_classes(noexec_row, &["pill", "good"]),
        "a CP tool with executable_by_caller=false must render NO `pill good`; row:\n{noexec_row}"
    );
    assert!(
        has_element_with_classes(noexec_row, &["pill", "neutral"]),
        "a CP tool with executable_by_caller=false must render its executable cell as \
         `pill neutral`; row:\n{noexec_row}"
    );
}

// =============================================================================================
// Criterion 3: the agents partial maps each agent's `status` to a pill per the vocabulary —
// active → `pill good`, suspended → `pill warn`, unknown → `pill neutral` (fallback) — with the
// status text verbatim inside the pill. Row-scoped per agent.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_agents_partial_status_pills_map_per_vocabulary_with_neutral_fallback() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let agents = fetch_page(&http, &dash, "partials/mcp/agents").await;

    // active → pill good "active".
    let active_row = row_of(&agents, &fx.agent_active);
    assert!(
        pill_text_contains(active_row, "good", "active"),
        "an `active` agent status must render `<span class=\"pill good\">active</span>`; \
         row:\n{active_row}"
    );

    // suspended → pill warn "suspended".
    let suspended_row = row_of(&agents, &fx.agent_suspended);
    assert!(
        pill_text_contains(suspended_row, "warn", "suspended"),
        "a `suspended` agent status must render `<span class=\"pill warn\">suspended</span>`; \
         row:\n{suspended_row}"
    );

    // unknown status → pill neutral (fallback), value verbatim.
    let unknown_row = row_of(&agents, &fx.agent_unknown);
    assert!(
        pill_text_contains(unknown_row, "neutral", &fx.status_weird),
        "an UNKNOWN agent status must fall back to `pill neutral` showing the value verbatim \
         ({:?}); row:\n{unknown_row}",
        fx.status_weird
    );
}

// =============================================================================================
// Criterion 4: the status partial's MCP server capability toggles (SSE / Resources / Prompts)
// are DESCRIPTIVE — both enabled and disabled render `pill neutral`; only the verbatim state word
// "enabled"/"disabled" differs. NEITHER state may be dressed as `pill warn` or `pill good`. The
// fixture sets a mix (sse=true, resources=false, prompts=true) so both states are exercised.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_status_partial_capability_toggles_are_descriptive_neutral_pills() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let status = fetch_page(&http, &dash, "partials/mcp/status").await;

    // An ENABLED and a DISABLED capability toggle each render as `pill neutral` with the state
    // word verbatim. ("enabled" is not a substring of "disabled" nor vice versa, so a `contains`
    // check attributes each state unambiguously.)
    let neutral_texts = texts_of_elements_with_classes(&status, &["pill", "neutral"]);
    assert!(
        neutral_texts.iter().any(|t| t.contains("enabled")),
        "an ENABLED capability toggle must render a `pill neutral` whose text is \"enabled\"; \
         neutral pill texts: {neutral_texts:?}; body:\n{status}"
    );
    assert!(
        neutral_texts.iter().any(|t| t.contains("disabled")),
        "a DISABLED capability toggle must render a `pill neutral` whose text is \"disabled\"; \
         neutral pill texts: {neutral_texts:?}; body:\n{status}"
    );

    // ...and NEITHER state may be dressed as a status pill: no `pill warn` / `pill good` may
    // carry the "enabled"/"disabled" capability text.
    for class in ["warn", "good"] {
        let texts = texts_of_elements_with_classes(&status, &["pill", class]);
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("enabled") || t.contains("disabled")),
            "capability toggles are descriptive and must NOT render as `pill {class}`; a \
             `pill {class}` carries capability text; `pill {class}` texts: {texts:?}; \
             body:\n{status}"
        );
    }
}

// =============================================================================================
// Criterion 5: structure/security across every MCP partial (and the shell) — every `<table>` is
// wrapped in `.tablewrap`; no inline `<style>`, no src-less `<script>`, no inline `style=`
// attribute; and the shell's CSP response header is still exactly `default-src 'self'`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_all_partials_wrap_tables_forbid_inline_and_shell_keeps_csp() {
    let fx = mcp_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    // The shell page: CSP header + no inline. (Same-origin <script src=htmx> is allowed.)
    let shell_url = dash.nonce_url("mcp");
    let resp = fetch(&http, &shell_url).await;
    assert_eq!(resp.status().as_u16(), 200, "GET /<nonce>/mcp must be 200");
    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("GET {shell_url} must carry a Content-Security-Policy header"))
        .to_string();
    assert_eq!(
        csp, "default-src 'self'",
        "the MCP shell CSP header must still be `default-src 'self'`"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        !shell.contains(SECRET_TOKEN),
        "the shell must never leak the bearer token; body:\n{shell}"
    );
    assert_no_inline(&shell, "mcp shell");

    // Each partial: tables wrapped + no inline.
    let status = fetch_page(&http, &dash, "partials/mcp/status").await;
    assert_no_inline(&status, "mcp status partial");
    assert_tables_wrapped(&status, "mcp status partial", 1);

    let tools = fetch_page(&http, &dash, "partials/mcp/tools").await;
    assert_no_inline(&tools, "mcp tools partial");
    assert_tables_wrapped(&tools, "mcp tools partial", 1);

    let agents = fetch_page(&http, &dash, "partials/mcp/agents").await;
    assert_no_inline(&agents, "mcp agents partial");
    assert_tables_wrapped(&agents, "mcp agents partial", 1);

    let grants = fetch_page(
        &http,
        &dash,
        &format!("partials/mcp/agent-grants?id={}", uid(AGENT_ACTIVE_ID)),
    )
    .await;
    assert_no_inline(&grants, "mcp agent-grants partial");
    assert_tables_wrapped(&grants, "mcp agent-grants partial", 1);

    // Guard against the grant fixture team leaking anywhere unexpected (sanity: it renders).
    assert!(
        grants.contains(&fx.grant_team),
        "the agent-grants partial must render the grant's team; body:\n{grants}"
    );
}
