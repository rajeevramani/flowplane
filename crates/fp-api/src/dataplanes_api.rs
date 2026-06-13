//! Dataplane management endpoints. The certificate registry and xDS binding internals
//! shipped in S5.4; S6 exposes the operator-facing REST surface.

use crate::error::{ApiError, ErrorBody};
use crate::resources::{resolve_team, ListQuery, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use fp_core::services::dataplanes as svc;
use fp_core::PrincipalCtx;
use fp_domain::dataplane::{Dataplane, ProxyCertificate};
use fp_domain::{DomainError, RequestId, TeamStatsOverview};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct DataplaneView {
    pub id: uuid::Uuid,
    pub team_id: uuid::Uuid,
    pub name: String,
    pub description: String,
    pub revision: i64,
    pub last_heartbeat_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_config_verify_at: Option<chrono::DateTime<chrono::Utc>>,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Dataplane> for DataplaneView {
    fn from(value: Dataplane) -> Self {
        Self {
            id: value.id.as_uuid(),
            team_id: value.team_id.as_uuid(),
            name: value.name,
            description: value.description,
            revision: value.version,
            last_heartbeat_at: value.last_heartbeat_at,
            last_config_verify_at: value.last_config_verify_at,
            total_requests: value.total_requests,
            total_errors: value.total_errors,
            warming_failures: value.warming_failures,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDataplaneBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProxyCertificateView {
    pub id: uuid::Uuid,
    pub team_id: uuid::Uuid,
    pub dataplane_id: uuid::Uuid,
    pub spiffe_uri: String,
    pub serial_number: String,
    pub issued_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<ProxyCertificate> for ProxyCertificateView {
    fn from(value: ProxyCertificate) -> Self {
        Self {
            id: value.id.as_uuid(),
            team_id: value.team_id.as_uuid(),
            dataplane_id: value.dataplane_id.as_uuid(),
            spiffe_uri: value.spiffe_uri,
            serial_number: value.serial_number,
            issued_at: value.issued_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
            revoked_reason: value.revoked_reason,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterProxyCertificateBody {
    pub dataplane: String,
    pub spiffe_uri: String,
    pub serial_number: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueProxyCertificateBody {
    pub dataplane: String,
    #[serde(default = "default_certificate_ttl_hours")]
    pub ttl_hours: i64,
}

fn default_certificate_ttl_hours() -> i64 {
    24
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IssuedProxyCertificateView {
    pub certificate: ProxyCertificateView,
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub ca_certificate_pem: String,
}

impl From<svc::IssuedProxyCertificate> for IssuedProxyCertificateView {
    fn from(value: svc::IssuedProxyCertificate) -> Self {
        Self {
            certificate: ProxyCertificateView::from(value.certificate),
            certificate_pem: value.certificate_pem,
            private_key_pem: value.private_key_pem,
            ca_certificate_pem: value.ca_certificate_pem,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeProxyCertificateBody {
    pub reason: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataplaneTelemetryBody {
    #[serde(default)]
    pub requests_delta: i64,
    #[serde(default)]
    pub errors_delta: i64,
    #[serde(default)]
    pub warming_failures_delta: i64,
    #[serde(default)]
    pub config_verified: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TeamStatsOverviewView {
    pub total_dataplanes: i64,
    pub live_dataplanes: i64,
    pub stale_dataplanes: i64,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
}

impl From<TeamStatsOverview> for TeamStatsOverviewView {
    fn from(value: TeamStatsOverview) -> Self {
        Self {
            total_dataplanes: value.total_dataplanes,
            live_dataplanes: value.live_dataplanes,
            stale_dataplanes: value.stale_dataplanes,
            total_requests: value.total_requests,
            total_errors: value.total_errors,
            warming_failures: value.warming_failures,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct EnvoyConfigQuery {
    /// Host or DNS name Envoy uses to reach the control plane xDS listener.
    #[serde(default = "default_xds_host")]
    pub xds_host: String,
    /// xDS listener port.
    #[serde(default = "default_xds_port")]
    pub xds_port: u16,
    /// Loopback admin port for Envoy.
    #[serde(default = "default_admin_port")]
    pub admin_port: u16,
    /// Dataplane client certificate path as seen by Envoy.
    pub cert_path: String,
    /// Dataplane private key path as seen by Envoy.
    pub key_path: String,
    /// Control-plane/client-CA bundle path as seen by Envoy.
    pub ca_path: String,
}

fn default_xds_host() -> String {
    "127.0.0.1".into()
}

fn default_xds_port() -> u16 {
    18000
}

fn default_admin_port() -> u16 {
    9901
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/dataplanes",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses(
        (status = 200, body = Page<DataplaneView>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn list_dataplanes(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<DataplaneView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_dataplanes(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(DataplaneView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/dataplanes",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateDataplaneBody,
    responses(
        (status = 201, body = DataplaneView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn create_dataplane(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<CreateDataplaneBody>,
) -> Result<(StatusCode, Json<DataplaneView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_dataplane(&state.pool, &ctx, team, &body.name, &body.description, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(DataplaneView::from(created))))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/dataplanes/{name}",
    tag = "Dataplanes",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Dataplane name"),
    ),
    responses(
        (status = 200, body = DataplaneView),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn get_dataplane(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<DataplaneView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_dataplane(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(DataplaneView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/dataplanes/{name}/telemetry",
    tag = "Dataplanes",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Dataplane name"),
    ),
    request_body = DataplaneTelemetryBody,
    responses(
        (status = 200, body = DataplaneView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn record_dataplane_telemetry(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<DataplaneTelemetryBody>,
) -> Result<Json<DataplaneView>, ApiError> {
    let run = async {
        if body.requests_delta < 0 || body.errors_delta < 0 || body.warming_failures_delta < 0 {
            return Err(DomainError::validation(
                "telemetry deltas must be non-negative",
            ));
        }
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::record_telemetry(
            &state.pool,
            &ctx,
            team,
            &name,
            svc::DataplaneTelemetry {
                requests_delta: body.requests_delta,
                errors_delta: body.errors_delta,
                warming_failures_delta: body.warming_failures_delta,
                config_verified: body.config_verified,
            },
            rid,
        )
        .await
    };
    run.await
        .map(|dataplane| Json(DataplaneView::from(dataplane)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/stats/overview",
    tag = "Stats",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses(
        (status = 200, body = TeamStatsOverviewView),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn stats_overview(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<TeamStatsOverviewView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::stats_overview(&state.pool, &ctx, team, rid).await
    };
    run.await
        .map(|overview| Json(TeamStatsOverviewView::from(overview)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/dataplanes/{name}/envoy-config",
    tag = "Dataplanes",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Dataplane name"),
        EnvoyConfigQuery,
    ),
    responses(
        (status = 200, content_type = "text/yaml", body = String),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn get_envoy_config(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Query(query): Query<EnvoyConfigQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let run = async {
        validate_bootstrap_query(&query)?;
        let team_ref = resolve_team(&state, &ctx, &team).await?;
        let dataplane = svc::get_dataplane(&state.pool, &ctx, team_ref, &name, rid).await?;
        Ok::<_, fp_domain::DomainError>(render_envoy_bootstrap(&team, &dataplane, &query))
    };
    let body = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(([(header::CONTENT_TYPE, "text/yaml; charset=utf-8")], body).into_response())
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/proxy-certificates",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses(
        (status = 200, body = Vec<ProxyCertificateView>),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn list_proxy_certificates(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<ProxyCertificateView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_certificates(&state.pool, &ctx, team, rid).await
    };
    run.await
        .map(|items| Json(items.into_iter().map(ProxyCertificateView::from).collect()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/proxy-certificates",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = RegisterProxyCertificateBody,
    responses(
        (status = 201, body = ProxyCertificateView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn register_proxy_certificate(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<RegisterProxyCertificateBody>,
) -> Result<(StatusCode, Json<ProxyCertificateView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::register_certificate(
            &state.pool,
            &ctx,
            team,
            svc::CertificateRegistration {
                dataplane: &body.dataplane,
                spiffe_uri: &body.spiffe_uri,
                serial_number: &body.serial_number,
                expires_at: body.expires_at,
            },
            rid,
        )
        .await
    };
    let cert = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(ProxyCertificateView::from(cert))))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/proxy-certificates/issue",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = IssueProxyCertificateBody,
    responses(
        (status = 201, body = IssuedProxyCertificateView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
        (status = 500, body = ErrorBody),
    ))]
pub async fn issue_proxy_certificate(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<IssueProxyCertificateBody>,
) -> Result<(StatusCode, Json<IssuedProxyCertificateView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::issue_certificate(
            &state.pool,
            &ctx,
            team,
            svc::CertificateIssueRequest {
                dataplane: &body.dataplane,
                ttl_hours: body.ttl_hours,
            },
            rid,
        )
        .await
    };
    let cert = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        StatusCode::CREATED,
        Json(IssuedProxyCertificateView::from(cert)),
    ))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/proxy-certificates/{serial_number}/revoke",
    tag = "Dataplanes",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("serial_number" = String, Path, description = "Certificate serial number"),
    ),
    request_body = RevokeProxyCertificateBody,
    responses(
        (status = 200, body = ProxyCertificateView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn revoke_proxy_certificate(
    State(state): State<AppState>,
    Path((team, serial_number)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<RevokeProxyCertificateBody>,
) -> Result<Json<ProxyCertificateView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::revoke_certificate(&state.pool, &ctx, team, &serial_number, &body.reason, rid).await
    };
    run.await
        .map(|cert| Json(ProxyCertificateView::from(cert)))
        .map_err(|e| ApiError::new(e, rid))
}

fn validate_bootstrap_query(query: &EnvoyConfigQuery) -> Result<(), DomainError> {
    for (name, value) in [
        ("xds_host", query.xds_host.as_str()),
        ("cert_path", query.cert_path.as_str()),
        ("key_path", query.key_path.as_str()),
        ("ca_path", query.ca_path.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(DomainError::validation(format!("{name} must not be empty")));
        }
        if value.chars().any(|c| c.is_control()) {
            return Err(DomainError::validation(format!(
                "{name} must not contain control characters"
            )));
        }
    }
    Ok(())
}

fn render_envoy_bootstrap(team: &str, dataplane: &Dataplane, query: &EnvoyConfigQuery) -> String {
    let node_id = format!("team={team}/dp-{}", dataplane.id.as_uuid());
    let cluster = format!("{team}-cluster");
    let dataplane_id = dataplane.id.as_uuid().to_string();
    format!(
        r#"node:
  id: {node_id}
  cluster: {cluster}
  metadata:
    team: {team}
    dataplane_id: {dataplane_id}
    dataplane_name: {dataplane_name}
admin:
  address:
    socket_address:
      address: 127.0.0.1
      port_value: {admin_port}
dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
      - envoy_grpc:
          cluster_name: xds_cluster
  cds_config:
    ads: {{}}
    resource_api_version: V3
  lds_config:
    ads: {{}}
    resource_api_version: V3
static_resources:
  clusters:
    - name: xds_cluster
      type: LOGICAL_DNS
      dns_lookup_family: V4_ONLY
      connect_timeout: 1s
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config:
            http2_protocol_options: {{}}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: {xds_host}
                      port_value: {xds_port}
      transport_socket:
        name: envoy.transport_sockets.tls
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
          common_tls_context:
            tls_certificates:
              - certificate_chain:
                  filename: {cert_path}
                private_key:
                  filename: {key_path}
            validation_context:
              trusted_ca:
                filename: {ca_path}
"#,
        node_id = yaml_quote(&node_id),
        cluster = yaml_quote(&cluster),
        team = yaml_quote(team),
        dataplane_id = yaml_quote(&dataplane_id),
        dataplane_name = yaml_quote(&dataplane.name),
        admin_port = query.admin_port,
        xds_host = yaml_quote(&query.xds_host),
        xds_port = query.xds_port,
        cert_path = yaml_quote(&query.cert_path),
        key_path = yaml_quote(&query.key_path),
        ca_path = yaml_quote(&query.ca_path),
    )
}

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn yaml_quote_escapes_double_quotes_and_backslashes() {
        assert_eq!(yaml_quote(r#"a\b"c"#), r#""a\\b\"c""#);
    }
}
