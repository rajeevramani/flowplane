//! Black-box OAuth token-selection contract tests for fpv2-d23.13.
//!
//! The built CLI is exercised against an ephemeral standards-shaped OIDC provider. Assertions use
//! only process output and the credential file reported by the CLI; production sources are not
//! imported or inspected.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener as StdTcpListener;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const ACCESS_SECRET: &str = "ACCESS_SECRET_fpv2_d23_13";
const ID_SECRET: &str = "ID_SECRET_fpv2_d23_13";

#[derive(Clone, Copy)]
enum TokenReply {
    Both,
    IdOnly,
}

#[derive(Clone)]
struct ProviderState {
    issuer: String,
    reply: TokenReply,
}

struct Provider {
    issuer: String,
    handle: JoinHandle<()>,
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn discovery(State(state): State<ProviderState>) -> Json<Value> {
    Json(json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}/authorize", state.issuer),
        "token_endpoint": format!("{}/token", state.issuer),
        "device_authorization_endpoint": format!("{}/device_authorization", state.issuer),
        "response_types_supported": ["code"],
        "grant_types_supported": [
            "authorization_code",
            "urn:ietf:params:oauth:grant-type:device_code"
        ],
        "code_challenge_methods_supported": ["S256"]
    }))
}

async fn authorize(Query(query): Query<HashMap<String, String>>) -> Response {
    let redirect_uri = query.get("redirect_uri").expect("redirect_uri");
    let state = query.get("state").expect("state");
    Redirect::temporary(&format!("{redirect_uri}?code=test-code&state={state}")).into_response()
}

async fn device_authorization() -> Json<Value> {
    Json(json!({
        "device_code": "test-device-code",
        "user_code": "FPV2-D23",
        "verification_uri": "https://example.invalid/activate",
        "verification_uri_complete": "https://example.invalid/activate?user_code=FPV2-D23",
        "expires_in": 60,
        "interval": 0
    }))
}

async fn token(State(state): State<ProviderState>) -> (StatusCode, Json<Value>) {
    let body = match state.reply {
        TokenReply::Both => json!({
            "access_token": ACCESS_SECRET,
            "id_token": ID_SECRET,
            "token_type": "Bearer",
            "expires_in": 3600
        }),
        TokenReply::IdOnly => json!({
            "id_token": ID_SECRET,
            "token_type": "Bearer",
            "expires_in": 3600
        }),
    };
    (StatusCode::OK, Json(body))
}

async fn start_provider(reply: TokenReply) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind OIDC fake");
    let addr = listener.local_addr().expect("OIDC fake address");
    let issuer = format!("http://{addr}");
    let state = ProviderState {
        issuer: issuer.clone(),
        reply,
    };
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/authorize", get(authorize))
        .route("/device_authorization", post(device_authorization))
        .route("/token", post(token))
        .with_state(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Provider { issuer, handle }
}

fn login_command(home: &Path, provider: &Provider) -> Command {
    let mut cmd = common::flowplane_cmd(home);
    cmd.arg("auth")
        .arg("login")
        .arg("--issuer")
        .arg(&provider.issuer)
        .arg("--client-id")
        .arg("flowplane-integration-test")
        .arg("--scope")
        .arg("openid");
    cmd
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn saved_credential(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("token saved to "))
        .expect("successful login must report credential path");
    std::fs::read_to_string(path)
        .expect("read credential file")
        .trim()
        .to_owned()
}

fn assert_id_only_failure_is_secret_safe(output: &Output, flow: &str) {
    assert!(
        !output.status.success(),
        "{flow}: ID-token-only response must fail"
    );
    let visible = combined_output(output);
    assert!(
        visible.to_ascii_lowercase().contains("access_token"),
        "{flow}: error must explicitly identify the missing access_token; output: {visible:?}"
    );
    assert!(
        !visible.contains(ID_SECRET),
        "{flow}: ID token leaked: {visible:?}"
    );
    assert!(
        !visible.contains(ACCESS_SECRET),
        "{flow}: access token leaked: {visible:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_both_tokens_persists_access_token() {
    let provider = start_provider(TokenReply::Both).await;
    let home = common::unique_tempdir();
    let output = login_command(&home, &provider)
        .arg("--device")
        .output()
        .expect("run device login");
    assert!(
        output.status.success(),
        "device login failed: {:?}",
        combined_output(&output)
    );
    assert_eq!(saved_credential(&output), ACCESS_SECRET);
    let visible = combined_output(&output);
    assert!(!visible.contains(ID_SECRET), "ID token must not be printed");
    assert!(
        !visible.contains(ACCESS_SECRET),
        "access token must not be printed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_id_token_only_fails_without_secret_leak() {
    let provider = start_provider(TokenReply::IdOnly).await;
    let home = common::unique_tempdir();
    let output = login_command(&home, &provider)
        .arg("--device")
        .output()
        .expect("run device login");
    assert_id_only_failure_is_secret_safe(&output, "device");
}

fn reserve_callback_url() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("reserve callback port");
    let port = listener.local_addr().expect("callback address").port();
    drop(listener);
    format!("http://127.0.0.1:{port}/callback")
}

async fn finish_pkce(child: &mut Child, lines: mpsc::Receiver<String>) {
    let url = tokio::task::spawn_blocking(move || {
        lines
            .recv_timeout(Duration::from_secs(10))
            .expect("PKCE CLI did not print authorization URL")
    })
    .await
    .expect("join URL reader");
    let start = url.find("http://").expect("authorization URL in output");
    let url = url[start..].trim();
    reqwest::get(url).await.expect("visit authorization URL");
    let _ = child;
}

async fn run_pkce(home: &Path, provider: &Provider) -> Output {
    let callback = reserve_callback_url();
    let mut child = login_command(home, provider)
        .arg("--pkce")
        .arg("--callback-url")
        .arg(callback)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn PKCE login");
    let stdout = child.stdout.take().expect("PKCE stdout");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.expect("read PKCE stdout");
            if line.contains("http://") {
                let _ = tx.send(line.clone());
            }
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
        }
        bytes
    });
    finish_pkce(&mut child, rx).await;
    let status = child.wait().expect("wait for PKCE login");
    let stdout = reader.join().expect("join stdout reader");
    let mut stderr = Vec::new();
    if let Some(err) = child.stderr.take() {
        use std::io::Read;
        BufReader::new(err)
            .read_to_end(&mut stderr)
            .expect("read PKCE stderr");
    }
    Output {
        status,
        stdout,
        stderr,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pkce_both_tokens_persists_access_token() {
    let provider = start_provider(TokenReply::Both).await;
    let home = common::unique_tempdir();
    let output = run_pkce(&home, &provider).await;
    assert!(
        output.status.success(),
        "PKCE login failed: {:?}",
        combined_output(&output)
    );
    assert_eq!(saved_credential(&output), ACCESS_SECRET);
    let visible = combined_output(&output);
    assert!(!visible.contains(ID_SECRET), "ID token must not be printed");
    assert!(
        !visible.contains(ACCESS_SECRET),
        "access token must not be printed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pkce_id_token_only_fails_without_secret_leak() {
    let provider = start_provider(TokenReply::IdOnly).await;
    let home = common::unique_tempdir();
    let output = run_pkce(&home, &provider).await;
    assert_id_only_failure_is_secret_safe(&output, "PKCE");
}
