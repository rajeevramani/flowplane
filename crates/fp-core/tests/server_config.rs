//! Black-box server configuration loading regressions.

#![allow(clippy::expect_used)]

use fp_core::config::ServerConfig;
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
