use std::collections::HashMap;
use std::convert::TryFrom;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info};
use utoipa::ToSchema;

use crate::{
    errors::Error,
    openapi::defaults::is_default_gateway_listener,
    storage::{CreateListenerRequest, ListenerData, ListenerRepository, UpdateListenerRequest},
    xds::filters::http::HttpFilterConfigEntry,
    xds::listener::{
        AccessLogConfig, FilterChainConfig, FilterConfig, FilterType, ListenerConfig,
        TlsContextConfig, TracingConfig,
    },
    xds::route::RouteConfig,
};

use super::{error::ApiError, routes::ApiState};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerResponse {
    pub name: String,
    pub address: String,
    pub port: Option<u16>,
    pub protocol: String,
    pub version: i64,
    #[schema(value_type = Object)]
    pub config: ListenerConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListListenersQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateListenerBody {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainInput>,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateListenerBody {
    pub address: String,
    pub port: u16,
    pub filter_chains: Vec<ListenerFilterChainInput>,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterChainInput {
    pub name: Option<String>,
    pub filters: Vec<ListenerFilterInput>,
    #[serde(default)]
    pub tls_context: Option<ListenerTlsContextInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFilterInput {
    pub name: String,
    #[serde(flatten)]
    pub filter_type: ListenerFilterTypeInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ListenerFilterTypeInput {
    #[serde(rename_all = "camelCase")]
    HttpConnectionManager {
        route_config_name: Option<String>,
        #[schema(value_type = Object)]
        inline_route_config: Option<Value>,
        #[serde(default)]
        access_log: Option<ListenerAccessLogInput>,
        #[serde(default)]
        tracing: Option<ListenerTracingInput>,
        #[serde(default)]
        #[schema(value_type = Vec<Object>)]
        http_filters: Vec<HttpFilterConfigEntry>,
    },
    #[serde(rename_all = "camelCase")]
    TcpProxy {
        cluster: String,
        #[serde(default)]
        access_log: Option<ListenerAccessLogInput>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerTlsContextInput {
    pub cert_chain_file: Option<String>,
    pub private_key_file: Option<String>,
    pub ca_cert_file: Option<String>,
    pub require_client_certificate: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerAccessLogInput {
    pub path: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerTracingInput {
    pub provider: String,
    #[schema(value_type = Object)]
    pub config: Value,
}

#[utoipa::path(
    post,
    path = "/api/v1/listeners",
    request_body = CreateListenerBody,
    responses(
        (status = 201, description = "Listener created", body = ListenerResponse),
        (status = 400, description = "Invalid listener payload"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "listeners"
)]
pub async fn create_listener_handler(
    State(state): State<ApiState>,
    Json(payload): Json<CreateListenerBody>,
) -> Result<(StatusCode, Json<ListenerResponse>), ApiError> {
    validate_create_listener_body(&payload)?;

    let repository = require_listener_repository(&state)?;
    let config = listener_config_from_create(&payload)?;
    let configuration = serde_json::to_value(&config).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to serialize listener configuration: {}",
            err
        )))
    })?;

    let request = CreateListenerRequest {
        name: payload.name.clone(),
        address: payload.address.clone(),
        port: Some(payload.port as i64),
        protocol: payload.protocol.clone(),
        configuration,
    };

    let created = repository.create(request).await.map_err(ApiError::from)?;
    info!(listener_id = %created.id, listener_name = %created.name, "Listener created via API");

    state
        .xds_state
        .refresh_listeners_from_repository()
        .await
        .map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after listener creation");
            ApiError::from(err)
        })?;

    let response = listener_response_from_data(created)?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/listeners",
    params(
        ("limit" = Option<i32>, Query, description = "Maximum number of listeners to return"),
        ("offset" = Option<i32>, Query, description = "Offset for paginated results"),
    ),
    responses(
        (status = 200, description = "List of listeners", body = [ListenerResponse]),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "listeners"
)]
pub async fn list_listeners_handler(
    State(state): State<ApiState>,
    Query(params): Query<ListListenersQuery>,
) -> Result<Json<Vec<ListenerResponse>>, ApiError> {
    let repository = require_listener_repository(&state)?;
    let rows = repository
        .list(params.limit, params.offset)
        .await
        .map_err(ApiError::from)?;

    let mut listeners = Vec::with_capacity(rows.len());
    for row in rows {
        listeners.push(listener_response_from_data(row)?);
    }

    Ok(Json(listeners))
}

#[utoipa::path(
    get,
    path = "/api/v1/listeners/{name}",
    params(("name" = String, Path, description = "Name of the listener")),
    responses(
        (status = 200, description = "Listener details", body = ListenerResponse),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "listeners"
)]
pub async fn get_listener_handler(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<ListenerResponse>, ApiError> {
    let repository = require_listener_repository(&state)?;
    let listener = repository
        .get_by_name(&name)
        .await
        .map_err(ApiError::from)?;
    let response = listener_response_from_data(listener)?;
    Ok(Json(response))
}

#[utoipa::path(
    put,
    path = "/api/v1/listeners/{name}",
    request_body = UpdateListenerBody,
    params(("name" = String, Path, description = "Name of the listener")),
    responses(
        (status = 200, description = "Listener updated", body = ListenerResponse),
        (status = 400, description = "Invalid listener payload"),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "listeners"
)]
pub async fn update_listener_handler(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Json(payload): Json<UpdateListenerBody>,
) -> Result<Json<ListenerResponse>, ApiError> {
    validate_update_listener_body(&payload)?;

    let repository = require_listener_repository(&state)?;
    let existing = repository
        .get_by_name(&name)
        .await
        .map_err(ApiError::from)?;

    let config = listener_config_from_update(name.clone(), &payload)?;
    let configuration = serde_json::to_value(&config).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to serialize listener configuration: {}",
            err
        )))
    })?;

    let request = UpdateListenerRequest {
        address: Some(payload.address.clone()),
        port: Some(Some(payload.port as i64)),
        protocol: payload.protocol.clone(),
        configuration: Some(configuration),
    };

    let updated = repository
        .update(&existing.id, request)
        .await
        .map_err(ApiError::from)?;

    info!(listener_id = %existing.id, listener_name = %name, "Listener updated via API");

    state
        .xds_state
        .refresh_listeners_from_repository()
        .await
        .map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after listener update");
            ApiError::from(err)
        })?;

    let response = listener_response_from_data(updated)?;
    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/listeners/{name}",
    params(("name" = String, Path, description = "Name of the listener")),
    responses(
        (status = 204, description = "Listener deleted"),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Listener repository unavailable"),
    ),
    tag = "listeners"
)]
pub async fn delete_listener_handler(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    if is_default_gateway_listener(&name) {
        return Err(ApiError::Conflict(
            "The default gateway listener cannot be deleted".to_string(),
        ));
    }

    let repository = require_listener_repository(&state)?;
    let existing = repository
        .get_by_name(&name)
        .await
        .map_err(ApiError::from)?;

    repository
        .delete(&existing.id)
        .await
        .map_err(ApiError::from)?;

    info!(listener_id = %existing.id, listener_name = %name, "Listener deleted via API");

    state
        .xds_state
        .refresh_listeners_from_repository()
        .await
        .map_err(|err| {
            error!(error = %err, "Failed to refresh xDS caches after listener deletion");
            ApiError::from(err)
        })?;

    Ok(StatusCode::NO_CONTENT)
}

fn require_listener_repository(state: &ApiState) -> Result<ListenerRepository, ApiError> {
    state
        .xds_state
        .listener_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Listener repository not configured"))
}

fn listener_response_from_data(data: ListenerData) -> Result<ListenerResponse, ApiError> {
    let config: ListenerConfig = serde_json::from_str(&data.configuration).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to parse stored listener configuration '{}': {}",
            data.name, err
        )))
    })?;

    Ok(ListenerResponse {
        name: data.name,
        address: data.address,
        port: port_from_i64(data.port)?,
        protocol: data.protocol,
        version: data.version,
        config,
    })
}

fn listener_config_from_create(body: &CreateListenerBody) -> Result<ListenerConfig, ApiError> {
    listener_config_from_parts(
        body.name.clone(),
        body.address.clone(),
        body.port,
        &body.filter_chains,
    )
}

fn listener_config_from_update(
    name: String,
    body: &UpdateListenerBody,
) -> Result<ListenerConfig, ApiError> {
    listener_config_from_parts(name, body.address.clone(), body.port, &body.filter_chains)
}

fn listener_config_from_parts(
    name: String,
    address: String,
    port: u16,
    filter_chains: &[ListenerFilterChainInput],
) -> Result<ListenerConfig, ApiError> {
    let chains = filter_chains
        .iter()
        .map(convert_filter_chain)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ListenerConfig {
        name,
        address,
        port: port.into(),
        filter_chains: chains,
    })
}

fn convert_filter_chain(input: &ListenerFilterChainInput) -> Result<FilterChainConfig, ApiError> {
    let filters = input
        .filters
        .iter()
        .map(convert_filter)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(FilterChainConfig {
        name: input.name.clone(),
        filters,
        tls_context: input.tls_context.as_ref().map(convert_tls_context),
    })
}

fn convert_filter(input: &ListenerFilterInput) -> Result<FilterConfig, ApiError> {
    Ok(FilterConfig {
        name: input.name.clone(),
        filter_type: convert_filter_type(&input.filter_type)?,
    })
}

fn convert_filter_type(input: &ListenerFilterTypeInput) -> Result<FilterType, ApiError> {
    match input {
        ListenerFilterTypeInput::HttpConnectionManager {
            route_config_name,
            inline_route_config,
            access_log,
            tracing,
            http_filters,
        } => {
            let inline_route_config = match inline_route_config {
                Some(value) => Some(parse_route_config(value)?),
                None => None,
            };

            if route_config_name
                .as_ref()
                .map(|name| name.trim().is_empty())
                .unwrap_or(true)
                && inline_route_config.is_none()
            {
                return Err(ApiError::from(Error::validation(
                    "HttpConnectionManager requires route_config_name or inline_route_config",
                )));
            }

            Ok(FilterType::HttpConnectionManager {
                route_config_name: route_config_name.as_ref().map(|s| s.trim().to_string()),
                inline_route_config,
                access_log: access_log.as_ref().map(convert_access_log),
                tracing: tracing.as_ref().map(convert_tracing).transpose()?,
                http_filters: http_filters.clone(),
            })
        }
        ListenerFilterTypeInput::TcpProxy {
            cluster,
            access_log,
        } => {
            if cluster.trim().is_empty() {
                return Err(ApiError::from(Error::validation(
                    "TCP proxy filter requires a non-empty cluster",
                )));
            }
            Ok(FilterType::TcpProxy {
                cluster: cluster.trim().to_string(),
                access_log: access_log.as_ref().map(convert_access_log),
            })
        }
    }
}

fn convert_tls_context(input: &ListenerTlsContextInput) -> TlsContextConfig {
    TlsContextConfig {
        cert_chain_file: input.cert_chain_file.clone(),
        private_key_file: input.private_key_file.clone(),
        ca_cert_file: input.ca_cert_file.clone(),
        require_client_certificate: input.require_client_certificate,
    }
}

fn convert_access_log(input: &ListenerAccessLogInput) -> AccessLogConfig {
    AccessLogConfig {
        path: input.path.clone(),
        format: input.format.clone(),
    }
}

fn convert_tracing(input: &ListenerTracingInput) -> Result<TracingConfig, ApiError> {
    Ok(TracingConfig {
        provider: input.provider.clone(),
        config: convert_tracing_config(&input.config)?,
    })
}

fn parse_route_config(value: &Value) -> Result<RouteConfig, ApiError> {
    serde_json::from_value(value.clone()).map_err(|err| {
        ApiError::from(Error::validation(format!(
            "Invalid inline route configuration: {}",
            err
        )))
    })
}

fn convert_tracing_config(value: &Value) -> Result<HashMap<String, String>, ApiError> {
    let map = value
        .as_object()
        .ok_or_else(|| ApiError::from(Error::validation("Tracing config must be a JSON object")))?;

    let mut config = HashMap::new();
    for (key, val) in map {
        if let Some(str_value) = val.as_str() {
            config.insert(key.clone(), str_value.to_string());
        } else {
            return Err(ApiError::from(Error::validation(
                "Tracing config values must be strings",
            )));
        }
    }

    Ok(config)
}

fn validate_create_listener_body(body: &CreateListenerBody) -> Result<(), ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::from(Error::validation(
            "Listener name cannot be empty",
        )));
    }
    validate_listener_common(&body.address, body.port, &body.filter_chains)
}

fn validate_update_listener_body(body: &UpdateListenerBody) -> Result<(), ApiError> {
    validate_listener_common(&body.address, body.port, &body.filter_chains)
}

fn validate_listener_common(
    address: &str,
    port: u16,
    filter_chains: &[ListenerFilterChainInput],
) -> Result<(), ApiError> {
    if address.trim().is_empty() {
        return Err(ApiError::from(Error::validation(
            "Listener address cannot be empty",
        )));
    }

    if port < 1024 {
        return Err(ApiError::from(Error::validation(
            "Listener port must be >= 1024",
        )));
    }

    if filter_chains.is_empty() {
        return Err(ApiError::from(Error::validation(
            "At least one filter chain is required",
        )));
    }

    for chain in filter_chains {
        if chain.filters.is_empty() {
            return Err(ApiError::from(Error::validation(
                "Each filter chain must include at least one filter",
            )));
        }

        for filter in &chain.filters {
            if filter.name.trim().is_empty() {
                return Err(ApiError::from(Error::validation(
                    "Filter name cannot be empty",
                )));
            }

            if let ListenerFilterTypeInput::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                ..
            } = &filter.filter_type
            {
                if route_config_name
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                    && inline_route_config.is_none()
                {
                    return Err(ApiError::from(Error::validation(
                        "HttpConnectionManager filter requires route_config_name or inline_route_config",
                    )));
                }

                if let Some(value) = inline_route_config {
                    parse_route_config(value)?;
                }
            }
        }
    }

    Ok(())
}

fn port_from_i64(port: Option<i64>) -> Result<Option<u16>, ApiError> {
    match port {
        Some(value) => u16::try_from(value).map(Some).map_err(|_| {
            ApiError::from(Error::internal(format!(
                "Stored listener port value '{}' is out of range",
                value
            )))
        }),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SimpleXdsConfig,
        storage::DbPool,
        xds::resources::LISTENER_TYPE_URL,
        xds::route::{
            PathMatch, RouteActionConfig, RouteConfig as InlineRouteConfig, RouteMatchConfig,
            RouteRule, VirtualHostConfig,
        },
        xds::XdsState,
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    async fn create_test_pool() -> DbPool {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .expect("create sqlite pool");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                service_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS routes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                path_prefix TEXT NOT NULL,
                cluster_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS listeners (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                address TEXT NOT NULL,
                port INTEGER,
                protocol TEXT NOT NULL DEFAULT 'HTTP',
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    async fn build_state() -> (Arc<XdsState>, ApiState) {
        let pool = create_test_pool().await;
        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        let api_state = ApiState {
            xds_state: state.clone(),
        };
        (state, api_state)
    }

    #[test]
    fn convert_http_filter_requires_route_source() {
        let result = convert_filter_type(&ListenerFilterTypeInput::HttpConnectionManager {
            route_config_name: None,
            inline_route_config: None,
            access_log: None,
            tracing: None,
            http_filters: Vec::new(),
        });

        assert!(result.is_err());
    }

    #[test]
    fn convert_http_filter_with_inline_route() {
        let route_config = InlineRouteConfig {
            name: "inline-route".to_string(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "vh".to_string(),
                domains: vec!["*".to_string()],
                routes: vec![RouteRule {
                    name: Some("all".to_string()),
                    r#match: RouteMatchConfig {
                        path: PathMatch::Prefix("/".to_string()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "backend".to_string(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        };
        let inline_route = serde_json::to_value(&route_config).unwrap();

        let result = convert_filter_type(&ListenerFilterTypeInput::HttpConnectionManager {
            route_config_name: None,
            inline_route_config: Some(inline_route),
            access_log: None,
            tracing: None,
            http_filters: Vec::new(),
        });

        assert!(result.is_ok());
        match result.unwrap() {
            FilterType::HttpConnectionManager {
                inline_route_config: Some(config),
                ..
            } => {
                assert_eq!(config.name, "inline-route");
            }
            other => panic!("expected HTTP connection manager, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn create_listener_handler_persists_and_refreshes_state() {
        let (state, api_state) = build_state().await;

        let payload = CreateListenerBody {
            name: "edge-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 10000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("primary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        };

        let (status, Json(resp)) = create_listener_handler(State(api_state.clone()), Json(payload))
            .await
            .expect("create listener");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(resp.name, "edge-listener");
        assert_eq!(resp.port, Some(10000));

        // Allow async cache refresh to complete.
        sleep(Duration::from_millis(50)).await;

        let cached = state.cached_resources(LISTENER_TYPE_URL);
        assert_eq!(cached.len(), 1, "listener cache should contain one entry");
    }

    #[tokio::test]
    async fn update_listener_handler_updates_repository() {
        let (state, api_state) = build_state().await;

        // Seed a listener so we can update it.
        let initial = CreateListenerBody {
            name: "edge-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 10000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("primary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        };

        let _ = create_listener_handler(State(api_state.clone()), Json(initial))
            .await
            .expect("seed listener");

        let update_payload = UpdateListenerBody {
            address: "127.0.0.1".to_string(),
            port: 11000,
            protocol: Some("HTTP".to_string()),
            filter_chains: vec![ListenerFilterChainInput {
                name: Some("default".to_string()),
                filters: vec![ListenerFilterInput {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                        route_config_name: Some("secondary-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        };

        let Json(updated) = update_listener_handler(
            State(api_state.clone()),
            Path("edge-listener".to_string()),
            Json(update_payload),
        )
        .await
        .expect("update listener");

        assert_eq!(updated.address, "127.0.0.1");
        assert_eq!(updated.port, Some(11000));
        assert_eq!(updated.version, 2);

        // Ensure cache reflects latest version.
        sleep(Duration::from_millis(50)).await;
        let cached = state.cached_resources(LISTENER_TYPE_URL);
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].name, "edge-listener");
        assert_eq!(cached[0].version, state.get_version_number());
    }
}
