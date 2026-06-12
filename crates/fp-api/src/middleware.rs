//! Request-id and tracing middleware (spec/10 §8a).
//!
//! Every request gets a [`RequestId`] (honoring a syntactically valid inbound
//! `x-request-id`), exposed to handlers via extensions, echoed in the `x-request-id`
//! response header, and recorded on the request's tracing span so one id links error body,
//! log lines, and trace.

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use fp_domain::RequestId;
use std::str::FromStr;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

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
    );
    let _guard = span.enter();

    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let status = response.status().as_u16();
    tracing::info!(request_id = %rid, status, elapsed_ms, "request completed");
    metrics::counter!("fp_api_requests_total", "status" => status.to_string()).increment(1);
    metrics::histogram!("fp_api_request_duration_ms").record(elapsed_ms as f64);

    if let Ok(value) = HeaderValue::from_str(&rid.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}
