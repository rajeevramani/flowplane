use std::net::SocketAddr;
use std::sync::Arc;

use axum::{serve::Listener, Router};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Duration;
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tracing::{error, info, warn};

use crate::{
    config::{ApiServerConfig, ApiTlsConfig},
    domain::filter_schema::{create_shared_registry, create_shared_registry_from_dir},
    errors::Error,
    utils::certificates::{load_certificate_bundle, CertificateInfo},
    xds::XdsState,
};

use super::routes::build_router_with_registry;

pub async fn start_api_server(config: ApiServerConfig, state: Arc<XdsState>) -> crate::Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.bind_address, config.port)
        .parse()
        .map_err(|e| Error::config(format!("Invalid API address: {}", e)))?;

    // Initialize filter schema registry - try to load from filter-schemas directory first
    let filter_schema_registry = {
        let schema_dir = std::path::Path::new("filter-schemas");
        if schema_dir.exists() {
            match create_shared_registry_from_dir(schema_dir) {
                Ok(registry) => {
                    info!(
                        path = %schema_dir.display(),
                        "Loaded filter schemas from directory"
                    );
                    registry
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to load filter schemas from directory, using built-in only"
                    );
                    create_shared_registry()
                }
            }
        } else {
            info!("No filter-schemas directory found, using built-in schemas only");
            create_shared_registry()
        }
    };
    {
        let registry = filter_schema_registry.read().await;
        info!(filter_count = registry.len(), "Initialized filter schema registry");
    }

    let router: Router = build_router_with_registry(state, Some(filter_schema_registry));

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| Error::transport(format!("Failed to bind API server: {}", e)))?;

    if let Some(tls_config) = config.tls.as_ref() {
        let (acceptor, certificate_info) = configure_tls_acceptor(tls_config)?;
        info!(
            address = %addr,
            subject = %certificate_info.subject,
            expires_at = %certificate_info.not_after,
            "Starting HTTPS API server"
        );
        run_tls_server(listener, acceptor, router).await?;
    } else {
        info!(address = %addr, "Starting HTTP API server");
        run_http_server(listener, router).await?;
    }

    info!("API server shutdown completed");
    Ok(())
}

async fn run_http_server(listener: TcpListener, router: Router) -> crate::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                warn!(error = %e, "API server shutdown listener failed");
            }
        })
        .await
        .map_err(|e| Error::transport(format!("API server error: {}", e)))
}

async fn run_tls_server(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    router: Router,
) -> crate::Result<()> {
    let tls_listener = TlsListener::new(listener, acceptor);
    axum::serve(tls_listener, router)
        .with_graceful_shutdown(async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                warn!(error = %e, "API server shutdown listener failed");
            }
        })
        .await
        .map_err(|e| Error::transport(format!("HTTPS API server error: {}", e)))
}

fn configure_tls_acceptor(tls: &ApiTlsConfig) -> crate::Result<(TlsAcceptor, CertificateInfo)> {
    let bundle = load_certificate_bundle(
        tls.cert_path.as_path(),
        tls.key_path.as_path(),
        tls.chain_path.as_deref(),
    )
    .map_err(|err| Error::config(format!("TLS configuration error: {err}")))?;

    let mut cert_chain = Vec::with_capacity(1 + bundle.intermediates.len());
    cert_chain.push(bundle.leaf.clone());
    cert_chain.extend(bundle.intermediates.clone());

    let provider = rustls::crypto::ring::default_provider();
    let builder = rustls::ServerConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|err| Error::config(format!("Invalid TLS protocol configuration: {err}")))?;

    let server_config = builder
        .with_no_client_auth()
        .with_single_cert(cert_chain, bundle.private_key.clone_key())
        .map_err(|err| Error::config(format!("Failed to load TLS certificate: {err}")))?;

    let info = bundle.info.clone();
    Ok((TlsAcceptor::from(Arc::new(server_config)), info))
}

struct TlsListener {
    listener: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsListener {
    fn new(listener: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self { listener, acceptor }
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => match self.acceptor.accept(stream).await {
                    Ok(tls_stream) => return (tls_stream, addr),
                    Err(err) => {
                        warn!(error = %err, %addr, "TLS handshake failed");
                        continue;
                    }
                },
                Err(err) => {
                    if is_connection_error(&err) {
                        continue;
                    }
                    error!("HTTPS accept error: {err}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

fn is_connection_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}
