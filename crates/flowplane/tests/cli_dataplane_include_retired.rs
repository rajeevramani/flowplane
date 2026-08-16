//! RED black-box CLI contract for fpv2-7f3.8 `dataplane list --include-retired`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use axum::extract::{Path, RawQuery, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Queries(Arc<Mutex<Vec<String>>>);

async fn list_dataplanes(
    State(queries): State<Queries>,
    Path(_team): Path<String>,
    RawQuery(query): RawQuery,
) -> Json<Value> {
    let query = query.unwrap_or_default();
    queries
        .0
        .lock()
        .expect("query recorder")
        .push(query.clone());
    let active = json!({
        "id": "019f1111-1111-7111-8111-111111111111",
        "name": "edge-a",
        "revision": 2,
        "retired_at": null,
        "retired_reason": null
    });
    let retired = json!({
        "id": "019f2222-2222-7222-8222-222222222222",
        "name": "edge-old",
        "revision": 4,
        "retired_at": "2026-08-16T10:00:00Z",
        "retired_reason": "hardware replacement"
    });
    let items = if query.split('&').any(|part| part == "include_retired=true") {
        vec![active, retired]
    } else {
        vec![active]
    };
    Json(json!({"total": items.len(), "limit": 50, "offset": 0, "items": items}))
}

async fn recorder() -> (String, Queries, tokio::task::JoinHandle<()>) {
    let queries = Queries::default();
    let app = Router::new()
        .route("/api/v1/teams/{team}/dataplanes", get(list_dataplanes))
        .with_state(queries.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ephemeral CLI recorder");
    let address = listener.local_addr().expect("recorder address");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), queries, task)
}

#[test]
fn dataplane_list_help_documents_include_retired() {
    let home = common::unique_tempdir();
    let output = common::flowplane_cmd(&home)
        .args(["dataplane", "list", "--help"])
        .output()
        .expect("run dataplane list help");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(
        help.contains("--include-retired"),
        "dataplane list help must document lifecycle discovery: {help}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn include_retired_cli_sets_query_and_renders_safe_lifecycle_metadata() {
    let (server, queries, task) = recorder().await;
    let home = common::unique_tempdir();
    let output = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", &server)
        .env("FLOWPLANE_TOKEN", "test-token")
        .args([
            "dataplane",
            "list",
            "--team",
            "payments",
            "--include-retired",
            "--output",
            "json",
        ])
        .output()
        .expect("run dataplane list --include-retired");
    task.abort();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recorded = queries.0.lock().expect("query recorder");
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0]
            .split('&')
            .any(|part| part == "include_retired=true"),
        "CLI query must opt into retired history: {:?}",
        recorded[0]
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON CLI output");
    let rendered = envelope.to_string();
    assert!(rendered.contains("edge-old"));
    assert!(rendered.contains("2026-08-16T10:00:00Z"));
    assert!(rendered.contains("hardware replacement"));
    for secret in [
        "private_key",
        "private_key_pem",
        "certificate_pem",
        "secret",
    ] {
        assert!(
            !rendered.contains(secret),
            "CLI leaked field {secret}: {rendered}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_dataplane_list_does_not_request_or_render_retired_rows() {
    let (server, queries, task) = recorder().await;
    let home = common::unique_tempdir();
    let output = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", &server)
        .env("FLOWPLANE_TOKEN", "test-token")
        .args([
            "dataplane",
            "list",
            "--team",
            "payments",
            "--output",
            "json",
        ])
        .output()
        .expect("run default dataplane list");
    task.abort();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recorded = queries.0.lock().expect("query recorder");
    assert_eq!(recorded.len(), 1);
    assert!(!recorded[0].contains("include_retired"));
    let rendered = String::from_utf8(output.stdout).expect("UTF-8 JSON");
    assert!(rendered.contains("edge-a"));
    assert!(!rendered.contains("edge-old"));
    assert!(!rendered.contains("hardware replacement"));
}
