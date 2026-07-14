use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

// The private-write helper (0600 file / 0700 parent) moved to the crate-level `paths`
// module so the server's token sinks share it (fpv2-wvp.1); re-exported to keep this
// module's callers unchanged.
pub(crate) use crate::paths::write_private_file;

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Args)]
pub struct GlobalOptions {
    /// Named context from the config file to use for this invocation.
    #[arg(long, global = true)]
    pub context: Option<String>,
    /// Control-plane server base URL (overrides the context and config file).
    #[arg(long, global = true, env = "FLOWPLANE_SERVER")]
    pub server: Option<String>,
    /// Team scope; defaults to the active context's team.
    #[arg(long, global = true, env = "FLOWPLANE_TEAM")]
    pub team: Option<String>,
    /// Organization scope; defaults to the active context's org.
    #[arg(long, global = true, env = "FLOWPLANE_ORG")]
    pub org: Option<String>,
    /// Bearer token; highest-priority token source (CLI-R-40). Falls back to
    /// `FLOWPLANE_TOKEN`, the selected context, the config file, then the credentials file.
    #[arg(long, global = true, env = "FLOWPLANE_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    /// Output format: table, json, yaml, or wide.
    #[arg(short = 'o', long, global = true, value_enum)]
    pub output: Option<OutputFormat>,
    /// Exactly equivalent to `--output json`; cannot be combined with `--output`
    /// (CLI-R-11: `-o/--output` is the single format selector).
    #[arg(long, global = true, conflicts_with = "output")]
    pub json: bool,
    /// Disable ANSI color in output.
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Suppress non-essential progress output.
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Enable verbose diagnostic logging.
    #[arg(long, global = true)]
    pub verbose: bool,
    /// Preview the request without sending it to the server.
    #[arg(long, global = true)]
    pub dry_run: bool,
    /// Skip the confirmation prompt for destructive operations.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    /// Expected current revision for optimistic-concurrency updates.
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
    /// Write output to this file instead of stdout.
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

/// Which tier of the token ladder (CLI-R-40 + the dev-file fallback) actually supplied
/// the effective token. Drives 401 diagnostics only — precedence itself is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenSource {
    /// `--token` / `FLOWPLANE_TOKEN` — a deliberate per-invocation choice.
    FlagOrEnv,
    /// The selected context's stored token.
    Context,
    /// The config file's top-level token.
    ConfigFile,
    /// The `credentials` file (written by `auth login`).
    Credentials,
    /// The well-known `~/.flowplane/dev-token` fallback (loopback servers only).
    DevFile,
}

impl TokenSource {
    /// A persistent store that can silently go stale (design AC 14): everything a user
    /// once wrote down, as opposed to the per-invocation flag/env tier or the dev file.
    pub(crate) fn is_persistent_store(self) -> bool {
        matches!(
            self,
            TokenSource::Context | TokenSource::ConfigFile | TokenSource::Credentials
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EffectiveConfig {
    pub(crate) server: String,
    pub(crate) org: Option<String>,
    pub(crate) team: Option<String>,
    pub(crate) token: Option<String>,
    /// Which ladder tier supplied `token` (`None` when no token resolved). `DevFile`
    /// drives the one-line stderr notice and the stale-token hint on a 401; persistent
    /// stores drive the shadowed-credential hint (design AC 14).
    pub(crate) token_source: Option<TokenSource>,
    /// True iff the dev-token fallback WOULD have applied (non-empty file + loopback
    /// server) — regardless of whether a higher tier won. Powers the shadowed-credential
    /// 401 hint without re-doing IO in the client.
    pub(crate) dev_fallback_available: bool,
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

pub(crate) fn effective(global: &GlobalOptions) -> Result<EffectiveConfig> {
    let file = read_config()?;
    let credentials = fs::read_to_string(credentials_path())
        .ok()
        .map(|s| s.trim().to_string());
    // Lowest-precedence token source: the server-written well-known dev-token file
    // (FP-DEC-0012). HOME-only path, trimmed like the credentials read; `resolve()`
    // applies the loopback gate and every higher-precedence source.
    let dev_token = crate::paths::dev_token_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|s| s.trim().to_string());
    let resolved = resolve(global, file, credentials, dev_token)?;
    if resolved.token_source == Some(TokenSource::DevFile) {
        // Never silent: auto-discovered credentials announce themselves (stderr, so
        // `$(flowplane auth token)`-style stdout consumers are unaffected).
        eprintln!("using dev token from ~/.flowplane/dev-token (dev mode)");
    }
    Ok(resolved)
}

/// Literal loopback test on the EFFECTIVE server URL, gating the dev-token fallback so a
/// stale local dev token is never sent to a remote control plane. Deliberately no DNS
/// resolution (design open question 1): only `localhost`, `127.0.0.0/8` IPv4 literals, and
/// the bracketed IPv6 loopback `[::1]` qualify (URL syntax requires the brackets).
///
/// The host MUST be extracted with the same URL semantics the HTTP client uses
/// (`reqwest::Url`), never a hand-written parser: the gate and the request builder have to
/// agree on the host, or a crafted URL (e.g. `https://evil.example?@127.0.0.1`) could pass
/// the gate while the request goes elsewhere. An unparseable URL fails closed (gate shut —
/// the request itself would fail anyway).
fn server_is_loopback(server: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    // `host_str` may carry IPv6 brackets; strip them before parsing.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.octets()[0] == 127;
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        return v6 == std::net::Ipv6Addr::LOCALHOST;
    }
    false
}

/// Pure precedence resolver (CLI-R-40): `flag > env > context > file > default` for every
/// value. The flag-or-env tier is already folded into `global.*` by clap (each arg's
/// `env = …`); this layers context → file → credentials/default beneath it, then — for the
/// token only, and only when the effective server is loopback — the dev-token file as the
/// lowest tier. IO-free so the precedence is unit-testable without touching process env or
/// the filesystem.
fn resolve(
    global: &GlobalOptions,
    file: CliConfig,
    credentials: Option<String>,
    dev_token: Option<String>,
) -> Result<EffectiveConfig> {
    let selected_name = global.context.as_ref().or(file.current_context.as_ref());
    let selected =
        selected_name.and_then(|name| file.contexts.iter().find(|ctx| &ctx.name == name));
    if let Some(name) = &global.context {
        if selected.is_none() {
            anyhow::bail!("context \"{name}\" does not exist");
        }
    }
    // The server is resolved before the token: the dev-token fallback is gated on it.
    let server = global
        .server
        .clone()
        .or_else(|| selected.map(|ctx| ctx.server.clone()))
        .or_else(|| file.base_url.clone())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string());
    // Whether the dev fallback WOULD apply (non-empty file + loopback server) — computed
    // unconditionally so 401 diagnostics can name a shadowed dev token (design AC 14).
    let dev_fallback_available =
        dev_token.as_deref().is_some_and(|s| !s.is_empty()) && server_is_loopback(&server);
    let explicit_token = global
        .token
        .clone()
        .map(|t| (t, TokenSource::FlagOrEnv))
        .or_else(|| {
            selected
                .and_then(|ctx| ctx.token.clone())
                .map(|t| (t, TokenSource::Context))
        })
        .or_else(|| file.token.clone().map(|t| (t, TokenSource::ConfigFile)))
        .or_else(|| credentials.map(|t| (t, TokenSource::Credentials)));
    let (token, token_source) = match explicit_token {
        // A non-empty explicit source wins outright.
        Some((token, source)) if !token.is_empty() => (Some(token), Some(source)),
        // A PRESENT-but-empty explicit source (e.g. `--token ""`, an empty credentials
        // file) yields no token AND suppresses the dev fallback — exactly the pre-fallback
        // behavior. An empty source must never cause an ambient dev credential to be sent.
        Some(_) => (None, None),
        None if dev_fallback_available => (dev_token, Some(TokenSource::DevFile)),
        None => (None, None),
    };
    Ok(EffectiveConfig {
        server,
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
        token_source,
        dev_fallback_available,
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
    use super::{resolve, CliConfig, GlobalOptions, NamedContext, TokenSource, DEFAULT_SERVER};

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
        let eff = resolve(&o, file_with_ctx(), Some("cred-token".to_string()), None).unwrap();
        assert_eq!(eff.server, "https://flag.example");
        assert_eq!(eff.org.as_deref(), Some("flag-org"));
        assert_eq!(eff.team.as_deref(), Some("flag-team"));
        assert_eq!(eff.token.as_deref(), Some("flag-token"));
    }

    #[test]
    fn context_beats_file_when_no_flag_or_env() {
        let eff = resolve(
            &opts(),
            file_with_ctx(),
            Some("cred-token".to_string()),
            None,
        )
        .unwrap();
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
        let eff = resolve(&opts(), file, Some("cred-token".to_string()), None).unwrap();
        assert_eq!(eff.server, "https://file.example");
        assert_eq!(eff.token.as_deref(), Some("file-token"));
    }

    #[test]
    fn credentials_then_default_are_the_lowest_token_and_server_tiers() {
        let eff = resolve(
            &opts(),
            CliConfig::default(),
            Some("cred-token".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(eff.token.as_deref(), Some("cred-token"));
        // No server anywhere → the built-in default.
        assert_eq!(eff.server, DEFAULT_SERVER);
        // No token anywhere → None.
        let eff = resolve(&opts(), CliConfig::default(), None, None).unwrap();
        assert_eq!(eff.token, None);
    }

    #[test]
    fn timeout_follows_uniform_precedence_flag_context_file_default() {
        // flag (folded from --timeout/FLOWPLANE_TIMEOUT) wins.
        let mut o = opts();
        o.timeout = Some(99);
        assert_eq!(
            resolve(&o, file_with_ctx(), None, None).unwrap().timeout,
            99
        );
        // no flag/env → selected context timeout (ctx sets 11).
        assert_eq!(
            resolve(&opts(), file_with_ctx(), None, None)
                .unwrap()
                .timeout,
            11
        );
        // no flag/env/context → file timeout.
        let file = CliConfig {
            timeout: Some(7),
            ..CliConfig::default()
        };
        assert_eq!(resolve(&opts(), file, None, None).unwrap().timeout, 7);
        // nothing anywhere → default 30.
        assert_eq!(
            resolve(&opts(), CliConfig::default(), None, None)
                .unwrap()
                .timeout,
            30
        );
    }

    #[test]
    fn unknown_explicit_context_is_an_error() {
        let mut o = opts();
        o.context = Some("does-not-exist".to_string());
        assert!(resolve(&o, CliConfig::default(), None, None).is_err());
    }

    // --- dev-token fallback (fpv2-wvp.4): lowest precedence, loopback-gated ---

    fn dev() -> Option<String> {
        Some("dev-file-token".to_string())
    }

    #[test]
    fn dev_token_used_only_when_nothing_else_and_server_is_loopback() {
        // Default server is loopback (127.0.0.1) and no other source: dev file wins.
        let eff = resolve(&opts(), CliConfig::default(), None, dev()).unwrap();
        assert_eq!(eff.token.as_deref(), Some("dev-file-token"));
        assert_eq!(eff.token_source, Some(TokenSource::DevFile));
    }

    #[test]
    fn every_other_token_source_beats_the_dev_file() {
        // flag/env tier
        let mut o = opts();
        o.token = Some("flag-token".to_string());
        let eff = resolve(&o, CliConfig::default(), None, dev()).unwrap();
        assert_eq!(eff.token.as_deref(), Some("flag-token"));
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
        // context tier (note: ctx server is non-loopback anyway; the point is precedence)
        let eff = resolve(&opts(), file_with_ctx(), None, dev()).unwrap();
        assert_eq!(eff.token.as_deref(), Some("ctx-token"));
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
        // file tier
        let file = CliConfig {
            token: Some("file-token".to_string()),
            ..CliConfig::default()
        };
        let eff = resolve(&opts(), file, None, dev()).unwrap();
        assert_eq!(eff.token.as_deref(), Some("file-token"));
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
        // credentials tier
        let eff = resolve(
            &opts(),
            CliConfig::default(),
            Some("cred-token".to_string()),
            dev(),
        )
        .unwrap();
        assert_eq!(eff.token.as_deref(), Some("cred-token"));
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
    }

    #[test]
    fn dev_token_never_used_for_a_non_loopback_server() {
        let mut o = opts();
        o.server = Some("https://cp.example.com".to_string());
        let eff = resolve(&o, CliConfig::default(), None, dev()).unwrap();
        assert_eq!(
            eff.token, None,
            "no token may be synthesized for a remote CP"
        );
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
    }

    #[test]
    fn empty_dev_token_file_is_ignored() {
        let eff = resolve(&opts(), CliConfig::default(), None, Some(String::new())).unwrap();
        assert_eq!(eff.token, None);
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
    }

    #[test]
    fn loopback_test_is_literal_no_dns() {
        use super::server_is_loopback;
        // qualifying
        assert!(server_is_loopback("http://localhost:8096"));
        assert!(server_is_loopback("http://LOCALHOST"));
        assert!(server_is_loopback("http://127.0.0.1:8080"));
        assert!(server_is_loopback("https://127.5.4.3/api"));
        assert!(server_is_loopback("http://[::1]:8080"));
        // not qualifying
        assert!(!server_is_loopback("https://cp.example.com"));
        assert!(!server_is_loopback("http://128.0.0.1:8080"));
        assert!(!server_is_loopback("http://10.0.0.1"));
        // a named host is never resolved, even if it WOULD resolve to loopback
        assert!(!server_is_loopback("http://my-local-alias:8080"));
        // host-suffix tricks must not qualify
        assert!(!server_is_loopback("http://127.0.0.1.evil.example"));
        assert!(!server_is_loopback("http://localhost.evil.example"));
        // IPv6 non-loopback
        assert!(!server_is_loopback("http://[2001:db8::1]:8080"));
    }

    #[test]
    fn loopback_gate_and_http_client_cannot_disagree_about_the_host() {
        use super::server_is_loopback;
        // Regressions for the hand-parser bypass class: URLs whose QUERY/FRAGMENT smuggle
        // an `@<loopback>` — reqwest targets evil.example, so the gate must say false.
        assert!(!server_is_loopback("https://evil.example?@127.0.0.1"));
        assert!(!server_is_loopback("https://evil.example#@[::1]"));
        assert!(!server_is_loopback("https://evil.example/?@localhost"));
        // Userinfo trick: the real host is evil.com.
        assert!(!server_is_loopback("http://127.0.0.1@evil.com/"));
        assert!(!server_is_loopback("http://localhost@evil.com:8080"));
        // Unparseable URLs fail closed (URL syntax requires brackets for IPv6, so a bare
        // `::1` authority is simply not a valid URL — documented contract).
        assert!(!server_is_loopback("http://::1:8080"));
        assert!(!server_is_loopback("not a url"));
        assert!(!server_is_loopback(""));
    }

    #[test]
    fn present_but_empty_explicit_source_suppresses_the_dev_fallback() {
        // `--token ""` (or an empty env/context/file/credentials value) must yield NO
        // token — never an ambient dev credential (pre-fallback behavior preserved).
        let mut o = opts();
        o.token = Some(String::new());
        let eff = resolve(&o, CliConfig::default(), None, dev()).unwrap();
        assert_eq!(eff.token, None);
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
        // empty config-file token
        let file = CliConfig {
            token: Some(String::new()),
            ..CliConfig::default()
        };
        let eff = resolve(&opts(), file, None, dev()).unwrap();
        assert_eq!(eff.token, None);
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
        // empty context token (context server forced to loopback so this genuinely tests
        // presence-suppression, not the loopback gate)
        let mut ctx0 = ctx("prod");
        ctx0.token = Some(String::new());
        ctx0.server = "http://127.0.0.1:9999".to_string();
        let file = CliConfig {
            current_context: Some("prod".to_string()),
            contexts: vec![ctx0],
            ..CliConfig::default()
        };
        let eff = resolve(&opts(), file, None, dev()).unwrap();
        assert_eq!(eff.token, None);
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
        // empty credentials file
        let eff = resolve(&opts(), CliConfig::default(), Some(String::new()), dev()).unwrap();
        assert_eq!(eff.token, None);
        assert_ne!(eff.token_source, Some(TokenSource::DevFile));
    }

    // --- AC 14: token-source classification + shadowed-dev-token availability ---

    #[test]
    fn token_source_classifies_each_ladder_tier() {
        // flag/env — a deliberate per-invocation choice, never a "persistent store".
        let mut o = opts();
        o.token = Some("flag-token".to_string());
        let eff = resolve(&o, CliConfig::default(), None, None).unwrap();
        assert_eq!(eff.token_source, Some(TokenSource::FlagOrEnv));
        assert!(!TokenSource::FlagOrEnv.is_persistent_store());
        // context
        let eff = resolve(&opts(), file_with_ctx(), None, None).unwrap();
        assert_eq!(eff.token_source, Some(TokenSource::Context));
        // config file
        let file = CliConfig {
            token: Some("file-token".to_string()),
            ..CliConfig::default()
        };
        let eff = resolve(&opts(), file, None, None).unwrap();
        assert_eq!(eff.token_source, Some(TokenSource::ConfigFile));
        // credentials
        let eff = resolve(
            &opts(),
            CliConfig::default(),
            Some("cred-token".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(eff.token_source, Some(TokenSource::Credentials));
        // no token at all
        let eff = resolve(&opts(), CliConfig::default(), None, None).unwrap();
        assert_eq!(eff.token_source, None);
        // persistent-store classification
        assert!(TokenSource::Context.is_persistent_store());
        assert!(TokenSource::ConfigFile.is_persistent_store());
        assert!(TokenSource::Credentials.is_persistent_store());
        assert!(!TokenSource::DevFile.is_persistent_store());
    }

    #[test]
    fn dev_fallback_available_tracks_file_and_loopback_independently_of_the_winner() {
        // Credentials win, but a live dev file on a loopback server is still flagged
        // available — this powers the shadowed-credential 401 hint.
        let eff = resolve(
            &opts(),
            CliConfig::default(),
            Some("cred-token".to_string()),
            dev(),
        )
        .unwrap();
        assert_eq!(eff.token_source, Some(TokenSource::Credentials));
        assert!(eff.dev_fallback_available);
        // Non-loopback server: not available, whatever the file says.
        let mut o = opts();
        o.server = Some("https://cp.example.com".to_string());
        o.token = Some("flag-token".to_string());
        let eff = resolve(&o, CliConfig::default(), None, dev()).unwrap();
        assert!(!eff.dev_fallback_available);
        // No/empty dev file: not available.
        let eff = resolve(
            &opts(),
            CliConfig::default(),
            Some("cred-token".to_string()),
            Some(String::new()),
        )
        .unwrap();
        assert!(!eff.dev_fallback_available);
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

// Permission tests for the private-write helper live with the helper in `crate::paths`.
