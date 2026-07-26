//! fpv2-80z.2 — `flowplane dashboard` overview PANEL partial restyle — black-box,
//! spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The dashboard is a
//! read-only loopback htmx server; every route lives under a per-launch nonce prefix. The
//! contract under test is the served partial `GET /<nonce>/partials/overview` (plus the
//! served stylesheet `/<nonce>/assets/dashboard.css`), backed by a stub upstream:
//!
//!   1. Both upstreams 200 → team totals render as a `.cards` grid: 4 `.card` elements,
//!      each with `.label`, `.value`, `.sub`; the Dataplanes card's value equals
//!      stats.total_dataplanes and its sub contains the live and stale counts. The legacy
//!      `<dl class="stats">` is GONE.
//!   2. The gateway panel is a `.panel` whose `h2` carries a health pill:
//!      "healthy"→`pill good`, "stale"→`pill warn`, "degraded"→`pill crit`; an unknown
//!      health string → `pill neutral` with the verbatim text shown. The legacy
//!      `health-{{...}}` class pattern is GONE.
//!   3. The dataplanes table is wrapped in `.tablewrap`; the served stylesheet has a `th`
//!      rule with `position: sticky`. Rows: live true→`pill good` "live", false→`pill warn`
//!      "stale"; config_verified_ever true→`pill good`, false→`pill neutral`. Legacy bare
//!      `class="live"`/`class="stale"` spans are GONE.
//!   4. The truncation banner (stats.total_dataplanes > listed rows) still shows the same
//!      text ("Showing first", "team-wide"); the stylesheet's `.banner` rule has
//!      `border-left: 3px`.
//!   5. 403 on either endpoint → "Not authorized" panel; 500/malformed → "unavailable"
//!      panel (content assertions).
//!   6. No inline `<style>` element, no inline `<script>` element, and no inline `style=`
//!      attribute anywhere in the served partial.
//!
//! Parallel-safety (invariant 18): every test spawns its own stub upstream and dashboard
//! child on ephemeral ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique
//! team name; nothing binds a fixed port. Every spawned server is killed via a Drop guard
//! in all paths, including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::json;
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-overview-token-do-not-leak-41c9";

const STATS_SUFFIX: &str = "/stats/overview";
const XDS_SUFFIX: &str = "/xds/status";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving canned (and mutable)
// responses for `/api/v1/teams/{team}/stats/overview` and `.../xds/status`. Mutable state lets
// one test walk several upstream scenarios (health values, error statuses) through a single
// spawned dashboard.
// =============================================================================================

struct StubState {
    /// (status, body) for `/api/v1/teams/{team}/stats/overview`.
    stats: Mutex<(u16, String)>,
    /// (status, body) for `/api/v1/teams/{team}/xds/status`.
    xds: Mutex<(u16, String)>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    fn set_stats(&self, status: u16, body: String) {
        *self.state.stats.lock().unwrap() = (status, body);
    }

    fn set_xds(&self, status: u16, body: String) {
        *self.state.xds.lock().unwrap() = (status, body);
    }
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let (code, body) = if path.ends_with(STATS_SUFFIX) {
        state.stats.lock().unwrap().clone()
    } else if path.ends_with(XDS_SUFFIX) {
        state.xds.lock().unwrap().clone()
    } else {
        (
            404,
            json!({ "code": "not_found", "message": "no such route" }).to_string(),
        )
    };
    (
        StatusCode::from_u16(code).expect("valid canned status"),
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

async fn start_stub(stats: (u16, String), xds: (u16, String)) -> StubUpstream {
    let state = Arc::new(StubState {
        stats: Mutex::new(stats),
        xds: Mutex::new(xds),
    });
    let app = Router::new().fallback(stub_handler).with_state(state.clone());
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

/// Stats overview with distinctive 3-digit values (3 digits so no thousands-separator /
/// humanization ambiguity can hide them in the rendered HTML).
fn stats_body(total_dataplanes: i64) -> String {
    json!({
        "total_dataplanes": total_dataplanes,
        "live_dataplanes": 653,
        "stale_dataplanes": 471,
        "total_requests": 911,
        "total_errors": 317,
        "warming_failures": 213
    })
    .to_string()
}

/// An xds status with the given gateway `health` and exactly two dataplanes:
///   * `names[0]`: live, config verified at some point  → live pill good, verified pill good
///   * `names[1]`: stale, never verified, no heartbeat  → live pill warn, verified pill neutral
fn xds_body(health: &str, names: &[String; 2]) -> String {
    json!({
        "health": health,
        "recent_nack_count": 2,
        "connected_count": 2,
        "dataplanes": [
            {
                "name": names[0], "id": uuid::Uuid::now_v7().to_string(), "live": true,
                "version": "v-distinctive-never-rendered",
                "last_heartbeat_at": "2026-01-01T00:00:00Z",
                "last_config_verify_at": "2026-01-02T03:04:05Z",
                "total_requests": 501, "total_errors": 7, "warming_failures": 1
            },
            {
                "name": names[1], "id": uuid::Uuid::now_v7().to_string(), "live": false,
                "version": "v-distinctive-never-rendered",
                "last_heartbeat_at": null,
                "last_config_verify_at": null,
                "total_requests": 0, "total_errors": 0, "warming_failures": 0
            }
        ]
    })
    .to_string()
}

fn two_names() -> [String; 2] {
    [unique("dp-alpha"), unique("dp-beta")]
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
    /// `http://127.0.0.1:<port>/<nonce>/<path>`.
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

/// GET the overview partial and assert the 200 baseline shared by every partial test.
async fn fetch_partial(http: &reqwest::Client, dash: &Dashboard) -> String {
    let url = dash.nonce_url("partials/overview");
    let resp = fetch(http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/partials/overview must serve the overview partial"
    );
    let partial = resp.text().await.expect("partial body");
    assert!(
        partial.contains('<'),
        "the overview partial must be HTML; body:\n{partial}"
    );
    assert!(
        !partial.contains(SECRET_TOKEN),
        "the partial must never leak the bearer token; body:\n{partial}"
    );
    partial
}

/// GET the served stylesheet.
async fn fetch_stylesheet(http: &reqwest::Client, dash: &Dashboard) -> String {
    let url = dash.nonce_url("assets/dashboard.css");
    let resp = fetch(http, &url).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET /<nonce>/assets/dashboard.css must serve the stylesheet"
    );
    let css = resp.text().await.expect("stylesheet body");
    assert!(
        !css.contains(SECRET_TOKEN),
        "the stylesheet must never leak the bearer token"
    );
    css
}

// =============================================================================================
// Markup assertion helpers (spec-level, tolerant of whitespace / extra classes).
// =============================================================================================

/// The opening tag (e.g. `<nav class="tabs">`) starting at the first occurrence of
/// `tag_start` (e.g. `<nav`), up to and including its closing `>`.
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

/// Does `tag` carry `class` as a whole class token (tolerant of multi-class attributes)?
fn tag_has_class(tag: &str, class: &str) -> bool {
    for needle in [
        format!("class=\"{class}\""),
        format!("class=\"{class} "),
        format!(" {class}\""),
        format!(" {class} "),
    ] {
        if tag.contains(&needle) {
            return true;
        }
    }
    false
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
        sets.push(after[..end].split_whitespace().map(str::to_string).collect());
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
/// attribute, the text between the end of its opening tag and the next `<`.
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
                // Capture the element's full inner region with depth counting, so
                // nested same-tag elements don't truncate it: `<span class="sub">
                // <span class="num">653</span> live …</span>` must yield the whole
                // sub, not just the inner num span.
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
                    region_end = after[text_start..].find('<').unwrap_or(after.len() - text_start);
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

/// The positions (byte offsets) of every element whose class tokens contain `class`.
fn class_positions(html: &str, class: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut rest = html;
    let mut offset = 0usize;
    while let Some(i) = rest.find("class=\"") {
        let after = &rest[i + "class=\"".len()..];
        let Some(end) = after.find('"') else { break };
        if after[..end].split_whitespace().any(|t| t == class) {
            positions.push(offset + i);
        }
        offset += i + "class=\"".len() + end;
        rest = &after[end..];
    }
    positions
}

/// The rule bodies of every CSS rule whose selector list includes `token` as a whole
/// selector part (e.g. `.banner`, `th`, `th:first-child`).
fn css_rule_bodies<'a>(css: &'a str, token: &str) -> Vec<&'a str> {
    let mut bodies = Vec::new();
    for block in css.split('}') {
        let Some((selector, body)) = block.split_once('{') else {
            continue;
        };
        let matches = selector
            .split([',', ' ', '\n', '\t', '>'])
            .any(|part| {
                let part = part.trim();
                part == token
                    || (part.starts_with(token)
                        && part[token.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| !c.is_alphanumeric() && c != '-' && c != '_'))
            });
        if matches {
            bodies.push(body);
        }
    }
    bodies
}

// =============================================================================================
// Criterion 1: both upstreams 200 → team totals render as a `.cards` grid of 4 `.card`
// elements (each with .label/.value/.sub); the Dataplanes card's value equals
// stats.total_dataplanes and its sub contains the live and stale counts. Legacy
// `<dl class="stats">` is GONE.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_totals_render_as_four_card_grid_with_dataplanes_value_and_live_stale_sub() {
    let names = two_names();
    // total == listed rows so no truncation banner interferes with this test.
    let stub = start_stub((200, stats_body(2)), (200, xds_body("healthy", &names))).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_partial(&http, &dash).await;

    // The totals render inside a `.cards` grid element.
    assert!(
        has_element_with_classes(&partial, &["cards"]),
        "the partial must render the team totals inside a `.cards` grid; body:\n{partial}"
    );

    // Exactly 4 `.card` elements, each carrying .label, .value, and .sub.
    let card_positions = class_positions(&partial, "card");
    assert_eq!(
        card_positions.len(),
        4,
        "the .cards grid must contain exactly 4 `.card` elements; body:\n{partial}"
    );
    let card_segment = |i: usize| -> &str {
        let start = card_positions[i];
        let end = card_positions.get(i + 1).copied().unwrap_or(partial.len());
        &partial[start..end]
    };
    for i in 0..4 {
        let seg = card_segment(i);
        for class in ["label", "value", "sub"] {
            assert!(
                has_element_with_classes(seg, &[class]),
                "card #{i} must carry a `.{class}` element; card segment:\n{seg}"
            );
        }
    }

    // The Dataplanes card: label mentions "Dataplanes", its value equals
    // stats.total_dataplanes, and its sub contains the live and stale counts.
    let dp_seg = (0..4)
        .map(card_segment)
        .find(|seg| seg.contains("Dataplanes"))
        .unwrap_or_else(|| {
            panic!("one of the 4 cards must be the Dataplanes card; body:\n{partial}")
        });
    let values = texts_of_elements_with_classes(dp_seg, &["value"]);
    assert!(
        values.iter().any(|v| v == "2"),
        "the Dataplanes card's .value must equal stats.total_dataplanes (2); \
         found .value texts: {values:?}; card segment:\n{dp_seg}"
    );
    let subs = texts_of_elements_with_classes(dp_seg, &["sub"]);
    assert!(
        subs.iter().any(|s| s.contains("653") && s.contains("471")),
        "the Dataplanes card's .sub must contain the live (653) and stale (471) counts; \
         found .sub texts: {subs:?}; card segment:\n{dp_seg}"
    );

    // The legacy <dl class="stats"> is GONE.
    for dl in all_opening_tags(&partial, "<dl") {
        assert!(
            !tag_has_class(dl, "stats"),
            "the legacy `<dl class=\"stats\">` totals markup must be gone; offending tag: {dl:?}"
        );
    }
}

// =============================================================================================
// Criterion 2: the gateway panel is a `.panel` whose `h2` carries a health pill:
// healthy→`pill good`, stale→`pill warn`, degraded→`pill crit`, unknown→`pill neutral` with
// the verbatim text shown. The legacy `health-{{...}}` class pattern is GONE.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_health_maps_to_pill_variants_and_drops_legacy_health_class() {
    let names = two_names();
    let stub = start_stub(
        (200, stats_body(2)),
        (200, xds_body("healthy", &names)),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    for (health, pill_variant) in [
        ("healthy", "good"),
        ("stale", "warn"),
        ("degraded", "crit"),
        ("flurboicated", "neutral"), // unknown health string → neutral, verbatim text
    ] {
        stub.set_xds(200, xds_body(health, &names));
        let partial = fetch_partial(&http, &dash).await;

        // The gateway panel is a `.panel`.
        assert!(
            has_element_with_classes(&partial, &["panel"]),
            "the gateway section must render as a `.panel`; body:\n{partial}"
        );

        // The Gateway heading is an h2 carrying the health pill.
        let gateway_at = partial.find("Gateway").unwrap_or_else(|| {
            panic!("the partial must render a Gateway panel; body:\n{partial}")
        });
        let h2_open = partial[..gateway_at].rfind("<h2").unwrap_or_else(|| {
            panic!("the Gateway heading must be an <h2>; body:\n{partial}")
        });
        let h2 = region(&partial[h2_open..], "<h2", "</h2>").unwrap_or_else(|| {
            panic!("the Gateway <h2> must be closed; body:\n{partial}")
        });

        assert!(
            has_element_with_classes(h2, &["pill", pill_variant]),
            "health {health:?} must render as `pill {pill_variant}` inside the Gateway h2; \
             h2:\n{h2}"
        );
        let pill_texts = texts_of_elements_with_classes(h2, &["pill", pill_variant]);
        assert!(
            pill_texts.iter().any(|t| t == health),
            "the health pill must show the verbatim health text {health:?}; \
             found pill texts: {pill_texts:?}; h2:\n{h2}"
        );

        // The legacy `health-{{...}}` class pattern is GONE.
        assert!(
            !partial.contains(&format!("health-{health}")),
            "the legacy `health-{{...}}` class pattern must be gone; body:\n{partial}"
        );
    }
}

// =============================================================================================
// Criterion 3: the dataplanes table is wrapped in `.tablewrap`; the served stylesheet has a
// `th` rule with `position: sticky`. Rows: live→`pill good` "live", stale→`pill warn`
// "stale"; verified-ever→`pill good`, never→`pill neutral`. Legacy bare
// class="live"/class="stale" spans GONE.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dataplanes_table_uses_tablewrap_pill_cells_and_sticky_header_css() {
    let names = two_names();
    let stub = start_stub(
        (200, stats_body(2)),
        (200, xds_body("healthy", &names)),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_partial(&http, &dash).await;

    // The table is wrapped in `.tablewrap`.
    let tablewrap_at = class_positions(&partial, "tablewrap")
        .first()
        .copied()
        .unwrap_or_else(|| {
            panic!("the dataplanes table must be wrapped in `.tablewrap`; body:\n{partial}")
        });
    let after_wrap = &partial[tablewrap_at..];
    assert!(
        after_wrap.contains("<table"),
        "a <table> must follow the `.tablewrap` wrapper; body:\n{partial}"
    );

    // Row-scoped pill assertions.
    let row_of = |name: &str| -> String {
        let at = partial
            .find(name)
            .unwrap_or_else(|| panic!("row for {name:?} must exist; body:\n{partial}"));
        let tr_open = partial[..at]
            .rfind("<tr")
            .unwrap_or_else(|| panic!("row for {name:?} must be a <tr>; body:\n{partial}"));
        region(&partial[tr_open..], "<tr", "</tr>")
            .unwrap_or_else(|| panic!("row for {name:?} must be closed; body:\n{partial}"))
            .to_string()
    };

    // names[0]: live + verified-ever → live pill good with text "live", verified pill good.
    let live_row = row_of(&names[0]);
    let live_pills = texts_of_elements_with_classes(&live_row, &["pill", "good"]);
    assert!(
        live_pills.iter().any(|t| t == "live"),
        "a live dataplane row must render a `pill good` with text \"live\"; \
         found pill-good texts: {live_pills:?}; row:\n{live_row}"
    );

    // names[1]: stale + never verified → live pill warn with text "stale", verified pill
    // neutral.
    let stale_row = row_of(&names[1]);
    let warn_pills = texts_of_elements_with_classes(&stale_row, &["pill", "warn"]);
    assert!(
        warn_pills.iter().any(|t| t == "stale"),
        "a stale dataplane row must render a `pill warn` with text \"stale\"; \
         found pill-warn texts: {warn_pills:?}; row:\n{stale_row}"
    );
    assert!(
        has_element_with_classes(&stale_row, &["pill", "neutral"]),
        "a never-verified dataplane row must render a `pill neutral` for config verification; \
         row:\n{stale_row}"
    );

    // Legacy bare class="live"/class="stale" spans are GONE.
    for span in all_opening_tags(&partial, "<span") {
        assert!(
            !tag_has_class(span, "live") && !tag_has_class(span, "stale"),
            "legacy bare class=\"live\"/class=\"stale\" spans must be gone; \
             offending tag: {span:?}"
        );
    }

    // The served stylesheet has a `th` rule with `position: sticky`.
    let css = fetch_stylesheet(&http, &dash).await;
    let th_bodies = css_rule_bodies(&css, "th");
    assert!(
        !th_bodies.is_empty(),
        "the stylesheet must contain a `th` rule; css:\n{css}"
    );
    assert!(
        th_bodies
            .iter()
            .any(|b| b.replace([' ', '\n', '\t'], "").contains("position:sticky")),
        "a `th` rule must set `position: sticky`; th rule bodies: {th_bodies:?}"
    );
}

// =============================================================================================
// Criterion 4: the truncation banner (stats.total_dataplanes > listed rows) still shows the
// same text ("Showing first", "team-wide"); the stylesheet's `.banner` rule has
// `border-left: 3px`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncation_banner_text_and_banner_css_border_left() {
    let names = two_names();
    // 902 team-wide dataplanes but only 2 listed rows → truncation banner.
    let stub = start_stub(
        (200, stats_body(902)),
        (200, xds_body("healthy", &names)),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_partial(&http, &dash).await;

    assert!(
        has_element_with_classes(&partial, &["banner"]),
        "the truncation notice must render as a `.banner` element; body:\n{partial}"
    );
    let banner_texts = texts_of_elements_with_classes(&partial, &["banner"]);
    assert!(
        banner_texts.iter().any(|t| t.contains("Showing first")),
        "the truncation banner must still say \"Showing first\"; \
         banner texts: {banner_texts:?}"
    );
    assert!(
        banner_texts.iter().any(|t| t.contains("team-wide")),
        "the truncation banner must still say \"team-wide\"; banner texts: {banner_texts:?}"
    );

    // The stylesheet's `.banner` rule has `border-left: 3px`.
    let css = fetch_stylesheet(&http, &dash).await;
    let banner_bodies = css_rule_bodies(&css, ".banner");
    assert!(
        !banner_bodies.is_empty(),
        "the stylesheet must contain a `.banner` rule; css:\n{css}"
    );
    assert!(
        banner_bodies
            .iter()
            .any(|b| b.replace([' ', '\n', '\t'], "").contains("border-left:3px")),
        "the `.banner` rule must set `border-left: 3px`; .banner rule bodies: {banner_bodies:?}"
    );
}

// =============================================================================================
// Criterion 5a: 403 on either endpoint → "Not authorized" panel (content assertion).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upstream_403_renders_not_authorized_panel() {
    let names = two_names();
    let stub = start_stub(
        (403, json!({ "code": "forbidden", "message": "nope" }).to_string()),
        (200, xds_body("healthy", &names)),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // 403 on the stats endpoint.
    let partial = fetch_partial(&http, &dash).await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the stats endpoint must render a \"Not authorized\" panel; body:\n{partial}"
    );

    // 403 on the xds endpoint (stats healthy again).
    stub.set_stats(200, stats_body(2));
    stub.set_xds(403, json!({ "code": "forbidden", "message": "nope" }).to_string());
    let partial = fetch_partial(&http, &dash).await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the xds endpoint must render a \"Not authorized\" panel; body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 5b: 500 or malformed upstream responses → "unavailable" panel (content
// assertion).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upstream_500_or_malformed_renders_unavailable_panel() {
    let names = two_names();
    let stub = start_stub(
        (
            500,
            json!({ "code": "internal", "message": "boom" }).to_string(),
        ),
        (200, xds_body("healthy", &names)),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // 500 on the stats endpoint.
    let partial = fetch_partial(&http, &dash).await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a 500 from the stats endpoint must render an \"unavailable\" panel; body:\n{partial}"
    );

    // 500 on the xds endpoint (stats healthy again).
    stub.set_stats(200, stats_body(2));
    stub.set_xds(500, json!({ "code": "internal", "message": "boom" }).to_string());
    let partial = fetch_partial(&http, &dash).await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a 500 from the xds endpoint must render an \"unavailable\" panel; body:\n{partial}"
    );

    // Malformed upstream body (200 with non-JSON content) on the stats endpoint.
    stub.set_xds(200, xds_body("healthy", &names));
    stub.set_stats(200, "this is not json{".to_string());
    let partial = fetch_partial(&http, &dash).await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a malformed stats response must render an \"unavailable\" panel; body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 6: no inline <style> element, no inline <script> element, and no inline style=
// attribute anywhere in the served partial.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_has_no_inline_style_script_elements_or_style_attributes() {
    let names = two_names();
    let stub = start_stub(
        (200, stats_body(2)),
        (200, xds_body("healthy", &names)),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_partial(&http, &dash).await;
    let lower = partial.to_lowercase();

    assert!(
        !lower.contains("<style"),
        "the partial must not contain an inline <style> element; body:\n{partial}"
    );

    // Any <script> tag must be an external script (carry src=); inline <script> elements
    // are forbidden. (The partial is not expected to contain scripts at all.)
    for tag in all_opening_tags(&lower, "<script") {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden in the partial; every script tag must \
             carry src=; offending tag: {tag:?}"
        );
    }

    assert!(
        !lower.contains(" style="),
        "the partial must not contain any inline style= attribute; body:\n{partial}"
    );
}
