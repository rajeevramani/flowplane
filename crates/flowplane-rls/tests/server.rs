//! Black-box integration tests for `flowplane-rls`.
//!
//! These tests treat the crate as opaque: they spin up the *real* gRPC
//! `RateLimitService` and the *real* HTTP admin server, then drive both over the
//! wire (tonic gRPC client + reqwest HTTP client). Nothing here mirrors the
//! crate's internal enforcement logic — it only asserts the observable contract.
//!
//! Adversarial intent: each test is shaped to expose a plausible bug class
//! (off-by-one in the window, tenant counter bleed, descriptor-order
//! non-canonicalization, stale policy after a snapshot replace, accidental
//! counting of unmatched descriptors), not merely to go green.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
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
use envoy_types::pb::google::protobuf::UInt64Value;
use serde_json::json;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity, Server};

use flowplane_rls::admin::{router, AdminState};
use flowplane_rls::config::{AdminCredential, RlsConfig, RlsGrpcTls};
use flowplane_rls::counter::InMemoryFixedWindow;
use flowplane_rls::grpc::{GrpcAuthMode, RlsService};
use flowplane_rls::policy::PolicyCache;
use flowplane_rls::server::grpc_server;

const OK: i32 = Code::Ok as i32;
const OVER: i32 = Code::OverLimit as i32;

fn unique_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

fn openssl(dir: &Path, args: &[&str]) {
    let out = Command::new("openssl")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("openssl CLI must be available for RLS mTLS tests");
    assert!(
        out.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct TestPki {
    dir: PathBuf,
}

impl TestPki {
    fn new() -> Self {
        let dir = unique_dir("fp-rls-mtls");
        std::fs::create_dir_all(&dir).expect("mkdir pki dir");
        openssl(
            &dir,
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-keyout",
                "ca.key",
                "-out",
                "ca.crt",
                "-days",
                "2",
                "-nodes",
                "-subj",
                "/CN=fp-rls-test-ca",
            ],
        );
        std::fs::write(
            dir.join("server-san.cnf"),
            "subjectAltName=DNS:localhost,IP:127.0.0.1\n",
        )
        .expect("write server san");
        openssl(
            &dir,
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-nodes",
                "-subj",
                "/CN=flowplane-rls",
            ],
        );
        openssl(
            &dir,
            &[
                "pkcs8",
                "-topk8",
                "-nocrypt",
                "-in",
                "server.key",
                "-out",
                "server.pk8.key",
            ],
        );
        openssl(
            &dir,
            &[
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                "server.crt",
                "-days",
                "2",
                "-extfile",
                "server-san.cnf",
            ],
        );
        Self { dir }
    }

    fn tls_paths(&self) -> RlsGrpcTls {
        RlsGrpcTls {
            cert_path: self.dir.join("server.crt"),
            key_path: self.dir.join("server.pk8.key"),
            client_ca_path: self.dir.join("ca.crt"),
        }
    }

    fn ca_pem(&self) -> Vec<u8> {
        std::fs::read(self.dir.join("ca.crt")).expect("ca pem")
    }

    fn client_identity(&self, name: &str) -> Identity {
        openssl(
            &self.dir,
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-keyout",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.csr"),
                "-nodes",
                "-subj",
                &format!("/CN={name}"),
            ],
        );
        openssl(
            &self.dir,
            &[
                "pkcs8",
                "-topk8",
                "-nocrypt",
                "-in",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.pk8.key"),
            ],
        );
        openssl(
            &self.dir,
            &[
                "x509",
                "-req",
                "-in",
                &format!("{name}.csr"),
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                &format!("{name}.crt"),
                "-days",
                "2",
            ],
        );
        Identity::from_pem(
            std::fs::read(self.dir.join(format!("{name}.crt"))).expect("client crt"),
            std::fs::read(self.dir.join(format!("{name}.pk8.key"))).expect("client key"),
        )
    }
}

/// A live RLS under test: the admin HTTP base URL and a connected gRPC client.
/// Both servers share the *same* `Arc<PolicyCache>` so a policy push over HTTP is
/// immediately visible to the gRPC enforcement path.
struct Harness {
    admin_base: String,
    grpc: RateLimitServiceClient<Channel>,
    http: reqwest::Client,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with_credential(None).await
    }

    async fn start_with_credential(credential: Option<AdminCredential>) -> Self {
        let policies = Arc::new(PolicyCache::new());
        let counters = Arc::new(InMemoryFixedWindow::new());

        // --- Admin HTTP server -------------------------------------------------
        let admin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin_addr = admin_listener.local_addr().unwrap();
        let state = AdminState {
            policies: Arc::clone(&policies),
            credential,
        };
        tokio::spawn(async move {
            axum::serve(admin_listener, router(state)).await.unwrap();
        });

        // --- gRPC server -------------------------------------------------------
        let svc = RlsService::new(
            Arc::clone(&policies),
            counters,
            GrpcAuthMode::InsecureDevOnly,
        );
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

        // Give both servers a moment to begin listening, then connect (with a
        // small retry loop so the test isn't flaky on a slow scheduler).
        tokio::time::sleep(Duration::from_millis(100)).await;
        let grpc = connect_with_retry(grpc_addr).await;

        Harness {
            admin_base: format!("http://{admin_addr}"),
            grpc,
            http: reqwest::Client::new(),
        }
    }

    /// Push a full policy snapshot. Asserts the documented `204 No Content`.
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

    async fn push_with_bearer(&self, body: serde_json::Value, token: &str) -> reqwest::StatusCode {
        self.http
            .post(format!("{}/api/v1/admin/rls/policies", self.admin_base))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
    }

    /// Issue one ShouldRateLimit call and return the overall code.
    async fn check(&mut self, domain: &str, entries: Vec<(&str, &str)>) -> i32 {
        let req = RateLimitRequest {
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
        };
        self.grpc
            .should_rate_limit(req)
            .await
            .unwrap()
            .into_inner()
            .overall_code
    }

    /// Issue one ShouldRateLimit call with a fully-specified single descriptor:
    /// caller controls the raw entry list (so duplicate keys can be sent) and the
    /// per-descriptor `hits_addend`, plus the request-level `hits_addend`. Returns
    /// the overall code.
    async fn check_raw(
        &mut self,
        domain: &str,
        entries: Vec<(&str, &str)>,
        descriptor_addend: Option<u64>,
        request_addend: u32,
    ) -> i32 {
        let req = RateLimitRequest {
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
                hits_addend: descriptor_addend.map(|value| UInt64Value { value }),
            }],
            hits_addend: request_addend,
        };
        self.grpc
            .should_rate_limit(req)
            .await
            .unwrap()
            .into_inner()
            .overall_code
    }
}

#[tokio::test]
async fn admin_policy_push_requires_credential_and_preserves_cache_on_401() {
    let mut h = Harness::start_with_credential(Some(
        AdminCredential::new("configured-token".to_string()).unwrap(),
    ))
    .await;

    let original = json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    });
    assert_eq!(
        h.push_with_bearer(original, "configured-token").await,
        reqwest::StatusCode::NO_CONTENT
    );
    assert_eq!(
        h.check("orgA|teamA|checkout", vec![("client_id", "bob")])
            .await,
        OK
    );
    assert_eq!(
        h.check("orgA|teamA|checkout", vec![("client_id", "bob")])
            .await,
        OVER
    );

    let replacement = json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "alice"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    });
    let resp = h
        .http
        .post(format!("{}/api/v1/admin/rls/policies", h.admin_base))
        .json(&replacement)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    assert_eq!(
        h.check("orgA|teamA|checkout", vec![("client_id", "alice")])
            .await,
        OK,
        "unauthorized replacement must not add the alice policy"
    );
    assert_eq!(
        h.check("orgA|teamA|checkout", vec![("client_id", "bob")])
            .await,
        OVER,
        "unauthorized replacement must not remove the original bob policy"
    );

    assert_eq!(
        h.push_with_bearer(replacement, "configured-token").await,
        reqwest::StatusCode::NO_CONTENT
    );
    assert_eq!(
        h.check("orgA|teamA|checkout", vec![("client_id", "alice")])
            .await,
        OK,
        "authorized replacement applies the new policy"
    );
    assert_eq!(
        h.check("orgA|teamA|checkout", vec![("client_id", "alice")])
            .await,
        OVER,
        "authorized replacement enforces the new policy"
    );
}

#[test]
fn server_config_refuses_insecure_production_grpc_even_with_port_zero() {
    let err = RlsConfig::resolve(
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
        None,
        None,
        true,
        false,
    )
    .expect_err("plaintext gRPC without the explicit dev gate must fail closed");

    assert!(err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"));
}

#[test]
fn server_config_allows_explicit_dev_insecure_grpc_on_loopback_port_zero() {
    let cfg = RlsConfig::resolve(
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
        None,
        None,
        true,
        true,
    )
    .expect("explicit local insecure gRPC is allowed for dev");

    assert_eq!(cfg.grpc_listen.port(), 0);
    assert!(cfg.grpc_listen.ip().is_loopback());
    assert!(cfg.allow_insecure_grpc);
}

#[tokio::test]
async fn grpc_server_requires_client_certificate_when_tls_triad_is_present() {
    let pki = TestPki::new();
    let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_addr = grpc_listener.local_addr().unwrap();
    let incoming = TcpIncoming::from(grpc_listener);
    let policies = Arc::new(PolicyCache::new());
    let counters = Arc::new(InMemoryFixedWindow::new());
    let svc = RlsService::new(
        Arc::clone(&policies),
        counters,
        GrpcAuthMode::InsecureDevOnly,
    );
    let config = RlsConfig::resolve(
        grpc_addr,
        "127.0.0.1:0".parse().unwrap(),
        None,
        Some(pki.tls_paths()),
        true,
        false,
    )
    .expect("TLS triad permits non-plaintext gRPC serving");
    let server = tokio::spawn(async move {
        grpc_server(&config)
            .expect("TLS server builder")
            .add_service(RateLimitServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let tls_without_client_cert = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(pki.ca_pem()))
        .domain_name("localhost");
    let no_cert_channel = Channel::from_shared(format!("https://{grpc_addr}"))
        .unwrap()
        .tls_config(tls_without_client_cert)
        .unwrap()
        .connect()
        .await;
    let rejected_without_client_cert = match no_cert_channel {
        Err(_) => true,
        Ok(channel) => RateLimitServiceClient::new(channel)
            .should_rate_limit(RateLimitRequest {
                domain: "org|team|checkout".to_string(),
                descriptors: Vec::new(),
                hits_addend: 0,
            })
            .await
            .is_err(),
    };
    assert!(
        rejected_without_client_cert,
        "server TLS must reject clients without a certificate"
    );

    let tls_with_client_cert = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(pki.ca_pem()))
        .identity(pki.client_identity("envoy"))
        .domain_name("localhost");
    let channel = Channel::from_shared(format!("https://{grpc_addr}"))
        .unwrap()
        .tls_config(tls_with_client_cert)
        .unwrap()
        .connect()
        .await
        .expect("client certificate should satisfy mTLS");
    let response = RateLimitServiceClient::new(channel)
        .should_rate_limit(RateLimitRequest {
            domain: "org|team|checkout".to_string(),
            descriptors: Vec::new(),
            hits_addend: 0,
        })
        .await
        .expect("mTLS client can call RLS")
        .into_inner();
    assert_eq!(response.overall_code, OK);
    server.abort();
}

async fn connect_with_retry(addr: std::net::SocketAddr) -> RateLimitServiceClient<Channel> {
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
    panic!("could not connect to gRPC server at {url}: {last_err:?}");
}

/// Acceptance #1 — end-to-end enforcement / counter keying.
/// rpu=2 (minute): first two calls OK, third OVER_LIMIT. Catches both an
/// off-by-one that limits too early and one that never limits.
#[tokio::test]
async fn enforces_limit_end_to_end() {
    let mut h = Harness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 2,
            "unit": "minute"
        }]
    }))
    .await;

    let d = "orgA|teamA|checkout";
    assert_eq!(
        h.check(d, vec![("client_id", "bob")]).await,
        OK,
        "1st request must be OK"
    );
    assert_eq!(
        h.check(d, vec![("client_id", "bob")]).await,
        OK,
        "2nd request must be OK"
    );
    assert_eq!(
        h.check(d, vec![("client_id", "bob")]).await,
        OVER,
        "3rd request must be OVER_LIMIT (rpu=2)"
    );
}

/// Acceptance #2 — tenant isolation over the wire.
/// Only `orgA|teamA|checkout` is pushed. The *same* descriptor under a different
/// domain namespace (`orgA|teamB|checkout`) shares no policy and no counter, so
/// it must never be limited regardless of how many times it is called.
#[tokio::test]
async fn tenant_namespaces_never_share_a_counter() {
    let mut h = Harness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 2,
            "unit": "minute"
        }]
    }))
    .await;

    // Hammer the *unconfigured* sibling namespace well past teamA's limit.
    let other = "orgA|teamB|checkout";
    for i in 1..=6 {
        assert_eq!(
            h.check(other, vec![("client_id", "bob")]).await,
            OK,
            "request #{i} to an unconfigured namespace must stay OK (no shared counter)"
        );
    }
}

/// Acceptance #3 — unmatched descriptor is never counted.
/// The domain has a policy, but the descriptor key/value does not match it, so
/// every call is OK even past the configured rpu.
#[tokio::test]
async fn unmatched_descriptor_is_not_limited() {
    let mut h = Harness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;

    let d = "orgA|teamA|checkout";
    // Wrong value for the right key.
    for i in 1..=4 {
        assert_eq!(
            h.check(d, vec![("client_id", "alice")]).await,
            OK,
            "non-matching descriptor value (#{i}) must not be counted"
        );
    }
    // Wrong key entirely.
    for i in 1..=4 {
        assert_eq!(
            h.check(d, vec![("user", "bob")]).await,
            OK,
            "non-matching descriptor key (#{i}) must not be counted"
        );
    }
}

/// Acceptance #4 — push replaces the set (full-snapshot semantics).
/// Snapshot A configures policy P; snapshot B omits P and configures Q. After B,
/// P's domain+descriptor must no longer be enforced (stays OK past P's old
/// limit), while Q is enforced. Catches a merge-instead-of-replace bug.
#[tokio::test]
async fn push_replaces_previous_snapshot() {
    let mut h = Harness::start().await;

    // Snapshot A: policy P on checkout, rpu=1.
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;
    // Sanity: P enforces before replacement.
    let p = "orgA|teamA|checkout";
    assert_eq!(h.check(p, vec![("client_id", "bob")]).await, OK);
    assert_eq!(
        h.check(p, vec![("client_id", "bob")]).await,
        OVER,
        "P should enforce under snapshot A (rpu=1)"
    );

    // Snapshot B: omits P entirely, introduces Q on a different domain, rpu=1.
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|payments",
            "descriptors": {"client_id": "carol"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;

    // P is gone: even a fresh descriptor value under P's domain is unenforced.
    for i in 1..=4 {
        assert_eq!(
            h.check(p, vec![("client_id", "bob")]).await,
            OK,
            "after snapshot B, removed policy P must not be enforced (call #{i})"
        );
    }

    // Q now enforces.
    let q = "orgA|teamA|payments";
    assert_eq!(
        h.check(q, vec![("client_id", "carol")]).await,
        OK,
        "Q 1st call OK"
    );
    assert_eq!(
        h.check(q, vec![("client_id", "carol")]).await,
        OVER,
        "Q must enforce under snapshot B (rpu=1)"
    );
}

/// Acceptance #5 — health endpoints.
#[tokio::test]
async fn health_endpoints_return_200() {
    let h = Harness::start().await;
    for path in ["healthz", "readyz"] {
        let resp = h
            .http
            .get(format!("{}/{path}", h.admin_base))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "GET /{path} must be 200"
        );
    }
}

/// Acceptance #6 — descriptor entry order is canonicalized.
/// A 2-entry descriptor sent in two different entry orders must hit the same
/// counter. With rpu=1: first call (one order) OK, second call (reversed order)
/// OVER_LIMIT. Catches a counter key that is order-sensitive.
#[tokio::test]
async fn descriptor_entry_order_hits_same_counter() {
    let mut h = Harness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|checkout",
            "descriptors": {"client_id": "bob", "path": "/buy"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;

    let d = "orgA|teamA|checkout";
    assert_eq!(
        h.check(d, vec![("client_id", "bob"), ("path", "/buy")])
            .await,
        OK,
        "1st call (order A) must be OK"
    );
    assert_eq!(
        h.check(d, vec![("path", "/buy"), ("client_id", "bob")])
            .await,
        OVER,
        "2nd call with reversed entry order must hit the same counter -> OVER_LIMIT"
    );
}

/// Per-descriptor `hits_addend` overrides the request-level one.
/// Policy rpu=4. A single request whose descriptor carries `hits_addend = 5`
/// while the request-level `hits_addend = 0` must trip OVER_LIMIT on the FIRST
/// call (5 > 4). This is only true if the descriptor-level addend is used; an
/// impl that wrongly used the request-level value (0 -> default 1 hit) would
/// return OK, so the first-call OVER_LIMIT is the discriminating assertion.
#[tokio::test]
async fn descriptor_hits_addend_overrides_request_level() {
    let mut h = Harness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|d",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 4,
            "unit": "minute"
        }]
    }))
    .await;

    let code = h
        .check_raw("orgA|teamA|d", vec![("client_id", "bob")], Some(5), 0)
        .await;
    assert_eq!(
        code, OVER,
        "single call adding 5 hits (descriptor-level addend) must exceed rpu=4 on the first call"
    );
}

/// A descriptor with a duplicate key is ambiguous and matches no policy.
/// Policy is keyed on {"client_id":"bob"}, rpu=1. Sending two entries that share
/// the key "client_id" must NOT collapse into a single matching descriptor, so
/// the request stays OK no matter how many times it is repeated — both when the
/// values differ ([alice, bob]) and when they are identical ([bob, bob]). A
/// silent de-dup/collapse to {"client_id":"bob"} would trip the limit.
#[tokio::test]
async fn duplicate_descriptor_key_matches_nothing() {
    let mut h = Harness::start().await;
    h.push(json!({
        "policies": [{
            "domain": "orgA|teamA|d",
            "descriptors": {"client_id": "bob"},
            "requests_per_unit": 1,
            "unit": "minute"
        }]
    }))
    .await;

    let d = "orgA|teamA|d";
    // Duplicate key, differing values — must not collapse to {"client_id":"bob"}.
    for i in 1..=3 {
        assert_eq!(
            h.check_raw(
                d,
                vec![("client_id", "alice"), ("client_id", "bob")],
                None,
                0
            )
            .await,
            OK,
            "duplicate key with differing values (call #{i}) must match nothing and stay OK"
        );
    }
    // Duplicate key, identical values — likewise ambiguous, matches nothing.
    for i in 1..=3 {
        assert_eq!(
            h.check_raw(d, vec![("client_id", "bob"), ("client_id", "bob")], None, 0)
                .await,
            OK,
            "duplicate key with identical values (call #{i}) must match nothing and stay OK"
        );
    }
}
