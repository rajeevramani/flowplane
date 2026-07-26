//! fpv2-80z.4 — `flowplane dashboard` SECRETS panel + auth surfaces UI restyle — black-box,
//! spec-driven contract suite.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented acceptance criteria — never the implementation. The contract under
//! test is the secrets partial `GET /<nonce>/partials/resources/secrets`, backed by a stub
//! upstream serving the four paged collections the partial sweeps
//! (`/api/v1/teams/{team}/{secrets,listeners,clusters,ai/providers}?limit=500&offset=N`).
//!
//! Acceptance criteria covered (served-HTML level):
//!   1. The secrets partial renders as a `.panel` whose h2 carries `<span class="count">`
//!      containing "metadata only, values never fetched"; the table sits in `.tablewrap`;
//!      NO `class="dataplanes"` anywhere; an expired secret renders
//!      `<span class="pill crit">EXPIRED</span>`; a secret expiring within 30 days renders
//!      `<span class="pill warn">expires ≤ 30d</span>`; a secret with no expiry shows the
//!      "no expiry" hint; the legacy `<span class="stale">` marker is GONE.
//!   2. The used-by column joins names from listeners / clusters / ai-providers; when the
//!      reference basis is incomplete (a reference source fails) the secrets still render
//!      and a banner about "Reference data" renders.
//!   3. A 403 from the secrets collection renders a "Not authorized" panel; a first-page
//!      500 renders an "unavailable" panel; no inline <style>, no inline <script>, no
//!      style= attribute in the partial.
//!
//! Parallel-safety: every test spawns its own stub upstream and dashboard child on ephemeral
//! ports (127.0.0.1:0) with an isolated `HOME` temp dir and a unique team name; nothing
//! binds a fixed port. Every spawned server is killed via a Drop guard in all paths,
//! including assertion failures.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
#![allow(dead_code)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

/// A distinctive bearer token so any leak into a response body would be unambiguous.
const SECRET_TOKEN: &str = "sekret-uistyle-secrets-token-do-not-leak-41c9";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

// =============================================================================================
// Stub upstream: a minimal in-test axum server on 127.0.0.1:0 serving the paged collections.
// Mutable per-collection state lets one test walk several upstream scenarios (200 fixtures,
// 403, 500) through a single spawned dashboard. Anything else 404s.
// =============================================================================================

struct Coll {
    status: u16,
    items: Vec<Value>,
    total: u64,
    /// Inject a failure at exactly this paging offset: `(offset, status)`.
    fail_at: Option<(u64, u16)>,
}

impl Coll {
    fn ok(items: Vec<Value>) -> Self {
        let total = items.len() as u64;
        Self {
            status: 200,
            items,
            total,
            fail_at: None,
        }
    }

    fn empty() -> Self {
        Self::ok(vec![])
    }
}

struct StubState {
    clusters: Mutex<Coll>,
    route_configs: Mutex<Coll>,
    listeners: Mutex<Coll>,
    rate_limit_domains: Mutex<Coll>,
    secrets: Mutex<Coll>,
    ai_providers: Mutex<Coll>,
}

struct StubUpstream {
    base_url: String,
    state: Arc<StubState>,
    handle: JoinHandle<()>,
}

impl StubUpstream {
    fn set_secrets_status(&self, status: u16) {
        self.state.secrets.lock().unwrap().status = status;
    }

    fn set_listeners_status(&self, status: u16) {
        self.state.listeners.lock().unwrap().status = status;
    }
}

impl Drop for StubUpstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Deterministic id with no accidental digit collisions.
fn item_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

async fn stub_handler(State(state): State<Arc<StubState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    let coll = if path.ends_with("/clusters") {
        &state.clusters
    } else if path.ends_with("/route-configs") {
        &state.route_configs
    } else if path.ends_with("/listeners") {
        &state.listeners
    } else if path.ends_with("/rate-limit-domains") {
        &state.rate_limit_domains
    } else if path.ends_with("/secrets") {
        &state.secrets
    } else if path.ends_with("/ai/providers") {
        &state.ai_providers
    } else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "no such route" })),
        )
            .into_response();
    };

    let mut limit: u64 = 500;
    let mut offset: u64 = 0;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "limit" => limit = v.parse().unwrap_or(limit),
                "offset" => offset = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    let coll = coll.lock().unwrap();
    if coll.status != 200 {
        return canned_error(coll.status);
    }
    if let Some((fail_offset, fail_status)) = coll.fail_at {
        if offset == fail_offset {
            return canned_error(fail_status);
        }
    }
    let len = coll.items.len() as u64;
    let start = offset.min(len);
    let end = offset.saturating_add(limit).min(len);
    let items: Vec<Value> = coll.items[start as usize..end as usize].to_vec();
    Json(json!({
        "items": items,
        "total": coll.total,
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

/// Start a stub serving the four collections the secrets partial sweeps (secrets, listeners,
/// clusters, ai/providers); route-configs and rate-limit-domains default to healthy empty.
async fn start_secrets_stub(
    secrets: Coll,
    listeners: Coll,
    clusters: Coll,
    ai_providers: Coll,
) -> StubUpstream {
    let state = Arc::new(StubState {
        clusters: Mutex::new(clusters),
        route_configs: Mutex::new(Coll::empty()),
        listeners: Mutex::new(listeners),
        rate_limit_domains: Mutex::new(Coll::empty()),
        secrets: Mutex::new(secrets),
        ai_providers: Mutex::new(ai_providers),
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

// =============================================================================================
// Fixture builders — shapes per the public fp-api (`SecretView`, `AiProviderView`) and
// fp-domain (`ListenerTlsConfig`, `UpstreamTlsConfig`) contracts.
// =============================================================================================

/// An RFC3339 timestamp `days` from now (negative = past).
fn rfc3339_in_days(days: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A `SecretView` item (metadata only, `value_redacted: true` — mirrors fp-api).
fn secret_item(id: &str, name: &str, expires_at: Value) -> Value {
    json!({
        "id": id,
        "team_id": item_id(999),
        "name": name,
        "description": "",
        "secret_type": "tls_certificate",
        "revision": 1,
        "encryption_key_id": "k1",
        "expires_at": expires_at,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "value_redacted": true,
    })
}

/// A listener serving its TLS certificate via SDS (`tls_certificate_sds_secret_name`).
fn listener_tls_cert_item(i: u64, name: &str, cert_secret_name: &str) -> Value {
    json!({
        "id": item_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "address": "0.0.0.0",
            "port": 8443,
            "tls_context": {
                "tls_certificate_sds_secret_name": cert_secret_name,
            }
        }
    })
}

/// A cluster verifying its upstream against an SDS-delivered CA
/// (`upstream_tls.validation_context_sds_secret_name`).
fn cluster_upstream_ca_item(i: u64, name: &str, ca_secret_name: &str) -> Value {
    json!({
        "id": item_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "endpoints": [{ "host": "10.0.0.1", "port": 8443 }],
            "use_tls": true,
            "upstream_tls": {
                "validation_context_sds_secret_name": ca_secret_name,
            }
        }
    })
}

/// An `AiProviderView` item whose spec references a secret by UUID (`credential_secret_id`).
fn ai_provider_item(i: u64, name: &str, credential_secret_id: &str) -> Value {
    json!({
        "id": item_id(i),
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
/// attribute, the text between the end of its opening tag and the next `<`, with depth
/// counting so nested same-tag elements don't truncate the region.
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
/// selector part (e.g. `.pill`, `.pill.crit`).
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

/// Assert the inline-forbidden rules shared by every partial:
/// no inline `<style>` element, every `<script>` carries src=, no inline `style=`.
fn assert_no_inline(partial: &str, which: &str) {
    let lower = partial.to_lowercase();
    assert!(
        !lower.contains("<style"),
        "the {which} partial must not contain an inline <style> element; body:\n{partial}"
    );
    for tag in all_opening_tags(&lower, "<script") {
        assert!(
            tag.contains("src="),
            "inline <script> elements are forbidden in the {which} partial; every script tag \
             must carry src=; offending tag: {tag:?}"
        );
    }
    assert!(
        !lower.contains(" style="),
        "the {which} partial must not contain any inline style= attribute; body:\n{partial}"
    );
}

/// The `<tr>...</tr>` region containing `needle` (row-scoped assertions).
fn row_of<'a>(partial: &'a str, needle: &str) -> &'a str {
    let at = partial
        .find(needle)
        .unwrap_or_else(|| panic!("row for {needle:?} must exist; body:\n{partial}"));
    let tr_open = partial[..at]
        .rfind("<tr")
        .unwrap_or_else(|| panic!("row for {needle:?} must be a <tr>; body:\n{partial}"));
    region(&partial[tr_open..], "<tr", "</tr>")
        .unwrap_or_else(|| panic!("row for {needle:?} must be closed; body:\n{partial}"))
}

// =============================================================================================
// Criterion 1: secrets partial — `.panel` h2 with `<span class="count">` containing
// "metadata only, values never fetched"; table in `.tablewrap`; NO class="dataplanes";
// expired secret → `<span class="pill crit">EXPIRED</span>`; secret expiring within 30 days
// → `<span class="pill warn">expires ≤ 30d</span>`; no-expiry secret → "no expiry" hint;
// legacy `<span class="stale">` is GONE.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_partial_panel_count_tablewrap_expiry_pills_and_no_stale() {
    let sec_expired = unique("sec-expired");
    let sec_soon = unique("sec-soon");
    let sec_noexp = unique("sec-noexp");
    let secrets = vec![
        secret_item(&item_id(1), &sec_expired, json!(rfc3339_in_days(-10))),
        secret_item(&item_id(2), &sec_soon, json!(rfc3339_in_days(10))),
        secret_item(&item_id(3), &sec_noexp, Value::Null),
    ];
    let stub = start_secrets_stub(
        Coll::ok(secrets),
        Coll::empty(),
        Coll::empty(),
        Coll::empty(),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/secrets").await;

    // The partial renders as a `.panel` whose h2 carries `<span class="count">` containing
    // "metadata only, values never fetched".
    assert!(
        has_element_with_classes(&partial, &["panel"]),
        "the secrets partial must render as a `.panel`; body:\n{partial}"
    );
    let h2 = region(&partial, "<h2", "</h2>")
        .unwrap_or_else(|| panic!("the secrets panel must have an <h2>; body:\n{partial}"));
    let count_texts = texts_of_elements_with_classes(h2, &["count"]);
    assert!(
        count_texts
            .iter()
            .any(|t| t.to_lowercase().contains("metadata only, values never fetched")),
        "the secrets panel h2 must carry `<span class=\"count\">` containing \"metadata only, \
         values never fetched\"; found count texts: {count_texts:?}; h2:\n{h2}"
    );

    // The table is wrapped in `.tablewrap`.
    if partial.contains("<table") {
        let wrap_at = class_positions(&partial, "tablewrap")
            .first()
            .copied()
            .unwrap_or_else(|| {
                panic!("the secrets table must be wrapped in `.tablewrap`; body:\n{partial}")
            });
        let table_at = partial.find("<table").expect("table present");
        assert!(
            wrap_at < table_at,
            "the `.tablewrap` wrapper must precede the <table> in the secrets partial; \
             body:\n{partial}"
        );
    } else {
        panic!("the secrets partial must render a table; body:\n{partial}");
    }

    // NO class="dataplanes" anywhere in the partial.
    assert!(
        !has_element_with_classes(&partial, &["dataplanes"]),
        "the secrets partial must not contain class=\"dataplanes\" anywhere; body:\n{partial}"
    );

    // The expired secret renders `<span class="pill crit">EXPIRED</span>`.
    let expired_row = row_of(&partial, &sec_expired);
    let crit_texts = texts_of_elements_with_classes(expired_row, &["pill", "crit"]);
    assert!(
        crit_texts.iter().any(|t| t == "EXPIRED"),
        "the expired secret row must show `<span class=\"pill crit\">EXPIRED</span>`; \
         found pill-crit texts: {crit_texts:?}; row:\n{expired_row}"
    );

    // The secret expiring within 30 days renders `<span class="pill warn">expires ≤ 30d</span>`.
    let soon_row = row_of(&partial, &sec_soon);
    let warn_texts = texts_of_elements_with_classes(soon_row, &["pill", "warn"]);
    assert!(
        warn_texts.iter().any(|t| t.contains("expires ≤ 30d")),
        "the soon-expiring secret row must show `<span class=\"pill warn\">expires ≤ 30d</span>`; \
         found pill-warn texts: {warn_texts:?}; row:\n{soon_row}"
    );

    // The secret with no expiry shows the "no expiry" hint.
    let noexp_row = row_of(&partial, &sec_noexp);
    assert!(
        noexp_row.to_lowercase().contains("no expiry"),
        "the no-expiry secret row must show the \"no expiry\" hint; row:\n{noexp_row}"
    );

    // The legacy `<span class="stale">` marker is GONE.
    assert!(
        !has_element_with_classes(&partial, &["stale"]),
        "the legacy `stale` marker must be gone from the secrets partial; body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 2: used-by column — a secret referenced by a listener (SDS cert name), a cluster
// (upstream CA name), and an ai-provider (credential_secret_id) renders the joined names;
// when the reference basis is incomplete (a reference source fails) the secrets still render
// and a banner about "Reference data" renders.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_used_by_join_and_incomplete_reference_data_banner() {
    let sec_tls = unique("sec-tls");
    let lst_name = unique("lst");
    let clu_name = unique("clu");
    let aip_name = unique("aip");
    let stub = start_secrets_stub(
        Coll::ok(vec![secret_item(&item_id(1), &sec_tls, Value::Null)]),
        Coll::ok(vec![listener_tls_cert_item(21, &lst_name, &sec_tls)]),
        Coll::ok(vec![cluster_upstream_ca_item(31, &clu_name, &sec_tls)]),
        Coll::ok(vec![ai_provider_item(41, &aip_name, &item_id(1))]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    let partial = fetch_page(&http, &dash, "partials/resources/secrets").await;

    // The used-by column of the referenced secret's row names all three referrers.
    let row = row_of(&partial, &sec_tls);
    for (kind, name) in [
        ("listener", &lst_name),
        ("cluster", &clu_name),
        ("ai-provider", &aip_name),
    ] {
        assert!(
            row.contains(name.as_str()),
            "the used-by column must name the referencing {kind} {name:?}; row:\n{row}"
        );
    }

    // A reference source fails (listeners 403) → the basis is incomplete: the secrets still
    // render (200, rows visible) and a banner about "Reference data" renders.
    stub.set_listeners_status(403);
    let partial = fetch_page(&http, &dash, "partials/resources/secrets").await;
    assert!(
        partial.contains(&sec_tls),
        "the secrets must still render when a reference source fails; body:\n{partial}"
    );
    assert!(
        has_element_with_classes(&partial, &["banner"]),
        "an incomplete reference basis must render a `.banner` element; body:\n{partial}"
    );
    let banner_texts = texts_of_elements_with_classes(&partial, &["banner"]);
    assert!(
        banner_texts
            .iter()
            .any(|t| t.to_lowercase().contains("reference data")),
        "the incomplete-basis banner must be about \"Reference data\"; \
         banner texts: {banner_texts:?}; body:\n{partial}"
    );
}

// =============================================================================================
// Criterion 3: secrets 403 → "Not authorized" panel; first-page 500 → "unavailable" panel;
// no inline style/script/style= in the partial.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_403_not_authorized_500_unavailable_and_no_inline() {
    let sec_name = unique("sec");
    let stub = start_secrets_stub(
        Coll::ok(vec![secret_item(&item_id(1), &sec_name, Value::Null)]),
        Coll::empty(),
        Coll::empty(),
        Coll::empty(),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    // 403 on the secrets collection (first page) → "Not authorized" panel.
    stub.set_secrets_status(403);
    let partial = fetch_page(&http, &dash, "partials/resources/secrets").await;
    assert!(
        partial.to_lowercase().contains("not authorized"),
        "a 403 from the secrets collection must render a \"Not authorized\" panel; \
         body:\n{partial}"
    );

    // First-page 500 → "unavailable" panel.
    stub.set_secrets_status(500);
    let partial = fetch_page(&http, &dash, "partials/resources/secrets").await;
    assert!(
        partial.to_lowercase().contains("unavailable"),
        "a first-page 500 from the secrets collection must render an \"unavailable\" panel; \
         body:\n{partial}"
    );

    // Healthy again: no inline <style>, no inline <script>, no style= in the partial.
    stub.set_secrets_status(200);
    let partial = fetch_page(&http, &dash, "partials/resources/secrets").await;
    assert_no_inline(&partial, "partials/resources/secrets");
}
