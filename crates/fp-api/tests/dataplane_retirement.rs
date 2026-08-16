//! RED REST/OpenAPI contract for fpv2-7f3.7 dataplane retirement.
//!
//! Uses the public router with real PostgreSQL and production token validation. Assertions are
//! derived only from the approved retirement contract.

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
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::x509::extension::{
    AuthorityKeyIdentifier, BasicConstraints, KeyUsage, SubjectKeyIdentifier,
};
use openssl::x509::{X509NameBuilder, X509};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Barrier;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

fn build_test_ca(ca_key: &PKey<Private>, issuer_for_key_id: Option<&X509>) -> X509 {
    let mut builder = X509::builder().expect("X.509 CA builder");
    builder.set_version(2).expect("CA version");
    let serial = BigNum::from_u32(1)
        .and_then(|number| number.to_asn1_integer())
        .expect("CA serial");
    builder.set_serial_number(&serial).expect("set CA serial");
    let mut name = X509NameBuilder::new().expect("CA name builder");
    name.append_entry_by_text("CN", "retirement issue-race CA")
        .expect("CA common name");
    let name = name.build();
    builder.set_subject_name(&name).expect("CA subject");
    builder.set_issuer_name(&name).expect("CA issuer");
    builder.set_pubkey(ca_key).expect("CA public key");
    builder
        .set_not_before(&Asn1Time::days_from_now(0).expect("CA not-before"))
        .expect("set CA not-before");
    builder
        .set_not_after(&Asn1Time::days_from_now(1).expect("CA not-after"))
        .expect("set CA not-after");
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .ca()
                .build()
                .expect("CA basic constraints"),
        )
        .expect("append CA basic constraints");
    builder
        .append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .expect("CA key usage"),
        )
        .expect("append CA key usage");
    let subject_key_identifier = SubjectKeyIdentifier::new()
        .build(&builder.x509v3_context(None, None))
        .expect("CA subject key identifier");
    builder
        .append_extension(subject_key_identifier)
        .expect("append CA subject key identifier");
    if let Some(issuer) = issuer_for_key_id {
        let authority_key_identifier = AuthorityKeyIdentifier::new()
            .keyid(true)
            .build(&builder.x509v3_context(Some(issuer), None))
            .expect("CA authority key identifier");
        builder
            .append_extension(authority_key_identifier)
            .expect("append CA authority key identifier");
    }
    builder
        .sign(ca_key, MessageDigest::sha256())
        .expect("sign CA");
    builder.build()
}

fn write_test_ca() -> (PathBuf, PathBuf) {
    let directory = std::env::temp_dir().join(unique("retirement-issue-race-ca"));
    std::fs::create_dir_all(&directory).expect("CA fixture directory");
    let key = PKey::from_rsa(Rsa::generate(2048).expect("CA RSA key")).expect("CA key");
    let template = build_test_ca(&key, None);
    let certificate = build_test_ca(&key, Some(&template));
    let certificate_path = directory.join("ca.crt");
    let key_path = directory.join("ca.key");
    std::fs::write(
        &certificate_path,
        certificate.to_pem().expect("CA certificate PEM"),
    )
    .expect("write CA certificate");
    std::fs::write(
        &key_path,
        key.private_key_to_pem_pkcs8().expect("CA key PEM"),
    )
    .expect("write CA key");
    (certificate_path, key_path)
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

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON response")
    }
}

async fn assert_no_retirement_evidence(
    pool: &sqlx::PgPool,
    dataplane_id: uuid::Uuid,
    request_id: uuid::Uuid,
) {
    let retired: bool =
        sqlx::query_scalar("SELECT retired_at IS NOT NULL FROM dataplanes WHERE id = $1")
            .bind(dataplane_id)
            .fetch_one(pool)
            .await
            .expect("dataplane retirement state");
    assert!(!retired, "rejected DELETE must not tombstone the dataplane");

    let lifecycle_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type IN \
         ('dataplane.retired', 'proxy_certificate.revoked') \
         AND payload->>'dataplane_id' = $1",
    )
    .bind(dataplane_id.to_string())
    .fetch_one(pool)
    .await
    .expect("rejected DELETE lifecycle event count");
    assert_eq!(
        lifecycle_events, 0,
        "rejected DELETE must not emit retirement or revocation evidence"
    );

    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE request_id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .expect("rejected DELETE audit count");
    assert_eq!(audits, 0, "rejected DELETE must not emit audit evidence");
}

#[test]
fn openapi_documents_dataplane_delete_reason_revision_and_retirement_responses() {
    let document = serde_json::to_value(fp_api::routes::openapi_document()).expect("OpenAPI JSON");
    let operation = &document["paths"]["/api/v1/teams/{team}/dataplanes/{name}"]["delete"];
    assert!(
        operation.is_object(),
        "OpenAPI must publish dataplane DELETE: {operation}"
    );
    let parameters = operation["parameters"]
        .as_array()
        .expect("DELETE parameters");
    assert!(
        parameters.iter().any(|parameter| {
            parameter["in"] == "header"
                && parameter["name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case("if-match"))
                && parameter["required"] == true
        }),
        "dataplane DELETE must require If-Match expected revision: {operation}"
    );
    let request_schema = &operation["requestBody"]["content"]["application/json"]["schema"];
    assert!(
        !request_schema.is_null(),
        "retirement must carry an operator reason: {operation}"
    );
    for status in ["204", "400", "401", "403", "404", "409"] {
        assert!(
            operation["responses"][status].is_object(),
            "DELETE must document {status}: {operation}"
        );
    }
}

#[tokio::test]
async fn delete_retires_with_if_match_and_reason_while_cross_org_caller_learns_nothing() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 8)
        .await
        .expect("connect real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");
    let issuer = DevIssuer::generate().expect("OIDC issuer");
    let validator = Arc::new(fp_core::OidcValidator::new(issuer.oidc_config()));
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("load test JWKS");

    let owner_subject = unique("retire-rest-owner");
    let owner_token = issuer
        .mint(&owner_subject, "owner@test.invalid", "Owner", 600)
        .expect("owner token");
    let outsider_subject = unique("retire-rest-outsider");
    let outsider_token = issuer
        .mint(&outsider_subject, "outsider@test.invalid", "Outsider", 600)
        .expect("outsider token");

    let org = identity::create_org(&pool, &unique("retire-rest-org"), "")
        .await
        .expect("owner org");
    let team = identity::create_team(&pool, org.id, &unique("retire-rest-team"), "")
        .await
        .expect("owner team");
    let owner =
        identity::upsert_user_by_subject(&pool, &owner_subject, "owner@test.invalid", "Owner")
            .await
            .expect("owner user");
    identity::add_org_membership(&pool, owner, org.id, OrgRole::Admin)
        .await
        .expect("owner membership");
    let outsider_org = identity::create_org(&pool, &unique("retire-rest-outsider-org"), "")
        .await
        .expect("outsider org");
    let outsider = identity::upsert_user_by_subject(
        &pool,
        &outsider_subject,
        "outsider@test.invalid",
        "Outsider",
    )
    .await
    .expect("outsider user");
    identity::add_org_membership(&pool, outsider, outsider_org.id, OrgRole::Admin)
        .await
        .expect("outsider membership");

    let dataplane_name = unique("retire-rest-dataplane");
    let application = app(pool.clone(), validator);
    let create_path = format!("/api/v1/teams/{}/dataplanes", team.name);
    let create = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&create_path)
                .header("authorization", format!("Bearer {owner_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"name": dataplane_name, "description": "retire me"})
                        .to_string(),
                ))
                .expect("create request"),
        )
        .await
        .expect("create response");
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = body_json(create).await;
    let dataplane_id = created["id"].as_str().expect("created dataplane id");
    let revision = created["revision"].as_i64().expect("created revision");
    let delete_path = format!("/api/v1/teams/{}/dataplanes/{dataplane_name}", team.name);
    let dataplane_uuid = uuid::Uuid::parse_str(dataplane_id).expect("dataplane UUID");

    let missing_if_match_request_id = uuid::Uuid::now_v7();
    let missing_if_match = application
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&delete_path)
                .header("authorization", format!("Bearer {owner_token}"))
                .header("content-type", "application/json")
                .header("x-request-id", missing_if_match_request_id.to_string())
                .body(Body::from(
                    serde_json::json!({"reason": "missing concurrency precondition"}).to_string(),
                ))
                .expect("missing If-Match request"),
        )
        .await
        .expect("missing If-Match response");
    assert_eq!(missing_if_match.status(), StatusCode::BAD_REQUEST);
    assert_no_retirement_evidence(&pool, dataplane_uuid, missing_if_match_request_id).await;

    let stale_if_match_request_id = uuid::Uuid::now_v7();
    let stale_if_match = application
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&delete_path)
                .header("authorization", format!("Bearer {owner_token}"))
                .header("if-match", (revision + 1).to_string())
                .header("content-type", "application/json")
                .header("x-request-id", stale_if_match_request_id.to_string())
                .body(Body::from(
                    serde_json::json!({"reason": "stale concurrency precondition"}).to_string(),
                ))
                .expect("stale If-Match request"),
        )
        .await
        .expect("stale If-Match response");
    assert_eq!(stale_if_match.status(), StatusCode::CONFLICT);
    assert_no_retirement_evidence(&pool, dataplane_uuid, stale_if_match_request_id).await;

    for (label, reason) in [("empty", String::new()), ("overlong", "x".repeat(501))] {
        let request_id = uuid::Uuid::now_v7();
        let invalid_reason = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&delete_path)
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("if-match", revision.to_string())
                    .header("content-type", "application/json")
                    .header("x-request-id", request_id.to_string())
                    .body(Body::from(
                        serde_json::json!({"reason": reason}).to_string(),
                    ))
                    .unwrap_or_else(|error| panic!("{label} reason request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("{label} reason response: {error}"));
        assert_eq!(
            invalid_reason.status(),
            StatusCode::BAD_REQUEST,
            "{label} retirement reason must be rejected"
        );
        assert_no_retirement_evidence(&pool, dataplane_uuid, request_id).await;
    }

    // Product issuance and retirement share a synchronized start, but the scheduler winner is not
    // part of the contract. Issuance may lose cleanly, or commit first and be revoked by retirement.
    let race_name = unique("retire-issue-race-dataplane");
    let race_create = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&create_path)
                .header("authorization", format!("Bearer {owner_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"name": race_name, "description": "issue-retire race"})
                        .to_string(),
                ))
                .expect("issue-race dataplane request"),
        )
        .await
        .expect("issue-race dataplane response");
    assert_eq!(race_create.status(), StatusCode::CREATED);
    let race_created = body_json(race_create).await;
    let race_dataplane_id = uuid::Uuid::parse_str(
        race_created["id"]
            .as_str()
            .expect("issue-race dataplane UUID"),
    )
    .expect("issue-race dataplane UUID syntax");
    let race_revision = race_created["revision"]
        .as_i64()
        .expect("issue-race dataplane revision");
    let race_delete_path = format!("/api/v1/teams/{}/dataplanes/{race_name}", team.name);
    let issue_path = format!("/api/v1/teams/{}/proxy-certificates/issue", team.name);
    let (ca_certificate_path, ca_key_path) = write_test_ca();
    std::env::set_var("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH", &ca_certificate_path);
    std::env::set_var("FLOWPLANE_CERT_ISSUER_CA_KEY_PATH", &ca_key_path);
    std::env::set_var("FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN", "flowplane.test");

    let barrier = Arc::new(Barrier::new(2));
    let issue_barrier = Arc::clone(&barrier);
    let issue_app = application.clone();
    let issue_token = owner_token.clone();
    let issue_name = race_name.clone();
    let issue = async move {
        issue_barrier.wait().await;
        issue_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&issue_path)
                    .header("authorization", format!("Bearer {issue_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"dataplane": issue_name, "ttl_hours": 1}).to_string(),
                    ))
                    .expect("concurrent issue request"),
            )
            .await
    };
    let retire_barrier = Arc::clone(&barrier);
    let retire_app = application.clone();
    let retire_token = owner_token.clone();
    let retire = async move {
        retire_barrier.wait().await;
        retire_app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&race_delete_path)
                    .header("authorization", format!("Bearer {retire_token}"))
                    .header("if-match", race_revision.to_string())
                    .header("content-type", "application/json")
                    .header("x-request-id", uuid::Uuid::now_v7().to_string())
                    .body(Body::from(
                        serde_json::json!({"reason": "issue-retire contention"}).to_string(),
                    ))
                    .expect("concurrent retirement request"),
            )
            .await
    };
    let (issue_result, retire_result) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(issue, retire)
        })
        .await
        .expect("issue-retire race completed within ten seconds");
    let issue_response = issue_result.expect("concurrent issue response");
    let retire_response = retire_result.expect("concurrent retirement response");
    assert_eq!(
        retire_response.status(),
        StatusCode::NO_CONTENT,
        "retirement must succeed regardless of scheduler winner"
    );
    assert!(
        matches!(
            issue_response.status(),
            StatusCode::CREATED | StatusCode::NOT_FOUND | StatusCode::CONFLICT
        ),
        "issuance must either commit before retirement or lose with a lifecycle classification, got {}",
        issue_response.status()
    );

    let unrevoked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proxy_certificates WHERE dataplane_id = $1 AND revoked_at IS NULL",
    )
    .bind(race_dataplane_id)
    .fetch_one(&pool)
    .await
    .expect("final issue-race active credential count");
    assert_eq!(
        unrevoked, 0,
        "retirement must leave zero active credentials"
    );
    let race_certificate_ids: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM proxy_certificates WHERE dataplane_id = $1 ORDER BY id")
            .bind(race_dataplane_id)
            .fetch_all(&pool)
            .await
            .expect("issue-race credential rows");
    for certificate_id in &race_certificate_ids {
        let event_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'proxy_certificate.revoked' \
             AND payload->>'certificate_id' = $1",
        )
        .bind(certificate_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("issue-race revocation event count");
        assert_eq!(
            event_count, 1,
            "each credential changed by retirement emits exactly one revocation event"
        );
    }
    let race_revocation_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = 'proxy_certificate.revoked' \
         AND payload->>'dataplane_id' = $1",
    )
    .bind(race_dataplane_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("issue-race scoped revocation evidence");
    assert_eq!(
        race_revocation_events,
        race_certificate_ids.len() as i64,
        "revocation evidence must neither duplicate nor orphan credentials"
    );

    let hidden = application
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&delete_path)
                .header("authorization", format!("Bearer {outsider_token}"))
                .header("if-match", revision.to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"reason": "hostile"}).to_string(),
                ))
                .expect("cross-org delete request"),
        )
        .await
        .expect("cross-org delete response");
    assert_eq!(
        hidden.status(),
        StatusCode::NOT_FOUND,
        "cross-org retirement is non-disclosing"
    );

    let retired = application
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&delete_path)
                .header("authorization", format!("Bearer {owner_token}"))
                .header("if-match", revision.to_string())
                .header("content-type", "application/json")
                .header("x-request-id", uuid::Uuid::now_v7().to_string())
                .body(Body::from(
                    serde_json::json!({"reason": "operator decommission"}).to_string(),
                ))
                .expect("retirement request"),
        )
        .await
        .expect("retirement response");
    assert_eq!(
        retired.status(),
        StatusCode::NO_CONTENT,
        "retirement is a DELETE tombstone"
    );
    assert_eq!(body_json(retired).await, serde_json::Value::Null);

    let stored: (bool, Option<String>) = sqlx::query_as(
        "SELECT retired_at IS NOT NULL, retired_reason FROM dataplanes WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(dataplane_id).expect("dataplane UUID"))
    .fetch_one(&pool)
    .await
    .expect("stored tombstone");
    assert_eq!(stored, (true, Some("operator decommission".to_owned())));

    let bounded = |request: Request<Body>| {
        let application = application.clone();
        async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                application.oneshot(request),
            )
            .await
            .expect("retired dataplane request completed within five seconds")
            .expect("retired dataplane response")
        }
    };

    let get = bounded(
        Request::builder()
            .uri(&delete_path)
            .header("authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .expect("retired get request"),
    )
    .await;
    assert_eq!(
        get.status(),
        StatusCode::NOT_FOUND,
        "default public get must omit a retired dataplane"
    );

    let list_path = format!("/api/v1/teams/{}/dataplanes", team.name);
    let list = bounded(
        Request::builder()
            .uri(&list_path)
            .header("authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .expect("default dataplane list request"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let listed = body_json(list).await;
    assert!(
        listed["items"]
            .as_array()
            .expect("dataplane list items")
            .iter()
            .all(|item| item["id"] != dataplane_id),
        "default public list must omit the retired row: {listed}"
    );

    let bootstrap_path = format!(
        "{delete_path}/envoy-config?cert_path=/certs/client.crt&key_path=/certs/client.key&ca_path=/certs/ca.crt&xds_host=cp.test&xds_port=18000"
    );
    let bootstrap = bounded(
        Request::builder()
            .uri(&bootstrap_path)
            .header("authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .expect("retired bootstrap request"),
    )
    .await;
    assert_eq!(
        bootstrap.status(),
        StatusCode::NOT_FOUND,
        "bootstrap/config retrieval must reject a retired dataplane"
    );

    let telemetry_path = format!("{delete_path}/telemetry");
    let telemetry = bounded(
        Request::builder()
            .method("POST")
            .uri(&telemetry_path)
            .header("authorization", format!("Bearer {owner_token}"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "idempotency_key": uuid::Uuid::now_v7().to_string(),
                    "requests_delta": 13,
                    "errors_delta": 2,
                    "warming_failures_delta": 1,
                    "config_verified": true
                })
                .to_string(),
            ))
            .expect("retired telemetry request"),
    )
    .await;
    assert_eq!(
        telemetry.status(),
        StatusCode::NOT_FOUND,
        "name-addressed telemetry must reject a retired dataplane"
    );

    let counters: (i64, i64, i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT total_requests, total_errors, warming_failures, last_heartbeat_at \
         FROM dataplanes WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(dataplane_id).expect("dataplane UUID"))
    .fetch_one(&pool)
    .await
    .expect("retired dataplane telemetry counters");
    assert_eq!(
        counters,
        (0, 0, 0, None),
        "rejected telemetry must not mutate the retired tombstone"
    );
}
