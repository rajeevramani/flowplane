//! Black-box server configuration loading regressions.

#![allow(clippy::expect_used)]

use fp_core::config::ServerConfig;
use fp_core::services::egress_policy::EgressPolicy;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct EnvRestore {
    values: Vec<(&'static str, Option<String>)>,
}

impl EnvRestore {
    fn capture(keys: &[&'static str]) -> Self {
        Self {
            values: keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect(),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.values {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn unique_temp_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("flowplane-{name}-{}-{nonce}", std::process::id()))
}

#[test]
fn config_file_rls_grpc_plaintext_fails_before_service_startup() {
    let keys = [
        "FLOWPLANE_CONFIG",
        "FLOWPLANE_DATABASE_URL",
        "DATABASE_URL",
        "FLOWPLANE_API_INSECURE",
        "FLOWPLANE_DEV_MODE",
        "FLOWPLANE_DEV_ALLOW_TENANT_FILE_PATHS",
        "FLOWPLANE_DEV_TENANT_FILE_BASE_DIRS",
        "FLOWPLANE_RLS_GRPC_URL",
        "FLOWPLANE_RLS_GRPC_ALLOW_PRODUCTION_PLAINTEXT",
        "FLOWPLANE_DATAPLANE_TLS_CERT",
        "FLOWPLANE_DATAPLANE_TLS_KEY",
        "FLOWPLANE_DATAPLANE_TLS_CLIENT_CA",
    ];
    let _restore = EnvRestore::capture(&keys);
    for key in keys {
        std::env::remove_var(key);
    }

    let path = unique_temp_path("rls-grpc-config.toml");
    std::fs::write(
        &path,
        r#"
database_url = "postgres://x@localhost/db"
api_insecure = true
rls_grpc_url = "rls.internal:8081"
"#,
    )
    .expect("write config");
    std::env::set_var("FLOWPLANE_CONFIG", &path);

    let err =
        ServerConfig::load().expect_err("file-configured RLS plaintext must fail before startup");
    let _ = std::fs::remove_file(&path);
    assert_eq!(err.code, fp_domain::ErrorCode::InvalidConfig);
    assert!(err.message.contains("FLOWPLANE_RLS_GRPC_URL"));
}

#[tokio::test]
async fn config_file_egress_policy_uses_database_deny_and_allowlist() {
    let keys = [
        "FLOWPLANE_CONFIG",
        "FLOWPLANE_DATABASE_URL",
        "DATABASE_URL",
        "FLOWPLANE_API_INSECURE",
        "FLOWPLANE_DEV_MODE",
        "FLOWPLANE_DEV_ALLOW_TENANT_FILE_PATHS",
        "FLOWPLANE_DEV_TENANT_FILE_BASE_DIRS",
        "FLOWPLANE_EGRESS_ALLOWED_DESTINATIONS",
        "FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS",
    ];
    let _restore = EnvRestore::capture(&keys);
    for key in keys {
        std::env::remove_var(key);
    }

    let path = unique_temp_path("egress-config.toml");
    std::fs::write(
        &path,
        r#"
database_url = "postgres://x@203.0.113.10:5432/db"
api_insecure = true
egress_allowed_destinations = "203.0.113.10:5432,203.0.113.11:8443"
"#,
    )
    .expect("write config");
    std::env::set_var("FLOWPLANE_CONFIG", &path);

    let config = ServerConfig::load().expect("load file config");
    let policy = EgressPolicy::from_server_config(&config).await;
    let _ = std::fs::remove_file(&path);

    policy
        .validate_host_port("203.0.113.10", 5432, "test destination")
        .await
        .expect_err("file-configured database destination deny wins");
    let allowed = policy
        .validate_host_port("203.0.113.11", 8443, "test destination")
        .await
        .expect("file-configured egress allowlist is used");
    assert_eq!(
        allowed.allowlist_match.map(|addr| addr.to_string()),
        Some("203.0.113.11:8443".into())
    );
}
