//! xDS gRPC server assembly. Production is [`serve_mtls`]: client certificates are
//! mandatory, verified against the configured CA, and bound to the certificate registry
//! by their SPIFFE URI. [`serve_plaintext`] exists for tests and dev mode only.

use crate::ads::{AdsService, TeamResolver};
use crate::capture::LearningCaptureService;
use crate::diagnostics::DiagnosticsService;
use crate::snapshot::SnapshotCache;
use fp_domain::{DomainError, DomainResult};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// SPIFFE URI SAN of the connecting peer — test-only injection point; the real path
/// extracts it from the TLS-verified client certificate.
#[derive(Clone)]
pub struct PeerSpiffe(pub String);

/// Extract the first `spiffe://` URI SAN from a DER-encoded certificate. Returns `None`
/// for unparseable certs or certs without one — the resolver then rejects the stream.
pub fn spiffe_uri_from_der(der: &[u8]) -> Option<String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    for ext in cert.extensions() {
        if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
            ext.parsed_extension()
        {
            for name in &san.general_names {
                if let x509_parser::extensions::GeneralName::URI(uri) = name {
                    if uri.starts_with("spiffe://") {
                        return Some((*uri).to_string());
                    }
                }
            }
        }
    }
    None
}

/// TLS material for the mandatory-mTLS xDS listener.
#[derive(Debug, Clone)]
pub struct XdsTlsPaths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// CA bundle that client (dataplane) certificates must chain to.
    pub client_ca_path: PathBuf,
}

fn read_pem(path: &std::path::Path, what: &str) -> DomainResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        DomainError::internal(format!("cannot read xDS {what} at {}: {e}", path.display()))
            .with_hint("check the FLOWPLANE_XDS_TLS_* paths and file permissions")
    })
}

/// Serve ADS over mandatory mTLS until `shutdown` resolves. Client certificates are
/// required at the TLS layer; tenant binding happens per stream via the resolver
/// (certificate registry). `revocations` carries revoked certificate ids from the outbox
/// consumer — matching live streams are terminated.
pub async fn serve_mtls(
    addr: SocketAddr,
    cache: Arc<SnapshotCache>,
    resolver: Arc<dyn TeamResolver>,
    revocations: tokio::sync::broadcast::Sender<Uuid>,
    nack_pool: sqlx::PgPool,
    tls: &XdsTlsPaths,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> DomainResult<()> {
    let identity = tonic::transport::Identity::from_pem(
        read_pem(&tls.cert_path, "server certificate")?,
        read_pem(&tls.key_path, "server key")?,
    );
    let client_ca =
        tonic::transport::Certificate::from_pem(read_pem(&tls.client_ca_path, "client CA")?);
    let tls_config = tonic::transport::ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(client_ca);

    let service = AdsService::new(
        cache,
        resolver.clone(),
        revocations,
        Some(nack_pool.clone()),
    )
    .into_server();
    let diagnostics = DiagnosticsService::new(resolver, nack_pool.clone()).into_server();
    let capture = LearningCaptureService::new(nack_pool.clone());
    tracing::info!(%addr, "xDS ADS server starting (mTLS, certificate-registry binding)");
    tonic::transport::Server::builder()
        .tls_config(tls_config)
        .map_err(|e| DomainError::internal(format!("xds tls config: {e}")))?
        .add_service(service)
        .add_service(diagnostics)
        .add_service(capture.clone().access_log_server())
        .add_service(capture.ext_proc_server())
        .serve_with_shutdown(addr, shutdown)
        .await
        .map_err(|e| DomainError::internal(format!("xds server: {e}")))
}

/// Serve ADS on `addr` until `shutdown` resolves. Plaintext — dev/test wiring only;
/// production uses [`serve_mtls`] and refuses to start without TLS material. NACKs are
/// persisted when a pool is supplied.
pub async fn serve_plaintext(
    addr: SocketAddr,
    cache: Arc<SnapshotCache>,
    resolver: Arc<dyn TeamResolver>,
    nack_pool: Option<sqlx::PgPool>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> DomainResult<()> {
    // The bus sender lives inside AdsService for the server's lifetime; plaintext mode has
    // no cert-bound streams so nothing ever publishes on it.
    let (revocations, _) = tokio::sync::broadcast::channel(16);
    let diagnostics = nack_pool
        .clone()
        .map(|pool| DiagnosticsService::new(resolver.clone(), pool).into_server());
    let capture = nack_pool.clone().map(LearningCaptureService::new);
    let service = AdsService::new(cache, resolver, revocations, nack_pool).into_server();
    tracing::info!(%addr, "xDS ADS server starting (plaintext dev mode)");
    let builder = tonic::transport::Server::builder().add_service(service);
    match (diagnostics, capture) {
        (Some(diagnostics), Some(capture)) => {
            builder
                .add_service(diagnostics)
                .add_service(capture.clone().access_log_server())
                .add_service(capture.ext_proc_server())
                .serve_with_shutdown(addr, shutdown)
                .await
        }
        (Some(diagnostics), None) => {
            builder
                .add_service(diagnostics)
                .serve_with_shutdown(addr, shutdown)
                .await
        }
        (None, Some(capture)) => {
            builder
                .add_service(capture.clone().access_log_server())
                .add_service(capture.ext_proc_server())
                .serve_with_shutdown(addr, shutdown)
                .await
        }
        (None, None) => builder.serve_with_shutdown(addr, shutdown).await,
    }
    .map_err(|e| DomainError::internal(format!("xds server: {e}")))
}
