//! Server configuration (spec/10 §10, D-005).
//!
//! Precedence for the **server**: environment variables > config file (TOML, path from
//! `FLOWPLANE_CONFIG`) > built-in defaults. The CLI client resolves flag-first instead
//! (D-005); that lives in fp-cli.
//!
//! Every value is validated before the server starts; an invalid configuration is an
//! `invalid_config` error with a hint — never a panic, never a silent fallback.

use fp_domain::{DomainError, DomainResult};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Fully resolved and validated server configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// REST + MCP listen address.
    pub api_addr: SocketAddr,
    /// xDS gRPC listen address (ADS/SDS + capture services).
    pub xds_addr: SocketAddr,
    /// PostgreSQL connection URL. Required.
    pub database_url: String,
    /// Maximum DB pool connections.
    pub db_max_connections: u32,
    /// TLS material for the API listener; `None` only when `api_insecure` is set (D-008).
    pub api_tls: Option<TlsConfig>,
    /// mTLS material for the xDS listener (server cert/key + client CA). xDS only serves
    /// over mTLS in production; `None` disables the listener outside dev mode (spec/04 §1.2).
    pub xds_tls: Option<XdsTlsConfig>,
    /// Explicit opt-in to serve the API over plaintext. Logs a startup warning.
    pub api_insecure: bool,
    /// Log output format.
    pub log_format: LogFormat,
    /// `tracing` env-filter directive (e.g. `info`, `fp_core=debug,info`).
    pub log_filter: String,
    /// OTLP endpoint for traces; `None` disables export (spans still recorded locally).
    pub otlp_endpoint: Option<String>,
    /// Dev mode: in-process OIDC issuer + seeded resources. Requires the `dev-oidc` build
    /// feature AND, in release builds, an explicit acknowledgment env var (spec/10 §4a).
    pub dev_mode: bool,
    /// OIDC issuer for production auth (any compliant IdP, Q-004). `None` + !dev_mode =
    /// degraded mode: authenticated endpoints answer 503 with a configuration hint.
    pub oidc: Option<OidcSettings>,
    /// Per-tenant mutating-request budget per minute (spec/10 §4a).
    pub tenant_write_limit_per_minute: u32,
    /// Local-only opt-in (#113): when true, an uninitialized non-dev instance with no
    /// operator-supplied bootstrap token falls back to generating one and logging it. Enabled
    /// only by the exact value `yes-this-is-local-only`; otherwise the instance fails closed.
    pub allow_logged_bootstrap_token: bool,
    /// Dev mode only: when set, the minted per-boot dev token is also written to this path so a
    /// sibling container's init step can read it (the token is otherwise only logged). Ignored
    /// outside dev mode. Env `FLOWPLANE_DEV_TOKEN_PATH`.
    pub dev_token_path: Option<PathBuf>,
    /// HTTP admin URL of the first-party rate-limit service. When set, the CP `rls_sync` worker
    /// pushes the policy set here on a 60 s reconcile (S5). `None` disables the worker. Env
    /// `FLOWPLANE_RLS_ADMIN_URL`. (The RLS gRPC URL that drives S6 CDS injection,
    /// `FLOWPLANE_RLS_GRPC_URL`, is a separate listener.)
    pub rls_admin_url: Option<String>,
    /// Seconds between `rls_sync` reconcile pushes (S5). Defaults to 60 (the design's reconcile
    /// window) and is clamped to 1..=60 — the knob may only *lower* the interval to make automated
    /// tests converge quickly, never raise it past the documented 60 s backstop. Env
    /// `FLOWPLANE_RLS_RECONCILE_SECS`.
    pub rls_reconcile_secs: u64,
    /// gRPC URL (`host:port`) of the first-party rate-limit service. When set, the CP synthesizes
    /// and injects the built-in `rate_limit_cluster` into CDS (S6) and defaults the
    /// `global_rate_limit` filter to it. `None` disables injection. Env `FLOWPLANE_RLS_GRPC_URL`.
    pub rls_grpc_url: Option<String>,
    /// mTLS material the synthesized `rate_limit_cluster` presents to / verifies the RLS with
    /// (S6). All three paths come together or not at all (see `resolve`). `None` => the built-in
    /// cluster dials the RLS in plaintext h2c (dev only). Env `FLOWPLANE_DATAPLANE_TLS_*`.
    pub dataplane_tls: Option<DataplaneTlsConfig>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OidcSettings {
    pub issuer: String,
    pub audience: String,
    pub jwks_uri: Option<String>,
    /// Optional operator-supplied CA bundle (PEM, one or more certs) that the OIDC
    /// HTTP client trusts *in addition to* the bundled webpki roots — needed when the
    /// IdP is reachable only through a TLS-intercepting egress proxy (#171). Lives
    /// inside `OidcSettings`, so it only takes effect when OIDC is configured.
    pub ca_bundle_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct XdsTlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// CA bundle that dataplane client certificates must chain to.
    pub client_ca_path: PathBuf,
}

/// mTLS material for the built-in `rate_limit_cluster` (S6, spec/04:31). The Envoy→RLS hop
/// authenticates with the dataplane client cert and verifies the RLS server cert against the CA.
/// All three paths are required together (fail-closed in `resolve`); the files are read by Envoy
/// on the dataplane host, so the control plane only ships the paths.
#[derive(Debug, Clone, PartialEq)]
pub struct DataplaneTlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// CA bundle the dataplane uses to verify the RLS server certificate.
    pub client_ca_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Structured JSON lines (production default).
    Json,
    /// Human-readable (development).
    Pretty,
}

/// Raw deserialization target for the TOML config file. All fields optional; merged under env.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    api_addr: Option<String>,
    xds_addr: Option<String>,
    database_url: Option<String>,
    db_max_connections: Option<u32>,
    api_tls_cert: Option<String>,
    api_tls_key: Option<String>,
    xds_tls_cert: Option<String>,
    xds_tls_key: Option<String>,
    xds_tls_client_ca: Option<String>,
    api_insecure: Option<bool>,
    dev_mode: Option<bool>,
    oidc_issuer: Option<String>,
    oidc_audience: Option<String>,
    oidc_jwks_uri: Option<String>,
    oidc_ca_bundle: Option<String>,
    log_format: Option<LogFormat>,
    log_filter: Option<String>,
    otlp_endpoint: Option<String>,
    dev_token_path: Option<String>,
    rls_admin_url: Option<String>,
    rls_grpc_url: Option<String>,
    dataplane_tls_cert: Option<String>,
    dataplane_tls_key: Option<String>,
    dataplane_tls_client_ca: Option<String>,
}

const DEFAULT_API_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_LOG_FILTER: &str = "info";

impl ServerConfig {
    /// Load from process environment + optional `FLOWPLANE_CONFIG` file.
    pub fn load() -> DomainResult<Self> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let file = match env.get("FLOWPLANE_CONFIG") {
            Some(path) => Self::read_file(Path::new(path))?,
            None => FileConfig::default(),
        };
        Self::resolve(&env, file)
    }

    fn read_file(path: &Path) -> DomainResult<FileConfig> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            DomainError::invalid_config(format!("cannot read config file {}: {e}", path.display()))
                .with_hint("check FLOWPLANE_CONFIG points at a readable TOML file")
        })?;
        toml::from_str(&raw).map_err(|e| {
            DomainError::invalid_config(format!("invalid TOML in {}: {e}", path.display()))
        })
    }

    /// Merge env (highest) over file over defaults, then validate.
    fn resolve(env: &HashMap<String, String>, file: FileConfig) -> DomainResult<Self> {
        let get = |key: &str| env.get(key).map(String::as_str);

        let api_addr_raw = get("FLOWPLANE_API_ADDR")
            .map(str::to_owned)
            .or(file.api_addr)
            .unwrap_or_else(|| DEFAULT_API_ADDR.to_string());
        let api_addr: SocketAddr = api_addr_raw.parse().map_err(|_| {
            DomainError::invalid_config(format!(
                "FLOWPLANE_API_ADDR \"{api_addr_raw}\" is not a valid socket address"
            ))
            .with_hint("use host:port, e.g. 0.0.0.0:8080")
        })?;

        let xds_addr_raw = get("FLOWPLANE_XDS_ADDR")
            .map(str::to_owned)
            .or(file.xds_addr)
            .unwrap_or_else(|| "0.0.0.0:18000".to_string());
        let xds_addr: SocketAddr = xds_addr_raw.parse().map_err(|_| {
            DomainError::invalid_config(format!(
                "FLOWPLANE_XDS_ADDR \"{xds_addr_raw}\" is not a valid socket address"
            ))
        })?;

        let database_url = get("FLOWPLANE_DATABASE_URL")
            .or(get("DATABASE_URL"))
            .map(str::to_owned)
            .or(file.database_url)
            .ok_or_else(|| {
                DomainError::invalid_config("database URL is not configured").with_hint(
                    "set FLOWPLANE_DATABASE_URL (or DATABASE_URL), e.g. postgres://user:pass@host/flowplane",
                )
            })?;

        let db_max_connections = match get("FLOWPLANE_DB_MAX_CONNECTIONS") {
            Some(raw) => raw.parse().map_err(|_| {
                DomainError::invalid_config(format!(
                    "FLOWPLANE_DB_MAX_CONNECTIONS \"{raw}\" is not a positive integer"
                ))
            })?,
            None => file
                .db_max_connections
                .unwrap_or(DEFAULT_DB_MAX_CONNECTIONS),
        };
        if db_max_connections == 0 {
            return Err(DomainError::invalid_config(
                "FLOWPLANE_DB_MAX_CONNECTIONS must be >= 1",
            ));
        }

        let cert = get("FLOWPLANE_API_TLS_CERT")
            .map(str::to_owned)
            .or(file.api_tls_cert);
        let key = get("FLOWPLANE_API_TLS_KEY")
            .map(str::to_owned)
            .or(file.api_tls_key);
        let api_tls = match (cert, key) {
            (Some(cert_path), Some(key_path)) => Some(TlsConfig {
                cert_path: cert_path.into(),
                key_path: key_path.into(),
            }),
            (None, None) => None,
            _ => {
                return Err(DomainError::invalid_config(
                    "FLOWPLANE_API_TLS_CERT and FLOWPLANE_API_TLS_KEY must be set together",
                ))
            }
        };

        let xds_cert = get("FLOWPLANE_XDS_TLS_CERT")
            .map(str::to_owned)
            .or(file.xds_tls_cert);
        let xds_key = get("FLOWPLANE_XDS_TLS_KEY")
            .map(str::to_owned)
            .or(file.xds_tls_key);
        let xds_client_ca = get("FLOWPLANE_XDS_TLS_CLIENT_CA")
            .map(str::to_owned)
            .or(file.xds_tls_client_ca);
        let xds_tls = match (xds_cert, xds_key, xds_client_ca) {
            (Some(cert_path), Some(key_path), Some(client_ca_path)) => Some(XdsTlsConfig {
                cert_path: cert_path.into(),
                key_path: key_path.into(),
                client_ca_path: client_ca_path.into(),
            }),
            (None, None, None) => None,
            _ => {
                return Err(DomainError::invalid_config(
                    "FLOWPLANE_XDS_TLS_CERT, FLOWPLANE_XDS_TLS_KEY, and \
                     FLOWPLANE_XDS_TLS_CLIENT_CA must be set together",
                )
                .with_hint("xDS mTLS needs the server identity AND the dataplane client CA"))
            }
        };

        let api_insecure = match get("FLOWPLANE_API_INSECURE") {
            Some(raw) => parse_bool("FLOWPLANE_API_INSECURE", raw)?,
            None => file.api_insecure.unwrap_or(false),
        };

        // D-008: plaintext API requires explicit opt-in.
        if api_tls.is_none() && !api_insecure {
            return Err(DomainError::invalid_config(
                "the API listener has no TLS material and plaintext was not explicitly allowed",
            )
            .with_hint(
                "set FLOWPLANE_API_TLS_CERT/FLOWPLANE_API_TLS_KEY, or opt in to plaintext with \
                 FLOWPLANE_API_INSECURE=true (e.g. behind a TLS-terminating proxy)",
            ));
        }

        let log_format = match get("FLOWPLANE_LOG_FORMAT") {
            Some("json") => LogFormat::Json,
            Some("pretty") => LogFormat::Pretty,
            Some(other) => {
                return Err(DomainError::invalid_config(format!(
                    "FLOWPLANE_LOG_FORMAT \"{other}\" is not one of: json, pretty"
                )))
            }
            None => file.log_format.unwrap_or(LogFormat::Json),
        };

        let log_filter = get("FLOWPLANE_LOG")
            .map(str::to_owned)
            .or(file.log_filter)
            .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_string());

        let otlp_endpoint = get("FLOWPLANE_OTLP_ENDPOINT")
            .map(str::to_owned)
            .or(file.otlp_endpoint);

        let dev_mode = match get("FLOWPLANE_DEV_MODE") {
            Some(raw) => parse_bool("FLOWPLANE_DEV_MODE", raw)?,
            None => file.dev_mode.unwrap_or(false),
        };

        let oidc_issuer = get("FLOWPLANE_OIDC_ISSUER")
            .map(str::to_owned)
            .or(file.oidc_issuer);
        let oidc_audience = get("FLOWPLANE_OIDC_AUDIENCE")
            .map(str::to_owned)
            .or(file.oidc_audience);
        let oidc = match (oidc_issuer, oidc_audience) {
            (Some(issuer), Some(audience)) => Some(OidcSettings {
                issuer,
                audience,
                jwks_uri: get("FLOWPLANE_OIDC_JWKS_URI")
                    .map(str::to_owned)
                    .or(file.oidc_jwks_uri),
                // Operator CA bundle for a TLS-intercepting proxy (#171); env over file.
                // Only meaningful here — when OIDC is enabled. Set without issuer/audience
                // (or in dev mode) it has no effect, since no `OidcSettings` is built.
                ca_bundle_path: get("FLOWPLANE_OIDC_CA_BUNDLE")
                    .map(str::to_owned)
                    .or(file.oidc_ca_bundle)
                    .map(PathBuf::from),
            }),
            (None, None) => None,
            _ => {
                return Err(DomainError::invalid_config(
                    "FLOWPLANE_OIDC_ISSUER and FLOWPLANE_OIDC_AUDIENCE must be set together",
                ))
            }
        };
        let tenant_write_limit_per_minute = match get("FLOWPLANE_TENANT_WRITE_LIMIT_PER_MIN") {
            Some(raw) => raw.parse().ok().filter(|v| *v >= 1).ok_or_else(|| {
                DomainError::invalid_config(format!(
                    "FLOWPLANE_TENANT_WRITE_LIMIT_PER_MIN \"{raw}\" is not a positive integer"
                ))
            })?,
            None => 120,
        };

        if dev_mode && oidc.is_some() {
            return Err(DomainError::invalid_config(
                "FLOWPLANE_DEV_MODE and FLOWPLANE_OIDC_* are mutually exclusive",
            )
            .with_hint("dev mode brings its own in-process issuer"));
        }

        // Only the exact opt-in value re-enables the legacy generate-and-log path (#113).
        let allow_logged_bootstrap_token =
            get("FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN") == Some("yes-this-is-local-only");

        let dev_token_path = get("FLOWPLANE_DEV_TOKEN_PATH")
            .map(PathBuf::from)
            .or_else(|| file.dev_token_path.map(PathBuf::from));

        let rls_admin_url = get("FLOWPLANE_RLS_ADMIN_URL")
            .map(str::to_owned)
            .or(file.rls_admin_url);

        let rls_grpc_url = get("FLOWPLANE_RLS_GRPC_URL")
            .map(str::to_owned)
            .or(file.rls_grpc_url);

        // Clamped to 1..=60: the knob exists only to make automated tests converge faster than the
        // design's 60 s reconcile window — it may LOWER the interval but never raise it past 60 s,
        // so the documented "missed delivery converges within 60 s" backstop always holds.
        let rls_reconcile_secs = get("FLOWPLANE_RLS_RECONCILE_SECS")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|s| *s > 0)
            .map(|s| s.min(60))
            .unwrap_or(60);

        let dataplane_tls_cert = get("FLOWPLANE_DATAPLANE_TLS_CERT")
            .map(str::to_owned)
            .or(file.dataplane_tls_cert);
        let dataplane_tls_key = get("FLOWPLANE_DATAPLANE_TLS_KEY")
            .map(str::to_owned)
            .or(file.dataplane_tls_key);
        let dataplane_tls_client_ca = get("FLOWPLANE_DATAPLANE_TLS_CLIENT_CA")
            .map(str::to_owned)
            .or(file.dataplane_tls_client_ca);
        let dataplane_tls = match (
            dataplane_tls_cert,
            dataplane_tls_key,
            dataplane_tls_client_ca,
        ) {
            (Some(cert_path), Some(key_path), Some(client_ca_path)) => Some(DataplaneTlsConfig {
                cert_path: cert_path.into(),
                key_path: key_path.into(),
                client_ca_path: client_ca_path.into(),
            }),
            (None, None, None) => None,
            _ => {
                return Err(DomainError::invalid_config(
                    "FLOWPLANE_DATAPLANE_TLS_CERT, FLOWPLANE_DATAPLANE_TLS_KEY, and \
                     FLOWPLANE_DATAPLANE_TLS_CLIENT_CA must be set together",
                )
                .with_hint(
                    "Envoy→RLS mTLS needs the dataplane client identity AND the RLS server CA",
                ))
            }
        };

        Ok(Self {
            api_addr,
            xds_addr,
            database_url,
            db_max_connections,
            api_tls,
            xds_tls,
            api_insecure,
            log_format,
            log_filter,
            otlp_endpoint,
            dev_mode,
            oidc,
            tenant_write_limit_per_minute,
            allow_logged_bootstrap_token,
            dev_token_path,
            rls_admin_url,
            rls_reconcile_secs,
            rls_grpc_url,
            dataplane_tls,
        })
    }
}

fn parse_bool(key: &str, raw: &str) -> DomainResult<bool> {
    match raw {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(DomainError::invalid_config(format!(
            "{key} \"{raw}\" is not a boolean (use true/false)"
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn base_env() -> HashMap<String, String> {
        HashMap::from([
            (
                "FLOWPLANE_DATABASE_URL".into(),
                "postgres://x@localhost/db".into(),
            ),
            ("FLOWPLANE_API_INSECURE".into(), "true".into()),
        ])
    }

    #[test]
    fn defaults_apply_when_only_required_values_present() {
        let cfg = ServerConfig::resolve(&base_env(), FileConfig::default())
            .expect("minimal valid env must resolve");
        assert_eq!(cfg.api_addr.port(), 8080);
        assert_eq!(cfg.db_max_connections, DEFAULT_DB_MAX_CONNECTIONS);
        assert_eq!(cfg.log_filter, "info");
        assert!(cfg.api_tls.is_none());
    }

    #[test]
    fn missing_database_url_yields_invalid_config_with_hint() {
        let err = ServerConfig::resolve(&HashMap::new(), FileConfig::default());
        let e = err.expect_err("must fail without a database URL");
        assert_eq!(e.code, fp_domain::ErrorCode::InvalidConfig);
        assert!(e.hint.is_some());
    }

    #[test]
    fn plaintext_without_optin_is_rejected() {
        let mut env = base_env();
        env.remove("FLOWPLANE_API_INSECURE");
        let err = ServerConfig::resolve(&env, FileConfig::default());
        assert!(
            err.is_err(),
            "D-008: no TLS and no explicit insecure opt-in must fail"
        );
    }

    #[test]
    fn env_overrides_file() {
        let mut env = base_env();
        env.insert("FLOWPLANE_API_ADDR".into(), "127.0.0.1:9999".into());
        let file = FileConfig {
            api_addr: Some("0.0.0.0:1111".into()),
            ..FileConfig::default()
        };
        let cfg = ServerConfig::resolve(&env, file);
        assert_eq!(cfg.ok().map(|c| c.api_addr.port()), Some(9999));
    }

    #[test]
    fn dev_token_path_resolves_env_over_file() {
        // Absent in both → None.
        let cfg = ServerConfig::resolve(&base_env(), FileConfig::default()).expect("resolves");
        assert_eq!(cfg.dev_token_path, None);

        // File only.
        let file = FileConfig {
            dev_token_path: Some("/from/file".into()),
            ..FileConfig::default()
        };
        let cfg = ServerConfig::resolve(&base_env(), file).expect("resolves");
        assert_eq!(cfg.dev_token_path, Some(PathBuf::from("/from/file")));

        // Env overrides file.
        let mut env = base_env();
        env.insert("FLOWPLANE_DEV_TOKEN_PATH".into(), "/from/env".into());
        let file = FileConfig {
            dev_token_path: Some("/from/file".into()),
            ..FileConfig::default()
        };
        let cfg = ServerConfig::resolve(&env, file).expect("resolves");
        assert_eq!(cfg.dev_token_path, Some(PathBuf::from("/from/env")));
    }

    #[test]
    fn rls_reconcile_secs_defaults_to_60_and_honors_env() {
        // Absent → the design's 60 s reconcile window.
        let cfg = ServerConfig::resolve(&base_env(), FileConfig::default()).expect("resolves");
        assert_eq!(cfg.rls_reconcile_secs, 60);

        // A valid override in 1..=60 is taken verbatim (tests lower it to converge fast).
        let mut env = base_env();
        env.insert("FLOWPLANE_RLS_RECONCILE_SECS".into(), "2".into());
        let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("resolves");
        assert_eq!(cfg.rls_reconcile_secs, 2);

        // A value above 60 is clamped to 60 — the knob can only lower the backstop, never raise it.
        let mut env = base_env();
        env.insert("FLOWPLANE_RLS_RECONCILE_SECS".into(), "300".into());
        let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("resolves");
        assert_eq!(cfg.rls_reconcile_secs, 60, "values above the 60 s backstop must clamp to 60");

        // Garbage and zero both fall back to the default rather than disabling the loop.
        for bad in ["0", "not-a-number", ""] {
            let mut env = base_env();
            env.insert("FLOWPLANE_RLS_RECONCILE_SECS".into(), bad.into());
            let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("resolves");
            assert_eq!(cfg.rls_reconcile_secs, 60, "input {bad:?} must fall back to 60");
        }
    }

    fn oidc_env() -> HashMap<String, String> {
        let mut env = base_env();
        env.insert("FLOWPLANE_OIDC_ISSUER".into(), "https://idp.test".into());
        env.insert("FLOWPLANE_OIDC_AUDIENCE".into(), "flowplane".into());
        env
    }

    #[test]
    fn oidc_ca_bundle_resolves_env_over_file() {
        // Absent in both → None on the resolved OidcSettings.
        let cfg = ServerConfig::resolve(&oidc_env(), FileConfig::default()).expect("resolves");
        assert_eq!(cfg.oidc.expect("oidc set").ca_bundle_path, None);

        // File only.
        let file = FileConfig {
            oidc_ca_bundle: Some("/from/file/ca.pem".into()),
            ..FileConfig::default()
        };
        let cfg = ServerConfig::resolve(&oidc_env(), file).expect("resolves");
        assert_eq!(
            cfg.oidc.expect("oidc set").ca_bundle_path,
            Some(PathBuf::from("/from/file/ca.pem"))
        );

        // Env overrides file.
        let mut env = oidc_env();
        env.insert("FLOWPLANE_OIDC_CA_BUNDLE".into(), "/from/env/ca.pem".into());
        let file = FileConfig {
            oidc_ca_bundle: Some("/from/file/ca.pem".into()),
            ..FileConfig::default()
        };
        let cfg = ServerConfig::resolve(&env, file).expect("resolves");
        assert_eq!(
            cfg.oidc.expect("oidc set").ca_bundle_path,
            Some(PathBuf::from("/from/env/ca.pem"))
        );
    }

    #[test]
    fn oidc_ca_bundle_is_noop_without_issuer_and_audience() {
        // The CA bundle lives inside OidcSettings, which is only built when both issuer
        // and audience are present. Set alone, it must not conjure an OidcSettings.
        let mut env = base_env();
        env.insert("FLOWPLANE_OIDC_CA_BUNDLE".into(), "/some/ca.pem".into());
        let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("resolves");
        assert!(cfg.oidc.is_none(), "CA bundle alone must not enable OIDC");
    }

    #[test]
    fn tls_paths_must_come_in_pairs() {
        let mut env = base_env();
        env.insert("FLOWPLANE_API_TLS_CERT".into(), "/tmp/c.pem".into());
        assert!(ServerConfig::resolve(&env, FileConfig::default()).is_err());
    }

    #[test]
    fn xds_tls_paths_must_come_in_triples() {
        for present in [
            vec!["FLOWPLANE_XDS_TLS_CERT"],
            vec!["FLOWPLANE_XDS_TLS_CERT", "FLOWPLANE_XDS_TLS_KEY"],
            vec!["FLOWPLANE_XDS_TLS_CLIENT_CA"],
        ] {
            let mut env = base_env();
            for key in &present {
                env.insert((*key).into(), "/tmp/x.pem".into());
            }
            assert!(
                ServerConfig::resolve(&env, FileConfig::default()).is_err(),
                "partial xDS TLS config {present:?} must be rejected"
            );
        }
        let mut env = base_env();
        for key in [
            "FLOWPLANE_XDS_TLS_CERT",
            "FLOWPLANE_XDS_TLS_KEY",
            "FLOWPLANE_XDS_TLS_CLIENT_CA",
        ] {
            env.insert(key.into(), "/tmp/x.pem".into());
        }
        let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("full triple ok");
        assert!(cfg.xds_tls.is_some());
    }

    #[test]
    fn dataplane_tls_paths_must_come_in_triples() {
        // Fail-closed: the Envoy→RLS mTLS material is all-or-nothing (acceptance: partial certs
        // are rejected at config load, before any CDS injection).
        for present in [
            vec!["FLOWPLANE_DATAPLANE_TLS_CERT"],
            vec![
                "FLOWPLANE_DATAPLANE_TLS_CERT",
                "FLOWPLANE_DATAPLANE_TLS_KEY",
            ],
            vec![
                "FLOWPLANE_DATAPLANE_TLS_KEY",
                "FLOWPLANE_DATAPLANE_TLS_CLIENT_CA",
            ],
            vec!["FLOWPLANE_DATAPLANE_TLS_CLIENT_CA"],
        ] {
            let mut env = base_env();
            for key in &present {
                env.insert((*key).into(), "/tmp/dp.pem".into());
            }
            assert!(
                ServerConfig::resolve(&env, FileConfig::default()).is_err(),
                "partial dataplane TLS config {present:?} must be rejected"
            );
        }
        let mut env = base_env();
        for key in [
            "FLOWPLANE_DATAPLANE_TLS_CERT",
            "FLOWPLANE_DATAPLANE_TLS_KEY",
            "FLOWPLANE_DATAPLANE_TLS_CLIENT_CA",
        ] {
            env.insert(key.into(), "/tmp/dp.pem".into());
        }
        let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("full triple ok");
        assert!(cfg.dataplane_tls.is_some());
        assert!(
            cfg.rls_grpc_url.is_none(),
            "tls without grpc url is allowed"
        );
    }

    #[test]
    fn rls_grpc_url_parsed_from_env() {
        let mut env = base_env();
        env.insert("FLOWPLANE_RLS_GRPC_URL".into(), "rls.internal:8081".into());
        let cfg = ServerConfig::resolve(&env, FileConfig::default()).expect("resolves");
        assert_eq!(cfg.rls_grpc_url.as_deref(), Some("rls.internal:8081"));
    }

    #[test]
    fn adversarial_values_rejected() {
        for (key, value) in [
            ("FLOWPLANE_API_ADDR", "not-an-addr"),
            ("FLOWPLANE_DB_MAX_CONNECTIONS", "-3"),
            ("FLOWPLANE_DB_MAX_CONNECTIONS", "0"),
            ("FLOWPLANE_API_INSECURE", "maybe"),
            ("FLOWPLANE_LOG_FORMAT", "xml"),
        ] {
            let mut env = base_env();
            env.insert(key.into(), value.into());
            assert!(
                ServerConfig::resolve(&env, FileConfig::default()).is_err(),
                "{key}={value} must be rejected"
            );
        }
    }
}
