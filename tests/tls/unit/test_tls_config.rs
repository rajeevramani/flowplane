use std::sync::Mutex;

use flowplane::config::tls::ApiTlsConfig;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn tls_config_disabled_by_default() {
    let _guard = ENV_MUTEX.lock().unwrap();

    std::env::remove_var("FLOWPLANE_API_TLS_ENABLED");
    std::env::remove_var("FLOWPLANE_API_TLS_CERT_PATH");
    std::env::remove_var("FLOWPLANE_API_TLS_KEY_PATH");
    std::env::remove_var("FLOWPLANE_API_TLS_CHAIN_PATH");

    let config = ApiTlsConfig::from_env().expect("config load");
    assert!(config.is_none());
}

#[test]
fn tls_config_enables_with_required_paths() {
    let _guard = ENV_MUTEX.lock().unwrap();

    std::env::set_var("FLOWPLANE_API_TLS_ENABLED", "true");
    std::env::set_var("FLOWPLANE_API_TLS_CERT_PATH", "/tmp/cert.pem");
    std::env::set_var("FLOWPLANE_API_TLS_KEY_PATH", "/tmp/key.pem");
    std::env::remove_var("FLOWPLANE_API_TLS_CHAIN_PATH");

    let config = ApiTlsConfig::from_env().expect("config load");
    let tls = config.expect("tls enabled");
    assert_eq!(tls.cert_path, std::path::PathBuf::from("/tmp/cert.pem"));
    assert_eq!(tls.key_path, std::path::PathBuf::from("/tmp/key.pem"));
    assert!(tls.chain_path.is_none());

    std::env::remove_var("FLOWPLANE_API_TLS_ENABLED");
    std::env::remove_var("FLOWPLANE_API_TLS_CERT_PATH");
    std::env::remove_var("FLOWPLANE_API_TLS_KEY_PATH");
}

#[test]
fn tls_config_errors_without_key_path() {
    let _guard = ENV_MUTEX.lock().unwrap();

    std::env::set_var("FLOWPLANE_API_TLS_ENABLED", "1");
    std::env::set_var("FLOWPLANE_API_TLS_CERT_PATH", "/tmp/cert.pem");
    std::env::remove_var("FLOWPLANE_API_TLS_KEY_PATH");

    let err = ApiTlsConfig::from_env().expect_err("missing key should error");
    let message = format!("{err}");
    assert!(message.contains("private key path"));

    std::env::remove_var("FLOWPLANE_API_TLS_ENABLED");
    std::env::remove_var("FLOWPLANE_API_TLS_CERT_PATH");
}
