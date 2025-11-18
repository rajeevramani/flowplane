//! Integration tests for Native API cross-team resource access prevention
//!
//! These tests verify that team-scoped users cannot access or discover resources
//! belonging to other teams through the Native API HTTP endpoints.

use axum::http::{Method, StatusCode};
use serde_json::json;
use std::sync::Arc;

use axum::{body::to_bytes, body::Body, http::Request, Router};
use flowplane::{
    auth::{
        token_service::{TokenSecretResponse, TokenService},
        validation::CreateTokenRequest,
    },
    config::SimpleXdsConfig,
    storage::{self, repository::AuditLogRepository, DbPool},
    xds::XdsState,
};
use hyper::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

// === Test Infrastructure ===

pub struct NativeApiApp {
    state: Arc<XdsState>,
    pub pool: DbPool,
    token_service: TokenService,
}

impl NativeApiApp {
    pub fn router(&self) -> Router {
        flowplane::api::routes::build_router(self.state.clone())
    }

    pub async fn issue_token(&self, name: &str, scopes: &[&str]) -> TokenSecretResponse {
        self.token_service
            .create_token(CreateTokenRequest::without_user(
                name.to_string(),
                None,
                None,
                scopes.iter().map(|scope| scope.to_string()).collect(),
                Some("native-api-cross-team-tests".into()),
            ))
            .await
            .expect("create token")
    }
}

pub async fn setup_native_api_app() -> NativeApiApp {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    storage::run_migrations(&pool).await.expect("run migrations for tests");

    // Create teams required for cross-team tests
    let team_id_a = uuid::Uuid::new_v4().to_string();
    let team_id_b = uuid::Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, $4)")
        .bind(&team_id_a)
        .bind("team-a")
        .bind("Team A")
        .bind("active")
        .execute(&pool)
        .await
        .expect("create team-a");

    sqlx::query("INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, $4)")
        .bind(&team_id_b)
        .bind("team-b")
        .bind("Team B")
        .bind("active")
        .execute(&pool)
        .await
        .expect("create team-b");

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));
    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
    let token_service = TokenService::with_sqlx(pool.clone(), audit_repo);

    NativeApiApp { state, pool, token_service }
}

pub async fn send_request(
    app: &NativeApiApp,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }

    let request = if let Some(json) = body {
        let bytes = serde_json::to_vec(&json).expect("serialize body");
        builder
            .header("content-type", "application/json")
            .body(Body::from(bytes))
            .expect("build request")
    } else {
        builder.body(Body::empty()).expect("build request")
    };

    app.router().oneshot(request).await.expect("request")
}

pub async fn read_json<T: DeserializeOwned>(response: Response<Body>) -> T {
    let bytes =
        to_bytes(response.into_body(), usize::MAX).await.expect("read response body as bytes");
    serde_json::from_slice(&bytes).expect("parse json response")
}

// === Cluster Cross-Team Access Tests ===

#[tokio::test]
async fn team_a_cannot_list_team_b_clusters() {
    let app = setup_native_api_app().await;

    // Create tokens for two different teams
    let team_a_token = app
        .issue_token("team-a-user", &["team:team-a:clusters:read", "team:team-a:clusters:write"])
        .await;
    let team_b_token = app
        .issue_token("team-b-user", &["team:team-b:clusters:read", "team:team-b:clusters:write"])
        .await;

    // Team B creates a cluster
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-backend",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);

    // Team A lists clusters - should NOT see Team B's cluster
    let list_response =
        send_request(&app, Method::GET, "/api/v1/clusters", Some(&team_a_token.token), None).await;
    assert_eq!(list_response.status(), StatusCode::OK);

    let clusters: Vec<Value> = read_json(list_response).await;
    assert!(
        !clusters.iter().any(|c| c["name"] == "team-b-backend"),
        "Team A should not see Team B's cluster in list"
    );
}

#[tokio::test]
async fn team_a_cannot_get_team_b_cluster() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:clusters:read"]).await;
    let team_b_token = app
        .issue_token("team-b-user", &["team:team-b:clusters:read", "team:team-b:clusters:write"])
        .await;

    // Team B creates a cluster
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-private",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);

    // Team A tries to get Team B's cluster by name
    let get_response = send_request(
        &app,
        Method::GET,
        "/api/v1/clusters/team-b-private",
        Some(&team_a_token.token),
        None,
    )
    .await;

    // Should return 404 (not 403) to avoid leaking existence
    assert_eq!(
        get_response.status(),
        StatusCode::NOT_FOUND,
        "Should return 404 to avoid leaking existence of Team B's resource"
    );
}

#[tokio::test]
async fn team_a_cannot_update_team_b_cluster() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:clusters:write"]).await;
    let team_b_token = app.issue_token("team-b-user", &["team:team-b:clusters:write"]).await;

    // Team B creates a cluster
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-cluster",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });

    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;

    // Team A tries to update Team B's cluster
    let update_payload = json!({
        "team": "team-a",
        "name": "team-b-cluster",
        "serviceName": "malicious-service",
        "endpoints": [{"host": "attacker.com", "port": 9999}],
        "connectTimeoutSeconds": 1
    });

    let update_response = send_request(
        &app,
        Method::PUT,
        "/api/v1/clusters/team-b-cluster",
        Some(&team_a_token.token),
        Some(update_payload),
    )
    .await;

    // Should return 404 (not 403)
    assert_eq!(update_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_a_cannot_delete_team_b_cluster() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:clusters:write"]).await;
    let team_b_token = app
        .issue_token("team-b-user", &["team:team-b:clusters:read", "team:team-b:clusters:write"])
        .await;

    // Team B creates a cluster
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-important",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });

    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;

    // Team A tries to delete Team B's cluster
    let delete_response = send_request(
        &app,
        Method::DELETE,
        "/api/v1/clusters/team-b-important",
        Some(&team_a_token.token),
        None,
    )
    .await;

    // Should return 404 (not 403)
    assert_eq!(delete_response.status(), StatusCode::NOT_FOUND);

    // Verify Team B can still access their cluster
    let verify_response = send_request(
        &app,
        Method::GET,
        "/api/v1/clusters/team-b-important",
        Some(&team_b_token.token),
        None,
    )
    .await;
    assert_eq!(verify_response.status(), StatusCode::OK);
}

// === Route Cross-Team Access Tests ===

#[tokio::test]
async fn team_a_cannot_list_team_b_routes() {
    let app = setup_native_api_app().await;

    let team_a_token = app
        .issue_token(
            "team-a-user",
            &["team:team-a:routes:read", "team:team-a:routes:write", "team:team-a:clusters:write"],
        )
        .await;
    let team_b_token = app
        .issue_token(
            "team-b-user",
            &["team:team-b:routes:read", "team:team-b:routes:write", "team:team-b:clusters:write"],
        )
        .await;

    // Team B creates a cluster first (FK requirement)
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-backend",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });
    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;

    // Team B creates a route
    let team_b_route = json!({
        "team": "team-b",
        "name": "team-b-route",
        "virtualHosts": [{
            "name": "default",
            "domains": ["*"],
            "routes": [{
                "name": "team-b",
                "match": {"path": {"type": "prefix", "value": "/team-b"}},
                "action": {"type": "forward", "cluster": "team-b-backend"}
            }]
        }]
    });

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/routes",
        Some(&team_b_token.token),
        Some(team_b_route),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);

    // Team A lists routes - should NOT see Team B's route
    let list_response =
        send_request(&app, Method::GET, "/api/v1/routes", Some(&team_a_token.token), None).await;
    assert_eq!(list_response.status(), StatusCode::OK);

    let routes: Vec<Value> = read_json(list_response).await;
    assert!(
        !routes.iter().any(|r| r["name"] == "team-b-route"),
        "Team A should not see Team B's route in list"
    );
}

#[tokio::test]
async fn team_a_cannot_get_team_b_route() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:routes:read"]).await;
    let team_b_token = app
        .issue_token("team-b-user", &["team:team-b:routes:write", "team:team-b:clusters:write"])
        .await;

    // Team B creates cluster and route
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-svc",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });
    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;

    let team_b_route = json!({
        "team": "team-b",
        "name": "team-b-private-route",
        "virtualHosts": [{
            "name": "default",
            "domains": ["*"],
            "routes": [{
                "name": "team-b-private",
                "match": {"path": {"type": "prefix", "value": "/private"}},
                "action": {"type": "forward", "cluster": "team-b-svc"}
            }]
        }]
    });

    send_request(
        &app,
        Method::POST,
        "/api/v1/routes",
        Some(&team_b_token.token),
        Some(team_b_route),
    )
    .await;

    // Team A tries to get Team B's route
    let get_response = send_request(
        &app,
        Method::GET,
        "/api/v1/routes/team-b-private-route",
        Some(&team_a_token.token),
        None,
    )
    .await;

    // Should return 404 (not 403)
    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_a_cannot_delete_team_b_route() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:routes:write"]).await;
    let team_b_token = app
        .issue_token("team-b-user", &["team:team-b:routes:write", "team:team-b:clusters:write"])
        .await;

    // Team B creates cluster and route
    let team_b_cluster = json!({
        "team": "team-b",
        "name": "team-b-backend",
        "serviceName": "team-b-service",
        "endpoints": [{"host": "team-b.local", "port": 8080}],
        "connectTimeoutSeconds": 5
    });
    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(team_b_cluster),
    )
    .await;

    let team_b_route = json!({
        "team": "team-b",
        "name": "team-b-route",
        "virtualHosts": [{
            "name": "default",
            "domains": ["*"],
            "routes": [{
                "name": "team-b-api",
                "match": {"path": {"type": "prefix", "value": "/api"}},
                "action": {"type": "forward", "cluster": "team-b-backend"}
            }]
        }]
    });

    send_request(
        &app,
        Method::POST,
        "/api/v1/routes",
        Some(&team_b_token.token),
        Some(team_b_route),
    )
    .await;

    // Team A tries to delete Team B's route
    let delete_response = send_request(
        &app,
        Method::DELETE,
        "/api/v1/routes/team-b-route",
        Some(&team_a_token.token),
        None,
    )
    .await;

    // Should return 404 (not 403)
    assert_eq!(delete_response.status(), StatusCode::NOT_FOUND);
}

// === Listener Cross-Team Access Tests ===

#[tokio::test]
async fn team_a_cannot_list_team_b_listeners() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:listeners:read"]).await;
    let team_b_token = app
        .issue_token("team-b-user", &["team:team-b:listeners:read", "team:team-b:listeners:write"])
        .await;

    // Team B creates a listener
    let team_b_listener = json!({
        "team": "team-b",
        "name": "team-b-listener",
        "address": "0.0.0.0",
        "port": 9090,
        "filterChains": [{
            "filters": [{
                "name": "envoy.filters.network.tcp_proxy",
                "type": "tcpProxy",
                "cluster": "team-b-backend"
            }]
        }]
    });

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/listeners",
        Some(&team_b_token.token),
        Some(team_b_listener),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);

    // Team A lists listeners - should NOT see Team B's listener
    let list_response =
        send_request(&app, Method::GET, "/api/v1/listeners", Some(&team_a_token.token), None).await;
    assert_eq!(list_response.status(), StatusCode::OK);

    let listeners: Vec<Value> = read_json(list_response).await;
    assert!(
        !listeners.iter().any(|l| l["name"] == "team-b-listener"),
        "Team A should not see Team B's listener in list"
    );
}

#[tokio::test]
async fn team_a_cannot_get_team_b_listener() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:listeners:read"]).await;
    let team_b_token = app.issue_token("team-b-user", &["team:team-b:listeners:write"]).await;

    // Team B creates a listener
    let team_b_listener = json!({
        "team": "team-b",
        "name": "team-b-private-listener",
        "address": "127.0.0.1",
        "port": 8888,
        "filterChains": [{
            "filters": [{
                "name": "envoy.filters.network.tcp_proxy",
                "type": "tcpProxy",
                "cluster": "team-b-svc"
            }]
        }]
    });

    send_request(
        &app,
        Method::POST,
        "/api/v1/listeners",
        Some(&team_b_token.token),
        Some(team_b_listener),
    )
    .await;

    // Team A tries to get Team B's listener
    let get_response = send_request(
        &app,
        Method::GET,
        "/api/v1/listeners/team-b-private-listener",
        Some(&team_a_token.token),
        None,
    )
    .await;

    // Should return 404 (not 403)
    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_a_cannot_delete_team_b_listener() {
    let app = setup_native_api_app().await;

    let team_a_token = app.issue_token("team-a-user", &["team:team-a:listeners:write"]).await;
    let team_b_token = app.issue_token("team-b-user", &["team:team-b:listeners:write"]).await;

    // Team B creates a listener
    let team_b_listener = json!({
        "team": "team-b",
        "name": "team-b-critical-listener",
        "address": "0.0.0.0",
        "port": 7777,
        "filterChains": [{
            "filters": [{
                "name": "envoy.filters.network.tcp_proxy",
                "type": "tcpProxy",
                "cluster": "team-b-backend"
            }]
        }]
    });

    send_request(
        &app,
        Method::POST,
        "/api/v1/listeners",
        Some(&team_b_token.token),
        Some(team_b_listener),
    )
    .await;

    // Team A tries to delete Team B's listener
    let delete_response = send_request(
        &app,
        Method::DELETE,
        "/api/v1/listeners/team-b-critical-listener",
        Some(&team_a_token.token),
        None,
    )
    .await;

    // Should return 404 (not 403)
    assert_eq!(delete_response.status(), StatusCode::NOT_FOUND);
}

// === Admin Resource Accessibility Tests ===

#[tokio::test]
async fn admin_users_can_access_all_team_resources() {
    let app = setup_native_api_app().await;

    // Team A and Team B create clusters
    let team_a_token = app.issue_token("team-a-user", &["team:team-a:clusters:write"]).await;
    let team_b_token = app.issue_token("team-b-user", &["team:team-b:clusters:write"]).await;

    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_a_token.token),
        Some(json!({
            "team": "team-a",
            "name": "team-a-cluster",
            "serviceName": "team-a-service",
            "endpoints": [{"host": "team-a.local", "port": 8080}],
            "connectTimeoutSeconds": 5
        })),
    )
    .await;

    send_request(
        &app,
        Method::POST,
        "/api/v1/clusters",
        Some(&team_b_token.token),
        Some(json!({
            "team": "team-b",
            "name": "team-b-cluster",
            "serviceName": "team-b-service",
            "endpoints": [{"host": "team-b.local", "port": 8080}],
            "connectTimeoutSeconds": 5
        })),
    )
    .await;

    // Admin should see both clusters
    let admin_token = app.issue_token("admin", &["admin:all"]).await;

    let list_response =
        send_request(&app, Method::GET, "/api/v1/clusters", Some(&admin_token.token), None).await;
    assert_eq!(list_response.status(), StatusCode::OK);

    let clusters: Vec<Value> = read_json(list_response).await;
    assert!(clusters.iter().any(|c| c["name"] == "team-a-cluster"));
    assert!(clusters.iter().any(|c| c["name"] == "team-b-cluster"));

    // Admin should be able to get specific team clusters
    let get_team_a = send_request(
        &app,
        Method::GET,
        "/api/v1/clusters/team-a-cluster",
        Some(&admin_token.token),
        None,
    )
    .await;
    assert_eq!(get_team_a.status(), StatusCode::OK);

    let get_team_b = send_request(
        &app,
        Method::GET,
        "/api/v1/clusters/team-b-cluster",
        Some(&admin_token.token),
        None,
    )
    .await;
    assert_eq!(get_team_b.status(), StatusCode::OK);
}
