//! HTTP client for Flowplane CLI
//!
//! Provides an authenticated HTTP client for interacting with the Flowplane API.
//! Supports multiple authentication methods and configuration options.

use anyhow::{Context, Result};
use reqwest::{Client, RequestBuilder, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tracing::{debug, trace};

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
}

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
}
