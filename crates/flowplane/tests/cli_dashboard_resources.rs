//! fpv2-cxw.1 — `flowplane dashboard` Resources explorer: sweep engine + tables view
//! (black-box, spec-driven contract suite).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * New routes under the nonce: `GET /<nonce>/resources` (HTML shell page) and three
//!     partials `GET /<nonce>/partials/resources/{clusters,route-configs,listeners}`.
//!   * The shell page is lazy: loading it triggers ZERO upstream requests; only fetching a
//!     partial sweeps that partial's collection.
//!   * The dashboard's ONLY upstream endpoints for these partials are the three paged
//!     team-scoped list GETs `/api/v1/teams/{team}/{clusters,route-configs,listeners}`,
//!     each with query `limit=500&offset=N`, walking offsets 0, 500, 1000, … until a short
//!     page. Responses are the uniform `{items, total, limit, offset}` envelope. No secret
//!     or value route is ever called.
//!   * Per-panel degradation: upstream 403 → that partial is HTTP 200 saying not
//!     authorized; other partials still render data. Mid-sweep 500/404 → HTTP 200 with a
//!     partial-data notice, still rendering the rows already fetched. First-page 500 →
//!     HTTP 200 with an "unavailable" state.
//!   * Upstream 401 → HTTP 286 (htmx stop-polling) naming `flowplane auth login`.
//!   * The bearer token never appears in any partial response body or header.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique
//! team/item names; nothing binds a fixed port, so the suite runs green under default
//! nextest parallelism. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;
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
const SECRET_TOKEN: &str = "sekret-resources-token-do-not-leak-9c1d";

/// The documented sweep page size.
const PAGE_LIMIT: u64 = 500;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the three paged team-scoped
// collection endpoints with real limit/offset paging, canned failures, and a full request
// journal (path + query string + auth header). Unknown paths are recorded too (so allowlist
// assertions see them) and answered 404.
// =============================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Clusters,
    RouteConfigs,
    Listeners,
}

impl Kind {
    fn segment(self) -> &'static str {
        match self {
            Kind::Clusters => "clusters",
            Kind::RouteConfigs => "route-configs",
            Kind::Listeners => "listeners",
        }
    }
}

/// One collection's canned behavior.
#[derive(Clone)]
struct CollectionCfg {
    /// Total items in the collection; pages are sliced from this per limit/offset.
    total: u64,
    /// Item-name prefix; item `i` is named `{prefix}-{i:04}`.
    prefix: String,
    /// Status for EVERY page of this collection (200 = healthy).
    status: u16,
    /// Inject a failure at exactly this offset: `(offset, status)`.
    fail_at_offset: Option<(u64, u16)>,
}

impl CollectionCfg {
    fn ok(total: u64, prefix: &str) -> Self {
        Self {
            total,
            prefix: prefix.to_string(),
            status: 200,
            fail_at_offset: None,
        }
    }

    fn failing(status: u16) -> Self {
        Self {
            total: 0,
            prefix: "unused".to_string(),
            status,
            fail_at_offset: None,
        }
    }

    fn item_name(&self, i: u64) -> String {
        format!("{}-{i:04}", self.prefix)
    }
}

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Raw query string (empty when absent) so offset/limit assertions can parse it.
    query: String,
    authorization: Option<String>,
}

impl Recorded {
    /// Parse this request's recorded query string with serde (same extractor as the stub).
    fn page(&self) -> PageQuery {
        let uri: axum::http::Uri = format!("/q?{}", self.query)
            .parse()
            .expect("recorded query");
        Query::<PageQuery>::try_from_uri(&uri)
            .map(|q| q.0)
            .unwrap_or_default()
    }
}

/// The paging query the dashboard is documented to send: `limit=500&offset=N`.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
struct PageQuery {
    limit: Option<u64>,
    offset: Option<u64>,
}

struct StubState {
    clusters: CollectionCfg,
    route_configs: CollectionCfg,
    listeners: CollectionCfg,
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

/// Deterministic id with no accidental digit collisions (a random UUID could contain e.g.
/// "1200" and satisfy a total-count assertion spuriously).
fn item_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

fn item(kind: Kind, cfg: &CollectionCfg, i: u64) -> Value {
    let spec = match kind {
        Kind::Clusters => json!({ "endpoints": [{ "host": "10.0.0.1", "port": 8080 }] }),
        Kind::Listeners => json!({ "address": "0.0.0.0", "port": 8080 }),
        Kind::RouteConfigs => json!({
            "virtual_hosts": [{
                "name": "vh",
                "domains": ["example.com"],
                "routes": [{
                    "name": "r",
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": "c-0" }
                }]
            }]
        }),
    };
    json!({
        "id": item_id(i),
        "name": cfg.item_name(i),
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T03:04:05Z",
        "spec": spec
    })
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

    let (kind, cfg) = if path.ends_with("/clusters") {
        (Kind::Clusters, &state.clusters)
    } else if path.ends_with("/route-configs") {
        (Kind::RouteConfigs, &state.route_configs)
    } else if path.ends_with("/listeners") {
        (Kind::Listeners, &state.listeners)
    } else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "no such route" })),
        )
            .into_response();
    };

    // Parse the paging query with serde via axum's Query extractor.
    let page: PageQuery = Query::<PageQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();
    let limit = page.limit.unwrap_or(50);
    let offset = page.offset.unwrap_or(0);

    if cfg.status != 200 {
        return canned_error(cfg.status);
    }
    if let Some((fail_offset, fail_status)) = cfg.fail_at_offset {
        if offset == fail_offset {
            return canned_error(fail_status);
        }
    }

    let end = offset.saturating_add(limit).min(cfg.total);
    let items: Vec<Value> = (offset..end).map(|i| item(kind, cfg, i)).collect();
    Json(json!({
        "items": items,
        "total": cfg.total,
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
    clusters: CollectionCfg,
    route_configs: CollectionCfg,
    listeners: CollectionCfg,
) -> StubUpstream {
    let state = Arc::new(StubState {
        clusters,
        route_configs,
        listeners,
        requests: Mutex::new(Vec::new()),
    });
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
    fn shell_url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/resources", self.port, self.nonce)
    }

    fn partial_url(&self, kind: Kind) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/{}",
            self.port,
            self.nonce,
            kind.segment()
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

    // Read the first stdout line on a std thread with a hard timeout, so a silent child
    // fails the test instead of hanging the suite.
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

/// The three allowed upstream collection paths for a team.
fn allowed_paths(team: &str) -> [String; 3] {
    [
        format!("/api/v1/teams/{team}/clusters"),
        format!("/api/v1/teams/{team}/route-configs"),
        format!("/api/v1/teams/{team}/listeners"),
    ]
}

/// Every recorded upstream request must target one of the three collection list paths, and
/// no path (including its query string) may mention "secret" or "value".
fn assert_upstream_allowlist(recorded: &[Recorded], team: &str) {
    let allowed = allowed_paths(team);
    for req in recorded {
        assert!(
            allowed.contains(&req.path),
            "the dashboard sent an upstream request outside the documented set: {:?} \
             (allowed: {allowed:?})",
            req.path
        );
        let full = format!("{}?{}", req.path, req.query).to_lowercase();
        assert!(
            !full.contains("secret") && !full.contains("value"),
            "upstream request must never target a secret/value route: {full:?}"
        );
    }
}

/// The partial-data notice must contain "partial" (case-insensitive) — but the count must
/// exceed the count of "partials" so an htmx URL like `/partials/resources/...` embedded in
/// the fragment cannot satisfy the assertion spuriously.
fn has_partial_data_notice(lower: &str) -> bool {
    lower.matches("partial").count() > lower.matches("partials").count()
}

// =============================================================================================
// Test 1: FIRST-OPEN LAZY LOADING — the `/resources` shell page triggers ZERO upstream
// requests; only fetching a partial sweeps that partial's collection.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resources_shell_is_lazy_and_partials_fetch_only_their_collection() {
    let cprefix = unique("cl");
    let stub = start_stub(
        CollectionCfg::ok(3, &cprefix),
        CollectionCfg::ok(3, &unique("rc")),
        CollectionCfg::ok(3, &unique("ls")),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // Shell page: 200, HTML.
    let resp = fetch(&http, &dash.shell_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/resources must serve the shell page"
    );
    let body = resp.text().await.expect("shell body");
    assert!(
        body.contains('<'),
        "the shell page must be HTML; body:\n{body}"
    );
    // Static wiring of first-open lazy loading: closed-by-default <details> panels
    // whose partial fetch fires on the first native toggle only.
    assert!(
        body.contains("<details") && !body.contains("<details open"),
        "panels must be closed-by-default <details> elements; body:\n{body}"
    );
    assert!(
        body.matches("hx-trigger=\"toggle once\"").count() >= 3,
        "each panel must fetch on its first toggle only; body:\n{body}"
    );

    // Grace period so even an asynchronously-fired upstream fetch would be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after_shell = stub.recorded();
    assert!(
        after_shell.is_empty(),
        "loading the shell page must trigger ZERO upstream requests (lazy loading); \
         recorded: {after_shell:?}"
    );

    // One partial (clusters) → upstream requests occurred ONLY for the clusters path.
    let resp = fetch(&http, &dash.partial_url(Kind::Clusters)).await;
    assert_eq!(resp.status().as_u16(), 200, "clusters partial must be 200");
    let body = resp.text().await.expect("clusters partial body");
    assert!(
        body.contains(&format!("{cprefix}-0000")),
        "clusters partial must render cluster rows; body:\n{body}"
    );

    let recorded = stub.recorded();
    assert!(
        !recorded.is_empty(),
        "fetching the clusters partial must call the upstream"
    );
    let clusters_path = format!("/api/v1/teams/{team}/clusters");
    for req in &recorded {
        assert_eq!(
            req.path, clusters_path,
            "after fetching ONLY the clusters partial, every upstream request must target \
             the clusters path; recorded: {recorded:?}"
        );
    }
    assert_upstream_allowlist(&recorded, &team);
}

// =============================================================================================
// Test 2: ALL PAGES TRAVERSED — 1200 clusters must be swept at offsets 0, 500, 1000 with
// limit=500, and the partial must render the total plus the first and last item.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clusters_sweep_walks_every_page_of_1200_items() {
    let cprefix = unique("cl");
    let clusters = CollectionCfg::ok(1200, &cprefix);
    let first_name = clusters.item_name(0);
    let last_name = clusters.item_name(1199);
    let stub = start_stub(
        clusters,
        CollectionCfg::ok(1, &unique("rc")),
        CollectionCfg::ok(1, &unique("ls")),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.partial_url(Kind::Clusters)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a fully successful sweep must yield a 200 partial"
    );
    let body = resp.text().await.expect("clusters partial body");

    assert!(
        body.contains("1200"),
        "the partial must show the total row count 1200; body:\n{body}"
    );
    assert!(
        body.contains(&first_name),
        "the partial must render the FIRST item ({first_name:?}) — page 1 of the sweep; \
         body:\n{body}"
    );
    assert!(
        body.contains(&last_name),
        "the partial must render the LAST item ({last_name:?}) — proof the sweep traversed \
         all pages, not just the first; body:\n{body}"
    );

    // Offset-walk contract: clusters requested at offsets 0, 500, 1000 (limit=500 each);
    // no other collection paths were touched.
    let recorded = stub.recorded();
    let clusters_path = format!("/api/v1/teams/{team}/clusters");
    let mut offsets = BTreeSet::new();
    for req in &recorded {
        assert_eq!(
            req.path, clusters_path,
            "the clusters sweep must not touch any other collection path; \
             recorded: {recorded:?}"
        );
        let page = req.page();
        assert_eq!(
            page.limit,
            Some(PAGE_LIMIT),
            "every sweep request must carry limit=500; got query {:?}",
            req.query
        );
        offsets.insert(page.offset.unwrap_or(0));
    }
    assert_eq!(
        offsets,
        BTreeSet::from([0, 500, 1000]),
        "the sweep must walk offsets 0, 500, 1000 (until the short page) and no others; \
         recorded: {recorded:?}"
    );
    assert_upstream_allowlist(&recorded, &team);
}

// =============================================================================================
// Test 3: UPSTREAM REQUEST ALLOWLIST — fetching all three partials produces requests ONLY to
// the three team-scoped collection paths; no secret/value route is ever called.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_partials_hit_only_the_three_collection_paths() {
    let cprefix = unique("cl");
    let rprefix = unique("rc");
    let lprefix = unique("ls");
    let stub = start_stub(
        CollectionCfg::ok(2, &cprefix),
        CollectionCfg::ok(2, &rprefix),
        CollectionCfg::ok(2, &lprefix),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    for (kind, prefix) in [
        (Kind::Clusters, &cprefix),
        (Kind::RouteConfigs, &rprefix),
        (Kind::Listeners, &lprefix),
    ] {
        let resp = fetch(&http, &dash.partial_url(kind)).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "{} partial must be 200",
            kind.segment()
        );
        let body = resp.text().await.expect("partial body");
        assert!(
            body.contains(&format!("{prefix}-0000")),
            "{} partial must render its rows; body:\n{body}",
            kind.segment()
        );
    }

    let recorded = stub.recorded();
    assert!(!recorded.is_empty(), "the upstream must have been called");
    assert_upstream_allowlist(&recorded, &team);
    // All three collections were actually swept.
    for path in allowed_paths(&team) {
        assert!(
            recorded.iter().any(|r| r.path == path),
            "collection path {path:?} must have been swept; recorded: {recorded:?}"
        );
    }
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
}

// =============================================================================================
// Test 4: PER-PANEL 403 — clusters forbidden → clusters partial is 200 saying not
// authorized; the listeners partial (200 upstream) still renders data.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_collection_degrades_only_its_own_partial() {
    let lprefix = unique("ls");
    let stub = start_stub(
        CollectionCfg::failing(403),
        CollectionCfg::ok(2, &unique("rc")),
        CollectionCfg::ok(2, &lprefix),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // Clusters: upstream 403 → partial is HTTP 200 with a not-authorized state.
    let resp = fetch(&http, &dash.partial_url(Kind::Clusters)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an upstream 403 must not fail the partial itself"
    );
    let body = resp.text().await.expect("clusters partial body");
    assert!(
        body.to_lowercase().contains("not authorized"),
        "the clusters partial must say not authorized on upstream 403; body:\n{body}"
    );

    // Listeners: healthy upstream → data still renders, no auth degradation.
    let resp = fetch(&http, &dash.partial_url(Kind::Listeners)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the listeners partial must be unaffected by the clusters 403"
    );
    let body = resp.text().await.expect("listeners partial body");
    assert!(
        body.contains(&format!("{lprefix}-0000")),
        "the listeners partial must still render its rows; body:\n{body}"
    );
    assert!(
        !body.to_lowercase().contains("not authorized"),
        "the listeners partial (200 upstream) must not show a not-authorized state; \
         body:\n{body}"
    );
    assert_upstream_allowlist(&stub.recorded(), &team);
}

// =============================================================================================
// Test 5: FAILURE CLASSES MID-SWEEP — page at offset 0 succeeds, offset 500 fails (500 and
// 404) → HTTP 200 with a partial-data notice, still rendering the rows from page 1.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_sweep_failure_yields_partial_data_notice_with_page_one_rows() {
    let http = client();
    for fail_status in [500u16, 404u16] {
        let cprefix = unique("cl");
        let mut clusters = CollectionCfg::ok(1200, &cprefix);
        clusters.fail_at_offset = Some((500, fail_status));
        let page_one_name = clusters.item_name(0);
        let stub = start_stub(
            clusters,
            CollectionCfg::ok(1, &unique("rc")),
            CollectionCfg::ok(1, &unique("ls")),
        )
        .await;
        let team = unique("team");
        let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

        let resp = fetch(&http, &dash.partial_url(Kind::Clusters)).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "a mid-sweep upstream {fail_status} must not fail the partial itself"
        );
        let body = resp.text().await.expect("clusters partial body");
        let lower = body.to_lowercase();
        assert!(
            has_partial_data_notice(&lower),
            "a mid-sweep upstream {fail_status} must produce a partial-data notice \
             (contains \"partial\"); body:\n{body}"
        );
        assert!(
            body.contains(&page_one_name),
            "rows already fetched (page 1, e.g. {page_one_name:?}) must still render after \
             a mid-sweep upstream {fail_status}; body:\n{body}"
        );
        assert_upstream_allowlist(&stub.recorded(), &team);
    }
}

// =============================================================================================
// Test 6: FIRST-PAGE FAILURE — 500 at offset 0 → HTTP 200, body says "unavailable".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_page_server_error_renders_unavailable_state() {
    let stub = start_stub(
        CollectionCfg::failing(500),
        CollectionCfg::ok(1, &unique("rc")),
        CollectionCfg::ok(1, &unique("ls")),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.partial_url(Kind::Clusters)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an upstream 500 on the first page must not fail the partial itself"
    );
    let body = resp.text().await.expect("clusters partial body");
    assert!(
        body.to_lowercase().contains("unavailable"),
        "a first-page upstream 500 must render an \"unavailable\" state; body:\n{body}"
    );
    assert_upstream_allowlist(&stub.recorded(), &team);
}

// =============================================================================================
// Test 7: upstream 401 → HTTP 286 (htmx stop-polling) naming `flowplane auth login`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_upstream_returns_286_and_names_auth_login() {
    let stub = start_stub(
        CollectionCfg::failing(401),
        CollectionCfg::ok(1, &unique("rc")),
        CollectionCfg::ok(1, &unique("ls")),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.partial_url(Kind::Clusters)).await;
    assert_eq!(
        resp.status().as_u16(),
        286,
        "upstream 401 must yield the htmx stop-polling status 286"
    );
    // Dashboard-global expiry: the banner must retarget the whole #resources main so
    // every panel is replaced, not just the panel that happened to fetch.
    assert_eq!(
        resp.headers()
            .get("HX-Retarget")
            .and_then(|v| v.to_str().ok()),
        Some("#resources"),
        "the 286 response must carry HX-Retarget: #resources"
    );
    assert_eq!(
        resp.headers()
            .get("HX-Reswap")
            .and_then(|v| v.to_str().ok()),
        Some("innerHTML"),
        "the 286 response must carry HX-Reswap: innerHTML"
    );
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("flowplane auth login"),
        "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
    );
}

// =============================================================================================
// Test 8: TOKEN NON-DISCLOSURE — the bearer token value never appears in any partial body
// or response header (shell page included).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bearer_token_never_appears_in_any_resources_response() {
    let stub = start_stub(
        CollectionCfg::ok(2, &unique("cl")),
        CollectionCfg::ok(2, &unique("rc")),
        CollectionCfg::ok(2, &unique("ls")),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let urls = [
        dash.shell_url(),
        dash.partial_url(Kind::Clusters),
        dash.partial_url(Kind::RouteConfigs),
        dash.partial_url(Kind::Listeners),
    ];
    for url in urls {
        let resp = fetch(&http, &url).await;
        assert_eq!(resp.status().as_u16(), 200, "GET {url} must be 200");
        for (name, value) in resp.headers() {
            let value_str = String::from_utf8_lossy(value.as_bytes()).to_string();
            assert!(
                !name.as_str().contains(SECRET_TOKEN) && !value_str.contains(SECRET_TOKEN),
                "response header {name:?} of {url} leaks the bearer token: {value_str:?}"
            );
        }
        let body = resp.text().await.expect("body");
        assert!(
            !body.contains(SECRET_TOKEN),
            "the response body of {url} leaks the bearer token; body:\n{body}"
        );
    }
    assert_upstream_allowlist(&stub.recorded(), &team);
}
