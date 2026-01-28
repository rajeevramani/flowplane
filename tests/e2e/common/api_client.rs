//! Typed API client for E2E tests
//!
//! Provides type-safe methods for all flowplane API operations:
//! - Authentication (bootstrap, login, token creation)
//! - Team management
//! - Resource management (clusters, routes, listeners, filters)

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::timeout::{with_timeout, TestTimeout};

/// Session information from login
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Session token (from cookie)
    pub session_token: String,
    /// CSRF token (for subsequent requests)
    pub csrf_token: String,
}

/// Test context with authenticated sessions
#[derive(Debug)]
pub struct TestContext {
    /// Admin session (from bootstrap)
    pub admin_session: SessionInfo,
    /// Admin PAT token
    pub admin_token: String,
    /// Team A info
    pub team_a_name: String,
    pub team_a_id: String,
    /// Team A developer token
    pub team_a_dev_token: Option<String>,
    /// Team A dataplane ID (created during setup)
    pub team_a_dataplane_id: String,
    /// Team B info
    pub team_b_name: String,
    pub team_b_id: String,
    /// Team B developer token
    pub team_b_dev_token: Option<String>,
    /// Team B dataplane ID (created during setup)
    pub team_b_dataplane_id: String,
}

/// API Client for flowplane
pub struct ApiClient {
    client: Client,
    base_url: String,
}

// Response types
#[derive(Debug, Deserialize)]
pub struct BootstrapResponse {
    #[serde(rename = "setupToken")]
    pub setup_token: String,
    #[serde(default)]
    pub message: String,
}

/// Login response - matches backend LoginResponseBody
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub session_id: String,
    pub csrf_token: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub teams: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Token creation response - matches backend TokenSecretResponse
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub token: String,
    pub id: String,
}

/// Team response - matches backend Team struct
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamResponse {
    pub id: String,
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub envoy_admin_port: Option<u16>,
}

/// Cluster response - matches backend ClusterResponse
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResponse {
    pub name: String,
    pub team: String,
    pub service_name: String,
    #[serde(default)]
    pub import_id: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Route config response - matches backend RouteConfigResponse
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteConfigResponse {
    pub name: String,
    pub team: String,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub cluster_targets: Option<String>,
    #[serde(default)]
    pub import_id: Option<String>,
    #[serde(default)]
    pub route_order: Option<i64>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Listener response - matches backend ListenerResponse
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerResponse {
    pub name: String,
    pub team: String,
    #[serde(default)]
    pub address: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub import_id: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Filter response - matches backend FilterResponse
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterResponse {
    pub id: String,
    pub name: String,
    pub filter_type: String,
    pub team: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub allowed_attachment_points: Vec<String>,
    #[serde(default)]
    pub attachment_count: Option<i64>,
}

/// Filter installation response - matches backend InstallFilterResponse
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterInstallationResponse {
    pub filter_id: String,
    pub listener_id: String,
    pub listener_name: String,
    pub order: i64,
}

/// Dataplane response - matches backend DataplaneResponse
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataplaneResponse {
    pub id: String,
    pub team: String,
    pub name: String,
    #[serde(default)]
    pub gateway_host: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

// Request types - match backend CreateClusterBody
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateClusterRequest {
    pub team: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    pub endpoints: Vec<ClusterEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_tls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_lookup_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lb_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health_checks: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circuit_breakers: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outlier_detection: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterEndpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct CreateRouteRequest {
    pub team: String,
    pub name: String,
    #[serde(rename = "virtualHosts")]
    pub virtual_hosts: Vec<VirtualHost>,
}

#[derive(Debug, Serialize)]
pub struct VirtualHost {
    pub name: String,
    pub domains: Vec<String>,
    pub routes: Vec<Route>,
}

#[derive(Debug, Serialize)]
pub struct Route {
    pub name: String,
    #[serde(rename = "match")]
    pub route_match: RouteMatch,
    pub action: RouteAction,
}

#[derive(Debug, Serialize)]
pub struct RouteMatch {
    pub path: PathMatch,
}

#[derive(Debug, Serialize)]
pub struct PathMatch {
    #[serde(rename = "type")]
    pub match_type: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct RouteAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub cluster: String,
    #[serde(rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    #[serde(rename = "prefixRewrite", skip_serializing_if = "Option::is_none")]
    pub prefix_rewrite: Option<String>,
    #[serde(rename = "retryPolicy", skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<serde_json::Value>,
}

/// Listener request - matches backend CreateListenerBody
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateListenerRequest {
    pub team: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The dataplane ID this listener belongs to (required)
    pub dataplane_id: String,
}

/// Dataplane request - matches backend CreateDataplaneBody
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDataplaneRequest {
    pub team: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterChainInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub filters: Vec<ListenerFilterInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_context: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterInput {
    pub name: String,
    #[serde(flatten)]
    pub filter_type: ListenerFilterTypeInput,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ListenerFilterTypeInput {
    #[serde(rename_all = "camelCase")]
    HttpConnectionManager {
        #[serde(skip_serializing_if = "Option::is_none")]
        route_config_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        inline_route_config: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        access_log: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tracing: Option<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        http_filters: Vec<Value>,
    },
    #[serde(rename_all = "camelCase")]
    TcpProxy { cluster: String },
}

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, base_url: base_url.into() }
    }

    /// Bootstrap the application (first-time setup)
    pub async fn bootstrap(
        &self,
        email: &str,
        password: &str,
        name: &str,
    ) -> anyhow::Result<BootstrapResponse> {
        let url = format!("{}/api/v1/bootstrap/initialize", self.base_url);
        let body = json!({
            "email": email,
            "password": password,
            "name": name,
        });

        let resp = self.client.post(&url).json(&body).send().await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Bootstrap failed: {} - {}", status, text);
        }

        let result: BootstrapResponse = resp.json().await?;
        Ok(result)
    }

    /// Login with email/password
    pub async fn login(&self, email: &str, password: &str) -> anyhow::Result<SessionInfo> {
        let url = format!("{}/api/v1/auth/login", self.base_url);
        let body = json!({
            "email": email,
            "password": password,
        });

        let resp = self.client.post(&url).json(&body).send().await?;

        let status = resp.status();

        // Extract session token from Set-Cookie header before consuming body
        let session_token = resp
            .cookies()
            .find(|c| c.name() == "fp_session")
            .map(|c| c.value().to_string())
            .unwrap_or_default();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Login failed: {} - {}", status, text);
        }

        let login_resp: LoginResponse = resp.json().await?;

        Ok(SessionInfo { session_token, csrf_token: login_resp.csrf_token })
    }

    /// Create a personal access token (PAT)
    pub async fn create_token(
        &self,
        session: &SessionInfo,
        name: &str,
        scopes: Vec<String>,
    ) -> anyhow::Result<TokenResponse> {
        let url = format!("{}/api/v1/tokens", self.base_url);
        let body = json!({
            "name": name,
            "description": format!("Token for {}", name),
            "scopes": scopes,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", session.session_token))
            .header("X-CSRF-Token", &session.csrf_token)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create token failed: {} - {}", status, text);
        }

        let result: TokenResponse = resp.json().await?;
        Ok(result)
    }

    /// Create a team
    pub async fn create_team(
        &self,
        token: &str,
        name: &str,
        display_name: Option<&str>,
    ) -> anyhow::Result<TeamResponse> {
        let url = format!("{}/api/v1/admin/teams", self.base_url);
        let body = json!({
            "name": name,
            "displayName": display_name.unwrap_or(name),
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create team failed: {} - {}", status, text);
        }

        let result: TeamResponse = resp.json().await?;
        Ok(result)
    }

    /// Create a cluster
    pub async fn create_cluster(
        &self,
        token: &str,
        req: &CreateClusterRequest,
    ) -> anyhow::Result<ClusterResponse> {
        let url = format!("{}/api/v1/clusters", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create cluster failed: {} - {}", status, text);
        }

        let result: ClusterResponse = resp.json().await?;
        Ok(result)
    }

    /// Create a route configuration
    pub async fn create_route(
        &self,
        token: &str,
        req: &CreateRouteRequest,
    ) -> anyhow::Result<RouteConfigResponse> {
        let url = format!("{}/api/v1/route-configs", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create route failed: {} - {}", status, text);
        }

        let result: RouteConfigResponse = resp.json().await?;
        Ok(result)
    }

    /// Create a listener
    pub async fn create_listener(
        &self,
        token: &str,
        req: &CreateListenerRequest,
    ) -> anyhow::Result<ListenerResponse> {
        let url = format!("{}/api/v1/listeners", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create listener failed: {} - {}", status, text);
        }

        let result: ListenerResponse = resp.json().await?;
        Ok(result)
    }

    /// Create a dataplane
    pub async fn create_dataplane(
        &self,
        token: &str,
        req: &CreateDataplaneRequest,
    ) -> anyhow::Result<DataplaneResponse> {
        let url = format!("{}/api/v1/teams/{}/dataplanes", self.base_url, req.team);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create dataplane failed: {} - {}", status, text);
        }

        let result: DataplaneResponse = resp.json().await?;
        Ok(result)
    }

    /// Create a dataplane idempotently - returns existing if already exists
    pub async fn create_dataplane_idempotent(
        &self,
        token: &str,
        req: &CreateDataplaneRequest,
    ) -> anyhow::Result<DataplaneResponse> {
        let url = format!("{}/api/v1/teams/{}/dataplanes", self.base_url, req.team);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(req)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            let result: DataplaneResponse = resp.json().await?;
            return Ok(result);
        }

        // If conflict (409), list dataplanes and find by name
        if status == StatusCode::CONFLICT {
            if let Ok(dataplanes) = self.list_dataplanes(token, &req.team).await {
                for dp in dataplanes {
                    if dp.name == req.name {
                        return Ok(dp);
                    }
                }
            }
        }

        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Create dataplane failed: {} - {}", status, text);
    }

    /// List dataplanes for a team
    pub async fn list_dataplanes(
        &self,
        token: &str,
        team: &str,
    ) -> anyhow::Result<Vec<DataplaneResponse>> {
        let url = format!("{}/api/v1/teams/{}/dataplanes", self.base_url, team);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("List dataplanes failed: {} - {}", status, text);
        }

        let result: Vec<DataplaneResponse> = resp.json().await?;
        Ok(result)
    }

    /// Delete a dataplane by team and name
    pub async fn delete_dataplane(
        &self,
        token: &str,
        team: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/teams/{}/dataplanes/{}", self.base_url, team, name);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Delete dataplane failed: {} - {}", status, text);
        }

        Ok(())
    }

    /// List all teams (for cleanup purposes)
    pub async fn list_teams(&self, token: &str) -> anyhow::Result<Vec<TeamResponse>> {
        let url = format!("{}/api/v1/admin/teams", self.base_url);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("List teams failed: {} - {}", status, text);
        }

        // Response is paginated: {"teams": [...], "total": N, ...}
        let body: Value = resp.json().await?;
        let teams = body
            .get("teams")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<TeamResponse>(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        Ok(teams)
    }

    /// Delete all dataplanes across all teams (for test cleanup)
    pub async fn delete_all_dataplanes(&self, token: &str) -> anyhow::Result<u32> {
        let teams = self.list_teams(token).await?;
        let mut deleted = 0;

        for team in teams {
            if let Ok(dataplanes) = self.list_dataplanes(token, &team.name).await {
                for dp in dataplanes {
                    if self.delete_dataplane(token, &team.name, &dp.name).await.is_ok() {
                        deleted += 1;
                    }
                }
            }
        }

        Ok(deleted)
    }

    /// Create a filter
    pub async fn create_filter(
        &self,
        token: &str,
        team: &str,
        name: &str,
        filter_type: &str,
        config: Value,
    ) -> anyhow::Result<FilterResponse> {
        let url = format!("{}/api/v1/filters", self.base_url);
        // API expects config in {"type": "...", "config": {...}} format
        let body = json!({
            "team": team,
            "name": name,
            "filterType": filter_type,
            "config": {
                "type": filter_type,
                "config": config
            },
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create filter failed: {} - {}", status, text);
        }

        let result: FilterResponse = resp.json().await?;
        Ok(result)
    }

    /// Install a filter on a listener
    pub async fn install_filter(
        &self,
        token: &str,
        filter_id: &str,
        listener_name: &str,
        order: Option<i64>,
    ) -> anyhow::Result<FilterInstallationResponse> {
        let url = format!("{}/api/v1/filters/{}/installations", self.base_url, filter_id);
        let body = json!({
            "listenerName": listener_name,
            "order": order.unwrap_or(100),
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Install filter failed: {} - {}", status, text);
        }

        let result: FilterInstallationResponse = resp.json().await?;
        Ok(result)
    }

    /// Attach a filter to a route
    /// This makes the filter active on the specified route.
    /// Endpoint: POST /api/v1/route-configs/{route}/filters
    /// Returns 204 No Content on success.
    pub async fn attach_filter_to_route(
        &self,
        token: &str,
        route_name: &str,
        filter_id: &str,
        order: Option<i64>,
    ) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/route-configs/{}/filters", self.base_url, route_name);
        let body = json!({
            "filterId": filter_id,
            "order": order.unwrap_or(1),
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Attach filter to route failed: {} - {}", status, text);
        }

        // Returns 204 No Content
        Ok(())
    }

    /// Configure filter at route-config level.
    /// This enables the filter for all routes in the route config.
    /// Endpoint: POST /api/v1/filters/{filter_id}/configurations
    pub async fn configure_filter_at_route_config(
        &self,
        token: &str,
        filter_id: &str,
        route_config_name: &str,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/filters/{}/configurations", self.base_url, filter_id);
        let body = json!({
            "scopeType": "route-config",
            "scopeId": route_config_name
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Configure filter at route-config failed: {} - {}", status, text);
        }

        let result: Value = resp.json().await?;
        Ok(result)
    }

    /// Add a route-specific filter configuration override
    /// This allows customizing filter config for a specific scope (route/vhost/listener).
    /// Endpoint: POST /api/v1/filters/{filter_id}/configurations
    /// scope_id format for routes: "{route-config-name}/{vhost-name}/{route-name}"
    pub async fn add_route_filter_override(
        &self,
        token: &str,
        filter_id: &str,
        scope_id: &str,
        config: Value,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/filters/{}/configurations", self.base_url, filter_id);
        let body = json!({
            "scopeType": "route",
            "scopeId": scope_id,
            "settings": {
                "behavior": "override",
                "config": config
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Add route filter override failed: {} - {}", status, text);
        }

        let result: Value = resp.json().await?;
        Ok(result)
    }

    /// Import OpenAPI spec
    pub async fn import_openapi(
        &self,
        token: &str,
        team: &str,
        spec: Value,
        listener_port: u16,
        dataplane_id: &str,
    ) -> anyhow::Result<Value> {
        let url = format!(
            "{}/api/v1/openapi/import?team={}&listener_mode=new&new_listener_name={}-listener&new_listener_port={}&dataplane_id={}",
            self.base_url, team, team, listener_port, dataplane_id
        );

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&spec)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Import OpenAPI failed: {} - {}", status, text);
        }

        let result: Value = resp.json().await?;
        Ok(result)
    }

    /// List clusters for a team
    pub async fn list_clusters(
        &self,
        token: &str,
        team: Option<&str>,
    ) -> anyhow::Result<Vec<ClusterResponse>> {
        let url = match team {
            Some(t) => format!("{}/api/v1/clusters?team={}", self.base_url, t),
            None => format!("{}/api/v1/clusters", self.base_url),
        };

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("List clusters failed: {} - {}", status, text);
        }

        let result: Vec<ClusterResponse> = resp.json().await?;
        Ok(result)
    }

    /// Get a filter by ID
    pub async fn get_filter(&self, token: &str, filter_id: &str) -> anyhow::Result<FilterResponse> {
        let url = format!("{}/api/v1/filters/{}", self.base_url, filter_id);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Get filter failed: {} - {}", status, text);
        }

        let result: FilterResponse = resp.json().await?;
        Ok(result)
    }

    /// Delete a filter by ID
    pub async fn delete_filter(&self, token: &str, filter_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/filters/{}", self.base_url, filter_id);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Delete filter failed: {} - {}", status, text);
        }

        Ok(())
    }

    /// Generic GET request with token auth
    pub async fn get(&self, token: &str, path: &str) -> anyhow::Result<(StatusCode, Value)> {
        let url = format!("{}{}", self.base_url, path);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(json!(null));
        Ok((status, body))
    }

    /// Generic POST request with token auth
    pub async fn post(
        &self,
        token: &str,
        path: &str,
        body: Value,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let url = format!("{}{}", self.base_url, path);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(json!(null));
        Ok((status, body))
    }

    /// Generic DELETE request with token auth
    pub async fn delete(&self, token: &str, path: &str) -> anyhow::Result<StatusCode> {
        let url = format!("{}{}", self.base_url, path);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        Ok(resp.status())
    }

    /// Check if bootstrap is needed
    pub async fn needs_bootstrap(&self) -> anyhow::Result<bool> {
        let url = format!("{}/api/v1/bootstrap/status", self.base_url);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            // If endpoint fails, assume bootstrap is needed
            return Ok(true);
        }

        let body: Value = resp.json().await?;
        Ok(body.get("needsInitialization").and_then(|v| v.as_bool()).unwrap_or(true))
    }

    /// Try to create a team, return Ok even if it already exists
    pub async fn create_team_idempotent(
        &self,
        token: &str,
        name: &str,
        display_name: Option<&str>,
    ) -> anyhow::Result<TeamResponse> {
        let url = format!("{}/api/v1/admin/teams", self.base_url);
        let body = json!({
            "name": name,
            "displayName": display_name.unwrap_or(name),
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            let result: TeamResponse = resp.json().await?;
            return Ok(result);
        }

        // If conflict (409), list teams and find by name
        // Note: GET /api/v1/admin/teams/{id} expects a UUID, not a name
        if status == StatusCode::CONFLICT {
            let list_url = format!("{}/api/v1/admin/teams", self.base_url);
            let list_resp = self
                .client
                .get(&list_url)
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await?;

            if list_resp.status().is_success() {
                // Response is paginated: {"teams": [...], "total": N, ...}
                let body: Value = list_resp.json().await?;
                if let Some(teams_array) = body.get("teams").and_then(|v| v.as_array()) {
                    for team_value in teams_array {
                        if let Ok(team) = serde_json::from_value::<TeamResponse>(team_value.clone())
                        {
                            if team.name == name {
                                return Ok(team);
                            }
                        }
                    }
                }
            }
        }

        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Create team failed: {} - {}", status, text);
    }
}

/// Standard test credentials - used for all shared infrastructure tests
pub const TEST_EMAIL: &str = "smoke@test.local";
pub const TEST_PASSWORD: &str = "SmokeTest123!";
pub const TEST_NAME: &str = "Smoke Test User";

/// Setup a basic dev context with bootstrap, login, and admin token.
/// This function is idempotent - safe to call multiple times with shared infrastructure.
///
/// Each test gets unique team names based on the test_name to ensure isolation.
pub async fn setup_dev_context(api: &ApiClient, test_name: &str) -> anyhow::Result<TestContext> {
    use super::shared_infra::unique_team_name;

    // Generate unique team names for this test
    let team_a_name = unique_team_name(&format!("{}-team-a", test_name));
    let team_b_name = unique_team_name(&format!("{}-team-b", test_name));

    // Check if bootstrap is needed
    let needs_bootstrap = with_timeout(TestTimeout::quick("Check bootstrap status"), async {
        api.needs_bootstrap().await
    })
    .await
    .unwrap_or(true);

    // Bootstrap only if needed (uses standard test credentials)
    if needs_bootstrap {
        let bootstrap = with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
            api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
        })
        .await?;
        assert!(bootstrap.setup_token.starts_with("fp_setup_"));
    }

    // Login with standard credentials
    let session = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await?;

    // Create admin token (may already exist, but tokens are user-scoped so this is fine)
    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "e2e-admin-token", vec!["admin:all".to_string()]).await
    })
    .await?;

    assert!(token_resp.token.starts_with("fp_pat_"));
    let admin_token = token_resp.token;

    // Create Team A with unique name for this test
    let team_a = with_timeout(TestTimeout::default_with_label("Create Team A"), async {
        api.create_team_idempotent(
            &admin_token,
            &team_a_name,
            Some(&format!("Team A for {}", test_name)),
        )
        .await
    })
    .await?;

    // Create Team B with unique name for this test
    let team_b = with_timeout(TestTimeout::default_with_label("Create Team B"), async {
        api.create_team_idempotent(
            &admin_token,
            &team_b_name,
            Some(&format!("Team B for {}", test_name)),
        )
        .await
    })
    .await?;

    // Create dataplane for Team A (idempotent - handles re-runs)
    let dataplane_a = with_timeout(TestTimeout::default_with_label("Create Dataplane A"), async {
        api.create_dataplane_idempotent(
            &admin_token,
            &CreateDataplaneRequest {
                team: team_a.name.clone(),
                name: format!("{}-dataplane", team_a.name),
                gateway_host: Some("127.0.0.1".to_string()),
                description: Some(format!("Default dataplane for {}", team_a.name)),
            },
        )
        .await
    })
    .await?;

    // Create dataplane for Team B (idempotent - handles re-runs)
    let dataplane_b = with_timeout(TestTimeout::default_with_label("Create Dataplane B"), async {
        api.create_dataplane_idempotent(
            &admin_token,
            &CreateDataplaneRequest {
                team: team_b.name.clone(),
                name: format!("{}-dataplane", team_b.name),
                gateway_host: Some("127.0.0.1".to_string()),
                description: Some(format!("Default dataplane for {}", team_b.name)),
            },
        )
        .await
    })
    .await?;

    Ok(TestContext {
        admin_session: session,
        admin_token,
        team_a_name: team_a.name,
        team_a_id: team_a.id,
        team_a_dev_token: None,
        team_a_dataplane_id: dataplane_a.id,
        team_b_name: team_b.name,
        team_b_id: team_b.id,
        team_b_dev_token: None,
        team_b_dataplane_id: dataplane_b.id,
    })
}

/// Helper to create a simple cluster pointing to an endpoint
pub fn simple_cluster(team: &str, name: &str, host: &str, port: u16) -> CreateClusterRequest {
    CreateClusterRequest {
        team: team.to_string(),
        name: name.to_string(),
        service_name: None,
        endpoints: vec![ClusterEndpoint { host: host.to_string(), port }],
        connect_timeout_seconds: None,
        use_tls: None,
        tls_server_name: None,
        dns_lookup_family: None,
        lb_policy: None,
        health_checks: vec![],
        circuit_breakers: None,
        outlier_detection: None,
        protocol_type: None,
    }
}

/// Helper to create a simple route with prefix match
pub fn simple_route(
    team: &str,
    name: &str,
    domain: &str,
    path_prefix: &str,
    cluster: &str,
) -> CreateRouteRequest {
    CreateRouteRequest {
        team: team.to_string(),
        name: name.to_string(),
        virtual_hosts: vec![VirtualHost {
            name: format!("{}-vh", name),
            domains: vec![domain.to_string()],
            routes: vec![Route {
                name: format!("{}-route", name),
                route_match: RouteMatch {
                    path: PathMatch {
                        match_type: "prefix".to_string(),
                        value: path_prefix.to_string(),
                    },
                },
                action: RouteAction {
                    action_type: "forward".to_string(),
                    cluster: cluster.to_string(),
                    timeout_seconds: Some(30),
                    prefix_rewrite: None,
                    retry_policy: None,
                },
            }],
        }],
    }
}

/// Helper to create a simple listener with HTTP connection manager
pub fn simple_listener(
    team: &str,
    name: &str,
    port: u16,
    route_config: &str,
    dataplane_id: &str,
) -> CreateListenerRequest {
    CreateListenerRequest {
        team: team.to_string(),
        name: name.to_string(),
        address: "0.0.0.0".to_string(),
        port,
        filter_chains: vec![ListenerFilterChainInput {
            name: Some("default".to_string()),
            filters: vec![ListenerFilterInput {
                name: "envoy.filters.network.http_connection_manager".to_string(),
                filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                    route_config_name: Some(route_config.to_string()),
                    inline_route_config: None,
                    access_log: None,
                    tracing: None,
                    http_filters: vec![],
                },
            }],
            tls_context: None,
        }],
        protocol: None,
        dataplane_id: dataplane_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_cluster_creation() {
        let cluster = simple_cluster("engineering", "test-cluster", "127.0.0.1", 8080);
        assert_eq!(cluster.team, "engineering");
        assert_eq!(cluster.name, "test-cluster");
        assert_eq!(cluster.endpoints.len(), 1);
        assert_eq!(cluster.endpoints[0].host, "127.0.0.1");
        assert_eq!(cluster.endpoints[0].port, 8080);
    }

    #[test]
    fn test_simple_route_creation() {
        let route =
            simple_route("engineering", "test-route", "api.test.local", "/api", "test-cluster");
        assert_eq!(route.team, "engineering");
        assert_eq!(route.virtual_hosts.len(), 1);
        assert_eq!(route.virtual_hosts[0].domains[0], "api.test.local");
    }

    #[test]
    fn test_simple_listener_creation() {
        let listener_req =
            simple_listener("engineering", "test-listener", 8080, "test-route", "dp-123");
        assert_eq!(listener_req.team, "engineering");
        assert_eq!(listener_req.name, "test-listener");
        assert_eq!(listener_req.port, 8080);
        assert_eq!(listener_req.address, "0.0.0.0");
        assert_eq!(listener_req.filter_chains.len(), 1);
        assert_eq!(listener_req.dataplane_id, "dp-123");
    }
}
