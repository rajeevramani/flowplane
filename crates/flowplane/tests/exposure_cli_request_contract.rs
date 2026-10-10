//! Black-box attachment request contract: only the built CLI and an owned HTTP fake.
//! The upstream is inert request metadata; no upstream, database or Envoy is contacted.
//! Fixtures follow the CLI conformance suite's isolated HOME/config/child-env recipe.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs::File;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{Method, StatusCode, Uri};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const DEADLINE: Duration = Duration::from_secs(10);
const EXPOSE_PATH: &str = "/api/v1/teams/test-team/expose";

struct Home(PathBuf);
impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Debug)]
struct CapturedRequest {
    method: Method,
    target: String,
    body: Vec<u8>,
}

struct FakeServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    task: JoinHandle<()>,
}
impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl FakeServer {
    async fn start(status: StatusCode, response: Value) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        // Capture every method and target, not just the expected route: extra reads,
        // mutation attempts, wrong scopes and retries must remain observable.
        let app = Router::new().fallback(move |method: Method, uri: Uri, body: Bytes| {
            let captured = Arc::clone(&captured);
            let response = response.clone();
            async move {
                captured
                    .lock()
                    .expect("request capture lock")
                    .push(CapturedRequest {
                        method,
                        target: uri.to_string(),
                        body: body.to_vec(),
                    });
                (status, Json(response))
            }
        });
        // Ownership passes directly to serve; there is no reserve/release bind race.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned loopback listener");
        let base_url = format!("http://{}", listener.local_addr().expect("fake address"));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve HTTP fake");
        });
        Self {
            base_url,
            requests,
            task,
        }
    }

    async fn stop(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

async fn expose(server: &FakeServer) -> Output {
    let home = Home(common::unique_tempdir());
    let stdout_path = home.0.join("stdout.log");
    let stderr_path = home.0.join("stderr.log");
    // common::flowplane_cmd uses env!("CARGO_BIN_EXE_flowplane"), env_clear(),
    // and an explicit private HOME/FLOWPLANE_CONFIG, never global env mutation.
    let child = common::flowplane_cmd(&home.0)
        .args([
            "expose",
            "http://8.8.8.8:8080",
            "--name",
            "service",
            "--path",
            "/shared",
            "--listener",
            "gateway",
            "--team",
            "test-team",
            "--server",
            &server.base_url,
            "--token",
            "fixture-token",
            "--timeout",
            "2",
            "-o",
            "json",
        ])
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            File::create(&stdout_path).expect("stdout capture"),
        ))
        .stderr(Stdio::from(
            File::create(&stderr_path).expect("stderr capture"),
        ))
        .spawn()
        .expect("spawn actual built CLI");
    let mut child = ChildGuard(child);
    let status = tokio::time::timeout(DEADLINE, async {
        loop {
            if let Some(status) = child.0.try_wait().expect("poll CLI child") {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "CLI exceeded {DEADLINE:?}; stdout: {:?}; stderr: {:?}",
            std::fs::read_to_string(&stdout_path),
            std::fs::read_to_string(&stderr_path)
        )
    });
    Output {
        status,
        stdout: std::fs::read(stdout_path).expect("read CLI stdout"),
        stderr: std::fs::read(stderr_path).expect("read CLI stderr"),
    }
}

fn assert_single_attachment_request(server: &FakeServer) {
    let requests = server.requests.lock().expect("request capture lock");
    assert_eq!(
        requests.len(),
        1,
        "exactly one request: no preflight reads, extra mutations or retries: {requests:?}"
    );
    let request = &requests[0];
    assert_eq!(request.method, Method::POST);
    assert_eq!(
        request.target, EXPOSE_PATH,
        "exact team-scoped endpoint, no query"
    );
    let body: Value = serde_json::from_slice(&request.body).expect("request must be JSON");
    assert!(body.is_object(), "request must be a JSON object: {body}");
    // get() intentionally distinguishes a missing selector from the required string;
    // deleting listener from the outgoing JSON must fail this contract.
    assert_eq!(
        body.get("listener"),
        Some(&json!("gateway")),
        "listener is required: {body}"
    );
    for (key, expected) in [
        ("name", "service"),
        ("upstream", "http://8.8.8.8:8080"),
        ("path", "/shared"),
    ] {
        assert_eq!(
            body.get(key),
            Some(&json!(expected)),
            "preserve {key}: {body}"
        );
    }
    for key in ["port", "public_base_url"] {
        assert!(
            body.get(key).is_none_or(Value::is_null),
            "attachment must not invent {key}: {body}"
        );
    }
}

fn attached_response() -> Value {
    // Public exposure response shape from exposure_shared_contract.rs. Normal CLI
    // envelopes wrap this HTTP object; the fake does not pre-wrap its response.
    json!({
        "mode": "attached",
        "cluster": {"id":"019f9999-0000-7000-8000-000000000001", "name":"service-upstream", "revision":1},
        "route_config": {"id":"019f9999-0000-7000-8000-000000000002", "name":"gateway-routes", "revision":2},
        "listener": {"id":"019f9999-0000-7000-8000-000000000003", "name":"gateway", "revision":1},
        "curl_url": "https://gateway.example/shared",
        "endpoint_source": "listener.public_base_url"
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_attachment_posts_exact_request_and_renders_success() {
    let response = attached_response();
    let mut server = FakeServer::start(StatusCode::CREATED, response.clone()).await;
    let output = expose(&server).await;
    server.stop().await;
    assert_single_attachment_request(&server);
    assert!(
        output.status.success(),
        "valid response must succeed; stdout: {:?}; stderr: {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered: Value = serde_json::from_slice(&output.stdout)
        .expect("entire stdout must be a JSON success envelope");
    assert_eq!(rendered["schemaVersion"], 1);
    assert!(
        rendered["kind"]
            .as_str()
            .is_some_and(|kind| !kind.is_empty()),
        "typed envelope kind: {rendered}"
    );
    assert_eq!(
        rendered["data"], response,
        "valid attachment response must survive parse/render intact"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_attachment_does_not_retry_a_retryable_mutation_failure() {
    let mut server = FakeServer::start(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"code":"unavailable", "message":"synthetic mutation failure"}),
    )
    .await;
    let output = expose(&server).await;
    server.stop().await;
    assert_single_attachment_request(&server);
    assert_eq!(
        output.status.code(),
        Some(7),
        "server failure exit class; stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "failure must not render a success payload"
    );
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error envelope");
    assert_eq!(error["status"], 503);
    assert_eq!(error["retryable"], true);
    assert_eq!(error["code"], "unavailable");
}
