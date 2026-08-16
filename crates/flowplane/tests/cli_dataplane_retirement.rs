//! RED black-box CLI contract for fpv2-7f3.7 dataplane retirement.
//!
//! Drives the built `flowplane` binary against an ephemeral HTTP recorder. No CLI implementation
//! source is inspected; argv, request path/body/precondition, exit status, and output are observable.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
struct RecordedDelete {
    path: String,
    if_match: Option<String>,
    reason: Option<String>,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Option<RecordedDelete>>>);

async fn get_dataplane(Path((_team, name)): Path<(String, String)>) -> Json<Value> {
    Json(json!({"id": uuid::Uuid::now_v7(), "name": name, "revision": 4}))
}

async fn delete_dataplane(
    State(recorder): State<Recorder>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    *recorder.0.lock().expect("recorder lock") = Some(RecordedDelete {
        path: format!("/api/v1/teams/{team}/dataplanes/{name}"),
        if_match: headers
            .get("if-match")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        reason: json["reason"].as_str().map(str::to_owned),
    });
    StatusCode::NO_CONTENT
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dataplane_delete_cli_sends_reason_and_explicit_revision_to_rest_delete() {
    let recorder = Recorder::default();
    let app = Router::new()
        .route(
            "/api/v1/teams/{team}/dataplanes/{name}",
            get(get_dataplane).delete(delete_dataplane),
        )
        .with_state(recorder.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ephemeral CLI recorder");
    let address = listener.local_addr().expect("recorder address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let home = common::unique_tempdir();

    let output = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", format!("http://{address}"))
        .env("FLOWPLANE_TOKEN", "test-token")
        .args([
            "dataplane",
            "delete",
            "edge-a",
            "--team",
            "payments",
            "--revision",
            "7",
            "--reason",
            "operator decommission",
            "--yes",
            "--output",
            "json",
        ])
        .output()
        .expect("run built flowplane dataplane delete");
    server.abort();

    assert_eq!(
        output.status.code(),
        Some(0),
        "dataplane delete must be a supported successful command; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "success writes no stderr");
    let recorded = recorder
        .0
        .lock()
        .expect("recorder lock")
        .clone()
        .expect("CLI must issue a DELETE request");
    assert_eq!(recorded.path, "/api/v1/teams/payments/dataplanes/edge-a");
    assert_eq!(recorded.if_match.as_deref(), Some("7"));
    assert_eq!(recorded.reason.as_deref(), Some("operator decommission"));
}

#[test]
fn dataplane_help_exposes_delete_as_a_destructive_revisioned_command() {
    let home = common::unique_tempdir();
    let output = common::flowplane_cmd(&home)
        .args(["dataplane", "delete", "--help"])
        .output()
        .expect("run dataplane delete help");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    for contract in ["--revision", "--reason", "--yes", "--team"] {
        assert!(
            help.contains(contract),
            "delete help must document {contract}: {help}"
        );
    }
}
