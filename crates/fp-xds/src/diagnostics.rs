//! Flowplane diagnostics gRPC surface (spec/04 §5.2). The service is mounted beside ADS
//! so dataplane agents can relay Envoy liveness/config health through the same mTLS
//! identity boundary.

use crate::ads::TeamResolver;
use fp_domain::DataplaneId;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::codegen::*;
use tonic::{Request, Response, Status, Streaming};

pub const SERVICE_NAME: &str = "flowplane.diagnostics.v1.EnvoyDiagnosticsService";

#[derive(Clone, PartialEq, prost::Message)]
pub struct DiagnosticsReport {
    #[prost(uint32, tag = "1")]
    pub schema_version: u32,
    #[prost(string, tag = "2")]
    pub report_id: String,
    #[prost(string, tag = "3")]
    pub dataplane_id: String,
    #[prost(message, optional, tag = "4")]
    pub observed_at: Option<prost_types::Timestamp>,
    #[prost(oneof = "diagnostics_report::Payload", tags = "10, 20")]
    pub payload: Option<diagnostics_report::Payload>,
}

pub mod diagnostics_report {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        ListenerState(super::ListenerStateReport),
        #[prost(message, tag = "20")]
        Heartbeat(super::HeartbeatReport),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ListenerStateReport {
    #[prost(string, tag = "1")]
    pub listener_name: String,
    #[prost(enumeration = "ResourceType", tag = "2")]
    pub resource_type: i32,
    #[prost(string, tag = "3")]
    pub resource_name: String,
    #[prost(string, tag = "4")]
    pub version_info: String,
    #[prost(string, tag = "5")]
    pub error_details: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct HeartbeatReport {
    #[prost(int64, tag = "1")]
    pub requests_delta: i64,
    #[prost(int64, tag = "2")]
    pub errors_delta: i64,
    #[prost(int64, tag = "3")]
    pub warming_failures_delta: i64,
    #[prost(bool, tag = "4")]
    pub config_verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum ResourceType {
    Unspecified = 0,
    Listener = 1,
    Route = 2,
    Cluster = 3,
    Secret = 4,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct DiagnosticsAck {
    #[prost(string, repeated, tag = "1")]
    pub report_ids: Vec<String>,
    #[prost(enumeration = "AckStatus", tag = "2")]
    pub status: i32,
    #[prost(string, tag = "3")]
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum AckStatus {
    Ok = 0,
    Invalid = 1,
    Unauthorized = 2,
}

pub type ResponseStream =
    Pin<Box<dyn Stream<Item = Result<DiagnosticsAck, Status>> + Send + 'static>>;

#[tonic::async_trait]
pub trait EnvoyDiagnosticsService: Send + Sync + 'static {
    type ReportDiagnosticsStream: Stream<Item = Result<DiagnosticsAck, Status>> + Send + 'static;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status>;
}

#[derive(Debug)]
pub struct EnvoyDiagnosticsServiceServer<T> {
    inner: Arc<T>,
}

impl<T> EnvoyDiagnosticsServiceServer<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }
}

impl<T, B> tonic::codegen::Service<http::Request<B>> for EnvoyDiagnosticsServiceServer<T>
where
    T: EnvoyDiagnosticsService,
    B: Body + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        match req.uri().path() {
            "/flowplane.diagnostics.v1.EnvoyDiagnosticsService/ReportDiagnostics" => {
                struct ReportDiagnosticsSvc<T: EnvoyDiagnosticsService>(Arc<T>);

                impl<T: EnvoyDiagnosticsService> tonic::server::StreamingService<DiagnosticsReport>
                    for ReportDiagnosticsSvc<T>
                {
                    type Response = DiagnosticsAck;
                    type ResponseStream = T::ReportDiagnosticsStream;
                    type Future = BoxFuture<tonic::Response<Self::ResponseStream>, tonic::Status>;

                    fn call(
                        &mut self,
                        request: tonic::Request<tonic::Streaming<DiagnosticsReport>>,
                    ) -> Self::Future {
                        let inner = Arc::clone(&self.0);
                        Box::pin(async move {
                            <T as EnvoyDiagnosticsService>::report_diagnostics(&inner, request)
                                .await
                        })
                    }
                }

                let inner = Arc::clone(&self.inner);
                Box::pin(async move {
                    let method = ReportDiagnosticsSvc(inner);
                    let codec = tonic_prost::ProstCodec::default();
                    let mut grpc = tonic::server::Grpc::new(codec);
                    Ok(grpc.streaming(method, req).await)
                })
            }
            _ => Box::pin(async move {
                let mut response = http::Response::new(tonic::body::Body::default());
                let headers = response.headers_mut();
                headers.insert(
                    tonic::Status::GRPC_STATUS,
                    (tonic::Code::Unimplemented as i32).into(),
                );
                headers.insert(
                    http::header::CONTENT_TYPE,
                    tonic::metadata::GRPC_CONTENT_TYPE,
                );
                Ok(response)
            }),
        }
    }
}

impl<T> Clone for EnvoyDiagnosticsServiceServer<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> tonic::server::NamedService for EnvoyDiagnosticsServiceServer<T> {
    const NAME: &'static str = SERVICE_NAME;
}

pub struct DiagnosticsService {
    resolver: Arc<dyn TeamResolver>,
    pool: sqlx::PgPool,
}

impl DiagnosticsService {
    pub fn new(resolver: Arc<dyn TeamResolver>, pool: sqlx::PgPool) -> Self {
        Self { resolver, pool }
    }

    pub fn into_server(self) -> EnvoyDiagnosticsServiceServer<Self> {
        EnvoyDiagnosticsServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl EnvoyDiagnosticsService for DiagnosticsService {
    type ReportDiagnosticsStream = ResponseStream;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
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
        let identity = self
            .resolver
            .resolve("diagnostics", peer_spiffe.as_deref())
            .await?;
        let Some(bound_dataplane_id) = identity.dataplane_id else {
            return Err(Status::unauthenticated(
                "diagnostics requires certificate-registry dataplane binding",
            ));
        };

        let mut inbound = request.into_inner();
        let pool = self.pool.clone();
        let team_id = identity.team_id;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<DiagnosticsAck, Status>>(32);

        tokio::spawn(async move {
            while let Some(report) = inbound.next().await {
                let ack = match report {
                    Ok(report) => process_report(&pool, team_id, bound_dataplane_id, report).await,
                    Err(status) => Err(status),
                };
                if tx.send(ack).await.is_err() {
                    return;
                }
            }
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx)) as Self::ReportDiagnosticsStream
        ))
    }
}

async fn process_report(
    pool: &sqlx::PgPool,
    team_id: fp_domain::TeamId,
    bound_dataplane_id: DataplaneId,
    report: DiagnosticsReport,
) -> Result<DiagnosticsAck, Status> {
    let ids = if report.report_id.is_empty() {
        Vec::new()
    } else {
        vec![report.report_id.clone()]
    };
    if report.schema_version == 0 || report.report_id.is_empty() || report.payload.is_none() {
        return Ok(ack(ids, AckStatus::Invalid, "invalid diagnostics report"));
    }
    let claimed = match DataplaneId::from_str(&report.dataplane_id) {
        Ok(id) => id,
        Err(_) => return Ok(ack(ids, AckStatus::Invalid, "invalid dataplane_id")),
    };
    if claimed != bound_dataplane_id {
        return Ok(ack(
            ids,
            AckStatus::Unauthorized,
            "dataplane_id does not match the client certificate",
        ));
    }

    let Some(payload) = report.payload else {
        return Ok(ack(ids, AckStatus::Invalid, "invalid diagnostics report"));
    };
    let (requests_delta, errors_delta, warming_delta, verified) = match payload {
        diagnostics_report::Payload::Heartbeat(heartbeat) => (
            heartbeat.requests_delta,
            heartbeat.errors_delta,
            heartbeat.warming_failures_delta,
            heartbeat.config_verified,
        ),
        diagnostics_report::Payload::ListenerState(listener) => {
            if listener.error_details.is_empty() {
                (0, 0, 0, true)
            } else {
                (0, 0, 1, false)
            }
        }
    };
    if requests_delta < 0 || errors_delta < 0 || warming_delta < 0 {
        return Ok(ack(ids, AckStatus::Invalid, "negative telemetry delta"));
    }

    fp_storage::repos::dataplanes::record_telemetry_by_id(
        pool,
        team_id,
        bound_dataplane_id,
        &report.report_id,
        requests_delta,
        errors_delta,
        warming_delta,
        verified,
    )
    .await
    .map_err(|e| Status::internal(e.to_string()))?;
    Ok(ack(ids, AckStatus::Ok, "accepted"))
}

fn ack(ids: Vec<String>, status: AckStatus, message: &str) -> DiagnosticsAck {
    DiagnosticsAck {
        report_ids: ids,
        status: status as i32,
        message: message.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct EnvoyDiagnosticsServiceClient<T> {
    inner: tonic::client::Grpc<T>,
}

impl<T> EnvoyDiagnosticsServiceClient<T>
where
    T: tonic::client::GrpcService<tonic::body::Body>,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
{
    pub fn new(inner: T) -> Self {
        Self {
            inner: tonic::client::Grpc::new(inner),
        }
    }

    pub async fn report_diagnostics(
        &mut self,
        request: impl tonic::IntoStreamingRequest<Message = DiagnosticsReport>,
    ) -> Result<tonic::Response<tonic::codec::Streaming<DiagnosticsAck>>, tonic::Status> {
        self.inner
            .ready()
            .await
            .map_err(|e| tonic::Status::unknown(format!("service was not ready: {}", e.into())))?;
        let codec = tonic_prost::ProstCodec::default();
        let path = http::uri::PathAndQuery::from_static(
            "/flowplane.diagnostics.v1.EnvoyDiagnosticsService/ReportDiagnostics",
        );
        let mut req = request.into_streaming_request();
        req.extensions_mut().insert(GrpcMethod::new(
            "flowplane.diagnostics.v1.EnvoyDiagnosticsService",
            "ReportDiagnostics",
        ));
        self.inner.streaming(req, path, codec).await
    }
}
