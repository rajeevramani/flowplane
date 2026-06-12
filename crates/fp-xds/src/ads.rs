//! ADS (SOTW) server: one bidirectional stream multiplexing CDS/RDS/LDS per dataplane
//! (spec/10 §5). Responses come from the snapshot cache (no per-request DB reads); pushes
//! follow make-before-break type ordering: clusters → routes → listeners.
//!
//! Team identity: S5.3 resolves the team via [`TeamResolver`]; the mTLS cert-registry
//! resolver lands in S5.4 — node-id resolution is for tests and dev mode ONLY.

use crate::snapshot::{SnapshotCache, CLUSTER_TYPE_URL, LISTENER_TYPE_URL, ROUTE_TYPE_URL};
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

/// Make-before-break push order (deletes are handled by SOTW full-set semantics).
const TYPE_ORDER: [&str; 3] = [CLUSTER_TYPE_URL, ROUTE_TYPE_URL, LISTENER_TYPE_URL];

/// Resolves the tenant a connecting dataplane belongs to.
pub trait TeamResolver: Send + Sync + 'static {
    /// `node_id` is Envoy's node.id; `peer_spiffe` is the SPIFFE URI SAN from the client
    /// cert when mTLS is configured (S5.4).
    fn resolve(&self, node_id: &str, peer_spiffe: Option<&str>) -> Result<TeamId, Status>;
}

/// Dev/test resolver: trusts `team=<uuid>` in the node id. NEVER for production — the
/// mTLS cert-registry resolver replaces it in S5.4.
pub struct NodeIdTeamResolver;

impl TeamResolver for NodeIdTeamResolver {
    fn resolve(&self, node_id: &str, _peer_spiffe: Option<&str>) -> Result<TeamId, Status> {
        let team = node_id
            .split('/')
            .find_map(|part| part.strip_prefix("team="))
            .ok_or_else(|| Status::unauthenticated("node.id must carry team=<uuid>"))?;
        TeamId::from_str(team)
            .map_err(|_| Status::unauthenticated("node.id team segment is not a UUID"))
    }
}

pub struct AdsService {
    cache: Arc<SnapshotCache>,
    resolver: Arc<dyn TeamResolver>,
}

impl AdsService {
    pub fn new(cache: Arc<SnapshotCache>, resolver: Arc<dyn TeamResolver>) -> Self {
        Self { cache, resolver }
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
        let peer_spiffe = request
            .extensions()
            .get::<crate::server::PeerSpiffe>()
            .map(|p| p.0.clone());
        let mut inbound = request.into_inner();
        let cache = self.cache.clone();
        let resolver = self.resolver.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<DiscoveryResponse, Status>>(32);

        tokio::spawn(async move {
            let mut team: Option<TeamId> = None;
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
                            match resolver.resolve(node_id, peer_spiffe.as_deref()) {
                                Ok(resolved) => {
                                    tracing::info!(team = %resolved, node = node_id,
                                        "dataplane connected");
                                    team = Some(resolved);
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

                        // ACK/NACK: a request echoing our last nonce is a response to our
                        // last push, not a new subscription.
                        if !request.response_nonce.is_empty()
                            && request.response_nonce == state.last_nonce
                        {
                            if let Some(error) = &request.error_detail {
                                metrics::counter!("fp_xds_nacks_total").increment(1);
                                tracing::warn!(team = %team_id, type_url,
                                    error = %error.message, "xDS NACK");
                                // S5.5: quarantine + persistence. For now: keep serving the
                                // last-known config; do not re-send unchanged bytes.
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
