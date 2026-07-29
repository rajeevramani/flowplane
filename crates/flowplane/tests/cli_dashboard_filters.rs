//! fpv2-cxw.5 — `flowplane dashboard` Resources explorer: HTTP FILTERS inventory panel
//! (black-box, spec-driven contract suite).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * Partial `GET /<nonce>/partials/resources/filters`. It sweeps TWO paged team-scoped
//!     list GETs only: `/api/v1/teams/{team}/listeners` and
//!     `/api/v1/teams/{team}/route-configs` (never clusters, never secrets).
//!   * Listener items carry `spec.http_filters`: `[{"filter": {"type": "<kind>", ...},
//!     "disabled": bool?}]`. Rendered contract (AC 2, AC 8):
//!       1. chain rows show the DOMAIN kind names with full attribute grids (e.g.
//!          `global_rate_limit` shows domain/timeout_ms/failure_mode_deny with values);
//!       2. a `disabled: true` chain entry shows a disabled indication;
//!       3. every listener also shows a `router` row marked "synthesized" (the router
//!          filter is appended at translation, never authored);
//!       4. `filter_overrides` at BOTH vhost and route scope render with domain kind
//!          names and their scope (vhost name / vhost+route names);
//!       5. the "Kinds in use" footer lists kinds IN USE only — never the whole catalog;
//!       6. a large inline JWKS is truncated, never rendered in full.
//!   * Failure classes: upstream 401 → HTTP 286 + `HX-Retarget` naming
//!     `flowplane auth login`; listeners 403 → HTTP 200 saying not authorized, naming
//!     listeners; route-configs first-page 500 → HTTP 200 with an "unavailable" state
//!     naming route configs.
//!
//! Fixture shapes are taken from the public domain contract in
//! `crates/fp-domain/src/gateway/filters.rs` (NOT from the dashboard implementation). In
//! particular the `local_rate_limit` override nests its bucket:
//! `{"type":"local_rate_limit","stat_prefix":..,"token_bucket":{max_tokens,
//! tokens_per_fill,fill_interval_ms}}` — a flat shape would fail `deny_unknown_fields`
//! deserialization upstream and simply not render.
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

/// A distinctive bearer token so the child considers itself logged in.
const SECRET_TOKEN: &str = "sekret-filters-token-do-not-leak-4f7a";

/// Unique, DIGIT-FREE name: hex chars of a v7 uuid mapped onto 'g'..'v'. Digit-free names
/// keep numeric assertions (e.g. the "25" of timeout_ms) from being satisfied spuriously
/// by a random name suffix.
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

/// Deterministic id with no accidental digit collisions (a random UUID could contain e.g.
/// "25" and satisfy an attribute assertion spuriously).
fn det_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving TWO path families —
// `/listeners` and `/route-configs` — with real limit/offset paging, canned failures, and a
// full request journal. Everything else (clusters, secrets, …) is recorded (so allowlist
// assertions see it) and answered 404.
// =============================================================================================

/// One collection's canned behavior: explicit items (paged by limit/offset) or a failure.
#[derive(Clone)]
struct Collection {
    items: Vec<Value>,
    /// Status for EVERY page of this collection (200 = healthy).
    status: u16,
}

impl Collection {
    fn ok(items: Vec<Value>) -> Self {
        Self { items, status: 200 }
    }

    fn failing(status: u16) -> Self {
        Self {
            items: Vec::new(),
            status,
        }
    }
}

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Recorded for journal completeness (Debug output in failure messages shows it); no
    /// assertion currently parses it.
    #[allow(dead_code)]
    query: String,
}

/// The paging query the dashboard is documented to send: `limit=500&offset=N`.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
struct PageQuery {
    limit: Option<u64>,
    offset: Option<u64>,
}

struct StubState {
    listeners: Collection,
    route_configs: Collection,
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

/// Serve one page of a collection with the uniform `{items,total,limit,offset}` envelope.
fn serve_page(cfg: &Collection, page: PageQuery) -> Response {
    if cfg.status != 200 {
        return canned_error(cfg.status);
    }
    let limit = page.limit.unwrap_or(50) as usize;
    let offset = page.offset.unwrap_or(0) as usize;
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
    state.requests.lock().unwrap().push(Recorded {
        path: path.clone(),
        query,
    });

    let page: PageQuery = Query::<PageQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();

    if path.ends_with("/listeners") {
        return serve_page(&state.listeners, page);
    }
    if path.ends_with("/route-configs") {
        return serve_page(&state.route_configs, page);
    }
    // Everything else (clusters, secrets, …) is recorded and answered 404. The filters
    // partial must not need them, so a 404 here must never break a test.
    canned_error(404)
}

async fn start_stub(listeners: Collection, route_configs: Collection) -> StubUpstream {
    let state = Arc::new(StubState {
        listeners,
        route_configs,
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
// Fixture builders — shapes per the fp-domain public contract (gateway/filters.rs,
// gateway/listener.rs, gateway/route_config.rs), all `deny_unknown_fields`.
// =============================================================================================

/// A listener item whose `spec.http_filters` is the given entry array.
fn listener_item(i: u64, name: &str, http_filters: Value) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "address": "0.0.0.0",
            "port": 8080,
            "http_filters": http_filters,
        }
    })
}

/// A listener item with NO authored filter chain at all.
fn plain_listener_item(i: u64, name: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": { "address": "0.0.0.0", "port": 8080 }
    })
}

/// A route-config item with one vhost (with `filter_overrides`) containing one route
/// (with `filter_overrides`).
fn route_config_item(
    i: u64,
    name: &str,
    vhost: &str,
    route: &str,
    vhost_overrides: Value,
    route_overrides: Value,
) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "virtual_hosts": [{
                "name": vhost,
                "domains": ["example.com"],
                "filter_overrides": vhost_overrides,
                "routes": [{
                    "name": route,
                    "match": { "prefix": { "prefix": "/" } },
                    "action": { "cluster": "upstream-svc" },
                    "filter_overrides": route_overrides,
                }]
            }]
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
    fn filters_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/filters",
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
/// either side. Used for "label with its value" / "kind with its scope" adjacency without
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

// =============================================================================================
// Test 1 (AC 2, AC 8 points 1-5): FULL INVENTORY — one rich fixture set:
//   listener A: chain [global_rate_limit, jwt_auth, cors(disabled)],
//   listener B: no authored chain at all,
//   route-config: vhost override cors, route overrides disable(compressor) + jwt_auth.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filters_inventory_renders_chains_router_rows_overrides_and_kinds_footer() {
    let listener_a = unique("edge");
    let listener_b = unique("bare");
    let rc_name = unique("rc");
    let vhost = unique("vh");
    let route = unique("rt");
    let grl_domain = unique("dom");

    let chain = json!([
        {
            "filter": {
                "type": "global_rate_limit",
                "domain": grl_domain,
                "service_cluster": "rate_limit_cluster",
                "timeout_ms": 25,
                "failure_mode_deny": true,
            }
        },
        {
            "filter": {
                "type": "jwt_auth",
                "providers": {
                    "main": {
                        "issuer": "https://idp.example.com",
                        "jwks": { "source": "inline", "jwks": "{\"keys\":[]}" }
                    }
                }
            }
        },
        // Disabled chain entry: cors is a kind the footer expects anyway (via the vhost
        // override), so the exact-kinds assertion holds whether or not disabled chain
        // entries count as "in use".
        {
            "filter": {
                "type": "cors",
                "allow_origin": [{ "match": "exact", "value": "https://a.example.com" }],
                "allow_methods": ["GET"],
            },
            "disabled": true,
        },
    ]);
    let vhost_overrides = json!([
        {
            "type": "cors",
            "allow_origin": [{ "match": "exact", "value": "https://a.example.com" }],
            "allow_methods": ["GET"],
        },
    ]);
    let route_overrides = json!([
        { "type": "disable", "filter_type": "compressor" },
        { "type": "jwt_auth", "requirement_name": "strict" },
    ]);

    let stub = start_stub(
        Collection::ok(vec![
            listener_item(0, &listener_a, chain),
            plain_listener_item(1, &listener_b),
        ]),
        Collection::ok(vec![route_config_item(
            2,
            &rc_name,
            &vhost,
            &route,
            vhost_overrides,
            route_overrides,
        )]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.filters_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the filters partial must be 200 on healthy upstreams"
    );
    let body = resp.text().await.expect("filters partial body");
    let lower = body.to_lowercase();

    // ---- (1) Chain rows: DOMAIN kind names with full attribute grids. ----
    assert!(
        lower.contains("global_rate_limit"),
        "the chain row must show the domain kind name global_rate_limit; body:\n{body}"
    );
    assert!(
        lower.contains("jwt_auth"),
        "the chain row must show the domain kind name jwt_auth; body:\n{body}"
    );
    assert!(
        near(&lower, "domain", &grl_domain, 400),
        "the global_rate_limit attribute grid must show \"domain\" with its value \
         {grl_domain:?}; body:\n{body}"
    );
    assert!(
        near(&lower, "timeout_ms", "25", 300),
        "the global_rate_limit attribute grid must show \"timeout_ms\" with \"25\"; \
         body:\n{body}"
    );
    assert!(
        near(&lower, "failure_mode_deny", "true", 300),
        "the global_rate_limit attribute grid must show \"failure_mode_deny\" with \
         \"true\"; body:\n{body}"
    );

    // ---- (2) The disabled chain entry (cors) shows a disabled indication. ----
    assert!(
        near(&lower, "cors", "disabled", 400),
        "the disabled: true cors chain entry must show a disabled indication; body:\n{body}"
    );

    // ---- (3) EVERY listener shows a synthesized router row — including the listener
    // with no authored chain at all (the router is appended at translation). ----
    let ai = body
        .find(&listener_a)
        .unwrap_or_else(|| panic!("listener {listener_a:?} must render; body:\n{body}"));
    let bi = body
        .find(&listener_b)
        .unwrap_or_else(|| panic!("listener {listener_b:?} must render; body:\n{body}"));
    let (first_region, second_region) = if ai < bi {
        (&lower[ai..bi], &lower[bi..])
    } else {
        (&lower[bi..ai], &lower[ai..])
    };
    // Regions are keyed on find-order; each listener's region must carry its own
    // synthesized router row regardless of which listener renders first.
    for region in [first_region, second_region] {
        assert!(
            region.contains("router"),
            "every listener must show a router row (region for one listener lacks it); \
             region:\n{region}\nfull body:\n{body}"
        );
        assert!(
            region.contains("synthesized"),
            "the router row must be marked \"synthesized\" for every listener; \
             region:\n{region}\nfull body:\n{body}"
        );
    }

    // ---- (4) Overrides at BOTH scopes render with domain kind names + their scope. ----
    assert!(
        body.contains(&vhost),
        "the vhost-scope override must name its vhost {vhost:?}; body:\n{body}"
    );
    assert!(
        body.contains(&route),
        "the route-scope override must name its route {route:?}; body:\n{body}"
    );
    assert!(
        near(&lower, &vhost, "cors", 800),
        "the cors vhost override must render next to its vhost scope {vhost:?}; \
         body:\n{body}"
    );
    assert!(
        near(&lower, &route, "compressor", 800),
        "the disable(compressor) route override must render next to its route scope \
         {route:?}; body:\n{body}"
    );
    assert!(
        near(&lower, &route, "jwt_auth", 800),
        "the jwt_auth route override must render next to its route scope {route:?}; \
         body:\n{body}"
    );

    // ---- (5) Footer lists kinds IN USE only. ----
    let fi = lower.find("kinds in use").unwrap_or_else(|| {
        panic!("the partial must have a \"Kinds in use\" footer; body:\n{body}")
    });
    let footer = &lower[fi..];
    for kind in ["cors", "compressor", "global_rate_limit", "jwt_auth"] {
        assert!(
            footer.contains(kind),
            "the kinds footer must list in-use kind {kind:?}; footer:\n{footer}"
        );
    }
    for unused in ["ext_authz", "rbac", "header_mutation", "health_check"] {
        assert!(
            !footer.contains(unused),
            "the kinds footer must list kinds IN USE only — unused kind {unused:?} must \
             not appear; footer:\n{footer}"
        );
    }
}

// =============================================================================================
// Test 2 (AC 8 point 6): INLINE JWKS TRUNCATION — a 64 KiB inline JWKS must never appear in
// full in the body; a truncation indicator ("chars)") must be present. The same fixture set
// carries a route-level local_rate_limit override in its VALID fp-domain shape (nested
// token_bucket) to prove it renders.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_jwks_is_truncated_and_never_rendered_in_full() {
    let listener = unique("edge");
    let rc_name = unique("rc");
    let vhost = unique("vh");
    let route = unique("rt");

    // A 65536-char inline JWKS (the fp-domain maximum). Non-uniform content so the
    // full-string containment check is meaningful.
    let jwks: String = (0..65536u32)
        .map(|i| char::from(b'a' + (i.wrapping_mul(7) % 26) as u8))
        .collect();
    assert_eq!(jwks.len(), 65536);

    let chain = json!([
        {
            "filter": {
                "type": "jwt_auth",
                "providers": {
                    "main": {
                        "issuer": "https://idp.example.com",
                        "jwks": { "source": "inline", "jwks": jwks }
                    }
                }
            }
        },
    ]);
    // Valid local_rate_limit override per fp-domain (LocalRateLimitConfig): stat_prefix +
    // nested token_bucket. A flat {max_tokens, ...} shape would be rejected by
    // deny_unknown_fields and never render.
    let route_overrides = json!([
        {
            "type": "local_rate_limit",
            "stat_prefix": "lr",
            "token_bucket": { "max_tokens": 30, "tokens_per_fill": 30, "fill_interval_ms": 1000 },
        },
    ]);

    let stub = start_stub(
        Collection::ok(vec![listener_item(0, &listener, chain)]),
        Collection::ok(vec![route_config_item(
            1,
            &rc_name,
            &vhost,
            &route,
            json!([]),
            route_overrides,
        )]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.filters_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the filters partial must be 200"
    );
    let body = resp.text().await.expect("filters partial body");

    // The jwt_auth chain row itself renders …
    assert!(
        body.to_lowercase().contains("jwt_auth"),
        "the jwt_auth chain row must render; body starts:\n{}",
        &body[..body.len().min(2000)]
    );
    // … but the 64 KiB inline JWKS never appears in full …
    assert!(
        !body.contains(&jwks),
        "the 65536-char inline JWKS must NOT appear in full in the body (body length {})",
        body.len()
    );
    // … and a truncation indicator is present.
    assert!(
        body.contains("chars)"),
        "a truncation indicator (\"chars)\") must be present for the oversized inline \
         JWKS; body:\n{body}"
    );

    // The correctly-shaped local_rate_limit route override renders with its domain kind name.
    assert!(
        body.to_lowercase().contains("local_rate_limit"),
        "the local_rate_limit route override (valid nested token_bucket shape) must \
         render; body:\n{body}"
    );
}

// =============================================================================================
// Test 3a: upstream 401 → HTTP 286 + HX-Retarget naming `flowplane auth login`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_upstream_returns_286_with_retarget_and_names_auth_login() {
    let stub = start_stub(Collection::failing(401), Collection::failing(401)).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.filters_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        286,
        "upstream 401 must yield the htmx stop-polling status 286"
    );
    let retarget = resp
        .headers()
        .get("HX-Retarget")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert!(
        retarget.as_deref().is_some_and(|v| !v.is_empty()),
        "the 286 response must carry an HX-Retarget header; got {retarget:?}"
    );
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("flowplane auth login"),
        "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
    );
}

// =============================================================================================
// Test 3b: listeners 403 → HTTP 200 saying not authorized, naming listeners.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_listeners_partial_says_not_authorized_naming_listeners() {
    let stub = start_stub(
        Collection::failing(403),
        Collection::ok(vec![route_config_item(
            0,
            &unique("rc"),
            &unique("vh"),
            &unique("rt"),
            json!([]),
            json!([]),
        )]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.filters_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a listeners 403 must not fail the filters partial itself"
    );
    let body = resp.text().await.expect("filters partial body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("not authorized"),
        "the partial must say not authorized on a listeners 403; body:\n{body}"
    );
    assert!(
        near(&lower, "not authorized", "listener", 300),
        "the not-authorized state must name listeners (the forbidden collection); \
         body:\n{body}"
    );
}

// =============================================================================================
// Test 3c: route-configs first-page 500 → HTTP 200 with an "unavailable" state naming
// route configs.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_configs_first_page_500_renders_unavailable_naming_route_configs() {
    let stub = start_stub(
        Collection::ok(vec![plain_listener_item(0, &unique("ls"))]),
        Collection::failing(500),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.filters_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a route-configs first-page 500 must not fail the filters partial itself"
    );
    let body = resp.text().await.expect("filters partial body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("unavailable"),
        "a route-configs first-page 500 must render an \"unavailable\" state; body:\n{body}"
    );
    assert!(
        near(&lower, "unavailable", "route", 300),
        "the unavailable state must name route configs (the failing collection); \
         body:\n{body}"
    );
}

// =============================================================================================
// Test 4: UPSTREAM REQUEST ALLOWLIST — the filters partial requests ONLY the two collection
// list paths (listeners + route-configs); never clusters, never secrets.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filters_partial_hits_only_listeners_and_route_configs() {
    let listener = unique("ls");
    let rc_name = unique("rc");
    let stub = start_stub(
        Collection::ok(vec![plain_listener_item(0, &listener)]),
        Collection::ok(vec![route_config_item(
            1,
            &rc_name,
            &unique("vh"),
            &unique("rt"),
            json!([]),
            json!([]),
        )]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.filters_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the filters partial must be 200"
    );
    let _ = resp.text().await.expect("filters partial body");

    // Grace period so even an asynchronously-fired extra upstream fetch would be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let recorded = stub.recorded();
    assert!(!recorded.is_empty(), "the upstream must have been called");
    let listeners_path = format!("/api/v1/teams/{team}/listeners");
    let route_configs_path = format!("/api/v1/teams/{team}/route-configs");
    for req in &recorded {
        assert!(
            req.path == listeners_path || req.path == route_configs_path,
            "the filters partial sent an upstream request outside the documented set \
             (only {listeners_path:?} and {route_configs_path:?} are allowed): {:?}; \
             all recorded: {recorded:?}",
            req.path
        );
        let full = format!("{}?{}", req.path, req.query).to_lowercase();
        assert!(
            !full.contains("cluster") && !full.contains("secret"),
            "the filters partial must never target a clusters/secrets route: {full:?}"
        );
    }
    // Both collections were actually swept.
    for path in [&listeners_path, &route_configs_path] {
        assert!(
            recorded.iter().any(|r| &r.path == path),
            "collection path {path:?} must have been swept; recorded: {recorded:?}"
        );
    }
}
