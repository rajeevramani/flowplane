//! # HTTP Request Tracing Middleware
//!
//! Custom Axum middleware that creates OpenTelemetry spans for HTTP requests.
//! This replaces tower-http's TraceLayer with direct OpenTelemetry SDK integration.

use axum::{extract::Request, middleware::Next, response::Response};
use metrics::{counter, histogram};
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
    tracing::debug!(
        method = %method,
        uri = %uri,
        "OpenTelemetry span created for HTTP request"
    );

    // Set span attributes after creation
    span.set_attribute(KeyValue::new("http.method", method.clone()));
    span.set_attribute(KeyValue::new("http.route", uri.clone()));

    // Execute the request within the span's context so downstream operations are nested
    let cx = opentelemetry::Context::current().with_span(span);
    let response = async move { next.run(request).await }.with_context(cx.clone()).await;

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
    tracing::debug!(
        status_code = status_code,
        elapsed_ms = elapsed.as_millis(),
        "OpenTelemetry span completed, dropping for export"
    );

    // Record HTTP metrics for Prometheus
    let method_label = method.clone();
    let path_label = normalize_path_for_metrics(&uri);
    let status_label = format!("{}", status_code);

    counter!(
        "http_requests_total",
        "method" => method_label.clone(),
        "path" => path_label.clone(),
        "status" => status_label.clone()
    )
    .increment(1);

    histogram!(
        "http_request_duration_seconds",
        "method" => method_label,
        "path" => path_label
    )
    .record(elapsed.as_secs_f64());

    // Span is exported when dropped (via context drop)
    drop(cx);

    response
}

/// Normalize path for metrics to avoid high cardinality
///
/// Replaces dynamic path segments (UUIDs, IDs) with placeholders to keep
/// cardinality manageable in Prometheus.
fn normalize_path_for_metrics(path: &str) -> String {
    // Common patterns to normalize:
    // - /api/v1/clusters/{uuid} -> /api/v1/clusters/:id
    // - /api/v1/routes/{uuid} -> /api/v1/routes/:id
    // - /api/v1/listeners/{name} -> /api/v1/listeners/:name

    let segments: Vec<&str> = path.split('/').collect();
    let mut normalized = Vec::with_capacity(segments.len());

    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            normalized.push(*segment);
            continue;
        }

        // Check if this segment looks like a UUID or numeric ID
        let is_uuid =
            segment.len() == 36 && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        let is_numeric = segment.chars().all(|c| c.is_ascii_digit());

        // Also check if the previous segment suggests this is a resource ID
        let prev_is_collection = i > 0
            && matches!(
                segments.get(i - 1).copied(),
                Some("clusters")
                    | Some("routes")
                    | Some("listeners")
                    | Some("teams")
                    | Some("users")
                    | Some("tokens")
                    | Some("learning-sessions")
                    | Some("aggregated-schemas")
            );

        if is_uuid || is_numeric || (prev_is_collection && !segment.is_empty()) {
            // Check if it's actually a known sub-resource path
            if matches!(*segment, "stats" | "health" | "schemas" | "members") {
                normalized.push(*segment);
            } else if prev_is_collection {
                normalized.push(":id");
            } else {
                normalized.push(*segment);
            }
        } else {
            normalized.push(*segment);
        }
    }

    normalized.join("/")
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

    #[test]
    fn test_normalize_path_for_metrics_basic() {
        assert_eq!(normalize_path_for_metrics("/api/v1/clusters"), "/api/v1/clusters");
        assert_eq!(normalize_path_for_metrics("/api/v1/routes"), "/api/v1/routes");
        assert_eq!(normalize_path_for_metrics("/health"), "/health");
    }

    #[test]
    fn test_normalize_path_for_metrics_with_uuid() {
        assert_eq!(
            normalize_path_for_metrics("/api/v1/clusters/550e8400-e29b-41d4-a716-446655440000"),
            "/api/v1/clusters/:id"
        );
        assert_eq!(
            normalize_path_for_metrics("/api/v1/routes/123e4567-e89b-12d3-a456-426614174000"),
            "/api/v1/routes/:id"
        );
    }

    #[test]
    fn test_normalize_path_for_metrics_with_name() {
        // Resource names after collections get normalized to :id
        assert_eq!(
            normalize_path_for_metrics("/api/v1/clusters/my-cluster"),
            "/api/v1/clusters/:id"
        );
        assert_eq!(
            normalize_path_for_metrics("/api/v1/listeners/http-listener"),
            "/api/v1/listeners/:id"
        );
    }

    #[test]
    fn test_normalize_path_for_metrics_preserves_subresources() {
        assert_eq!(
            normalize_path_for_metrics("/api/v1/teams/my-team/members"),
            "/api/v1/teams/:id/members"
        );
    }

    #[test]
    fn test_normalize_path_for_metrics_numeric_id() {
        assert_eq!(normalize_path_for_metrics("/api/v1/users/12345"), "/api/v1/users/:id");
    }
}
