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
    // + 3 proxy-certificate + 1 xds-nacks operations.
    // + 4 secrets operations + 2 dataplane/stats telemetry operations.
    // Updating this pin is a deliberate speed bump when the surface changes: the doc IS
    // the contract.
    assert_eq!(
        operations, 46,
        "expected 46 documented operations, got {operations}"
    );
    assert!(json["components"]["securitySchemes"]["bearerAuth"].is_object());
    for path in [
        "/api/v1/teams/{team}/clusters",
        "/api/v1/teams/{team}/route-configs/{name}",
    ] {
        assert!(paths.contains_key(path), "missing {path}");
    }
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
    assert!(body.contains("cluster_name: xds_cluster"));
    assert!(body.contains("filename: \"/certs/client.crt\""));
    assert!(body.contains("filename: \"/certs/client.key\""));
    assert!(body.contains("filename: \"/certs/ca.crt\""));

    let certs = format!("/api/v1/teams/{}/proxy-certificates", team.name);
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
