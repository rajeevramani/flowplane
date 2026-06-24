//! Integration test for GitHub #171 — OIDC validator `ca_bundle_path`.
//!
//! The control-plane OIDC validator fetches the IdP discovery doc + JWKS over
//! HTTPS using a client that normally trusts only the bundled webpki roots.
//! Behind a TLS-intercepting proxy (a server cert signed by a private CA) that
//! fetch fails with `UnknownIssuer`. `OidcConfig::ca_bundle_path` lets an
//! operator add that CA to the trust store so the fetch succeeds. This is
//! additive (does not replace the bundled roots) and fail-closed on a bad
//! bundle.
//!
//! These tests stand up a real local HTTPS IdP whose leaf cert is signed by a
//! throwaway CA, and assert:
//!   1. With `ca_bundle_path = Some(<that CA>)`, a token validates (Ok).
//!   2. With `ca_bundle_path = None`, the very first `validate()` fails because
//!      the JWKS fetch can't complete the TLS handshake (UnknownIssuer) — which
//!      proves the positive case exercised real trust, not a bypass.
//!   3. (bonus) Explicit `jwks_uri = Some(...)` works the same with the bundle.
//!
//! Constitution inv 18 (parallel-safe): every server binds `127.0.0.1:0` and we
//! read back the ephemeral port; temp CA files are uniquely named per process +
//! test tag and cleaned up; no shared global state, no DB, no env vars.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde_json::json;

use fp_core::oidc::{OidcConfig, OidcValidator};

const KID: &str = "test-ca-bundle-kid";
const AUDIENCE: &str = "flowplane-test";

/// Ensure rustls has a process-wide default crypto provider. Idempotent: the
/// first caller wins, everyone else's `install_default` returns Err and we
/// ignore it.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// base64url, no padding — for deriving the JWK `n`/`e` from the RSA key.
fn base64url(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A throwaway RSA signing key plus its matching single-key JWKS document.
struct SigningMaterial {
    encoding: EncodingKey,
    jwks_json: String,
}

impl SigningMaterial {
    fn generate() -> Self {
        let mut rng = rsa::rand_core::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key");
        let public = private.to_public_key();
        let jwks_json = json!({
            "keys": [{
                "kty": "RSA",
                "kid": KID,
                "use": "sig",
                "alg": "RS256",
                "n": base64url(&public.n().to_bytes_be()),
                "e": base64url(&public.e().to_bytes_be()),
            }]
        })
        .to_string();
        let der = private.to_pkcs1_der().expect("pkcs1 der");
        Self {
            encoding: EncodingKey::from_rsa_der(der.as_bytes()),
            jwks_json,
        }
    }

    fn mint(&self, issuer: &str, subject: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": issuer,
            "aud": AUDIENCE,
            "sub": subject,
            "email": "user@example.test",
            "name": "Test User",
            "iat": now,
            "nbf": now - 5,
            "exp": now + 3600,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.to_string());
        encode(&header, &claims, &self.encoding).expect("mint token")
    }
}

/// A self-contained throwaway CA + a leaf cert signed by it carrying SANs for
/// `IP:127.0.0.1` and `DNS:localhost`.
struct TlsMaterial {
    /// PEM of the CA cert — this is what goes into `ca_bundle_path`.
    ca_cert_pem: String,
    /// PEM of the leaf cert chain the server presents.
    leaf_cert_pem: String,
    /// PEM of the leaf private key.
    leaf_key_pem: String,
}

impl TlsMaterial {
    fn generate() -> Self {
        // --- Throwaway CA ---
        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Flowplane Test Throwaway CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");

        // --- Leaf signed by the CA, with SANs for 127.0.0.1 + localhost ---
        let leaf_key = KeyPair::generate().expect("leaf key");
        let mut leaf_params = CertificateParams::new(Vec::<String>::new()).expect("leaf params");
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "127.0.0.1");
        leaf_params.is_ca = IsCa::NoCa;
        leaf_params.subject_alt_names = vec![
            SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
            SanType::DnsName("localhost".try_into().expect("dns san")),
        ];
        leaf_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &ca_cert, &ca_key)
            .expect("leaf signed by ca");

        Self {
            ca_cert_pem: ca_cert.pem(),
            leaf_cert_pem: leaf_cert.pem(),
            leaf_key_pem: leaf_key.serialize_pem(),
        }
    }
}

/// Write the CA PEM to a uniquely-named temp file (process id + tag) and return
/// a guard that deletes it on drop. Parallel-safe per constitution inv 18.
struct TempCaFile {
    path: PathBuf,
}

impl TempCaFile {
    fn new(tag: &str, pem: &str) -> Self {
        let name = format!(
            "fp-oidc-ca-{}-{}-{}.pem",
            std::process::id(),
            tag,
            uuid::Uuid::now_v7().simple()
        );
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, pem).expect("write ca bundle");
        Self { path }
    }
}

impl Drop for TempCaFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone)]
struct AppState {
    discovery_json: Arc<String>,
    jwks_json: Arc<String>,
}

async fn well_known(State(state): State<AppState>) -> ([(&'static str, &'static str); 1], String) {
    (
        [("content-type", "application/json")],
        (*state.discovery_json).clone(),
    )
}

async fn jwks(State(state): State<AppState>) -> ([(&'static str, &'static str); 1], String) {
    (
        [("content-type", "application/json")],
        (*state.jwks_json).clone(),
    )
}

/// A running HTTPS IdP. Holds the spawned server task; aborting it on drop
/// guarantees the listener does not leak across tests.
struct IdpServer {
    issuer: String,
    jwks_uri: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for IdpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Stand up the IdP serving `/.well-known/openid-configuration` and `/jwks`
/// over HTTPS with the given leaf cert/key. Binds `127.0.0.1:0`.
async fn start_idp(tls: &TlsMaterial, jwks_json: String) -> IdpServer {
    // Pre-bind so we can learn the ephemeral port BEFORE building the app — the
    // discovery doc must embed the JWKS URL with the real port.
    let std_listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let local: SocketAddr = std_listener.local_addr().expect("local addr");
    let port = local.port();

    let issuer = format!("https://127.0.0.1:{port}");
    let jwks_uri = format!("{issuer}/jwks");
    let discovery_json = json!({ "jwks_uri": jwks_uri }).to_string();

    let state = AppState {
        discovery_json: Arc::new(discovery_json),
        jwks_json: Arc::new(jwks_json),
    };
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(well_known))
        .route("/jwks", get(jwks))
        .with_state(state);

    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        tls.leaf_cert_pem.clone().into_bytes(),
        tls.leaf_key_pem.clone().into_bytes(),
    )
    .await
    .expect("rustls config from pem");

    let task = tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(std_listener, config)
            .serve(app.into_make_service())
            .await;
    });

    IdpServer {
        issuer,
        jwks_uri,
        task,
    }
}

/// Positive: with the throwaway CA in `ca_bundle_path`, the private-CA IdP is
/// trusted and a token signed by its key validates.
#[tokio::test]
async fn ca_bundle_makes_private_ca_idp_trusted() {
    ensure_crypto_provider();

    let signing = SigningMaterial::generate();
    let tls = TlsMaterial::generate();
    let idp = start_idp(&tls, signing.jwks_json.clone()).await;

    let ca_file = TempCaFile::new("positive", &tls.ca_cert_pem);
    let token = signing.mint(&idp.issuer, "subject-positive");

    let config = OidcConfig {
        issuer: idp.issuer.clone(),
        audience: AUDIENCE.to_string(),
        jwks_uri: None, // force discovery via {issuer}/.well-known/openid-configuration
        ca_bundle_path: Some(ca_file.path.clone()),
    };

    // try_new must succeed: the CA bundle is valid PEM.
    let validator = OidcValidator::try_new(config).expect("validator with valid ca bundle");

    // First validate() triggers the network fetch (discovery + JWKS) honoring
    // the CA bundle; it should succeed and return the expected subject.
    let claims = validator
        .validate(&token)
        .await
        .expect("token validates when CA bundle is trusted");

    assert_eq!(claims.subject, "subject-positive");
    assert_eq!(claims.email.as_deref(), Some("user@example.test"));
}

/// Negative: same private-CA IdP, but NO `ca_bundle_path`. The first
/// `validate()` must error because the JWKS-fetch path's TLS handshake fails
/// (UnknownIssuer — the leaf is signed by a CA not in the bundled webpki roots).
/// With `jwks_uri: None` the first HTTPS call is the discovery fetch, but the
/// handshake failure is the same: only the bundled roots are trusted, so the
/// private CA is rejected. This proves the positive case exercised real trust
/// rather than a bypass.
#[tokio::test]
async fn without_ca_bundle_trust_fails() {
    ensure_crypto_provider();

    let signing = SigningMaterial::generate();
    let tls = TlsMaterial::generate();
    let idp = start_idp(&tls, signing.jwks_json.clone()).await;

    let token = signing.mint(&idp.issuer, "subject-negative");

    let config = OidcConfig {
        issuer: idp.issuer.clone(),
        audience: AUDIENCE.to_string(),
        jwks_uri: None,
        ca_bundle_path: None, // no operator CA => only the bundled roots are trusted
    };

    // Building the validator is fine (no bundle to validate); the failure must
    // surface on the first validate(), which performs the TLS fetch.
    let validator = OidcValidator::try_new(config).expect("validator (no bundle) constructs");

    let result = validator.validate(&token).await;

    // We deliberately do not over-constrain the exact error code/message: the
    // point is that the JWKS fetch could not complete the TLS handshake
    // (UnknownIssuer), so validation fails closed.
    assert!(
        result.is_err(),
        "expected validate() to fail without the CA bundle (TLS UnknownIssuer on JWKS fetch), got: {result:?}"
    );
}

/// Bonus: discovery skipped via explicit `jwks_uri` still honors the CA bundle.
#[tokio::test]
async fn explicit_jwks_uri_with_ca_bundle_validates() {
    ensure_crypto_provider();

    let signing = SigningMaterial::generate();
    let tls = TlsMaterial::generate();
    let idp = start_idp(&tls, signing.jwks_json.clone()).await;

    let ca_file = TempCaFile::new("explicit", &tls.ca_cert_pem);
    let token = signing.mint(&idp.issuer, "subject-explicit");

    let config = OidcConfig {
        issuer: idp.issuer.clone(),
        audience: AUDIENCE.to_string(),
        jwks_uri: Some(idp.jwks_uri.clone()), // skip discovery, hit JWKS directly
        ca_bundle_path: Some(ca_file.path.clone()),
    };

    let validator = OidcValidator::try_new(config).expect("validator with valid ca bundle");
    let claims = validator
        .validate(&token)
        .await
        .expect("token validates via explicit jwks_uri with CA bundle");

    assert_eq!(claims.subject, "subject-explicit");
}
