use crate::cli::config::{effective, EffectiveConfig, GlobalOptions, OutputFormat};
use crate::cli::output::{
    print_mutation_summary, render, render_error, render_revision_conflict, render_transport_error,
    resolve_kind,
};
use anyhow::{Context, Result};
use serde_json::json;
use serde_json::Value;
use std::fs;

pub(crate) struct RestClient {
    http: reqwest::Client,
    config: EffectiveConfig,
    global: GlobalOptions,
    /// When false, failed sub-requests do not print their own error envelope; the caller
    /// (e.g. `apply`) aggregates failures into a single envelope (CLI-R-30).
    report_errors: bool,
}

impl RestClient {
    pub(crate) fn new(global: GlobalOptions) -> Result<Self> {
        let config = effective(&global)?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout))
            .build()?;
        Ok(Self {
            http,
            config,
            global,
            report_errors: true,
        })
    }

    /// Build a client whose sub-request failures are returned silently (no per-call error
    /// envelope), for orchestrating commands that emit one aggregate error envelope.
    pub(crate) fn quiet_errors(mut self) -> Self {
        self.report_errors = false;
        self
    }

    /// Test-only constructor bypassing context resolution (no config/credential files).
    #[cfg(test)]
    pub(crate) fn for_tests(config: EffectiveConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
            global: GlobalOptions {
                context: None,
                server: None,
                team: None,
                org: None,
                token: None,
                output: None,
                json: false,
                no_color: false,
                quiet: false,
                verbose: false,
                dry_run: false,
                yes: false,
                revision: None,
                fields: Vec::new(),
                timeout: None,
                out: None,
            },
            report_errors: true,
        }
    }

    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        self.request_inner(method, path, body, self.global.revision, true, false)
            .await
    }

    pub(crate) async fn request_and_render(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        self.request_inner(method, path, body, self.global.revision, true, true)
            .await
    }

    pub(crate) async fn request_with_revision(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        revision: Option<i64>,
    ) -> Result<Option<Value>> {
        self.request_inner(method, path, body, revision, true, false)
            .await
    }

    /// Perform a mutation without emitting any per-call output (no render, no summary).
    ///
    /// Used by orchestrating commands (e.g. `apply`) that aggregate several sub-requests
    /// and render a single summary envelope themselves — emitting a per-resource envelope
    /// here would break the one-document-per-invocation JSON contract (CLI-R-15).
    pub(crate) async fn request_quiet(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        revision: Option<i64>,
    ) -> Result<Option<Value>> {
        self.request_inner(method, path, body, revision, false, false)
            .await
    }

    pub(crate) async fn get_optional(&self, path: &str) -> Result<Option<Value>> {
        let url = self.url(path);
        let mut req = self.http.request(reqwest::Method::GET, url);
        req = self.add_auth_headers(req, None);
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => return Err(self.transport_failure(&err)),
        };
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            return Err(self.http_failure(status, request_id, &text));
        }
        if text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .context("parse response JSON")
            .map(Some)
    }

    /// Turn a non-success HTTP response into an error: a rendered envelope normally, or a
    /// silent message-carrying error when this client suppresses per-call errors (apply).
    fn http_failure(
        &self,
        status: reqwest::StatusCode,
        request_id: Option<String>,
        text: &str,
    ) -> anyhow::Error {
        // Make 401s self-diagnosing (design AC 12 + AC 14). An auto-discovered dev token
        // the server rejects is almost always stale (CP restarted before rewriting the
        // file, or the file belongs to another local instance). Conversely, a stale
        // PERSISTENT stored token (context/config/credentials — typically a dev token
        // stored via `auth login` before a CP restart) that outranks a live dev-token
        // file gets the shadowed-credential hint; the flag/env tier is a deliberate
        // per-invocation choice and never triggers it.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            use crate::cli::config::TokenSource;
            match self.config.token_source {
                Some(TokenSource::DevFile) => eprintln!(
                    "hint: the dev token from ~/.flowplane/dev-token may be stale — restart \
                     the dev control plane or remove the file"
                ),
                Some(source)
                    if source.is_persistent_store() && self.config.dev_fallback_available =>
                {
                    eprintln!(
                        "hint: a stored credential may be shadowing the local dev token — \
                         remove it (e.g. `rm ~/.flowplane/credentials`, or the token in \
                         your config/context) to use auto-discovery from \
                         ~/.flowplane/dev-token"
                    )
                }
                _ => {}
            }
        }
        if self.report_errors {
            render_error(&self.global, status, request_id, text)
        } else {
            anyhow::anyhow!("{}", crate::cli::output::error_message(status, text))
        }
    }

    /// Read a resource's current `revision` for read-modify-write (CLI-R-47). A quiet GET:
    /// it neither renders nor surfaces errors (a missing/failed read just yields `None`, and
    /// the mutation proceeds without `If-Match`, letting the server report the real failure).
    async fn read_current_revision(&self, path: &str) -> Option<i64> {
        let url = self.url(path);
        let req = self.add_auth_headers(self.http.request(reqwest::Method::GET, url), None);
        let response = req.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let text = response.text().await.ok()?;
        serde_json::from_str::<Value>(&text)
            .ok()?
            .get("revision")
            .and_then(Value::as_i64)
    }

    /// Transport failure as an error: rendered envelope normally, silent message otherwise.
    fn transport_failure(&self, err: &reqwest::Error) -> anyhow::Error {
        if self.report_errors {
            render_transport_error(&self.global, err)
        } else {
            anyhow::anyhow!("{err}")
        }
    }

    pub(crate) async fn request_text(&self, method: reqwest::Method, path: &str) -> Result<String> {
        if self.global.dry_run && method != reqwest::Method::GET {
            let plan = json!({ "method": method.as_str(), "path": path });
            render(&self.global, "plan", &plan)?;
            return Ok(String::new());
        }
        let url = self.url(path);
        let mut req = self.http.request(method, url);
        req = self.add_auth_headers(req, self.global.revision);
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => return Err(self.transport_failure(&err)),
        };
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            return Err(self.http_failure(status, request_id, &text));
        }
        if let Some(out) = &self.global.out {
            fs::write(out, &text).with_context(|| format!("write {}", out.display()))?;
        } else {
            print!("{text}");
        }
        Ok(text)
    }

    async fn request_inner(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        revision: Option<i64>,
        render_response: bool,
        always_render_success: bool,
    ) -> Result<Option<Value>> {
        if self.global.dry_run && method != reqwest::Method::GET {
            let plan = json!({ "method": method.as_str(), "path": path, "body": body });
            render(&self.global, "plan", &plan)?;
            return Ok(Some(plan));
        }
        // CLI-R-22/26: a destructive DELETE prompts `[y/N]` on a TTY, is skipped by `--yes`,
        // and fails fast on a non-interactive terminal — before any network call (incl. the
        // RMW read below). Only for interactive command clients (apply uses a quiet client).
        if method == reqwest::Method::DELETE && self.report_errors {
            crate::cli::confirm::confirm_destructive(
                &self.global,
                &format!("delete {}", path.trim_start_matches('/')),
            )?;
        }
        let method_label = method.as_str().to_string();
        let is_get = method == reqwest::Method::GET;
        // CLI-R-47: read-modify-write. On an update/delete with no explicit `--revision`, read
        // the resource's current revision first and send it as `If-Match`, so concurrent edits
        // are detected (a stale write loses with a 409) instead of silently last-write-wins.
        let is_mutation = matches!(
            method,
            reqwest::Method::PATCH | reqwest::Method::DELETE | reqwest::Method::PUT
        );
        let revision = match revision {
            Some(explicit) => Some(explicit),
            None if is_mutation => self.read_current_revision(path).await,
            None => None,
        };
        let url = self.url(path);
        let mut req = self.http.request(method, url);
        req = self.add_auth_headers(req, revision);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => return Err(self.transport_failure(&err)),
        };
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            // CLI-R-47: a revision race surfaces a 409/412 that names BOTH revisions — the one
            // we attempted (If-Match) and the server's current one (in its message).
            if self.report_errors
                && revision.is_some()
                && matches!(
                    status,
                    reqwest::StatusCode::CONFLICT | reqwest::StatusCode::PRECONDITION_FAILED
                )
            {
                return Err(render_revision_conflict(
                    &self.global,
                    status,
                    request_id,
                    &text,
                    revision.unwrap_or_default(),
                ));
            }
            return Err(self.http_failure(status, request_id, &text));
        }
        if status == reqwest::StatusCode::NO_CONTENT || text.trim().is_empty() {
            if render_response && !self.global.quiet {
                // CLI-R-10: a body-less mutation (e.g. delete) still emits a JSON/YAML
                // envelope under `-o json`/`-o yaml`; only human formats print prose.
                if matches!(
                    self.global.format(),
                    OutputFormat::Json | OutputFormat::Yaml
                ) {
                    let result = json!({
                        "result": "ok",
                        "method": method_label,
                        "path": path,
                    });
                    render(&self.global, "mutationResult", &result)?;
                } else {
                    print_mutation_summary(&self.global, &method_label, path, None)?;
                }
            }
            return Ok(None);
        }
        let value: Value = serde_json::from_str(&text).context("parse response JSON")?;
        if render_response
            && (always_render_success
                || is_get
                || matches!(
                    self.global.format(),
                    OutputFormat::Json | OutputFormat::Yaml
                ))
        {
            render(&self.global, &resolve_kind(path, &value), &value)?;
        } else if render_response {
            print_mutation_summary(&self.global, &method_label, path, Some(&value))?;
        }
        Ok(Some(value))
    }

    pub(crate) fn team(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.config.team.clone())
            .ok_or_else(|| anyhow::anyhow!("team is required; pass --team or configure a context"))
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.config.server.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn add_auth_headers(
        &self,
        mut req: reqwest::RequestBuilder,
        revision: Option<i64>,
    ) -> reqwest::RequestBuilder {
        if let Some(token) = &self.config.token {
            req = req.bearer_auth(token);
        }
        if let Some(org) = &self.config.org {
            req = req.header("X-Flowplane-Org", org);
        }
        if let Some(revision) = revision {
            req = req.header(reqwest::header::IF_MATCH, revision.to_string());
        }
        req
    }

    /// Status-preserving GET (fpv2-03m.1): unlike `request`, a non-success response is
    /// returned as a typed [`ReadError::Status`] carrying the HTTP status, so callers
    /// (the dashboard) can branch on 401 vs 403 vs other. Renders nothing, prints no
    /// hints, and never emits an error envelope — presentation is the caller's job.
    #[allow(dead_code)] // first consumer is the dashboard data path (fpv2-03m.3)
    pub(crate) async fn get_json(&self, path: &str) -> std::result::Result<Value, ReadError> {
        let url = self.url(path);
        let req = self.add_auth_headers(self.http.request(reqwest::Method::GET, url), None);
        let response = req.send().await.map_err(ReadError::Transport)?;
        let status = response.status();
        // A body-read failure is a transport-class error even when the status line was
        // 2xx — never a `Status`, which is reserved for real non-success statuses.
        let text = response.text().await.map_err(ReadError::Transport)?;
        if !status.is_success() {
            return Err(ReadError::Status { status, body: text });
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(ReadError::Decode)
    }
}

/// Error type for the status-preserving read seam. Existing CLI call sites keep the
/// rendered `anyhow` path (`http_failure`); this exists so a caller can distinguish
/// upstream statuses without parsing rendered output.
#[derive(Debug)]
#[allow(dead_code)] // first consumer is the dashboard data path (fpv2-03m.3)
pub(crate) enum ReadError {
    /// The server answered with a non-success HTTP status.
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    /// The exchange failed below HTTP semantics: connect/timeout/TLS, or the response
    /// body could not be read (regardless of status line).
    Transport(reqwest::Error),
    /// A success response whose body was not valid JSON.
    Decode(serde_json::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Status { status, .. } => write!(f, "upstream returned {status}"),
            ReadError::Transport(err) => write!(f, "transport error: {err}"),
            ReadError::Decode(err) => write!(f, "invalid JSON in response: {err}"),
        }
    }
}

impl std::error::Error for ReadError {}

#[cfg(test)]
mod read_seam_tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::cli::config::EffectiveConfig;
    use axum::routing::get;
    use axum::Router;

    fn client_for(server: String) -> RestClient {
        RestClient::for_tests(EffectiveConfig {
            server,
            org: Some("test-org".into()),
            team: Some("test-team".into()),
            token: Some("test-bearer-token".into()),
            token_source: None,
            dev_fallback_available: false,
            timeout: 5,
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_scope: None,
            callback_url: None,
        })
    }

    /// Loopback stub on an ephemeral port (parallel-safe: port 0, no shared state).
    async fn spawn_stub(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub to an ephemeral port");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        base
    }

    #[tokio::test]
    async fn success_with_body_parses_json() {
        let base = spawn_stub(Router::new().route("/ok", get(|| async { r#"{"a":1}"# }))).await;
        let value = client_for(base).get_json("/ok").await.expect("ok");
        assert_eq!(value, serde_json::json!({"a": 1}));
    }

    #[tokio::test]
    async fn success_with_empty_body_is_null() {
        let base = spawn_stub(Router::new().route("/empty", get(|| async { "" }))).await;
        let value = client_for(base).get_json("/empty").await.expect("ok");
        assert_eq!(value, Value::Null);
    }

    #[tokio::test]
    async fn unauthorized_preserves_401_and_body() {
        let base = spawn_stub(Router::new().route(
            "/secure",
            get(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    r#"{"error":"unauthorized"}"#,
                )
            }),
        ))
        .await;
        match client_for(base).get_json("/secure").await {
            Err(ReadError::Status { status, body }) => {
                assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
                assert!(body.contains("unauthorized"), "body preserved: {body}");
            }
            other => panic!("expected Status(401), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn forbidden_preserves_403() {
        let base = spawn_stub(Router::new().route(
            "/denied",
            get(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    r#"{"error":"forbidden"}"#,
                )
            }),
        ))
        .await;
        match client_for(base).get_json("/denied").await {
            Err(ReadError::Status { status, .. }) => {
                assert_eq!(status, reqwest::StatusCode::FORBIDDEN)
            }
            other => panic!("expected Status(403), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn server_error_preserves_500() {
        let base = spawn_stub(Router::new().route(
            "/boom",
            get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "oops") }),
        ))
        .await;
        match client_for(base).get_json("/boom").await {
            Err(ReadError::Status { status, body }) => {
                assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(body, "oops");
            }
            other => panic!("expected Status(500), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_json_success_body_is_decode_error() {
        let base = spawn_stub(Router::new().route("/text", get(|| async { "not json" }))).await;
        match client_for(base).get_json("/text").await {
            Err(ReadError::Decode(_)) => {}
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unreachable_server_is_transport_error() {
        // A listener that accepts and immediately drops every connection: the client
        // deterministically sees a closed connection before any HTTP response, with no
        // window for another process to claim the port (parallel-safe).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        match client_for(base).get_json("/any").await {
            Err(ReadError::Transport(_)) => {}
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_json_sends_bearer_and_org_headers() {
        let base = spawn_stub(Router::new().route(
            "/echo",
            get(|headers: axum::http::HeaderMap| async move {
                let auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let org = headers
                    .get("X-Flowplane-Org")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                axum::Json(serde_json::json!({"auth": auth, "org": org}))
            }),
        ))
        .await;
        let value = client_for(base).get_json("/echo").await.expect("ok");
        assert_eq!(value["auth"], "Bearer test-bearer-token");
        assert_eq!(value["org"], "test-org");
    }
}
