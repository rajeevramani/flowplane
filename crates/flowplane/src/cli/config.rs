use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Args)]
pub struct GlobalOptions {
    #[arg(long, global = true)]
    pub context: Option<String>,
    #[arg(long, global = true, env = "FLOWPLANE_SERVER")]
    pub server: Option<String>,
    #[arg(long, global = true, env = "FLOWPLANE_TEAM")]
    pub team: Option<String>,
    #[arg(long, global = true, env = "FLOWPLANE_ORG")]
    pub org: Option<String>,
    /// Bearer token; highest-priority token source (CLI-R-40). Falls back to
    /// `FLOWPLANE_TOKEN`, the selected context, the config file, then the credentials file.
    #[arg(long, global = true, env = "FLOWPLANE_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    #[arg(short = 'o', long, global = true, value_enum)]
    pub output: Option<OutputFormat>,
    /// Exactly equivalent to `--output json`; cannot be combined with `--output`
    /// (CLI-R-11: `-o/--output` is the single format selector).
    #[arg(long, global = true, conflicts_with = "output")]
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
    /// Comma-separated field names to project reader output to (CLI-R-51). Applied inside
    /// the envelope `data` (per item for lists); `schemaVersion`/`kind` always survive.
    #[arg(long, global = true, value_delimiter = ',')]
    pub fields: Vec<String>,
    /// HTTP timeout in seconds. Uniform precedence (CLI-R-40/41):
    /// flag > `FLOWPLANE_TIMEOUT` > context > config file > default (30).
    #[arg(long, global = true, env = "FLOWPLANE_TIMEOUT")]
    pub timeout: Option<u64>,
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
    pub(crate) timeout: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct EffectiveConfig {
    pub(crate) server: String,
    pub(crate) org: Option<String>,
    pub(crate) team: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) timeout: u64,
    pub(crate) oidc_issuer: Option<String>,
    pub(crate) oidc_client_id: Option<String>,
    pub(crate) oidc_scope: Option<String>,
    pub(crate) callback_url: Option<String>,
}

impl GlobalOptions {
    /// Resolve the effective output format.
    ///
    /// Precedence (CLI-R-11/12): `--json` is exactly `-o json`; an explicit `-o/--output`
    /// always wins; otherwise the default is `table` on an interactive stdout and `json`
    /// when stdout is not a TTY (so `flowplane … | jq` works without `-o json`).
    pub(crate) fn format(&self) -> OutputFormat {
        self.format_for(std::io::stdout().is_terminal())
    }

    /// TTY-parameterized core of [`GlobalOptions::format`], split out for deterministic tests.
    pub(crate) fn format_for(&self, stdout_is_tty: bool) -> OutputFormat {
        if self.json {
            return OutputFormat::Json;
        }
        if let Some(output) = self.output {
            return output;
        }
        if stdout_is_tty {
            OutputFormat::Table
        } else {
            OutputFormat::Json
        }
    }

    /// Whether styled (ANSI) output is permitted (CLI-R-16).
    ///
    /// Disabled by `--no-color`, the `NO_COLOR` env var (any value), a non-TTY stdout, or
    /// when output is redirected to a file via `--out`.
    pub(crate) fn use_color(&self) -> bool {
        self.use_color_for(std::io::stdout().is_terminal())
    }

    /// TTY-parameterized core of [`GlobalOptions::use_color`], split out for deterministic tests.
    pub(crate) fn use_color_for(&self, stdout_is_tty: bool) -> bool {
        !self.no_color
            && self.out.is_none()
            && stdout_is_tty
            && std::env::var_os("NO_COLOR").is_none()
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
    let credentials = fs::read_to_string(credentials_path())
        .ok()
        .map(|s| s.trim().to_string());
    resolve(global, file, credentials)
}

/// Pure precedence resolver (CLI-R-40): `flag > env > context > file > default` for every
/// value. The flag-or-env tier is already folded into `global.*` by clap (each arg's
/// `env = …`); this layers context → file → credentials/default beneath it. IO-free so the
/// precedence is unit-testable without touching process env or the filesystem.
fn resolve(
    global: &GlobalOptions,
    file: CliConfig,
    credentials: Option<String>,
) -> Result<EffectiveConfig> {
    let selected_name = global.context.as_ref().or(file.current_context.as_ref());
    let selected =
        selected_name.and_then(|name| file.contexts.iter().find(|ctx| &ctx.name == name));
    if let Some(name) = &global.context {
        if selected.is_none() {
            anyhow::bail!("context \"{name}\" does not exist");
        }
    }
    let token = global
        .token
        .clone()
        .or_else(|| selected.and_then(|ctx| ctx.token.clone()))
        .or_else(|| file.token.clone())
        .or(credentials)
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
            .or_else(|| selected.and_then(|ctx| ctx.org.clone()))
            .or_else(|| file.org.clone()),
        team: global
            .team
            .clone()
            .or_else(|| selected.and_then(|ctx| ctx.team.clone()))
            .or_else(|| file.team.clone()),
        token,
        timeout: global
            .timeout
            .or_else(|| selected.and_then(|ctx| ctx.timeout))
            .or(file.timeout)
            .unwrap_or(DEFAULT_TIMEOUT_SECS),
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod resolve_tests {
    use super::{resolve, CliConfig, GlobalOptions, NamedContext, DEFAULT_SERVER};

    fn opts() -> GlobalOptions {
        GlobalOptions {
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
        }
    }

    fn ctx(name: &str) -> NamedContext {
        NamedContext {
            name: name.to_string(),
            server: "https://ctx.example".to_string(),
            org: Some("ctx-org".to_string()),
            team: Some("ctx-team".to_string()),
            token: Some("ctx-token".to_string()),
            timeout: Some(11),
        }
    }

    fn file_with_ctx() -> CliConfig {
        CliConfig {
            current_context: Some("prod".to_string()),
            contexts: vec![ctx("prod")],
            base_url: Some("https://file.example".to_string()),
            org: Some("file-org".to_string()),
            team: Some("file-team".to_string()),
            token: Some("file-token".to_string()),
            ..CliConfig::default()
        }
    }

    #[test]
    fn flag_or_env_tier_beats_context_file_and_credentials() {
        // `global.*` carries the resolved flag-or-env value (clap folds env in); it wins.
        let mut o = opts();
        o.server = Some("https://flag.example".to_string());
        o.org = Some("flag-org".to_string());
        o.team = Some("flag-team".to_string());
        o.token = Some("flag-token".to_string());
        let eff = resolve(&o, file_with_ctx(), Some("cred-token".to_string())).unwrap();
        assert_eq!(eff.server, "https://flag.example");
        assert_eq!(eff.org.as_deref(), Some("flag-org"));
        assert_eq!(eff.team.as_deref(), Some("flag-team"));
        assert_eq!(eff.token.as_deref(), Some("flag-token"));
    }

    #[test]
    fn context_beats_file_when_no_flag_or_env() {
        let eff = resolve(&opts(), file_with_ctx(), Some("cred-token".to_string())).unwrap();
        // current_context = prod, so the context values win over the bare file values.
        assert_eq!(eff.server, "https://ctx.example");
        assert_eq!(eff.org.as_deref(), Some("ctx-org"));
        assert_eq!(eff.team.as_deref(), Some("ctx-team"));
        assert_eq!(eff.token.as_deref(), Some("ctx-token"));
    }

    #[test]
    fn file_beats_credentials_and_default_when_no_context() {
        let file = CliConfig {
            base_url: Some("https://file.example".to_string()),
            org: Some("file-org".to_string()),
            team: Some("file-team".to_string()),
            token: Some("file-token".to_string()),
            ..CliConfig::default()
        };
        let eff = resolve(&opts(), file, Some("cred-token".to_string())).unwrap();
        assert_eq!(eff.server, "https://file.example");
        assert_eq!(eff.token.as_deref(), Some("file-token"));
    }

    #[test]
    fn credentials_then_default_are_the_lowest_token_and_server_tiers() {
        let eff = resolve(
            &opts(),
            CliConfig::default(),
            Some("cred-token".to_string()),
        )
        .unwrap();
        assert_eq!(eff.token.as_deref(), Some("cred-token"));
        // No server anywhere → the built-in default.
        assert_eq!(eff.server, DEFAULT_SERVER);
        // No token anywhere → None.
        let eff = resolve(&opts(), CliConfig::default(), None).unwrap();
        assert_eq!(eff.token, None);
    }

    #[test]
    fn timeout_follows_uniform_precedence_flag_context_file_default() {
        // flag (folded from --timeout/FLOWPLANE_TIMEOUT) wins.
        let mut o = opts();
        o.timeout = Some(99);
        assert_eq!(resolve(&o, file_with_ctx(), None).unwrap().timeout, 99);
        // no flag/env → selected context timeout (ctx sets 11).
        assert_eq!(resolve(&opts(), file_with_ctx(), None).unwrap().timeout, 11);
        // no flag/env/context → file timeout.
        let file = CliConfig {
            timeout: Some(7),
            ..CliConfig::default()
        };
        assert_eq!(resolve(&opts(), file, None).unwrap().timeout, 7);
        // nothing anywhere → default 30.
        assert_eq!(
            resolve(&opts(), CliConfig::default(), None)
                .unwrap()
                .timeout,
            30
        );
    }

    #[test]
    fn unknown_explicit_context_is_an_error() {
        let mut o = opts();
        o.context = Some("does-not-exist".to_string());
        assert!(resolve(&o, CliConfig::default(), None).is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod format_tests {
    use super::{GlobalOptions, OutputFormat};

    fn opts() -> GlobalOptions {
        GlobalOptions {
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
        }
    }

    #[test]
    fn json_flag_forces_json_regardless_of_tty() {
        let mut o = opts();
        o.json = true;
        assert_eq!(o.format_for(true), OutputFormat::Json);
        assert_eq!(o.format_for(false), OutputFormat::Json);
    }

    #[test]
    fn explicit_output_always_wins() {
        let mut o = opts();
        o.output = Some(OutputFormat::Yaml);
        assert_eq!(o.format_for(true), OutputFormat::Yaml);
        assert_eq!(o.format_for(false), OutputFormat::Yaml);
    }

    #[test]
    fn default_is_table_on_tty_json_off_tty() {
        let o = opts();
        assert_eq!(o.format_for(true), OutputFormat::Table);
        assert_eq!(o.format_for(false), OutputFormat::Json);
    }

    #[test]
    fn color_disabled_by_flag_file_or_non_tty() {
        let mut o = opts();
        o.no_color = true;
        assert!(!o.use_color_for(true), "--no-color disables color");

        let mut o = opts();
        o.out = Some(std::path::PathBuf::from("/tmp/x"));
        assert!(!o.use_color_for(true), "--out disables color");

        let o = opts();
        assert!(!o.use_color_for(false), "non-TTY disables color");
    }
}

#[cfg(test)]
#[cfg(unix)]
#[allow(clippy::expect_used)]
mod tests {
    use super::write_private_file;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let seq = TEMP_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "flowplane-cli-perms-{}-{suffix}-{seq}",
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
