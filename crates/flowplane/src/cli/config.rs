use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) callback_url: Option<String>,
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
    pub(crate) callback_url: Option<String>,
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
    write_private_file(&path, toml::to_string_pretty(config)?)
        .with_context(|| format!("write {}", path.display()))
}

pub(crate) fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    ensure_private_parent_dir(path)?;
    write_private_file_contents(path, contents.as_ref())
}

fn ensure_private_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if parent.as_os_str().is_empty() {
            return Ok(());
        }
        let existed = parent.exists();
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        if !existed || parent.file_name().is_some_and(|name| name == ".flowplane") {
            set_private_dir_permissions(parent)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file_contents(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn write_private_file_contents(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
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
        callback_url: std::env::var("FLOWPLANE_OIDC_CALLBACK_URL")
            .ok()
            .or(file.callback_url),
    })
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::write_private_file;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "flowplane-cli-perms-{}-{suffix}",
            std::process::id()
        ))
    }

    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn private_file_write_creates_private_flowplane_dir_and_file() {
        let root = temp_root();
        let path = root.join(".flowplane").join("credentials");

        write_private_file(&path, "bearer-token").expect("write private file");

        assert_eq!(mode(path.parent().expect("parent")), 0o700);
        assert_eq!(mode(&path), 0o600);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn private_file_write_restricts_existing_flowplane_dir_and_file() {
        let root = temp_root();
        let parent = root.join(".flowplane");
        let path = parent.join("config.toml");
        fs::create_dir_all(&parent).expect("create parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755))
            .expect("set parent permissions");
        fs::write(&path, "token = \"old\"").expect("write existing file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("set file permissions");

        write_private_file(&path, "token = \"new\"").expect("rewrite private file");

        assert_eq!(mode(&parent), 0o700);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            fs::read_to_string(&path).expect("read file"),
            "token = \"new\""
        );

        fs::remove_dir_all(root).expect("cleanup");
    }
}
