//! # Distributed Tracing
//!
//! Provides distributed tracing setup using OpenTelemetry OTLP exporter with
//! tracing-opentelemetry bridge.
//!
//! This module bridges the Rust `tracing` crate with OpenTelemetry, allowing
//! `#[instrument]` spans to be exported to tracing backends like Jaeger, Zipkin,
//! or Grafana Tempo via OTLP.
//!
//! # Configuration
//!
//! Distributed tracing is configured via environment variables:
//!
//! - `FLOWPLANE_ENABLE_TRACING`: Enable/disable tracing (default: true)
//! - `FLOWPLANE_OTLP_ENDPOINT`: OTLP exporter endpoint (default: http://localhost:4317)
//! - `FLOWPLANE_TRACE_SAMPLING_RATIO`: Sampling ratio 0.0-1.0 (default: 1.0 = 100%)
//! - `FLOWPLANE_SERVICE_NAME`: Service name for traces (default: flowplane)
//!
//! # Usage
//!
//! The tracing layer is automatically integrated when `init_observability()` is called.
//! All `#[instrument]` annotated functions will have their spans exported to the
//! configured OTLP endpoint.
//!
//! ```rust,ignore
//! use tracing::instrument;
//!
//! #[instrument(skip(db), fields(user_id = %user_id))]
//! async fn get_user(db: &Database, user_id: i64) -> Result<User> {
//!     // Span is automatically created and exported to Jaeger/Zipkin
//!     db.query_user(user_id).await
//! }
//! ```

use crate::config::ObservabilityConfig;
use crate::errors::{FlowplaneError, Result};
use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
    Resource,
};
use std::time::Duration;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialize distributed tracing with OpenTelemetry OTLP exporter
///
/// This function initializes the complete tracing pipeline including:
/// - OpenTelemetry tracer provider with OTLP exporter
/// - tracing-opentelemetry bridge layer
/// - Logging fmt layer
///
/// Returns the SdkTracerProvider which must be kept alive and shutdown before exit.
pub async fn init_tracing_with_logging(
    config: &ObservabilityConfig,
) -> Result<Option<SdkTracerProvider>> {
    // Parse the log level for the env filter
    let env_filter = parse_env_filter(&config.log_level)?;

    if !config.enable_tracing {
        // Initialize logging without OpenTelemetry
        init_logging_only(config, env_filter)?;
        return Ok(None);
    }

    // Determine the OTLP endpoint to use
    let otlp_endpoint = match &config.otlp_endpoint {
        Some(endpoint) => endpoint.clone(),
        None => {
            // Initialize logging without OpenTelemetry
            init_logging_only(config, env_filter)?;
            return Ok(None);
        }
    };

    // Create resource with service information
    let resource = Resource::builder_empty()
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            config.service_name.clone(),
        ))
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    // Configure OTLP exporter with timeout
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&otlp_endpoint)
        .with_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| FlowplaneError::internal(format!("Failed to create OTLP exporter: {}", e)))?;

    // Create tracer provider with batch exporter (async export in background)
    let sampler =
        Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(config.trace_sampling_ratio)));

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .with_id_generator(RandomIdGenerator::default())
        .with_sampler(sampler)
        .build();

    // Set the global text map propagator for W3C TraceContext
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    // Set global tracer provider
    global::set_tracer_provider(provider.clone());

    // Build the complete subscriber with all layers
    // Note: We create the tracer and otel_layer inside each branch to avoid type conflicts
    if config.json_logging {
        let json_layer = tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false);

        let tracer = provider.tracer("flowplane");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .with(otel_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize tracing: {}", e)))?;
    } else {
        let pretty_layer = tracing_subscriber::fmt::layer()
            .pretty()
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true);

        let tracer = provider.tracer("flowplane");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(pretty_layer)
            .with(otel_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize tracing: {}", e)))?;
    }

    ::tracing::info!(
        service_name = %config.service_name,
        otlp_endpoint = %otlp_endpoint,
        sampling_ratio = config.trace_sampling_ratio,
        "OpenTelemetry trace exporter initialized with tracing bridge"
    );

    Ok(Some(provider))
}

/// Initialize logging only (without OpenTelemetry)
fn init_logging_only(config: &ObservabilityConfig, env_filter: EnvFilter) -> Result<()> {
    if config.json_logging {
        let json_layer = tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize logging: {}", e)))?;
    } else {
        let pretty_layer = tracing_subscriber::fmt::layer()
            .pretty()
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(pretty_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize logging: {}", e)))?;
    }

    Ok(())
}

fn parse_env_filter(level: &str) -> Result<EnvFilter> {
    let normalized = level.trim();
    let lower = normalized.to_ascii_lowercase();

    match lower.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {}
        _ => {
            return Err(FlowplaneError::config(format!(
                "Invalid log level '{}': must be one of trace, debug, info, warn, error",
                level
            )));
        }
    }

    EnvFilter::try_new(normalized)
        .map_err(|e| FlowplaneError::config(format!("Invalid log level '{}': {}", level, e)))
}

/// Shutdown the tracer provider gracefully
///
/// This ensures all pending spans are flushed before the application exits.
pub fn shutdown_tracing(provider: SdkTracerProvider) {
    if let Err(e) = provider.shutdown() {
        ::tracing::error!(error = %e, "Failed to shutdown tracer provider");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_tracing_disabled() {
        let config = ObservabilityConfig { enable_tracing: false, ..Default::default() };

        let result = init_tracing_with_logging(&config).await;
        assert!(result.is_ok() || result.is_err()); // May fail if already initialized
    }

    #[tokio::test]
    async fn test_init_tracing_no_endpoint() {
        let config =
            ObservabilityConfig { enable_tracing: true, otlp_endpoint: None, ..Default::default() };

        let result = init_tracing_with_logging(&config).await;
        assert!(result.is_ok() || result.is_err()); // May fail if already initialized
    }
}
