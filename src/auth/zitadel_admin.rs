//! Zitadel Management API client for Dynamic Client Registration.
//!
//! Provides a thin wrapper around the Zitadel Management API to create
//! machine users and generate client credentials. Used by the DCR proxy
//! endpoint (`POST /api/v1/oauth/register`).
//!
//! # Authentication
//!
//! Uses a Personal Access Token (PAT) for the Zitadel Management API,
//! configured via `FLOWPLANE_ZITADEL_ADMIN_PAT`.

use crate::api::error::ApiError;
use serde::Deserialize;

/// Client for the Zitadel Management API.
///
/// Authenticates with a PAT and provides methods to create machine users,
/// generate client secrets, and assign role grants.
#[derive(Clone)]
pub struct ZitadelAdminClient {
    base_url: String,
    pat: String,
    http: reqwest::Client,
    /// Host header derived from the issuer URL for Zitadel instance resolution.
    issuer_host: Option<String>,
}

/// Response from creating a machine user via Zitadel Management API.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMachineUserResponse {
    user_id: String,
}

/// Response from generating a client secret via Zitadel Management API.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSecretResponse {
    client_id: String,
    client_secret: String,
}

impl ZitadelAdminClient {
    /// Create a new client from environment variables.
    ///
    /// Required:
    /// - `FLOWPLANE_ZITADEL_ADMIN_PAT`: Personal Access Token for the Management API
    ///
    /// Optional:
    /// - `FLOWPLANE_ZITADEL_ADMIN_URL`: Management API base URL (defaults to `FLOWPLANE_ZITADEL_ISSUER`)
    ///
    /// Returns `None` if the PAT is not configured.
    pub fn from_env() -> Option<Self> {
        let pat = std::env::var("FLOWPLANE_ZITADEL_ADMIN_PAT").ok()?;
        if pat.is_empty() {
            tracing::warn!("FLOWPLANE_ZITADEL_ADMIN_PAT is empty — DCR proxy disabled");
            return None;
        }

        let base_url = std::env::var("FLOWPLANE_ZITADEL_ADMIN_URL")
            .or_else(|_| std::env::var("FLOWPLANE_ZITADEL_ISSUER"))
            .ok()?;

        // Extract host header from FLOWPLANE_ZITADEL_ISSUER for Zitadel instance resolution.
        // When the admin URL uses a container name (e.g., http://zitadel:8080), we still
        // need to send the issuer's host so Zitadel resolves the correct instance.
        let issuer_host = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok().and_then(|issuer| {
            url::Url::parse(&issuer).ok().and_then(|u| {
                u.host_str().map(|h| match u.port() {
                    Some(p) => format!("{h}:{p}"),
                    None => h.to_string(),
                })
            })
        });

        tracing::info!(base_url = %base_url, "Zitadel admin client configured for DCR proxy");
        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            pat,
            http: reqwest::Client::new(),
            issuer_host,
        })
    }

    /// Attach the Host header if configured (for Zitadel instance resolution).
    fn with_host_header(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.issuer_host {
            Some(host) => builder.header("Host", host),
            None => builder,
        }
    }

    /// Create a machine user in Zitadel.
    ///
    /// Returns the Zitadel user ID on success.
    pub async fn create_machine_user(
        &self,
        username: &str,
        name: &str,
    ) -> Result<String, ApiError> {
        let url = format!("{}/management/v1/users/machine", self.base_url);

        let body = serde_json::json!({
            "userName": username,
            "name": name,
            "description": format!("DCR-registered agent: {name}"),
            "accessTokenType": "ACCESS_TOKEN_TYPE_JWT",
        });

        let req = self.http.post(&url).bearer_auth(&self.pat).json(&body);

        let resp =
            self.with_host_header(req).send().await.map_err(|e| {
                ApiError::internal(format!("Zitadel create machine user failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Zitadel create machine user error");
            return Err(ApiError::internal(format!(
                "Zitadel Management API error ({status}): {body}"
            )));
        }

        let result: CreateMachineUserResponse = resp
            .json()
            .await
            .map_err(|e| ApiError::internal(format!("Zitadel response parse failed: {e}")))?;

        Ok(result.user_id)
    }

    /// Generate a client secret for a machine user.
    ///
    /// Returns `(client_id, client_secret)` on success.
    pub async fn create_client_secret(&self, user_id: &str) -> Result<(String, String), ApiError> {
        let url = format!("{}/management/v1/users/{}/secret", self.base_url, user_id);

        let req = self.http.put(&url).bearer_auth(&self.pat).json(&serde_json::json!({}));

        let resp = self
            .with_host_header(req)
            .send()
            .await
            .map_err(|e| ApiError::internal(format!("Zitadel create secret failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Zitadel create secret error");
            return Err(ApiError::internal(format!(
                "Zitadel Management API error ({status}): {body}"
            )));
        }

        let result: CreateSecretResponse = resp.json().await.map_err(|e| {
            ApiError::internal(format!("Zitadel secret response parse failed: {e}"))
        })?;

        Ok((result.client_id, result.client_secret))
    }

    /// Add a role grant for a user on a given project.
    ///
    /// `role_keys` are the Zitadel role keys (e.g., `["team-01:clusters:read"]`).
    pub async fn add_user_grant(
        &self,
        user_id: &str,
        project_id: &str,
        role_keys: Vec<String>,
    ) -> Result<(), ApiError> {
        if role_keys.is_empty() {
            return Ok(());
        }

        let url = format!("{}/management/v1/users/{}/grants", self.base_url, user_id);

        let body = serde_json::json!({
            "projectId": project_id,
            "roleKeys": role_keys,
        });

        let req = self.http.post(&url).bearer_auth(&self.pat).json(&body);

        let resp = self
            .with_host_header(req)
            .send()
            .await
            .map_err(|e| ApiError::internal(format!("Zitadel add user grant failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Zitadel add user grant error");
            return Err(ApiError::internal(format!(
                "Zitadel Management API error ({status}): {body}"
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_returns_none_when_pat_unset() {
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_PAT");
        assert!(ZitadelAdminClient::from_env().is_none());
    }

    #[test]
    fn from_env_returns_none_when_pat_empty() {
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_PAT", "");
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_URL", "http://zitadel:8080");
        let result = ZitadelAdminClient::from_env();
        assert!(result.is_none());
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_PAT");
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_URL");
    }

    #[test]
    fn from_env_uses_admin_url_when_set() {
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_PAT", "test-pat");
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_URL", "http://zitadel:8080");
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "http://localhost:8081");

        let client = ZitadelAdminClient::from_env();
        assert!(client.is_some());
        let client = client.as_ref().map(|c| &c.base_url);
        assert_eq!(client, Some(&"http://zitadel:8080".to_string()));

        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_PAT");
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_URL");
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
    }

    #[test]
    fn from_env_falls_back_to_issuer_url() {
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_PAT", "test-pat");
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_URL");
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "http://localhost:8081");

        let client = ZitadelAdminClient::from_env();
        assert!(client.is_some());
        let client = client.as_ref().map(|c| &c.base_url);
        assert_eq!(client, Some(&"http://localhost:8081".to_string()));

        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_PAT");
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
    }

    #[test]
    fn from_env_extracts_issuer_host_header() {
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_PAT", "test-pat");
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_URL", "http://zitadel:8080");
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "http://localhost:8081");

        let client = ZitadelAdminClient::from_env();
        assert!(client.is_some());
        assert_eq!(
            client.as_ref().and_then(|c| c.issuer_host.clone()),
            Some("localhost:8081".to_string())
        );

        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_PAT");
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_URL");
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
    }

    #[test]
    fn from_env_strips_trailing_slash() {
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_PAT", "test-pat");
        std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_URL", "http://zitadel:8080/");
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "http://localhost:8081");

        let client = ZitadelAdminClient::from_env();
        assert!(client.is_some());
        let client = client.as_ref().map(|c| &c.base_url);
        assert_eq!(client, Some(&"http://zitadel:8080".to_string()));

        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_PAT");
        std::env::remove_var("FLOWPLANE_ZITADEL_ADMIN_URL");
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
    }
}
