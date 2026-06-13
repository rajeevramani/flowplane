use crate::cli::config::{effective, EffectiveConfig, GlobalOptions, OutputFormat};
use crate::cli::output::{print_mutation_summary, render, render_error};
use anyhow::{Context, Result};
use serde_json::json;
use serde_json::Value;
use std::fs;

pub(crate) struct RestClient {
    http: reqwest::Client,
    config: EffectiveConfig,
    global: GlobalOptions,
}

impl RestClient {
    pub(crate) fn new(global: GlobalOptions) -> Result<Self> {
        let config = effective(&global)?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(global.timeout))
            .build()?;
        Ok(Self {
            http,
            config,
            global,
        })
    }

    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        self.request_inner(method, path, body, self.global.revision, true)
            .await
    }

    pub(crate) async fn request_with_revision(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        revision: Option<i64>,
    ) -> Result<Option<Value>> {
        self.request_inner(method, path, body, revision, true).await
    }

    pub(crate) async fn get_optional(&self, path: &str) -> Result<Option<Value>> {
        let url = self.url(path);
        let mut req = self.http.request(reqwest::Method::GET, url);
        req = self.add_auth_headers(req, None);
        let response = req.send().await.context("send request")?;
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
            return Err(render_error(status, request_id, &text));
        }
        if text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .context("parse response JSON")
            .map(Some)
    }

    pub(crate) async fn request_text(&self, method: reqwest::Method, path: &str) -> Result<String> {
        if self.global.dry_run && method != reqwest::Method::GET {
            let plan = json!({ "method": method.as_str(), "path": path });
            render(&self.global, &plan)?;
            return Ok(String::new());
        }
        let url = self.url(path);
        let mut req = self.http.request(method, url);
        req = self.add_auth_headers(req, self.global.revision);
        let response = req.send().await.context("send request")?;
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            return Err(render_error(status, request_id, &text));
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
    ) -> Result<Option<Value>> {
        if self.global.dry_run && method != reqwest::Method::GET {
            let plan = json!({ "method": method.as_str(), "path": path, "body": body });
            render(&self.global, &plan)?;
            return Ok(Some(plan));
        }
        let url = self.url(path);
        let method_label = method.as_str().to_string();
        let is_get = method == reqwest::Method::GET;
        let mut req = self.http.request(method, url);
        req = self.add_auth_headers(req, revision);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await.context("send request")?;
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            return Err(render_error(status, request_id, &text));
        }
        if status == reqwest::StatusCode::NO_CONTENT || text.trim().is_empty() {
            if !self.global.quiet {
                print_mutation_summary(&self.global, &method_label, path, None)?;
            }
            return Ok(None);
        }
        let value: Value = serde_json::from_str(&text).context("parse response JSON")?;
        if render_response
            && (is_get
                || matches!(
                    self.global.format(),
                    OutputFormat::Json | OutputFormat::Yaml
                ))
        {
            render(&self.global, &value)?;
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
