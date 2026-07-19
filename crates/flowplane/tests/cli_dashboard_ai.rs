//! fpv2-0t4.4 — `flowplane dashboard` AI tab — black-box, spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * New page `GET /<nonce>/ai` (nav: Overview / Resources / APIs / Learning / AI) with a
//!     container that lazy-loads `GET /<nonce>/partials/ai/overview` via htmx (load once).
//!     The shell page itself performs NO upstream fetch — only the partial does.
//!   * The overview partial fetches, against the CP:
//!     `GET /api/v1/teams/{team}/ai/providers?limit=500&offset=0`,
//!     `.../ai/routes?limit=500&offset=0`, `.../ai/budgets?limit=500&offset=0` (Page
//!     envelopes), and paged `GET .../ai/usage?since=<RFC3339>&until=<RFC3339>&limit=500`
//!     fetches — ONE per page, every page sharing the identical captured window pair; a
//!     fixture under 500 grouped rows yields exactly one fetch per render`
//!     per partial render, where `until − since` is EXACTLY 24 hours. If any usage row has
//!     (S5 will add a `.../route-configs?limit=500&offset=0` mapping fetch for the
//!     id → name mapping.
//!   * Rendered HTML: cards (provider count, routes active, routes stale, "Tokens (24h)" =
//!     SUM of windowed usage items' `total_tokens`); providers panel lists ONLY
//!     `openai`/`openai-compatible` kinds (any other kind is hidden and a banner mentions
//!     hidden providers); route backend chains render provider NAMES joined with "→" in
//!     priority order (priority 0 first); budgets show `state.used_units / state.limit_units`,
//!     a meter, the mode pill ("shadow"/"enforcing" verbatim), and a near-limit warning
//!     banner (id `ai-near-limit`) naming every budget at ≥ 80% of its window — and only
//!     those.
//!   * Degradation per the dashboard conventions: providers upstream 403 → "Not authorized"
//!     section; 500 → "unavailable"; upstream 401 → HTTP 286 naming `flowplane auth login`.
//!   * CRITICAL negative: the bearer token never appears in any response body.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique
//! team/resource names; nothing binds a fixed port. Every spawned server is killed via a
//! Drop guard in all paths, including assertion failures.

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

/// A distinctive bearer token so any leak into a response body is unambiguous.
const SECRET_TOKEN: &str = "sekret-ai-tab-token-do-not-leak-7e2b";

/// The documented list page size for every overview collection fetch.
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

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the AI read model (providers /
// routes / budgets / usage) plus the route-configs list, canned failures, and a full request
// journal (path + query + auth + the status the stub answered). Unknown paths are recorded
// too and answered 404, so allowlist / negative assertions see every request the dashboard
// makes.
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
    fn page(&self) -> PageQuery {
        let uri: axum::http::Uri = format!("/q?{}", self.query)
            .parse()
            .expect("recorded query");
        Query::<PageQuery>::try_from_uri(&uri)
            .map(|q| q.0)
            .unwrap_or_default()
    }

    fn usage_query(&self) -> UsageQuery {
        let uri: axum::http::Uri = format!("/q?{}", self.query)
            .parse()
            .expect("recorded query");
        Query::<UsageQuery>::try_from_uri(&uri)
            .map(|q| q.0)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
struct PageQuery {
    limit: Option<u64>,
    offset: Option<u64>,
}

/// The usage endpoint's query contract: RFC3339 `since`/`until` plus `limit`.
#[derive(Debug, Default, Clone, serde::Deserialize)]
struct UsageQuery {
    since: Option<String>,
    until: Option<String>,
    limit: Option<u64>,
}

struct StubState {
    team: String,
    /// Status for the ai/providers LIST endpoint (200 = healthy) — the degradation lever.
    providers_status: u16,
    providers: Vec<Value>,
    routes: Vec<Value>,
    budgets: Vec<Value>,
    usage: Vec<Value>,
    route_configs: Vec<Value>,
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

fn route_request(state: &StubState, path: &str, page: PageQuery) -> Response {
    let prefix = format!("/api/v1/teams/{}/", state.team);
    let Some(rest) = path.strip_prefix(&prefix) else {
        return canned_error(404);
    };
    let segs: Vec<&str> = rest.split('/').collect();

    match segs.as_slice() {
        ["ai", "providers"] => {
            if state.providers_status != 200 {
                return canned_error(state.providers_status);
            }
            paged(&state.providers, page)
        }
        ["ai", "routes"] => paged(&state.routes, page),
        ["ai", "budgets"] => paged(&state.budgets, page),
        ["ai", "usage"] => paged(&state.usage, page),
        ["route-configs"] => paged(&state.route_configs, page),
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

    let page: PageQuery = Query::<PageQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();

    let response = route_request(&state, &path, page);
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

/// `spec.limit_units` deliberately DIVERGES from `state.limit_units`: the contract says the
/// row (and the ≥ 80% warning) read the STATE numbers, so any implementation reading the
/// spec's limit gets caught.
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

fn usage_item(route_config_id: u64, provider_id: u64, total_tokens: u64) -> Value {
    json!({
        "route_config_id": uid(route_config_id),
        "provider_id": uid(provider_id),
        "prompt_tokens": 7,
        "completion_tokens": total_tokens - 7,
        "total_tokens": total_tokens,
        "event_count": 3,
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

    fn ai_shell_url(&self) -> String {
        self.page_url("ai")
    }

    fn overview_partial_url(&self) -> String {
        self.page_url("partials/ai/overview")
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

/// The recorded fetches of the AI usage endpoint.
fn usage_fetches(recorded: &[Recorded], team: &str) -> Vec<Recorded> {
    let path = format!("/api/v1/teams/{team}/ai/usage");
    recorded
        .iter()
        .filter(|r| r.path == path)
        .cloned()
        .collect()
}

/// WINDOW CONTRACT: `since`/`until` are RFC3339 and `until − since` is EXACTLY 24 hours;
/// `limit=500` is carried.
fn assert_usage_window(req: &Recorded) {
    let q = req.usage_query();
    let since = q
        .since
        .unwrap_or_else(|| panic!("the usage fetch must carry `since`; query: {:?}", req.query));
    let until = q
        .until
        .unwrap_or_else(|| panic!("the usage fetch must carry `until`; query: {:?}", req.query));
    let since_t = chrono::DateTime::parse_from_rfc3339(&since)
        .unwrap_or_else(|e| panic!("usage `since` must be RFC3339, got {since:?}: {e}"));
    let until_t = chrono::DateTime::parse_from_rfc3339(&until)
        .unwrap_or_else(|e| panic!("usage `until` must be RFC3339, got {until:?}: {e}"));
    assert_eq!(
        until_t - since_t,
        chrono::Duration::hours(24),
        "usage window must span EXACTLY 24 hours; since={since:?} until={until:?}"
    );
    assert_eq!(
        q.limit,
        Some(PAGE_LIMIT),
        "the usage fetch must carry limit=500; query: {:?}",
        req.query
    );
}

/// Assert some recorded request hit `path` with `limit=500&offset=0`.
fn assert_paged_fetch(recorded: &[Recorded], path: &str) {
    let matching: Vec<&Recorded> = recorded.iter().filter(|r| r.path == path).collect();
    assert!(
        !matching.is_empty(),
        "the overview partial must fetch {path}; recorded paths: {:?}",
        recorded.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    assert!(
        matching.iter().any(|r| {
            let p = r.page();
            p.limit == Some(PAGE_LIMIT) && p.offset.unwrap_or(0) == 0
        }),
        "{path} must be fetched with limit=500&offset=0; recorded: {matching:?}"
    );
}

/// Extract the value rendered on the stat card labeled `label`.
fn card_value(body: &str, label: &str) -> String {
    let needle = format!(">{label}<");
    let idx = body
        .find(&needle)
        .unwrap_or_else(|| panic!("no card labeled {label:?}; body:\n{body}"));
    let before = &body[..idx];
    let vstart = before
        .rfind("class=\"value\"")
        .unwrap_or_else(|| panic!("no value span before the {label:?} card label; body:\n{body}"));
    let after = &before[vstart..];
    let open = after
        .find('>')
        .unwrap_or_else(|| panic!("malformed value span for card {label:?}"));
    after[open + 1..]
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// The table-row (`<tr>…</tr>`) fragment containing `marker`. Skips occurrences of the
/// marker that are NOT inside a row (e.g. a banner chip naming the same resource).
fn row_containing<'a>(body: &'a str, marker: &str) -> &'a str {
    let mut from = 0;
    while let Some(rel) = body[from..].find(marker) {
        let idx = from + rel;
        if let Some(start) = body[..idx].rfind("<tr") {
            // Inside a row only if no row CLOSED between "<tr" and the marker.
            if !body[start..idx].contains("</tr>") {
                let end = idx + body[idx..].find("</tr>").unwrap_or(body.len() - idx);
                return &body[start..end];
            }
        }
        from = idx + marker.len();
    }
    panic!("expected a table row containing {marker:?} in body:\n{body}");
}

// =============================================================================================
// Fixture: three providers (openai, openai-compatible, and one "weird-kind" that must be
// hidden), three routes (one with a 2-backend chain whose JSON order is the REVERSE of
// priority order, one plain active, one stale), two budgets (one at 90% of its STATE window
// → warn; one at 1% whose SPEC limit would falsely read 83% → must not warn), and two
// usage rows with route_config_ids summing to a distinctive token total.
// =============================================================================================

const TOKENS_A: u64 = 12345;
const TOKENS_B: u64 = 54321;
const TOKENS_SUM: u64 = TOKENS_A + TOKENS_B; // 66666 — distinctive.

struct AiFixture {
    stub_state: StubState,
    team: String,
    prov_openai: String,
    prov_compat: String,
    prov_weird: String,
    route_chain: String,
    route_plain: String,
    route_stale: String,
    budget_hot: String,
    budget_cool: String,
}

fn ai_fixture() -> AiFixture {
    let team = unique("team");
    let prov_openai = unique("prov-oai");
    let prov_compat = unique("prov-compat");
    let prov_weird = unique("prov-weird");
    let route_chain = unique("route-chain");
    let route_plain = unique("route-plain");
    let route_stale = unique("route-stale");
    let budget_hot = unique("budget-hot");
    let budget_cool = unique("budget-cool");
    let rc_name = unique("rc");

    let providers = vec![
        provider_item(200, &prov_openai, "openai"),
        provider_item(201, &prov_compat, "openai-compatible"),
        provider_item(202, &prov_weird, "weird-kind"),
    ];

    // route_chain's backends are listed with priority 1 FIRST, so a chain rendered in JSON
    // order (instead of priority order) puts the wrong provider first.
    let routes = vec![
        route_item(
            210,
            &route_chain,
            "active",
            vec![backend(200, 1), backend(201, 0)],
        ),
        route_item(211, &route_plain, "active", vec![backend(200, 0)]),
        route_item(212, &route_stale, "stale", vec![backend(201, 0)]),
    ];

    let budgets = vec![
        // 90 / 100 (state) = 90% → near-limit warn. Spec limit 999999 would read 0%.
        budget_item(220, &budget_hot, "enforcing", 999_999, 90, 100),
        // 10 / 1000 (state) = 1% → NO warn. Spec limit 12 would falsely read 83%.
        budget_item(221, &budget_cool, "shadow", 12, 10, 1000),
    ];

    let usage = vec![
        usage_item(300, 200, TOKENS_A),
        usage_item(300, 201, TOKENS_B),
    ];
    let route_configs = vec![route_config_item(300, &rc_name)];

    AiFixture {
        stub_state: StubState {
            team: team.clone(),
            providers_status: 200,
            providers,
            routes,
            budgets,
            usage,
            route_configs,
            requests: Mutex::new(Vec::new()),
        },
        team,
        prov_openai,
        prov_compat,
        prov_weird,
        route_chain,
        route_plain,
        route_stale,
        budget_hot,
        budget_cool,
    }
}

// =============================================================================================
// Test 1: SHELL PAGE — GET /<nonce>/ai is a 200 HTML page with the five-tab nav and an
// htmx container that lazy-loads /partials/ai/overview (load once). CRITICAL negative: the
// shell itself performs NO upstream fetch, and its body never leaks the bearer token.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_shell_page_serves_nav_and_lazy_container_with_no_upstream_fetch() {
    let fx = ai_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let resp = fetch(&http, &dash.ai_shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/ai must serve the AI shell page"
    );
    let shell = resp.text().await.expect("shell body");
    assert!(
        shell.contains('<'),
        "the shell must be HTML; body:\n{shell}"
    );
    for tab in ["Overview", "Resources", "APIs", "Learning", "AI"] {
        assert!(
            shell.contains(tab),
            "the nav must name the {tab:?} tab; body:\n{shell}"
        );
    }
    assert!(
        shell.contains("partials/ai/overview"),
        "the shell must lazy-load /partials/ai/overview; body:\n{shell}"
    );
    assert!(
        shell.contains("hx-get"),
        "the overview container must load via htmx (hx-get); body:\n{shell}"
    );
    assert!(
        shell.contains("hx-trigger=\"load once\""),
        "the overview container must fetch on load, once; body:\n{shell}"
    );

    // Give any (incorrect) fire-and-forget upstream fetch a moment to land, then assert
    // the shell page triggered NONE.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let recorded = stub.recorded();
    assert!(
        recorded.is_empty(),
        "the AI shell page must perform NO upstream fetch — only the partial does; \
         recorded: {recorded:?}"
    );
    assert_bearer_and_no_leak(&recorded, &[&shell]);
}

// =============================================================================================
// Test 2: HAPPY PATH — the overview partial renders the cards (tokens = SUM of windowed
// usage rows), the kind-filtered providers panel + hidden banner, priority-ordered backend
// chains, budget rows (state numbers, meter, verbatim mode pills) and the ≥ 80% near-limit
// banner. Journal: providers/routes/budgets fetched limit=500&offset=0;
// EXACTLY ONE usage fetch per render with an exact 24h RFC3339 window (asserted across two
// renders); bearer auth everywhere; no token leak; no secret path.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_overview_renders_cards_chains_budgets_and_honors_window_contract() {
    let fx = ai_fixture();
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let resp = fetch(&http, &dash.overview_partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the AI overview partial must be 200"
    );
    let body = resp.text().await.expect("overview body");

    // --- Cards. The provider count counts the SUPPORTED (openai / openai-compatible)
    // providers — i.e. the ones the panel renders; the unsupported one is surfaced via the
    // hidden-provider banner instead (asserted below).
    assert_eq!(
        card_value(&body, "Providers"),
        "2",
        "the provider-count card must count the supported providers (weird-kind is hidden, \
         not counted); body:\n{body}"
    );
    assert_eq!(
        card_value(&body, "Routes active"),
        "2",
        "the routes-active card must count status=active routes; body:\n{body}"
    );
    assert_eq!(
        card_value(&body, "Routes stale"),
        "1",
        "the routes-stale card must count status=stale routes; body:\n{body}"
    );
    assert_eq!(
        card_value(&body, "Tokens (24h)"),
        TOKENS_SUM.to_string(),
        "the Tokens (24h) card must equal the SUM of windowed usage items' total_tokens \
         ({TOKENS_A} + {TOKENS_B}); body:\n{body}"
    );

    // --- Providers panel: only openai / openai-compatible kinds render; the weird kind is
    // hidden (never by name) and a banner mentions hidden providers.
    assert!(
        body.contains(fx.prov_openai.as_str()),
        "the openai provider must render by name; body:\n{body}"
    );
    assert!(
        body.contains(fx.prov_compat.as_str()),
        "the openai-compatible provider must render by name; body:\n{body}"
    );
    assert!(
        !body.contains(fx.prov_weird.as_str()),
        "a provider with an unsupported kind must NOT render by name; body:\n{body}"
    );
    assert!(
        body.to_lowercase().contains("hidden"),
        "a banner must mention that a provider was hidden; body:\n{body}"
    );

    // --- Routes panel: backend chains as provider names joined with "→" in priority order
    // (the priority-0 provider FIRST, even though the JSON lists it second).
    let chain_row = row_containing(&body, &fx.route_chain);
    assert!(
        chain_row.contains('→'),
        "the backend chain must join provider names with \"→\"; row:\n{chain_row}"
    );
    let pos_compat = chain_row
        .find(fx.prov_compat.as_str())
        .unwrap_or_else(|| panic!("chain must name {:?}; row:\n{chain_row}", fx.prov_compat));
    let pos_openai = chain_row
        .find(fx.prov_openai.as_str())
        .unwrap_or_else(|| panic!("chain must name {:?}; row:\n{chain_row}", fx.prov_openai));
    assert!(
        pos_compat < pos_openai,
        "the priority-0 provider ({}) must come FIRST in the chain, before the priority-1 \
         provider ({}); row:\n{chain_row}",
        fx.prov_compat,
        fx.prov_openai
    );
    assert!(
        body.contains(fx.route_plain.as_str()) && body.contains(fx.route_stale.as_str()),
        "all routes must render; body:\n{body}"
    );
    let stale_row = row_containing(&body, &fx.route_stale);
    assert!(
        stale_row.contains("stale"),
        "the stale route's status must render; row:\n{stale_row}"
    );

    // --- Budgets: used / limit from STATE, a meter, verbatim mode pills.
    let hot_row = row_containing(&body, &fx.budget_hot);
    assert!(
        hot_row.contains("90 / 100"),
        "the hot budget must show state.used_units / state.limit_units (90 / 100); \
         row:\n{hot_row}"
    );
    assert!(
        hot_row.contains("enforcing"),
        "the hot budget's mode pill must say \"enforcing\" verbatim; row:\n{hot_row}"
    );
    let cool_row = row_containing(&body, &fx.budget_cool);
    assert!(
        cool_row.contains("10 / 1000"),
        "the cool budget must show state.used_units / state.limit_units (10 / 1000), \
         NOT the spec limit; row:\n{cool_row}"
    );
    assert!(
        cool_row.contains("shadow"),
        "the cool budget's mode pill must say \"shadow\" verbatim; row:\n{cool_row}"
    );
    assert!(
        body.contains("meter"),
        "budget rows must render a meter; body:\n{body}"
    );
    assert!(
        body.contains("pill"),
        "budget modes must render as pills; body:\n{body}"
    );

    // --- Near-limit banner: names the ≥ 80% budget, and ONLY that one.
    let lower = body.to_lowercase();
    assert!(
        body.contains("ai-near-limit") || lower.contains("near limit") || body.contains("≥ 80%"),
        "a near-limit warning banner must render for the 90% budget; body:\n{body}"
    );
    let banner_start = body
        .find("ai-near-limit")
        .or_else(|| lower.find("near limit"))
        .expect("banner located above");
    let banner = &body[banner_start
        ..banner_start
            + body[banner_start..]
                .find("</div>")
                .unwrap_or(body.len() - banner_start)];
    assert!(
        banner.contains(fx.budget_hot.as_str()),
        "the near-limit banner must NAME the ≥ 80% budget {:?}; banner:\n{banner}",
        fx.budget_hot
    );
    assert!(
        !banner.contains(fx.budget_cool.as_str()),
        "a budget below 80% (of its STATE window) must NOT appear in the near-limit banner; \
         banner:\n{banner}"
    );

    // --- Journal: the three list fetches, page envelope query. (route-configs mapping is S5.)
    let recorded = stub.recorded();
    let base = format!("/api/v1/teams/{}", fx.team);
    assert_paged_fetch(&recorded, &format!("{base}/ai/providers"));
    assert_paged_fetch(&recorded, &format!("{base}/ai/routes"));
    assert_paged_fetch(&recorded, &format!("{base}/ai/budgets"));

    // WINDOW CONTRACT: exactly ONE usage fetch for this render, exact 24h RFC3339 window.
    let usage = usage_fetches(&recorded, &fx.team);
    assert_eq!(
        usage.len(),
        1,
        "EXACTLY ONE usage fetch per partial render; got: {usage:?}"
    );
    assert_usage_window(&usage[0]);

    // --- Second render: one MORE usage fetch (still exactly one per render), same window
    // contract.
    let resp = fetch(&http, &dash.overview_partial_url()).await;
    assert_eq!(resp.status().as_u16(), 200);
    let second = resp.text().await.expect("second overview body");
    let usage = usage_fetches(&stub.recorded(), &fx.team);
    assert_eq!(
        usage.len(),
        2,
        "each partial render performs exactly one usage fetch (2 renders → 2 fetches); \
         got: {usage:?}"
    );
    assert_usage_window(&usage[1]);

    assert_no_secret_paths(&stub.recorded());
    assert_bearer_and_no_leak(&stub.recorded(), &[&body, &second]);
}

// =============================================================================================
// Test 3: DEGRADATION — providers upstream 403 → HTTP 200 partial with a "Not authorized"
// section (and none of the team's data); providers upstream 500 → HTTP 200 partial with an
// "unavailable" state. No token leak in either body.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_overview_degrades_on_providers_403_and_500() {
    let http = client();

    // Providers upstream 403 → not-authorized section.
    {
        let fx = ai_fixture();
        let team = fx.team.clone();
        let prov = fx.prov_openai.clone();
        let budget = fx.budget_hot.clone();
        let mut state = fx.stub_state;
        state.providers_status = 403;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.overview_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 403 must not fail the overview partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("not authorized"),
            "the overview must say \"Not authorized\" on upstream 403; body:\n{body}"
        );
        assert!(
            !body.contains(prov.as_str()) && !body.contains(budget.as_str()),
            "no team data may render on 403; body:\n{body}"
        );
        assert_no_secret_paths(&stub.recorded());
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }

    // Providers upstream 500 → unavailable state.
    {
        let fx = ai_fixture();
        let team = fx.team.clone();
        let mut state = fx.stub_state;
        state.providers_status = 500;
        let stub = start_stub(state).await;
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
        let resp = fetch(&http, &dash.overview_partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "an upstream 500 must not fail the overview partial itself"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.to_lowercase().contains("unavailable"),
            "the overview must render an \"unavailable\" state on upstream 500; body:\n{body}"
        );
        assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
    }
}

// =============================================================================================
// Test 4: AUTH EXPIRY — upstream 401 → HTTP 286 (htmx stop-polling) naming
// `flowplane auth login`, same conventions as every other tab. No token leak.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_overview_upstream_401_yields_286_naming_auth_login() {
    let fx = ai_fixture();
    let team = fx.team.clone();
    let mut state = fx.stub_state;
    state.providers_status = 401;
    let stub = start_stub(state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let resp = fetch(&http, &dash.overview_partial_url()).await;
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
    assert_bearer_and_no_leak(&stub.recorded(), &[&body]);
}

// =============================================================================================
// Test 5 (reconcile pass 1, Codex finding): USAGE PAGINATION — a first page of exactly 500
// grouped rows must trigger a follow-up page fetch; the Tokens (24h) card sums ALL pages;
// and every usage page fetch in one render carries the IDENTICAL captured since/until pair
// ("one captured instant" — not "one HTTP request").
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_overview_sums_usage_across_pages_under_one_window_pair() {
    let mut fx = ai_fixture();
    // 500 rows of 7 tokens fill page 1 exactly; 3 rows of 17 land on page 2.
    fx.stub_state.usage = (0..503u64)
        .map(|i| usage_item(1000 + i, 200, if i < 500 { 7 } else { 17 }))
        .collect();
    let expected_sum = 500 * 7 + 3 * 17;
    let stub = start_stub(fx.stub_state).await;
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &fx.team);
    let http = client();

    let resp = fetch(&http, &dash.overview_partial_url()).await;
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("overview body");

    assert_eq!(
        card_value(&body, "Tokens (24h)"),
        expected_sum.to_string(),
        "the Tokens (24h) card must sum EVERY usage page, not just the first; body:\n{body}"
    );

    let usage = usage_fetches(&stub.recorded(), &fx.team);
    assert_eq!(
        usage.len(),
        2,
        "a full first page (500 rows) must trigger exactly one follow-up page; got: {usage:?}"
    );
    for req in &usage {
        assert_usage_window(req);
    }
    let q0 = usage[0].usage_query();
    let q1 = usage[1].usage_query();
    assert_eq!(
        (q0.since.as_deref(), q0.until.as_deref()),
        (q1.since.as_deref(), q1.until.as_deref()),
        "every usage page in one render must share the IDENTICAL captured window pair"
    );
    assert_eq!(usage[0].page().offset.unwrap_or(0), 0);
    assert_eq!(
        usage[1].page().offset,
        Some(500),
        "the follow-up fetch must continue at offset=500; got: {:?}",
        usage[1].query
    );
}
