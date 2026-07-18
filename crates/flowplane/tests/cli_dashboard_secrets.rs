//! fpv2-cxw.3 — `flowplane dashboard` Resources explorer: SECRETS panel (black-box,
//! spec-driven contract suite; SECURITY-SENSITIVE, design AC 4).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the slice's documented contract — never the implementation. Contract under test:
//!
//!   * Partial `GET /<nonce>/partials/resources/secrets`. It sweeps FOUR paged team-scoped
//!     list GETs only (`limit=500&offset=N`, uniform `{items,total,limit,offset}` envelope):
//!     `/api/v1/teams/{team}/secrets` (metadata list) plus the three reference sources
//!     `/listeners`, `/clusters`, `/ai/providers`.
//!   * Secret items are metadata ONLY (fp-api `SecretView`): id, team_id, name, description,
//!     secret_type, revision, encryption_key_id, expires_at, created_at, updated_at,
//!     value_redacted — never a value.
//!   * USED-BY join (AC 4): SDS references (`tls_context.tls_certificate_sds_secret_name`,
//!     `tls_context.validation_context_sds_secret_name` on listeners,
//!     `upstream_tls.validation_context_sds_secret_name` on clusters) match by secret NAME;
//!     AI-provider `credential_secret_id` matches by secret ID (UUID). The rendered used-by
//!     names the referencing resource. A secret whose NAME merely equals a provider's
//!     credential UUID (but whose own id differs) must NOT be shown as referenced.
//!   * SECURITY (why this slice is human-gated): NO secret VALUE is ever requested — no
//!     upstream path contains "/value", nothing under `/secrets/<anything>` is ever fetched
//!     (only the flat `/secrets?limit...` list), and a distinctive fake value planted in the
//!     stub's 404 fallback must never surface in any dashboard response.
//!   * Expiry (AC 4): `expires_at` within 30 days → a warning indication near that row
//!     (case-insensitive "30d" or "expire"); past date → "EXPIRED"; null → no warning.
//!   * Failure classes: secrets 401 → HTTP 286 + `HX-Retarget: #resources` naming
//!     `flowplane auth login`; secrets 403 → HTTP 200 saying not authorized; a reference
//!     source (e.g. listeners) 403 → secrets STILL render (200, rows visible) with an
//!     incomplete-references indication ("incomplete" or "unknown", case-insensitive).
//!   * The bearer token never appears in any response body or header.
//!
//! Fixture shapes are taken from the public contracts in `fp-api` (`SecretView`,
//! `AiProviderView`) and `fp-domain` (`ListenerTlsConfig`, `UpstreamTlsConfig`,
//! `AiProviderSpec`) — NOT from the dashboard implementation. Note `ListenerTlsConfig`
//! validation requires a certificate source, so the client-CA-only listener carries inline
//! cert files alongside its `validation_context_sds_secret_name`.
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
const SECRET_TOKEN: &str = "sekret-secrets-token-do-not-leak-8b3e";

/// A distinctive fake secret value planted in the stub's 404 fallback body. If the
/// dashboard ever fetches a non-list route (e.g. a per-secret GET or a value route) AND
/// leaks the response, this string surfaces — the assertion that it never appears in any
/// dashboard response is the response-body half of the no-value-leak contract.
const FAKE_SECRET_VALUE: &str = "FAKE-SECRET-VALUE-canary-zqvxw-must-never-leak";

/// Unique, DIGIT-FREE name: hex chars of a v7 uuid mapped onto 'g'..'v'. Digit-free names
/// keep numeric assertions from being satisfied spuriously by a random name suffix.
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

/// Deterministic id, so the AI-provider join fixture can control exactly which secret id
/// equals the provider's `credential_secret_id` (and which does not).
fn det_id(i: u64) -> String {
    format!("00000000-0000-7000-8000-{i:012}")
}

// =============================================================================================
// Stub upstream: an in-test axum server on 127.0.0.1:0 serving FOUR path families —
// `/secrets`, `/listeners`, `/clusters`, `/ai/providers` — with real limit/offset paging,
// canned failures, and a full request journal (path + query + auth header). Everything else
// (including anything under `/secrets/<x>`) is recorded and answered 404 with the fake-value
// canary in the body.
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
    /// Raw query string (empty when absent).
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
    secrets: Collection,
    listeners: Collection,
    clusters: Collection,
    ai_providers: Collection,
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

    // NOTE: only the FLAT list paths match. `/secrets/<name>`, `/secrets/<name>/value`,
    // `/secrets/<name>/rotate`, … all fall through to the canary 404 below (and are
    // recorded, so allowlist assertions see them).
    if path.ends_with("/secrets") {
        return serve_page(&state.secrets, page);
    }
    if path.ends_with("/listeners") {
        return serve_page(&state.listeners, page);
    }
    if path.ends_with("/clusters") {
        return serve_page(&state.clusters, page);
    }
    if path.ends_with("/ai/providers") {
        return serve_page(&state.ai_providers, page);
    }
    // Fallback: recorded + 404 carrying the fake secret value. If the dashboard ever
    // requests a per-secret route AND reflects the response, the canary surfaces.
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "code": "not_found",
            "message": "no such route",
            "value": FAKE_SECRET_VALUE,
        })),
    )
        .into_response()
}

async fn start_stub(
    secrets: Collection,
    listeners: Collection,
    clusters: Collection,
    ai_providers: Collection,
) -> StubUpstream {
    let state = Arc::new(StubState {
        secrets,
        listeners,
        clusters,
        ai_providers,
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
// Fixture builders — shapes per the public fp-api / fp-domain contracts.
// =============================================================================================

/// A `SecretView` item (metadata only, `value_redacted: true` — mirrors fp-api).
fn secret_item(id: &str, name: &str, expires_at: Value) -> Value {
    json!({
        "id": id,
        "team_id": det_id(999),
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
        "id": det_id(i),
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

/// An mTLS listener whose client-CA validation context comes from SDS. `ListenerTlsConfig`
/// requires a certificate source, so this one uses inline cert files (NOT an SDS cert) —
/// keeping the cert-SDS secret referenced by exactly one listener.
fn listener_client_ca_item(i: u64, name: &str, ca_secret_name: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": {
            "address": "0.0.0.0",
            "port": 9443,
            "tls_context": {
                "cert_chain_file": "/etc/tls/cert.pem",
                "private_key_file": "/etc/tls/key.pem",
                "require_client_certificate": true,
                "validation_context_sds_secret_name": ca_secret_name,
            }
        }
    })
}

/// A plain listener with no TLS context at all.
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

/// A cluster verifying its upstream against an SDS-delivered CA
/// (`upstream_tls.validation_context_sds_secret_name`).
fn cluster_upstream_ca_item(i: u64, name: &str, ca_secret_name: &str) -> Value {
    json!({
        "id": det_id(i),
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

/// A plain cluster with no upstream TLS.
fn plain_cluster_item(i: u64, name: &str) -> Value {
    json!({
        "id": det_id(i),
        "name": name,
        "revision": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "spec": { "endpoints": [{ "host": "10.0.0.2", "port": 8080 }] }
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

    fn secrets_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/partials/resources/secrets",
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

/// The four documented upstream list paths for the secrets partial.
fn allowed_paths(team: &str) -> [String; 4] {
    [
        format!("/api/v1/teams/{team}/secrets"),
        format!("/api/v1/teams/{team}/listeners"),
        format!("/api/v1/teams/{team}/clusters"),
        format!("/api/v1/teams/{team}/ai/providers"),
    ]
}

/// Byte offsets of every occurrence of `needle` in `haystack`.
fn occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        out.push(start + pos);
        start += pos + needle.len();
    }
    out
}

/// Markup-agnostic row regions: each expected secret name's FIRST occurrence starts its
/// region; the region runs to the next expected name's occurrence (or end of body).
/// Works whatever order the panel renders rows in, without assuming any HTML structure
/// beyond "a row starts at its secret's name and the used-by cell follows it".
fn row_regions(body: &str, names: &[&str]) -> Vec<(String, usize, usize)> {
    let mut starts: Vec<(usize, String)> = names
        .iter()
        .map(|n| {
            let i = body.find(*n).unwrap_or_else(|| {
                panic!("secret row {n:?} must render in the partial; body:\n{body}")
            });
            (i, (*n).to_string())
        })
        .collect();
    starts.sort();
    let mut out = Vec::new();
    for (k, (start, name)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|(i, _)| *i).unwrap_or(body.len());
        out.push((name.clone(), *start, end));
    }
    out
}

/// The `(start, end)` region of one secret's row.
fn region_of(regions: &[(String, usize, usize)], name: &str) -> (usize, usize) {
    regions
        .iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, s, e)| (*s, *e))
        .unwrap_or_else(|| panic!("no region computed for {name:?}"))
}

// =============================================================================================
// Test 1 (AC 4): USED-BY JOIN — four referenced secrets (listener TLS cert by NAME,
// listener client-CA by NAME, cluster upstream-CA by NAME, AI provider credential by UUID),
// one unreferenced secret, and the id-vs-name trap: a secret whose NAME equals the
// provider's credential UUID string (but whose own id differs) must NOT show the provider.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn used_by_joins_sds_by_name_and_ai_credential_by_uuid() {
    // Prefixes fix the relative alphabetical order so the trap row (uuid name, sorts before
    // letters) and the credential row ("zz-", sorts last) stay far apart under both
    // insertion order and any name-sorted rendering.
    let n_cert = unique("gg-edgecert");
    let n_ca = unique("hh-clientca");
    let n_upca = unique("jj-upstreamca");
    let n_unref = unique("kk-unref");
    let n_cred = unique("zz-aicred");

    let l_edge = unique("edge");
    let l_mtls = unique("mtls");
    let c_backend = unique("backend");
    let p_ai = unique("prov");

    // The provider's credential UUID: the legit secret's ID equals it; the trap secret's
    // NAME equals it (as a string) while its own id differs.
    let u_cred = det_id(90);
    let trap_name = u_cred.clone();

    let secrets = Collection::ok(vec![
        secret_item(&det_id(91), &trap_name, Value::Null), // trap: name == provider's UUID
        secret_item(&det_id(1), &n_cert, Value::Null),
        secret_item(&det_id(2), &n_ca, Value::Null),
        secret_item(&det_id(3), &n_upca, Value::Null),
        secret_item(&det_id(4), &n_unref, Value::Null),
        secret_item(&u_cred, &n_cred, Value::Null), // legit: ID == provider's UUID
    ]);
    let listeners = Collection::ok(vec![
        listener_tls_cert_item(10, &l_edge, &n_cert),
        listener_client_ca_item(11, &l_mtls, &n_ca),
    ]);
    let clusters = Collection::ok(vec![cluster_upstream_ca_item(20, &c_backend, &n_upca)]);
    let providers = Collection::ok(vec![ai_provider_item(30, &p_ai, &u_cred)]);

    let stub = start_stub(secrets, listeners, clusters, providers).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.secrets_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the secrets partial must be 200 on healthy upstreams"
    );
    let body = resp.text().await.expect("secrets partial body");
    let lower = body.to_lowercase();

    // The trap row itself must render (its name is the UUID string).
    assert!(
        lower.contains(&trap_name),
        "the trap secret (name {trap_name:?}) must render as a row; body:\n{body}"
    );

    // Row regions keyed on the five letter-named secrets (the trap's name is the UUID and
    // may legitimately also appear inside the legit row if the credential id is rendered,
    // so it cannot serve as a region boundary).
    let names = [
        n_cert.as_str(),
        n_ca.as_str(),
        n_upca.as_str(),
        n_unref.as_str(),
        n_cred.as_str(),
    ];
    let regions = row_regions(&lower, &names);

    // (a) listener TLS-cert SDS reference, by NAME.
    let (s, e) = region_of(&regions, &n_cert);
    assert!(
        lower[s..e].contains(&l_edge),
        "the row for {n_cert:?} must name its referencing listener {l_edge:?} \
         (tls_certificate_sds_secret_name join by NAME); region:\n{}\nfull body:\n{body}",
        &lower[s..e]
    );

    // (b) listener client-CA validation SDS reference, by NAME.
    let (s, e) = region_of(&regions, &n_ca);
    assert!(
        lower[s..e].contains(&l_mtls),
        "the row for {n_ca:?} must name its referencing listener {l_mtls:?} \
         (validation_context_sds_secret_name join by NAME); region:\n{}\nfull body:\n{body}",
        &lower[s..e]
    );

    // (c) cluster upstream-CA SDS reference, by NAME.
    let (s, e) = region_of(&regions, &n_upca);
    assert!(
        lower[s..e].contains(&c_backend),
        "the row for {n_upca:?} must name its referencing cluster {c_backend:?} \
         (upstream_tls.validation_context_sds_secret_name join by NAME); region:\n{}\n\
         full body:\n{body}",
        &lower[s..e]
    );

    // (d) AI provider credential reference, by SECRET ID — even though the secret's NAME
    // differs from the UUID.
    let (cred_s, cred_e) = region_of(&regions, &n_cred);
    assert!(
        lower[cred_s..cred_e].contains(&p_ai),
        "the row for {n_cred:?} (id == the provider's credential_secret_id) must name its \
         referencing AI provider {p_ai:?} (join by UUID, not name); region:\n{}\n\
         full body:\n{body}",
        &lower[cred_s..cred_e]
    );

    // Unreferenced secret: its row shows NO referencing resource.
    let (s, e) = region_of(&regions, &n_unref);
    for referencing in [&l_edge, &l_mtls, &c_backend, &p_ai] {
        assert!(
            !lower[s..e].contains(referencing.as_str()),
            "the unreferenced secret {n_unref:?} must show no reference, but its row \
             region contains {referencing:?}; region:\n{}\nfull body:\n{body}",
            &lower[s..e]
        );
    }

    // ID-VS-NAME TRAP: the provider's name may appear ONLY inside the legit secret's row
    // region. If the trap row (name == UUID string, different id) wrongly showed the
    // provider, an occurrence would land outside [cred_s, cred_e] and fail here.
    for pos in occurrences(&lower, &p_ai) {
        assert!(
            (cred_s..cred_e).contains(&pos),
            "AI provider {p_ai:?} appears at byte {pos} OUTSIDE the row region \
             [{cred_s},{cred_e}) of the id-matched secret {n_cred:?} — the trap secret \
             (NAME equal to the credential UUID, id different) must NOT show the provider \
             reference (the join is by secret ID); body:\n{body}"
        );
    }

    // Security invariant holds on the happy path too.
    assert!(
        !body.contains(FAKE_SECRET_VALUE),
        "the 404-fallback fake secret value must never surface in a dashboard response; \
         body:\n{body}"
    );
}

// =============================================================================================
// Test 2 (AC 4): EXPIRY — expires in ~10 days → warning near that row; expired yesterday →
// "EXPIRED" near that row; null expiry → no warning for that row.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_warnings_soon_expired_and_none() {
    let n_soon = unique("gg-soon");
    let n_gone = unique("hh-gone");
    let n_none = unique("jj-none");

    let now = chrono::Utc::now();
    let soon = (now + chrono::Duration::days(10)).to_rfc3339();
    let past = (now - chrono::Duration::days(1)).to_rfc3339();

    let secrets = Collection::ok(vec![
        secret_item(&det_id(1), &n_soon, json!(soon)),
        secret_item(&det_id(2), &n_gone, json!(past)),
        secret_item(&det_id(3), &n_none, Value::Null),
    ]);
    let stub = start_stub(
        secrets,
        Collection::ok(vec![]),
        Collection::ok(vec![]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.secrets_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the secrets partial must be 200 on healthy upstreams"
    );
    let body = resp.text().await.expect("secrets partial body");
    let lower = body.to_lowercase();

    let names = [n_soon.as_str(), n_gone.as_str(), n_none.as_str()];
    let regions = row_regions(&lower, &names);

    // Expires within 30 days → a warning indication near that row.
    let (s, e) = region_of(&regions, &n_soon);
    assert!(
        lower[s..e].contains("30d") || lower[s..e].contains("expire"),
        "the secret expiring in ~10 days ({n_soon:?}) must carry a warning indication \
         (\"30d\" or \"expire\", case-insensitive) near its row; region:\n{}\n\
         full body:\n{body}",
        &lower[s..e]
    );

    // Past expiry → an EXPIRED indication near that row.
    let (s, e) = region_of(&regions, &n_gone);
    assert!(
        lower[s..e].contains("expired"),
        "the already-expired secret ({n_gone:?}) must carry an \"EXPIRED\" indication \
         near its row; region:\n{}\nfull body:\n{body}",
        &lower[s..e]
    );

    // Null expiry → no warning for that row.
    let (s, e) = region_of(&regions, &n_none);
    for warning in ["expired", "30d", "warn"] {
        assert!(
            !lower[s..e].contains(warning),
            "the never-expiring secret ({n_none:?}) must carry no warning, but its row \
             region contains {warning:?}; region:\n{}\nfull body:\n{body}",
            &lower[s..e]
        );
    }
}

// =============================================================================================
// Test 3 (SECURITY, the reason this slice is human-gated): NO secret VALUE is ever
// requested. With a full fixture set, every recorded upstream request targets one of the
// FOUR documented list paths; nothing contains "/value"; nothing extends `/secrets` with a
// sub-path; and the stub's fake value canary never appears in the dashboard response.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_partial_never_requests_a_secret_value() {
    let n_cert = unique("gg-cert");
    let n_cred = unique("hh-cred");
    let u_cred = det_id(90);

    let secrets = Collection::ok(vec![
        secret_item(&det_id(1), &n_cert, Value::Null),
        secret_item(&u_cred, &n_cred, Value::Null),
    ]);
    let listeners = Collection::ok(vec![listener_tls_cert_item(10, &unique("edge"), &n_cert)]);
    let clusters = Collection::ok(vec![plain_cluster_item(20, &unique("backend"))]);
    let providers = Collection::ok(vec![ai_provider_item(30, &unique("prov"), &u_cred)]);

    let stub = start_stub(secrets, listeners, clusters, providers).await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.secrets_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "the secrets partial must be 200 on healthy upstreams"
    );
    let body = resp.text().await.expect("secrets partial body");
    assert!(
        body.contains(&n_cert) && body.contains(&n_cred),
        "the secrets partial must render its rows; body:\n{body}"
    );
    assert!(
        !body.contains(FAKE_SECRET_VALUE),
        "the 404-fallback fake secret value must never surface in a dashboard response \
         (would mean a non-list route was fetched AND leaked); body:\n{body}"
    );

    // Grace period so even an asynchronously-fired extra upstream fetch would be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let recorded = stub.recorded();
    assert!(!recorded.is_empty(), "the upstream must have been called");
    let allowed = allowed_paths(&team);
    let secrets_path = format!("/api/v1/teams/{team}/secrets");
    for req in &recorded {
        // Exactly one of the four documented list paths.
        assert!(
            allowed.contains(&req.path),
            "the secrets partial sent an upstream request outside the documented set \
             (allowed: {allowed:?}): {:?}; all recorded: {recorded:?}",
            req.path
        );
        // No value route, ever — path or query.
        let full = format!("{}?{}", req.path, req.query).to_lowercase();
        assert!(
            !full.contains("/value"),
            "NO upstream request may contain \"/value\": {full:?}"
        );
        // Nothing under /secrets/<anything> — only the flat list.
        assert!(
            !req.path.starts_with(&format!("{secrets_path}/")),
            "NO upstream request may extend /secrets with a sub-path (per-secret GET): \
             {:?}; all recorded: {recorded:?}",
            req.path
        );
    }
    // The secrets metadata list itself was actually swept.
    assert!(
        recorded.iter().any(|r| r.path == secrets_path),
        "the flat secrets list {secrets_path:?} must have been swept; \
         recorded: {recorded:?}"
    );
}

// =============================================================================================
// Test 4a: secrets 401 → HTTP 286 + `HX-Retarget: #resources` naming `flowplane auth login`.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_secrets_returns_286_with_retarget_and_names_auth_login() {
    let stub = start_stub(
        Collection::failing(401),
        Collection::ok(vec![]),
        Collection::ok(vec![]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.secrets_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        286,
        "a secrets 401 must yield the htmx stop-polling status 286"
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
    assert!(
        !body.contains(FAKE_SECRET_VALUE),
        "the fake secret value must never surface; body:\n{body}"
    );
}

// =============================================================================================
// Test 4b: secrets 403 → HTTP 200 saying not authorized.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_secrets_partial_says_not_authorized() {
    let stub = start_stub(
        Collection::failing(403),
        Collection::ok(vec![plain_listener_item(10, &unique("edge"))]),
        Collection::ok(vec![plain_cluster_item(20, &unique("backend"))]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.secrets_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a secrets 403 must not fail the partial itself"
    );
    let body = resp.text().await.expect("secrets partial body");
    assert!(
        body.to_lowercase().contains("not authorized"),
        "the secrets partial must say not authorized on a secrets 403; body:\n{body}"
    );
    assert!(
        !body.contains(FAKE_SECRET_VALUE),
        "the fake secret value must never surface; body:\n{body}"
    );
}

// =============================================================================================
// Test 4c: a REFERENCE SOURCE (listeners) 403 → the secrets rows STILL render (HTTP 200)
// with an incomplete-references indication ("incomplete" or "unknown", case-insensitive).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_reference_source_still_renders_secrets_with_incomplete_indication() {
    let n_cert = unique("gg-cert");
    let n_other = unique("hh-other");
    let secrets = Collection::ok(vec![
        secret_item(&det_id(1), &n_cert, Value::Null),
        secret_item(&det_id(2), &n_other, Value::Null),
    ]);
    let stub = start_stub(
        secrets,
        Collection::failing(403), // listeners: reference source forbidden
        Collection::ok(vec![plain_cluster_item(20, &unique("backend"))]),
        Collection::ok(vec![]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);

    let resp = fetch(&client(), &dash.secrets_url()).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a listeners (reference source) 403 must not fail the secrets partial"
    );
    let body = resp.text().await.expect("secrets partial body");
    let lower = body.to_lowercase();
    assert!(
        body.contains(&n_cert) && body.contains(&n_other),
        "the secrets rows must STILL render when a reference source is forbidden; \
         body:\n{body}"
    );
    assert!(
        lower.contains("incomplete") || lower.contains("unknown"),
        "a forbidden reference source must produce an incomplete-references indication \
         (\"incomplete\" or \"unknown\", case-insensitive); body:\n{body}"
    );
    assert!(
        !body.contains(FAKE_SECRET_VALUE),
        "the fake secret value must never surface; body:\n{body}"
    );
}

// =============================================================================================
// Test 5: TOKEN NON-DISCLOSURE — the bearer token never appears in any response body or
// header (shell page and secrets partial), and neither does the fake value canary.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bearer_token_never_appears_in_any_secrets_response() {
    let n_cert = unique("gg-cert");
    let u_cred = det_id(90);
    let secrets = Collection::ok(vec![
        secret_item(&det_id(1), &n_cert, Value::Null),
        secret_item(&u_cred, &unique("hh-cred"), Value::Null),
    ]);
    let stub = start_stub(
        secrets,
        Collection::ok(vec![listener_tls_cert_item(10, &unique("edge"), &n_cert)]),
        Collection::ok(vec![plain_cluster_item(20, &unique("backend"))]),
        Collection::ok(vec![ai_provider_item(30, &unique("prov"), &u_cred)]),
    )
    .await;
    let team = unique("team");
    let dash = spawn_dashboard(common::unique_tempdir(), &stub.base_url, &team);
    let http = client();

    for url in [dash.shell_url(), dash.secrets_url()] {
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
        assert!(
            !body.contains(FAKE_SECRET_VALUE),
            "the response body of {url} leaks the fake secret value canary; body:\n{body}"
        );
    }

    // Sanity: the upstream sweeps DID carry the token (it exists, and only upstream).
    let recorded = stub.recorded();
    let want_auth = format!("Bearer {SECRET_TOKEN}");
    assert!(
        recorded
            .iter()
            .any(|r| r.authorization.as_deref() == Some(want_auth.as_str())),
        "upstream sweeps must carry the bearer token (otherwise the non-disclosure \
         assertions above prove nothing); recorded: {recorded:?}"
    );
}
