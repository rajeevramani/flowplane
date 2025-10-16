//! # HTTP Request Tracing Middleware
//!
//! Custom Axum middleware that creates OpenTelemetry spans for HTTP requests.
//! This replaces tower-http's TraceLayer with direct OpenTelemetry SDK integration.

use axum::{extract::Request, middleware::Next, response::Response};
use opentelemetry::{
    global,
    trace::{Span, SpanKind, Status, Tracer},
    KeyValue,
};
use std::time::Instant;

/// Axum middleware that creates an OpenTelemetry span for each HTTP request
///
/// This middleware:
/// - Creates a span with http.method, http.route, http.status_code attributes
/// - Records request latency
/// - Sets span status based on HTTP status code
/// - Works with any OpenTelemetry backend (Zipkin, Jaeger, etc.)
pub async fn trace_http_requests(request: Request, next: Next) -> Response {
    use opentelemetry::trace::{FutureExt, TraceContextExt};

    let tracer = global::tracer("flowplane-http");

    // Extract request information
    let method = request.method().to_string();
    let uri = request.uri().path().to_string();
    let start = Instant::now();

    // Create span with http semantic conventions
    let mut span = tracer
        .span_builder(format!("{} {}", method, uri))
        .with_kind(SpanKind::Server)
        .start(&tracer);

    // Debug: Log that span was created
    tracing::info!(
        method = %method,
        uri = %uri,
        "OpenTelemetry span created for HTTP request"
    );

    // Set span attributes after creation
    span.set_attribute(KeyValue::new("http.method", method.clone()));
    span.set_attribute(KeyValue::new("http.route", uri.clone()));

    // Execute the request within the span's context so downstream operations are nested
    let cx = opentelemetry::Context::current().with_span(span);
    let response = async move {
        next.run(request).await
    }
    .with_context(cx.clone())
    .await;

    // Get the span back from context to update attributes
    let span = cx.span();

    // Record response attributes
    let status_code = response.status().as_u16();
    let elapsed = start.elapsed();

    // Update span with response information
    span.set_attribute(KeyValue::new("http.status_code", status_code as i64));
    span.set_attribute(KeyValue::new("http.response_time_ms", elapsed.as_millis() as i64));

    // Set span status based on HTTP status code
    if status_code >= 500 {
        span.set_status(Status::error("Server error"));
    } else if status_code >= 400 {
        span.set_status(Status::error("Client error"));
    } else {
        span.set_status(Status::Ok);
    }

    // Debug: Log span completion
    tracing::info!(
        status_code = status_code,
        elapsed_ms = elapsed.as_millis(),
        "OpenTelemetry span completed, dropping for export"
    );

    // Span is exported when dropped (via context drop)
    drop(cx);

    response
}

/// Create a span for a specific operation (for manual instrumentation)
///
/// Use this helper to create spans in business logic code that isn't HTTP-related.
///
/// # Example
///
/// ```rust,no_run
/// use flowplane::observability::http_tracing::create_operation_span;
/// use opentelemetry::trace::{SpanKind, Span};
/// use opentelemetry::KeyValue;
///
/// async fn process_data(data: &str) {
///     let mut span = create_operation_span("process_data", SpanKind::Internal);
///     span.set_attribute(KeyValue::new("data.length", data.len() as i64));
///
///     // Do work...
///
///     drop(span); // Export span
/// }
/// ```
pub fn create_operation_span(operation: &str, kind: SpanKind) -> global::BoxedSpan {
    let tracer = global::tracer("flowplane");
    tracer.span_builder(operation.to_string()).with_kind(kind).start(&tracer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::StatusCode, routing::get, Router};
    use http::Request;
    use tower::ServiceExt;

    async fn test_handler() -> &'static str {
        "OK"
    }

    #[tokio::test]
    async fn test_trace_middleware_creates_span() {
        // Note: This test verifies the middleware doesn't panic.
        // Actual span export verification requires an OTLP collector.

        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(axum::middleware::from_fn(trace_http_requests));

        let request = Request::builder().uri("/test").method("GET").body(Body::empty()).unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
