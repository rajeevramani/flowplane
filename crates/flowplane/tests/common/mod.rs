//! Shared, parallel-safe test scaffolding for the CLI conformance suite.
//!
//! This module is neutral test infrastructure — an ephemeral HTTP mock that mimics the
//! Flowplane REST surface, plus a helper that runs the *built* `flowplane` binary as a
//! subprocess with an isolated `HOME`/`FLOWPLANE_CONFIG` and explicit per-child env. It
//! deliberately contains no assertions and no knowledge of the CLI's output internals:
//! tests drive the binary black-box and assert against the documented contract.
//!
//! Parallel-safety (invariant 18): the mock binds `127.0.0.1:0` (unique port per test) and
//! every child process gets its own temp dir + env, so the suite runs green under default
//! nextest parallelism with no `--test-threads=1`.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A running mock server. Aborts its task on drop.
pub struct MockServer {
    base_url: String,
    handle: JoinHandle<()>,
}

impl MockServer {
    /// `http://127.0.0.1:<port>` to pass as `FLOWPLANE_SERVER` / `--server`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Start an ephemeral mock of the resource REST surface on a unique loopback port.
///
/// Behavior (enough for the output-model slice):
/// - `GET    /api/v1/teams/{team}/clusters`         → 200, a two-item list
/// - `GET    /api/v1/teams/{team}/clusters/{name}`  → 200, a single object
/// - `POST   /api/v1/teams/{team}/clusters`         → 201, the created object
/// - `PATCH  /api/v1/teams/{team}/clusters/{name}`  → 200, the updated object
/// - `DELETE /api/v1/teams/{team}/clusters/{name}`  → 204, empty body
pub async fn start_mock() -> MockServer {
    let app = Router::new()
        .route(
            "/api/v1/teams/{team}/clusters",
            get(list_clusters).post(create_cluster),
        )
        .route(
            "/api/v1/teams/{team}/clusters/{name}",
            get(get_cluster)
                .patch(update_cluster)
                .delete(delete_cluster),
        );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server to an ephemeral port");
    let addr = listener.local_addr().expect("mock server local addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    MockServer {
        base_url: format!("http://{addr}"),
        handle,
    }
}

async fn list_clusters(Path(_team): Path<String>) -> Json<Value> {
    Json(json!([
        { "name": "alpha", "revision": 1, "service_name": "alpha-svc" },
        { "name": "beta", "revision": 2, "service_name": "beta-svc" }
    ]))
}

/// Error injection: a resource name like `err-404`/`err-429`/`err-503`/`err-401`/`err-403`
/// makes the item endpoints return that HTTP status with a JSON error body, so error-path
/// tests can drive every status class deterministically. Any other name is a normal 200.
fn injected_error(name: &str) -> Option<(StatusCode, Value)> {
    let code = name.strip_prefix("err-")?.parse::<u16>().ok()?;
    let status = StatusCode::from_u16(code).ok()?;
    let body = match status {
        // 401 deliberately omits a `hint` so the client must synthesize the login hint.
        StatusCode::UNAUTHORIZED => {
            json!({ "code": "unauthorized", "message": "missing or invalid token" })
        }
        // 403 names the (resource, action) the way deny_to_error does server-side.
        StatusCode::FORBIDDEN => json!({
            "code": "forbidden",
            "message": "access denied",
            "hint": "forbidden: team:payments resource=clusters action=delete"
        }),
        StatusCode::NOT_FOUND => json!({ "code": "not_found", "message": "cluster not found" }),
        StatusCode::TOO_MANY_REQUESTS => json!({ "code": "rate_limited", "message": "slow down" }),
        _ => json!({ "code": status.as_str(), "message": "server error" }),
    };
    Some((status, body))
}

async fn get_cluster(Path((_team, name)): Path<(String, String)>) -> (StatusCode, Json<Value>) {
    if let Some((status, body)) = injected_error(&name) {
        return (status, Json(body));
    }
    (
        StatusCode::OK,
        Json(json!({ "name": name, "revision": 1, "service_name": "svc" })),
    )
}

async fn create_cluster(
    Path(_team): Path<String>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("created")
        .to_string();
    (
        StatusCode::CREATED,
        Json(json!({ "name": name, "revision": 1 })),
    )
}

async fn update_cluster(
    Path((_team, name)): Path<(String, String)>,
    Json(_body): Json<Value>,
) -> Json<Value> {
    Json(json!({ "name": name, "revision": 2 }))
}

async fn delete_cluster(Path((_team, name)): Path<(String, String)>) -> Response {
    if let Some((status, body)) = injected_error(&name) {
        return (status, Json(body)).into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// A loopback URL with no listener — connecting to it fails fast with "connection refused",
/// for exercising the transport-error path. Binds an ephemeral port then drops it so the
/// port is (almost certainly) free again, avoiding a fixed-port collision.
pub fn dead_base_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    format!("http://{addr}")
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique temp directory for one test's isolated `HOME`/`FLOWPLANE_CONFIG`.
pub fn unique_tempdir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flowplane-cli-it-{}-{nanos}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// A `Command` for the built `flowplane` binary with a clean, isolated environment.
///
/// Inherits nothing output-affecting from the parent: `HOME` and `FLOWPLANE_CONFIG` point
/// at a fresh temp dir, `NO_COLOR` is cleared, and `--server`/token are left to the caller.
pub fn flowplane_cmd(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    cmd.env_clear();
    // Keep a minimal PATH so the process can run; everything else is explicit.
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home);
    cmd.env("FLOWPLANE_CONFIG", home.join("config.toml"));
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("FLOWPLANE_TOKEN");
    cmd.env_remove("FLOWPLANE_SERVER");
    cmd
}
