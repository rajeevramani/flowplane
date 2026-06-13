use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";

#[derive(Debug, Clone, Args)]
pub struct GlobalOptions {
    #[arg(long, global = true)]
    pub context: Option<String>,
    #[arg(long, global = true, env = "FLOWPLANE_SERVER")]
    pub server: Option<String>,
    #[arg(long, global = true)]
    pub team: Option<String>,
    #[arg(long, global = true)]
    pub org: Option<String>,
    #[arg(short = 'o', long, global = true, value_enum)]
    pub output: Option<OutputFormat>,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    pub no_color: bool,
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub dry_run: bool,
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    #[arg(long, global = true)]
    pub revision: Option<i64>,
    #[arg(long, global = true, default_value_t = 30)]
    pub timeout: u64,
    #[arg(long, global = true)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
    Wide,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CliConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_context: Option<String>,
    #[serde(default)]
    pub(crate) contexts: Vec<NamedContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oidc_issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oidc_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oidc_scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NamedContext {
    pub(crate) name: String,
    pub(crate) server: String,
    pub(crate) org: Option<String>,
    pub(crate) team: Option<String>,
    pub(crate) token: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct EffectiveConfig {
    pub(crate) server: String,
    pub(crate) org: Option<String>,
    pub(crate) team: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) oidc_issuer: Option<String>,
    pub(crate) oidc_client_id: Option<String>,
    pub(crate) oidc_scope: Option<String>,
}

impl GlobalOptions {
    pub(crate) fn format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output.unwrap_or(OutputFormat::Table)
        }
    }
}

pub(crate) fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("FLOWPLANE_CONFIG") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".flowplane").join("config.toml")
}

pub(crate) fn credentials_path() -> PathBuf {
    config_path()
        .parent()
        .map(|p| p.join("credentials"))
        .unwrap_or_else(|| PathBuf::from(".flowplane/credentials"))
}

pub(crate) fn read_config() -> Result<CliConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(CliConfig::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn write_config(config: &CliConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, toml::to_string_pretty(config)?)
        .with_context(|| format!("write {}", path.display()))
}

pub(crate) fn effective(global: &GlobalOptions) -> Result<EffectiveConfig> {
    let file = read_config()?;
    let selected_name = global.context.as_ref().or(file.current_context.as_ref());
    let selected =
        selected_name.and_then(|name| file.contexts.iter().find(|ctx| &ctx.name == name));
    if let Some(name) = &global.context {
        if selected.is_none() {
            anyhow::bail!("context \"{name}\" does not exist");
        }
    }
    let token = std::env::var("FLOWPLANE_TOKEN")
        .ok()
        .or_else(|| selected.and_then(|ctx| ctx.token.clone()))
        .or_else(|| file.token.clone())
        .or_else(|| {
            fs::read_to_string(credentials_path())
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty());
    Ok(EffectiveConfig {
        server: global
            .server
            .clone()
            .or_else(|| selected.map(|ctx| ctx.server.clone()))
            .or_else(|| file.base_url.clone())
            .unwrap_or_else(|| DEFAULT_SERVER.to_string()),
        org: global
            .org
            .clone()
            .or_else(|| std::env::var("FLOWPLANE_ORG").ok())
            .or_else(|| selected.and_then(|ctx| ctx.org.clone()))
            .or_else(|| file.org.clone()),
        team: global
            .team
            .clone()
            .or_else(|| std::env::var("FLOWPLANE_TEAM").ok())
            .or_else(|| selected.and_then(|ctx| ctx.team.clone()))
            .or_else(|| file.team.clone()),
        token,
        oidc_issuer: std::env::var("FLOWPLANE_OIDC_ISSUER")
            .ok()
            .or(file.oidc_issuer),
        oidc_client_id: std::env::var("FLOWPLANE_OIDC_CLIENT_ID")
            .ok()
            .or(file.oidc_client_id),
        oidc_scope: std::env::var("FLOWPLANE_OIDC_SCOPE")
            .ok()
            .or(file.oidc_scope),
    })
}
