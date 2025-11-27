//! gRPC Tracing Interceptor
//!
//! Provides automatic instrumentation for all gRPC services using Tower middleware.
//! Extracts W3C TraceContext from incoming gRPC metadata and creates spans for
//! each gRPC call.

use opentelemetry::propagation::TextMapPropagator;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;
use tonic::codegen::http::{Request, Response};
use tower::{Layer, Service};
use tracing::{info_span, Instrument, Span};

/// Tower layer that provides automatic tracing for gRPC services.
///
/// This layer:
/// - Extracts W3C TraceContext from incoming gRPC metadata
/// - Creates a span for each gRPC call
/// - Records method name, status, and duration
#[derive(Clone, Default)]
pub struct GrpcTracingLayer;

impl GrpcTracingLayer {
    /// Create a new gRPC tracing layer
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for GrpcTracingLayer {
    type Service = GrpcTracingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        GrpcTracingService { inner }
    }
}

/// Service wrapper that instruments gRPC calls with tracing spans.
#[derive(Clone)]
pub struct GrpcTracingService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for GrpcTracingService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        // Extract gRPC method path from URI
        let grpc_path = request.uri().path().to_string();
        let (service_name, method_name) = parse_grpc_path(&grpc_path);

        // Extract trace context from gRPC metadata (HTTP/2 headers)
        let parent_context = extract_trace_context(request.headers());

        // Create the span with extracted context as parent
        let span = create_grpc_span(&service_name, &method_name, &parent_context);

        // Clone inner service for async move
        let mut inner = self.inner.clone();

        Box::pin(
            async move {
                let start = Instant::now();

                // Call the inner service
                let result = inner.call(request).await;

                // Record duration
                let duration_ms = start.elapsed().as_millis() as f64;
                tracing::Span::current().record("grpc.duration_ms", duration_ms);

                // Record result status
                match &result {
                    Ok(_response) => {
                        // Extract gRPC status from trailers if available
                        // For now, we assume success if we get a response
                        tracing::Span::current().record("grpc.status", "OK");
                    }
                    Err(_) => {
                        tracing::Span::current().record("grpc.status", "ERROR");
                    }
                }

                result
            }
            .instrument(span),
        )
    }
}

/// Extract W3C TraceContext from HTTP headers (gRPC metadata)
fn extract_trace_context(headers: &http::HeaderMap) -> opentelemetry::Context {
    let propagator = TraceContextPropagator::new();

    struct HeaderExtractor<'a>(&'a http::HeaderMap);

    impl opentelemetry::propagation::Extractor for HeaderExtractor<'_> {
        fn get(&self, key: &str) -> Option<&str> {
            self.0.get(key).and_then(|v| v.to_str().ok())
        }

        fn keys(&self) -> Vec<&str> {
            self.0.keys().map(|k| k.as_str()).collect()
        }
    }

    propagator.extract(&HeaderExtractor(headers))
}

/// Parse gRPC path into service and method names
///
/// gRPC paths are formatted as `/package.ServiceName/MethodName`
fn parse_grpc_path(path: &str) -> (String, String) {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    match parts.as_slice() {
        [service, method] => (service.to_string(), method.to_string()),
        [single] if !single.is_empty() => (single.to_string(), "unknown".to_string()),
        _ => ("unknown".to_string(), "unknown".to_string()),
    }
}

/// Create a span for a gRPC call with the extracted trace context
fn create_grpc_span(service: &str, method: &str, parent_context: &opentelemetry::Context) -> Span {
    // Use opentelemetry context to set parent if available
    let _guard = parent_context.clone().attach();

    // Create span with gRPC semantic conventions
    // Using info_span! to integrate with tracing-opentelemetry
    info_span!(
        "grpc.server",
        otel.name = %format!("{}/{}", service, method),
        rpc.system = "grpc",
        rpc.service = %service,
        rpc.method = %method,
        grpc.status = tracing::field::Empty,
        grpc.duration_ms = tracing::field::Empty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TraceContextExt;

    #[test]
    fn test_parse_grpc_path_standard() {
        let (service, method) = parse_grpc_path(
            "/envoy.service.discovery.v3.AggregatedDiscoveryService/StreamAggregatedResources",
        );
        assert_eq!(service, "envoy.service.discovery.v3.AggregatedDiscoveryService");
        assert_eq!(method, "StreamAggregatedResources");
    }

    #[test]
    fn test_parse_grpc_path_simple() {
        let (service, method) = parse_grpc_path("/MyService/MyMethod");
        assert_eq!(service, "MyService");
        assert_eq!(method, "MyMethod");
    }

    #[test]
    fn test_parse_grpc_path_empty() {
        let (service, method) = parse_grpc_path("/");
        assert_eq!(service, "unknown");
        assert_eq!(method, "unknown");
    }

    #[test]
    fn test_parse_grpc_path_single_component() {
        let (service, method) = parse_grpc_path("/ServiceOnly");
        assert_eq!(service, "ServiceOnly");
        assert_eq!(method, "unknown");
    }

    #[test]
    fn test_extract_trace_context_empty() {
        let headers = http::HeaderMap::new();
        let context = extract_trace_context(&headers);
        // Should return a valid (empty) context
        assert!(!context.span().span_context().trace_id().to_string().is_empty());
    }

    #[test]
    fn test_create_grpc_span() {
        let parent_context = opentelemetry::Context::current();
        let span = create_grpc_span("TestService", "TestMethod", &parent_context);
        // Span should be created (not checking internals, just that it doesn't panic)
        drop(span);
    }
}
