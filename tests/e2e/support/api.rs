use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Buf, http::Method, http::Uri, Request};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde_json::json;
use std::net::SocketAddr;

use flowplane::auth::team::CreateTeamRequest;
use flowplane::auth::token_service::TokenService;
use flowplane::auth::validation::CreateTokenRequest;
use flowplane::storage::repositories::team::{SqlxTeamRepository, TeamRepository};
use flowplane::storage::{create_pool, DatabaseConfig};

#[allow(dead_code)]
pub async fn ensure_team_exists(team_name: &str) -> anyhow::Result<()> {
    let pool = create_pool(&DatabaseConfig::from_env()).await?;
    let repo = SqlxTeamRepository::new(pool);

    // Check if team exists
    if repo.get_team_by_name(team_name).await?.is_some() {
        return Ok(());
    }

    // Create team if it doesn't exist
    let request = CreateTeamRequest {
        name: team_name.to_string(),
        display_name: format!("E2E Test Team {}", team_name),
        description: Some("Auto-created for E2E tests".to_string()),
        owner_user_id: None,
        settings: None,
    };

    repo.create_team(request).await?;
    Ok(())
}

#[allow(dead_code)]
pub async fn create_pat(scopes: Vec<&str>) -> anyhow::Result<String> {
    let pool = create_pool(&DatabaseConfig::from_env()).await?;
    let audit =
        std::sync::Arc::new(flowplane::storage::repository::AuditLogRepository::new(pool.clone()));
    let svc = TokenService::with_sqlx(pool, audit);
    let short = uuid::Uuid::new_v4().to_string().chars().take(8).collect::<String>();
    let req = CreateTokenRequest::without_user(
        format!("e2e-token-{}", short),
        Some("e2e token".into()),
        None,
        scopes.into_iter().map(|s| s.to_string()).collect(),
        Some("e2e".into()),
    );
    let secret = svc.create_token(req).await?.token;
    Ok(secret)
}

#[allow(dead_code)]
pub async fn wait_http_ready(addr: SocketAddr) {
    use super::retry::{retry_with_backoff, RetryConfig};

    let config =
        RetryConfig::fast().with_description(format!("HTTP server at {} to be ready", addr));

    retry_with_backoff(config, || async {
        tokio::net::TcpStream::connect(addr)
            .await
            .map(|_| ())
            .map_err(|e| format!("Connection failed: {}", e))
    })
    .await
    .expect("HTTP server should become ready");
}

#[allow(dead_code)]
pub async fn post_create_api(
    api_addr: SocketAddr,
    bearer: &str,
    team: &str,
    domain: &str,
    prefix: &str,
    cluster_name: &str,
    endpoint: &str,
) -> anyhow::Result<serde_json::Value> {
    // Use default port 10000 for backward compatibility
    post_create_api_on_port(api_addr, bearer, team, domain, prefix, cluster_name, endpoint, 10000)
        .await
}

#[allow(dead_code)]
pub async fn post_create_api_on_port(
    api_addr: SocketAddr,
    bearer: &str,
    team: &str,
    domain: &str,
    prefix: &str,
    cluster_name: &str,
    endpoint: &str,
    listener_port: u16,
) -> anyhow::Result<serde_json::Value> {
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);
    // Create new listener with specified port
    let uri: Uri = format!(
        "http://{}/api/v1/openapi/import?team={}&listener_mode=new&new_listener_name={}-listener&new_listener_port={}",
        api_addr, team, cluster_name, listener_port
    )
    .parse()?;

    // Create a minimal OpenAPI 3.0 spec for import
    let openapi_spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "E2E Test API",
            "version": "1.0.0",
            "x-flowplane-domain": domain
        },
        "servers": [
            {
                "url": format!("http://{}", endpoint)
            }
        ],
        "paths": {
            prefix: {
                "get": {
                    "operationId": cluster_name,
                    "responses": {
                        "200": {
                            "description": "Success"
                        }
                    }
                }
            }
        }
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", bearer))
        .body(Full::<Bytes>::from(openapi_spec.to_string()))?;

    let res = client.request(req).await?;
    let status = res.status();
    let bytes = res.into_body().collect().await?.to_bytes();
    if !status.is_success() {
        anyhow::bail!("create api failed: {} - {}", status, String::from_utf8_lossy(bytes.chunk()))
    }
    let json: serde_json::Value = serde_json::from_slice(bytes.chunk())?;
    Ok(json)
}

/// Append a route to an existing listener using OpenAPI import with listener_mode=existing.
/// This is the proper way to add routes to an existing listener.
#[allow(dead_code)]
pub async fn post_append_route_to_listener(
    api_addr: SocketAddr,
    bearer: &str,
    team: &str,
    listener_name: &str,
    domain: &str,
    prefix: &str,
    cluster_name: &str,
    endpoint: &str,
) -> anyhow::Result<serde_json::Value> {
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);
    // Use existing listener to add the route
    let uri: Uri = format!(
        "http://{}/api/v1/openapi/import?team={}&listener_mode=existing&existing_listener_name={}",
        api_addr, team, listener_name
    )
    .parse()?;

    // Create a minimal OpenAPI 3.0 spec for import
    let openapi_spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": format!("E2E Append Route {}", cluster_name),
            "version": "1.0.0",
            "x-flowplane-domain": domain
        },
        "servers": [
            {
                "url": format!("http://{}", endpoint)
            }
        ],
        "paths": {
            prefix: {
                "get": {
                    "operationId": cluster_name,
                    "responses": {
                        "200": {
                            "description": "Success"
                        }
                    }
                }
            }
        }
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", bearer))
        .body(Full::<Bytes>::from(openapi_spec.to_string()))?;

    let res = client.request(req).await?;
    let status = res.status();
    let bytes = res.into_body().collect().await?.to_bytes();
    if !status.is_success() {
        anyhow::bail!(
            "append route to listener failed: {} - {}",
            status,
            String::from_utf8_lossy(bytes.chunk())
        )
    }
    let json: serde_json::Value = serde_json::from_slice(bytes.chunk())?;
    Ok(json)
}

#[allow(dead_code)]
pub async fn post_append_route(
    api_addr: SocketAddr,
    bearer: &str,
    team: &str,
    route_name: &str,
    domain: &str,
    prefix: &str,
    cluster_name: &str,
    _note: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);
    let uri: Uri = format!("http://{}/api/v1/routes", api_addr).parse()?;
    let body = json!({
        "team": team,
        "name": route_name,
        "virtualHosts": [{
            "name": format!("{}-vh", route_name),
            "domains": [domain],
            "routes": [{
                "name": route_name,
                "match": {"path": {"type": "prefix", "value": prefix}},
                "action": {"type": "forward", "cluster": cluster_name, "timeoutSeconds": 3}
            }]
        }]
    });
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", bearer))
        .body(Full::<Bytes>::from(body.to_string()))?;

    let res = client.request(req).await?;
    let status = res.status();
    let bytes = res.into_body().collect().await?.to_bytes();
    if !status.is_success() {
        anyhow::bail!(
            "append route failed: {} - {}",
            status,
            String::from_utf8_lossy(bytes.chunk())
        )
    }
    let json: serde_json::Value = serde_json::from_slice(bytes.chunk())?;
    Ok(json)
}
