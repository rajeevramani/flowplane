use std::net::SocketAddr;
use std::sync::Arc;

use axum::{serve::Listener, Router};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Duration;
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tracing::{error, info, warn};

use crate::{
    auth::zitadel::ZitadelAuthState,
    config::{ApiServerConfig, ApiTlsConfig, AuthMode},
    domain::filter_schema::{create_shared_registry, create_shared_registry_from_dir},
    errors::Error,
    utils::certificates::{load_certificate_bundle, CertificateInfo},
    xds::XdsState,
};

#[cfg(feature = "dev-oidc")]
use crate::auth::zitadel::{JwksCache, ZitadelConfig};

use super::routes::build_router_with_registry;

/// Handle returned from dev-mode mock OIDC startup.
///
/// Must outlive the API server: dropping the `MockOidcServer` aborts the
/// background task that serves JWKS / userinfo, which would break the
/// Zitadel middleware mid-flight.
#[cfg(feature = "dev-oidc")]
struct DevAuthBundle {
    auth_state: ZitadelAuthState,
    _mock: crate::dev::oidc_server::MockOidcServer,
}

#[cfg(feature = "dev-oidc")]
async fn start_dev_mock_oidc(state: &Arc<XdsState>) -> crate::Result<DevAuthBundle> {
    use crate::dev::oidc_server::{MockOidcConfig, MockOidcServer};
    let mock = MockOidcServer::start(MockOidcConfig::default())
        .await
        .map_err(|e| Error::config(format!("failed to start dev mock OIDC server: {e}")))?;
    info!(issuer = %mock.issuer, "Dev mode: mock OIDC server started");

    let zitadel_config = ZitadelConfig::from_mock(&mock);
    let pool =
        state.pool.clone().ok_or_else(|| Error::config("DB pool required for dev auth state"))?;
    let auth_rate_limiter = Arc::new(crate::api::rate_limit::RateLimiter::auth_from_env());
    let permission_cache = Arc::new(crate::auth::cache::PermissionCache::from_env());
    let auth_state = ZitadelAuthState {
        jwks_cache: JwksCache::new(&zitadel_config),
        config: Arc::new(zitadel_config),
        pool,
        permission_cache,
        auth_rate_limiter,
    };

    if let Ok(path) = std::env::var("FLOWPLANE_CREDENTIALS_PATH") {
        match mock.issue_token().await {
            Ok(token) => {
                if let Err(e) = crate::auth::dev_token::write_credentials_to_path(
                    &token,
                    std::path::Path::new(&path),
                ) {
                    warn!(error = %e, path = %path, "failed to write dev credentials file");
                } else {
                    info!(path = %path, "Dev credentials written");
                }
            }
            Err(e) => warn!(error = %e, "failed to mint dev token from mock OIDC"),
        }
    }

    Ok(DevAuthBundle { auth_state, _mock: mock })
}

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

    let auth_mode = crate::config::AuthMode::from_env()
        .map_err(|e| Error::config(format!("invalid FLOWPLANE_AUTH_MODE: {e}")))?;

    #[cfg(feature = "dev-oidc")]
    let dev_bundle =
        if auth_mode == AuthMode::Dev { Some(start_dev_mock_oidc(&state).await?) } else { None };

    #[cfg(not(feature = "dev-oidc"))]
    {
        if auth_mode == AuthMode::Dev {
            return Err(Error::config(
                "FLOWPLANE_AUTH_MODE=dev requires a build with the `dev-oidc` cargo feature enabled",
            ));
        }
    }

    #[cfg(feature = "dev-oidc")]
    let zitadel_override = dev_bundle.as_ref().map(|b| b.auth_state.clone());
    #[cfg(not(feature = "dev-oidc"))]
    let zitadel_override: Option<ZitadelAuthState> = None;

    let router: Router =
        build_router_with_registry(state, Some(filter_schema_registry), zitadel_override);

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
