//! fpv2-cxw.2 — `flowplane dashboard` Resources explorer: TOPOLOGY view
//! (black-box, spec-driven contract suite).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * New partial `GET /<nonce>/partials/resources/topology`. It sweeps the SAME three
//!     paged team-scoped list GETs as the tables view
//!     (`/api/v1/teams/{team}/{clusters,route-configs,listeners}`, limit=500/offset walk)
//!     and renders the listener → route-config → route → cluster chain.
//!   * Cluster chips carry a `data-cluster="<name>"` hover-highlight hook and show endpoint
//!     count, a TLS marker, and the lb policy. The view carries a visible best-effort /
//!     non-transactional snapshot label.
//!   * Weighted routes fan out to every referenced cluster with its weight.
//!   * Dangling references (listener → missing route config, route → missing cluster) are
//!     rendered as explicit "unresolved reference" markers — never silently dropped.
//!   * Past the node budget the view degrades to table markup (no chips / no
//!     `data-cluster=`) with a visible "budget" notice, still naming the resources.
//!   * Failure states: collection 403 → HTTP 200 partial saying not authorized naming the
//!     collection; first-page 500 → HTTP 200 saying unavailable naming the collection;
//!     upstream 401 → HTTP 286 naming `flowplane auth login` with `HX-Retarget: #resources`.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique
//! resource names; nothing binds a fixed port. Every spawned server is killed via a Drop
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

/// A distinctive bearer token (the topology suite never asserts on it directly, but the
/// child needs a token to be considered logged in).
const SECRET_TOKEN: &str = "sekret-topology-token-do-not-leak-4f7a";

/// The documented sweep page size.
const PAGE_LIMIT: u64 = 500;

/// Unique, DIGIT-FREE name: hex chars of a v7 uuid mapped onto 'g'..'v'. Digit-free names
/// keep numeric assertions (endpoint count "3", weights "90"/"10") from being satisfied
/// spuriously by a random name suffix.
fn unique(prefix: &str) -> String {
    let tail: String = uuid::Uuid::now_v7().simple().to_string()[20..]
        .chars()
        .map(|c| {
            let v = c.to_digit(16).expect("hex digit");
            char::from(b'g' + v as u8)
        })
        .collect();
    format!("{prefix}-{tail}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the three paged team-scoped
// collection endpoints from EXPLICIT fixture item lists, with real limit/offset paging,
// canned failures, and a request journal. Unknown paths are answered 404.
// =============================================================================================

/// One collection's canned behavior: explicit items (paged by limit/offset) or a failure.
#[derive(Clone)]
struct Collection {
    items: Vec<Value>,
    /// Status for EVERY page of this collection (200 = healthy).
    status: u16,
    /// Inject a failure at exactly this offset: `(offset, status)`.
    fail_at_offset: Option<(u64, u16)>,
}

impl Collection {
    fn ok(items: Vec<Value>) -> Self {
        Self {
            items,
            status: 200,
            fail_at_offset: None,
        }
    }

    fn failing(status: u16) -> Self {
        Self {
            items: Vec::new(),
            status,
            fail_at_offset: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    query: String,
}

/// The paging query the dashboard is documented to send: `limit=500&offset=N`.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
struct PageQuery {
    limit: Option<u64>,
    offset: Option<u64>,
}

struct StubState {
    clusters: Collection,
    route_configs: Collection,
    listeners: Collection,
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

/// Wrap a spec into the uniform stored-resource item shape. Timestamps deliberately use
/// only the digits 0/1/2/6 so they can never satisfy a numeric content assertion ("3",
/// "90", "10") spuriously.
fn resource_item(name: &str, spec: Value) -> Value {
    json!({
        "id": uuid::Uuid::now_v7().to_string(),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": spec
    })
}

/// A cluster item. `hosts_ports` chooses digits carefully per test (numeric assertions).
fn cluster_item(name: &str, endpoints: &[(&str, u16)], use_tls: bool, lb_policy: &str) -> Value {
    let eps: Vec<Value> = endpoints
        .iter()
        .map(|(h, p)| json!({ "host": h, "port": p }))
        .collect();
    resource_item(
        name,
        json!({ "endpoints": eps, "use_tls": use_tls, "lb_policy": lb_policy }),
    )
}

/// A listener item; `route_config: None` omits the binding key entirely (unbound listener).
fn listener_item(name: &str, route_config: Option<&str>) -> Value {
    let mut spec = json!({ "address": "0.0.0.0", "port": 8080 });
    if let Some(rc) = route_config {
        spec["route_config"] = json!(rc);
    }
    resource_item(name, spec)
}

/// A route-config item from explicit virtual hosts.
fn route_config_item(name: &str, virtual_hosts: Value) -> Value {
    resource_item(name, json!({ "virtual_hosts": virtual_hosts }))
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    state.requests.lock().unwrap().push(Recorded {
        path: path.clone(),
        query,
    });

    let cfg = if path.ends_with("/clusters") {
        &state.clusters
    } else if path.ends_with("/route-configs") {
        &state.route_configs
    } else if path.ends_with("/listeners") {
        &state.listeners
    } else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "no such route" })),
        )
            .into_response();
    };

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

    let total = cfg.items.len() as u64;
    let start = offset.min(total) as usize;
    let end = offset.saturating_add(limit).min(total) as usize;
    Json(json!({
        "items": cfg.items[start..end].to_vec(),
        "total": total,
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
    clusters: Collection,
    route_configs: Collection,
    listeners: Collection,
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
    fn topology_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/topology",
            self.port, self.nonce
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

/// Extract the full HTML element (opening tag through matching close, depth-tracked on the
/// tag name) whose opening tag contains `attr`. Panics with the body if absent.
fn element_with_attr<'a>(body: &'a str, attr: &str) -> &'a str {
    let attr_idx = body
        .find(attr)
        .unwrap_or_else(|| panic!("body must contain {attr:?}; body:\n{body}"));
    let open_idx = body[..attr_idx]
        .rfind('<')
        .unwrap_or_else(|| panic!("{attr:?} must sit inside an HTML tag; body:\n{body}"));
    let tag: String = body[open_idx + 1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    assert!(
        !tag.is_empty(),
        "{attr:?} must sit inside a named HTML element; body:\n{body}"
    );

    // Walk forward counting same-tag opens/closes (with a word boundary after the tag name)
    // to find this element's own close.
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}");
    let boundary_ok = |s: &str, pat_len: usize| {
        s[pat_len..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'))
    };
    let mut depth = 0usize;
    let mut pos = open_idx + 1;
    loop {
        let next_open = body[pos..]
            .match_indices(&open_pat)
            .find(|(i, _)| boundary_ok(&body[pos + i..], open_pat.len()))
            .map(|(i, _)| pos + i);
        let next_close = body[pos..]
            .match_indices(&close_pat)
            .find(|(i, _)| boundary_ok(&body[pos + i..], close_pat.len()))
            .map(|(i, _)| pos + i);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                pos = o + 1;
            }
            (_, Some(c)) => {
                if depth == 0 {
                    let end = body[c..].find('>').map(|i| c + i + 1).unwrap_or(body.len());
                    return &body[open_idx..end];
                }
                depth -= 1;
                pos = c + 1;
            }
            // No matching close (void/self-closing element): fall back to the rest of the
            // body from the opening tag — a superset, still anchored at the element.
            _ => return &body[open_idx..],
        }
    }
}

/// Every recorded upstream request must target one of the three collection list paths.
fn assert_upstream_allowlist(recorded: &[Recorded], team: &str) {
    let allowed = [
        format!("/api/v1/teams/{team}/clusters"),
        format!("/api/v1/teams/{team}/route-configs"),
        format!("/api/v1/teams/{team}/listeners"),
    ];
    for req in recorded {
        assert!(
            allowed.contains(&req.path),
            "the topology sweep sent an upstream request outside the documented set: {:?} \
             (allowed: {allowed:?})",
            req.path
        );
    }
}

// =============================================================================================
// Test 1: CHAIN RENDERING (AC 1) — one listener → one route config → one route → one cluster
// (3 endpoints, TLS, least-request). The chip carries the data-cluster hover hook and shows
// endpoint count / TLS / lb policy; the view carries the best-effort snapshot label; the
// sweep hits only the three collection paths with limit=500.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_renders_full_listener_route_cluster_chain() {
    let cluster = unique("cl");
    let rc = unique("rc");
    let listener = unique("ls");
    let route = unique("route");

    // Endpoint hosts/ports deliberately contain no digit '3' so the endpoint-count "3"
    // assertion inside the chip cannot be satisfied by an address.
    let stub = start_stub(
        Collection::ok(vec![cluster_item(
            &cluster,
            &[("10.0.0.1", 8080), ("10.0.0.2", 8080), ("10.0.0.4", 8080)],
            true,
            "least-request",
        )]),
        Collection::ok(vec![route_config_item(
            &rc,
            json!([{
                "name": "vh",
                "domains": ["e.com"],
                "routes": [{
                    "name": route,
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": cluster }
                }]
            }]),
        )]),
        Collection::ok(vec![listener_item(&listener, Some(&rc))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/partials/resources/topology must be 200 on a healthy sweep"
    );
    let body = resp.text().await.expect("topology body");

    // Every node of the chain is named.
    for (what, name) in [
        ("listener", &listener),
        ("route config", &rc),
        ("route", &route),
        ("cluster", &cluster),
    ] {
        assert!(
            body.contains(name.as_str()),
            "topology must render the {what} name {name:?}; body:\n{body}"
        );
    }

    // The cluster chip: an element carrying data-cluster="<name>" (hover-highlight hook)
    // whose content includes the cluster name, endpoint count 3, a TLS marker, and the
    // lb policy.
    let hook = format!("data-cluster=\"{cluster}\"");
    let chip = element_with_attr(&body, &hook);
    assert!(
        chip.contains(&cluster),
        "the cluster name must appear inside the element carrying {hook}; element:\n{chip}"
    );
    assert!(
        chip.contains('3'),
        "the cluster chip must show the endpoint count 3; element:\n{chip}"
    );
    assert!(
        chip.to_lowercase().contains("tls"),
        "the cluster chip must carry a TLS marker for use_tls=true; element:\n{chip}"
    );
    assert!(
        chip.contains("least-request"),
        "the cluster chip must show the lb policy \"least-request\"; element:\n{chip}"
    );

    // Visible snapshot label.
    let lower = body.to_lowercase();
    assert!(
        lower.contains("best-effort"),
        "the topology view must carry the visible \"best-effort\" snapshot label; body:\n{body}"
    );
    assert!(
        lower.contains("non-transactional snapshot"),
        "the topology view must carry the visible \"non-transactional snapshot\" label; \
         body:\n{body}"
    );

    // Sweep contract: only the three collection list paths, each page with limit=500.
    let recorded = stub.recorded();
    assert!(
        !recorded.is_empty(),
        "the topology partial must sweep the upstream collections"
    );
    assert_upstream_allowlist(&recorded, &team);
    for req in &recorded {
        let uri: axum::http::Uri = format!("/q?{}", req.query).parse().expect("recorded query");
        let page = Query::<PageQuery>::try_from_uri(&uri)
            .map(|q| q.0)
            .unwrap_or_default();
        assert_eq!(
            page.limit,
            Some(PAGE_LIMIT),
            "every topology sweep request must carry limit=500; got query {:?}",
            req.query
        );
    }
}

// =============================================================================================
// Test 2: WEIGHTED FAN-OUT (AC 1) — a route with weighted_clusters blue(90)/green(10), both
// clusters existing → both cluster names and both weights render.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_renders_weighted_cluster_fan_out_with_weights() {
    let blue = unique("blue");
    let green = unique("green");
    let rc = unique("rc");
    let listener = unique("ls");

    // Endpoint host contains neither "90" nor "10" so the weight assertions stay meaningful.
    let eps: &[(&str, u16)] = &[("172.22.44.66", 8080)];
    let stub = start_stub(
        Collection::ok(vec![
            cluster_item(&blue, eps, false, "round-robin"),
            cluster_item(&green, eps, false, "round-robin"),
        ]),
        Collection::ok(vec![route_config_item(
            &rc,
            json!([{
                "name": "vh",
                "domains": ["e.com"],
                "routes": [{
                    "name": "split",
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "weighted_clusters": [
                        { "cluster": blue, "weight": 90 },
                        { "cluster": green, "weight": 10 }
                    ] }
                }]
            }]),
        )]),
        Collection::ok(vec![listener_item(&listener, Some(&rc))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(resp.status().as_u16(), 200, "topology partial must be 200");
    let body = resp.text().await.expect("topology body");

    assert!(
        body.contains(&blue),
        "a weighted route must fan out to BOTH clusters — missing {blue:?}; body:\n{body}"
    );
    assert!(
        body.contains(&green),
        "a weighted route must fan out to BOTH clusters — missing {green:?}; body:\n{body}"
    );
    assert!(
        body.contains("90"),
        "the weighted fan-out must render the weight 90; body:\n{body}"
    );
    assert!(
        body.contains("10"),
        "the weighted fan-out must render the weight 10; body:\n{body}"
    );
}

// =============================================================================================
// Test 3: UNRESOLVED REFERENCES (AC 9) — a listener bound to a route config the sweep does
// not return, and a route whose action cluster does not exist, are both rendered as
// explicit "unresolved reference" markers. Nothing is silently dropped.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_marks_dangling_references_as_unresolved() {
    let ghost_rc = unique("ghost-rc");
    let ghost_cluster = unique("ghost-cl");
    let dangling_listener = unique("ls-dangling");
    let rc = unique("rc");
    let listener = unique("ls");

    let stub = start_stub(
        // No clusters at all — the route's cluster reference is dangling.
        Collection::ok(vec![]),
        Collection::ok(vec![route_config_item(
            &rc,
            json!([{
                "name": "vh",
                "domains": ["e.com"],
                "routes": [{
                    "name": "r1",
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": ghost_cluster }
                }]
            }]),
        )]),
        Collection::ok(vec![
            // Bound to a route config the sweep never returns.
            listener_item(&dangling_listener, Some(&ghost_rc)),
            listener_item(&listener, Some(&rc)),
        ]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(resp.status().as_u16(), 200, "topology partial must be 200");
    let body = resp.text().await.expect("topology body");
    let lower = body.to_lowercase();

    assert!(
        lower.contains("unresolved reference"),
        "dangling references must be marked \"unresolved reference\"; body:\n{body}"
    );
    assert!(
        body.contains(&ghost_rc),
        "the missing route-config name {ghost_rc:?} must still be rendered (not silently \
         dropped); body:\n{body}"
    );
    assert!(
        body.contains(&ghost_cluster),
        "the missing cluster name {ghost_cluster:?} must still be rendered (not silently \
         dropped); body:\n{body}"
    );
    assert!(
        body.contains(&dangling_listener),
        "the listener with the dangling binding ({dangling_listener:?}) must still render; \
         body:\n{body}"
    );
}

// =============================================================================================
// Test 4: UNBOUND LISTENER — a listener without route_config renders with an explicit
// unbound indication.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_renders_unbound_listener_with_indication() {
    let listener = unique("ls-unbound");
    let stub = start_stub(
        Collection::ok(vec![]),
        Collection::ok(vec![]),
        Collection::ok(vec![listener_item(&listener, None)]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(resp.status().as_u16(), 200, "topology partial must be 200");
    let body = resp.text().await.expect("topology body");
    let lower = body.to_lowercase();

    assert!(
        body.contains(&listener),
        "the unbound listener {listener:?} must still render; body:\n{body}"
    );
    assert!(
        lower.contains("no route config") || lower.contains("unbound"),
        "an unbound listener must carry an explicit indication (\"no route config\" or \
         \"unbound\"); body:\n{body}"
    );
}

// =============================================================================================
// Test 5: DEGRADE PAST NODE BUDGET (AC 6) — one route config with 50 vhosts x 200 routes
// (10,000 routes, the legal single-config maximum) must push the view over its node budget:
// 200 partial with a visible "budget" notice, table markup instead of chips (no
// data-cluster=), still naming the listener and route config.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_degrades_to_tables_past_node_budget() {
    let cluster = unique("cl");
    let rc = unique("rc-huge");
    let listener = unique("ls");

    // 50 vhosts x 200 routes = 10,000 routes, built once.
    let vhosts: Vec<Value> = (0..50)
        .map(|v| {
            let routes: Vec<Value> = (0..200)
                .map(|r| {
                    json!({
                        "name": format!("r-{v}-{r}"),
                        "match": { "prefix": { "prefix": "/" } },
                        "action": { "cluster": cluster }
                    })
                })
                .collect();
            json!({
                "name": format!("vh-{v}"),
                "domains": [format!("d{v}.e.com")],
                "routes": routes
            })
        })
        .collect();

    let stub = start_stub(
        Collection::ok(vec![cluster_item(
            &cluster,
            &[("172.22.44.66", 8080)],
            false,
            "round-robin",
        )]),
        Collection::ok(vec![route_config_item(&rc, Value::Array(vhosts))]),
        Collection::ok(vec![listener_item(&listener, Some(&rc))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the topology partial must still be 200 past the node budget"
    );
    let body = resp.text().await.expect("topology body");
    let lower = body.to_lowercase();

    assert!(
        lower.contains("budget"),
        "past the node budget the view must carry a visible degrade notice naming the \
         budget; body starts:\n{}",
        &body[..body.len().min(4000)]
    );
    assert!(
        !body.contains("data-cluster="),
        "past the node budget the view must degrade to tables — no chips, so no \
         data-cluster= hooks; body starts:\n{}",
        &body[..body.len().min(4000)]
    );
    assert!(
        body.contains(&listener),
        "the degraded table view must still name the listener {listener:?}"
    );
    assert!(
        body.contains(&rc),
        "the degraded table view must still name the route config {rc:?}"
    );
}

// =============================================================================================
// Test 6a: TOPOLOGY FAILURE STATE — clusters 403 → topology partial is HTTP 200 saying not
// authorized and naming the clusters collection.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_reports_forbidden_clusters_collection() {
    let rc = unique("rc");
    let listener = unique("ls");
    let stub = start_stub(
        Collection::failing(403),
        Collection::ok(vec![route_config_item(
            &rc,
            json!([{
                "name": "vh",
                "domains": ["e.com"],
                "routes": [{
                    "name": "r1",
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": "whatever" }
                }]
            }]),
        )]),
        Collection::ok(vec![listener_item(&listener, Some(&rc))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an upstream 403 on one collection must not fail the topology partial itself"
    );
    let body = resp.text().await.expect("topology body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("not authorized"),
        "the topology partial must say not authorized when clusters return 403; body:\n{body}"
    );
    assert!(
        lower.contains("clusters"),
        "the not-authorized state must name the clusters collection; body:\n{body}"
    );
}

// =============================================================================================
// Test 6b: TOPOLOGY FAILURE STATE — route-configs 500 on the FIRST page → HTTP 200 saying
// unavailable and naming the route-configs collection.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_reports_unavailable_route_configs_collection() {
    let cluster = unique("cl");
    let listener = unique("ls");
    let stub = start_stub(
        Collection::ok(vec![cluster_item(
            &cluster,
            &[("172.22.44.66", 8080)],
            false,
            "round-robin",
        )]),
        Collection::failing(500),
        Collection::ok(vec![listener_item(&listener, None)]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an upstream 500 on the route-configs first page must not fail the topology \
         partial itself"
    );
    let body = resp.text().await.expect("topology body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("unavailable"),
        "the topology partial must say unavailable when route-configs 500 on the first \
         page; body:\n{body}"
    );
    assert!(
        lower.contains("route configs") || lower.contains("route-configs"),
        "the unavailable state must name the route-configs collection; body:\n{body}"
    );
}

// =============================================================================================
// Test 7: upstream 401 on any collection → HTTP 286 (htmx stop-polling) naming
// `flowplane auth login`, retargeting #resources.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_unauthorized_upstream_returns_286_and_names_auth_login() {
    let stub = start_stub(
        Collection::failing(401),
        Collection::ok(vec![]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        286,
        "upstream 401 must yield the htmx stop-polling status 286"
    );
    assert_eq!(
        resp.headers()
            .get("HX-Retarget")
            .and_then(|v| v.to_str().ok()),
        Some("#resources"),
        "the 286 response must carry HX-Retarget: #resources"
    );
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("flowplane auth login"),
        "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
    );
}

// =============================================================================================
// Test 9: mid-sweep failure on page 2 of a collection → topology still renders (best-effort)
// with a partial-data banner naming the incomplete collection.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topology_mid_sweep_failure_renders_partial_banner_naming_collection() {
    let listener = unique("edge");
    let rc = unique("routes");
    let target = unique("checkout");
    // 700 clusters: the target on page 1, then page 2 (offset 500) fails with 500.
    let mut clusters = vec![cluster_item(
        &target,
        &[("172.20.4.7", 8181)],
        false,
        "round-robin",
    )];
    for i in 1..700 {
        clusters.push(cluster_item(
            &format!("{}-{i}", unique("filler")),
            &[("172.20.4.8", 8181)],
            false,
            "round-robin",
        ));
    }
    let mut clusters = Collection::ok(clusters);
    clusters.fail_at_offset = Some((500, 500));

    let vhosts = json!([{ "name": "vh", "domains": ["e.example.com"], "routes": [
        { "name": unique("r"), "match": { "prefix": { "prefix": "/" } },
          "action": { "cluster": target } }
    ]}]);
    let stub = start_stub(
        clusters,
        Collection::ok(vec![route_config_item(&rc, vhosts)]),
        Collection::ok(vec![listener_item(&listener, Some(&rc))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.topology_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a mid-sweep failure must not fail the whole topology partial"
    );
    let body = resp.text().await.expect("body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("partial data") && lower.contains("clusters"),
        "the partial-data banner must name the incomplete clusters collection; body:\n{body}"
    );
    assert!(
        body.contains(&listener) && body.contains(&target),
        "topology must still render the chain best-effort from page-1 data; body:\n{body}"
    );
}
