use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::info;

use flowplane::config::{ApiServerConfig, DatabaseConfig, SimpleXdsConfig, XdsResourceConfig};
use flowplane::openapi::defaults::ensure_default_gateway_resources;
use flowplane::storage::create_pool;
use flowplane::xds::{start_database_xds_server_with_state, XdsState};

#[derive(Debug)]
pub struct ControlPlaneHandle {
    api_handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    xds_shutdown: Option<oneshot::Sender<()>>,
    pub api_addr: SocketAddr,
    #[allow(dead_code)]
    pub xds_addr: SocketAddr,
    #[allow(dead_code)]
    pub db_path: PathBuf,
}

#[allow(dead_code)]
impl ControlPlaneHandle {
    pub async fn start(
        db_path: PathBuf,
        api_addr: SocketAddr,
        xds_addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        // Configure database from provided sqlite path
        let db_url = format!("sqlite://{}", db_path.display());
        std::env::set_var("DATABASE_URL", &db_url);

        // Create DB pool and shared state
        let pool = create_pool(&DatabaseConfig::from_env()).await?;

        let simple_config = SimpleXdsConfig {
            bind_address: xds_addr.ip().to_string(),
            port: xds_addr.port(),
            resources: XdsResourceConfig { listener_port: 10000, ..Default::default() },
            tls: None,
        };

        let state = Arc::new(XdsState::with_database(simple_config, pool));
        ensure_default_gateway_resources(&state).await?;

        // Start xDS server with oneshot shutdown
        let (sd_tx, sd_rx) = oneshot::channel::<()>();
        let xds_state = state.clone();
        let xds_addr_clone = xds_addr;
        tokio::spawn(async move {
            info!(addr = %xds_addr_clone, "Starting xDS server for test");
            let shutdown = async move {
                let _ = sd_rx.await;
            };
            let _ = start_database_xds_server_with_state(xds_state, shutdown).await;
        });

        // Start API server in a task; abort on drop
        let api_cfg = ApiServerConfig {
            bind_address: api_addr.ip().to_string(),
            port: api_addr.port(),
            tls: None,
        };
        let api_state = state.clone();
        let api_handle = tokio::spawn(async move {
            flowplane::api::start_api_server(api_cfg, api_state).await.map_err(|e| e.into())
        });

        Ok(Self { api_handle, xds_shutdown: Some(sd_tx), api_addr, xds_addr, db_path })
    }

    #[allow(dead_code)]
    pub async fn start_with_xds_tls(
        db_path: PathBuf,
        api_addr: SocketAddr,
        xds_addr: SocketAddr,
        xds_tls: Option<flowplane::config::XdsTlsConfig>,
    ) -> anyhow::Result<Self> {
        let db_url = format!("sqlite://{}", db_path.display());
        std::env::set_var("DATABASE_URL", &db_url);

        let pool = create_pool(&DatabaseConfig::from_env()).await?;

        let simple_config = SimpleXdsConfig {
            bind_address: xds_addr.ip().to_string(),
            port: xds_addr.port(),
            resources: XdsResourceConfig { listener_port: 10000, ..Default::default() },
            tls: xds_tls,
        };

        let state = Arc::new(XdsState::with_database(simple_config, pool));
        ensure_default_gateway_resources(&state).await?;

        let (sd_tx, sd_rx) = oneshot::channel::<()>();
        let xds_state = state.clone();
        let xds_addr_clone = xds_addr;
        tokio::spawn(async move {
            info!(addr = %xds_addr_clone, "Starting xDS server for test (TLS capable)");
            let shutdown = async move {
                let _ = sd_rx.await;
            };
            let _ = start_database_xds_server_with_state(xds_state, shutdown).await;
        });

        let api_cfg = ApiServerConfig {
            bind_address: api_addr.ip().to_string(),
            port: api_addr.port(),
            tls: None,
        };
        let api_state = state.clone();
        let api_handle = tokio::spawn(async move {
            flowplane::api::start_api_server(api_cfg, api_state).await.map_err(|e| e.into())
        });

        Ok(Self { api_handle, xds_shutdown: Some(sd_tx), api_addr, xds_addr, db_path })
    }

    #[allow(dead_code)]
    pub async fn wait_until_ready(&self) {
        // Basic TCP readiness: wait for API port to accept connections
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(self.api_addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

impl Drop for ControlPlaneHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.xds_shutdown.take() {
            let _ = tx.send(());
        }
        self.api_handle.abort();
        // DB file removal is left to test-specific teardown guard.
    }
}
