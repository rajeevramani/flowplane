//! # Distributed Tracing
//!
//! Provides distributed tracing setup using OpenTelemetry and Jaeger.

use crate::config::ObservabilityConfig;
use crate::errors::{MagayaError, Result};

/// Initialize distributed tracing with OpenTelemetry and Jaeger
pub async fn init_tracing(config: &ObservabilityConfig) -> Result<()> {
    if !config.enable_tracing {
        return Ok(());
    }

    // For now, we'll implement a basic setup without external dependencies
    // In a production environment, you would set up OpenTelemetry with Jaeger

    tracing::info!(
        service_name = %config.service_name,
        jaeger_endpoint = ?config.jaeger_endpoint,
        "Distributed tracing configuration loaded (implementation pending)"
    );

    // TODO: Implement actual OpenTelemetry/Jaeger integration
    // This would involve setting up the OpenTelemetry tracer with Jaeger exporter
    // For now, we're just using the built-in tracing subscriber setup in logging.rs

    Ok(())
}

/// Create a tracing context for cross-service requests
pub struct TracingContext {
    /// Trace ID for correlation across services
    pub trace_id: String,
    /// Span ID for the current operation
    pub span_id: String,
    /// Parent span ID if this is a child span
    pub parent_span_id: Option<String>,
    /// Baggage for carrying context
    pub baggage: std::collections::HashMap<String, String>,
}

impl TracingContext {
    /// Create a new root tracing context
    pub fn new_root() -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            span_id: uuid::Uuid::new_v4().to_string(),
            parent_span_id: None,
            baggage: std::collections::HashMap::new(),
        }
    }

    /// Create a child tracing context
    pub fn new_child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: uuid::Uuid::new_v4().to_string(),
            parent_span_id: Some(self.span_id.clone()),
            baggage: self.baggage.clone(),
        }
    }

    /// Add baggage item
    pub fn with_baggage<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.baggage.insert(key.into(), value.into());
        self
    }

    /// Get baggage item
    pub fn get_baggage(&self, key: &str) -> Option<&String> {
        self.baggage.get(key)
    }

    /// Convert to HTTP headers for propagation
    pub fn to_headers(&self) -> std::collections::HashMap<String, String> {
        let mut headers = std::collections::HashMap::new();

        // Use standard trace context headers
        headers.insert("traceparent".to_string(),
            format!("00-{}-{}-01", self.trace_id, self.span_id));

        if let Some(parent) = &self.parent_span_id {
            headers.insert("tracestate".to_string(),
                format!("parent={}", parent));
        }

        // Add baggage
        if !self.baggage.is_empty() {
            let baggage_str = self.baggage
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            headers.insert("baggage".to_string(), baggage_str);
        }

        headers
    }

    /// Create from HTTP headers
    pub fn from_headers(headers: &std::collections::HashMap<String, String>) -> Option<Self> {
        let traceparent = headers.get("traceparent")?;
        let parts: Vec<&str> = traceparent.split('-').collect();

        if parts.len() != 4 || parts[0] != "00" {
            return None;
        }

        let trace_id = parts[1].to_string();
        let span_id = parts[2].to_string();

        let parent_span_id = headers.get("tracestate")
            .and_then(|ts| ts.strip_prefix("parent="))
            .map(|p| p.to_string());

        let mut baggage = std::collections::HashMap::new();
        if let Some(baggage_str) = headers.get("baggage") {
            for item in baggage_str.split(',') {
                if let Some((key, value)) = item.split_once('=') {
                    baggage.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }

        Some(Self {
            trace_id,
            span_id,
            parent_span_id,
            baggage,
        })
    }
}

/// Macro for creating a traced async function
#[macro_export]
macro_rules! traced_fn {
    ($name:expr, $($field:tt)*) => {
        tracing::info_span!($name, $($field)*)
    };
}

/// Helper for extracting trace context from HTTP requests
pub fn extract_trace_context(headers: &axum::http::HeaderMap) -> Option<TracingContext> {
    let header_map: std::collections::HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    TracingContext::from_headers(&header_map)
}

/// Helper for injecting trace context into HTTP requests
pub fn inject_trace_context(
    context: &TracingContext,
    headers: &mut axum::http::HeaderMap,
) -> Result<()> {
    let context_headers = context.to_headers();

    for (key, value) in context_headers {
        let header_name = key.parse::<axum::http::HeaderName>()
            .map_err(|e| MagayaError::internal(format!("Invalid header name '{}': {}", key, e)))?;
        let header_value = value.parse::<axum::http::HeaderValue>()
            .map_err(|e| MagayaError::internal(format!("Invalid header value '{}': {}", value, e)))?;

        headers.insert(header_name, header_value);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_tracing_disabled() {
        let config = ObservabilityConfig {
            enable_tracing: false,
            ..Default::default()
        };

        let result = init_tracing(&config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_init_tracing_enabled() {
        let config = ObservabilityConfig {
            enable_tracing: true,
            jaeger_endpoint: Some("http://localhost:14268/api/traces".to_string()),
            ..Default::default()
        };

        let result = init_tracing(&config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_tracing_context_creation() {
        let root_context = TracingContext::new_root();
        assert!(!root_context.trace_id.is_empty());
        assert!(!root_context.span_id.is_empty());
        assert!(root_context.parent_span_id.is_none());
        assert!(root_context.baggage.is_empty());

        let child_context = root_context.new_child();
        assert_eq!(child_context.trace_id, root_context.trace_id);
        assert_ne!(child_context.span_id, root_context.span_id);
        assert_eq!(child_context.parent_span_id, Some(root_context.span_id));
    }

    #[test]
    fn test_tracing_context_baggage() {
        let context = TracingContext::new_root()
            .with_baggage("user_id", "12345")
            .with_baggage("session_id", "abcdef");

        assert_eq!(context.get_baggage("user_id"), Some(&"12345".to_string()));
        assert_eq!(context.get_baggage("session_id"), Some(&"abcdef".to_string()));
        assert_eq!(context.get_baggage("nonexistent"), None);
    }

    #[test]
    fn test_tracing_context_headers() {
        let context = TracingContext::new_root()
            .with_baggage("user_id", "12345");

        let headers = context.to_headers();
        assert!(headers.contains_key("traceparent"));
        assert!(headers.contains_key("baggage"));

        let traceparent = headers.get("traceparent").unwrap();
        assert!(traceparent.starts_with("00-"));
        assert!(traceparent.contains(&context.trace_id));
        assert!(traceparent.contains(&context.span_id));

        let baggage = headers.get("baggage").unwrap();
        assert!(baggage.contains("user_id=12345"));
    }

    #[test]
    fn test_tracing_context_from_headers() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("traceparent".to_string(),
            "00-12345678901234567890123456789012-1234567890123456-01".to_string());
        headers.insert("baggage".to_string(), "user_id=12345,session_id=abcdef".to_string());

        let context = TracingContext::from_headers(&headers).unwrap();
        assert_eq!(context.trace_id, "12345678901234567890123456789012");
        assert_eq!(context.span_id, "1234567890123456");
        assert_eq!(context.get_baggage("user_id"), Some(&"12345".to_string()));
        assert_eq!(context.get_baggage("session_id"), Some(&"abcdef".to_string()));
    }

    #[test]
    fn test_tracing_context_invalid_headers() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("traceparent".to_string(), "invalid".to_string());

        let context = TracingContext::from_headers(&headers);
        assert!(context.is_none());
    }

    #[test]
    fn test_tracing_context_roundtrip() {
        let original = TracingContext::new_root()
            .with_baggage("user_id", "12345")
            .with_baggage("request_id", "req-67890");

        let headers = original.to_headers();
        let reconstructed = TracingContext::from_headers(&headers).unwrap();

        assert_eq!(original.trace_id, reconstructed.trace_id);
        assert_eq!(original.span_id, reconstructed.span_id);
        assert_eq!(original.get_baggage("user_id"), reconstructed.get_baggage("user_id"));
        assert_eq!(original.get_baggage("request_id"), reconstructed.get_baggage("request_id"));
    }

    #[test]
    fn test_macro_compilation() {
        let _span = traced_fn!("test_operation", user_id = "12345");
    }
}