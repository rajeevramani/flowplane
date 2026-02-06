//! Control Plane lifecycle management for E2E tests
//!
//! Manages startup and shutdown of the flowplane control plane (API + xDS servers)
//! with proper timeout handling and cleanup.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, watch};
use tracing::{error, info};

use flowplane::config::{ApiServerConfig, DatabaseConfig, SimpleXdsConfig, XdsResourceConfig};
use flowplane::openapi::defaults::ensure_default_gateway_resources;
use flowplane::secrets::SecretBackendRegistry;
use flowplane::storage::{create_pool, run_migrations};
use flowplane::xds::{start_database_xds_server_with_state, XdsState};

use super::timeout::{retry_with_timeout, STARTUP_TIMEOUT};

/// Control plane configuration
#[derive(Debug, Clone)]
pub struct ControlPlaneConfig {
    /// PostgreSQL database URL
    pub db_url: String,
    /// API server bind address
    pub api_addr: SocketAddr,
    /// xDS server bind address
    pub xds_addr: SocketAddr,
    /// Default listener port for xDS resources
    pub default_listener_port: u16,
    /// TLS configuration for xDS server (optional)
    pub xds_tls: Option<flowplane::config::XdsTlsConfig>,
}

impl ControlPlaneConfig {
    /// Create a new config with the given ports
    pub fn new(db_url: String, api_port: u16, xds_port: u16, default_listener_port: u16) -> Self {
        Self {
            db_url,
            api_addr: SocketAddr::from(([127, 0, 0, 1], api_port)),
            xds_addr: SocketAddr::from(([127, 0, 0, 1], xds_port)),
            default_listener_port,
            xds_tls: None,
        }
    }

    /// Add mTLS configuration for xDS server
    pub fn with_xds_tls(mut self, tls: flowplane::config::XdsTlsConfig) -> Self {
        self.xds_tls = Some(tls);
        self
    }
}

/// Handle to a running control plane instance
pub struct ControlPlaneHandle {
    api_handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    xds_shutdown: Option<oneshot::Sender<()>>,
    api_error_rx: watch::Receiver<Option<String>>,
    /// API server address
    pub api_addr: SocketAddr,
    /// xDS server address
    pub xds_addr: SocketAddr,
    /// Database URL (for verification/debugging)
    pub db_url: String,
}

impl std::fmt::Debug for ControlPlaneHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlPlaneHandle")
            .field("api_addr", &self.api_addr)
            .field("xds_addr", &self.xds_addr)
            .field("db_url", &self.db_url)
            .finish()
    }
}

impl ControlPlaneHandle {
    /// Start a new control plane instance
    pub async fn start(config: ControlPlaneConfig) -> anyhow::Result<Self> {
        info!(
            api_addr = %config.api_addr,
            xds_addr = %config.xds_addr,
            db_url = %config.db_url,
            "Starting control plane for E2E test"
        );

        // Create DB pool from provided PostgreSQL URL
        let db_config = DatabaseConfig {
            url: config.db_url.clone(),
            auto_migrate: true,
            max_connections: 10,
            min_connections: 1,
            ..Default::default()
        };

        info!("Creating database pool and running migrations...");
        let pool = create_pool(&db_config).await?;
        run_migrations(&pool).await?;
        info!("Database migrations completed");

        let simple_config = SimpleXdsConfig {
            bind_address: config.xds_addr.ip().to_string(),
            port: config.xds_addr.port(),
            resources: XdsResourceConfig {
                listener_port: config.default_listener_port,
                ..Default::default()
            },
            tls: config.xds_tls,
            envoy_admin: Default::default(),
        };

        // Create state without Arc first so we can initialize the secret backend registry
        let mut state_struct = XdsState::with_database(simple_config, pool.clone());

        // Enable mock certificate backend for E2E tests
        // This allows testing the certificate API without requiring Vault
        std::env::set_var("FLOWPLANE_USE_MOCK_CERT_BACKEND", "1");

        // Initialize secret backend registry with mock certificate backend
        // Note: encryption service may be None in test environment, but we can
        // still create the registry for certificate backend functionality
        let encryption = state_struct.encryption_service.clone();
        match SecretBackendRegistry::from_env(pool.clone(), encryption, None).await {
            Ok(registry) => {
                info!(
                    backends = ?registry.registered_backends(),
                    has_cert_backend = registry.has_certificate_backend(),
                    cert_backend_type = ?registry.certificate_backend_type(),
                    "Initialized secret backend registry for E2E tests"
                );
                state_struct.set_secret_backend_registry(registry);
            }
            Err(e) => {
                error!(error = %e, "Failed to initialize secret backend registry");
            }
        }

        let state = Arc::new(state_struct);
        ensure_default_gateway_resources(&state).await?;
        info!("Default gateway resources created");

        // Start xDS server with oneshot shutdown
        let (sd_tx, sd_rx) = oneshot::channel::<()>();
        let xds_state = state.clone();
        let xds_addr = config.xds_addr;
        tokio::spawn(async move {
            info!(addr = %xds_addr, "Starting xDS server for E2E test");
            let shutdown = async move {
                let _ = sd_rx.await;
            };
            if let Err(e) = start_database_xds_server_with_state(xds_state, shutdown).await {
                error!(error = %e, "xDS server failed");
            }
        });

        // Create a channel to track API server errors
        let (api_error_tx, api_error_rx) = watch::channel::<Option<String>>(None);

        // Start API server in a task
        let api_cfg = ApiServerConfig {
            bind_address: config.api_addr.ip().to_string(),
            port: config.api_addr.port(),
            tls: None,
        };
        let api_state = state.clone();
        let api_addr_clone = config.api_addr;
        let api_handle = tokio::spawn(async move {
            info!(addr = %api_addr_clone, "Starting API server for E2E test");
            let result = flowplane::api::start_api_server(api_cfg, api_state).await;
            if let Err(ref e) = result {
                error!(error = %e, "API server failed");
                let _ = api_error_tx.send(Some(e.to_string()));
            }
            result.map_err(|e| e.into())
        });

        // Give servers a moment to bind
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Check if API server failed immediately
        if api_handle.is_finished() {
            let result = api_handle.await;
            match result {
                Ok(Ok(())) => {
                    anyhow::bail!("API server exited unexpectedly")
                }
                Ok(Err(e)) => {
                    anyhow::bail!("API server failed to start: {}", e)
                }
                Err(e) => {
                    anyhow::bail!("API server task panicked: {}", e)
                }
            }
        }

        Ok(Self {
            api_handle,
            xds_shutdown: Some(sd_tx),
            api_error_rx,
            api_addr: config.api_addr,
            xds_addr: config.xds_addr,
            db_url: config.db_url,
        })
    }

    /// Wait for the control plane to be ready with timeout
    pub async fn wait_ready(&self) -> anyhow::Result<()> {
        // Check if API server has already failed
        if let Some(err) = self.api_error_rx.borrow().as_ref() {
            anyhow::bail!("API server failed during startup: {}", err);
        }

        let api_addr = self.api_addr;
        let error_rx = self.api_error_rx.clone();

        // Wait for API server to be ready, but also check for errors
        let api_ready = retry_with_timeout(
            STARTUP_TIMEOUT,
            Duration::from_millis(100),
            "CP API server ready",
            move || {
                let error_rx = error_rx.clone();
                async move {
                    // Check for startup errors
                    if let Some(err) = error_rx.borrow().as_ref() {
                        return Err(format!("API server failed: {}", err));
                    }
                    tokio::net::TcpStream::connect(api_addr)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("Connection failed: {}", e))
                }
            },
        )
        .await;

        // Check again for any errors that occurred during wait
        if let Some(err) = self.api_error_rx.borrow().as_ref() {
            anyhow::bail!("API server failed during startup: {}", err);
        }

        api_ready?;

        // Also verify xDS port is listening
        let xds_addr = self.xds_addr;
        retry_with_timeout(
            Duration::from_secs(10),
            Duration::from_millis(100),
            "CP xDS server ready",
            move || async move {
                tokio::net::TcpStream::connect(xds_addr)
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("xDS connection failed: {}", e))
            },
        )
        .await?;

        info!(api = %self.api_addr, xds = %self.xds_addr, "Control plane ready");
        Ok(())
    }

    /// Get the API base URL
    pub fn api_url(&self) -> String {
        format!("http://{}", self.api_addr)
    }

    /// Graceful shutdown of the control plane
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.xds_shutdown.take() {
            let _ = tx.send(());
        }
        self.api_handle.abort();
        // Give components time to clean up
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

impl Drop for ControlPlaneHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.xds_shutdown.take() {
            let _ = tx.send(());
        }
        self.api_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_control_plane_config() {
        let db_url = "postgresql://postgres:postgres@localhost:5432/test".to_string();
        let config = ControlPlaneConfig::new(db_url, 9000, 9001, 10000);

        assert_eq!(config.api_addr.port(), 9000);
        assert_eq!(config.xds_addr.port(), 9001);
        assert_eq!(config.default_listener_port, 10000);
        assert!(config.xds_tls.is_none());
    }
}
