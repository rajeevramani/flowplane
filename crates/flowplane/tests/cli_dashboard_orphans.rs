//! fpv2-cxw.6 — `flowplane dashboard` Resources explorer: ORPHANS panel (black-box,
//! spec-driven contract suite).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * Partial `GET /<nonce>/partials/resources/orphans`. It performs paged sweeps of SIX
//!     team-scoped list collections (uniform `{items,total,limit,offset}` envelope):
//!     `/api/v1/teams/{team}/listeners`, `/route-configs`, `/clusters`,
//!     `/rate-limit-domains`, `/secrets`, `/ai/providers`.
//!   * Four orphan kinds are flagged (design AC 3), each with a labeled row:
//!       1. "unreferenced cluster" — a cluster no route action, aggregate-cluster list, or
//!          filter-level cluster reference names;
//!       2. "unbound listener" — a listener without `route_config`;
//!       3. "unattached rate-limit domain" — no BUILT-IN limiter listener
//!          (`service_cluster` exactly `rate_limit_cluster`, composed
//!          `<uuid36>|<uuid36>|<base>` domain) attaches it; external limiter clusters are
//!          EXCLUDED from attachment;
//!       4. "unreferenced secret" — matched under neither the SDS-name key nor the
//!          AI-provider credential-UUID key.
//!   * FALSE-POSITIVE GUARDS: a cluster referenced by a route action, a secret referenced
//!     by AI-provider credential UUID, and a domain attached via a composed built-in
//!     limiter domain must NOT be flagged.
//!   * SUPPRESSION (design AC 9, the slice's core safety rule): if ANY of the six sweeps
//!     is incomplete (mid-sweep page failure, 403, …) the partial is HTTP 200, the body
//!     carries a suppression notice (case-insensitive "suppressed"), and NO orphan-kind
//!     claim phrase ("unreferenced cluster", "unbound listener", "unattached",
//!     "unreferenced secret") may appear — even when a genuine orphan exists in the data
//!     the sweep did retrieve.
//!   * Upstream 401 → HTTP 286 + `HX-Retarget: #resources` naming `flowplane auth login`.
//!     The bearer token never appears in any response body or header.
//!
//! Fixture shapes are taken from the public contracts in `fp-domain` (`ListenerSpec`,
//! `ClusterSpec`, `RouteConfigSpec`, `AiProviderSpec`, gateway filters — all
//! `deny_unknown_fields`) and `fp-api` (`SecretView` metadata; rate-limit-domain items
//! carry no spec) — NOT from the dashboard implementation.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique,
//! digit-free resource names; nothing binds a fixed port. Every spawned server is killed
//! via a Drop guard in all paths, including assertion failures.

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

/// A distinctive bearer token so any leak into a dashboard response is unambiguous.
const SECRET_TOKEN: &str = "sekret-orphans-token-do-not-leak-9c1d";

/// The built-in limiter's `service_cluster` sentinel value (documented contract).
const BUILT_IN_CLUSTER: &str = "rate_limit_cluster";

/// The four orphan-kind claim phrases the suppression contract forbids (case-insensitive).
const CLAIM_PHRASES: [&str; 4] = [
    "unreferenced cluster",
    "unbound listener",
    "unattached",
    "unreferenced secret",
];

/// Unique, DIGIT-FREE name: hex chars of a v7 uuid mapped onto 'g'..'v'. Digit-free names
/// keep content assertions from being satisfied spuriously by a random name suffix.
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

/// Deterministic id, so the AI-provider join fixture controls exactly which secret id
/// equals the provider's `credential_secret_id`.
fn det_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving the SIX documented collection
// list paths with real limit/offset paging, canned failures (whole-collection or injected at
// an exact offset), and a full request journal. Everything else is recorded and 404'd.
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

/// The six collections the orphans partial is documented to sweep.
struct Fixture {
    listeners: Collection,
    route_configs: Collection,
    clusters: Collection,
    domains: Collection,
    secrets: Collection,
    ai_providers: Collection,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            listeners: Collection::ok(vec![]),
            route_configs: Collection::ok(vec![]),
            clusters: Collection::ok(vec![]),
            domains: Collection::ok(vec![]),
            secrets: Collection::ok(vec![]),
            ai_providers: Collection::ok(vec![]),
        }
    }
}

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Recorded for journal completeness (Debug output in failure messages shows it).
    #[allow(dead_code)]
    query: String,
    authorization: Option<String>,
}

/// The paging query the dashboard is documented to send: `limit=500&offset=N`.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
struct PageQuery {
    limit: Option<u64>,
    offset: Option<u64>,
}

struct StubState {
    fixture: Fixture,
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

/// Serve one page of a collection with the uniform `{items,total,limit,offset}` envelope,
/// honoring whole-collection failures and exact-offset failure injection.
fn serve_page(cfg: &Collection, page: PageQuery) -> Response {
    if cfg.status != 200 {
        return canned_error(cfg.status);
    }
    let offset = page.offset.unwrap_or(0);
    if let Some((fail_offset, fail_status)) = cfg.fail_at_offset {
        if offset == fail_offset {
            return canned_error(fail_status);
        }
    }
    let limit = page.limit.unwrap_or(50) as usize;
    let offset = offset as usize;
    let end = offset.saturating_add(limit).min(cfg.items.len());
    let items: Vec<Value> = cfg.items.get(offset..end).unwrap_or(&[]).to_vec();
    Json(json!({
        "items": items,
        "total": cfg.items.len(),
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

    let f = &state.fixture;
    if path.ends_with("/listeners") {
        return serve_page(&f.listeners, page);
    }
    if path.ends_with("/route-configs") {
        return serve_page(&f.route_configs, page);
    }
    if path.ends_with("/clusters") {
        return serve_page(&f.clusters, page);
    }
    if path.ends_with("/rate-limit-domains") {
        return serve_page(&f.domains, page);
    }
    if path.ends_with("/secrets") {
        return serve_page(&f.secrets, page);
    }
    if path.ends_with("/ai/providers") {
        return serve_page(&f.ai_providers, page);
    }
    // Everything else is recorded (so allowlist assertions see it) and answered 404.
    canned_error(404)
}

async fn start_stub(fixture: Fixture) -> StubUpstream {
    let state = Arc::new(StubState {
        fixture,
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
// Fixture builders — shapes per the public fp-domain / fp-api contracts.
// =============================================================================================

/// A cluster item with plain endpoints (no TLS, nothing referencing anything).
fn cluster_item(i: u64, name: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": { "endpoints": [{ "host": "10.0.0.1", "port": 8080 }] }
    })
}

/// A listener item; `route_config: None` omits the binding key entirely (unbound listener).
fn listener_item(i: u64, name: &str, route_config: Option<&str>) -> Value {
    let mut spec = json!({ "address": "0.0.0.0", "port": 8080 });
    if let Some(rc) = route_config {
        spec["route_config"] = json!(rc);
    }
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": spec
    })
}

/// A BOUND listener carrying one built-in `global_rate_limit` filter whose persisted domain
/// is the CP-composed `<uuid36>|<uuid36>|<base>` value (attachment mechanism, AC 3).
fn built_in_limiter_listener_item(i: u64, name: &str, route_config: &str, domain: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "address": "0.0.0.0",
            "port": 8081,
            "route_config": route_config,
            "http_filters": [{
                "filter": {
                    "type": "global_rate_limit",
                    "domain": domain,
                    "service_cluster": BUILT_IN_CLUSTER,
                }
            }]
        }
    })
}

/// A route-config item with one vhost and one route whose action targets `cluster`.
fn route_config_item(i: u64, name: &str, route: &str, cluster: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "virtual_hosts": [{
                "name": "vh",
                "domains": ["e.example.com"],
                "routes": [{
                    "name": route,
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": cluster }
                }]
            }]
        }
    })
}

/// A rate-limit-domain item: NO `spec` field (documented shape).
fn domain_item(i: u64, name: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
    })
}

/// A `SecretView` item (fp-api metadata shape, `value_redacted: true`).
fn secret_item(id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "team_id": det_id(999),
        "name": name,
        "description": "",
        "secret_type": "tls_certificate",
        "revision": 1,
        "encryption_key_id": "k1",
        "expires_at": Value::Null,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "value_redacted": true,
    })
}

/// An `AiProviderView` item whose spec references a secret by UUID (`credential_secret_id`).
fn ai_provider_item(i: u64, name: &str, credential_secret_id: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "kind": "openai",
            "base_url": "https://api.openai.com",
            "credential_secret_id": credential_secret_id,
        }
    })
}

/// A fresh CP-composed persisted domain value: `<36-char-UUID>|<36-char-UUID>|<base>`.
fn composed_domain(base: &str) -> String {
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    assert_eq!(a.len(), 36);
    assert_eq!(b.len(), 36);
    format!("{a}|{b}|{base}")
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
    fn orphans_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/orphans",
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

/// True iff some occurrence of `anchor` in `haystack` has `needle` within `window` bytes on
/// either side. Used for "orphan-kind label with its resource name" adjacency without
/// assuming markup order. Window edges are snapped to char boundaries.
fn near(haystack: &str, anchor: &str, needle: &str, window: usize) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(anchor) {
        let idx = start + pos;
        let mut lo = idx.saturating_sub(window);
        while !haystack.is_char_boundary(lo) {
            lo -= 1;
        }
        let mut hi = (idx + anchor.len() + window).min(haystack.len());
        while !haystack.is_char_boundary(hi) {
            hi += 1;
        }
        if haystack[lo..hi].contains(needle) {
            return true;
        }
        start = idx + anchor.len();
    }
    false
}

/// The suppression contract: no orphan-kind claim phrase may appear anywhere in the body.
fn assert_no_orphan_claims(lower: &str, body: &str, why: &str) {
    for phrase in CLAIM_PHRASES {
        assert!(
            !lower.contains(phrase),
            "SUPPRESSION (AC 9) violated: with {why}, the orphan-claim phrase {phrase:?} \
             must NOT appear anywhere in the partial; body:\n{body}"
        );
    }
}

/// The six documented upstream list paths for the orphans partial.
fn allowed_paths(team: &str) -> [String; 6] {
    [
        format!("/api/v1/teams/{team}/listeners"),
        format!("/api/v1/teams/{team}/route-configs"),
        format!("/api/v1/teams/{team}/clusters"),
        format!("/api/v1/teams/{team}/rate-limit-domains"),
        format!("/api/v1/teams/{team}/secrets"),
        format!("/api/v1/teams/{team}/ai/providers"),
    ]
}

// =============================================================================================
// Test 1 (AC 3): ALL FOUR ORPHAN KINDS + FALSE-POSITIVE GUARDS — exactly one defect of each
// kind plus one healthy counterpart of each:
//   * cluster:  c_orphan referenced by nothing  vs  c_used named by a route action;
//   * listener: l_unbound (no route_config)     vs  l_bound (bound to the route config);
//   * domain:   d_orphan (nothing attaches it)  vs  d_attached, attached via a BUILT-IN
//               limiter listener with the composed `<uuid>|<uuid>|<base>` domain;
//   * secret:   s_orphan (no SDS name, no UUID) vs  s_used, whose ID equals the AI
//               provider's `credential_secret_id`.
// All four orphan rows must render naming the planted resources; none of the healthy
// counterparts may appear as an orphan claim. The sweep hits exactly the six documented
// collection paths.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_four_orphan_kinds_flagged_and_healthy_counterparts_are_not() {
    let c_orphan = unique("gg-lonecluster");
    let c_used = unique("hh-usedcluster");
    let l_unbound = unique("jj-lonelistener");
    let l_bound = unique("kk-boundlistener");
    let l_limiter = unique("ll-limiter");
    let d_orphan = unique("mm-lonedomain");
    let d_attached = unique("nn-useddomain");
    let s_orphan = unique("pp-lonesecret");
    let s_used = unique("qq-usedsecret");
    let rc = unique("rr-routes");
    let route = unique("rt");
    let provider = unique("ss-prov");

    // The provider's credential UUID equals the healthy secret's ID (join by ID, AC guard).
    let u_cred = det_id(90);

    let fixture = Fixture {
        listeners: Collection::ok(vec![
            listener_item(10, &l_unbound, None), // ORPHAN: unbound listener
            listener_item(11, &l_bound, Some(&rc)),
            // Attachment mechanism for d_attached: built-in limiter, composed domain.
            // Bound to the route config so it is not itself an unbound-listener orphan.
            built_in_limiter_listener_item(12, &l_limiter, &rc, &composed_domain(&d_attached)),
        ]),
        route_configs: Collection::ok(vec![route_config_item(20, &rc, &route, &c_used)]),
        clusters: Collection::ok(vec![
            cluster_item(30, &c_orphan), // ORPHAN: unreferenced cluster
            cluster_item(31, &c_used),   // referenced by the route action
        ]),
        domains: Collection::ok(vec![
            domain_item(40, &d_orphan),   // ORPHAN: unattached rate-limit domain
            domain_item(41, &d_attached), // attached via the composed built-in limiter
        ]),
        secrets: Collection::ok(vec![
            secret_item(&det_id(50), &s_orphan), // ORPHAN: unreferenced secret
            secret_item(&u_cred, &s_used),       // referenced by AI-provider credential UUID
        ]),
        ai_providers: Collection::ok(vec![ai_provider_item(60, &provider, &u_cred)]),
    };

    let stub = start_stub(fixture).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.orphans_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the orphans partial must be 200 on healthy, complete sweeps"
    );
    let body = resp.text().await.expect("orphans partial body");
    let lower = body.to_lowercase();

    // ---- All four orphan kinds flagged, each labeled row naming its planted resource. ----
    let flagged: [(&str, &String); 4] = [
        ("unreferenced cluster", &c_orphan),
        ("unbound listener", &l_unbound),
        ("unattached", &d_orphan),
        ("unreferenced secret", &s_orphan),
    ];
    for (label, orphan) in flagged {
        assert!(
            lower.contains(label),
            "the orphans partial must carry a {label:?} labeled row (AC 3); body:\n{body}"
        );
        assert!(
            body.contains(orphan.as_str()),
            "the {label:?} orphan row must name the planted resource {orphan:?}; \
             body:\n{body}"
        );
        assert!(
            near(&lower, label, orphan, 800),
            "the planted resource {orphan:?} must appear NEAR its {label:?} label \
             (a labeled row, not two disconnected mentions); body:\n{body}"
        );
    }

    // ---- FALSE-POSITIVE GUARDS: no healthy counterpart may appear inside any orphan
    // claim. Each healthy name must sit near NO claim phrase (if a healthy resource were
    // wrongly flagged, its name would render in a labeled row → near a claim phrase). ----
    let healthy: [(&str, &String); 5] = [
        ("route-action-referenced cluster", &c_used),
        ("bound listener", &l_bound),
        ("built-in limiter listener", &l_limiter),
        ("composed-domain-attached rate-limit domain", &d_attached),
        ("AI-credential-referenced secret", &s_used),
    ];
    for (what, name) in healthy {
        for phrase in CLAIM_PHRASES {
            assert!(
                !near(&lower, phrase, name, 800),
                "FALSE POSITIVE: the healthy {what} {name:?} appears near the orphan-claim \
                 phrase {phrase:?} — healthy counterparts must never be flagged; body:\n{body}"
            );
        }
    }

    // Token non-disclosure holds on the happy path too.
    assert!(
        !body.contains(SECRET_TOKEN),
        "the orphans partial body leaks the bearer token; body:\n{body}"
    );

    // ---- Sweep contract: the six documented collection paths, nothing else. ----
    // Grace period so even an asynchronously-fired extra upstream fetch would be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let recorded = stub.recorded();
    assert!(!recorded.is_empty(), "the upstream must have been called");
    let allowed = allowed_paths(&team);
    for req in &recorded {
        assert!(
            allowed.contains(&req.path),
            "the orphans partial sent an upstream request outside the documented six \
             collections (allowed: {allowed:?}): {:?}; all recorded: {recorded:?}",
            req.path
        );
    }
    for path in &allowed {
        assert!(
            recorded.iter().any(|r| &r.path == path),
            "the orphans partial must sweep all SIX collections — {path:?} was never \
             requested; recorded: {recorded:?}"
        );
    }
}

// =============================================================================================
// Test 2 (AC 9): SUPPRESSION ON MID-SWEEP FAILURE — clusters page 2 fails (700 items, 500 at
// offset 500) while a genuine unbound-listener orphan exists → HTTP 200, a suppression
// notice, and NO orphan-claim phrase anywhere (the planted orphan's claim is withheld).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_sweep_cluster_failure_suppresses_all_orphan_claims() {
    let l_unbound = unique("jj-lonelistener");

    // 700 clusters: page 1 (offset 0) succeeds, page 2 (offset 500) fails with 500.
    let mut clusters: Vec<Value> = Vec::with_capacity(700);
    for i in 0..700u64 {
        clusters.push(cluster_item(1000 + i, &format!("{}-{i}", unique("filler"))));
    }
    let mut clusters = Collection::ok(clusters);
    clusters.fail_at_offset = Some((500, 500));

    let fixture = Fixture {
        // A genuine orphan the suppressed partial must NOT claim.
        listeners: Collection::ok(vec![listener_item(10, &l_unbound, None)]),
        clusters,
        ..Default::default()
    };

    let stub = start_stub(fixture).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.orphans_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an incomplete sweep must not fail the orphans partial itself (HTTP 200 + \
         suppression notice)"
    );
    let body = resp.text().await.expect("orphans partial body");
    let lower = body.to_lowercase();

    assert!(
        lower.contains("suppressed"),
        "with the clusters sweep incomplete (page-2 500) the partial must carry a \
         suppression notice (case-insensitive \"suppressed\"); body:\n{body}"
    );
    assert_no_orphan_claims(
        &lower,
        &body,
        "the clusters sweep incomplete (page-2 500) and a genuine unbound listener planted",
    );
}

// =============================================================================================
// Test 3 (AC 9): SUPPRESSION ON 403 — the secrets collection is forbidden while genuine
// defects exist (unbound listener, unreferenced cluster) → HTTP 200, a suppression notice,
// and NO orphan-claim phrase anywhere.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_secrets_collection_suppresses_all_orphan_claims() {
    let l_unbound = unique("jj-lonelistener");
    let c_orphan = unique("gg-lonecluster");

    let fixture = Fixture {
        listeners: Collection::ok(vec![listener_item(10, &l_unbound, None)]),
        clusters: Collection::ok(vec![cluster_item(30, &c_orphan)]),
        secrets: Collection::failing(403),
        ..Default::default()
    };

    let stub = start_stub(fixture).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.orphans_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a secrets 403 must not fail the orphans partial itself (HTTP 200 + suppression \
         notice)"
    );
    let body = resp.text().await.expect("orphans partial body");
    let lower = body.to_lowercase();

    assert!(
        lower.contains("suppressed"),
        "with the secrets collection forbidden (403) the partial must carry a suppression \
         notice (case-insensitive \"suppressed\"); body:\n{body}"
    );
    assert_no_orphan_claims(
        &lower,
        &body,
        "the secrets collection forbidden (403) and genuine defects planted",
    );
}

// =============================================================================================
// Test 4: upstream 401 → HTTP 286 (htmx stop-polling) + `HX-Retarget: #resources` naming
// `flowplane auth login`; the bearer token never appears in any response body or header.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_upstream_returns_286_with_retarget_and_names_auth_login() {
    let fixture = Fixture {
        clusters: Collection::failing(401),
        ..Default::default()
    };

    let stub = start_stub(fixture).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.orphans_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        286,
        "an upstream 401 must yield the htmx stop-polling status 286"
    );
    assert_eq!(
        resp.headers()
            .get("HX-Retarget")
            .and_then(|v| v.to_str().ok()),
        Some("#resources"),
        "the 286 response must carry HX-Retarget: #resources"
    );
    for (name, value) in resp.headers() {
        let value_str = String::from_utf8_lossy(value.as_bytes()).to_string();
        assert!(
            !name.as_str().contains(SECRET_TOKEN) && !value_str.contains(SECRET_TOKEN),
            "response header {name:?} leaks the bearer token: {value_str:?}"
        );
    }
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("flowplane auth login"),
        "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
    );
    assert!(
        !body.contains(SECRET_TOKEN),
        "the 286 body leaks the bearer token; body:\n{body}"
    );

    // Sanity: the upstream sweep DID carry the token (so non-disclosure proves something).
    let recorded = stub.recorded();
    let want_auth = format!("Bearer {SECRET_TOKEN}");
    assert!(
        recorded
            .iter()
            .any(|r| r.authorization.as_deref() == Some(want_auth.as_str())),
        "upstream sweeps must carry the bearer token; recorded: {recorded:?}"
    );
}

// =============================================================================================
// Test 5: a malformed AI provider (unreadable credential_secret_id) suppresses the whole
// analysis — its credential secret must NEVER be claimed unreferenced.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_ai_provider_suppresses_all_orphan_claims() {
    let provider = unique("pp-brokenprovider");
    let s_cred = unique("ss-credsecret");
    // The provider item lacks credential_secret_id entirely — join-field validation
    // must suppress rather than let the secret surface as unreferenced.
    let broken_provider = json!({
        "id": det_id(70),
        "name": provider,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": { "kind": "openai", "base_url": "https://api.openai.com" }
    });
    let fixture = Fixture {
        secrets: Collection::ok(vec![secret_item(&det_id(71), &s_cred)]),
        ai_providers: Collection::ok(vec![broken_provider]),
        ..Default::default()
    };

    let stub = start_stub(fixture).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.orphans_url()).await;
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("orphans partial body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("suppressed"),
        "a malformed AI provider must suppress the analysis; body:\n{body}"
    );
    assert_no_orphan_claims(&lower, &body, "a malformed AI provider present");
    assert!(
        !body.contains(&s_cred) || !lower.contains("unreferenced secret"),
        "the credential secret must never be claimed unreferenced; body:\n{body}"
    );
}
