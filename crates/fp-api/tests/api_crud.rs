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
    // whoami + 3 resources x 5 + 9 team/member/grant + 5 agent + 7 org + 4 dataplane
    // + 4 proxy-certificate + 3 ops/xds diagnostics operations.
    // + 4 secrets operations + 2 dataplane/stats telemetry operations.
    // + 10 API lifecycle/MCP tool operations + 6 learning-session operations.
    // + 5 discovery-session operations.
    // + 2 expose shortcut operations.
    // + 2 route-generation plan operations.
    // + 5 AI provider operations + 5 AI route operations.
    // + 5 AI budget operations + 1 AI usage operation + 1 AI trace operation.
    // + 2 AI retention operations (GET/PUT).
    // + 1 RLS force-repush admin operation.
    // + 14 rate-limit CRUD operations (5 domain + 5 policy + 4 override).
    // Updating this pin is a deliberate speed bump when the surface changes: the doc IS
    // the contract.
    assert_eq!(
        operations, 113,
        "expected 113 documented operations, got {operations}"
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
        "/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/reject",
        "/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/publish",
        "/api/v1/teams/{team}/learning-sessions",
        "/api/v1/teams/{team}/learning-sessions/{session}",
        "/api/v1/teams/{team}/learning-sessions/{session}/stop",
        "/api/v1/teams/{team}/learning-sessions/{session}/spec-version",
        "/api/v1/teams/{team}/learning-discovery-sessions",
        "/api/v1/teams/{team}/learning-discovery-sessions/{session}",
        "/api/v1/teams/{team}/learning-discovery-sessions/{session}/stop",
        "/api/v1/teams/{team}/learning-discovery-sessions/{session}/spec-versions",
        "/api/v1/teams/{team}/ai/providers",
        "/api/v1/teams/{team}/ai/routes",
        "/api/v1/teams/{team}/ai/trace",
        "/api/v1/teams/{team}/ai/retention",
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
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
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
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
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
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
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
                "port": 10001,
                "public_base_url": "https://gateway.example"
            })),
            None,
        ))
        .await
        .expect("expose");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["port"], 10001);
    assert_eq!(body["curl_url"], "https://gateway.example/");
    assert_eq!(body["endpoint_source"], "listener.public_base_url");
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
    assert_eq!(
        body["listener"]["spec"]["public_base_url"],
        "https://gateway.example"
    );

    let no_endpoint_name = unique("local");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &expose_base,
            Some(serde_json::json!({
                "name": no_endpoint_name,
                "upstream": "http://127.0.0.1:3001",
                "path": "/local",
                "port": 10002
            })),
            None,
        ))
        .await
        .expect("expose without endpoint");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert!(body.get("curl_url").is_none(), "curl_url must be omitted");
    assert_eq!(body["endpoint_source"], "unconfigured");

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
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(response).await["code"], "org_selector_required");

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
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
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
                "idempotency_key": "telemetry-1",
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

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &telemetry,
            Some(serde_json::json!({
                "idempotency_key": "telemetry-1",
                "requests_delta": 10,
                "errors_delta": 2,
                "warming_failures_delta": 1,
                "config_verified": true
            })),
        ))
        .await
        .expect("telemetry retry");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["total_requests"], 10);
    assert_eq!(body["total_errors"], 2);
    assert_eq!(body["warming_failures"], 1);

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
    let other_team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("other team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "secret-http@test", "Secret HTTP")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let query_pool = pool.clone();
    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
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
    let request_with_revision =
        |method: &str, path: &str, revision: i64, body: Option<serde_json::Value>| {
            let mut req = request(method, path, body);
            req.headers_mut().insert(
                axum::http::header::IF_MATCH,
                revision.to_string().parse().expect("revision header"),
            );
            req
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
    let secret_id = body["id"].as_str().expect("secret id").to_string();
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
        .clone()
        .oneshot(request_with_revision(
            "POST",
            &format!("{item}/rotate"),
            99,
            Some(serde_json::json!({
                "spec": {"type": "generic_secret", "secret": "d29ybGQ="}
            })),
        ))
        .await
        .expect("stale rotate secret");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_of(response).await["code"], "revision_mismatch");

    let providers = format!("/api/v1/teams/{}/ai/providers", team.name);
    let provider_name = unique("openai");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &providers,
            Some(serde_json::json!({
                "name": provider_name,
                "spec": {
                    "kind": "openai-compatible",
                    "base_url": "https://llm.example",
                    "path_prefix": "/v1",
                    "credential_secret_id": secret_id,
                    "models": ["gpt-5-mini"]
                }
            })),
        ))
        .await
        .expect("create AI provider");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let provider_id = body["id"].as_str().expect("provider id").to_string();
    assert_eq!(body["name"], provider_name);
    assert_eq!(body["spec"]["credential_secret_id"], secret_id);
    assert_eq!(body["revision"], 1);

    let other_providers = format!("/api/v1/teams/{}/ai/providers", other_team.name);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &other_providers,
            Some(serde_json::json!({
                "name": unique("openai"),
                "spec": {
                    "kind": "openai-compatible",
                    "base_url": "https://llm.example",
                    "credential_secret_id": secret_id,
                    "models": ["gpt-5-mini"]
                }
            })),
        ))
        .await
        .expect("cross-team AI provider secret");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let provider = format!("{providers}/{provider_name}");
    let response = app
        .clone()
        .oneshot(request_with_revision(
            "PATCH",
            &provider,
            1,
            Some(serde_json::json!({
                "spec": {
                    "kind": "openai",
                    "base_url": "https://api.openai.com",
                    "path_prefix": "/v1",
                    "credential_secret_id": secret_id,
                    "models": ["gpt-5"]
                }
            })),
        ))
        .await
        .expect("update AI provider");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_of(response).await["revision"], 2);

    let routes = format!("/api/v1/teams/{}/ai/routes", team.name);
    let route_name = unique("airoute");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &routes,
            Some(serde_json::json!({
                "name": route_name,
                "spec": {
                    "listener_port": 19081,
                    "backends": [{
                        "provider_id": provider_id,
                        "models": ["gpt-5"],
                        "weight": 1
                    }]
                }
            })),
        ))
        .await
        .expect("create AI route");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["name"], route_name);
    assert_eq!(body["status"], "active");
    assert_eq!(
        body["materialized"]["listener_name"],
        format!("ai-{route_name}-listener")
    );
    assert_eq!(body["revision"], 1);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/teams/{}/listeners/ai-{route_name}-listener",
                team.name
            ),
            None,
        ))
        .await
        .expect("get materialized listener");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/teams/{}/route-configs/ai-{route_name}-routes",
                team.name
            ),
            None,
        ))
        .await
        .expect("get materialized AI route config");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let (materialized_route_config_id, materialized_route_config): (
        uuid::Uuid,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT id, spec FROM route_configs WHERE team_id = $1 AND name = $2 AND owner_kind = 'ai'",
    )
    .bind(team.id.as_uuid())
    .bind(format!("ai-{route_name}-routes"))
    .fetch_one(&query_pool)
    .await
    .expect("managed AI route config");
    let ai_route_rules = materialized_route_config["virtual_hosts"][0]["routes"]
        .as_array()
        .expect("AI routes");
    let fallback = ai_route_rules
        .iter()
        .find(|route| route["name"] == "no-eligible-backend")
        .expect("fallback route");
    assert_eq!(fallback["action"]["direct_response"]["status"], 400);

    let budgets = format!("/api/v1/teams/{}/ai/budgets", team.name);
    let budget_name = unique("aibudget");
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &budgets,
            Some(serde_json::json!({
                "name": budget_name,
                "spec": {
                    "mode": "shadow",
                    "limit_units": 100,
                    "window_seconds": 3600,
                    "provider_id": provider_id,
                    "route_config_id": materialized_route_config_id,
                    "prompt_token_weight": 1,
                    "completion_token_weight": 2
                }
            })),
        ))
        .await
        .expect("create AI budget");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    assert_eq!(body["name"], budget_name);
    assert_eq!(body["revision"], 1);

    fp_storage::repos::ai::record_usage_event_and_settle_budgets(
        &query_pool,
        fp_storage::repos::ai::AiUsageEventInsert {
            team_id: team.id,
            route_config_id: fp_domain::RouteConfigId::from(materialized_route_config_id),
            provider_id: fp_domain::AiProviderId::from(
                uuid::Uuid::parse_str(&provider_id).expect("provider uuid"),
            ),
            backend_position: Some(0),
            usage: fp_domain::OpenAiTokenUsage {
                prompt_tokens: 3,
                completion_tokens: 4,
                total_tokens: 7,
            },
        },
    )
    .await
    .expect("record AI usage");

    let used_units: i64 =
        sqlx::query_scalar("SELECT used_units FROM ai_budget_counters WHERE team_id = $1")
            .bind(team.id.as_uuid())
            .fetch_one(&query_pool)
            .await
            .expect("budget counter");
    assert_eq!(used_units, 11);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/teams/{}/ai/usage", team.name),
            None,
        ))
        .await
        .expect("get AI usage");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body[0]["prompt_tokens"], 3);
    assert_eq!(body[0]["completion_tokens"], 4);
    assert_eq!(body[0]["total_tokens"], 7);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/teams/{}/ai/usage", other_team.name),
            None,
        ))
        .await
        .expect("get other-team AI usage");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(json_of(response)
        .await
        .as_array()
        .expect("usage")
        .is_empty());

    let response = app
        .clone()
        .oneshot(request_with_revision(
            "PATCH",
            &provider,
            2,
            Some(serde_json::json!({
                "spec": {
                    "kind": "openai",
                    "base_url": "https://api2.openai.example",
                    "path_prefix": "/v1",
                    "credential_secret_id": secret_id,
                    "models": ["gpt-5"]
                }
            })),
        ))
        .await
        .expect("update referenced AI provider");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_of(response).await["revision"], 3);

    let route = format!("{routes}/{route_name}");
    let response = app
        .clone()
        .oneshot(request("GET", &route, None))
        .await
        .expect("get stale AI route");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["status"], "stale");
    assert_eq!(body["revision"], 2);

    let response = app
        .clone()
        .oneshot(request_with_revision("DELETE", &provider, 3, None))
        .await
        .expect("delete referenced AI provider");
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .clone()
        .oneshot(request_with_revision("DELETE", &route, 2, None))
        .await
        .expect("delete AI route");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(request("GET", &providers, None))
        .await
        .expect("list AI providers");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(json_of(response).await["items"]
        .as_array()
        .expect("items")
        .iter()
        .any(|provider| provider["name"] == provider_name));

    let response = app
        .clone()
        .oneshot(request_with_revision("DELETE", &provider, 3, None))
        .await
        .expect("delete AI provider");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = json_of(response).await;
    assert!(body["message"]
        .as_str()
        .expect("message")
        .contains(&budget_name));

    let response = app
        .clone()
        .oneshot(request_with_revision(
            "DELETE",
            &format!("{budgets}/{budget_name}"),
            1,
            None,
        ))
        .await
        .expect("delete AI budget");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(request_with_revision("DELETE", &provider, 3, None))
        .await
        .expect("delete AI provider");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .oneshot(request_with_revision(
            "POST",
            &format!("{item}/rotate"),
            1,
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

// Regression for #138: a type-malformed JSON body must return the standard
// error envelope (validation_failed -> 400), not axum's bare plain-text 422.
#[tokio::test]
async fn malformed_json_body_returns_validation_envelope() {
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
        .mint(&subject, "malformed@test", "Malformed", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "malformed@test", "Malformed")
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
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });

    // `port` typed as a string -> JSON deserialization failure.
    let bad = r#"{"name":"x","spec":{"endpoints":[{"host":"10.0.0.1","port":"oops"}]}}"#;
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/teams/{}/clusters", team.name))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(bad))
        .expect("request");

    let response = app.oneshot(request).await.expect("send");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_of(response).await;
    assert_eq!(body["code"], "validation_failed");
    assert!(
        body.get("request_id").and_then(|v| v.as_str()).is_some(),
        "envelope must carry request_id, got: {body}"
    );
}

// Slice s4 (ai-gateway-e2e-trace): team-scoped AI trace retrieval over HTTP through the
// real middleware stack — correlated hop timeline on a hit, a distinguishable miss with
// the never-traced-classes hint, cross-org 404, and missing-grant 403.
#[tokio::test]
async fn ai_trace_retrieval_over_http() {
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

    // Org A: an admin (implicit team access) and a member holding only an unrelated grant.
    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let team = identity::create_team(&pool, org_a.id, &unique("team"), "")
        .await
        .expect("team");
    let admin_sub = unique("sub-admin");
    let admin_token = issuer
        .mint(&admin_sub, "trace-admin@test", "Trace Admin", 600)
        .expect("mint admin");
    let admin = identity::upsert_user_by_subject(&pool, &admin_sub, "trace-admin@test", "A")
        .await
        .expect("admin");
    identity::add_org_membership(&pool, admin, org_a.id, OrgRole::Admin)
        .await
        .expect("admin member");

    let member_sub = unique("sub-member");
    let member_token = issuer
        .mint(&member_sub, "trace-member@test", "Trace Member", 600)
        .expect("mint member");
    let member = identity::upsert_user_by_subject(&pool, &member_sub, "trace-member@test", "M")
        .await
        .expect("member");
    identity::add_org_membership(&pool, member, org_a.id, OrgRole::Member)
        .await
        .expect("member membership");
    identity::add_grant(
        &pool,
        member,
        org_a.id,
        team.id,
        fp_domain::authz::Resource::AiProviders,
        fp_domain::authz::Action::Read,
        None,
    )
    .await
    .expect("unrelated grant");

    // Org B: an admin of a different org (cross-org caller).
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let outsider_sub = unique("sub-outsider");
    let outsider_token = issuer
        .mint(&outsider_sub, "trace-outsider@test", "Outsider", 600)
        .expect("mint outsider");
    let outsider =
        identity::upsert_user_by_subject(&pool, &outsider_sub, "trace-outsider@test", "O")
            .await
            .expect("outsider");
    identity::add_org_membership(&pool, outsider, org_b.id, OrgRole::Admin)
        .await
        .expect("outsider member");

    // Seed one trace row for the team (the write path is slice s2's; here it is fixture data).
    let request_id = uuid::Uuid::now_v7().to_string();
    fp_storage::repos::ai_trace::upsert_trace_event(
        &pool,
        &fp_storage::repos::ai_trace::AiTraceEventUpsert {
            team_id: team.id,
            request_id: request_id.clone(),
            trace_id: Some("0af7651916cd43dd8448eb211c80319c".into()),
            route_config_id: fp_domain::RouteConfigId::from(uuid::Uuid::now_v7()),
            listener_id: None,
            provider_id: None,
            model: Some("gpt-5".into()),
            status_code: Some(200),
            hops: serde_json::json!([
                {"hop": "route_match", "started_at": "2026-07-04T00:00:00.100Z",
                 "ended_at": "2026-07-04T00:00:00.200Z", "outcome": "matched",
                 "origin": "listener", "failed": false, "detail": {}},
                {"hop": "upstream", "started_at": "2026-07-04T00:00:00.300Z",
                 "ended_at": "2026-07-04T00:00:00.900Z", "outcome": "ok",
                 "origin": "upstream", "failed": false, "detail": {"status": 200}}
            ]),
        },
    )
    .await
    .expect("seed trace row");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    let request = |token: &str, path: &str| {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request")
    };

    // Hit: correlated hop timeline, no miss payload.
    let response = app
        .clone()
        .oneshot(request(
            &admin_token,
            &format!(
                "/api/v1/teams/{}/ai/trace?request_id={request_id}",
                team.name
            ),
        ))
        .await
        .expect("trace hit");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    let traces = body["traces"].as_array().expect("traces");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0]["request_id"], request_id);
    assert_eq!(traces[0]["trace_id"], "0af7651916cd43dd8448eb211c80319c");
    let hops = traces[0]["hops"].as_array().expect("hops");
    assert_eq!(hops[0]["hop"], "route_match");
    assert_eq!(hops[1]["hop"], "upstream");
    assert!(
        body.get("miss").is_none(),
        "hit must not carry a miss: {body}"
    );

    // trace_id filter returns the same row.
    let response = app
        .clone()
        .oneshot(request(
            &admin_token,
            &format!(
                "/api/v1/teams/{}/ai/trace?trace_id=0af7651916cd43dd8448eb211c80319c&limit=5",
                team.name
            ),
        ))
        .await
        .expect("trace by trace_id");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert!(body["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .any(|t| t["request_id"] == request_id.as_str()));

    // Miss: unknown request_id → distinguishable "no trace row found" with the hint naming
    // the never-traced classes (design Risk 5).
    let response = app
        .clone()
        .oneshot(request(
            &admin_token,
            &format!(
                "/api/v1/teams/{}/ai/trace?request_id={}",
                team.name,
                uuid::Uuid::now_v7()
            ),
        ))
        .await
        .expect("trace miss");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert!(body["traces"].as_array().expect("traces").is_empty());
    assert_eq!(body["miss"]["message"], "no trace row found");
    let hint = body["miss"]["hint"].as_str().expect("miss hint");
    for class in [
        "TCP/TLS-level failures",
        "client disconnect before request headers",
        "pre-ExtProc declared-filter denials",
    ] {
        assert!(hint.contains(class), "hint must name {class:?}: {hint}");
    }

    // Cross-org: an org-B token querying team A's request_id gets 404 (org-boundary
    // mapping) and zero rows — denied in the service before any repo read.
    let response = app
        .clone()
        .oneshot(request(
            &outsider_token,
            &format!(
                "/api/v1/teams/{}/ai/trace?request_id={request_id}",
                team.name
            ),
        ))
        .await
        .expect("cross-org trace");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = json_of(response).await;
    assert!(
        body.get("traces").is_none(),
        "denial must return no rows: {body}"
    );

    // Missing grant: a same-org member with other team grants but without (ai-usage, read)
    // gets 403 naming the missing grant.
    let response = app
        .clone()
        .oneshot(request(
            &member_token,
            &format!(
                "/api/v1/teams/{}/ai/trace?request_id={request_id}",
                team.name
            ),
        ))
        .await
        .expect("missing-grant trace");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_of(response).await;
    assert_eq!(body["code"], "forbidden");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("ai-usage:read"),
        "403 must name the missing grant: {body}"
    );
}

// Slice s5 (ai-gateway-e2e-trace): retention policy surface over HTTP through the real
// middleware stack — GET enforces (ai-usage, read), PUT enforces (ai-usage, update) with the
// budget-CRUD mutation shape (authorize, validate, tx, audit row), cross-org 404, and the
// default-vs-stored policy view.
#[tokio::test]
async fn ai_retention_crud_authz_and_audit_over_http() {
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

    // Org A: an admin, a reader holding only (ai-usage, read), and a member holding only an
    // unrelated grant.
    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let team = identity::create_team(&pool, org_a.id, &unique("team"), "")
        .await
        .expect("team");
    let admin_sub = unique("sub-admin");
    let admin_token = issuer
        .mint(&admin_sub, "ret-admin@test", "Retention Admin", 600)
        .expect("mint admin");
    let admin = identity::upsert_user_by_subject(&pool, &admin_sub, "ret-admin@test", "A")
        .await
        .expect("admin");
    identity::add_org_membership(&pool, admin, org_a.id, OrgRole::Admin)
        .await
        .expect("admin member");

    let reader_sub = unique("sub-reader");
    let reader_token = issuer
        .mint(&reader_sub, "ret-reader@test", "Retention Reader", 600)
        .expect("mint reader");
    let reader = identity::upsert_user_by_subject(&pool, &reader_sub, "ret-reader@test", "R")
        .await
        .expect("reader");
    identity::add_org_membership(&pool, reader, org_a.id, OrgRole::Member)
        .await
        .expect("reader membership");
    identity::add_grant(
        &pool,
        reader,
        org_a.id,
        team.id,
        fp_domain::authz::Resource::AiUsage,
        fp_domain::authz::Action::Read,
        None,
    )
    .await
    .expect("reader grant");

    let member_sub = unique("sub-member");
    let member_token = issuer
        .mint(&member_sub, "ret-member@test", "Retention Member", 600)
        .expect("mint member");
    let member = identity::upsert_user_by_subject(&pool, &member_sub, "ret-member@test", "M")
        .await
        .expect("member");
    identity::add_org_membership(&pool, member, org_a.id, OrgRole::Member)
        .await
        .expect("member membership");
    identity::add_grant(
        &pool,
        member,
        org_a.id,
        team.id,
        fp_domain::authz::Resource::AiProviders,
        fp_domain::authz::Action::Read,
        None,
    )
    .await
    .expect("unrelated grant");

    // Org B: an admin of a different org (cross-org caller).
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let outsider_sub = unique("sub-outsider");
    let outsider_token = issuer
        .mint(&outsider_sub, "ret-outsider@test", "Outsider", 600)
        .expect("mint outsider");
    let outsider = identity::upsert_user_by_subject(&pool, &outsider_sub, "ret-outsider@test", "O")
        .await
        .expect("outsider");
    identity::add_org_membership(&pool, outsider, org_b.id, OrgRole::Admin)
        .await
        .expect("outsider member");

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    let path = format!("/api/v1/teams/{}/ai/retention", team.name);
    let get = |token: &str| {
        Request::builder()
            .method("GET")
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .expect("get request")
    };
    let put = |token: &str, body: serde_json::Value| {
        Request::builder()
            .method("PUT")
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("put request")
    };
    let audit_rows = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_log \
             WHERE team_id = $1 AND action = 'ai_retention.set' AND outcome = 'success'",
        )
        .bind(team.id.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("audit count")
    };

    // No policy row yet: GET reports the built-in 30-day default.
    let response = app
        .clone()
        .oneshot(get(&admin_token))
        .await
        .expect("get default");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["trace_ttl_days"], 30);
    assert_eq!(body["is_default"], true);
    assert!(body.get("revision").is_none(), "default has no revision");

    // PUT creates the policy (mutation shape: authorize, validate, tx, audit row).
    let response = app
        .clone()
        .oneshot(put(&admin_token, serde_json::json!({"trace_ttl_days": 7})))
        .await
        .expect("put create");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["trace_ttl_days"], 7);
    assert_eq!(body["is_default"], false);
    assert_eq!(body["revision"], 1);
    assert_eq!(
        audit_rows().await,
        1,
        "PUT must commit exactly one audit row"
    );

    // GET now reads the stored policy.
    let response = app
        .clone()
        .oneshot(get(&admin_token))
        .await
        .expect("get stored");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["trace_ttl_days"], 7);
    assert_eq!(body["is_default"], false);

    // PUT replaces (one policy per team) and bumps the revision; second audit row.
    let response = app
        .clone()
        .oneshot(put(&admin_token, serde_json::json!({"trace_ttl_days": 14})))
        .await
        .expect("put replace");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["trace_ttl_days"], 14);
    assert_eq!(body["revision"], 2);
    assert_eq!(audit_rows().await, 2);

    // Validation fails closed BEFORE any write: out-of-bounds TTLs are rejected and leave
    // no audit row behind.
    for bad_ttl in [0, -1, 366] {
        let response = app
            .clone()
            .oneshot(put(
                &admin_token,
                serde_json::json!({"trace_ttl_days": bad_ttl}),
            ))
            .await
            .expect("put invalid");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "trace_ttl_days {bad_ttl} must be rejected"
        );
    }
    assert_eq!(audit_rows().await, 2, "rejected PUTs must not audit");

    // (ai-usage, read) alone: GET allowed, PUT denied naming the update grant.
    let response = app
        .clone()
        .oneshot(get(&reader_token))
        .await
        .expect("reader get");
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(put(&reader_token, serde_json::json!({"trace_ttl_days": 5})))
        .await
        .expect("reader put");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_of(response).await;
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("ai-usage:update"),
        "403 must name the missing update grant: {body}"
    );

    // Unrelated grant only: GET denied naming the read grant.
    let response = app
        .clone()
        .oneshot(get(&member_token))
        .await
        .expect("member get");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_of(response).await;
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("ai-usage:read"),
        "403 must name the missing read grant: {body}"
    );

    // Cross-org: 404 per the org-boundary mapping, for both verbs.
    let response = app
        .clone()
        .oneshot(get(&outsider_token))
        .await
        .expect("outsider get");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(put(
            &outsider_token,
            serde_json::json!({"trace_ttl_days": 5}),
        ))
        .await
        .expect("outsider put");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // The stored policy is untouched by every denied/rejected attempt above.
    let ttl: i32 =
        sqlx::query_scalar("SELECT trace_ttl_days FROM ai_retention_policies WHERE team_id = $1")
            .bind(team.id.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("policy row");
    assert_eq!(ttl, 14);

    // #226 optimistic concurrency (If-Match). The policy is at revision 2 / ttl 14 here,
    // untouched by all the rejected attempts above.
    let put_if_match = |token: &str, body: serde_json::Value, revision: i64| {
        Request::builder()
            .method("PUT")
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("if-match", revision.to_string())
            .body(Body::from(body.to_string()))
            .expect("put if-match request")
    };
    // A stale revision (1, current is 2) is rejected with 409 and writes nothing.
    let response = app
        .clone()
        .oneshot(put_if_match(
            &admin_token,
            serde_json::json!({"trace_ttl_days": 21}),
            1,
        ))
        .await
        .expect("put stale");
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "a stale If-Match must be rejected, not silently last-writer-win"
    );
    assert_eq!(
        audit_rows().await,
        2,
        "a rejected stale write commits no audit row"
    );
    let response = app
        .clone()
        .oneshot(get(&admin_token))
        .await
        .expect("get after stale");
    assert_eq!(
        json_of(response).await["trace_ttl_days"],
        14,
        "the stale write left the stored policy untouched"
    );
    // The current revision (2) succeeds and bumps to 3.
    let response = app
        .clone()
        .oneshot(put_if_match(
            &admin_token,
            serde_json::json!({"trace_ttl_days": 21}),
            2,
        ))
        .await
        .expect("put current revision");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["trace_ttl_days"], 21);
    assert_eq!(body["revision"], 3);
    assert_eq!(audit_rows().await, 3);
    // Omitting If-Match still creates-or-replaces (the revision is optional), bumping to 4.
    let response = app
        .clone()
        .oneshot(put(&admin_token, serde_json::json!({"trace_ttl_days": 28})))
        .await
        .expect("put without if-match");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_of(response).await["revision"], 4);

    // A present-but-malformed If-Match is a validation error, NOT treated as absent — it must
    // never fall through to a silent create-or-replace. Policy stays at revision 4.
    let malformed = Request::builder()
        .method("PUT")
        .uri(&path)
        .header("authorization", format!("Bearer {admin_token}"))
        .header("content-type", "application/json")
        .header("if-match", "not-a-revision")
        .body(Body::from(
            serde_json::json!({"trace_ttl_days": 9}).to_string(),
        ))
        .expect("malformed if-match request");
    let response = app
        .clone()
        .oneshot(malformed)
        .await
        .expect("put malformed if-match");
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "a present-but-malformed If-Match must be rejected, not treated as absent"
    );
    let response = app
        .clone()
        .oneshot(get(&admin_token))
        .await
        .expect("get after malformed");
    assert_eq!(
        json_of(response).await["revision"],
        4,
        "the malformed-If-Match write must not have changed the policy"
    );
}
