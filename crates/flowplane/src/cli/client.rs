use crate::cli::config::{effective, EffectiveConfig, GlobalOptions, OutputFormat};
use crate::cli::output::{
    derive_kind, print_mutation_summary, render, render_error, render_revision_conflict,
    render_transport_error,
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
            render(&self.global, &derive_kind(path, &value), &value)?;
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
}
