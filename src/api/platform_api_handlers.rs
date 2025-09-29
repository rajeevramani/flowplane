use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use utoipa::{IntoParams, ToSchema};

use crate::storage::repository_simple::ApiDefinitionData;
use crate::{
    api::{error::ApiError, routes::ApiState},
    platform_api::materializer::{
        AppendRouteOutcome, CreateDefinitionOutcome, PlatformApiMaterializer,
    },
    validation::requests::api_definition::{AppendRouteBody, CreateApiDefinitionBody},
};
use axum::http::header;
use axum::response::Response;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiDefinitionResponse {
    id: String,
    bootstrap_uri: String,
    routes: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppendRouteResponse {
    api_id: String,
    route_id: String,
    revision: i64,
    bootstrap_uri: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiDefinitionSummary {
    id: String,
    team: String,
    domain: String,
    listener_isolation: bool,
    bootstrap_uri: Option<String>,
    version: i64,
    created_at: String,
    updated_at: String,
}

impl From<ApiDefinitionData> for ApiDefinitionSummary {
    fn from(row: ApiDefinitionData) -> Self {
        Self {
            id: row.id,
            team: row.team,
            domain: row.domain,
            listener_isolation: row.listener_isolation,
            bootstrap_uri: row.bootstrap_uri,
            version: row.version,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, serde::Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListDefinitionsQuery {
    #[serde(default)]
    #[param(required = false)]
    pub team: Option<String>,
    #[serde(default)]
    #[param(required = false)]
    pub domain: Option<String>,
    #[serde(default)]
    #[param(required = false)]
    pub limit: Option<i32>,
    #[serde(default)]
    #[param(required = false)]
    pub offset: Option<i32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/api-definitions",
    params(ListDefinitionsQuery),
    responses(
        (status = 200, description = "List API definitions", body = [ApiDefinitionSummary])
    ),
    tag = "platform-api"
)]
pub async fn list_api_definitions_handler(
    State(state): State<ApiState>,
    Query(_q): Query<ListDefinitionsQuery>,
) -> Result<Json<Vec<ApiDefinitionSummary>>, ApiError> {
    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;

    let items = repo.list_definitions().await.map_err(ApiError::from)?;
    Ok(Json(items.into_iter().map(ApiDefinitionSummary::from).collect()))
}

#[utoipa::path(
    get,
    path = "/api/v1/api-definitions/{id}",
    params(("id" = String, Path, description = "API definition ID")),
    responses(
        (status = 200, description = "API definition", body = ApiDefinitionSummary),
        (status = 404, description = "Not found")
    ),
    tag = "platform-api"
)]
pub async fn get_api_definition_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<ApiDefinitionSummary>, ApiError> {
    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;
    let row = repo.get_definition(&id).await.map_err(ApiError::from)?;
    Ok(Json(ApiDefinitionSummary::from(row)))
}

#[derive(Debug, serde::Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapQuery {
    #[serde(default)]
    #[param(required = false)]
    pub format: Option<String>, // yaml|json (default yaml)
    #[serde(default)]
    #[param(required = false)]
    pub scope: Option<String>, // all|team|allowlist (default all)
    #[serde(default)]
    #[param(required = false)]
    pub allowlist: Option<Vec<String>>, // names when scope=allowlist
    #[serde(default)]
    #[param(required = false)]
    pub include_default: Option<bool>, // default false in team scope
}

#[utoipa::path(
    get,
    path = "/api/v1/api-definitions/{id}/bootstrap",
    params(("id" = String, Path, description = "API definition ID"), BootstrapQuery),
    responses(
        (status = 200, description = "Envoy bootstrap"),
        (status = 404, description = "Not found")
    ),
    tag = "platform-api"
)]
pub async fn get_bootstrap_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(q): Query<BootstrapQuery>,
) -> Result<Response, ApiError> {
    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;

    let def = repo.get_definition(&id).await.map_err(ApiError::from)?;

    let format = q.format.as_deref().unwrap_or("yaml").to_lowercase();
    let scope = q.scope.as_deref().unwrap_or("all").to_lowercase();
    let include_default = q.include_default.unwrap_or(false);
    let allowlist = q.allowlist.unwrap_or_default();

    // Build ADS bootstrap with node metadata for scoping
    let xds_addr = state.xds_state.config.bind_address.clone();
    let xds_port = state.xds_state.config.port;
    let node_id = format!("team={}/dp-{}", def.team, uuid::Uuid::new_v4());

    let metadata = match scope.as_str() {
        "team" => serde_json::json!({
            "team": def.team,
            "include_default": include_default,
        }),
        "allowlist" => serde_json::json!({
            "team": def.team,
            "listener_allowlist": allowlist,
        }),
        _ => serde_json::json!({}),
    };

    let bootstrap = serde_json::json!({
        "admin": {
            "access_log_path": "/tmp/envoy_admin.log",
            "address": { "socket_address": { "address": "127.0.0.1", "port_value": 9901 } }
        },
        "node": { "id": node_id, "metadata": metadata },
        "dynamic_resources": {
            "lds_config": { "ads": {} },
            "cds_config": { "ads": {} },
            "ads_config": {
                "api_type": "GRPC",
                "transport_api_version": "V3",
                "grpc_services": [ { "envoy_grpc": { "cluster_name": "xds_cluster" } } ]
            }
        },
        "static_resources": {
            "clusters": [
                {
                    "name": "xds_cluster",
                    "type": "LOGICAL_DNS",
                    "dns_lookup_family": "V4_ONLY",
                    "connect_timeout": "1s",
                    "http2_protocol_options": {},
                    "load_assignment": {
                        "cluster_name": "xds_cluster",
                        "endpoints": [ { "lb_endpoints": [ { "endpoint": { "address": { "socket_address": { "address": xds_addr, "port_value": xds_port } } } } ] } ]
                    }
                }
            ]
        }
    });

    let response = if format == "json" {
        let body = serde_json::to_vec(&bootstrap)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    } else {
        let yaml = serde_yaml::to_string(&bootstrap)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/yaml")
            .body(axum::body::Body::from(yaml))
            .unwrap()
    };

    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/api-definitions",
    request_body = CreateApiDefinitionBody,
    responses(
        (status = 201, description = "API definition created", body = CreateApiDefinitionResponse),
        (status = 400, description = "Invalid request"),
        (status = 409, description = "Collision detected"),
        (status = 403, description = "Forbidden")
    ),
    tag = "platform-api"
)]
pub async fn create_api_definition_handler(
    State(state): State<ApiState>,
    Json(payload): Json<CreateApiDefinitionBody>,
) -> Result<(StatusCode, Json<CreateApiDefinitionResponse>), ApiError> {
    let spec = payload.into_spec().map_err(ApiError::from)?;

    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let outcome: CreateDefinitionOutcome =
        materializer.create_definition(spec).await.map_err(ApiError::from)?;

    let created_route_ids = outcome.routes.iter().map(|route| route.id.clone()).collect();

    Ok((
        StatusCode::CREATED,
        Json(CreateApiDefinitionResponse {
            id: outcome.definition.id,
            bootstrap_uri: outcome.bootstrap_uri,
            routes: created_route_ids,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/api-definitions/{id}/routes",
    params(("id" = String, Path, description = "API definition ID")),
    request_body = AppendRouteBody,
    responses(
        (status = 202, description = "Route appended", body = AppendRouteResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict")
    ),
    tag = "platform-api"
)]
pub async fn append_route_handler(
    State(state): State<ApiState>,
    Path(api_definition_id): Path<String>,
    Json(payload): Json<AppendRouteBody>,
) -> Result<(StatusCode, Json<AppendRouteResponse>), ApiError> {
    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let route_spec = payload.into_route_spec(None).map_err(ApiError::from)?;

    let AppendRouteOutcome { definition, route, bootstrap_uri } =
        materializer.append_route(&api_definition_id, route_spec).await.map_err(ApiError::from)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(AppendRouteResponse {
            api_id: definition.id,
            route_id: route.id,
            revision: definition.version,
            bootstrap_uri,
        }),
    ))
}
