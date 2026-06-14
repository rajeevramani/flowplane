//! S4 integration: full CRUD over HTTP through the real middleware stack (dev-issuer
//! tokens via the production validation path), plus the document-parity pin.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

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
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::x509::extension::{BasicConstraints, KeyUsage};
use openssl::x509::X509NameBuilder;
use std::path::PathBuf;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("json")
}

fn write_test_ca(prefix: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("flowplane-test-ca-{}", unique(prefix)));
    std::fs::create_dir_all(&dir).expect("ca dir");
    let ca_key = PKey::from_rsa(Rsa::generate(2048).expect("rsa")).expect("pkey");
    let mut builder = openssl::x509::X509::builder().expect("x509");
    builder.set_version(2).expect("version");
    let serial = BigNum::from_u32(1)
        .and_then(|n| n.to_asn1_integer())
        .expect("serial");
    builder.set_serial_number(&serial).expect("serial");
    let mut name = X509NameBuilder::new().expect("name");
    name.append_entry_by_text("CN", "Flowplane Test CA")
        .expect("cn");
    let name = name.build();
    builder.set_subject_name(&name).expect("subject");
    builder.set_issuer_name(&name).expect("issuer");
    builder.set_pubkey(&ca_key).expect("pubkey");
    let not_before = Asn1Time::days_from_now(0).expect("not before");
    let not_after = Asn1Time::days_from_now(1).expect("not after");
    builder.set_not_before(&not_before).expect("not before");
    builder.set_not_after(&not_after).expect("not after");
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .ca()
                .build()
                .expect("basic constraints"),
        )
        .expect("basic constraints");
    builder
        .append_extension(
            KeyUsage::new()
                .key_cert_sign()
                .crl_sign()
                .build()
                .expect("key usage"),
        )
        .expect("key usage");
    builder
        .sign(&ca_key, MessageDigest::sha256())
        .expect("sign");
    let ca_cert_path = dir.join("ca.crt");
    let ca_key_path = dir.join("ca.key");
    std::fs::write(&ca_cert_path, builder.build().to_pem().expect("ca pem")).expect("write cert");
    std::fs::write(
        &ca_key_path,
        ca_key.private_key_to_pem_pkcs8().expect("key pem"),
    )
    .expect("write key");
    (ca_cert_path, ca_key_path)
}

#[test]
fn openapi_document_covers_every_registered_operation() {
    let doc = fp_api::routes::openapi_document();
    let json = serde_json::to_value(&doc).expect("doc");
    let paths = json["paths"].as_object().expect("paths");
    let mut operations = 0;
    for item in paths.values() {
        operations += item.as_object().map(|o| o.len()).unwrap_or(0);
    }
    // whoami + 3 resources x 5 + 9 team/member/grant + 7 org + 4 dataplane
    // + 4 proxy-certificate + 3 ops/xds diagnostics operations.
    // + 4 secrets operations + 2 dataplane/stats telemetry operations.
    // + 5 API lifecycle operations + 5 learning-session operations.
    // + 2 expose shortcut operations.
    // Updating this pin is a deliberate speed bump when the surface changes: the doc IS
    // the contract.
    assert_eq!(
        operations, 61,
        "expected 61 documented operations, got {operations}"
    );
    assert!(json["components"]["securitySchemes"]["bearerAuth"].is_object());
    let schemas = json["components"]["schemas"].as_object().expect("schemas");
    for schema in [
        "GlobalRateLimitConfig",
        "RateLimitRequestType",
        "RouteConfigSpec",
        "ListenerSpec",
        "ClusterSpec",
    ] {
        assert!(
            schemas.contains_key(schema),
            "missing OpenAPI schema {schema}"
        );
    }
    for path in [
        "/api/v1/teams/{team}/clusters",
        "/api/v1/teams/{team}/route-configs/{name}",
        "/api/v1/teams/{team}/expose",
        "/api/v1/teams/{team}/expose/{name}",
        "/api/v1/teams/{team}/api-definitions/{name}/status",
        "/api/v1/teams/{team}/learning-sessions",
        "/api/v1/teams/{team}/learning-sessions/{session}",
        "/api/v1/teams/{team}/learning-sessions/{session}/stop",
        "/api/v1/teams/{team}/xds/status",
        "/api/v1/teams/{team}/ops/trace",
    ] {
        assert!(paths.contains_key(path), "missing {path}");
    }
}

#[tokio::test]
async fn learning_session_lifecycle_over_http() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "learn@test", "Learn", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "learn@test", "Learn")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
    });

    let request =
        |method: &str, path: &str, body: Option<serde_json::Value>, revision: Option<i64>| {
            let mut builder = Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"));
            if let Some(revision) = revision {
                builder = builder.header("if-match", revision.to_string());
            }
            match body {
                Some(json) => builder
                    .header("content-type", "application/json")
                    .body(Body::from(json.to_string())),
                None => builder.body(Body::empty()),
            }
            .expect("request")
        };

    let api_name = unique("catalog");
    let api_base = format!("/api/v1/teams/{}/api-definitions", team.name);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &api_base,
            Some(serde_json::json!({
                "name": api_name,
                "display_name": "Catalog"
            })),
            None,
        ))
        .await
        .expect("create api");
    assert_eq!(response.status(), StatusCode::CREATED);

    let session_name = unique("capture");
    let learn_base = format!("/api/v1/teams/{}/learning-sessions", team.name);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &learn_base,
            Some(serde_json::json!({
                "name": session_name,
                "api": api_name,
                "target_sample_count": 25,
                "max_duration_seconds": 60,
                "max_bytes": 4096,
                "max_distinct_paths": 20
            })),
            None,
        ))
        .await
        .expect("start learning session");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["name"], session_name);
    assert_eq!(body["status"], "capturing");
    assert_eq!(body["sample_count"], 0);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{learn_base}/{session_name}"),
            None,
            None,
        ))
        .await
        .expect("get session");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["target_sample_count"], 25);

    let response = app
        .clone()
        .oneshot(request("GET", &learn_base, None, None))
        .await
        .expect("list sessions");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["total"], 1);

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("{learn_base}/{session_name}/stop"),
            None,
            None,
        ))
        .await
        .expect("stop session");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["status"], "completed");
    assert!(body["completed_at"].is_string());

    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("{learn_base}/{session_name}"),
            None,
            None,
        ))
        .await
        .expect("cancel completed session");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn api_definition_import_status_and_delete_over_http() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("sub");
    let token = issuer.mint(&subject, "api@test", "Api", 600).expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "api@test", "Api")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
    });

    let request =
        |method: &str, path: &str, body: Option<serde_json::Value>, revision: Option<i64>| {
            let mut builder = Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"));
            if let Some(revision) = revision {
                builder = builder.header("if-match", revision.to_string());
            }
            match body {
                Some(json) => builder
                    .header("content-type", "application/json")
                    .body(Body::from(json.to_string())),
                None => builder.body(Body::empty()),
            }
            .expect("request")
        };

    let api_name = unique("catalog");
    let base = format!("/api/v1/teams/{}/api-definitions", team.name);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &base,
            Some(serde_json::json!({
                "name": api_name,
                "display_name": "Catalog",
                "openapi": {
                    "openapi": "3.0.3",
                    "info": {"title": "Catalog", "version": "1.0.0"},
                    "paths": {
                        "/items": {"get": {"operationId": "listItems"}},
                        "/items/{id}": {"post": {}}
                    }
                }
            })),
            None,
        ))
        .await
        .expect("create api");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["api"]["name"], api_name);
    assert_eq!(body["latest_spec"]["version"], 1);
    assert_eq!(body["tool_count"], 2);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{base}/{api_name}/status"),
            None,
            None,
        ))
        .await
        .expect("status");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["tool_count"], 2);
    assert_eq!(body["route_binding_count"], 0);

    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("{base}/{api_name}"),
            None,
            Some(1),
        ))
        .await
        .expect("delete api");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/{api_name}"), None, None))
        .await
        .expect("get deleted api");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn full_crud_journey_over_http_with_bearer_auth() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Identity: an org admin in a fresh org/team, authenticated by a real RS256 token.
    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "crud@test", "Crud", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "crud@test", "Crud")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
    });

    let request =
        |method: &str, path: &str, body: Option<serde_json::Value>, revision: Option<i64>| {
            let mut builder = Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"));
            if let Some(revision) = revision {
                builder = builder.header("if-match", revision.to_string());
            }
            match body {
                Some(json) => builder
                    .header("content-type", "application/json")
                    .body(Body::from(json.to_string())),
                None => builder.body(Body::empty()),
            }
            .expect("request")
        };

    let base = format!("/api/v1/teams/{}/clusters", team.name);
    let cluster = unique("crud");
    let spec = serde_json::json!({"endpoints": [{"host": "10.0.0.1", "port": 8080}]});

    // Create -> 201 with revision 1.
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &base,
            Some(serde_json::json!({"name": cluster, "spec": spec})),
            None,
        ))
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(json_of(response).await["revision"], 1);

    // Duplicate create -> 409 conflict envelope.
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &base,
            Some(serde_json::json!({"name": cluster, "spec": spec})),
            None,
        ))
        .await
        .expect("dup");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_of(response).await["code"], "conflict");

    // Update without If-Match -> 400 with the actionable hint.
    let item = format!("{base}/{cluster}");
    let update = serde_json::json!({"spec": {"endpoints": [{"host": "10.0.0.2", "port": 9090}]}});
    let response = app
        .clone()
        .oneshot(request("PATCH", &item, Some(update.clone()), None))
        .await
        .expect("no revision");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_of(response).await;
    assert!(body["hint"].as_str().expect("hint").contains("If-Match"));

    // Update with stale revision -> 409 revision_mismatch.
    let response = app
        .clone()
        .oneshot(request("PATCH", &item, Some(update.clone()), Some(7)))
        .await
        .expect("stale");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_of(response).await["code"], "revision_mismatch");

    // Update with the right revision -> 200, revision 2.
    let response = app
        .clone()
        .oneshot(request("PATCH", &item, Some(update), Some(1)))
        .await
        .expect("update");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_of(response).await["revision"], 2);

    // List shows it in the uniform envelope.
    let response = app
        .clone()
        .oneshot(request("GET", &base, None, None))
        .await
        .expect("list");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert!(body["total"].as_i64().expect("total") >= 1);
    assert!(body["items"].is_array());

    // Delete with current revision -> 204; subsequent GET -> 404 envelope.
    let response = app
        .clone()
        .oneshot(request("DELETE", &item, None, Some(2)))
        .await
        .expect("delete");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(request("GET", &item, None, None))
        .await
        .expect("get");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_of(response).await["code"], "not_found");

    // S7.7d: expose shortcut creates normal gateway resources and unexpose removes them.
    let expose_name = unique("demo");
    let expose_base = format!("/api/v1/teams/{}/expose", team.name);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &expose_base,
            Some(serde_json::json!({
                "name": expose_name,
                "upstream": "http://127.0.0.1:3001",
                "path": "/",
                "port": 10001
            })),
            None,
        ))
        .await
        .expect("expose");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["port"], 10001);
    assert_eq!(body["curl_url"], "http://127.0.0.1:10001/");
    assert_eq!(body["cluster"]["name"], format!("{expose_name}-upstream"));
    assert_eq!(body["cluster"]["spec"]["endpoints"][0]["host"], "127.0.0.1");
    assert_eq!(
        body["route_config"]["name"],
        format!("{expose_name}-routes")
    );
    assert_eq!(body["listener"]["name"], expose_name);
    assert_eq!(
        body["listener"]["spec"]["route_config"],
        format!("{expose_name}-routes")
    );

    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("{expose_base}/{expose_name}"),
            None,
            None,
        ))
        .await
        .expect("unexpose");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["cluster_name"], format!("{expose_name}-upstream"));

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/teams/{}/listeners/{expose_name}", team.name),
            None,
            None,
        ))
        .await
        .expect("get exposed listener");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // S7.8f: advanced gateway specs round-trip through REST + storage without projection loss.
    let primary = unique("primary");
    let canary = unique("canary");
    let rls = unique("rls");
    for name in [&primary, &canary, &rls] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &base,
                Some(serde_json::json!({
                    "name": name,
                    "spec": {"endpoints": [{"host": "10.0.0.10", "port": 8080}]}
                })),
                None,
            ))
            .await
            .expect("create parity cluster");
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let rc_name = unique("routes");
    let route_base = format!("/api/v1/teams/{}/route-configs", team.name);
    let route_spec = serde_json::json!({
        "virtual_hosts": [{
            "name": "default",
            "domains": ["api.example.test"],
            "rate_limits": [{
                "stage": 1,
                "actions": [{"type": "generic_key", "descriptor_value": "vhost", "descriptor_key": "scope"}]
            }],
            "routes": [{
                "name": "items",
                "match": {"regex": {"pattern": "^/v[0-9]+/items$"}},
                "headers": [{"name": "x-api-version", "type": "exact", "value": "2"}],
                "query_parameters": [{"name": "preview", "type": "present", "value": true}],
                "action": {
                    "weighted_clusters": [
                        {"cluster": primary, "weight": 80},
                        {"cluster": canary, "weight": 20}
                    ],
                    "timeout_secs": 10,
                    "retry_policy": {
                        "retry_on": "5xx,connect-failure",
                        "num_retries": 2,
                        "per_try_timeout_secs": 3,
                        "retriable_status_codes": [502, 503]
                    },
                    "rate_limits": [{
                        "actions": [{"type": "request_headers", "header_name": "x-api-key", "descriptor_key": "api_key"}]
                    }]
                }
            }],
            "filter_overrides": []
        }]
    });
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &route_base,
            Some(serde_json::json!({"name": rc_name, "spec": route_spec})),
            None,
        ))
        .await
        .expect("create advanced route config");
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{route_base}/{rc_name}"),
            None,
            None,
        ))
        .await
        .expect("get advanced route config");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    let route = &body["spec"]["virtual_hosts"][0]["routes"][0];
    assert_eq!(route["match"]["regex"]["pattern"], "^/v[0-9]+/items$");
    assert_eq!(route["headers"][0]["type"], "exact");
    assert_eq!(route["query_parameters"][0]["type"], "present");
    assert_eq!(route["action"]["weighted_clusters"][1]["cluster"], canary);
    assert_eq!(
        route["action"]["retry_policy"]["retriable_status_codes"][1],
        503
    );
    assert_eq!(
        route["action"]["rate_limits"][0]["actions"][0]["type"],
        "request_headers"
    );

    let listener_name = unique("edge");
    let listener_base = format!("/api/v1/teams/{}/listeners", team.name);
    let listener_spec = serde_json::json!({
        "address": "0.0.0.0",
        "port": 18080,
        "protocol": "http2",
        "route_config": rc_name,
        "access_logs": [{"path": "/tmp/flowplane-access.log", "text_format": "%REQ(:METHOD)% %RESPONSE_CODE%\\n"}],
        "http_filters": [{
            "filter": {
                "type": "global_rate_limit",
                "domain": "flowplane",
                "service_cluster": rls,
                "timeout_ms": 50,
                "failure_mode_deny": true,
                "stage": 1,
                "request_type": "external",
                "stat_prefix": "edge_rls",
                "enable_x_ratelimit_headers": true,
                "disable_x_envoy_ratelimited_header": true,
                "rate_limited_status": 429,
                "status_on_error": 503
            }
        }]
    });
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &listener_base,
            Some(serde_json::json!({"name": listener_name, "spec": listener_spec})),
            None,
        ))
        .await
        .expect("create advanced listener");
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{listener_base}/{listener_name}"),
            None,
            None,
        ))
        .await
        .expect("get advanced listener");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["spec"]["protocol"], "http2");
    assert_eq!(
        body["spec"]["access_logs"][0]["path"],
        "/tmp/flowplane-access.log"
    );
    assert_eq!(
        body["spec"]["http_filters"][0]["filter"]["type"],
        "global_rate_limit"
    );
    assert_eq!(
        body["spec"]["http_filters"][0]["filter"]["service_cluster"],
        rls
    );
}

#[tokio::test]
async fn multi_org_user_selects_active_org_with_header() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "multi-http@test", "Multi HTTP", 600)
        .expect("mint");

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team = identity::create_team(&pool, org_a.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "multi-http@test", "Multi HTTP")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org_a.id, OrgRole::Admin)
        .await
        .expect("member a");
    identity::add_org_membership(&pool, user, org_b.id, OrgRole::Viewer)
        .await
        .expect("member b");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/whoami")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("whoami");
    assert_eq!(response.status(), StatusCode::OK);
    let whoami = json_of(response).await;
    assert!(
        whoami["org_id"].is_null(),
        "ambiguous users have no active org"
    );
    assert_eq!(
        whoami["memberships"].as_array().expect("memberships").len(),
        2
    );

    let path = format!("/api/v1/teams/{}/clusters", team.name);
    let base = || {
        Request::builder()
            .method("GET")
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
    };

    let response = app
        .clone()
        .oneshot(base().body(Body::empty()).expect("request"))
        .await
        .expect("ambiguous");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(
            base()
                .header("x-flowplane-org", org_a.name)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("selected");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["total"], 0);
    assert!(body["items"].is_array());
}

#[tokio::test]
async fn proxy_certificate_registry_flow_over_http() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "cert-http@test", "Cert HTTP", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "cert-http@test", "Cert HTTP")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
    });

    let request = |method: &str, path: &str, body: Option<serde_json::Value>| {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(match body {
                Some(json) => Body::from(json.to_string()),
                None => Body::empty(),
            })
            .expect("request")
    };

    let dataplanes = format!("/api/v1/teams/{}/dataplanes", team.name);
    let dataplane = unique("dp");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &dataplanes,
            Some(serde_json::json!({"name": dataplane, "description": "edge"})),
        ))
        .await
        .expect("create dataplane");
    assert_eq!(response.status(), StatusCode::CREATED);

    let telemetry = format!("{dataplanes}/{dataplane}/telemetry");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &telemetry,
            Some(serde_json::json!({
                "requests_delta": 10,
                "errors_delta": 2,
                "warming_failures_delta": 1,
                "config_verified": true
            })),
        ))
        .await
        .expect("telemetry");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["total_requests"], 10);
    assert_eq!(body["total_errors"], 2);
    assert_eq!(body["warming_failures"], 1);
    assert!(body["last_heartbeat_at"].is_string());

    let stats = format!("/api/v1/teams/{}/stats/overview", team.name);
    let response = app
        .clone()
        .oneshot(request("GET", &stats, None))
        .await
        .expect("stats");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["total_dataplanes"], 1);
    assert_eq!(body["live_dataplanes"], 1);
    assert_eq!(body["total_requests"], 10);

    let config_path = format!(
        "{dataplanes}/{dataplane}/envoy-config?cert_path=/certs/client.crt&key_path=/certs/client.key&ca_path=/certs/ca.crt&xds_host=cp.local&xds_port=18000"
    );
    let response = app
        .clone()
        .oneshot(request("GET", &config_path, None))
        .await
        .expect("envoy config");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/yaml; charset=utf-8")
    );
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec(),
    )
    .expect("utf8");
    serde_yaml::from_str::<serde_yaml::Value>(&body).expect("mTLS bootstrap is valid YAML");
    assert!(body.contains(&format!("id: \"team={}/dp-", team.id.as_uuid())));
    assert!(body.contains("cluster_name: xds_cluster"));
    assert!(body.contains("filename: \"/certs/client.crt\""));
    assert!(body.contains("filename: \"/certs/client.key\""));
    assert!(body.contains("filename: \"/certs/ca.crt\""));

    let dev_config_path =
        format!("{dataplanes}/{dataplane}/envoy-config?mode=dev&xds_host=127.0.0.1");
    let response = app
        .clone()
        .oneshot(request("GET", &dev_config_path, None))
        .await
        .expect("dev envoy config");
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec(),
    )
    .expect("utf8");
    serde_yaml::from_str::<serde_yaml::Value>(&body).expect("dev bootstrap is valid YAML");
    assert!(body.contains("cluster_name: xds_cluster"));
    assert!(!body.contains("transport_socket:"));
    assert!(!body.contains("filename:"));

    let certs = format!("/api/v1/teams/{}/proxy-certificates", team.name);
    let (ca_cert_path, ca_key_path) = write_test_ca("issue");
    std::env::set_var("FLOWPLANE_CERT_ISSUER_CA_CERT_PATH", &ca_cert_path);
    std::env::set_var("FLOWPLANE_CERT_ISSUER_CA_KEY_PATH", &ca_key_path);
    std::env::set_var("FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN", "flowplane.test");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("{certs}/issue"),
            Some(serde_json::json!({
                "dataplane": dataplane,
                "ttl_hours": 1
            })),
        ))
        .await
        .expect("issue cert");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let issued_serial = body["certificate"]["serial_number"]
        .as_str()
        .expect("issued serial")
        .to_owned();
    assert_eq!(
        body["certificate"]["spiffe_uri"],
        format!(
            "spiffe://flowplane.test/org/{}/team/{}/proxy/{}",
            org.id.as_uuid(),
            team.id.as_uuid(),
            body["certificate"]["dataplane_id"].as_str().expect("dp id")
        )
    );
    assert!(body["certificate_pem"]
        .as_str()
        .expect("cert pem")
        .contains("BEGIN CERTIFICATE"));
    assert!(body["private_key_pem"]
        .as_str()
        .expect("key pem")
        .contains("BEGIN PRIVATE KEY"));
    assert!(body["ca_certificate_pem"]
        .as_str()
        .expect("ca pem")
        .contains("BEGIN CERTIFICATE"));

    let serial = unique("serial");
    let spiffe = format!(
        "spiffe://flowplane.test/org/{}/team/{}/proxy/{}",
        org.name, team.name, dataplane
    );
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &certs,
            Some(serde_json::json!({
                "dataplane": dataplane,
                "spiffe_uri": spiffe,
                "serial_number": serial,
                "expires_at": "2099-01-01T00:00:00Z"
            })),
        ))
        .await
        .expect("register cert");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["serial_number"], serial);
    assert!(body["revoked_at"].is_null());

    let response = app
        .clone()
        .oneshot(request("GET", &certs, None))
        .await
        .expect("list certs");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert!(body
        .as_array()
        .expect("cert list")
        .iter()
        .any(|cert| cert["serial_number"] == serial));
    assert!(body
        .as_array()
        .expect("cert list")
        .iter()
        .any(|cert| cert["serial_number"] == issued_serial));

    let revoke = format!("{certs}/{serial}/revoke");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &revoke,
            Some(serde_json::json!({"reason": "rotation"})),
        ))
        .await
        .expect("revoke cert");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["revoked_reason"], "rotation");
    assert!(body["revoked_at"].is_string());

    let response = app
        .oneshot(request(
            "POST",
            &revoke,
            Some(serde_json::json!({"reason": "again"})),
        ))
        .await
        .expect("double revoke");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_of(response).await["code"], "conflict");
}

#[tokio::test]
async fn secret_values_are_write_only_over_http() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
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
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "secret-http@test", "Secret HTTP", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "secret-http@test", "Secret HTTP")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
    });

    let request = |method: &str, path: &str, body: Option<serde_json::Value>| {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(match body {
                Some(json) => Body::from(json.to_string()),
                None => Body::empty(),
            })
            .expect("request")
    };

    let base = format!("/api/v1/teams/{}/secrets", team.name);
    let name = unique("secret");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &base,
            Some(serde_json::json!({
                "name": name,
                "description": "token",
                "spec": {"type": "generic_secret", "secret": "aGVsbG8="}
            })),
        ))
        .await
        .expect("create secret");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["value_redacted"], true);
    assert!(body.get("spec").is_none());
    assert_eq!(body["revision"], 1);

    let item = format!("{base}/{name}");
    let response = app
        .clone()
        .oneshot(request("GET", &item, None))
        .await
        .expect("get secret");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["value_redacted"], true);
    assert!(body.get("spec").is_none());

    let response = app
        .clone()
        .oneshot(request("GET", &base, None))
        .await
        .expect("list secrets");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert!(body["items"]
        .as_array()
        .expect("items")
        .iter()
        .any(|secret| secret["name"] == name && secret["value_redacted"] == true));

    let response = app
        .oneshot(request(
            "POST",
            &format!("{item}/rotate"),
            Some(serde_json::json!({
                "spec": {"type": "generic_secret", "secret": "d29ybGQ="}
            })),
        ))
        .await
        .expect("rotate secret");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["revision"], 2);
    assert_eq!(body["value_redacted"], true);
    assert!(body.get("spec").is_none());
}
