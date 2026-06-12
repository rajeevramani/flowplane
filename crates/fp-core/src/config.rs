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
}

#[derive(Debug, Clone, PartialEq)]
pub struct OidcSettings {
    pub issuer: String,
    pub audience: String,
    pub jwks_uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
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
    api_insecure: Option<bool>,
    dev_mode: Option<bool>,
    oidc_issuer: Option<String>,
    oidc_audience: Option<String>,
    oidc_jwks_uri: Option<String>,
    log_format: Option<LogFormat>,
    log_filter: Option<String>,
    otlp_endpoint: Option<String>,
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

        Ok(Self {
            api_addr,
            xds_addr,
            database_url,
            db_max_connections,
            api_tls,
            api_insecure,
            log_format,
            log_filter,
            otlp_endpoint,
            dev_mode,
            oidc,
            tenant_write_limit_per_minute,
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
    fn tls_paths_must_come_in_pairs() {
        let mut env = base_env();
        env.insert("FLOWPLANE_API_TLS_CERT".into(), "/tmp/c.pem".into());
        assert!(ServerConfig::resolve(&env, FileConfig::default()).is_err());
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
