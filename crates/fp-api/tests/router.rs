//! In-process router integration tests (real PostgreSQL, no network listener).
//!
//! These complement the CI boot smoke: same contracts, but failures pinpoint the router
//! rather than the whole binary. Skipped (with a notice) when FLOWPLANE_TEST_DATABASE_URL
//! is unset; CI always sets it.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt;

async fn test_app() -> Option<axum::Router> {
    test_app_with_xds_readiness(None).await
}

async fn test_app_with_xds_readiness(
    xds_readiness: Option<fp_api::state::XdsReadiness>,
) -> Option<axum::Router> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 2)
        .await
        .expect("test database must be reachable");
    fp_storage::migrate(&pool).await.expect("migrations apply");
    // Recorder may already be installed by a sibling test; both cases are fine.
    let prometheus = PrometheusBuilder::new().build_recorder().handle();
    Some(fp_api::build_router(fp_api::AppState {
        pool,
        prometheus,
        version: "test",
        validator: None,
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(120)),
        xds_readiness,
    }))
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body must be JSON")
}

#[tokio::test]
async fn healthz_reports_ok_and_version() {
    let Some(app) = test_app().await else { return };
    let response = app
        .oneshot(
            Request::get("/healthz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["version"], "test");
}

#[tokio::test]
async fn readyz_passes_with_live_database() {
    let Some(app) = test_app().await else { return };
    let response = app
        .oneshot(
            Request::get("/readyz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["status"], "ready");
    assert_eq!(json["checks"][0]["name"], "database");
    assert_eq!(json["checks"][0]["ok"], true);
}

#[tokio::test]
async fn readyz_fails_when_xds_consumer_failed() {
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let Some(app) = test_app_with_xds_readiness(Some(fp_api::state::XdsReadiness {
        consumer: "test-xds",
        max_lag: 0,
        failed,
    }))
    .await
    else {
        return;
    };
    let response = app
        .oneshot(
            Request::get("/readyz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json = body_json(response).await;
    let checks = json["details"]["checks"].as_array().expect("checks");
    assert!(checks.iter().any(|check| {
        check["name"] == "xds_outbox_consumer"
            && check["ok"] == false
            && check["detail"] == "consumer task exited with error"
    }));
}

#[tokio::test]
async fn unknown_path_returns_standard_envelope_with_request_id() {
    let Some(app) = test_app().await else { return };
    let response = app
        .oneshot(
            Request::get("/definitely/not/a/route")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let rid_header = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .expect("x-request-id header present");
    let json = body_json(response).await;
    assert_eq!(json["code"], "not_found");
    assert!(json["hint"].as_str().is_some(), "404 carries a hint");
    assert_eq!(json["request_id"], rid_header, "envelope and header agree");
}

#[tokio::test]
async fn valid_inbound_request_id_is_honored_and_echoed() {
    let Some(app) = test_app().await else { return };
    let supplied = "0196fdb1-7000-7000-8000-00000000abcd";
    let response = app
        .oneshot(
            Request::get("/healthz")
                .header("x-request-id", supplied)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
        Some(supplied)
    );
}

#[tokio::test]
async fn malformed_inbound_request_id_is_replaced_not_trusted() {
    let Some(app) = test_app().await else { return };
    // Transport-invalid bytes (control chars) are already rejected by the HTTP layer;
    // these are transport-VALID values that are not UUIDs — our middleware must replace
    // them rather than echo attacker-controlled strings into logs and error bodies.
    for hostile in [
        "not-a-uuid",
        "0196fdb1-7000-7000-8000-tooshort",
        "'; DROP TABLE users;--",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get("/healthz")
                    .header("x-request-id", hostile)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let echoed = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .expect("header");
        assert_ne!(echoed, hostile, "hostile value must not be echoed");
        assert!(
            uuid::Uuid::parse_str(echoed).is_ok(),
            "replacement is a valid UUID, got {echoed:?}"
        );
    }
}
