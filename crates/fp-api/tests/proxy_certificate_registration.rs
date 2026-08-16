//! RED integration contract for fpv2-7f3.2 external certificate registration.
//!
//! The system under test is the public REST router backed by real PostgreSQL. Certificate
//! fixtures are real X.509 chains rooted in the configured xDS client trust bundle.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::{hash, MessageDigest};
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::x509::extension::{
    AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
    SubjectKeyIdentifier,
};
use openssl::x509::{X509NameBuilder, X509};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

struct EnvRestore {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set_path(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        std::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[derive(Clone)]
struct TestPki {
    root: X509,
    intermediate: X509,
    intermediate_key: PKey<Private>,
}

struct LeafFixture {
    certificate: X509,
    private_key: PKey<Private>,
    chain_pem: String,
    not_before_unix: i64,
    not_after_unix: i64,
}

fn name(common_name: &str) -> openssl::x509::X509Name {
    let mut builder = X509NameBuilder::new().expect("X.509 name builder");
    builder
        .append_entry_by_text("CN", common_name)
        .expect("X.509 common name");
    builder.build()
}

fn serial(hex: &str) -> openssl::asn1::Asn1Integer {
    BigNum::from_hex_str(hex)
        .and_then(|value| value.to_asn1_integer())
        .expect("X.509 serial")
}

fn build_root(key: &PKey<Private>, issuer_for_key_id: Option<&X509>) -> X509 {
    let mut builder = X509::builder().expect("root builder");
    builder.set_version(2).expect("root version");
    builder
        .set_serial_number(&serial("01"))
        .expect("root serial");
    let root_name = name("fpv2-7f3.2 test root");
    builder.set_subject_name(&root_name).expect("root subject");
    builder.set_issuer_name(&root_name).expect("root issuer");
    builder.set_pubkey(key).expect("root public key");
    builder
        .set_not_before(&Asn1Time::days_from_now(0).expect("root not-before"))
        .expect("set root not-before");
    builder
        .set_not_after(&Asn1Time::days_from_now(2).expect("root not-after"))
        .expect("set root not-after");
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .ca()
                .build()
                .expect("root basic constraints"),
        )
        .expect("append root basic constraints");
    builder
        .append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .expect("root key usage"),
        )
        .expect("append root key usage");
    let ski = SubjectKeyIdentifier::new()
        .build(&builder.x509v3_context(None, None))
        .expect("root SKI");
    builder.append_extension(ski).expect("append root SKI");
    if let Some(issuer) = issuer_for_key_id {
        let aki = AuthorityKeyIdentifier::new()
            .keyid(true)
            .build(&builder.x509v3_context(Some(issuer), None))
            .expect("root AKI");
        builder.append_extension(aki).expect("append root AKI");
    }
    builder
        .sign(key, MessageDigest::sha256())
        .expect("sign root");
    builder.build()
}

fn test_pki(label: &str) -> TestPki {
    let root_key = PKey::from_rsa(Rsa::generate(2048).expect("root RSA")).expect("root key");
    let root_template = build_root(&root_key, None);
    let root = build_root(&root_key, Some(&root_template));

    let intermediate_key =
        PKey::from_rsa(Rsa::generate(2048).expect("intermediate RSA")).expect("intermediate key");
    let mut builder = X509::builder().expect("intermediate builder");
    builder.set_version(2).expect("intermediate version");
    builder
        .set_serial_number(&serial("02"))
        .expect("intermediate serial");
    builder
        .set_subject_name(&name(&format!("{label} intermediate")))
        .expect("intermediate subject");
    builder
        .set_issuer_name(root.subject_name())
        .expect("intermediate issuer");
    builder
        .set_pubkey(&intermediate_key)
        .expect("intermediate public key");
    builder
        .set_not_before(&Asn1Time::days_from_now(0).expect("intermediate not-before"))
        .expect("set intermediate not-before");
    builder
        .set_not_after(&Asn1Time::days_from_now(2).expect("intermediate not-after"))
        .expect("set intermediate not-after");
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .ca()
                .pathlen(0)
                .build()
                .expect("intermediate basic constraints"),
        )
        .expect("append intermediate basic constraints");
    builder
        .append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .expect("intermediate key usage"),
        )
        .expect("append intermediate key usage");
    let ski = SubjectKeyIdentifier::new()
        .build(&builder.x509v3_context(Some(&root), None))
        .expect("intermediate SKI");
    builder
        .append_extension(ski)
        .expect("append intermediate SKI");
    let aki = AuthorityKeyIdentifier::new()
        .keyid(true)
        .build(&builder.x509v3_context(Some(&root), None))
        .expect("intermediate AKI");
    builder
        .append_extension(aki)
        .expect("append intermediate AKI");
    builder
        .sign(&root_key, MessageDigest::sha256())
        .expect("sign intermediate");

    TestPki {
        root,
        intermediate: builder.build(),
        intermediate_key,
    }
}

fn leaf_fixture(
    pki: &TestPki,
    spiffe_uri: &str,
    client_auth: bool,
    serial_hex: &str,
) -> LeafFixture {
    let key = PKey::from_rsa(Rsa::generate(2048).expect("leaf RSA")).expect("leaf key");
    let now = chrono::Utc::now().timestamp();
    let not_before_unix = now - 60;
    let not_after_unix = now + 3600;
    let mut builder = X509::builder().expect("leaf builder");
    builder.set_version(2).expect("leaf version");
    builder
        .set_serial_number(&serial(serial_hex))
        .expect("leaf serial");
    builder
        .set_subject_name(&name("Flowplane external dataplane"))
        .expect("leaf subject");
    builder
        .set_issuer_name(pki.intermediate.subject_name())
        .expect("leaf issuer");
    builder.set_pubkey(&key).expect("leaf public key");
    builder
        .set_not_before(&Asn1Time::from_unix(not_before_unix).expect("leaf not-before"))
        .expect("set leaf not-before");
    builder
        .set_not_after(&Asn1Time::from_unix(not_after_unix).expect("leaf not-after"))
        .expect("set leaf not-after");
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .build()
                .expect("leaf basic constraints"),
        )
        .expect("append leaf basic constraints");
    builder
        .append_extension(
            KeyUsage::new()
                .critical()
                .digital_signature()
                .build()
                .expect("leaf key usage"),
        )
        .expect("append leaf key usage");
    let eku = if client_auth {
        ExtendedKeyUsage::new().client_auth().build()
    } else {
        ExtendedKeyUsage::new().server_auth().build()
    }
    .expect("leaf EKU");
    builder.append_extension(eku).expect("append leaf EKU");
    let san = SubjectAlternativeName::new()
        .uri(spiffe_uri)
        .build(&builder.x509v3_context(Some(&pki.intermediate), None))
        .expect("leaf SPIFFE URI SAN");
    builder.append_extension(san).expect("append leaf SAN");
    let ski = SubjectKeyIdentifier::new()
        .build(&builder.x509v3_context(Some(&pki.intermediate), None))
        .expect("leaf SKI");
    builder.append_extension(ski).expect("append leaf SKI");
    let aki = AuthorityKeyIdentifier::new()
        .keyid(true)
        .build(&builder.x509v3_context(Some(&pki.intermediate), None))
        .expect("leaf AKI");
    builder.append_extension(aki).expect("append leaf AKI");
    builder
        .sign(&pki.intermediate_key, MessageDigest::sha256())
        .expect("sign leaf");
    let certificate = builder.build();
    let chain_pem = format!(
        "{}{}",
        String::from_utf8(certificate.to_pem().expect("leaf PEM")).expect("leaf PEM UTF-8"),
        String::from_utf8(pki.intermediate.to_pem().expect("intermediate PEM"))
            .expect("intermediate PEM UTF-8")
    );
    LeafFixture {
        certificate,
        private_key: key,
        chain_pem,
        not_before_unix,
        not_after_unix,
    }
}

fn write_root(root: &X509) -> PathBuf {
    let directory = std::env::temp_dir().join(unique("flowplane-registration-root"));
    std::fs::create_dir_all(&directory).expect("trust-root directory");
    let path = directory.join("xds-client-ca.pem");
    std::fs::write(&path, root.to_pem().expect("root PEM")).expect("write trust root");
    path
}

fn app(pool: sqlx::PgPool, validator: Arc<fp_core::OidcValidator>) -> axum::Router {
    fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(validator),
        write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        xds_degraded: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    })
}

fn request(
    token: &str,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |json| Body::from(json.to_string())))
        .expect("HTTP request")
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

async fn assert_rejected(response: axum::response::Response, label: &str) {
    let status = response.status();
    let body = json(response).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "{label} must fail closed, got {status}: {body}"
    );
    assert_ne!(status, StatusCode::UNAUTHORIZED, "{label}: {body}");
}

#[test]
fn external_registration_openapi_requires_certificate_chain_pem() {
    let document = serde_json::to_value(fp_api::routes::openapi_document()).expect("OpenAPI JSON");
    let operation = &document["paths"]["/api/v1/teams/{team}/proxy-certificates"]["post"];
    let schema_ref = operation["requestBody"]["content"]["application/json"]["schema"]["$ref"]
        .as_str()
        .expect("registration request schema reference");
    let schema_name = schema_ref
        .strip_prefix("#/components/schemas/")
        .expect("local registration schema reference");
    let schema = &document["components"]["schemas"][schema_name];
    let required = schema["required"].as_array().expect("required fields");
    assert!(
        required
            .iter()
            .any(|field| field == "certificate_chain_pem"),
        "external registration must require certificate_chain_pem: {schema}"
    );
    for caller_asserted in ["spiffe_uri", "serial_number", "expires_at"] {
        assert!(
            schema["properties"][caller_asserted].is_null(),
            "external registration must not accept caller-asserted {caller_asserted}: {schema}"
        );
    }
}

#[tokio::test]
async fn external_registration_verifies_chain_derives_metadata_and_fails_closed() {
    let Ok(database_url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&database_url, 6)
        .await
        .expect("connect to real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");

    let issuer = DevIssuer::generate().expect("OIDC issuer");
    let validator = Arc::new(fp_core::OidcValidator::new(issuer.oidc_config()));
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("load JWKS");
    let owner_subject = unique("registration-owner");
    let owner_token = issuer
        .mint(
            &owner_subject,
            "registration-owner@test",
            "Registration Owner",
            600,
        )
        .expect("owner token");
    let outsider_subject = unique("registration-outsider");
    let outsider_token = issuer
        .mint(
            &outsider_subject,
            "registration-outsider@test",
            "Registration Outsider",
            600,
        )
        .expect("outsider token");

    let owner_org = identity::create_org(&pool, &unique("registration-org"), "")
        .await
        .expect("owner org");
    let owner_team = identity::create_team(&pool, owner_org.id, &unique("registration-team"), "")
        .await
        .expect("owner team");
    let owner_user = identity::upsert_user_by_subject(
        &pool,
        &owner_subject,
        "registration-owner@test",
        "Registration Owner",
    )
    .await
    .expect("owner user");
    identity::add_org_membership(&pool, owner_user, owner_org.id, OrgRole::Admin)
        .await
        .expect("owner membership");

    let outsider_org = identity::create_org(&pool, &unique("outsider-org"), "")
        .await
        .expect("outsider org");
    let outsider_user = identity::upsert_user_by_subject(
        &pool,
        &outsider_subject,
        "registration-outsider@test",
        "Registration Outsider",
    )
    .await
    .expect("outsider user");
    identity::add_org_membership(&pool, outsider_user, outsider_org.id, OrgRole::Admin)
        .await
        .expect("outsider membership");

    // Construct the first router while xDS client trust is absent; production configuration is
    // process-startup state, not something registration may discover lazily after a request.
    let unconfigured_trust = EnvRestore::remove("FLOWPLANE_XDS_TLS_CLIENT_CA");
    let unconfigured_app = app(pool.clone(), validator.clone());
    let dataplane_name = unique("external-dataplane");
    let dataplanes_path = format!("/api/v1/teams/{}/dataplanes", owner_team.name);
    let response = unconfigured_app
        .clone()
        .oneshot(request(
            &owner_token,
            "POST",
            &dataplanes_path,
            Some(serde_json::json!({
                "name": dataplane_name,
                "description": "external certificate registration"
            })),
        ))
        .await
        .expect("create dataplane response");
    let status = response.status();
    let dataplane_body = json(response).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create dataplane: {dataplane_body}"
    );
    let dataplane_id = dataplane_body["id"]
        .as_str()
        .expect("dataplane UUID")
        .to_owned();
    let spiffe_uri = format!(
        "spiffe://flowplane.test/org/{}/team/{}/proxy/{dataplane_id}",
        owner_org.id.as_uuid(),
        owner_team.id.as_uuid()
    );
    let registration_path = format!("/api/v1/teams/{}/proxy-certificates", owner_team.name);

    let trusted_pki = test_pki("trusted");
    let trust_root_path = write_root(&trusted_pki.root);
    let trusted_leaf = leaf_fixture(&trusted_pki, &spiffe_uri, true, "000A");
    let expected_fingerprint = hash(
        MessageDigest::sha256(),
        &trusted_leaf.certificate.to_der().expect("leaf DER"),
    )
    .expect("SHA-256 leaf fingerprint")
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect::<String>();

    // Registration must never fall back to plaintext/dev trust when the xDS client root is absent.
    let response = unconfigured_app
        .oneshot(request(
            &owner_token,
            "POST",
            &registration_path,
            Some(serde_json::json!({
                "dataplane": dataplane_name,
                "certificate_chain_pem": trusted_leaf.chain_pem
            })),
        ))
        .await
        .expect("unconfigured-trust response");
    assert_rejected(response, "unconfigured xDS client trust").await;
    drop(unconfigured_trust);

    let _configured = EnvRestore::set_path("FLOWPLANE_XDS_TLS_CLIENT_CA", &trust_root_path);
    let app = app(pool.clone(), validator);
    let response = app
        .clone()
        .oneshot(request(
            &owner_token,
            "POST",
            &registration_path,
            Some(serde_json::json!({
                "dataplane": dataplane_name,
                "certificate_chain_pem": trusted_leaf.chain_pem
            })),
        ))
        .await
        .expect("valid registration response");
    let status = response.status();
    let registered = json(response).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid registration: {registered}"
    );
    assert_eq!(registered["spiffe_uri"], spiffe_uri);
    assert_eq!(registered["fingerprint_sha256"], expected_fingerprint);
    assert_eq!(registered["serial_number"], "a");
    let issued_at = chrono::DateTime::parse_from_rfc3339(
        registered["issued_at"]
            .as_str()
            .expect("derived not-before"),
    )
    .expect("issued_at RFC 3339");
    let expires_at = chrono::DateTime::parse_from_rfc3339(
        registered["expires_at"]
            .as_str()
            .expect("derived not-after"),
    )
    .expect("expires_at RFC 3339");
    assert_eq!(issued_at.timestamp(), trusted_leaf.not_before_unix);
    assert_eq!(expires_at.timestamp(), trusted_leaf.not_after_unix);

    // Exercise external-registration serialization independently on a fresh dataplane. One active
    // exact row exists before two replacement chains race for the single remaining overlap slot.
    let race_dataplane_name = unique("external-race-dataplane");
    let response = app
        .clone()
        .oneshot(request(
            &owner_token,
            "POST",
            &dataplanes_path,
            Some(serde_json::json!({
                "name": race_dataplane_name,
                "description": "external registration overlap race"
            })),
        ))
        .await
        .expect("create registration-race dataplane");
    let race_status = response.status();
    let race_dataplane = json(response).await;
    assert_eq!(
        race_status,
        StatusCode::CREATED,
        "create registration-race dataplane: {race_dataplane}"
    );
    let race_dataplane_id = race_dataplane["id"]
        .as_str()
        .expect("registration-race dataplane UUID");
    let race_spiffe_uri = format!(
        "spiffe://flowplane.test/org/{}/team/{}/proxy/{race_dataplane_id}",
        owner_org.id.as_uuid(),
        owner_team.id.as_uuid()
    );
    let race_initial = leaf_fixture(&trusted_pki, &race_spiffe_uri, true, "1A");
    let race_left = leaf_fixture(&trusted_pki, &race_spiffe_uri, true, "1B");
    let race_right = leaf_fixture(&trusted_pki, &race_spiffe_uri, true, "1C");
    let response = app
        .clone()
        .oneshot(request(
            &owner_token,
            "POST",
            &registration_path,
            Some(serde_json::json!({
                "dataplane": race_dataplane_name,
                "certificate_chain_pem": race_initial.chain_pem
            })),
        ))
        .await
        .expect("register race baseline");
    assert_eq!(response.status(), StatusCode::CREATED);

    let (left, right) = tokio::join!(
        app.clone().oneshot(request(
            &owner_token,
            "POST",
            &registration_path,
            Some(serde_json::json!({
                "dataplane": race_dataplane_name,
                "certificate_chain_pem": race_left.chain_pem
            })),
        )),
        app.clone().oneshot(request(
            &owner_token,
            "POST",
            &registration_path,
            Some(serde_json::json!({
                "dataplane": race_dataplane_name,
                "certificate_chain_pem": race_right.chain_pem
            })),
        ))
    );
    let left = left.expect("left concurrent registration");
    let right = right.expect("right concurrent registration");
    let statuses = [left.status(), right.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1,
        "exactly one concurrent external registration may claim the second slot: {statuses:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1,
        "the losing concurrent registration must map stably to conflict: {statuses:?}"
    );

    let malformed = "-----BEGIN CERTIFICATE-----\nnot base64 DER\n-----END CERTIFICATE-----\n";
    let private_key_pem = String::from_utf8(
        trusted_leaf
            .private_key
            .private_key_to_pem_pkcs8()
            .expect("private key PEM"),
    )
    .expect("private key PEM UTF-8");
    let second_leaf = leaf_fixture(&trusted_pki, &spiffe_uri, true, "0B");
    let ambiguous = format!(
        "{}{}{}",
        trusted_leaf
            .certificate
            .to_pem()
            .map(String::from_utf8)
            .expect("first leaf PEM")
            .expect("first leaf UTF-8"),
        second_leaf
            .certificate
            .to_pem()
            .map(String::from_utf8)
            .expect("second leaf PEM")
            .expect("second leaf UTF-8"),
        String::from_utf8(trusted_pki.intermediate.to_pem().expect("intermediate PEM"))
            .expect("intermediate UTF-8")
    );
    let wrong_profile = leaf_fixture(&trusted_pki, &spiffe_uri, false, "0C");
    let wrong_dataplane_uri = format!(
        "spiffe://flowplane.test/org/{}/team/{}/proxy/{}",
        owner_org.id.as_uuid(),
        owner_team.id.as_uuid(),
        uuid::Uuid::now_v7()
    );
    let wrong_dataplane = leaf_fixture(&trusted_pki, &wrong_dataplane_uri, true, "0D");
    let wrong_pki = test_pki("wrong-root");
    let wrong_root = leaf_fixture(&wrong_pki, &spiffe_uri, true, "0E");

    for (label, chain) in [
        ("malformed PEM", malformed.to_owned()),
        (
            "private-key PEM",
            format!("{}{}", trusted_leaf.chain_pem, private_key_pem),
        ),
        ("ambiguous multiple leaves", ambiguous),
        ("wrong EKU/profile", wrong_profile.chain_pem),
        ("SPIFFE/dataplane mismatch", wrong_dataplane.chain_pem),
        ("wrong trust root", wrong_root.chain_pem),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                &owner_token,
                "POST",
                &registration_path,
                Some(serde_json::json!({
                    "dataplane": dataplane_name,
                    "certificate_chain_pem": chain
                })),
            ))
            .await
            .unwrap_or_else(|error| panic!("{label} response: {error}"));
        assert_rejected(response, label).await;
    }

    // A valid chain must not weaken the existing tenant boundary or disclose the target team.
    let response = app
        .clone()
        .oneshot(request(
            &outsider_token,
            "POST",
            &registration_path,
            Some(serde_json::json!({
                "dataplane": dataplane_name,
                "certificate_chain_pem": trusted_leaf.chain_pem
            })),
        ))
        .await
        .expect("cross-tenant response");
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "cross-tenant registration must preserve non-disclosure"
    );

    let response = app
        .oneshot(request(&owner_token, "GET", &registration_path, None))
        .await
        .expect("list certificates");
    let status = response.status();
    let certificates = json(response).await;
    assert_eq!(status, StatusCode::OK, "list certificates: {certificates}");
    let matching = certificates
        .as_array()
        .expect("certificate list")
        .iter()
        .filter(|certificate| certificate["dataplane_id"] == dataplane_id)
        .count();
    assert_eq!(
        matching, 1,
        "only the trusted valid chain may mutate the registry: {certificates}"
    );
}
