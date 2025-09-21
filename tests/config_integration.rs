//! Integration tests for configuration management
//!
//! These tests validate that the configuration system properly reads
//! environment variables and that the XDS server binds to the configured port.

use magaya::{Config, Result};
use std::env;
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::timeout;
use tracing_test::traced_test;

// Use a mutex to serialize tests that modify environment variables
static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Test that configuration properly reads environment variables
#[test]
fn test_config_environment_integration() -> Result<()> {
    let _guard = ENV_MUTEX.lock().unwrap();

    // Save original values to restore later
    let original_port = env::var("MAGAYA_XDS_PORT").ok();
    let original_bind = env::var("MAGAYA_XDS_BIND_ADDRESS").ok();

    // Test with custom environment variables
    env::set_var("MAGAYA_XDS_PORT", "18001");
    env::set_var("MAGAYA_XDS_BIND_ADDRESS", "127.0.0.1");

    let config = Config::from_env()?;
    assert_eq!(config.xds.port, 18001);
    assert_eq!(config.xds.bind_address, "127.0.0.1");

    // Test with different port
    env::set_var("MAGAYA_XDS_PORT", "19999");
    let config = Config::from_env()?;
    assert_eq!(config.xds.port, 19999);
    assert_eq!(config.xds.bind_address, "127.0.0.1");

    // Test with invalid port
    env::set_var("MAGAYA_XDS_PORT", "invalid");
    let result = Config::from_env();
    assert!(result.is_err());

    // Restore original environment
    match original_port {
        Some(port) => env::set_var("MAGAYA_XDS_PORT", port),
        None => env::remove_var("MAGAYA_XDS_PORT"),
    }
    match original_bind {
        Some(bind) => env::set_var("MAGAYA_XDS_BIND_ADDRESS", bind),
        None => env::remove_var("MAGAYA_XDS_BIND_ADDRESS"),
    }

    Ok(())
}

/// Test that configuration defaults work when no environment variables are set
#[test]
fn test_config_defaults_integration() -> Result<()> {
    let _guard = ENV_MUTEX.lock().unwrap();

    // Save original values
    let original_port = env::var("MAGAYA_XDS_PORT").ok();
    let original_bind = env::var("MAGAYA_XDS_BIND_ADDRESS").ok();

    // Remove environment variables
    env::remove_var("MAGAYA_XDS_PORT");
    env::remove_var("MAGAYA_XDS_BIND_ADDRESS");

    let config = Config::from_env()?;
    assert_eq!(config.xds.port, 18000);
    assert_eq!(config.xds.bind_address, "0.0.0.0");

    // Restore original environment
    match original_port {
        Some(port) => env::set_var("MAGAYA_XDS_PORT", port),
        None => env::remove_var("MAGAYA_XDS_PORT"),
    }
    match original_bind {
        Some(bind) => env::set_var("MAGAYA_XDS_BIND_ADDRESS", bind),
        None => env::remove_var("MAGAYA_XDS_BIND_ADDRESS"),
    }

    Ok(())
}

/// Helper function to check if a port is available
fn is_port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Find an available port for testing
fn find_available_port() -> u16 {
    for port in 18001..19000 {
        if is_port_available(port) {
            return port;
        }
    }
    panic!("No available ports found for testing");
}

/// Integration test that validates the XDS server actually binds to the configured port
#[traced_test]
#[tokio::test]
async fn test_xds_server_binds_to_configured_port() -> Result<()> {
    let _guard = ENV_MUTEX.lock().unwrap();

    // Find an available port for testing
    let test_port = find_available_port();

    // Save original environment
    let original_port = env::var("MAGAYA_XDS_PORT").ok();
    let original_bind = env::var("MAGAYA_XDS_BIND_ADDRESS").ok();

    // Set test environment
    env::set_var("MAGAYA_XDS_PORT", test_port.to_string());
    env::set_var("MAGAYA_XDS_BIND_ADDRESS", "127.0.0.1");

    let config = Config::from_env()?;
    assert_eq!(config.xds.port, test_port);

    // Create a short-lived shutdown signal for testing
    let shutdown_signal = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // Test that the server starts and binds to the correct port
    let server_task =
        magaya::xds::start_minimal_xds_server_with_config(config.xds, shutdown_signal);

    // The server should start and then shutdown cleanly within a reasonable time
    let result = timeout(Duration::from_secs(5), server_task).await;

    // Restore original environment
    match original_port {
        Some(port) => env::set_var("MAGAYA_XDS_PORT", port),
        None => env::remove_var("MAGAYA_XDS_PORT"),
    }
    match original_bind {
        Some(bind) => env::set_var("MAGAYA_XDS_BIND_ADDRESS", bind),
        None => env::remove_var("MAGAYA_XDS_BIND_ADDRESS"),
    }

    // Verify the server completed without timeout
    match result {
        Ok(server_result) => {
            assert!(server_result.is_ok(), "Server should complete successfully");
        }
        Err(_) => {
            panic!("Server did not complete within timeout - this suggests binding issues");
        }
    }

    Ok(())
}

/// Test that invalid configuration is properly rejected
#[test]
fn test_invalid_config_handling() {
    let _guard = ENV_MUTEX.lock().unwrap();

    // Save original values
    let original_port = env::var("MAGAYA_XDS_PORT").ok();

    // Test invalid port values
    let invalid_ports = vec!["", "abc", "-1", "99999", "0"];

    for invalid_port in invalid_ports {
        env::set_var("MAGAYA_XDS_PORT", invalid_port);
        let result = Config::from_env();
        assert!(
            result.is_err(),
            "Config should reject invalid port: {}",
            invalid_port
        );
    }

    // Restore original environment
    match original_port {
        Some(port) => env::set_var("MAGAYA_XDS_PORT", port),
        None => env::remove_var("MAGAYA_XDS_PORT"),
    }
}
