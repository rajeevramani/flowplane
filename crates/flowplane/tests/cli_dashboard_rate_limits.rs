//! fpv2-cxw.4 — `flowplane dashboard` Resources explorer: RATE LIMITS panel
//! (black-box, spec-driven contract suite).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * Partial `GET /<nonce>/partials/resources/rate-limits`. It sweeps TWO paged
//!     team-scoped list GETs (limit=500/offset walk, uniform `{items,total,limit,offset}`
//!     envelope): `/api/v1/teams/{team}/rate-limit-domains` and
//!     `/api/v1/teams/{team}/listeners` (listeners feed the attachment join).
//!   * ATTACHMENT SEMANTICS: the panel says a domain is "attached by <listener names>" and
//!     never claims policy references. A listener's `global_rate_limit` filter is the
//!     BUILT-IN limiter iff `service_cluster` is exactly `rate_limit_cluster`; its persisted
//!     domain value is the CP-composed `<36-char-UUID>|<36-char-UUID>|<base domain>`, and
//!     the dashboard must match it to the domain row named `<base domain>` by structurally
//!     stripping exactly that prefix (never split-on-`|` — a base domain may itself contain
//!     `|`). Any other `service_cluster` is an EXTERNAL limiter and is EXCLUDED from
//!     attachment claims entirely. A domain no built-in listener attaches shows an
//!     "unattached" indication.
//!   * Per-domain policies are LAZY: the rate-limits partial itself never fetches policies;
//!     opening a domain row triggers
//!     `GET /<nonce>/partials/resources/rate-limit-policies?domain=<percent-encoded name>`
//!     which sweeps `/api/v1/teams/{team}/rate-limit-domains/<percent-encoded name>/policies`.
//!   * Failure classes: upstream 401 → HTTP 286 with `HX-Retarget: #resources` naming
//!     `flowplane auth login`; 403 on rate-limit-domains → HTTP 200 saying not authorized;
//!     listeners 403/unavailable → domains still render, attachment declared UNKNOWN
//!     (never "unattached").
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
const SECRET_TOKEN: &str = "sekret-rate-limits-token-do-not-leak-2b8e";

/// The built-in limiter's `service_cluster` sentinel value (documented contract).
const BUILT_IN_CLUSTER: &str = "rate_limit_cluster";

/// Unique, DIGIT-FREE name: hex chars of a v7 uuid mapped onto 'g'..'v'. Digit-free names
/// keep numeric assertions (e.g. the "40" of a 40/minute policy) from being satisfied
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

/// Deterministic id with no accidental digit collisions (a random UUID could contain e.g.
/// "40" and satisfy a policy-limit assertion spuriously).
fn det_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving FOUR path families —
// `/rate-limit-domains`, `/listeners`, any `/policies` sub-path, and 404 for everything else
// (clusters/route-configs included) — with real limit/offset paging, canned failures, and a
// full request journal (path + query). Percent-encoding in request paths is preserved
// verbatim in the journal (axum does not decode `uri().path()`), so encoding assertions see
// exactly what the dashboard sent.
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
    domains: Collection,
    listeners: Collection,
    policies: Collection,
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

/// A listener item carrying one `global_rate_limit` http filter with the given persisted
/// domain value and service cluster.
fn rl_listener_item(i: u64, name: &str, domain: &str, service_cluster: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "address": "0.0.0.0",
            "port": 8080,
            "http_filters": [{
                "filter": {
                    "type": "global_rate_limit",
                    "domain": domain,
                    "service_cluster": service_cluster,
                }
            }]
        }
    })
}

/// A listener item with NO rate-limit filter at all.
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

/// A policy item under a domain (documented shape).
fn policy_item(i: u64, name: &str, domain_id: &str) -> Value {
    json!({
        "id": det_id(i),
        "domain_id": domain_id,
        "name": name,
        "spec": {
            "descriptors": { "k": "v" },
            "requests_per_unit": 40,
            "unit": "minute",
        },
        "descriptors_canonical": "k=v",
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
    })
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
    // `uri().path()` is the raw (still percent-encoded) path as sent on the wire.
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    state.requests.lock().unwrap().push(Recorded {
        path: path.clone(),
        query,
    });

    let page: PageQuery = Query::<PageQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();

    // Policies sub-path family first: `/rate-limit-domains/<name>/policies`.
    if path.contains("/policies") {
        return serve_page(&state.policies, page);
    }
    if path.ends_with("/rate-limit-domains") {
        return serve_page(&state.domains, page);
    }
    if path.ends_with("/listeners") {
        return serve_page(&state.listeners, page);
    }
    // Everything else (clusters, route-configs, …) is recorded and answered 404. The
    // rate-limits partial must not need them, so a 404 here must never break a test.
    canned_error(404)
}

async fn start_stub(
    domains: Collection,
    listeners: Collection,
    policies: Collection,
) -> StubUpstream {
    let state = Arc::new(StubState {
        domains,
        listeners,
        policies,
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
    fn rate_limits_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/rate-limits",
            self.port, self.nonce
        )
    }

    /// The lazy per-domain policies partial. `encoded_domain` must already be
    /// percent-encoded exactly as the caller wants it on the wire.
    fn policies_url(&self, encoded_domain: &str) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/rate-limit-policies?domain={}",
            self.port, self.nonce, encoded_domain
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

/// True iff `lower` contains an "attached by" occurrence that is NOT part of
/// "unattached by" — a positive attachment claim.
fn has_positive_attached_by(lower: &str) -> bool {
    let b = lower.as_bytes();
    let pat: &[u8] = b"attached by";
    if b.len() < pat.len() {
        return false;
    }
    for i in 0..=(b.len() - pat.len()) {
        if &b[i..i + pat.len()] == pat {
            let preceded_by_un = i >= 2 && &b[i - 2..i] == b"un";
            if !preceded_by_un {
                return true;
            }
        }
    }
    false
}

/// A fresh CP-composed persisted domain value: `<36-char-UUID>|<36-char-UUID>|<base>`.
fn composed_domain(base: &str) -> String {
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    assert_eq!(a.len(), 36);
    assert_eq!(b.len(), 36);
    format!("{a}|{b}|{base}")
}

/// No `/policies` upstream request may have been made (lazy-loading contract).
fn assert_no_policies_requests(recorded: &[Recorded]) {
    for req in recorded {
        assert!(
            !req.path.contains("/policies"),
            "the rate-limits partial must NOT fetch any policies (lazy contract); \
             recorded a policies request: {:?}",
            req.path
        );
    }
}

// =============================================================================================
// Test 1 (AC 5, AC 10): ATTACHMENT — two built-in listeners attach the checkout domain via
// the composed `<uuid>|<uuid>|<base>` value; an EXTERNAL limiter (service_cluster != rate_
// limit_cluster) whose domain equals the idle domain's name verbatim must be excluded
// entirely, leaving idle unattached.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn built_in_listeners_attach_domain_and_external_limiter_is_excluded() {
    let checkout = unique("checkout");
    let idle = unique("idle");
    let edge_a = unique("edge-a");
    let edge_b = unique("edge-b");
    let edge_ext = unique("edge-ext");

    let composed = composed_domain(&checkout);
    let stub = start_stub(
        Collection::ok(vec![domain_item(0, &checkout), domain_item(1, &idle)]),
        Collection::ok(vec![
            rl_listener_item(10, &edge_a, &composed, BUILT_IN_CLUSTER),
            rl_listener_item(11, &edge_b, &composed, BUILT_IN_CLUSTER),
            // EXTERNAL limiter: its domain equals the idle domain's name verbatim, which is
            // exactly the trap — an implementation that counts external limiters would
            // wrongly claim idle is attached.
            rl_listener_item(12, &edge_ext, &idle, "ext-rls"),
        ]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.rate_limits_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the rate-limits partial must be 200 on healthy upstreams"
    );
    let body = resp.text().await.expect("rate-limits partial body");
    let lower = body.to_lowercase();

    // Both domain rows render.
    assert!(
        body.contains(&checkout),
        "the partial must render the {checkout:?} domain row; body:\n{body}"
    );
    assert!(
        body.contains(&idle),
        "the partial must render the {idle:?} domain row; body:\n{body}"
    );

    // A positive attachment claim exists ("… attached by …", not "unattached").
    assert!(
        has_positive_attached_by(&lower),
        "the partial must say the checkout domain is attached by its listeners; body:\n{body}"
    );

    // Attribution: split the body at the two domain rows and require the listener names in
    // the checkout region only.
    let ci = body.find(&checkout).expect("checkout row present");
    let ii = body.find(&idle).expect("idle row present");
    let (checkout_region, idle_region) = if ci < ii {
        (&body[ci..ii], &body[ii..])
    } else {
        (&body[ci..], &body[ii..ci])
    };
    for listener in [&edge_a, &edge_b] {
        assert!(
            checkout_region.contains(listener.as_str()),
            "the checkout domain must be shown attached by {listener:?}; \
             checkout region:\n{checkout_region}"
        );
        assert!(
            !idle_region.contains(listener.as_str()),
            "listener {listener:?} attaches checkout, not idle; idle region:\n{idle_region}"
        );
    }

    // The idle domain shows an unattached indication — and checkout does not.
    assert!(
        idle_region.to_lowercase().contains("unattached"),
        "the idle domain (no built-in listener) must show an unattached indication; \
         idle region:\n{idle_region}"
    );
    assert!(
        !checkout_region.to_lowercase().contains("unattached"),
        "the checkout domain is attached and must not be claimed unattached; \
         checkout region:\n{checkout_region}"
    );

    // The EXTERNAL limiter is excluded from attachment claims ENTIRELY: its listener name
    // appears nowhere in the panel, so no "attached by <edge-ext>" claim can exist.
    assert!(
        !body.contains(&edge_ext),
        "the external limiter listener {edge_ext:?} (service_cluster != rate_limit_cluster) \
         must be excluded from attachment claims entirely; body:\n{body}"
    );

    // The partial swept both upstream collections, and fetched NO policies (lazy contract).
    let recorded = stub.recorded();
    let domains_path = format!("/api/v1/teams/{team}/rate-limit-domains");
    let listeners_path = format!("/api/v1/teams/{team}/listeners");
    assert!(
        recorded.iter().any(|r| r.path == domains_path),
        "the partial must sweep {domains_path:?}; recorded: {recorded:?}"
    );
    assert!(
        recorded.iter().any(|r| r.path == listeners_path),
        "the partial must sweep {listeners_path:?} for the attachment join; \
         recorded: {recorded:?}"
    );
    assert_no_policies_requests(&recorded);
}

// =============================================================================================
// Test 2 (AC 10): ADVERSARIAL PIPE BASE — a base domain that itself contains `|` must be
// matched by structurally stripping exactly the `<uuid>|<uuid>|` prefix, never by splitting
// on `|` (which would truncate at the first inner pipe and leave the domain "unattached").
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn base_domain_containing_pipes_is_matched_structurally_not_split() {
    let base = format!("multi|part|{}", unique("domain"));
    let edge = unique("edge-pipe");

    let composed = composed_domain(&base);
    let stub = start_stub(
        Collection::ok(vec![domain_item(0, &base)]),
        Collection::ok(vec![rl_listener_item(
            10,
            &edge,
            &composed,
            BUILT_IN_CLUSTER,
        )]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.rate_limits_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "rate-limits partial must be 200"
    );
    let body = resp.text().await.expect("rate-limits partial body");
    let lower = body.to_lowercase();

    // The FULL base domain name renders — no truncation at the first inner `|`.
    assert!(
        body.contains(&base),
        "the domain row must show the FULL base domain name {base:?} \
         (no truncation at an inner '|'); body:\n{body}"
    );
    // …and it is attached by the listener.
    assert!(
        has_positive_attached_by(&lower),
        "the pipe-containing base domain must be shown attached; body:\n{body}"
    );
    assert!(
        body.contains(&edge),
        "the attaching listener {edge:?} must be named; body:\n{body}"
    );
    // A split-on-'|' implementation would derive base "multi", fail to match the row, and
    // claim it unattached — the single domain here must carry NO unattached claim.
    assert!(
        !lower.contains("unattached"),
        "the pipe-containing base domain is attached; an \"unattached\" claim means the \
         composed prefix was split on '|' instead of structurally stripped; body:\n{body}"
    );
}

// =============================================================================================
// Test 3: LAZY POLICIES + PERCENT-ENCODING — the rate-limits partial makes NO policies
// request; the policies partial for a domain named with a space sweeps exactly one upstream
// policies request whose path carries the percent-encoded name, and renders the policy.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policies_are_lazy_and_domain_name_is_percent_encoded_upstream() {
    let base = format!("spaced domain-{}", unique("d"));
    let encoded = base.replace(' ', "%20");
    let policy_name = unique("pol");

    let stub = start_stub(
        Collection::ok(vec![domain_item(0, &base)]),
        Collection::ok(vec![]),
        Collection::ok(vec![policy_item(20, &policy_name, &det_id(0))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // Load the rate-limits partial: it must not fetch any policies.
    let resp = fetch(&http, &dash.rate_limits_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "rate-limits partial must be 200"
    );
    let body = resp.text().await.expect("rate-limits partial body");
    assert!(
        body.contains(&base),
        "the spaced domain row must render; body:\n{body}"
    );
    // Grace period so even an asynchronously-fired policies fetch would be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_no_policies_requests(&stub.recorded());

    // Opening the domain row → the policies partial, with the name percent-encoded.
    let resp = fetch(&http, &dash.policies_url(&encoded)).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the policies partial must be 200 on a healthy upstream"
    );
    let body = resp.text().await.expect("policies partial body");
    assert!(
        body.contains(&policy_name),
        "the policies partial must render the policy name {policy_name:?}; body:\n{body}"
    );
    assert!(
        body.contains("40") && body.to_lowercase().contains("minute"),
        "the policies partial must render the 40/minute limit; body:\n{body}"
    );

    // Exactly ONE upstream policies request, path percent-encoded (no raw space).
    let recorded = stub.recorded();
    let policies_reqs: Vec<&Recorded> = recorded
        .iter()
        .filter(|r| r.path.contains("/policies"))
        .collect();
    assert_eq!(
        policies_reqs.len(),
        1,
        "exactly one upstream policies request must have been made; \
         recorded: {recorded:?}"
    );
    let preq = policies_reqs[0];
    let want_fragment = format!("rate-limit-domains/{encoded}/policies");
    assert!(
        preq.path.contains(&want_fragment),
        "the upstream policies path must carry the percent-encoded domain name \
         ({want_fragment:?}); got: {:?}",
        preq.path
    );
    assert!(
        !preq.path.contains(' '),
        "the upstream policies path must not contain a raw space; got: {:?}",
        preq.path
    );
}

// =============================================================================================
// Test 4 (AC 10): GENUINELY UNATTACHED — one domain, zero listeners with rate-limit filters
// → the unattached indication renders.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_with_no_rate_limit_listeners_shows_unattached() {
    let lonely = unique("lonely");
    let stub = start_stub(
        Collection::ok(vec![domain_item(0, &lonely)]),
        // A listener exists but carries no rate-limit filter — it must not count.
        Collection::ok(vec![plain_listener_item(10, &unique("plain"))]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.rate_limits_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "rate-limits partial must be 200"
    );
    let body = resp.text().await.expect("rate-limits partial body");
    assert!(
        body.contains(&lonely),
        "the domain row must render; body:\n{body}"
    );
    assert!(
        body.to_lowercase().contains("unattached"),
        "a domain no built-in listener attaches must show an unattached indication; \
         body:\n{body}"
    );
}

// =============================================================================================
// Test 5: LISTENERS FORBIDDEN → ATTACHMENT UNKNOWN — domains 200, listeners 403 → the
// partial is 200, domains still render, attachment is declared UNKNOWN (never "unattached").
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_listeners_render_domains_with_attachment_unknown() {
    let dname = unique("dom");
    let stub = start_stub(
        Collection::ok(vec![domain_item(0, &dname)]),
        Collection::failing(403),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.rate_limits_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a listeners 403 must not fail the rate-limits partial itself"
    );
    let body = resp.text().await.expect("rate-limits partial body");
    let lower = body.to_lowercase();
    assert!(
        body.contains(&dname),
        "domains must still render when the listeners sweep is forbidden; body:\n{body}"
    );
    assert!(
        lower.contains("attachment") && lower.contains("unknown"),
        "with listeners unavailable the partial must declare attachment unknown; \
         body:\n{body}"
    );
    // No "unattached" CLAIM may appear for the domain. The claim would live in the domain's
    // row, so assert on the region from the domain name onward — a page-level notice that
    // merely *negates* unattached claims (e.g. `no "unattached" claims are made`) sits
    // before the rows and is consistent with the contract.
    let di = body.find(&dname).expect("domain row present");
    let row_region = &body[di..];
    assert!(
        !row_region.to_lowercase().contains("unattached"),
        "with listeners unavailable no domain row may claim \"unattached\"; \
         row region:\n{row_region}"
    );
    assert!(
        row_region.to_lowercase().contains("unknown"),
        "the domain row itself must carry the unknown-attachment indication; \
         row region:\n{row_region}"
    );
}

// =============================================================================================
// Test 6a: upstream 401 on rate-limit-domains → HTTP 286 + HX-Retarget: #resources naming
// `flowplane auth login`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_domains_returns_286_and_names_auth_login() {
    let stub = start_stub(
        Collection::failing(401),
        Collection::ok(vec![]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.rate_limits_url()).await;
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
// Test 6b: upstream 403 on rate-limit-domains → HTTP 200 saying not authorized.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_domains_partial_says_not_authorized() {
    let stub = start_stub(
        Collection::failing(403),
        Collection::ok(vec![]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.rate_limits_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an upstream 403 on rate-limit-domains must not fail the partial itself"
    );
    let body = resp.text().await.expect("rate-limits partial body");
    assert!(
        body.to_lowercase().contains("not authorized"),
        "the partial must say not authorized on upstream 403; body:\n{body}"
    );
}
