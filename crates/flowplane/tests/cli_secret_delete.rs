//! Black-box CLI contract for supported secret deletion.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::delete;
use axum::Router;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
struct RecordedDelete {
    path: String,
    if_match: Option<String>,
    body_empty: bool,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Option<RecordedDelete>>>);

async fn delete_secret(
    State(recorder): State<Recorder>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> StatusCode {
    *recorder.0.lock().expect("recorder lock") = Some(RecordedDelete {
        path: format!("/api/v1/teams/{team}/secrets/{name}"),
        if_match: headers
            .get("if-match")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body_empty: body.is_empty(),
    });
    StatusCode::NO_CONTENT
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secret_delete_sends_revisioned_bodyless_rest_delete() {
    let recorder = Recorder::default();
    let app = Router::new()
        .route("/api/v1/teams/{team}/secrets/{name}", delete(delete_secret))
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
            "secret",
            "delete",
            "db-password",
            "--team",
            "payments",
            "--revision",
            "7",
            "--yes",
            "--output",
            "json",
        ])
        .output()
        .expect("run secret delete");
    server.abort();

    assert_eq!(
        output.status.code(),
        Some(0),
        "secret delete must succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let recorded = recorder
        .0
        .lock()
        .expect("recorder lock")
        .clone()
        .expect("CLI DELETE request");
    assert_eq!(recorded.path, "/api/v1/teams/payments/secrets/db-password");
    assert_eq!(recorded.if_match.as_deref(), Some("7"));
    assert!(recorded.body_empty, "secret deletion has no request body");
}

#[test]
fn secret_delete_requires_confirmation_before_network_access() {
    let home = common::unique_tempdir();
    let output = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", "http://127.0.0.1:9")
        .env("FLOWPLANE_TOKEN", "test-token")
        .args([
            "secret",
            "delete",
            "db-password",
            "--team",
            "payments",
            "--revision",
            "7",
        ])
        .output()
        .expect("run unconfirmed secret delete");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--yes"), "confirmation guidance: {stderr}");
    assert!(
        !stderr.contains("connect"),
        "must fail before network: {stderr}"
    );
}

#[test]
fn secret_delete_help_documents_revision_confirmation_and_no_reason() {
    let home = common::unique_tempdir();
    let output = common::flowplane_cmd(&home)
        .args(["secret", "delete", "--help"])
        .output()
        .expect("run secret delete help");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    for contract in ["--revision", "--yes", "--team"] {
        assert!(
            help.contains(contract),
            "help must document {contract}: {help}"
        );
    }
    assert!(
        !help.contains("--reason"),
        "secret delete has no reason flag: {help}"
    );
}
