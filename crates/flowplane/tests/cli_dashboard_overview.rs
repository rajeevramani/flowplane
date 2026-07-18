//! fpv2-03m.3 — `flowplane dashboard` Overview live data (black-box, adversarial).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test, for the
//! polled partial `GET http://127.0.0.1:<port>/<nonce>/partials/overview`:
//!
//!   * The dashboard calls EXACTLY two upstream endpoints, with `Authorization: Bearer
//!     <token>` (+ `X-Flowplane-Org` when an org is set):
//!       - `/api/v1/teams/{team}/stats/overview`
//!       - `/api/v1/teams/{team}/xds/status`
//!   * Both upstreams 200 → HTTP 200 HTML: team totals come FROM STATS (not from counting
//!     xds rows); per-dataplane rows show the name, "ever verified"/"never verified" from
//!     `last_config_verify_at`, a humanized heartbeat age ("… ago") or "never"; the health
//!     string renders; the xds `version` value is NOT rendered as an applied version.
//!   * Truncation banner ("Showing first N of M", "limited to the listed dataplanes",
//!     "team-wide") exactly when stats.total_dataplanes exceeds the listed xds dataplanes;
//!     no banner when the counts agree.
//!   * Per-panel degradation: 403 on one endpoint → that panel says "Not authorized", the
//!     other still renders, HTTP 200. 500 on one endpoint → that panel is "unavailable",
//!     the other still renders, HTTP 200.
//!   * 401 on either endpoint → status 286 (htmx stop-polling) naming `flowplane auth login`.
//!   * The bearer token never appears in any partial response body or header.
//!   * E2E smoke against the REAL fp-api router over loopback (shared test PostgreSQL),
//!     covering both non-env token sources: `~/.flowplane/credentials` and the loopback
//!     `~/.flowplane/dev-token` fallback.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and unique
//! org/team names; nothing binds a fixed port, so the suite runs green under default
//! nextest parallelism. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
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

/// A distinctive bearer token so any leak into a partial response is unambiguous.
const SECRET_TOKEN: &str = "sekret-overview-token-do-not-leak-4e7a";

/// A distinctive xds `version` value that must NOT be rendered anywhere in the partial.
const DISTINCTIVE_VERSION: i64 = 999_888_777;

const STATS_SUFFIX: &str = "/stats/overview";
const XDS_SUFFIX: &str = "/xds/status";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 that records every request (path +
// auth/org headers) and returns canned per-endpoint responses. Unknown paths are recorded too
// (so the "no other request" assertion sees them) and answered 404.
// =============================================================================================

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    authorization: Option<String>,
    org: Option<String>,
}

struct StubState {
    /// (status, body) for `/api/v1/teams/{team}/stats/overview`.
    stats: (u16, Value),
    /// (status, body) for `/api/v1/teams/{team}/xds/status`.
    xds: (u16, Value),
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

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    state.requests.lock().unwrap().push(Recorded {
        path: path.clone(),
        authorization: header("authorization"),
        org: header("x-flowplane-org"),
    });

    let (code, body) = if path.ends_with(STATS_SUFFIX) {
        state.stats.clone()
    } else if path.ends_with(XDS_SUFFIX) {
        state.xds.clone()
    } else {
        (
            404,
            json!({ "code": "not_found", "message": "no such route" }),
        )
    };
    (
        StatusCode::from_u16(code).expect("valid canned status"),
        Json(body),
    )
        .into_response()
}

async fn start_stub(stats: (u16, Value), xds: (u16, Value)) -> StubUpstream {
    let state = Arc::new(StubState {
        stats,
        xds,
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
    fn partial_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/overview",
            self.port, self.nonce
        )
    }
}

struct DashOpts {
    home: PathBuf,
    server: String,
    /// `FLOWPLANE_TOKEN`; `None` exercises file-based token sources under `home`.
    token: Option<String>,
    team: String,
    org: Option<String>,
}

/// Spawn `flowplane dashboard` with an isolated HOME and the standard env, read the single
/// stdout announcement line (30s timeout), and parse out port + nonce.
fn spawn_dashboard(opts: &DashOpts) -> Dashboard {
    let mut cmd = common::flowplane_cmd(&opts.home);
    // flowplane_cmd points FLOWPLANE_CONFIG at <home>/config.toml; repoint it to the
    // real layout (<home>/.flowplane/config.toml) so the credentials file this test
    // writes to <home>/.flowplane/credentials is where the binary actually looks
    // (credentials_path derives from the config path's parent).
    cmd.env(
        "FLOWPLANE_CONFIG",
        opts.home.join(".flowplane").join("config.toml"),
    )
    .env("FLOWPLANE_SERVER", &opts.server)
    .env("FLOWPLANE_TEAM", &opts.team)
    .env("FLOWPLANE_DASHBOARD_NO_BROWSER", "1")
    .arg("dashboard")
    .stdout(Stdio::piped())
    // stderr → null: the server outlives this test's reads and an unread full pipe could
    // block the child (the dev-token fallback prints a notice we deliberately ignore).
    .stderr(Stdio::null());
    if let Some(token) = &opts.token {
        cmd.env("FLOWPLANE_TOKEN", token);
    }
    if let Some(org) = &opts.org {
        cmd.env("FLOWPLANE_ORG", org);
    }

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

/// Fetch the overview partial with a startup tolerance: retry on transport errors and 5xx
/// until a non-5xx response arrives or 15s elapse. Terminal statuses (200/286/4xx) return.
async fn fetch_partial(http: &reqwest::Client, url: &str) -> reqwest::Response {
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
// Canned upstream payloads.
// =============================================================================================

/// Stats overview with distinctive 3-digit values (3 digits so no thousands-separator /
/// humanization ambiguity can hide them in the rendered HTML).
fn stats_body(total_dataplanes: i64) -> Value {
    json!({
        "total_dataplanes": total_dataplanes,
        "live_dataplanes": 653,
        "stale_dataplanes": 47,
        "total_requests": 911,
        "total_errors": 317,
        "warming_failures": 213
    })
}

/// An xds status listing exactly three dataplanes:
///   * `names[0]`: verified + recent-ish heartbeat  → "ever verified", "… ago"
///   * `names[1]`: never verified, no heartbeat     → "never verified", "never"
///   * `names[2]`: verified, no heartbeat           → "ever verified", "never"
///
/// All three carry the distinctive `version` that must never be rendered. The top level
/// includes an extra numeric field the contract allows ("…other numeric fields").
fn xds_body(names: &[String; 3]) -> Value {
    json!({
        "health": "healthy",
        "recent_nack_count": 2,
        "connected_count": 3,
        "dataplanes": [
            {
                "name": names[0], "id": uuid::Uuid::now_v7().to_string(), "live": true,
                "version": DISTINCTIVE_VERSION,
                "last_heartbeat_at": "2026-01-01T00:00:00Z",
                "last_config_verify_at": "2026-01-02T03:04:05Z",
                "total_requests": 501, "total_errors": 7, "warming_failures": 1
            },
            {
                "name": names[1], "id": uuid::Uuid::now_v7().to_string(), "live": false,
                "version": DISTINCTIVE_VERSION,
                "last_heartbeat_at": null,
                "last_config_verify_at": null,
                "total_requests": 0, "total_errors": 0, "warming_failures": 0
            },
            {
                "name": names[2], "id": uuid::Uuid::now_v7().to_string(), "live": true,
                "version": DISTINCTIVE_VERSION,
                "last_heartbeat_at": null,
                "last_config_verify_at": "2026-01-03T00:00:00Z",
                "total_requests": 410, "total_errors": 3, "warming_failures": 2
            }
        ]
    })
}

fn three_names() -> [String; 3] {
    [unique("dp-alpha"), unique("dp-beta"), unique("dp-gamma")]
}

// =============================================================================================
// Test 1: happy path — totals FROM STATS, truncation banner, ever/never verified, humanized
// heartbeat, health, version non-display, and the upstream request-set contract.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_happy_path_totals_from_stats_with_truncation_banner() {
    let names = three_names();
    // Stats says 700 dataplanes; xds lists only 3 → totals must come from stats and the
    // truncation banner must appear.
    let stub = start_stub((200, stats_body(700)), (200, xds_body(&names))).await;
    let team = unique("team");
    let dash = spawn_dashboard(&DashOpts {
        home: common::unique_tempdir(),
        server: stub.base_url.clone(),
        token: Some(SECRET_TOKEN.into()),
        team: team.clone(),
        org: None,
    });

    let resp = fetch_partial(&client(), &dash.partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "both upstreams 200 → the partial must be 200"
    );
    let body = resp.text().await.expect("partial body");
    let lower = body.to_lowercase();

    // Team totals come FROM STATS: 700 total even though only 3 xds rows are listed, plus
    // the other distinctive stats numbers.
    for needle in ["700", "653", "317"] {
        assert!(
            body.contains(needle),
            "partial must render the stats number {needle:?} (totals must come from the \
             stats endpoint, not from counting xds rows); body:\n{body}"
        );
    }

    // Truncation banner: exact count phrasing + both scope-clarifying phrases.
    assert!(
        lower.contains("showing first 3 of 700"),
        "truncation banner must say \"Showing first 3 of 700\"; body:\n{body}"
    );
    assert!(
        lower.contains("limited to the listed dataplanes"),
        "truncation banner must contain \"limited to the listed dataplanes\"; body:\n{body}"
    );
    assert!(
        lower.contains("team-wide"),
        "truncation banner must contain \"team-wide\"; body:\n{body}"
    );

    // Per-dataplane rows: every listed name renders.
    for name in &names {
        assert!(
            body.contains(name.as_str()),
            "partial must render a row for dataplane {name:?}; body:\n{body}"
        );
    }

    // Verified state: "never verified" contains "ever verified" as a substring, so compare
    // occurrence counts — two rows are verified, one is not.
    let ever = lower.matches("ever verified").count();
    let never_verified = lower.matches("never verified").count();
    assert!(
        never_verified >= 1,
        "the row with null last_config_verify_at must say \"never verified\"; body:\n{body}"
    );
    assert!(
        ever > never_verified,
        "rows with non-null last_config_verify_at must say \"ever verified\" (found {ever} \
         \"ever verified\" incl. {never_verified} inside \"never verified\"); body:\n{body}"
    );

    // Heartbeat: humanized age for the non-null row; "never" for both null rows. The
    // "never verified" text itself accounts for one "never", so with two null-heartbeat
    // rows we must see at least 3 occurrences in total.
    assert!(
        lower.contains("ago"),
        "non-null last_heartbeat_at must render a humanized age containing \"ago\"; body:\n{body}"
    );
    let nevers = lower.matches("never").count();
    assert!(
        nevers >= 3,
        "two null-heartbeat rows must each render \"never\" (plus one \"never verified\"): \
         expected >= 3 occurrences of \"never\", found {nevers}; body:\n{body}"
    );

    // Health string renders.
    assert!(
        lower.contains("healthy"),
        "partial must render the xds health string; body:\n{body}"
    );

    // The xds `version` value must NOT be displayed as an applied version.
    assert!(
        !body.contains(&DISTINCTIVE_VERSION.to_string()),
        "the xds \"version\" value ({DISTINCTIVE_VERSION}) must not be rendered; body:\n{body}"
    );

    // Upstream request-set contract: EXACTLY the two documented paths, both hit.
    let stats_path = format!("/api/v1/teams/{team}{STATS_SUFFIX}");
    let xds_path = format!("/api/v1/teams/{team}{XDS_SUFFIX}");
    let recorded = stub.recorded();
    assert!(
        !recorded.is_empty(),
        "the dashboard must have called the upstream"
    );
    for req in &recorded {
        assert!(
            req.path == stats_path || req.path == xds_path,
            "the dashboard sent an upstream request outside the documented set: {:?} \
             (allowed: {stats_path:?}, {xds_path:?})",
            req.path
        );
    }
    assert!(
        recorded.iter().any(|r| r.path == stats_path),
        "the stats endpoint must have been called; recorded: {recorded:?}"
    );
    assert!(
        recorded.iter().any(|r| r.path == xds_path),
        "the xds status endpoint must have been called; recorded: {recorded:?}"
    );
}

// =============================================================================================
// Test 2: counts agree → NO truncation banner.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_truncation_banner_when_counts_agree() {
    let names = three_names();
    // Stats total (3) equals the number of listed xds dataplanes (3).
    let stub = start_stub((200, stats_body(3)), (200, xds_body(&names))).await;
    let dash = spawn_dashboard(&DashOpts {
        home: common::unique_tempdir(),
        server: stub.base_url.clone(),
        token: Some(SECRET_TOKEN.into()),
        team: unique("team"),
        org: None,
    });

    let resp = fetch_partial(&client(), &dash.partial_url()).await;
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("partial body");
    let lower = body.to_lowercase();

    assert!(
        !lower.contains("showing first"),
        "no truncation banner when stats total equals the listed dataplanes; body:\n{body}"
    );
    assert!(
        !lower.contains("limited to the listed dataplanes"),
        "no truncation banner when counts agree; body:\n{body}"
    );
    // Sanity: the page still rendered real data.
    assert!(body.contains(names[0].as_str()), "rows must render");
}

// =============================================================================================
// Test 3: 403 on ONE endpoint → that panel says "Not authorized", the other panel still
// renders its data, HTTP 200 (per-panel degradation, both directions).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_on_one_endpoint_degrades_only_that_panel() {
    let http = client();
    let forbidden = || {
        (
            403,
            json!({ "code": "forbidden", "message": "access denied" }),
        )
    };

    // Direction A: stats 403, xds 200 → "Not authorized" appears, xds rows still render.
    {
        let names = three_names();
        let stub = start_stub(forbidden(), (200, xds_body(&names))).await;
        let dash = spawn_dashboard(&DashOpts {
            home: common::unique_tempdir(),
            server: stub.base_url.clone(),
            token: Some(SECRET_TOKEN.into()),
            team: unique("team"),
            org: None,
        });
        let resp = fetch_partial(&http, &dash.partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "a per-panel 403 must not fail the whole partial"
        );
        let body = resp.text().await.expect("body");
        let lower = body.to_lowercase();
        assert!(
            lower.contains("not authorized"),
            "stats panel must show \"Not authorized\" on upstream 403; body:\n{body}"
        );
        for name in &names {
            assert!(
                body.contains(name.as_str()),
                "the xds panel must still render dataplane {name:?} despite the stats 403; \
                 body:\n{body}"
            );
        }
        assert!(
            lower.contains("healthy"),
            "the xds panel must still render its health string; body:\n{body}"
        );
    }

    // Direction B: xds 403, stats 200 → "Not authorized" appears, stats numbers still render.
    {
        let stub = start_stub((200, stats_body(700)), forbidden()).await;
        let dash = spawn_dashboard(&DashOpts {
            home: common::unique_tempdir(),
            server: stub.base_url.clone(),
            token: Some(SECRET_TOKEN.into()),
            team: unique("team"),
            org: None,
        });
        let resp = fetch_partial(&http, &dash.partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "a per-panel 403 must not fail the whole partial"
        );
        let body = resp.text().await.expect("body");
        let lower = body.to_lowercase();
        assert!(
            lower.contains("not authorized"),
            "xds panel must show \"Not authorized\" on upstream 403; body:\n{body}"
        );
        for needle in ["700", "653", "317"] {
            assert!(
                body.contains(needle),
                "the stats panel must still render {needle:?} despite the xds 403; body:\n{body}"
            );
        }
    }
}

// =============================================================================================
// Test 4: 401 on either endpoint → status 286 (htmx stop-polling) naming `flowplane auth
// login` — the session is dead, so the page must stop polling, not degrade a single panel.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_upstream_returns_286_and_names_auth_login() {
    let http = client();
    let unauthorized = || {
        (
            401,
            json!({ "code": "unauthorized", "message": "missing or invalid token" }),
        )
    };

    // 401 on the stats endpoint.
    {
        let names = three_names();
        let stub = start_stub(unauthorized(), (200, xds_body(&names))).await;
        let dash = spawn_dashboard(&DashOpts {
            home: common::unique_tempdir(),
            server: stub.base_url.clone(),
            token: Some(SECRET_TOKEN.into()),
            team: unique("team"),
            org: None,
        });
        let resp = fetch_partial(&http, &dash.partial_url()).await;
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
    }

    // 401 on the xds endpoint (either endpoint must trigger the same stop-polling response).
    {
        let stub = start_stub((200, stats_body(3)), unauthorized()).await;
        let dash = spawn_dashboard(&DashOpts {
            home: common::unique_tempdir(),
            server: stub.base_url.clone(),
            token: Some(SECRET_TOKEN.into()),
            team: unique("team"),
            org: None,
        });
        let resp = fetch_partial(&http, &dash.partial_url()).await;
        assert_eq!(
            resp.status().as_u16(),
            286,
            "upstream 401 on the xds endpoint must also yield 286"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.contains("flowplane auth login"),
            "the 286 body must tell the user to run \"flowplane auth login\"; body:\n{body}"
        );
    }
}

// =============================================================================================
// Test 5: 500 on one endpoint → that panel renders an "unavailable" state, the other panel
// renders fine, and the partial itself is HTTP 200 (no crash, keep polling).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_error_on_one_endpoint_renders_unavailable_panel() {
    let names = three_names();
    let stub = start_stub(
        (500, json!({ "code": "internal", "message": "boom" })),
        (200, xds_body(&names)),
    )
    .await;
    let dash = spawn_dashboard(&DashOpts {
        home: common::unique_tempdir(),
        server: stub.base_url.clone(),
        token: Some(SECRET_TOKEN.into()),
        team: unique("team"),
        org: None,
    });

    let resp = fetch_partial(&client(), &dash.partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "an upstream 500 on one endpoint must not fail the whole partial"
    );
    let body = resp.text().await.expect("body");
    let lower = body.to_lowercase();
    assert!(
        lower.contains("unavailable"),
        "the stats panel must render an \"unavailable\" state on upstream 500; body:\n{body}"
    );
    for name in &names {
        assert!(
            body.contains(name.as_str()),
            "the xds panel must still render dataplane {name:?} despite the stats 500; \
             body:\n{body}"
        );
    }
    assert!(
        lower.contains("healthy"),
        "the xds panel must still render its health string; body:\n{body}"
    );
}

// =============================================================================================
// Test 6: the Authorization bearer and X-Flowplane-Org headers reach the upstream on every
// call; the token never appears in the partial response body or headers.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bearer_and_org_headers_reach_upstream_and_token_never_leaks() {
    let names = three_names();
    let stub = start_stub((200, stats_body(3)), (200, xds_body(&names))).await;
    let org = unique("org");
    let dash = spawn_dashboard(&DashOpts {
        home: common::unique_tempdir(),
        server: stub.base_url.clone(),
        token: Some(SECRET_TOKEN.into()),
        team: unique("team"),
        org: Some(org.clone()),
    });

    let resp = fetch_partial(&client(), &dash.partial_url()).await;
    assert_eq!(resp.status().as_u16(), 200);

    // Token non-disclosure: neither in response headers…
    for (name, value) in resp.headers() {
        let value_str = String::from_utf8_lossy(value.as_bytes()).to_string();
        assert!(
            !name.as_str().contains(SECRET_TOKEN) && !value_str.contains(SECRET_TOKEN),
            "partial response header {name:?} leaks the bearer token: {value_str:?}"
        );
    }
    // …nor in the body.
    let body = resp.text().await.expect("body");
    assert!(
        !body.contains(SECRET_TOKEN),
        "the partial response body leaks the bearer token"
    );

    // Every recorded upstream request carried the bearer token and the org header.
    let recorded = stub.recorded();
    assert!(!recorded.is_empty(), "the upstream must have been called");
    let want_auth = format!("Bearer {SECRET_TOKEN}");
    for req in &recorded {
        assert_eq!(
            req.authorization.as_deref(),
            Some(want_auth.as_str()),
            "upstream request to {} must carry `Authorization: Bearer <token>`; got {:?}",
            req.path,
            req.authorization
        );
        assert_eq!(
            req.org.as_deref(),
            Some(org.as_str()),
            "upstream request to {} must carry `X-Flowplane-Org: {org}`; got {:?}",
            req.path,
            req.org
        );
    }
}

// =============================================================================================
// Test 7: E2E smoke against the REAL fp-api router (loopback, shared test PostgreSQL), run
// TWICE to cover both non-env token sources: (a) ~/.flowplane/credentials, (b) the loopback
// ~/.flowplane/dev-token fallback. The OIDC login flow itself is out of scope here.
// =============================================================================================

fn write_token_file(home: &Path, file_name: &str, token: &str) {
    let dir = home.join(".flowplane");
    std::fs::create_dir_all(&dir).expect("create ~/.flowplane");
    // Trailing newline is allowed by the token-file contract; write one deliberately.
    std::fs::write(dir.join(file_name), format!("{token}\n")).expect("write token file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_overview_against_real_control_plane_both_token_sources() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Tenancy fixture: one org, one team, one agent whose fpat_ bearer token holds exactly
    // (stats, read) — the resource that guards BOTH overview endpoints.
    let (team_name, token) = {
        use fp_domain::AgentKind;
        use fp_storage::repos::identity;

        let org = identity::create_org(&pool, &unique("org"), "")
            .await
            .expect("org");
        let team = identity::create_team(&pool, org.id, &unique("team"), "")
            .await
            .expect("team");
        let token = format!(
            "fpat_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut tx = pool.begin().await.expect("begin");
        let agent = identity::create_agent_tx(
            &mut tx,
            org.id,
            &unique("agent"),
            AgentKind::CpTool,
            &identity::hash_agent_token(&token),
            None,
        )
        .await
        .expect("agent");
        identity::add_agent_grant_in_tx(
            &mut tx,
            agent.id,
            org.id,
            team.id,
            fp_domain::authz::Resource::Stats,
            fp_domain::authz::Action::Read,
            None,
        )
        .await
        .expect("agent grant");
        tx.commit().await.expect("commit");
        (team.name, token)
    };

    // The REAL control plane: the production router served over loopback.
    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle(),
        version: "test",
        validator: None,
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind CP to an ephemeral port");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let http = client();

    // Run (a): prod/credentials path — token in ~/.flowplane/credentials, no FLOWPLANE_TOKEN.
    {
        let home = common::unique_tempdir();
        write_token_file(&home, "credentials", &token);
        let dash = spawn_dashboard(&DashOpts {
            home,
            server: base_url.clone(),
            token: None,
            team: team_name.clone(),
            org: None,
        });
        assert_overview_smoke(&http, &dash, "credentials-file token source").await;
    }

    // Run (b): dev-token path — token in ~/.flowplane/dev-token, no FLOWPLANE_TOKEN, no
    // credentials file. The server URL is loopback so the dev-token fallback applies (a
    // stderr notice is expected and ignored).
    {
        let home = common::unique_tempdir();
        write_token_file(&home, "dev-token", &token);
        let dash = spawn_dashboard(&DashOpts {
            home,
            server: base_url.clone(),
            token: None,
            team: team_name.clone(),
            org: None,
        });
        assert_overview_smoke(&http, &dash, "dev-token fallback token source").await;
    }

    server.abort();
}

/// Shared E2E assertion: the partial is 200, renders zero-ish team totals for the freshly
/// seeded (dataplane-less) team, and shows no auth degradation. Kept implementation-neutral:
/// only the number "0" and the absence of the two auth-failure markers are asserted.
async fn assert_overview_smoke(http: &reqwest::Client, dash: &Dashboard, ctx: &str) {
    let resp = fetch_partial(http, &dash.partial_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "{ctx}: the overview partial against the real CP must be 200"
    );
    let body = resp.text().await.expect("partial body");
    let lower = body.to_lowercase();
    assert!(
        body.contains('0'),
        "{ctx}: the totals panel must render (the seeded team has no dataplanes, so \
         zero-ish totals are expected); body:\n{body}"
    );
    assert!(
        !lower.contains("not authorized"),
        "{ctx}: the token holds (stats, read) — no panel may be \"Not authorized\"; \
         body:\n{body}"
    );
    assert!(
        !lower.contains("auth login"),
        "{ctx}: a valid token must not produce the re-login message; body:\n{body}"
    );
}
