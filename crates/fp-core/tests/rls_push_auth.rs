//! Black-box integration tests for the CP-side half of the RLS admin push credential
//! (`fp_core::services::rls_sync::reconcile_once`, feature slice fpv2-9sf.3, design AC7).
//!
//! Contract under test: `reconcile_once(pool, admin_url, admin_token, client)` POSTs the
//! full snapshot to `{admin_url}/api/v1/admin/rls/policies`; `Some(token)` must produce an
//! `Authorization: Bearer <token>` header, `None` must produce NO Authorization header at
//! all; non-2xx and transport failures surface as `Err` so the reconcile loop retries.
//!
//! Adversarial intent — bug classes each test is shaped to expose:
//!   - a header that is not byte-exact "Bearer <token>" (extra whitespace, wrong scheme,
//!     lowercase "bearer", token echoed without the scheme),
//!   - a stale/default Authorization header leaking when the token is None (e.g. a client
//!     default header, or `Some("")` semantics accidentally applied to None),
//!   - a 401 swallowed into Ok(0) instead of Err (the retry loop would then never repush),
//!   - a mismatched bearer against the REAL flowplane-rls admin quietly "succeeding",
//!   - TLS verification skipped by the pushing client (a CA-pinned client reaching a server
//!     from a different CA would mean danger_accept_invalid_certs-style bypass — the push
//!     must never be DELIVERED, not merely error afterwards).
//!
//! The config fail-closed matrix (ServerConfig / RlsConfig::resolve xor rules) is covered
//! by unit tests elsewhere and deliberately not duplicated here; `RlsConfig::resolve` is
//! used only as the constructor for the real receiving side's credential + TLS material.
//!
//! Parallel-safety: shared external DB (skip-if-unset `FLOWPLANE_TEST_DATABASE_URL`), so we
//! never assert global snapshot counts — an empty-or-whatever snapshot is fine because the
//! subject here is the request envelope, not the body composition (that is rls_sync.rs's
//! job). All listeners bind 127.0.0.1:0; PEMs go to unique per-test temp dirs.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fp_core::services::rls_sync as sync;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use sqlx::PgPool;

use flowplane_rls::admin::{router, AdminState};
use flowplane_rls::config::RlsConfig;
use flowplane_rls::policy::PolicyCache;
use flowplane_rls::server::admin_rustls_config;

// ============================================================================================
// DB fixture — mirrored verbatim from tests/rls_sync.rs (skip-if-unset shared PG).
// ============================================================================================

struct Harness {
    pool: PgPool,
}

async fn harness() -> Option<Harness> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(Harness { pool })
}

// ============================================================================================
// Stub RLS admin: captures the Authorization header (raw bytes) of every POST to the
// policies route; the returned status is mutable so a single stub can go 401 -> 204.
// ============================================================================================

/// What the stub saw for one POST: the raw Authorization header (None = header absent),
/// plus every OTHER header name, so a token leaking under a non-standard name is caught.
#[derive(Debug, Clone)]
struct Seen {
    authorization: Option<Vec<u8>>,
    other_header_names: Vec<String>,
    body: serde_json::Value,
}

#[derive(Clone)]
struct StubState {
    status: Arc<Mutex<axum::http::StatusCode>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

async fn stub_handler(
    axum::extract::State(state): axum::extract::State<StubState>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::http::StatusCode {
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .map(|v| v.as_bytes().to_vec());
    let other_header_names = headers
        .keys()
        .filter(|k| *k != axum::http::header::AUTHORIZATION)
        .map(|k| k.as_str().to_string())
        .collect();
    state.seen.lock().expect("seen lock").push(Seen {
        authorization,
        other_header_names,
        body,
    });
    *state.status.lock().expect("status lock")
}

fn stub_router(state: StubState) -> axum::Router {
    axum::Router::new()
        .route(
            "/api/v1/admin/rls/policies",
            axum::routing::post(stub_handler),
        )
        .with_state(state)
}

/// Plain-HTTP capture stub on 127.0.0.1:0. Returns (addr, state).
async fn spawn_stub(initial_status: axum::http::StatusCode) -> (SocketAddr, StubState) {
    let state = StubState {
        status: Arc::new(Mutex::new(initial_status)),
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let app = stub_router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });
    (addr, state)
}

// ============================================================================================
// Cert minting (rcgen 0.13) — CA + server leaf for 127.0.0.1, idioms from
// crates/flowplane-rls/tests/admin_auth.rs / crates/fp-core/tests/oidc_ca_bundle.rs.
// ============================================================================================

struct TestCa {
    cert: rcgen::Certificate,
    key: KeyPair,
}

impl TestCa {
    fn mint(common_name: &str) -> Self {
        let key = KeyPair::generate().expect("ca key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let cert = params.self_signed(&key).expect("self-signed ca");
        Self { cert, key }
    }

    fn pem(&self) -> String {
        self.cert.pem()
    }

    /// A server-auth leaf for 127.0.0.1 / localhost, signed by this CA.
    fn mint_server_leaf(&self) -> (String, String) {
        let key = KeyPair::generate().expect("server leaf key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("leaf params");
        params
            .distinguished_name
            .push(DnType::CommonName, "127.0.0.1");
        params.is_ca = IsCa::NoCa;
        params.subject_alt_names = vec![
            SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
            SanType::DnsName("localhost".try_into().expect("dns san")),
        ];
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params
            .signed_by(&key, &self.cert, &self.key)
            .expect("server leaf signed by ca");
        (cert.pem(), key.serialize_pem())
    }
}

/// A uniquely-named per-test temp dir for PEM files, deleted on drop.
struct TempCertDir {
    dir: PathBuf,
}

impl TempCertDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rls-push-auth-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("create temp cert dir");
        Self { dir }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, contents).expect("write temp file");
        path
    }
}

impl Drop for TempCertDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ============================================================================================
// Real RLS admin over HTTPS + bearer — the receiving side of the round-trip tests.
// The ONLY way to obtain credential + TLS material is RlsConfig::resolve (env map).
// ============================================================================================

const RLS_TOKEN: &str = "cp-push-bearer-0123456789abcdef-fpv2-9sf3";

struct RealRls {
    /// e.g. "https://127.0.0.1:PORT" (no path — reconcile_once appends the route).
    base: String,
    /// CA-pinned client (trusts ONLY the harness CA, full verification).
    client: reqwest::Client,
    _cert_dir: TempCertDir,
}

/// Resolve a real RlsConfig for a given cert/key pair + token, serve the REAL admin
/// router over axum-server rustls on 127.0.0.1:0, and return a client pinned to `pin_ca`.
/// `pin_ca` == the serving CA for the matched-trust tests; a different CA for the
/// pinning-enforcement test.
async fn spawn_real_rls(serving_ca: &TestCa, pin_ca: &TestCa) -> RealRls {
    let (server_cert_pem, server_key_pem) = serving_ca.mint_server_leaf();
    let cert_dir = TempCertDir::new();
    let cert_path = cert_dir.write("admin-cert.pem", &server_cert_pem);
    let key_path = cert_dir.write("admin-key.pem", &server_key_pem);

    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("FLOWPLANE_RLS_GRPC_LISTEN".into(), "127.0.0.1:0".into());
    env.insert("FLOWPLANE_RLS_ADMIN_LISTEN".into(), "127.0.0.1:0".into());
    // gRPC TLS is out of scope for this slice; the loopback hatch satisfies resolve.
    env.insert(
        "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC".into(),
        "yes-this-is-local-only".into(),
    );
    env.insert(
        "FLOWPLANE_RLS_ADMIN_TLS_CERT".into(),
        cert_path.display().to_string(),
    );
    env.insert(
        "FLOWPLANE_RLS_ADMIN_TLS_KEY".into(),
        key_path.display().to_string(),
    );
    env.insert("FLOWPLANE_RLS_ADMIN_TOKEN".into(), RLS_TOKEN.into());

    let config =
        RlsConfig::resolve(&env).expect("resolve must accept a full admin TLS pair + token");
    let admin_tls = config
        .admin_tls
        .as_ref()
        .expect("resolve must surface admin_tls for a full cert/key pair");
    let credential = config
        .admin_credential
        .expect("resolve must surface admin_credential when the token is set");

    let rustls_config = admin_rustls_config(admin_tls)
        .await
        .expect("admin_rustls_config with a valid pair");

    let state = AdminState {
        policies: Arc::new(PolicyCache::new()),
        credential: Some(Arc::new(credential)),
    };
    let app = router(state);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind rls admin");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("rls admin addr");
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(std_listener, rustls_config)
            .serve(app.into_make_service())
            .await;
    });

    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(
            reqwest::Certificate::from_pem(pin_ca.pem().as_bytes()).expect("pin ca cert"),
        )
        .danger_accept_invalid_certs(false)
        .build()
        .expect("ca-pinned reqwest client");

    let rls = RealRls {
        base: format!("https://{addr}"),
        client,
        _cert_dir: cert_dir,
    };

    // Readiness: wait until the TLS acceptor answers at all. When the client is pinned to
    // the SERVING CA we insist on a healthy 200; when deliberately mis-pinned, a handshake
    // error still proves the listener is up (a refused TCP connect would keep looping).
    let trust_matches = std::ptr::eq(serving_ca, pin_ca);
    let url = format!("{}/healthz", rls.base);
    let mut last: Option<String> = None;
    for _ in 0..50 {
        match rls.client.get(&url).send().await {
            Ok(resp) if resp.status() == reqwest::StatusCode::OK => return rls,
            Ok(resp) => last = Some(format!("status {}", resp.status())),
            Err(e) => {
                if !trust_matches && !e.is_connect() {
                    // Listener is up; only the (intended) trust failure remains.
                    return rls;
                }
                last = Some(format!("error {e}"));
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("real RLS admin listener never became reachable: {last:?}");
}

// ============================================================================================
// AC1 — bearer present: header is byte-exactly "Bearer tok"; 204 push succeeds.
// ============================================================================================

#[tokio::test]
async fn some_token_sends_exactly_bearer_token_and_succeeds_on_204() {
    let Some(h) = harness().await else { return };
    let (addr, stub) = spawn_stub(axum::http::StatusCode::NO_CONTENT).await;
    let client = reqwest::Client::new();

    let count = sync::reconcile_once(&h.pool, &format!("http://{addr}"), Some("tok"), &client)
        .await
        .expect("push with a bearer against a 204 stub must succeed");

    let seen = stub.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1, "exactly one POST per reconcile");
    let auth = seen[0]
        .authorization
        .as_deref()
        .expect("Authorization header must be present when admin_token is Some");
    assert_eq!(
        auth,
        b"Bearer tok",
        "header must be byte-exactly `Bearer tok` — scheme-cased, single space, no \
         padding/quotes/trailing bytes (got {:?})",
        String::from_utf8_lossy(auth)
    );

    // The envelope must still carry a real snapshot body (the auth header is additive,
    // not a replacement for the payload).
    let policies = seen[0]
        .body
        .get("policies")
        .and_then(|v| v.as_array())
        .expect("body still has a policies array");
    assert_eq!(
        policies.len(),
        count,
        "returned count matches the pushed policy array length"
    );
}

// ============================================================================================
// AC2 — token None: NO Authorization header at all (absent, not empty), and no token
// smuggled under any other header name.
// ============================================================================================

#[tokio::test]
async fn none_token_sends_no_authorization_header_at_all() {
    let Some(h) = harness().await else { return };
    let (addr, stub) = spawn_stub(axum::http::StatusCode::NO_CONTENT).await;
    let client = reqwest::Client::new();

    sync::reconcile_once(&h.pool, &format!("http://{addr}"), None, &client)
        .await
        .expect("tokenless push against a 204 stub must succeed (dev path)");

    let seen = stub.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1, "exactly one POST per reconcile");
    assert!(
        seen[0].authorization.is_none(),
        "admin_token None must produce NO Authorization header — got {:?} \
         (an empty `Bearer ` or `Bearer` header is a bug, not a pass)",
        seen[0]
            .authorization
            .as_deref()
            .map(String::from_utf8_lossy)
    );
    // No credential-shaped header under another name either.
    for name in &seen[0].other_header_names {
        assert!(
            !name.to_ascii_lowercase().contains("auth")
                && !name.to_ascii_lowercase().contains("token"),
            "tokenless push must not carry a credential under an alternate header: {name}"
        );
    }
}

// ============================================================================================
// AC3 — 401 surfaces as Err; once the endpoint accepts, the SAME call path succeeds
// (this is the retry contract the reconcile loop depends on).
// ============================================================================================

#[tokio::test]
async fn unauthorized_401_is_err_and_subsequent_accepting_push_succeeds() {
    let Some(h) = harness().await else { return };
    let (addr, stub) = spawn_stub(axum::http::StatusCode::UNAUTHORIZED).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}");

    // A 401 must NOT masquerade as Ok — the reconcile loop only retries on Err.
    sync::reconcile_once(&h.pool, &url, Some("whatever"), &client)
        .await
        .expect_err("a 401 from the RLS admin endpoint must surface as Err (unavailable)");

    // The endpoint starts accepting; the identical retry now succeeds.
    *stub.status.lock().expect("lock") = axum::http::StatusCode::NO_CONTENT;
    sync::reconcile_once(&h.pool, &url, Some("whatever"), &client)
        .await
        .expect("after the endpoint accepts, the retry must succeed");

    // Both attempts really hit the wire (the Err was a server 401, not a client short-circuit).
    assert_eq!(
        stub.seen.lock().expect("lock").len(),
        2,
        "both the rejected and the retried push must reach the endpoint"
    );
}

// ============================================================================================
// AC4a — real round-trip: matched token against the REAL flowplane-rls admin over
// verified HTTPS -> Ok.
// ============================================================================================

#[tokio::test]
async fn matched_token_against_real_rls_admin_over_https_succeeds() {
    let Some(h) = harness().await else { return };
    let ca = TestCa::mint("Flowplane push-auth Test CA");
    let rls = spawn_real_rls(&ca, &ca).await;

    let count = sync::reconcile_once(&h.pool, &rls.base, Some(RLS_TOKEN), &rls.client)
        .await
        .expect("matched bearer over verified https against the real RLS admin must succeed");
    // Shared DB: the snapshot may contain anything (including nothing) — only sanity here.
    let _ = count;
}

// ============================================================================================
// AC4b — mismatched token against the real RLS admin -> Err; the matched retry then
// succeeds. (The RLS-side "set unchanged on rejected push" property is covered by the S2
// suite in flowplane-rls/tests/admin_auth.rs; the CP-side observable is Err + retry-Ok.)
// ============================================================================================

#[tokio::test]
async fn mismatched_token_against_real_rls_admin_is_err_then_matched_retry_succeeds() {
    let Some(h) = harness().await else { return };
    let ca = TestCa::mint("Flowplane push-auth Test CA");
    let rls = spawn_real_rls(&ca, &ca).await;

    sync::reconcile_once(
        &h.pool,
        &rls.base,
        Some("wrong-token-entirely"),
        &rls.client,
    )
    .await
    .expect_err("a mismatched bearer must surface as a push failure (401 -> Err)");

    // Absent credential must fail the same way (None against a credentialed RLS).
    sync::reconcile_once(&h.pool, &rls.base, None, &rls.client)
        .await
        .expect_err("a tokenless push against a credentialed RLS admin must be Err");

    // Retry path: the same pool/client with the RIGHT token now goes through.
    sync::reconcile_once(&h.pool, &rls.base, Some(RLS_TOKEN), &rls.client)
        .await
        .expect("the matched-token retry after rejected pushes must succeed");
}

// ============================================================================================
// AC5 — CA pinning enforced: client pinned to CA #1 vs a server from CA #2 -> Err, and
// the push is never DELIVERED (a TLS-over-capture stub proves zero requests landed —
// stronger than Err alone, which a post-delivery failure could also produce).
// ============================================================================================

#[tokio::test]
async fn ca_pinned_client_refuses_server_from_other_ca_and_no_push_is_delivered() {
    let Some(h) = harness().await else { return };

    let pin_ca = TestCa::mint("Flowplane push-auth pinned CA #1");
    let other_ca = TestCa::mint("Flowplane push-auth serving CA #2");

    // A capture stub served over TLS with CA #2's leaf: if the handshake were wrongly
    // accepted, the capture buffer would record the delivered snapshot.
    let (server_cert_pem, server_key_pem) = other_ca.mint_server_leaf();
    let cert_dir = TempCertDir::new();
    let cert_path = cert_dir.write("stub-cert.pem", &server_cert_pem);
    let key_path = cert_dir.write("stub-key.pem", &server_key_pem);

    // admin_rustls_config is the public way to build the acceptor config (it installs the
    // crypto provider itself); obtain the RlsAdminTls through resolve, as in the real path.
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("FLOWPLANE_RLS_GRPC_LISTEN".into(), "127.0.0.1:0".into());
    env.insert("FLOWPLANE_RLS_ADMIN_LISTEN".into(), "127.0.0.1:0".into());
    env.insert(
        "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC".into(),
        "yes-this-is-local-only".into(),
    );
    env.insert(
        "FLOWPLANE_RLS_ADMIN_TLS_CERT".into(),
        cert_path.display().to_string(),
    );
    env.insert(
        "FLOWPLANE_RLS_ADMIN_TLS_KEY".into(),
        key_path.display().to_string(),
    );
    env.insert("FLOWPLANE_RLS_ADMIN_TOKEN".into(), RLS_TOKEN.into());
    let config = RlsConfig::resolve(&env).expect("resolve tls pair for the mis-CA stub");
    let rustls_config = admin_rustls_config(config.admin_tls.as_ref().expect("admin_tls surfaced"))
        .await
        .expect("rustls config for the mis-CA stub");

    let state = StubState {
        status: Arc::new(Mutex::new(axum::http::StatusCode::NO_CONTENT)),
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    let app = stub_router(state.clone());
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind tls stub");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("tls stub addr");
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(std_listener, rustls_config)
            .serve(app.into_make_service())
            .await;
    });

    // Client trusts ONLY CA #1 — the server presents CA #2's leaf.
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(
            reqwest::Certificate::from_pem(pin_ca.pem().as_bytes()).expect("pin ca"),
        )
        .danger_accept_invalid_certs(false)
        .build()
        .expect("pinned client");

    // Give the acceptor a moment; the discriminating assertions below don't race it (a
    // connection refused and a handshake rejection are both Err + zero deliveries).
    tokio::time::sleep(Duration::from_millis(100)).await;

    sync::reconcile_once(
        &h.pool,
        &format!("https://{addr}"),
        Some(RLS_TOKEN),
        &client,
    )
    .await
    .expect_err("a CA-pinned client must refuse a server certified by a different CA");

    assert!(
        state.seen.lock().expect("lock").is_empty(),
        "the snapshot (and bearer) must NEVER be delivered across an unverified TLS \
         session — a captured request here means certificate verification was bypassed"
    );
}
