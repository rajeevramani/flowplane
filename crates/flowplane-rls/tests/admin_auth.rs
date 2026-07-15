//! Black-box integration tests for the CP-facing admin listener's server TLS +
//! bearer credential (feature slice fpv2-9sf.2, design AC5/AC6 wire-level).
//!
//! These tests treat the crate as opaque: they resolve a real `RlsConfig` from
//! an env map (the only way to obtain an `AdminCredential`), serve the real
//! admin router over HTTPS via the public `admin_rustls_config` entry point,
//! and attack it over the wire with reqwest. Policy application is observed
//! through the real gRPC enforcement path (same `Arc<PolicyCache>`), so the
//! discriminating assertion for every rejected push is "the enforced set did
//! not change", not merely a status code.
//!
//! Adversarial intent — bug classes each test is shaped to expose:
//!   - a 401 that is returned AFTER the policy set was already swapped
//!     (auth check ordered after the handler side effect),
//!   - prefix/substring token comparison (trailing-garbage token accepted),
//!   - a router that only guards the routes it knows about (unknown paths
//!     falling through to an unauthenticated 404 instead of default-deny 401),
//!   - Open/Protected classification drift (ROUTES parity is asserted exactly),
//!   - non-UTF-8 header bytes panicking or bypassing the comparison,
//!   - TLS accidentally not applied to the admin listener (plaintext dial
//!     succeeding would leak the bearer on the wire).
//!
//! Config-matrix behavior of `RlsConfig::resolve` (xor of TOKEN/TOKEN_FILE,
//! partial TLS pairs, hatch values) is covered by unit tests elsewhere and
//! deliberately NOT duplicated here; `resolve` is used only as the constructor
//! for the credential + TLS config under test.
//!
//! Parallel-safety: every listener binds 127.0.0.1:0, every test writes its
//! PEMs to a unique per-test temp dir (pid + atomic counter), no fixed ports,
//! no shared globals, no process-global env mutation (resolve takes a map).
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use envoy_types::pb::envoy::extensions::common::ratelimit::v3::{
    rate_limit_descriptor::Entry, RateLimitDescriptor,
};
use envoy_types::pb::envoy::service::ratelimit::v3::rate_limit_response::Code;
use envoy_types::pb::envoy::service::ratelimit::v3::{
    rate_limit_service_client::RateLimitServiceClient,
    rate_limit_service_server::RateLimitServiceServer, RateLimitRequest,
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde_json::json;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Channel, Server};

use flowplane_rls::admin::{router, AdminState, AuthClass, ROUTES};
use flowplane_rls::config::RlsConfig;
use flowplane_rls::counter::InMemoryFixedWindow;
use flowplane_rls::grpc::RlsService;
use flowplane_rls::policy::PolicyCache;
use flowplane_rls::server::admin_rustls_config;

const OK: i32 = Code::Ok as i32;
const OVER: i32 = Code::OverLimit as i32;

/// The bearer the harness configures. Deliberately long enough that a
/// prefix-compare bug (trailing garbage accepted) is distinguishable.
const GOOD_TOKEN: &str = "s3cr3t-admin-bearer-0123456789abcdef";

// ---------------------------------------------------------------------------
// Cert minting (rcgen 0.13) — CA + server leaf for 127.0.0.1/localhost.
// ---------------------------------------------------------------------------

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
            "rls-admin-auth-{}-{}",
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

// ---------------------------------------------------------------------------
// Harness: HTTPS admin (credential configured) + plaintext gRPC observer.
// ---------------------------------------------------------------------------

/// A live RLS under test: HTTPS admin with a bearer credential, plus a
/// plaintext loopback gRPC server sharing the SAME `Arc<PolicyCache>` so the
/// enforced set is observable independently of the admin listener under attack.
struct HttpsHarness {
    /// e.g. "https://127.0.0.1:PORT" — clients must verify against `ca_pem`.
    admin_base: String,
    admin_port: u16,
    grpc: RateLimitServiceClient<Channel>,
    /// reqwest client that trusts ONLY the minted CA and does full verification.
    http: reqwest::Client,
    _cert_dir: TempCertDir,
}

impl HttpsHarness {
    async fn start() -> Self {
        let ca = TestCa::mint("Flowplane RLS admin-auth Test CA");
        let (server_cert_pem, server_key_pem) = ca.mint_server_leaf();

        let cert_dir = TempCertDir::new();
        let cert_path = cert_dir.write("admin-cert.pem", &server_cert_pem);
        let key_path = cert_dir.write("admin-key.pem", &server_key_pem);

        // The ONLY way to obtain an AdminCredential is RlsConfig::resolve.
        // gRPC TLS is out of scope for this slice, so the loopback gRPC hatch
        // is set; the admin side is fully secured (TLS pair + token), so no
        // admin hatch is needed.
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
        env.insert("FLOWPLANE_RLS_ADMIN_TOKEN".into(), GOOD_TOKEN.into());

        let config = RlsConfig::resolve(&env)
            .expect("resolve must accept a full admin TLS pair + token env");
        let admin_tls = config
            .admin_tls
            .as_ref()
            .expect("resolve must surface admin_tls when the cert/key pair is set");
        let credential = config
            .admin_credential
            .expect("resolve must surface admin_credential when the token is set");

        let rustls_config = admin_rustls_config(admin_tls)
            .await
            .expect("admin_rustls_config with a valid pair");

        let policies = Arc::new(PolicyCache::new());
        let counters = Arc::new(InMemoryFixedWindow::new());

        // --- HTTPS admin server (axum-server rustls acceptor) ----------------
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind admin");
        std_listener.set_nonblocking(true).expect("nonblocking");
        let admin_addr = std_listener.local_addr().expect("admin addr");
        let state = AdminState {
            policies: Arc::clone(&policies),
            credential: Some(Arc::new(credential)),
        };
        let app = router(state);
        tokio::spawn(async move {
            let _ = axum_server::from_tcp_rustls(std_listener, rustls_config)
                .serve(app.into_make_service())
                .await;
        });

        // --- plaintext gRPC observer (same PolicyCache) -----------------------
        let svc = RlsService::new(Arc::clone(&policies), counters);
        let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let grpc_addr = grpc_listener.local_addr().unwrap();
        let incoming = TcpIncoming::from(grpc_listener);
        tokio::spawn(async move {
            Server::builder()
                .add_service(RateLimitServiceServer::new(svc))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });

        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(
                reqwest::Certificate::from_pem(ca.pem().as_bytes()).expect("ca cert for client"),
            )
            .danger_accept_invalid_certs(false)
            .build()
            .expect("reqwest client");

        let harness = HttpsHarness {
            admin_base: format!("https://{admin_addr}"),
            admin_port: admin_addr.port(),
            grpc: connect_grpc_with_retry(grpc_addr).await,
            http,
            _cert_dir: cert_dir,
        };

        // Readiness: /healthz is Open, so a tokenless 200 over verified TLS
        // proves the https acceptor is up before any test assertion runs.
        harness.wait_healthy().await;
        harness
    }

    async fn wait_healthy(&self) {
        let url = format!("{}/healthz", self.admin_base);
        let mut last: Option<String> = None;
        for _ in 0..50 {
            match self.http.get(&url).send().await {
                Ok(resp) if resp.status() == reqwest::StatusCode::OK => return,
                Ok(resp) => last = Some(format!("status {}", resp.status())),
                Err(e) => last = Some(format!("error {e}")),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("https admin listener never became healthy: {last:?}");
    }

    /// Raw push: POST the snapshot with an arbitrary (or absent) Authorization
    /// header value. Returns the response; callers assert the status.
    async fn push_raw(
        &self,
        body: &serde_json::Value,
        auth: Option<HeaderValue>,
    ) -> reqwest::Response {
        let mut req = self
            .http
            .post(format!("{}/api/v1/admin/rls/policies", self.admin_base))
            .json(body);
        if let Some(value) = auth {
            req = req.header(AUTHORIZATION, value);
        }
        req.send().await.expect("https request must complete")
    }

    /// Push with the CORRECT bearer; asserts the documented 204.
    async fn push_authed(&self, body: &serde_json::Value) {
        let auth = HeaderValue::from_str(&format!("Bearer {GOOD_TOKEN}")).unwrap();
        let resp = self.push_raw(body, Some(auth)).await;
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::NO_CONTENT,
            "authorized policy push must return 204 No Content"
        );
    }

    /// One ShouldRateLimit call for {domain, client_id=bob}; returns the code.
    async fn check(&mut self, domain: &str) -> i32 {
        let req = RateLimitRequest {
            domain: domain.to_string(),
            descriptors: vec![RateLimitDescriptor {
                entries: vec![Entry {
                    key: "client_id".to_string(),
                    value: "bob".to_string(),
                }],
                limit: None,
                hits_addend: None,
            }],
            hits_addend: 0,
        };
        self.grpc
            .should_rate_limit(req)
            .await
            .expect("gRPC observer call must succeed")
            .into_inner()
            .overall_code
    }

    /// The seeded rpu=1 policy on `domain` still enforces: OK then OVER.
    /// This is the discriminating "set unchanged" observation — a snapshot
    /// replaced by an attacker's push would leave this domain unenforced.
    async fn assert_domain_enforces(&mut self, domain: &str) {
        assert_eq!(
            self.check(domain).await,
            OK,
            "seeded policy on {domain}: 1st call must be OK"
        );
        assert_eq!(
            self.check(domain).await,
            OVER,
            "seeded policy on {domain}: 2nd call must be OVER_LIMIT (rpu=1) — \
             proves the original snapshot is still the enforced set"
        );
    }

    /// The attacker's would-be policy on `domain` is NOT enforced (rpu=1 would
    /// trip on call 2 if the unauthorized push had been applied).
    async fn assert_domain_unenforced(&mut self, domain: &str) {
        for i in 1..=3 {
            assert_eq!(
                self.check(domain).await,
                OK,
                "attacker policy on {domain} must NOT be enforced (call #{i}) — \
                 an unauthorized push must never replace the snapshot"
            );
        }
    }
}

async fn connect_grpc_with_retry(addr: std::net::SocketAddr) -> RateLimitServiceClient<Channel> {
    let url = format!("http://{addr}");
    let mut last_err = None;
    for _ in 0..50 {
        match RateLimitServiceClient::connect(url.clone()).await {
            Ok(client) => return client,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    panic!("could not connect to gRPC observer at {url}: {last_err:?}");
}

fn snapshot(domain: &str) -> serde_json::Value {
    json!({
        "policies": [{
            "domain": domain,
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    })
}

fn bearer(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).expect("ascii bearer header")
}

// ---------------------------------------------------------------------------
// AC5a — correct bearer over https: 204 AND the set is applied.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn correct_token_push_succeeds_and_is_applied() {
    let mut h = HttpsHarness::start().await;
    h.push_authed(&snapshot("orgA|teamA|checkout")).await;

    // The push must have really landed in the enforced set, not just 204'd.
    h.assert_domain_enforces("orgA|teamA|checkout").await;
}

// ---------------------------------------------------------------------------
// AC5b — wrong token: 401 AND the previously-seeded set is unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrong_token_gets_401_and_does_not_replace_the_set() {
    let mut h = HttpsHarness::start().await;

    // Seed a known-good snapshot with the real credential.
    h.push_authed(&snapshot("orgA|teamA|seeded")).await;

    // Attack: a DIFFERENT snapshot (which would both drop the seeded policy
    // and introduce the attacker's) pushed with a wrong token.
    let resp = h
        .push_raw(
            &snapshot("orgA|teamA|attacker"),
            Some(bearer("wrong-token-entirely")),
        )
        .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "wrong bearer must be rejected with 401"
    );

    // The enforced set must be exactly the seeded one: seeded still enforces,
    // attacker's domain does not.
    h.assert_domain_unenforced("orgA|teamA|attacker").await;
    h.assert_domain_enforces("orgA|teamA|seeded").await;
}

// ---------------------------------------------------------------------------
// AC5c — absent Authorization header: 401, set unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_authorization_header_gets_401_and_set_is_unchanged() {
    let mut h = HttpsHarness::start().await;
    h.push_authed(&snapshot("orgA|teamA|seeded")).await;

    let resp = h.push_raw(&snapshot("orgA|teamA|attacker"), None).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "push without any Authorization header must be rejected with 401"
    );

    h.assert_domain_unenforced("orgA|teamA|attacker").await;
    h.assert_domain_enforces("orgA|teamA|seeded").await;
}

// ---------------------------------------------------------------------------
// AC5d — malformed Authorization headers: all 401, set unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_authorization_headers_all_get_401() {
    let mut h = HttpsHarness::start().await;
    h.push_authed(&snapshot("orgA|teamA|seeded")).await;

    let malformed: Vec<(&str, HeaderValue)> = vec![
        ("wrong scheme", HeaderValue::from_static("Basic abc")),
        ("scheme only, no token", HeaderValue::from_static("Bearer")),
        (
            "scheme + space, empty token",
            HeaderValue::from_static("Bearer "),
        ),
        (
            "token with trailing garbage (prefix-compare trap)",
            bearer(&format!("{GOOD_TOKEN}x")),
        ),
        (
            "token with trailing space+word",
            bearer(&format!("{GOOD_TOKEN} extra")),
        ),
        (
            "bare token, no scheme",
            HeaderValue::from_str(GOOD_TOKEN).unwrap(),
        ),
    ];

    for (label, value) in malformed {
        let resp = h
            .push_raw(&snapshot("orgA|teamA|attacker"), Some(value))
            .await;
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "malformed Authorization ({label}) must be rejected with 401"
        );
    }

    // None of the malformed attempts may have replaced the snapshot.
    h.assert_domain_unenforced("orgA|teamA|attacker").await;
    h.assert_domain_enforces("orgA|teamA|seeded").await;
}

// ---------------------------------------------------------------------------
// AC5e — non-UTF-8 Authorization header bytes: 401, no panic, set unchanged.
// HeaderValue accepts opaque bytes 0x20..=0xFF (except DEL), so a value like
// b"Bearer \xff\xfe" is constructible and travels on the wire.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_utf8_authorization_header_gets_401() {
    let mut h = HttpsHarness::start().await;
    h.push_authed(&snapshot("orgA|teamA|seeded")).await;

    let value = HeaderValue::from_bytes(b"Bearer \xff\xfe")
        .expect("HeaderValue permits opaque non-UTF-8 bytes");
    let resp = h
        .push_raw(&snapshot("orgA|teamA|attacker"), Some(value))
        .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "non-UTF-8 Authorization bytes must be rejected with 401 (not 5xx / not accepted)"
    );

    h.assert_domain_unenforced("orgA|teamA|attacker").await;
    h.assert_domain_enforces("orgA|teamA|seeded").await;
}

// ---------------------------------------------------------------------------
// AC5f — health endpoints stay Open over https: 200 without any token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_endpoints_are_open_over_https_without_token() {
    let h = HttpsHarness::start().await;
    for path in ["/healthz", "/readyz"] {
        let resp = h
            .http
            .get(format!("{}{path}", h.admin_base))
            .send()
            .await
            .expect("https GET must complete");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "GET {path} must be 200 without a token even when a credential is configured"
        );
    }
}

// ---------------------------------------------------------------------------
// AC5g — default-deny parity against the shared route inventory.
// ---------------------------------------------------------------------------

/// Every route the router is built from must behave per its declared class
/// when no token is presented: Protected -> 401 (for ANY method — the guard
/// runs before routing, so even a method the route doesn't serve is 401),
/// Open -> 200 on GET. An UNDECLARED path must also be 401 (default-deny
/// covers future/unknown routes; an unauthenticated 404 would be the bug).
/// Finally the inventory itself is pinned: exactly 3 entries — anyone adding
/// a route must come here and consciously classify it.
#[tokio::test]
async fn route_inventory_default_deny_parity() {
    let mut h = HttpsHarness::start().await;
    h.push_authed(&snapshot("orgA|teamA|seeded")).await;

    // --- pin the inventory -------------------------------------------------
    assert_eq!(
        ROUTES.len(),
        3,
        "ROUTES changed size — a route was added/removed; update this parity \
         test and consciously classify the new route: {ROUTES:?}"
    );
    let class_of = |path: &str| -> &AuthClass {
        &ROUTES
            .iter()
            .find(|(p, _)| *p == path)
            .unwrap_or_else(|| panic!("expected route {path} missing from ROUTES: {ROUTES:?}"))
            .1
    };
    assert!(
        matches!(class_of("/api/v1/admin/rls/policies"), AuthClass::Protected),
        "the policy-push route must be declared Protected"
    );
    assert!(
        matches!(class_of("/healthz"), AuthClass::Open),
        "/healthz must be declared Open"
    );
    assert!(
        matches!(class_of("/readyz"), AuthClass::Open),
        "/readyz must be declared Open"
    );

    // --- wire-level parity, driven from the inventory itself ----------------
    for (path, class) in ROUTES {
        assert!(
            path.starts_with('/'),
            "route path {path:?} must be absolute"
        );
        match class {
            AuthClass::Open => {
                let resp = h
                    .http
                    .get(format!("{}{path}", h.admin_base))
                    .send()
                    .await
                    .expect("https GET must complete");
                assert_eq!(
                    resp.status(),
                    reqwest::StatusCode::OK,
                    "Open route {path} must be 200 without a token"
                );
            }
            AuthClass::Protected => {
                for method in [reqwest::Method::GET, reqwest::Method::POST] {
                    let resp = h
                        .http
                        .request(method.clone(), format!("{}{path}", h.admin_base))
                        .send()
                        .await
                        .expect("https request must complete");
                    assert_eq!(
                        resp.status(),
                        reqwest::StatusCode::UNAUTHORIZED,
                        "Protected route {path} ({method}) must be 401 without a token \
                         (auth must run before routing/method dispatch)"
                    );
                }
            }
        }
    }

    // --- default-deny on paths NOT in the inventory --------------------------
    for path in ["/api/v1/admin/rls/other", "/definitely/not/a/route"] {
        for method in [reqwest::Method::GET, reqwest::Method::POST] {
            let resp = h
                .http
                .request(method.clone(), format!("{}{path}", h.admin_base))
                .send()
                .await
                .expect("https request must complete");
            assert_eq!(
                resp.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "undeclared path {path} ({method}) must get 401 without a token — \
                 default-deny must cover routes that do not exist yet (a tokenless \
                 404 would leak route-existence and break the deny-by-default model)"
            );
        }
    }

    // Sanity: the probing above must not have disturbed the enforced set.
    h.assert_domain_enforces("orgA|teamA|seeded").await;
}

// ---------------------------------------------------------------------------
// AC5h — timing-oracle hygiene smoke: both same-length and different-length
// wrong tokens are rejected (no timing assertion, just both 401).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn same_length_and_different_length_wrong_tokens_both_rejected() {
    let h = HttpsHarness::start().await;

    // Same length as GOOD_TOKEN, last byte flipped.
    let mut same_len = GOOD_TOKEN.to_string();
    same_len.pop();
    same_len.push('X');
    assert_eq!(same_len.len(), GOOD_TOKEN.len());
    assert_ne!(same_len, GOOD_TOKEN);

    for (label, tok) in [
        ("same-length wrong token", same_len.as_str()),
        ("different-length wrong token", "short"),
    ] {
        let resp = h
            .push_raw(&snapshot("orgA|teamA|attacker"), Some(bearer(tok)))
            .await;
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{label} must be rejected with 401"
        );
    }
}

// ---------------------------------------------------------------------------
// AC5-transport — the bearer only ever travels over TLS in this configuration:
// a plaintext http:// dial against the https listener must not work.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plaintext_dial_to_https_admin_listener_fails() {
    let h = HttpsHarness::start().await;

    // Plain client (no TLS anywhere) hitting the TLS acceptor's port.
    let plain = reqwest::Client::new();
    let result = plain
        .post(format!(
            "http://127.0.0.1:{}/api/v1/admin/rls/policies",
            h.admin_port
        ))
        .header(AUTHORIZATION, bearer(GOOD_TOKEN))
        .json(&snapshot("orgA|teamA|plaintext"))
        .send()
        .await;

    match result {
        Err(_) => (), // handshake/protocol failure — good
        Ok(resp) => {
            // If the acceptor answered plaintext HTTP at all, it must not be a
            // success — a 204 here would mean the bearer traveled in the clear.
            assert!(
                !resp.status().is_success(),
                "plaintext HTTP against the https admin listener must not succeed, got {}",
                resp.status()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Dev-mode control — credential None keeps the loopback dev path tokenless.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn credential_none_allows_tokenless_push() {
    // Plain http + no credential: matches the existing tests/server.rs mode.
    let policies = Arc::new(PolicyCache::new());
    let admin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let state = AdminState {
        policies: Arc::clone(&policies),
        credential: None,
    };
    tokio::spawn(async move {
        axum::serve(admin_listener, router(state)).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{admin_addr}/api/v1/admin/rls/policies"))
        .json(&snapshot("orgA|teamA|dev"))
        .send()
        .await
        .expect("plaintext dev push must complete");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "with no credential configured, a tokenless push must still be 204 (dev path)"
    );
}
