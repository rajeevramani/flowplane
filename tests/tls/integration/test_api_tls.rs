use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
    sync::{Arc, Once},
    time::Duration,
};

use anyhow::Context;
use axum::{routing::get, Json, Router};
use flowplane::config::ApiTlsConfig;
use http::StatusCode;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use rustls::pki_types::pem::PemObject;
use serde::Serialize;
use tokio::{
    net::{TcpListener, TcpStream},
    time::{sleep, timeout},
};
use tokio_rustls::TlsAcceptor;

use super::super::support::TestCertificateFiles;

static INIT_CRYPTO: Once = Once::new();

/// Initialize the crypto provider (called once per test run)
fn init_crypto() {
    INIT_CRYPTO.call_once(|| {
        use rustls::crypto::{ring, CryptoProvider};
        if CryptoProvider::get_default().is_none() {
            let _ = ring::default_provider().install_default();
        }
    });
}

/// Allocate a free port by binding to port 0
fn allocate_port() -> anyhow::Result<u16> {
    let listener = StdTcpListener::bind("127.0.0.1:0").context("bind to port 0")?;
    Ok(listener.local_addr().context("get local addr")?.port())
}

/// Wait for a listener to become available with a timeout
async fn wait_for_listener(addr: SocketAddr) -> anyhow::Result<()> {
    timeout(Duration::from_secs(5), async {
        for _ in 0..100 {
            match TcpStream::connect(addr).await {
                Ok(stream) => {
                    drop(stream);
                    return Ok(());
                }
                Err(_) => sleep(Duration::from_millis(50)).await,
            }
        }
        anyhow::bail!("server at {} did not become ready in time", addr)
    })
    .await
    .context("timeout waiting for listener")??;
    Ok(())
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

/// Simple health handler for testing
async fn health_handler() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse { status: "ok".to_string() }))
}

/// Create a minimal test router with just a health endpoint
fn test_router() -> Router {
    Router::new().route("/health", get(health_handler))
}

/// Start a simple HTTP server for testing
async fn start_test_http_server(port: u16) -> anyhow::Result<()> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    axum::serve(listener, test_router()).await?;
    Ok(())
}

/// Start a simple HTTPS server for testing with provided TLS config
async fn start_test_https_server(port: u16, tls_config: &ApiTlsConfig) -> anyhow::Result<()> {
    use flowplane::utils::certificates::load_certificate_bundle;

    let bundle = load_certificate_bundle(
        tls_config.cert_path.as_path(),
        tls_config.key_path.as_path(),
        tls_config.chain_path.as_deref(),
    )?;

    let mut cert_chain = Vec::with_capacity(1 + bundle.intermediates.len());
    cert_chain.push(bundle.leaf.clone());
    cert_chain.extend(bundle.intermediates.clone());

    let provider = rustls::crypto::ring::default_provider();
    let builder = rustls::ServerConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()?;

    let server_config = builder
        .with_no_client_auth()
        .with_single_cert(cert_chain, bundle.private_key.clone_key())?;

    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;

    // Custom TLS listener similar to the real API server
    let tls_listener = TlsListener::new(listener, acceptor);
    axum::serve(tls_listener, test_router()).await?;
    Ok(())
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

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => match self.acceptor.accept(stream).await {
                    Ok(tls_stream) => return (tls_stream, addr),
                    Err(_) => continue,
                },
                Err(_) => {
                    sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[tokio::test]
async fn http_mode_preserved_when_tls_disabled() -> anyhow::Result<()> {
    init_crypto();
    let port = allocate_port()?;

    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let handle = tokio::spawn(start_test_http_server(port));
    wait_for_listener(server_addr).await?;

    let client: Client<HttpConnector, Empty<Bytes>> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let uri = format!("http://127.0.0.1:{port}/health").parse().context("parse URI")?;
    let response = client.get(uri).await.context("http request")?;
    assert_eq!(response.status(), StatusCode::OK);

    handle.abort();
    let _ = handle.await;
    Ok(())
}

#[tokio::test]
async fn https_server_serves_requests() -> anyhow::Result<()> {
    init_crypto();
    let port = allocate_port()?;
    let certs = TestCertificateFiles::localhost(time::Duration::days(60))?;
    let tls = ApiTlsConfig {
        cert_path: certs.cert_path.clone(),
        key_path: certs.key_path.clone(),
        chain_path: None,
    };

    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let tls_clone = tls.clone();
    let handle = tokio::spawn(async move { start_test_https_server(port, &tls_clone).await });
    wait_for_listener(server_addr).await?;

    let cert_pem = std::fs::read(&certs.cert_path).context("read certificate")?;
    let mut roots = rustls::RootCertStore::empty();
    let mut cert_iter = rustls::pki_types::CertificateDer::pem_slice_iter(&cert_pem);
    let leaf = cert_iter
        .next()
        .ok_or_else(|| anyhow::anyhow!("cert not present"))?
        .context("invalid cert")?;
    roots.add(leaf).context("add certificate to root store")?;

    let client_tls =
        rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(client_tls)
        .https_only()
        .enable_http1()
        .build();

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(https);
    let uri = format!("https://localhost:{port}/health").parse().context("parse URI")?;
    let response = timeout(Duration::from_secs(5), client.get(uri))
        .await
        .context("https request timeout")?
        .context("https request")?;
    assert_eq!(response.status(), StatusCode::OK);

    handle.abort();
    let _ = handle.await;
    Ok(())
}

#[tokio::test]
async fn https_startup_fails_with_mismatched_key() -> anyhow::Result<()> {
    init_crypto();
    let port = allocate_port()?;
    let certs = TestCertificateFiles::localhost(time::Duration::days(60))?;
    let mismatched_key = certs.mismatched_key()?;
    let tls = ApiTlsConfig {
        cert_path: certs.cert_path.clone(),
        key_path: mismatched_key,
        chain_path: None,
    };

    let result = timeout(Duration::from_secs(5), start_test_https_server(port, &tls))
        .await
        .context("TLS startup timeout")?;

    let err = result.expect_err("TLS mismatch should fail startup");
    let message = format!("{err}");
    assert!(
        message.contains("do not match") || message.contains("invalid"),
        "Expected error about mismatched keys, got: {message}"
    );
    Ok(())
}
