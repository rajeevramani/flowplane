//! Request-id and tracing middleware (spec/10 §8a).
//!
//! Every request gets a [`RequestId`] (honoring a syntactically valid inbound
//! `x-request-id`), exposed to handlers via extensions, echoed in the `x-request-id`
//! response header, and recorded on the request's tracing span so one id links error body,
//! log lines, and trace. Inbound W3C `traceparent` context is honored, so Flowplane spans
//! join the caller's distributed trace.

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use fp_domain::RequestId;
use opentelemetry::propagation::Extractor;
use std::str::FromStr;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

struct HeaderMapExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderMapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

pub async fn request_id(mut request: Request, next: Next) -> Response {
    let rid = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| RequestId::from_str(value).ok())
        .unwrap_or_else(RequestId::generate);

    request.extensions_mut().insert(rid);

    let span = tracing::info_span!(
        "http_request",
        request_id = %rid,
        method = %request.method(),
        path = %request.uri().path(),
        trace_id = tracing::field::Empty,
    );

    // Join the caller's W3C trace context when present (no-op otherwise).
    let parent = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderMapExtractor(request.headers()))
    });
    // Err here means the OTel layer is missing or the span already started — neither is
    // reachable with our init order; degrade to a fresh trace rather than failing the request.
    if let Err(e) = span.set_parent(parent) {
        tracing::debug!("could not set span parent from traceparent: {e}");
    }
    {
        use opentelemetry::trace::TraceContextExt;
        let otel_span_context = span.context();
        let otel_span_context = otel_span_context.span().span_context().clone();
        if otel_span_context.is_valid() {
            span.record(
                "trace_id",
                tracing::field::display(otel_span_context.trace_id()),
            );
        }
    }

    let started = std::time::Instant::now();
    let rid_for_log = rid;
    let mut response = next.run(request).instrument(span.clone()).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let status = response.status().as_u16();
    span.in_scope(|| {
        tracing::info!(request_id = %rid_for_log, status, elapsed_ms, "request completed");
    });
    metrics::counter!("fp_api_requests_total", "status" => status.to_string()).increment(1);
    metrics::histogram!("fp_api_request_duration_ms").record(elapsed_ms as f64);

    if let Ok(value) = HeaderValue::from_str(&rid.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}
