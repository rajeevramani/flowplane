use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use tower::ServiceExt;

use super::support::{read_json, setup_platform_api_app};

#[tokio::test]
async fn import_openapi_json_creates_api_definition() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("api-importer", &["api-definitions:write"]).await;

    let openapi_spec = r#"{
        "openapi": "3.0.0",
        "info": {
            "title": "Payment API",
            "version": "1.0.0"
        },
        "servers": [
            {"url": "https://api.payments.com:8443"}
        ],
        "paths": {
            "/v1/transactions": {
                "get": {
                    "summary": "List transactions",
                    "responses": {
                        "200": {"description": "Success"}
                    }
                }
            },
            "/v1/accounts": {
                "post": {
                    "summary": "Create account",
                    "responses": {
                        "201": {"description": "Created"}
                    }
                }
            }
        }
    }"#;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/api-definitions/from-openapi?team=payments&listenerIsolation=false")
        .header(header::AUTHORIZATION, format!("Bearer {}", token.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(openapi_spec))
        .expect("build request");

    let response = app.router().oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = read_json(response).await;

    // Verify response structure
    assert!(body.get("id").and_then(|v| v.as_str()).is_some(), "missing id");
    assert!(body.get("bootstrapUri").and_then(|v| v.as_str()).is_some(), "missing bootstrapUri");
    let routes = body.get("routes").and_then(|v| v.as_array()).expect("missing routes array");
    assert_eq!(routes.len(), 2, "should create 2 routes from 2 OpenAPI paths");

    // Verify database persistence
    let definition_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_definitions")
        .fetch_one(&app.pool)
        .await
        .expect("count api definitions");
    assert_eq!(definition_count, 1);

    let stored_domain: String =
        sqlx::query_scalar("SELECT domain FROM api_definitions WHERE team = 'payments'")
            .fetch_one(&app.pool)
            .await
            .expect("fetch domain");
    assert_eq!(stored_domain, "api.payments.com");

    // Verify routes were created
    let route_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_routes")
        .fetch_one(&app.pool)
        .await
        .expect("count routes");
    assert_eq!(route_count, 2);

    // Verify routes have correct paths
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT match_value FROM api_routes ORDER BY route_order")
            .fetch_all(&app.pool)
            .await
            .expect("fetch paths");
    assert!(paths.contains(&"/v1/transactions".to_string()), "should have /v1/transactions route");
    assert!(paths.contains(&"/v1/accounts".to_string()), "should have /v1/accounts route");
}

#[tokio::test]
async fn import_openapi_yaml_creates_api_definition() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("yaml-importer", &["api-definitions:write"]).await;

    let openapi_spec = r#"
openapi: 3.0.0
info:
  title: User Service
  version: 2.0.0
servers:
  - url: http://users.internal.svc:8080
paths:
  /users:
    get:
      summary: List users
      responses:
        '200':
          description: OK
  /users/{id}:
    get:
      summary: Get user
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
      responses:
        '200':
          description: OK
"#;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/api-definitions/from-openapi?team=identity&listenerIsolation=true")
        .header(header::AUTHORIZATION, format!("Bearer {}", token.token))
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(openapi_spec))
        .expect("build request");

    let response = app.router().oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = read_json(response).await;

    assert!(body.get("id").is_some());
    let routes = body.get("routes").and_then(|v| v.as_array()).expect("routes array");
    assert_eq!(routes.len(), 2);

    // Verify listener isolation was enabled
    let listener_isolation: bool = sqlx::query_scalar(
        "SELECT listener_isolation FROM api_definitions WHERE team = 'identity'",
    )
    .fetch_one(&app.pool)
    .await
    .expect("fetch listener_isolation");
    assert!(listener_isolation, "listener_isolation should be true");

    // Verify listener was created (name would be auto-generated as "platform-{short_id}-listener")
    let listener_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM listeners WHERE name LIKE 'platform-%'")
            .fetch_one(&app.pool)
            .await
            .expect("count listeners");
    assert!(listener_count >= 1, "isolated listener should be created");
}

#[tokio::test]
async fn import_openapi_rejects_empty_body() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("test-token", &["api-definitions:write"]).await;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/api-definitions/from-openapi?team=test")
        .header(header::AUTHORIZATION, format!("Bearer {}", token.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .expect("build request");

    let response = app.router().oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_openapi_rejects_invalid_spec() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("bad-spec-importer", &["api-definitions:write"]).await;

    let invalid_spec = r#"{
        "openapi": "3.0.0",
        "info": {"title": "Bad API", "version": "1.0.0"},
        "servers": [],
        "paths": {}
    }"#;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/api-definitions/from-openapi?team=bad")
        .header(header::AUTHORIZATION, format!("Bearer {}", token.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(invalid_spec))
        .expect("build request");

    let response = app.router().oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_openapi_with_base_path_combines_paths() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("basepath-tester", &["api-definitions:write"]).await;

    let openapi_spec = r#"{
        "openapi": "3.0.0",
        "info": {"title": "Versioned API", "version": "1.0.0"},
        "servers": [{"url": "https://api.example.com/v2"}],
        "paths": {
            "/health": {
                "get": {
                    "responses": {"200": {"description": "OK"}}
                }
            }
        }
    }"#;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/api-definitions/from-openapi?team=versioned")
        .header(header::AUTHORIZATION, format!("Bearer {}", token.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(openapi_spec))
        .expect("build request");

    let response = app.router().oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::CREATED);

    // Verify the path was combined correctly
    let stored_path: String = sqlx::query_scalar("SELECT match_value FROM api_routes")
        .fetch_one(&app.pool)
        .await
        .expect("fetch route path");
    assert_eq!(stored_path, "/v2/health", "base path should be combined with route path");
}
