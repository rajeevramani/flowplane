//! ADS (SOTW) server: one bidirectional stream multiplexing CDS/RDS/LDS per dataplane
//! (spec/10 §5). Responses come from the snapshot cache (no per-request DB reads); pushes
//! follow make-before-break type ordering: clusters → routes → listeners.
//!
//! Team identity: production resolution is the mTLS certificate registry
//! ([`CertRegistryResolver`]) — the client cert's SPIFFE URI is looked up as a whole and
//! the matched row's team is authoritative (SAN segments and node ids are never trusted,
//! spec/04 §1.3). Node-id resolution is for tests and dev mode ONLY.

use crate::snapshot::{
    SnapshotCache, CLUSTER_TYPE_URL, ENDPOINT_TYPE_URL, LISTENER_TYPE_URL, ROUTE_TYPE_URL,
};
use envoy_types::pb::envoy::service::discovery::v3::aggregated_discovery_service_server::{
    AggregatedDiscoveryService, AggregatedDiscoveryServiceServer,
};
use envoy_types::pb::envoy::service::discovery::v3::{
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};
use fp_domain::TeamId;
use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

/// Make-before-break push order (deletes are handled by SOTW full-set semantics):
/// clusters warm before their endpoints arrive, routes before the listeners that bind them.
const TYPE_ORDER: [&str; 4] = [
    CLUSTER_TYPE_URL,
    ENDPOINT_TYPE_URL,
    ROUTE_TYPE_URL,
    LISTENER_TYPE_URL,
];

/// The authenticated identity of a connected dataplane.
#[derive(Debug, Clone, Copy)]
pub struct PeerIdentity {
    pub team_id: TeamId,
    /// The certificate-registry row backing this stream. Revocation of this id terminates
    /// the stream. `None` only under the dev node-id resolver.
    pub certificate_id: Option<Uuid>,
}

/// Resolves the tenant a connecting dataplane belongs to.
#[tonic::async_trait]
pub trait TeamResolver: Send + Sync + 'static {
    /// `node_id` is Envoy's node.id (attribution only); `peer_spiffe` is the SPIFFE URI
    /// SAN extracted from the verified client certificate when mTLS is configured.
    async fn resolve(
        &self,
        node_id: &str,
        peer_spiffe: Option<&str>,
    ) -> Result<PeerIdentity, Status>;
}

/// Dev/test resolver: trusts `team=<uuid>` in the node id. NEVER for production.
pub struct NodeIdTeamResolver;

#[tonic::async_trait]
impl TeamResolver for NodeIdTeamResolver {
    async fn resolve(
        &self,
        node_id: &str,
        _peer_spiffe: Option<&str>,
    ) -> Result<PeerIdentity, Status> {
        let team = node_id
            .split('/')
            .find_map(|part| part.strip_prefix("team="))
            .ok_or_else(|| Status::unauthenticated("node.id must carry team=<uuid>"))?;
        let team_id = TeamId::from_str(team)
            .map_err(|_| Status::unauthenticated("node.id team segment is not a UUID"))?;
        Ok(PeerIdentity {
            team_id,
            certificate_id: None,
        })
    }
}

/// Production resolver: the full SPIFFE URI keys a registry row that must be unrevoked
/// and unexpired; the row's team is the stream's tenant. Every failure mode — no cert, no
/// row, revoked, expired, registry unreachable — fails closed with one indistinct message
/// (no oracle for which condition failed).
pub struct CertRegistryResolver {
    pool: sqlx::PgPool,
}

impl CertRegistryResolver {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[tonic::async_trait]
impl TeamResolver for CertRegistryResolver {
    async fn resolve(
        &self,
        node_id: &str,
        peer_spiffe: Option<&str>,
    ) -> Result<PeerIdentity, Status> {
        let Some(uri) = peer_spiffe else {
            return Err(Status::unauthenticated(
                "a client certificate with a SPIFFE URI SAN is required for xDS",
            ));
        };
        match fp_storage::repos::dataplanes::find_active_certificate(&self.pool, uri).await {
            Ok(Some(cert)) => {
                tracing::info!(team = %cert.team_id, node = node_id,
                    serial = %cert.serial_number, "dataplane authenticated via certificate registry");
                Ok(PeerIdentity {
                    team_id: cert.team_id,
                    certificate_id: Some(cert.id.as_uuid()),
                })
            }
            Ok(None) => Err(Status::unauthenticated(
                "certificate is not registered, is revoked, or has expired",
            )),
            Err(e) => {
                // Fail closed: a registry outage authenticates nobody.
                tracing::error!("certificate registry lookup failed: {e}");
                Err(Status::unauthenticated("certificate registry unavailable"))
            }
        }
    }
}

/// Forward certificate revocations from an outbox batch onto the stream-kill bus. Wired
/// next to the snapshot handler in the xDS outbox consumer.
pub fn publish_revocations(
    revocations: &tokio::sync::broadcast::Sender<Uuid>,
    events: &[fp_storage::outbox::StoredEvent],
) {
    for stored in events {
        if let fp_domain::event::DomainEvent::ProxyCertificateRevoked { certificate_id, .. } =
            &stored.event
        {
            // Send fails only when no stream is subscribed — nothing to kill.
            let _ = revocations.send(*certificate_id);
        }
    }
}

pub struct AdsService {
    cache: Arc<SnapshotCache>,
    resolver: Arc<dyn TeamResolver>,
    /// Broadcasts revoked certificate ids; streams bound to a revoked cert terminate.
    revocations: tokio::sync::broadcast::Sender<Uuid>,
    /// NACK persistence (S5.5); `None` keeps NACK handling in-memory only (tests).
    nack_pool: Option<sqlx::PgPool>,
}

impl AdsService {
    pub fn new(
        cache: Arc<SnapshotCache>,
        resolver: Arc<dyn TeamResolver>,
        revocations: tokio::sync::broadcast::Sender<Uuid>,
        nack_pool: Option<sqlx::PgPool>,
    ) -> Self {
        Self {
            cache,
            resolver,
            revocations,
            nack_pool,
        }
    }

    pub fn into_server(self) -> AggregatedDiscoveryServiceServer<Self> {
        AggregatedDiscoveryServiceServer::new(self)
    }
}

/// Per-stream, per-type subscription state.
#[derive(Default)]
struct TypeState {
    subscribed: bool,
    sent_version: Option<u64>,
    last_nonce: String,
    /// Sorted resource names of the last request — a request with different names is a
    /// subscription change and must be answered, never classified as an ACK (the warming
    /// listener that adds an RDS name echoes the last nonce; spec/04 §2.4).
    resource_names: Vec<String>,
}

fn response_for(
    type_url: &str,
    version: u64,
    resources: Vec<envoy_types::pb::google::protobuf::Any>,
    nonce_seq: &mut u64,
) -> (DiscoveryResponse, String) {
    *nonce_seq += 1;
    let nonce = nonce_seq.to_string();
    (
        DiscoveryResponse {
            version_info: version.to_string(),
            resources,
            type_url: type_url.to_string(),
            nonce: nonce.clone(),
            ..Default::default()
        },
        nonce,
    )
}

#[tonic::async_trait]
impl AggregatedDiscoveryService for AdsService {
    type StreamAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DiscoveryResponse, Status>> + Send>>;
    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send>>;

    async fn stream_aggregated_resources(
        &self,
        request: Request<Streaming<DiscoveryRequest>>,
    ) -> Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        // SPIFFE URI from the TLS-verified client certificate (mTLS path); the extension
        // is a test-only injection fallback.
        let peer_spiffe = request
            .peer_certs()
            .and_then(|certs| {
                certs
                    .first()
                    .and_then(|der| crate::server::spiffe_uri_from_der(der.as_ref()))
            })
            .or_else(|| {
                request
                    .extensions()
                    .get::<crate::server::PeerSpiffe>()
                    .map(|p| p.0.clone())
            });
        let mut inbound = request.into_inner();
        let cache = self.cache.clone();
        let resolver = self.resolver.clone();
        let mut revocations = self.revocations.subscribe();
        let nack_pool = self.nack_pool.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<DiscoveryResponse, Status>>(32);

        tokio::spawn(async move {
            let mut team: Option<TeamId> = None;
            let mut node_label = String::new();
            let mut certificate_id: Option<Uuid> = None;
            let mut states: HashMap<String, TypeState> = HashMap::new();
            let mut nonce_seq: u64 = 0;
            let mut changes = cache.watch();

            loop {
                tokio::select! {
                    message = inbound.next() => {
                        let request = match message {
                            Some(Ok(request)) => request,
                            Some(Err(e)) => {
                                tracing::debug!("ads stream error: {e}");
                                return;
                            }
                            None => return, // client closed
                        };

                        // First request must identify the node; team is fixed for the stream.
                        if team.is_none() {
                            let node_id = request
                                .node
                                .as_ref()
                                .map(|n| n.id.as_str())
                                .unwrap_or_default();
                            match resolver.resolve(node_id, peer_spiffe.as_deref()).await {
                                Ok(identity) => {
                                    tracing::info!(team = %identity.team_id, node = node_id,
                                        "dataplane connected");
                                    team = Some(identity.team_id);
                                    node_label = node_id.to_string();
                                    certificate_id = identity.certificate_id;
                                }
                                Err(status) => {
                                    let _ = tx.send(Err(status)).await;
                                    return;
                                }
                            }
                        }
                        let Some(team_id) = team else { return };
                        let type_url = request.type_url.clone();
                        if !TYPE_ORDER.contains(&type_url.as_str()) {
                            tracing::debug!(type_url, "ignoring unsupported type url");
                            continue;
                        }
                        let state = states.entry(type_url.clone()).or_default();
                        let mut requested_names = request.resource_names.clone();
                        requested_names.sort();
                        let subscription_changed = requested_names != state.resource_names;
                        state.resource_names = requested_names;

                        // ACK/NACK: a request echoing our last nonce WITH an unchanged
                        // subscription is a response to our last push. A changed name set
                        // is a subscription update and always gets a response.
                        if !subscription_changed
                            && !request.response_nonce.is_empty()
                            && request.response_nonce == state.last_nonce
                        {
                            if let Some(error) = &request.error_detail {
                                metrics::counter!("fp_xds_nacks_total").increment(1);
                                tracing::warn!(team = %team_id, type_url,
                                    error = %error.message, "xDS NACK");
                                // S5.5: quarantine what changed (serve last-good bytes) and
                                // persist the event. The cache notification wakes this very
                                // stream to push the corrected set.
                                let quarantined = cache
                                    .apply_nack(team_id, &type_url, &error.message)
                                    .await;
                                if let Some(pool) = &nack_pool {
                                    let record = fp_storage::repos::xds_nacks::NackRecord {
                                        team_id,
                                        node_id: node_label.clone(),
                                        type_url: type_url.clone(),
                                        version_rejected: request.version_info.clone(),
                                        error_message: error.message.clone(),
                                        quarantined_resources: quarantined,
                                    };
                                    let pool = pool.clone();
                                    // Best-effort, off the stream path.
                                    tokio::spawn(async move {
                                        if let Err(e) =
                                            fp_storage::repos::xds_nacks::record(&pool, &record)
                                                .await
                                        {
                                            tracing::error!("failed to persist NACK: {e}");
                                        }
                                    });
                                }
                            }
                            state.subscribed = true;
                            continue;
                        }

                        // New subscription (or re-subscribe): answer immediately.
                        state.subscribed = true;
                        let snapshot = cache.team(team_id).await;
                        if let Some(set) = snapshot.for_type_url(&type_url) {
                            let (response, nonce) = response_for(
                                &type_url, set.version, set.resources.clone(), &mut nonce_seq,
                            );
                            state.sent_version = Some(set.version);
                            state.last_nonce = nonce;
                            if tx.send(Ok(response)).await.is_err() {
                                return;
                            }
                        }
                    }
                    revoked = revocations.recv() => {
                        match revoked {
                            Ok(cert_id) => {
                                if certificate_id == Some(cert_id) {
                                    tracing::warn!(team = ?team, cert = %cert_id,
                                        "terminating xDS stream: certificate revoked");
                                    let _ = tx.send(Err(Status::permission_denied(
                                        "certificate has been revoked",
                                    ))).await;
                                    return;
                                }
                            }
                            // Lagged: we may have missed our own revocation — fail closed
                            // for cert-bound streams; reconnect re-validates at the registry.
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                if certificate_id.is_some() {
                                    let _ = tx.send(Err(Status::unavailable(
                                        "revocation feed lagged; reconnect to re-validate",
                                    ))).await;
                                    return;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                return; // server shutting down
                            }
                        }
                    }
                    changed = changes.changed() => {
                        if changed.is_err() {
                            return; // cache dropped: server shutting down
                        }
                        let Some(team_id) = team else { continue };
                        let (_, changed_team) = *changes.borrow();
                        if changed_team.is_some() && changed_team != Some(team_id) {
                            continue; // another tenant's change
                        }
                        let snapshot = cache.team(team_id).await;
                        // Push changed types in make-before-break order.
                        for type_url in TYPE_ORDER {
                            let Some(state) = states.get_mut(type_url) else { continue };
                            if !state.subscribed {
                                continue;
                            }
                            let Some(set) = snapshot.for_type_url(type_url) else { continue };
                            if state.sent_version == Some(set.version) {
                                continue;
                            }
                            let (response, nonce) = response_for(
                                type_url, set.version, set.resources.clone(), &mut nonce_seq,
                            );
                            state.sent_version = Some(set.version);
                            state.last_nonce = nonce;
                            if tx.send(Ok(response)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn delta_aggregated_resources(
        &self,
        _request: Request<Streaming<DeltaDiscoveryRequest>>,
    ) -> Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        // Honest SOTW-only for v1.0 (spec/10 §5): no fake delta (v1 smell §8.10).
        Err(Status::unimplemented(
            "delta xDS is not supported; configure ADS with state-of-the-world",
        ))
    }
}
