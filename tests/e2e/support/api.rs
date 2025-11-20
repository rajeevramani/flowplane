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
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);
    let uri: Uri = format!("http://{}/api/v1/openapi/import?team={}", api_addr, team).parse()?;

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

#[allow(dead_code)]
pub async fn post_append_route(
    api_addr: SocketAddr,
    bearer: &str,
    _api_id: &str, // Deprecated: no longer used with new OpenAPI import
    prefix: &str,
    cluster_name: &str,
    endpoint: &str,
    note: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);
    // Note: This function is deprecated and should not be used with new OpenAPI import API
    let uri: Uri = format!("http://{}/api/v1/routes", api_addr).parse()?;
    let body = json!({
        "route": {
            "match": {"prefix": prefix},
            "cluster": {"name": cluster_name, "endpoint": endpoint},
            "timeoutSeconds": 3
        },
        "deploymentNote": note
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
