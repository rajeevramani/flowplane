use crate::cli::config::{effective, EffectiveConfig, GlobalOptions, OutputFormat};
use crate::cli::output::{print_mutation_summary, render, render_error};
use anyhow::{Context, Result};
use serde_json::json;
use serde_json::Value;

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
        if self.global.dry_run && method != reqwest::Method::GET {
            let plan = json!({ "method": method.as_str(), "path": path, "body": body });
            render(&self.global, &plan)?;
            return Ok(Some(plan));
        }
        let url = format!(
            "{}/{}",
            self.config.server.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let method_label = method.as_str().to_string();
        let is_get = method == reqwest::Method::GET;
        let mut req = self.http.request(method, url);
        if let Some(token) = &self.config.token {
            req = req.bearer_auth(token);
        }
        if let Some(org) = &self.config.org {
            req = req.header("X-Flowplane-Org", org);
        }
        if let Some(revision) = self.global.revision {
            req = req.header(reqwest::header::IF_MATCH, revision.to_string());
        }
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
        if is_get
            || matches!(
                self.global.format(),
                OutputFormat::Json | OutputFormat::Yaml
            )
        {
            render(&self.global, &value)?;
        } else {
            print_mutation_summary(&self.global, &method_label, path, Some(&value))?;
        }
        Ok(Some(value))
    }

    pub(crate) fn team(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.config.team.clone())
            .ok_or_else(|| anyhow::anyhow!("team is required; pass --team or configure a context"))
    }
}
