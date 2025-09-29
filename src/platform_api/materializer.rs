use std::sync::Arc;

use serde_json::Value;
// use sqlx::Sqlite;

use super::{
    audit, bootstrap,
    validation::{ensure_domain_available, ensure_route_available},
};
use crate::errors::{Error, Result};
use crate::storage::{
    ApiDefinitionData, ApiDefinitionRepository, ApiRouteData, AuditLogRepository,
    CreateApiDefinitionRequest, CreateApiRouteRequest,
};
use crate::xds::XdsState;

/// High-level specification for creating a Platform API definition.
pub struct ApiDefinitionSpec {
    pub team: String,
    pub domain: String,
    pub listener_isolation: bool,
    pub isolation_listener: Option<ListenerInput>,
    pub tls_config: Option<Value>,
    pub routes: Vec<RouteSpec>,
}

/// Route configuration supplied to the materializer.
#[derive(Clone)]
pub struct RouteSpec {
    pub match_type: String,
    pub match_value: String,
    pub case_sensitive: bool,
    pub rewrite_prefix: Option<String>,
    pub rewrite_regex: Option<String>,
    pub rewrite_substitution: Option<String>,
    pub upstream_targets: Value,
    pub timeout_seconds: Option<i64>,
    pub override_config: Option<Value>,
    pub deployment_note: Option<String>,
    pub route_order: Option<i64>,
}

impl RouteSpec {
    fn into_request(self, definition_id: &str, route_order: i64) -> CreateApiRouteRequest {
        CreateApiRouteRequest {
            api_definition_id: definition_id.to_string(),
            match_type: self.match_type,
            match_value: self.match_value,
            case_sensitive: self.case_sensitive,
            rewrite_prefix: self.rewrite_prefix,
            rewrite_regex: self.rewrite_regex,
            rewrite_substitution: self.rewrite_substitution,
            upstream_targets: self.upstream_targets,
            timeout_seconds: self.timeout_seconds,
            override_config: self.override_config,
            deployment_note: self.deployment_note,
            route_order,
        }
    }
}

pub struct CreateDefinitionOutcome {
    pub definition: ApiDefinitionData,
    pub routes: Vec<ApiRouteData>,
    pub bootstrap_uri: String,
}

pub struct AppendRouteOutcome {
    pub definition: ApiDefinitionData,
    pub route: ApiRouteData,
    pub bootstrap_uri: String,
}

pub struct PlatformApiMaterializer {
    state: Arc<XdsState>,
    repository: ApiDefinitionRepository,
    audit_repo: AuditLogRepository,
}

impl PlatformApiMaterializer {
    pub fn new(state: Arc<XdsState>) -> Result<Self> {
        let repository = state
            .api_definition_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("API definition repository is not configured"))?;

        let audit_repo = AuditLogRepository::new(repository.pool());

        Ok(Self { state, repository, audit_repo })
    }

    pub async fn create_definition(
        &self,
        spec: ApiDefinitionSpec,
    ) -> Result<CreateDefinitionOutcome> {
        ensure_domain_available(&self.repository, &spec.team, &spec.domain).await?;

        // Pre-check isolation conflicts to fail fast
        if spec.listener_isolation {
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository is not configured"))?;
            let params = spec.isolation_listener.as_ref().ok_or_else(|| {
                Error::validation("listener parameters are required for isolation mode")
            })?;

            if let Some(name) = &params.name {
                if let Ok(existing) = listener_repo.get_by_name(name).await {
                    if existing.address != params.bind_address
                        || existing.port.unwrap_or_default() as u32 != params.port
                    {
                        return Err(Error::validation(
                            "listener.name reuse must match bindAddress and port",
                        ));
                    }
                }
            } else {
                let current = listener_repo.list(Some(1000), None).await?;
                if current.iter().any(|l| {
                    l.address == params.bind_address
                        && l.port.unwrap_or_default() as u32 == params.port
                }) {
                    return Err(Error::validation(
                        "Requested listener address:port is already in use",
                    ));
                }
            }
        }

        // Create definition and routes (non-transactional), but compensate on listener failure
        let definition = self
            .repository
            .create_definition(CreateApiDefinitionRequest {
                team: spec.team.clone(),
                domain: spec.domain.clone(),
                listener_isolation: spec.listener_isolation,
                tls_config: spec.tls_config.clone(),
                metadata: None,
            })
            .await?;

        let mut created_routes = Vec::with_capacity(spec.routes.len());
        for (idx, route_spec) in spec.routes.iter().cloned().enumerate() {
            ensure_route_available(
                &self.repository,
                &definition.id,
                &route_spec.match_type,
                &route_spec.match_value,
            )
            .await?;
            let order = route_spec.route_order.unwrap_or(idx as i64);
            let created_route = self
                .repository
                .create_route(route_spec.into_request(&definition.id, order))
                .await?;
            created_routes.push(created_route);
        }

        if spec.listener_isolation {
            if let Err(err) = self
                .materialize_isolated_listener(
                    &definition,
                    &created_routes,
                    spec.isolation_listener.as_ref(),
                )
                .await
            {
                // Compensating delete to avoid partial writes
                let _ = self.repository.delete_definition(&definition.id).await;
                return Err(err);
            }
            self.state.refresh_listeners_from_repository().await?;
        }

        let (definition, bootstrap_uri) =
            bootstrap::persist_bootstrap_metadata(&self.repository, &definition, &created_routes)
                .await?;

        audit::record_create_event(
            &self.audit_repo,
            &definition.id,
            &definition.team,
            &definition.domain,
        )
        .await?;

        if spec.listener_isolation {
            self.state.refresh_listeners_from_repository().await?;
        }
        self.refresh_caches().await?;

        Ok(CreateDefinitionOutcome { definition, routes: created_routes, bootstrap_uri })
    }

    pub async fn append_route(
        &self,
        definition_id: &str,
        spec: RouteSpec,
    ) -> Result<AppendRouteOutcome> {
        let definition = self.repository.get_definition(definition_id).await?;
        let existing_routes = self.repository.list_routes(definition_id).await?;

        ensure_route_available(
            &self.repository,
            definition_id,
            &spec.match_type,
            &spec.match_value,
        )
        .await?;

        let order = spec.route_order.unwrap_or(existing_routes.len() as i64);

        let route = self.repository.create_route(spec.into_request(definition_id, order)).await?;

        let mut all_routes = existing_routes;
        all_routes.push(route.clone());

        let (definition, bootstrap_uri) =
            bootstrap::persist_bootstrap_metadata(&self.repository, &definition, &all_routes)
                .await?;

        audit::record_route_appended_event(
            &self.audit_repo,
            &definition.id,
            &route.id,
            &route.match_type,
            &route.match_value,
        )
        .await?;

        self.refresh_caches().await?;

        Ok(AppendRouteOutcome { definition, route, bootstrap_uri })
    }

    async fn refresh_caches(&self) -> Result<()> {
        self.state.refresh_routes_from_repository().await?;
        self.state.refresh_platform_api_resources().await?;
        Ok(())
    }

    async fn materialize_isolated_listener(
        &self,
        definition: &ApiDefinitionData,
        routes: &[ApiRouteData],
        listener: Option<&ListenerInput>,
    ) -> Result<()> {
        use crate::platform_api::filter_overrides::typed_per_filter_config;
        use crate::storage::CreateListenerRequest;
        use crate::xds::listener::ListenerConfig as XListenerConfig;
        use crate::xds::route::{
            PathMatch, RouteActionConfig, RouteConfig as XRouteConfig, RouteMatchConfig, RouteRule,
            VirtualHostConfig,
        };

        let listener_repo = self
            .state
            .listener_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Listener repository is not configured"))?;

        let params = listener.ok_or_else(|| {
            Error::validation("listener parameters are required for isolation mode")
        })?;

        // Build a dedicated route configuration
        let route_config_name = format!("platform-api-{}", short_id(&definition.id));
        let mut vhost = VirtualHostConfig {
            name: format!("{}-vhost", short_id(&definition.id)),
            domains: vec![definition.domain.clone()],
            routes: Vec::with_capacity(routes.len()),
            typed_per_filter_config: Default::default(),
        };

        for route in routes {
            let cluster_name = build_cluster_name(&definition.id, &route.id);
            let action = RouteActionConfig::Cluster {
                name: cluster_name,
                timeout: route.timeout_seconds.map(|v| v as u64),
                prefix_rewrite: route.rewrite_prefix.clone(),
                path_template_rewrite: route.rewrite_regex.clone(),
            };

            let path = match route.match_type.to_lowercase().as_str() {
                "prefix" => PathMatch::Prefix(route.match_value.clone()),
                "path" | "exact" => PathMatch::Exact(route.match_value.clone()),
                other => {
                    return Err(Error::validation(format!(
                        "Unsupported route match type '{}' for isolated listener",
                        other
                    )));
                }
            };

            vhost.routes.push(RouteRule {
                name: Some(format!("platform-api-{}", short_id(&route.id))),
                r#match: RouteMatchConfig { path, headers: None, query_parameters: None },
                action,
                typed_per_filter_config: typed_per_filter_config(&route.override_config)?,
            });
        }

        let route_config =
            XRouteConfig { name: route_config_name.clone(), virtual_hosts: vec![vhost] };

        // Build listener config
        let listener_name = params
            .name
            .clone()
            .unwrap_or_else(|| format!("platform-{}-listener", short_id(&definition.id)));

        let listener_config = XListenerConfig {
            name: listener_name.clone(),
            address: params.bind_address.clone(),
            port: params.port,
            filter_chains: vec![crate::xds::listener::FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![crate::xds::listener::FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: crate::xds::listener::FilterType::HttpConnectionManager {
                        route_config_name: None,
                        inline_route_config: Some(route_config),
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        };

        let mut listener_value = serde_json::to_value(&listener_config).map_err(|e| {
            Error::internal(format!("Failed to serialize isolated listener config: {}", e))
        })?;
        if let Some(obj) = listener_value.as_object_mut() {
            obj.insert(
                "flowplaneGateway".to_string(),
                serde_json::json!({ "team": definition.team }),
            );
        }

        if !listener_repo.exists_by_name(&listener_name).await.unwrap_or(false) {
            let _created_listener = listener_repo
                .create(CreateListenerRequest {
                    name: listener_name,
                    address: params.bind_address.clone(),
                    port: Some(params.port as i64),
                    protocol: Some(params.protocol.clone()),
                    configuration: listener_value,
                })
                .await?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ListenerInput {
    pub name: Option<String>,
    pub bind_address: String,
    pub port: u32,
    pub protocol: String,
    pub tls_config: Option<Value>,
}

fn build_cluster_name(definition_id: &str, route_id: &str) -> String {
    format!("platform-{}-{}", short_id(definition_id), short_id(route_id))
}

fn short_id(id: &str) -> String {
    let candidate: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
    if candidate.is_empty() {
        "platform".into()
    } else {
        candidate.to_lowercase()
    }
}
