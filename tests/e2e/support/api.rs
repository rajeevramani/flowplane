use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Buf, http::Method, http::Uri, Request};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::sleep;

use flowplane::auth::token_service::TokenService;
use flowplane::auth::validation::CreateTokenRequest;
use flowplane::storage::{create_pool, DatabaseConfig};

#[allow(dead_code)]
pub async fn create_pat(scopes: Vec<&str>) -> anyhow::Result<String> {
    let pool = create_pool(&DatabaseConfig::from_env()).await?;
    let audit = std::sync::Arc::new(
        flowplane::storage::repository_simple::AuditLogRepository::new(pool.clone()),
    );
    let svc = TokenService::with_sqlx(pool, audit);
    let short = uuid::Uuid::new_v4().to_string().chars().take(8).collect::<String>();
    let req = CreateTokenRequest {
        name: format!("e2e-token-{}", short),
        description: Some("e2e token".into()),
        expires_at: None,
        scopes: scopes.into_iter().map(|s| s.to_string()).collect(),
        created_by: Some("e2e".into()),
    };
    let secret = svc.create_token(req).await?.token;
    Ok(secret)
}

#[allow(dead_code)]
pub async fn wait_http_ready(addr: SocketAddr) {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
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
    let uri: Uri = format!("http://{}/api/v1/api-definitions", api_addr).parse()?;
    let body = json!({
        "team": team,
        "domain": domain,
        "listenerIsolation": false,
        "routes": [
            {
                "match": {"prefix": prefix},
                "cluster": {"name": cluster_name, "endpoint": endpoint},
                "timeoutSeconds": 3
            }
        ]
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
        anyhow::bail!("create api failed: {} - {}", status, String::from_utf8_lossy(bytes.chunk()))
    }
    let json: serde_json::Value = serde_json::from_slice(bytes.chunk())?;
    Ok(json)
}

#[allow(dead_code)]
pub async fn post_append_route(
    api_addr: SocketAddr,
    bearer: &str,
    api_id: &str,
    prefix: &str,
    cluster_name: &str,
    endpoint: &str,
    note: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);
    let uri: Uri =
        format!("http://{}/api/v1/api-definitions/{}/routes", api_addr, api_id).parse()?;
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
