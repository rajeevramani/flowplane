use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, Request, StatusCode},
    Json,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, warn};
use utoipa::{IntoParams, ToSchema};

use crate::{
    api::{error::ApiError, routes::ApiState},
    openapi::{
        self, build_gateway_plan,
        defaults::{
            DEFAULT_GATEWAY_ADDRESS, DEFAULT_GATEWAY_CLUSTER, DEFAULT_GATEWAY_LISTENER,
            DEFAULT_GATEWAY_PORT, DEFAULT_GATEWAY_ROUTES,
        },
        GatewayOptions, GatewaySummary,
    },
    storage::{
        ClusterRepository, ListenerRepository, RouteRepository, UpdateRouteRepositoryRequest,
    },
    xds::route::{
        PathMatch as XdsPathMatch, RouteActionConfig as XdsRouteActionConfig,
        RouteConfig as XdsRouteConfig,
    },
};

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
pub struct GatewayQuery {
    /// Unique name for the generated gateway resources.
    pub name: String,
    /// Optional listener name override; providing this enables dedicated listener mode.
    #[serde(default)]
    pub listener: Option<String>,
    /// Optional listener port (dedicated mode only).
    #[serde(default)]
    pub port: Option<u16>,
    /// Optional listener bind address (dedicated mode only).
    #[serde(default)]
    pub bind_address: Option<String>,
    /// Optional listener protocol (dedicated mode only).
    #[serde(default)]
    pub protocol: Option<String>,
}

/// Binary OpenAPI payload accepted by the gateway import endpoint.
#[derive(Debug, ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct OpenApiSpecBody(pub Vec<u8>);

fn default_address() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    10000
}

fn default_protocol() -> String {
    "HTTP".to_string()
}

#[utoipa::path(
    post,
    path = "/api/v1/gateways/openapi",
    params(GatewayQuery),
    request_body(
        description = "OpenAPI 3.0 document in JSON or YAML format",
        content(
            (OpenApiSpecBody = "application/yaml"),
            (OpenApiSpecBody = "application/x-yaml"),
            (OpenApiSpecBody = "application/json")
        )
    ),
    responses(
        (status = 201, description = "Gateway created from OpenAPI document", body = GatewaySummary),
        (status = 400, description = "Invalid OpenAPI specification"),
        (status = 409, description = "Gateway resources conflict"),
        (status = 500, description = "Internal server error")
    ),
    tag = "gateways"
)]
pub async fn create_gateway_from_openapi_handler(
    State(state): State<ApiState>,
    Query(params): Query<GatewayQuery>,
    request: Request<Body>,
) -> Result<(StatusCode, Json<GatewaySummary>), ApiError> {
    let (parts, body) = request.into_parts();
    let collected = body
        .collect()
        .await
        .map_err(|err| ApiError::BadRequest(format!("Failed to read body: {}", err)))?;

    let bytes = collected.to_bytes();

    if bytes.is_empty() {
        return Err(ApiError::BadRequest(
            "OpenAPI specification body must not be empty".to_string(),
        ));
    }

    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok());

    let document = parse_openapi_document(&bytes, content_type.as_ref())?;

    let wants_dedicated = params.listener.is_some()
        || params.port.is_some()
        || params.bind_address.is_some()
        || params.protocol.is_some();

    let listener_name = if let Some(name) = params.listener.clone() {
        name
    } else if wants_dedicated {
        format!("{}-listener", params.name)
    } else {
        DEFAULT_GATEWAY_LISTENER.to_string()
    };

    let (bind_address, port, protocol) = if wants_dedicated {
        (
            params.bind_address.clone().unwrap_or_else(default_address),
            params.port.unwrap_or_else(default_port),
            params.protocol.clone().unwrap_or_else(default_protocol),
        )
    } else {
        (DEFAULT_GATEWAY_ADDRESS.to_string(), DEFAULT_GATEWAY_PORT, "HTTP".to_string())
    };

    let options = GatewayOptions {
        name: params.name.clone(),
        bind_address,
        port,
        protocol,
        shared_listener: !wants_dedicated,
        listener_name,
    };

    let plan = build_gateway_plan(document, options)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    let cluster_repo = require_cluster_repository(&state)?;
    let route_repo = require_route_repository(&state)?;
    let listener_repo = require_listener_repository(&state)?;

    for cluster in &plan.cluster_requests {
        if cluster_repo.exists_by_name(&cluster.name).await? {
            return Err(ApiError::Conflict(format!("Cluster '{}' already exists", cluster.name)));
        }
    }

    let mut created_clusters: Vec<String> = Vec::new();
    for request in &plan.cluster_requests {
        match cluster_repo.create(request.clone()).await {
            Ok(_) => created_clusters.push(request.name.clone()),
            Err(err) => {
                rollback_import(
                    &listener_repo,
                    &route_repo,
                    &cluster_repo,
                    None,
                    None,
                    &created_clusters,
                )
                .await;
                return Err(ApiError::from(err));
            }
        }
    }

    if let Some(route_request) = plan.route_request.as_ref() {
        if route_repo.exists_by_name(&route_request.name).await? {
            rollback_import(
                &listener_repo,
                &route_repo,
                &cluster_repo,
                None,
                None,
                &created_clusters,
            )
            .await;
            return Err(ApiError::Conflict(format!(
                "Route configuration '{}' already exists",
                route_request.name
            )));
        }

        let route_name = route_request.name.clone();
        if let Err(err) = route_repo.create(route_request.clone()).await {
            rollback_import(
                &listener_repo,
                &route_repo,
                &cluster_repo,
                None,
                None,
                &created_clusters,
            )
            .await;
            return Err(ApiError::from(err));
        }

        if let Some(listener_request) = plan.listener_request.as_ref() {
            if listener_repo.exists_by_name(&listener_request.name).await? {
                rollback_import(
                    &listener_repo,
                    &route_repo,
                    &cluster_repo,
                    None,
                    Some(&route_name),
                    &created_clusters,
                )
                .await;
                return Err(ApiError::Conflict(format!(
                    "Listener '{}' already exists",
                    listener_request.name
                )));
            }

            if let Err(err) = listener_repo.create(listener_request.clone()).await {
                rollback_import(
                    &listener_repo,
                    &route_repo,
                    &cluster_repo,
                    None,
                    Some(&route_name),
                    &created_clusters,
                )
                .await;
                return Err(ApiError::from(err));
            }
        }
    } else if let Some(virtual_host) = plan.default_virtual_host.as_ref() {
        let default_route =
            route_repo.get_by_name(DEFAULT_GATEWAY_ROUTES).await.map_err(ApiError::from)?;

        let mut route_value: Value =
            serde_json::from_str(&default_route.configuration).map_err(|err| {
                ApiError::Internal(format!(
                    "Failed to parse default gateway route configuration: {}",
                    err
                ))
            })?;

        openapi::strip_gateway_tags(&mut route_value);

        let mut route_config: XdsRouteConfig =
            serde_json::from_value(route_value).map_err(|err| {
                ApiError::Internal(format!(
                    "Failed to deserialize default gateway route configuration: {}",
                    err
                ))
            })?;

        if route_config.virtual_hosts.iter().any(|vh| vh.name == virtual_host.name) {
            rollback_import(
                &listener_repo,
                &route_repo,
                &cluster_repo,
                None,
                None,
                &created_clusters,
            )
            .await;
            return Err(ApiError::Conflict(format!(
                "Gateway '{}' already exists",
                plan.summary.gateway
            )));
        }

        let mut new_virtual_host = virtual_host.clone();
        new_virtual_host.domains.retain(|domain| domain != "*");
        if new_virtual_host.domains.is_empty() {
            if let Some(domain) = virtual_host.domains.first() {
                new_virtual_host.domains.push(domain.clone());
            } else {
                new_virtual_host.domains.push("*".to_string());
            }
        }

        let path_prefix_summary = new_virtual_host
            .routes
            .first()
            .map(|route| match &route.r#match.path {
                XdsPathMatch::Exact(value) => value.clone(),
                XdsPathMatch::Prefix(value) => value.clone(),
                XdsPathMatch::Regex(value) => value.clone(),
                XdsPathMatch::Template(value) => value.clone(),
            })
            .unwrap_or_else(|| "/".to_string());

        let cluster_summary = new_virtual_host
            .routes
            .first()
            .map(|route| match &route.action {
                XdsRouteActionConfig::Cluster { name, .. } => name.clone(),
                XdsRouteActionConfig::WeightedClusters { clusters, .. } => clusters
                    .first()
                    .map(|cluster| cluster.name.clone())
                    .unwrap_or_else(|| DEFAULT_GATEWAY_CLUSTER.to_string()),
                XdsRouteActionConfig::Redirect { .. } => "__redirect__".to_string(),
            })
            .unwrap_or_else(|| DEFAULT_GATEWAY_CLUSTER.to_string());

        route_config.virtual_hosts.insert(0, new_virtual_host);

        let mut route_value = serde_json::to_value(&route_config).map_err(|err| {
            ApiError::Internal(format!(
                "Failed to serialize default gateway route configuration: {}",
                err
            ))
        })?;
        openapi::attach_gateway_tag(&mut route_value, &plan.summary.gateway);

        let update_request = UpdateRouteRepositoryRequest {
            path_prefix: Some(path_prefix_summary),
            cluster_name: Some(cluster_summary),
            configuration: Some(route_value),
        };

        if let Err(err) = route_repo.update(&default_route.id, update_request).await {
            rollback_import(
                &listener_repo,
                &route_repo,
                &cluster_repo,
                None,
                None,
                &created_clusters,
            )
            .await;
            return Err(ApiError::from(err));
        }
    }

    state.xds_state.refresh_clusters_from_repository().await?;
    state.xds_state.refresh_routes_from_repository().await?;
    state.xds_state.refresh_listeners_from_repository().await?;

    info!(
        gateway = %plan.summary.gateway,
        listener = %plan.summary.listener,
        route = %plan.summary.route_config,
        clusters = plan.summary.clusters.len(),
        "Gateway created from OpenAPI"
    );

    Ok((StatusCode::CREATED, Json(plan.summary)))
}

fn parse_openapi_document(
    bytes: &Bytes,
    mime: Option<&mime::Mime>,
) -> Result<openapiv3::OpenAPI, ApiError> {
    if let Some(mime) = mime {
        if mime.subtype() == mime::JSON {
            return serde_json::from_slice(bytes).map_err(|err| {
                ApiError::BadRequest(format!("Invalid OpenAPI JSON document: {}", err))
            });
        }

        if mime.subtype() == "yaml"
            || mime.subtype() == "x-yaml"
            || mime.suffix().map(|name| name == "yaml").unwrap_or(false)
        {
            return parse_yaml(bytes);
        }
    }

    match serde_json::from_slice(bytes) {
        Ok(doc) => Ok(doc),
        Err(json_err) => match parse_yaml(bytes) {
            Ok(doc) => Ok(doc),
            Err(yaml_err) => Err(ApiError::BadRequest(format!(
                "Failed to parse OpenAPI document. JSON error: {}; YAML error: {:?}",
                json_err, yaml_err
            ))),
        },
    }
}

fn parse_yaml(bytes: &Bytes) -> Result<openapiv3::OpenAPI, ApiError> {
    let value: serde_json::Value = serde_yaml::from_slice(bytes)
        .map_err(|err| ApiError::BadRequest(format!("Invalid OpenAPI YAML document: {}", err)))?;
    serde_json::from_value(value)
        .map_err(|err| ApiError::BadRequest(format!("Invalid OpenAPI YAML structure: {}", err)))
}

fn require_cluster_repository(state: &ApiState) -> Result<ClusterRepository, ApiError> {
    state
        .xds_state
        .cluster_repository
        .clone()
        .ok_or_else(|| ApiError::service_unavailable("Cluster repository not available"))
}

fn require_route_repository(state: &ApiState) -> Result<RouteRepository, ApiError> {
    state
        .xds_state
        .route_repository
        .clone()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not available"))
}

fn require_listener_repository(state: &ApiState) -> Result<ListenerRepository, ApiError> {
    state
        .xds_state
        .listener_repository
        .clone()
        .ok_or_else(|| ApiError::service_unavailable("Listener repository not available"))
}

async fn rollback_import(
    listener_repo: &ListenerRepository,
    route_repo: &RouteRepository,
    cluster_repo: &ClusterRepository,
    listener: Option<&str>,
    route: Option<&str>,
    clusters: &[String],
) {
    if let Some(listener_name) = listener {
        if let Err(err) = listener_repo.delete_by_name(listener_name).await {
            warn!(
                error = %format!("{}", err),
                listener = %listener_name,
                "Failed to rollback listener"
            );
        }
    }

    if let Some(route_name) = route {
        if let Err(err) = route_repo.delete_by_name(route_name).await {
            warn!(
                error = %format!("{}", err),
                route = %route_name,
                "Failed to rollback route"
            );
        }
    }

    for cluster in clusters {
        if let Err(err) = cluster_repo.delete_by_name(cluster).await {
            warn!(
                error = %format!("{}", err),
                cluster = %cluster,
                "Failed to rollback cluster"
            );
        }
    }
}
