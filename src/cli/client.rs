//! HTTP client for Flowplane CLI
//!
//! Provides an authenticated HTTP client for interacting with the Flowplane API.
//! Supports multiple authentication methods and configuration options.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, trace};

use crate::auth::models::PersonalAccessToken;

/// HTTP client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL for the Flowplane API (e.g., "http://localhost:8080")
    pub base_url: String,

    /// Personal access token for authentication
    pub token: String,

    /// Request timeout in seconds
    pub timeout: u64,

    /// Enable verbose request/response logging
    pub verbose: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8080".to_string(),
            token: String::new(),
            timeout: 30,
            verbose: false,
        }
    }
}

/// Authenticated HTTP client for Flowplane API
#[derive(Debug, Clone)]
pub struct FlowplaneClient {
    client: Client,
    config: ClientConfig,
}

impl FlowplaneClient {
    /// Create a new Flowplane client with the given configuration
    pub fn new(config: ClientConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { client, config })
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Build a GET request with authentication
    pub fn get(&self, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.config.base_url, path);
        debug!("GET {}", url);

        self.client.get(&url).bearer_auth(&self.config.token)
    }

    /// Build a POST request with authentication
    pub fn post(&self, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.config.base_url, path);
        debug!("POST {}", url);

        self.client.post(&url).bearer_auth(&self.config.token)
    }

    /// Build a PUT request with authentication
    pub fn put(&self, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.config.base_url, path);
        debug!("PUT {}", url);

        self.client.put(&url).bearer_auth(&self.config.token)
    }

    /// Build a DELETE request with authentication
    pub fn delete(&self, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.config.base_url, path);
        debug!("DELETE {}", url);

        self.client.delete(&url).bearer_auth(&self.config.token)
    }

    /// Send a GET request and deserialize the JSON response
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.get(path).send().await.context("Failed to send GET request")?;

        self.handle_response(response).await
    }

    /// Send a POST request with JSON body and deserialize the response
    pub async fn post_json<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R> {
        if self.config.verbose {
            let body_json = serde_json::to_string_pretty(body)
                .unwrap_or_else(|_| "<unable to serialize>".to_string());
            trace!("Request body:\n{}", body_json);
        }

        let response =
            self.post(path).json(body).send().await.context("Failed to send POST request")?;

        self.handle_response(response).await
    }

    /// Send a PUT request with JSON body and deserialize the response
    pub async fn put_json<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R> {
        if self.config.verbose {
            let body_json = serde_json::to_string_pretty(body)
                .unwrap_or_else(|_| "<unable to serialize>".to_string());
            trace!("Request body:\n{}", body_json);
        }

        let response =
            self.put(path).json(body).send().await.context("Failed to send PUT request")?;

        self.handle_response(response).await
    }

    /// Send a DELETE request and handle the response
    pub async fn delete_json<R: DeserializeOwned>(&self, path: &str) -> Result<R> {
        let response = self.delete(path).send().await.context("Failed to send DELETE request")?;

        self.handle_response(response).await
    }

    /// Send a DELETE request that expects no content response (204)
    pub async fn delete_no_content(&self, path: &str) -> Result<()> {
        let response = self.delete(path).send().await.context("Failed to send DELETE request")?;

        let status = response.status();
        debug!("Response status: {}", status);

        if !status.is_success() {
            let error_text =
                response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());

            if self.config.verbose {
                trace!("Error response:\n{}", error_text);
            }

            anyhow::bail!("HTTP request failed with status {}: {}", status, error_text);
        }

        Ok(())
    }

    /// Handle HTTP response, checking status and deserializing JSON
    async fn handle_response<T: DeserializeOwned>(&self, response: Response) -> Result<T> {
        let status = response.status();
        debug!("Response status: {}", status);

        if !status.is_success() {
            let error_text =
                response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());

            if self.config.verbose {
                trace!("Error response:\n{}", error_text);
            }

            anyhow::bail!("HTTP request failed with status {}: {}", status, error_text);
        }

        let body = response.text().await.context("Failed to read response body")?;

        if self.config.verbose {
            trace!("Response body:\n{}", body);
        }

        serde_json::from_str(&body)
            .with_context(|| format!("Failed to deserialize response: {}", body))
    }

    // === Bootstrap API Methods ===

    /// Generate a setup token via bootstrap initialization and create first admin user
    pub async fn bootstrap_initialize(
        &self,
        email: &str,
        password: &str,
        name: &str,
    ) -> Result<BootstrapInitializeResponse> {
        let request = BootstrapInitializeRequest {
            email: email.to_string(),
            password: password.to_string(),
            name: name.to_string(),
        };

        self.post_json("/api/v1/bootstrap/initialize", &request).await
    }

    // === Token Management API Methods ===

    /// List personal access tokens
    pub async fn list_tokens(&self, limit: i64, offset: i64) -> Result<Vec<PersonalAccessToken>> {
        let path = format!("/api/v1/tokens?limit={}&offset={}", limit, offset);
        self.get_json(&path).await
    }

    /// Get a specific token by ID
    pub async fn get_token(&self, id: &str) -> Result<PersonalAccessToken> {
        let path = format!("/api/v1/tokens/{}", id);
        self.get_json(&path).await
    }

    /// Revoke a personal access token
    pub async fn revoke_token(&self, id: &str) -> Result<PersonalAccessToken> {
        let path = format!("/api/v1/tokens/{}/revoke", id);
        self.post_json::<EmptyRequest, PersonalAccessToken>(&path, &EmptyRequest {}).await
    }

    /// Create a new personal access token
    pub async fn create_token(&self, request: CreateTokenRequest) -> Result<CreateTokenResponse> {
        self.post_json("/api/v1/tokens", &request).await
    }
}

// === Data Transfer Objects (DTOs) ===

/// Request body for bootstrap initialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeRequest {
    pub email: String,
    pub password: String,
    pub name: String,
}

/// Response from bootstrap initialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeResponse {
    pub setup_token: String,
    pub expires_at: DateTime<Utc>,
    pub max_usage_count: i64,
    pub message: String,
    pub next_steps: Vec<String>,
}

/// Token with its secret value (only returned on creation)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSecretResponse {
    pub id: String,
    pub token: String,
}

/// Request to create a new personal access token
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenRequest {
    pub name: String,
    pub description: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub created_by: Option<String>,
}

/// Response from creating a token
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenResponse {
    pub id: String,
    pub token: String,
}

/// Empty request body for endpoints that don't require a body
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmptyRequest {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_config_default() {
        let config = ClientConfig::default();
        assert_eq!(config.base_url, "http://localhost:8080");
        assert_eq!(config.token, "");
        assert_eq!(config.timeout, 30);
        assert!(!config.verbose);
    }

    #[test]
    fn test_client_creation() {
        let config = ClientConfig {
            base_url: "http://example.com".to_string(),
            token: "test_token".to_string(),
            timeout: 60,
            verbose: true,
        };

        let client = FlowplaneClient::new(config.clone());
        assert!(client.is_ok());

        let client = client.unwrap();
        assert_eq!(client.base_url(), "http://example.com");
    }

    #[test]
    fn test_bootstrap_initialize_request_serialization() {
        let request = BootstrapInitializeRequest {
            email: "admin@example.com".to_string(),
            password: "secure123".to_string(),
            name: "Admin User".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("email"));
        assert!(json.contains("admin@example.com"));
        assert!(json.contains("password"));
        assert!(json.contains("name"));
    }

    #[test]
    fn test_bootstrap_initialize_response_deserialization() {
        let json = r#"{
            "setupToken": "fp_setup_abc.xyz123",
            "expiresAt": "2025-01-17T00:00:00Z",
            "maxUsageCount": 1,
            "message": "Setup token generated",
            "nextSteps": ["Step 1", "Step 2"]
        }"#;

        let response: BootstrapInitializeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.setup_token, "fp_setup_abc.xyz123");
        assert_eq!(response.max_usage_count, 1);
        assert_eq!(response.message, "Setup token generated");
        assert_eq!(response.next_steps.len(), 2);
    }

    #[test]
    fn test_token_secret_response_deserialization() {
        let json = r#"{
            "id": "token-456",
            "token": "fp_pat_def.123"
        }"#;

        let response: TokenSecretResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "token-456");
        assert_eq!(response.token, "fp_pat_def.123");
    }

    #[test]
    fn test_create_token_request_serialization() {
        let request = CreateTokenRequest {
            name: "test-token".to_string(),
            description: Some("Test description".to_string()),
            expires_at: None,
            scopes: vec!["clusters:read".to_string(), "routes:write".to_string()],
            created_by: Some("admin".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("name"));
        assert!(json.contains("test-token"));
        assert!(json.contains("scopes"));
        assert!(json.contains("clusters:read"));
        assert!(json.contains("routes:write"));
        assert!(json.contains("createdBy"));
        assert!(json.contains("admin"));
    }

    #[test]
    fn test_create_token_request_minimal() {
        let request = CreateTokenRequest {
            name: "minimal-token".to_string(),
            description: None,
            expires_at: None,
            scopes: vec!["admin:all".to_string()],
            created_by: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("minimal-token"));
        assert!(json.contains("admin:all"));
    }

    #[test]
    fn test_create_token_response_deserialization() {
        let json = r#"{
            "id": "new-token-789",
            "token": "fp_pat_new.secret"
        }"#;

        let response: CreateTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "new-token-789");
        assert_eq!(response.token, "fp_pat_new.secret");
    }

    #[test]
    fn test_empty_request_serialization() {
        let request = EmptyRequest {};
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_request_builder_methods() {
        let config = ClientConfig {
            base_url: "http://localhost:8080".to_string(),
            token: "test_token".to_string(),
            timeout: 30,
            verbose: false,
        };

        let client = FlowplaneClient::new(config).unwrap();

        // Test that methods build URLs correctly
        let get_req = client.get("/api/v1/tokens");
        let post_req = client.post("/api/v1/tokens");
        let put_req = client.put("/api/v1/tokens/123");
        let delete_req = client.delete("/api/v1/tokens/123");

        // Verify that methods return RequestBuilder (this compiles = success)
        let _: RequestBuilder = get_req;
        let _: RequestBuilder = post_req;
        let _: RequestBuilder = put_req;
        let _: RequestBuilder = delete_req;
    }

    #[test]
    fn test_dto_round_trip_bootstrap_request() {
        let original = BootstrapInitializeRequest {
            email: "roundtrip@example.com".to_string(),
            password: "secure456".to_string(),
            name: "Test Admin".to_string(),
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: BootstrapInitializeRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.email, original.email);
        assert_eq!(deserialized.password, original.password);
        assert_eq!(deserialized.name, original.name);
    }

    #[test]
    fn test_dto_round_trip_create_token_request() {
        let original = CreateTokenRequest {
            name: "roundtrip-token".to_string(),
            description: Some("Round trip description".to_string()),
            expires_at: None,
            scopes: vec!["scope1".to_string(), "scope2".to_string()],
            created_by: Some("tester".to_string()),
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: CreateTokenRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, original.name);
        assert_eq!(deserialized.description, original.description);
        assert_eq!(deserialized.scopes, original.scopes);
        assert_eq!(deserialized.created_by, original.created_by);
    }
}
