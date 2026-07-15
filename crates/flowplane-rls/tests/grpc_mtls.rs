//! Black-box integration tests for the Envoy-facing gRPC listener's server-side
//! mTLS (feature slice fpv2-9sf.1).
//!
//! These tests treat the crate as opaque: they build the real tonic server via
//! the public `flowplane_rls::server::grpc_server(&RlsConfig)` entry point with
//! a `grpc_tls` triad (server cert/key + client CA), then attack it over the
//! wire with four classes of dialer:
//!
//!   1. a client whose certificate chains to the configured client CA
//!      (must succeed AND be enforced — proving the RPC really hit the counter),
//!   2. a TLS client with NO client certificate (must be rejected),
//!   3. a TLS client with a certificate from a DIFFERENT, untrusted CA
//!      (must be rejected),
//!   4. a plaintext (non-TLS) dialer (must be rejected).
//!
//! Adversarial intent: each test is shaped to expose a plausible bug class —
//! optional-instead-of-mandatory client certs, a client-CA parameter that is
//! silently ignored, TLS applied to the wrong listener, rejected handshakes
//! still reaching the enforcement path (counter pollution), or `grpc_tls: None`
//! accidentally breaking the plaintext path.
//!
//! Config-layer edge cases (AC1/AC2/AC3/AC9 — partial triads, missing files,
//! resolve() parsing) are covered by unit tests elsewhere and deliberately NOT
//! duplicated here. We construct `RlsConfig` literally rather than through
//! `resolve()` because the env-variable names are not part of the contract
//! given to this test author.
//!
//! Parallel-safety: every listener binds `127.0.0.1:0`, every test writes its
//! PEMs to a unique per-test temp dir (pid + atomic counter), no fixed ports,
//! no shared globals.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

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
use serde_json::json;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use flowplane_rls::admin::{router, AdminState};
use flowplane_rls::config::{RlsConfig, RlsGrpcTls};
use flowplane_rls::counter::InMemoryFixedWindow;
use flowplane_rls::grpc::RlsService;
use flowplane_rls::policy::PolicyCache;
use flowplane_rls::server::grpc_server;

const OK: i32 = Code::Ok as i32;
const OVER: i32 = Code::OverLimit as i32;

/// How long a single negative-path RPC attempt may take before we call it a
/// hang (neither accepted nor rejected) and fail the test loudly.
const RPC_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Cert minting (rcgen 0.13)
// ---------------------------------------------------------------------------

/// A CA that can sign leaves: the self-signed CA cert plus its key pair.
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

    /// A server leaf for 127.0.0.1 / localhost, signed by this CA.
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

    /// A client-auth leaf signed by this CA.
    fn mint_client_leaf(&self, common_name: &str) -> (String, String) {
        let key = KeyPair::generate().expect("client leaf key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("leaf params");
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let cert = params
            .signed_by(&key, &self.cert, &self.key)
            .expect("client leaf signed by ca");
        (cert.pem(), key.serialize_pem())
    }
}

/// All the PEM material one test needs: one trusted CA that signs both the
/// server leaf and the good client leaf, plus a second, entirely separate CA
/// with its own client leaf for the untrusted-CA attack.
struct TestCerts {
    ca_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
    evil_client_cert_pem: String,
    evil_client_key_pem: String,
}

impl TestCerts {
    fn generate() -> Self {
        let ca = TestCa::mint("Flowplane RLS mTLS Test CA #1 (trusted)");
        let (server_cert_pem, server_key_pem) = ca.mint_server_leaf();
        let (client_cert_pem, client_key_pem) = ca.mint_client_leaf("rls-test-client");

        // A completely unrelated CA — its leaves must never be accepted.
        let evil_ca = TestCa::mint("Flowplane RLS mTLS Test CA #2 (untrusted)");
        let (evil_client_cert_pem, evil_client_key_pem) =
            evil_ca.mint_client_leaf("rls-evil-client");

        Self {
            ca_pem: ca.pem(),
            server_cert_pem,
            server_key_pem,
            client_cert_pem,
            client_key_pem,
            evil_client_cert_pem,
            evil_client_key_pem,
        }
    }
}

/// A uniquely-named per-test temp dir for the PEM files, deleted on drop.
/// pid + atomic counter keeps parallel tests (and parallel nextest processes)
/// from colliding.
struct TempCertDir {
    dir: PathBuf,
}

impl TempCertDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rls-mtls-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("create temp cert dir");
        Self { dir }
    }

    fn write(&self, name: &str, pem: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, pem).expect("write pem");
        path
    }
}

impl Drop for TempCertDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A live mTLS-enabled RLS under test: plaintext admin HTTP (admin TLS is a
/// later slice), mTLS gRPC built via the public `grpc_server(&RlsConfig)`
/// entry point, both sharing one `Arc<PolicyCache>`.
struct MtlsHarness {
    admin_base: String,
    grpc_port: u16,
    http: reqwest::Client,
    certs: TestCerts,
    // Keep the PEM files alive for the lifetime of the harness in case the
    // implementation (re)reads them lazily.
    _cert_dir: TempCertDir,
}

impl MtlsHarness {
    async fn start() -> Self {
        let certs = TestCerts::generate();
        let cert_dir = TempCertDir::new();
        let cert_path = cert_dir.write("server-cert.pem", &certs.server_cert_pem);
        let key_path = cert_dir.write("server-key.pem", &certs.server_key_pem);
        let client_ca_path = cert_dir.write("client-ca.pem", &certs.ca_pem);

        let policies = Arc::new(PolicyCache::new());
        let counters = Arc::new(InMemoryFixedWindow::new());

        // --- Admin HTTP server (plaintext; admin TLS is a later slice) -------
        let admin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin_addr = admin_listener.local_addr().unwrap();
        let state = AdminState {
            policies: Arc::clone(&policies),
        };
        tokio::spawn(async move {
            axum::serve(admin_listener, router(state)).await.unwrap();
        });

        // --- mTLS gRPC server, built through the PUBLIC config surface -------
        let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let grpc_addr = grpc_listener.local_addr().unwrap();
        let config = RlsConfig {
            // We bind our own ephemeral listener and serve_with_incoming, so
            // the listen addrs in the config are placeholders.
            grpc_listen: "127.0.0.1:0".parse().unwrap(),
            admin_listen: "127.0.0.1:0".parse().unwrap(),
            grpc_tls: Some(RlsGrpcTls {
                cert_path,
                key_path,
                client_ca_path,
            }),
        };
        let mut server = grpc_server(&config).expect("grpc_server with valid TLS triad");
        let svc = RlsService::new(Arc::clone(&policies), counters);
        let incoming = TcpIncoming::from(grpc_listener);
        tokio::spawn(async move {
            server
                .add_service(RateLimitServiceServer::new(svc))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });

        let harness = MtlsHarness {
            admin_base: format!("http://{admin_addr}"),
            grpc_port: grpc_addr.port(),
            http: reqwest::Client::new(),
            certs,
            _cert_dir: cert_dir,
        };

        // Wait until the mTLS listener accepts a *trusted* handshake so that
        // later negative attempts observe a live server (rejection == policy,
        // not "not up yet"). Connecting a channel issues no RPC, so this does
        // not touch any counter.
        harness.connect_trusted_with_retry().await;
        harness
    }

    /// Push a full policy snapshot over plaintext admin HTTP (204 expected).
    async fn push(&self, body: serde_json::Value) {
        let resp = self
            .http
            .post(format!("{}/api/v1/admin/rls/policies", self.admin_base))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::NO_CONTENT,
            "policy push must return 204 No Content"
        );
    }

    fn trusted_tls(&self) -> ClientTlsConfig {
        ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&self.certs.ca_pem))
            .domain_name("localhost")
            .identity(Identity::from_pem(
                &self.certs.client_cert_pem,
                &self.certs.client_key_pem,
            ))
    }

    /// TLS config that trusts the server but presents NO client certificate.
    fn no_identity_tls(&self) -> ClientTlsConfig {
        ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&self.certs.ca_pem))
            .domain_name("localhost")
    }

    /// TLS config presenting a client certificate from the UNTRUSTED CA #2.
    fn untrusted_identity_tls(&self) -> ClientTlsConfig {
        ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&self.certs.ca_pem))
            .domain_name("localhost")
            .identity(Identity::from_pem(
                &self.certs.evil_client_cert_pem,
                &self.certs.evil_client_key_pem,
            ))
    }

    /// One-shot TLS dial with the given client config (no retries — callers
    /// decide whether failure is expected).
    async fn tls_connect(&self, tls: ClientTlsConfig) -> Result<Channel, tonic::transport::Error> {
        Channel::from_shared(format!("https://127.0.0.1:{}", self.grpc_port))
            .unwrap()
            .tls_config(tls)
            .unwrap()
            .connect()
            .await
    }

    async fn connect_trusted_with_retry(&self) -> RateLimitServiceClient<Channel> {
        let mut last_err = None;
        for _ in 0..50 {
            match self.tls_connect(self.trusted_tls()).await {
                Ok(channel) => return RateLimitServiceClient::new(channel),
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        panic!(
            "trusted mTLS client could not connect to 127.0.0.1:{}: {last_err:?}",
            self.grpc_port
        );
    }

    /// Assert that a dialer with the given TLS config can never complete a
    /// ShouldRateLimit RPC. Depending on the TLS version the rejection may
    /// surface at connect time (handshake alert) or on the first RPC (TLS 1.3
    /// delivers the certificate-required alert after the client believes the
    /// handshake finished) — both are valid rejections; a SUCCESSFUL RPC is
    /// the bug.
    async fn assert_rpc_rejected(&self, tls: ClientTlsConfig, who: &str) {
        match self.tls_connect(tls).await {
            Err(_) => (), // rejected at the TLS/connect layer — good
            Ok(channel) => {
                let mut client = RateLimitServiceClient::new(channel);
                let attempt = tokio::time::timeout(
                    RPC_ATTEMPT_TIMEOUT,
                    client.should_rate_limit(rl_request(
                        "orgA|teamA|checkout",
                        vec![("client_id", "bob")],
                    )),
                )
                .await;
                match attempt {
                    Err(_) => panic!(
                        "{who}: RPC neither succeeded nor was rejected within \
                         {RPC_ATTEMPT_TIMEOUT:?} — server appears to hang instead of \
                         rejecting the handshake"
                    ),
                    Ok(result) => assert!(
                        result.is_err(),
                        "{who}: ShouldRateLimit must NOT succeed, got: {result:?}"
                    ),
                }
            }
        }
    }
}

fn rl_request(domain: &str, entries: Vec<(&str, &str)>) -> RateLimitRequest {
    RateLimitRequest {
        domain: domain.to_string(),
        descriptors: vec![RateLimitDescriptor {
            entries: entries
                .into_iter()
                .map(|(k, v)| Entry {
                    key: k.to_string(),
                    value: v.to_string(),
                })
                .collect(),
            limit: None,
            hits_addend: None,
        }],
        hits_addend: 0,
    }
}

async fn check(client: &mut RateLimitServiceClient<Channel>, domain: &str) -> i32 {
    client
        .should_rate_limit(rl_request(domain, vec![("client_id", "bob")]))
        .await
        .expect("trusted mTLS client RPC must succeed")
        .into_inner()
        .overall_code
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// AC4a — a client whose certificate chains to the configured client CA can
/// call ShouldRateLimit AND the call is really enforced: with rpu=1 the first
/// call is OK and the second OVER_LIMIT. Catches a TLS listener that
/// terminates TLS but never wires the service (or short-circuits enforcement),
/// and a server cert/CA mixup that would break the trusted path entirely.
#[tokio::test]
async fn trusted_client_cert_rpc_succeeds_and_is_enforced() {
    let h = MtlsHarness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;

    let mut client = h.connect_trusted_with_retry().await;
    let d = "orgA|teamA|checkout";
    assert_eq!(
        check(&mut client, d).await,
        OK,
        "1st trusted call must be OK"
    );
    assert_eq!(
        check(&mut client, d).await,
        OVER,
        "2nd trusted call must be OVER_LIMIT (rpu=1) — proves the RPC hit the real counter"
    );
}

/// AC4b — a TLS client that trusts the server but presents NO client
/// certificate must be rejected: the RPC never succeeds. Catches
/// optional-instead-of-mandatory client-cert verification (the classic
/// "request but don't require" misconfiguration).
#[tokio::test]
async fn client_without_certificate_is_rejected() {
    let h = MtlsHarness::start().await;
    h.assert_rpc_rejected(h.no_identity_tls(), "no-client-cert dialer")
        .await;
}

/// AC4c — a TLS client presenting a certificate from a DIFFERENT CA (not the
/// configured client CA) must be rejected identically. Catches a client-CA
/// parameter that is loaded but never actually used for chain verification
/// (e.g. an accept-any-cert verifier).
#[tokio::test]
async fn client_with_untrusted_ca_certificate_is_rejected() {
    let h = MtlsHarness::start().await;
    h.assert_rpc_rejected(
        h.untrusted_identity_tls(),
        "untrusted-CA client-cert dialer",
    )
    .await;
}

/// AC4e — a PLAINTEXT (non-TLS) dialer against the mTLS listener must fail:
/// no successful RPC. Catches TLS applied to the wrong listener or a fallback
/// that silently accepts cleartext HTTP/2.
#[tokio::test]
async fn plaintext_dialer_against_mtls_listener_is_rejected() {
    let h = MtlsHarness::start().await;

    // Dial with a plain http:// endpoint — no TLS at all.
    let attempt = Channel::from_shared(format!("http://127.0.0.1:{}", h.grpc_port))
        .unwrap()
        .connect()
        .await;
    match attempt {
        Err(_) => (), // rejected at connect — good
        Ok(channel) => {
            // TCP accept alone isn't a violation; a successful RPC is.
            let mut client = RateLimitServiceClient::new(channel);
            let result = tokio::time::timeout(
                RPC_ATTEMPT_TIMEOUT,
                client.should_rate_limit(rl_request(
                    "orgA|teamA|checkout",
                    vec![("client_id", "bob")],
                )),
            )
            .await;
            match result {
                Err(_) => panic!(
                    "plaintext dialer: RPC neither succeeded nor was rejected within \
                     {RPC_ATTEMPT_TIMEOUT:?}"
                ),
                Ok(result) => assert!(
                    result.is_err(),
                    "plaintext RPC against the mTLS listener must fail, got: {result:?}"
                ),
            }
        }
    }
}

/// AC4d — the discriminating counter assertion: rejected callers must never
/// reach the enforcement path. Against a rpu=1 policy, hammer the listener
/// with a no-cert dialer and an untrusted-CA dialer; afterwards the FIRST
/// trusted call must still be OK (budget untouched) and only the second OVER.
/// Catches a listener that rejects the RPC *response* but has already counted
/// the hit, or any pre-verification service dispatch.
#[tokio::test]
async fn rejected_callers_do_not_consume_rate_limit_budget() {
    let h = MtlsHarness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;

    // Several rejected attempts of each flavor, all targeting the SAME
    // domain+descriptor the policy counts.
    for _ in 0..2 {
        h.assert_rpc_rejected(h.no_identity_tls(), "no-client-cert dialer")
            .await;
        h.assert_rpc_rejected(
            h.untrusted_identity_tls(),
            "untrusted-CA client-cert dialer",
        )
        .await;
    }

    // Budget must be untouched: first trusted call OK, second OVER.
    let mut client = h.connect_trusted_with_retry().await;
    let d = "orgA|teamA|checkout";
    assert_eq!(
        check(&mut client, d).await,
        OK,
        "first TRUSTED call must be OK — rejected callers must not have consumed the rpu=1 budget"
    );
    assert_eq!(
        check(&mut client, d).await,
        OVER,
        "second trusted call must be OVER_LIMIT — proves the counter is live, so the previous \
         OK was a real budget check, not a no-op"
    );
}

/// Control — `grpc_tls: None` must keep serving PLAINTEXT gRPC. Catches a
/// regression where introducing the TLS branch breaks (or accidentally
/// mandates TLS on) the existing plaintext deployment mode.
#[tokio::test]
async fn grpc_tls_none_still_serves_plaintext() {
    let policies = Arc::new(PolicyCache::new());
    let counters = Arc::new(InMemoryFixedWindow::new());

    // Admin for the policy push.
    let admin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let state = AdminState {
        policies: Arc::clone(&policies),
    };
    tokio::spawn(async move {
        axum::serve(admin_listener, router(state)).await.unwrap();
    });

    // Plain gRPC via the same public entry point, TLS disabled.
    let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_addr = grpc_listener.local_addr().unwrap();
    let config = RlsConfig {
        grpc_listen: "127.0.0.1:0".parse().unwrap(),
        admin_listen: "127.0.0.1:0".parse().unwrap(),
        grpc_tls: None,
    };
    let mut server = grpc_server(&config).expect("grpc_server without TLS");
    let svc = RlsService::new(Arc::clone(&policies), counters);
    let incoming = TcpIncoming::from(grpc_listener);
    tokio::spawn(async move {
        server
            .add_service(RateLimitServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    // Plaintext client connects with a small retry loop.
    let url = format!("http://{grpc_addr}");
    let mut client = None;
    for _ in 0..50 {
        match RateLimitServiceClient::connect(url.clone()).await {
            Ok(c) => {
                client = Some(c);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let mut client = client.expect("plaintext client must connect when grpc_tls is None");

    // And enforcement still works end-to-end.
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("http://{admin_addr}/api/v1/admin/rls/policies"))
        .json(&json!({
            "policies": [{
                "domain": "orgA|teamA|plain",
                "descriptors": {"client_id": "bob"},
                "requests_per_unit": 1,
                "unit": "minute"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);

    assert_eq!(check(&mut client, "orgA|teamA|plain").await, OK);
    assert_eq!(check(&mut client, "orgA|teamA|plain").await, OVER);
}
