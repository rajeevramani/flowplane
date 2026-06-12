//! xDS gRPC server assembly. S5.3: plaintext bind for tests/dev wiring; S5.4 adds the
//! mandatory-mTLS production path with cert-registry team binding.

use crate::ads::{AdsService, TeamResolver};
use crate::snapshot::SnapshotCache;
use fp_domain::{DomainError, DomainResult};
use std::net::SocketAddr;
use std::sync::Arc;

/// SPIFFE URI SAN of the connecting peer, injected by the TLS layer (S5.4).
#[derive(Clone)]
pub struct PeerSpiffe(pub String);

/// Serve ADS on `addr` until `shutdown` resolves. Plaintext — dev/test wiring only;
/// the production entry point (S5.4) enforces mTLS and refuses to start without it.
pub async fn serve_plaintext(
    addr: SocketAddr,
    cache: Arc<SnapshotCache>,
    resolver: Arc<dyn TeamResolver>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> DomainResult<()> {
    let service = AdsService::new(cache, resolver).into_server();
    tracing::info!(%addr, "xDS ADS server starting (plaintext dev mode)");
    tonic::transport::Server::builder()
        .add_service(service)
        .serve_with_shutdown(addr, shutdown)
        .await
        .map_err(|e| DomainError::internal(format!("xds server: {e}")))
}
