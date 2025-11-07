//! Webhook delivery service for learning session state changes
//!
//! This module provides webhook notification delivery for all learning session
//! lifecycle events, with configurable endpoints and retry logic.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::{
    errors::{Error, Result},
    storage::repositories::LearningSessionStatus,
};

/// Webhook event representing a learning session state change
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningSessionWebhookEvent {
    /// Event type identifier
    pub event_type: String,
    /// Timestamp when the event occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Session ID
    pub session_id: String,
    /// Team that owns the session
    pub team: String,
    /// Previous status (if state transition)
    pub previous_status: Option<LearningSessionStatus>,
    /// Current status
    pub current_status: LearningSessionStatus,
    /// Route pattern being learned
    pub route_pattern: String,
    /// Target sample count
    pub target_sample_count: i64,
    /// Current sample count
    pub current_sample_count: i64,
    /// Progress percentage (0-100)
    pub progress_percentage: f64,
    /// Error message (if failed)
    pub error_message: Option<String>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

impl LearningSessionWebhookEvent {
    /// Create a webhook event for session activation
    pub fn activated(
        session_id: String,
        team: String,
        route_pattern: String,
        target_sample_count: i64,
    ) -> Self {
        Self {
            event_type: "learning_session.activated".to_string(),
            timestamp: chrono::Utc::now(),
            session_id,
            team,
            previous_status: Some(LearningSessionStatus::Pending),
            current_status: LearningSessionStatus::Active,
            route_pattern,
            target_sample_count,
            current_sample_count: 0,
            progress_percentage: 0.0,
            error_message: None,
            metadata: serde_json::json!({}),
        }
    }

    /// Create a webhook event for session completion
    pub fn completed(
        session_id: String,
        team: String,
        route_pattern: String,
        target_sample_count: i64,
        current_sample_count: i64,
    ) -> Self {
        let progress_percentage = if target_sample_count > 0 {
            (current_sample_count as f64 / target_sample_count as f64) * 100.0
        } else {
            0.0
        };

        Self {
            event_type: "learning_session.completed".to_string(),
            timestamp: chrono::Utc::now(),
            session_id,
            team,
            previous_status: Some(LearningSessionStatus::Completing),
            current_status: LearningSessionStatus::Completed,
            route_pattern,
            target_sample_count,
            current_sample_count,
            progress_percentage,
            error_message: None,
            metadata: serde_json::json!({}),
        }
    }

    /// Create a webhook event for session failure
    pub fn failed(
        session_id: String,
        team: String,
        route_pattern: String,
        error_message: String,
        current_sample_count: i64,
        target_sample_count: i64,
    ) -> Self {
        let progress_percentage = if target_sample_count > 0 {
            (current_sample_count as f64 / target_sample_count as f64) * 100.0
        } else {
            0.0
        };

        Self {
            event_type: "learning_session.failed".to_string(),
            timestamp: chrono::Utc::now(),
            session_id,
            team,
            previous_status: None,
            current_status: LearningSessionStatus::Failed,
            route_pattern,
            target_sample_count,
            current_sample_count,
            progress_percentage,
            error_message: Some(error_message),
            metadata: serde_json::json!({}),
        }
    }
}

/// Webhook endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEndpoint {
    /// Unique identifier for this endpoint
    pub id: String,
    /// URL to deliver webhooks to
    pub url: String,
    /// Optional secret for HMAC signature verification
    pub secret: Option<String>,
    /// Event types to subscribe to (empty = all events)
    pub event_types: Vec<String>,
    /// Team filter (empty = all teams)
    pub teams: Vec<String>,
    /// Whether this endpoint is active
    pub enabled: bool,
}

impl WebhookEndpoint {
    /// Check if this endpoint should receive the given event
    pub fn should_receive(&self, event: &LearningSessionWebhookEvent) -> bool {
        if !self.enabled {
            return false;
        }

        // Check event type filter
        if !self.event_types.is_empty() && !self.event_types.contains(&event.event_type) {
            return false;
        }

        // Check team filter
        if !self.teams.is_empty() && !self.teams.contains(&event.team) {
            return false;
        }

        true
    }
}

/// Webhook delivery service
#[derive(Clone, Debug)]
pub struct WebhookService {
    /// HTTP client for webhook delivery
    client: reqwest::Client,
    /// Configured webhook endpoints
    endpoints: Arc<tokio::sync::RwLock<Vec<WebhookEndpoint>>>,
    /// Broadcast channel for webhook events
    tx: broadcast::Sender<LearningSessionWebhookEvent>,
}

impl WebhookService {
    /// Create a new webhook service
    pub fn new() -> (Self, broadcast::Receiver<LearningSessionWebhookEvent>) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let (tx, rx) = broadcast::channel(100);

        let service =
            Self { client, endpoints: Arc::new(tokio::sync::RwLock::new(Vec::new())), tx };

        (service, rx)
    }

    /// Add a webhook endpoint
    pub async fn add_endpoint(&self, endpoint: WebhookEndpoint) {
        let mut endpoints = self.endpoints.write().await;
        // Remove existing endpoint with same ID if exists
        endpoints.retain(|e| e.id != endpoint.id);
        endpoints.push(endpoint);
    }

    /// Remove a webhook endpoint
    pub async fn remove_endpoint(&self, endpoint_id: &str) {
        let mut endpoints = self.endpoints.write().await;
        endpoints.retain(|e| e.id != endpoint_id);
    }

    /// Get all configured endpoints
    pub async fn list_endpoints(&self) -> Vec<WebhookEndpoint> {
        self.endpoints.read().await.clone()
    }

    /// Publish an event to the broadcast channel
    pub async fn publish_event(&self, event: LearningSessionWebhookEvent) {
        // Broadcast to all subscribers
        if let Err(e) = self.tx.send(event.clone()) {
            warn!(error = %e, "Failed to broadcast webhook event");
        }

        // Deliver to configured endpoints
        self.deliver_to_endpoints(event).await;
    }

    /// Deliver event to all matching webhook endpoints
    async fn deliver_to_endpoints(&self, event: LearningSessionWebhookEvent) {
        let endpoints = self.endpoints.read().await.clone();

        for endpoint in endpoints {
            if endpoint.should_receive(&event) {
                let client = self.client.clone();
                let event_clone = event.clone();
                let endpoint_clone = endpoint.clone();

                // Spawn delivery task
                tokio::spawn(async move {
                    Self::deliver_webhook(client, endpoint_clone, event_clone).await;
                });
            }
        }
    }

    /// Deliver a webhook to a specific endpoint with retry logic
    async fn deliver_webhook(
        client: reqwest::Client,
        endpoint: WebhookEndpoint,
        event: LearningSessionWebhookEvent,
    ) {
        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY_MS: u64 = 1000;

        for attempt in 1..=MAX_RETRIES {
            match Self::send_webhook(&client, &endpoint, &event).await {
                Ok(status) => {
                    info!(
                        endpoint_id = %endpoint.id,
                        endpoint_url = %endpoint.url,
                        event_type = %event.event_type,
                        session_id = %event.session_id,
                        status_code = status.as_u16(),
                        attempt = attempt,
                        "Webhook delivered successfully"
                    );
                    return;
                }
                Err(e) => {
                    if attempt < MAX_RETRIES {
                        warn!(
                            endpoint_id = %endpoint.id,
                            endpoint_url = %endpoint.url,
                            event_type = %event.event_type,
                            session_id = %event.session_id,
                            error = %e,
                            attempt = attempt,
                            "Webhook delivery failed, retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS * attempt as u64))
                            .await;
                    } else {
                        error!(
                            endpoint_id = %endpoint.id,
                            endpoint_url = %endpoint.url,
                            event_type = %event.event_type,
                            session_id = %event.session_id,
                            error = %e,
                            attempts = MAX_RETRIES,
                            "Webhook delivery failed after all retries"
                        );
                    }
                }
            }
        }
    }

    /// Send a webhook HTTP request
    async fn send_webhook(
        client: &reqwest::Client,
        endpoint: &WebhookEndpoint,
        event: &LearningSessionWebhookEvent,
    ) -> Result<reqwest::StatusCode> {
        let body = serde_json::to_string(event)
            .map_err(|e| Error::internal(format!("Failed to serialize webhook event: {}", e)))?;

        let mut request = client.post(&endpoint.url).header("Content-Type", "application/json");

        // Add HMAC signature if secret is configured
        if let Some(secret) = &endpoint.secret {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;

            type HmacSha256 = Hmac<Sha256>;

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .map_err(|e| Error::internal(format!("Invalid webhook secret: {}", e)))?;
            mac.update(body.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            request = request.header("X-Flowplane-Signature", format!("sha256={}", signature));
        }

        let response = request
            .body(body)
            .send()
            .await
            .map_err(|e| Error::internal(format!("Webhook delivery failed: {}", e)))?;

        let status = response.status();

        if !status.is_success() {
            return Err(Error::internal(format!(
                "Webhook endpoint returned error status: {}",
                status
            )));
        }

        Ok(status)
    }
}

impl Default for WebhookService {
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_endpoint_should_receive_all() {
        let endpoint = WebhookEndpoint {
            id: "test".to_string(),
            url: "http://example.com".to_string(),
            secret: None,
            event_types: vec![],
            teams: vec![],
            enabled: true,
        };

        let event = LearningSessionWebhookEvent::activated(
            "session-1".to_string(),
            "team-a".to_string(),
            "^/api/.*".to_string(),
            1000,
        );

        assert!(endpoint.should_receive(&event));
    }

    #[test]
    fn test_webhook_endpoint_event_type_filter() {
        let endpoint = WebhookEndpoint {
            id: "test".to_string(),
            url: "http://example.com".to_string(),
            secret: None,
            event_types: vec!["learning_session.completed".to_string()],
            teams: vec![],
            enabled: true,
        };

        let activated_event = LearningSessionWebhookEvent::activated(
            "session-1".to_string(),
            "team-a".to_string(),
            "^/api/.*".to_string(),
            1000,
        );

        let completed_event = LearningSessionWebhookEvent::completed(
            "session-1".to_string(),
            "team-a".to_string(),
            "^/api/.*".to_string(),
            1000,
            1000,
        );

        assert!(!endpoint.should_receive(&activated_event));
        assert!(endpoint.should_receive(&completed_event));
    }

    #[test]
    fn test_webhook_endpoint_team_filter() {
        let endpoint = WebhookEndpoint {
            id: "test".to_string(),
            url: "http://example.com".to_string(),
            secret: None,
            event_types: vec![],
            teams: vec!["team-a".to_string()],
            enabled: true,
        };

        let team_a_event = LearningSessionWebhookEvent::activated(
            "session-1".to_string(),
            "team-a".to_string(),
            "^/api/.*".to_string(),
            1000,
        );

        let team_b_event = LearningSessionWebhookEvent::activated(
            "session-2".to_string(),
            "team-b".to_string(),
            "^/api/.*".to_string(),
            1000,
        );

        assert!(endpoint.should_receive(&team_a_event));
        assert!(!endpoint.should_receive(&team_b_event));
    }

    #[test]
    fn test_webhook_endpoint_disabled() {
        let endpoint = WebhookEndpoint {
            id: "test".to_string(),
            url: "http://example.com".to_string(),
            secret: None,
            event_types: vec![],
            teams: vec![],
            enabled: false,
        };

        let event = LearningSessionWebhookEvent::activated(
            "session-1".to_string(),
            "team-a".to_string(),
            "^/api/.*".to_string(),
            1000,
        );

        assert!(!endpoint.should_receive(&event));
    }

    #[tokio::test]
    async fn test_webhook_service_add_remove_endpoints() {
        let (service, _rx) = WebhookService::new();

        let endpoint = WebhookEndpoint {
            id: "test-1".to_string(),
            url: "http://example.com".to_string(),
            secret: None,
            event_types: vec![],
            teams: vec![],
            enabled: true,
        };

        service.add_endpoint(endpoint.clone()).await;

        let endpoints = service.list_endpoints().await;
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].id, "test-1");

        service.remove_endpoint("test-1").await;

        let endpoints = service.list_endpoints().await;
        assert_eq!(endpoints.len(), 0);
    }

    #[tokio::test]
    async fn test_webhook_event_created() {
        let event = LearningSessionWebhookEvent::activated(
            "session-1".to_string(),
            "team-a".to_string(),
            "^/api/v1/.*".to_string(),
            1000,
        );

        assert_eq!(event.event_type, "learning_session.activated");
        assert_eq!(event.session_id, "session-1");
        assert_eq!(event.team, "team-a");
        assert_eq!(event.current_status, LearningSessionStatus::Active);
        assert_eq!(event.target_sample_count, 1000);
    }
}
