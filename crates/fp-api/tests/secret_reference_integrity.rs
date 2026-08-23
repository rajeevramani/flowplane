//! fpv2-a09.1 public-REST secret-reference contract tests.
//!
//! The first missing/wrong-type listener case was independently authored from acceptance
//! criteria under a production-source firewall and captured behavioral RED. Producer-owned
//! cases then extend the same real-router/real-PostgreSQL harness across authorization,
//! normalized rows, cluster parity, immutable rotation and reference removal.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy, UpstreamTlsConfig};
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct Env {
    app: axum::Router,
    pool: PgPool,
    token: String,
    limited_token: String,
    team_name: String,
}

async fn env() -> Option<Env> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    std::env::set_var(
        "FLOWPLANE_SECRET_ENCRYPTION_KEY",
        "12345678901234567890123456789012",
    );
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("admin-sub");
    let email = format!("{}@test", unique("admin"));
    let token = issuer
        .mint(&subject, &email, "Secret Ref Admin", 600)
        .expect("token");
    let limited_subject = unique("limited-sub");
    let limited_email = format!("{}@test", unique("limited"));
    let limited_token = issuer
        .mint(&limited_subject, &limited_email, "Limited Listener", 600)
        .expect("limited token");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, &email, "Secret Ref Admin")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership");
    let limited_user = identity::upsert_user_by_subject(
        &pool,
        &limited_subject,
        &limited_email,
        "Limited Listener",
    )
    .await
    .expect("limited user");
    identity::add_org_membership(&pool, limited_user, org.id, OrgRole::Member)
        .await
        .expect("limited org membership");
    identity::add_team_membership(&pool, limited_user, team.id)
        .await
        .expect("limited membership");
    for resource in [Resource::Listeners, Resource::Clusters] {
        for action in [Action::Create, Action::Read, Action::Update] {
            identity::add_grant(
                &pool,
                limited_user,
                org.id,
                team.id,
                resource,
                action,
                Some(user),
            )
            .await
            .expect("gateway grant");
        }
    }

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        xds_degraded: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    Some(Env {
        app,
        pool,
        token,
        limited_token,
        team_name: team.name,
    })
}

fn request(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request")
}

async fn send(
    env: &Env,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    send_as(env, &env.token, method, uri, body).await
}

async fn send_as(
    env: &Env,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let response = env
        .app
        .clone()
        .oneshot(request(method, uri, token, body))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON response")
    };
    (status, body)
}

fn listener_body(name: &str, secret_name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "spec": {
            "address": "127.0.0.1",
            "port": 18443,
            "protocol": "https",
            "tls_context": {
                "tls_certificate_sds_secret_name": secret_name
            }
        }
    })
}

fn cluster_body(name: &str, secret_name: &str) -> serde_json::Value {
    let spec = ClusterSpec {
        endpoints: vec![Endpoint {
            host: "api.example.test".into(),
            port: 443,
            weight: None,
        }],
        aggregate_clusters: Vec::new(),
        lb_policy: LbPolicy::RoundRobin,
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 5,
        use_tls: true,
        upstream_tls: Some(UpstreamTlsConfig {
            sni: Some("api.example.test".into()),
            validation_context_sds_secret_name: Some(secret_name.into()),
            ca_cert_file: None,
            auto_sni_san_validation: true,
            insecure_skip_verify: false,
        }),
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
        outlier_detection: None,
    };
    serde_json::json!({"name": name, "spec": spec})
}

#[tokio::test]
async fn listener_create_rejects_missing_and_wrong_type_sds_refs_without_creating_owner() {
    let Some(env) = env().await else { return };
    let generic_secret = unique("generic");
    let (status, body) = send(
        &env,
        "POST",
        &format!("/api/v1/teams/{}/secrets", env.team_name),
        Some(serde_json::json!({
            "name": generic_secret,
            "spec": {"type": "generic_secret", "secret": "dG9rZW4="}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "secret fixture: {body}");

    for (case, secret_name, expected) in [
        ("missing", unique("absent"), StatusCode::NOT_FOUND),
        (
            "wrong-type",
            generic_secret.clone(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let listener = unique(case);
        let collection = format!("/api/v1/teams/{}/listeners", env.team_name);
        let (status, body) = send(
            &env,
            "POST",
            &collection,
            Some(listener_body(&listener, &secret_name)),
        )
        .await;
        assert_eq!(
            status, expected,
            "{case} SDS secret reference must fail closed: {body}"
        );

        let (get_status, get_body) =
            send(&env, "GET", &format!("{collection}/{listener}"), None).await;
        assert_eq!(
            get_status,
            StatusCode::NOT_FOUND,
            "failed {case} create must leave no listener owner: {get_body}"
        );
    }
}

#[tokio::test]
async fn listener_sds_use_requires_secrets_read_and_persists_normalized_ref() {
    let Some(env) = env().await else { return };
    let secret_name = unique("server-cert");
    let (status, body) = send(
        &env,
        "POST",
        &format!("/api/v1/teams/{}/secrets", env.team_name),
        Some(serde_json::json!({
            "name": secret_name,
            "spec": {
                "type": "tls_certificate",
                "certificate_chain": "test-certificate",
                "private_key": "test-private-key"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "secret fixture: {body}");

    let collection = format!("/api/v1/teams/{}/listeners", env.team_name);
    let (plain_status, plain_body) = send_as(
        &env,
        &env.limited_token,
        "POST",
        &collection,
        Some(serde_json::json!({
            "name": unique("plain"),
            "spec": {"address": "127.0.0.1", "port": 18080}
        })),
    )
    .await;
    assert_eq!(
        plain_status,
        StatusCode::CREATED,
        "no-SDS listener must not require Secrets/Read: {plain_body}"
    );

    let denied_name = unique("denied-sds");
    let (denied_status, denied_body) = send_as(
        &env,
        &env.limited_token,
        "POST",
        &collection,
        Some(listener_body(&denied_name, &secret_name)),
    )
    .await;
    assert_eq!(
        denied_status,
        StatusCode::FORBIDDEN,
        "SDS use without Secrets/Read must fail closed: {denied_body}"
    );

    let allowed_name = unique("allowed-sds");
    let (allowed_status, allowed_body) = send(
        &env,
        "POST",
        &collection,
        Some(listener_body(&allowed_name, &secret_name)),
    )
    .await;
    assert_eq!(
        allowed_status,
        StatusCode::CREATED,
        "allowed SDS: {allowed_body}"
    );
    assert!(!allowed_body.to_string().contains("test-private-key"));
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM listener_secret_refs r \
         JOIN listeners l ON l.id = r.listener_id \
         JOIN secrets s ON s.id = r.secret_id \
         WHERE l.name = $1 AND s.name = $2 AND r.usage = 'tls_certificate'",
    )
    .bind(&allowed_name)
    .bind(&secret_name)
    .fetch_one(&env.pool)
    .await
    .expect("normalized ref");
    assert_eq!(refs, 1);
}

#[tokio::test]
async fn cluster_create_rejects_missing_and_wrong_type_validation_context_refs() {
    let Some(env) = env().await else { return };
    let generic_secret = unique("generic-cluster");
    let (status, body) = send(
        &env,
        "POST",
        &format!("/api/v1/teams/{}/secrets", env.team_name),
        Some(serde_json::json!({
            "name": generic_secret,
            "spec": {"type": "generic_secret", "secret": "dG9rZW4="}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "secret fixture: {body}");

    for (case, secret_name, expected) in [
        ("missing", unique("absent-ca"), StatusCode::NOT_FOUND),
        ("wrong-type", generic_secret, StatusCode::BAD_REQUEST),
    ] {
        let (status, body) = send(
            &env,
            "POST",
            &format!("/api/v1/teams/{}/clusters", env.team_name),
            Some(cluster_body(&unique(case), &secret_name)),
        )
        .await;
        assert_eq!(status, expected, "{case} cluster SDS ref: {body}");
    }

    let ca_name = unique("upstream-ca");
    let (status, body) = send(
        &env,
        "POST",
        &format!("/api/v1/teams/{}/secrets", env.team_name),
        Some(serde_json::json!({
            "name": ca_name,
            "spec": {
                "type": "certificate_validation_context",
                "trusted_ca": "test-ca"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "CA fixture: {body}");

    let clusters = format!("/api/v1/teams/{}/clusters", env.team_name);
    let plain_name = unique("cluster-plain");
    let mut plain_body = cluster_body(&plain_name, "unused");
    plain_body["spec"]["use_tls"] = serde_json::json!(false);
    plain_body["spec"]["upstream_tls"] = serde_json::Value::Null;
    let (status, body) = send_as(
        &env,
        &env.limited_token,
        "POST",
        &clusters,
        Some(plain_body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "no-SDS cluster must not require Secrets/Read: {body}"
    );

    let denied_name = unique("cluster-denied");
    let (status, body) = send_as(
        &env,
        &env.limited_token,
        "POST",
        &clusters,
        Some(cluster_body(&denied_name, &ca_name)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cluster SDS use without Secrets/Read: {body}"
    );

    let allowed_name = unique("cluster-allowed");
    let (status, body) = send(
        &env,
        "POST",
        &clusters,
        Some(cluster_body(&allowed_name, &ca_name)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "cluster SDS fixture: {body}");
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cluster_secret_refs r \
         JOIN clusters c ON c.id = r.cluster_id \
         JOIN secrets s ON s.id = r.secret_id \
         WHERE c.name = $1 AND s.name = $2 AND r.usage = 'validation_context'",
    )
    .bind(&allowed_name)
    .bind(&ca_name)
    .fetch_one(&env.pool)
    .await
    .expect("normalized cluster ref");
    assert_eq!(refs, 1);

    let mut plain_spec = cluster_body(&allowed_name, "unused")["spec"].clone();
    plain_spec["use_tls"] = serde_json::json!(false);
    plain_spec["upstream_tls"] = serde_json::Value::Null;
    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("{clusters}/{allowed_name}"))
                .header("authorization", format!("Bearer {}", env.limited_token))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(
                    serde_json::json!({"spec": plain_spec}).to_string(),
                ))
                .expect("cluster update request"),
        )
        .await
        .expect("cluster update response");
    assert_eq!(response.status(), StatusCode::OK);
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cluster_secret_refs r \
         JOIN clusters c ON c.id = r.cluster_id WHERE c.name = $1",
    )
    .bind(&allowed_name)
    .fetch_one(&env.pool)
    .await
    .expect("cluster ref count");
    assert_eq!(refs, 0);
}

#[tokio::test]
async fn rotation_rejects_type_change_and_allows_same_type() {
    let Some(env) = env().await else { return };
    let name = unique("rotate-generic");
    let collection = format!("/api/v1/teams/{}/secrets", env.team_name);
    let (status, body) = send(
        &env,
        "POST",
        &collection,
        Some(serde_json::json!({
            "name": name,
            "spec": {"type": "generic_secret", "secret": "b25l"}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "secret fixture: {body}");

    let rotate = |spec: serde_json::Value, revision: i64| {
        Request::builder()
            .method("POST")
            .uri(format!("{collection}/{name}/rotate"))
            .header("authorization", format!("Bearer {}", env.token))
            .header("content-type", "application/json")
            .header("if-match", revision.to_string())
            .body(Body::from(serde_json::json!({"spec": spec}).to_string()))
            .expect("rotate request")
    };
    let response = env
        .app
        .clone()
        .oneshot(rotate(
            serde_json::json!({
                "type": "tls_certificate",
                "certificate_chain": "certificate",
                "private_key": "private-key"
            }),
            1,
        ))
        .await
        .expect("wrong-type response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let (get_status, metadata) = send(&env, "GET", &format!("{collection}/{name}"), None).await;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(metadata["revision"], 1);
    assert_eq!(metadata["secret_type"], "generic_secret");

    let response = env
        .app
        .clone()
        .oneshot(rotate(
            serde_json::json!({"type": "generic_secret", "secret": "dHdv"}),
            1,
        ))
        .await
        .expect("same-type response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let metadata: serde_json::Value = serde_json::from_slice(&bytes).expect("metadata");
    assert_eq!(metadata["revision"], 2);
    assert_eq!(metadata["secret_type"], "generic_secret");
    assert!(!metadata.to_string().contains("dHdv"));
}

#[tokio::test]
async fn removing_listener_sds_ref_does_not_require_secrets_read() {
    let Some(env) = env().await else { return };
    let secret_name = unique("remove-cert");
    let secrets = format!("/api/v1/teams/{}/secrets", env.team_name);
    let (status, body) = send(
        &env,
        "POST",
        &secrets,
        Some(serde_json::json!({
            "name": secret_name,
            "spec": {
                "type": "tls_certificate",
                "certificate_chain": "test-certificate",
                "private_key": "test-private-key"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "secret fixture: {body}");

    let listener_name = unique("remove-sds");
    let listeners = format!("/api/v1/teams/{}/listeners", env.team_name);
    let (status, body) = send(
        &env,
        "POST",
        &listeners,
        Some(listener_body(&listener_name, &secret_name)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "listener fixture: {body}");

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("{listeners}/{listener_name}"))
                .header("authorization", format!("Bearer {}", env.limited_token))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(
                    serde_json::json!({
                        "spec": {"address": "127.0.0.1", "port": 18443}
                    })
                    .to_string(),
                ))
                .expect("update request"),
        )
        .await
        .expect("update response");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "removing a reference does not use/discover a secret"
    );
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM listener_secret_refs r \
         JOIN listeners l ON l.id = r.listener_id WHERE l.name = $1",
    )
    .bind(&listener_name)
    .fetch_one(&env.pool)
    .await
    .expect("ref count");
    assert_eq!(refs, 0);
}
