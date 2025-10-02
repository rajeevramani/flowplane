//! Transformers for converting between Native and Platform API representations

use serde_json::{json, Value};

use crate::{
    api::handlers::ClusterResponse,
    api::platform_service_handlers::{ServiceDefinition, ServiceEndpoint, ServiceResponse},
    api::platform_api_definitions::{ApiDefinitionResponse, UpstreamConfig, UpstreamEndpoint},
    xds::{ClusterSpec, EndpointSpec},
};

/// Transform a Native API cluster to a Platform API service
pub fn cluster_to_service(cluster: &ClusterSpec) -> ServiceResponse {
    let endpoints: Vec<ServiceEndpoint> = cluster.endpoints.iter().map(|ep| {
        // Parse host:port format
        let (host, port) = if let Some(colon_pos) = ep.host.rfind(':') {
            let host_part = &ep.host[..colon_pos];
            let port_part = ep.host[colon_pos + 1..].parse().unwrap_or(80);
            (host_part.to_string(), port_part)
        } else {
            (ep.host.clone(), 80)
        };

        ServiceEndpoint {
            host,
            port,
            weight: 100, // Default weight if not specified
        }
    }).collect();

    ServiceResponse {
        name: cluster.service_name.clone().unwrap_or_else(|| cluster.name.clone()),
        description: Some(format!("Native cluster: {}", cluster.name)),
        endpoints,
        load_balancing_strategy: cluster.lb_policy.clone().unwrap_or_else(|| "ROUND_ROBIN".to_string()),
        health_check: cluster.health_checks.first().map(|hc| {
            crate::api::platform_service_handlers::ServiceHealthCheck {
                r#type: hc.r#type.clone().unwrap_or_else(|| "http".to_string()),
                path: hc.path.clone(),
                interval_seconds: hc.interval_seconds.unwrap_or(10),
                timeout_seconds: hc.timeout_seconds.unwrap_or(5),
                healthy_threshold: hc.healthy_threshold.unwrap_or(2),
                unhealthy_threshold: hc.unhealthy_threshold.unwrap_or(3),
            }
        }),
        circuit_breaker: cluster.circuit_breakers.as_ref().and_then(|cb| {
            cb.thresholds.first().map(|threshold| {
                crate::api::platform_service_handlers::ServiceCircuitBreaker {
                    max_requests: threshold.max_requests,
                    max_pending_requests: threshold.max_pending_requests,
                    max_connections: threshold.max_connections,
                    max_retries: threshold.max_retries,
                }
            })
        }),
        outlier_detection: cluster.outlier_detection.as_ref().map(|od| {
            crate::api::platform_service_handlers::ServiceOutlierDetection {
                consecutive_5xx: od.consecutive_5xx,
                interval_seconds: od.interval.map(|i| i as u32),
                base_ejection_time_seconds: od.base_ejection_time.map(|t| t as u32),
                max_ejection_percent: od.max_ejection_percent,
                enforcing_consecutive_5xx: od.enforcing_consecutive_5xx,
            }
        }),
        metadata: json!({
            "source": "native_api",
            "cluster_name": cluster.name,
            "created_at": cluster.created_at,
            "updated_at": cluster.updated_at,
        }),
        created_at: cluster.created_at.clone().unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        updated_at: cluster.updated_at.clone().unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
    }
}

/// Transform a Platform API service to a Native API cluster response
pub fn service_to_cluster_response(service: &ServiceDefinition, cluster_name: &str) -> ClusterResponse {
    ClusterResponse {
        name: cluster_name.to_string(),
        service_name: Some(service.name.clone()),
        endpoints: service.endpoints.iter().map(|ep| EndpointSpec {
            host: format!("{}:{}", ep.host, ep.port),
        }).collect(),
        connect_timeout_seconds: Some(5), // Default
        use_tls: Some(false), // Default, could be derived from port
        tls_server_name: None,
        dns_lookup_family: None,
        lb_policy: Some(service.load_balancing_strategy.clone().unwrap_or_else(|| "ROUND_ROBIN".to_string())),
        health_checks: service.health_check.as_ref().map(|hc| vec![
            crate::xds::HealthCheckSpec {
                r#type: Some(hc.r#type.clone()),
                path: hc.path.clone(),
                host: None,
                method: None,
                interval_seconds: Some(hc.interval_seconds as u64),
                timeout_seconds: Some(hc.timeout_seconds as u64),
                healthy_threshold: Some(hc.healthy_threshold as u64),
                unhealthy_threshold: Some(hc.unhealthy_threshold as u64),
                expected_statuses: None,
            }
        ]).unwrap_or_default(),
        circuit_breakers: service.circuit_breaker.as_ref().map(|cb| {
            crate::xds::CircuitBreakersSpec {
                thresholds: vec![
                    crate::xds::CircuitBreakerThresholdsSpec {
                        priority: Some("DEFAULT".to_string()),
                        max_connections: cb.max_connections,
                        max_pending_requests: cb.max_pending_requests,
                        max_requests: cb.max_requests,
                        max_retries: cb.max_retries,
                    }
                ],
            }
        }),
        outlier_detection: service.outlier_detection.as_ref().map(|od| {
            crate::xds::OutlierDetectionSpec {
                consecutive_5xx: od.consecutive_5xx,
                interval: od.interval_seconds.map(|s| s as u64),
                base_ejection_time: od.base_ejection_time_seconds.map(|s| s as u64),
                max_ejection_percent: od.max_ejection_percent,
                enforcing_consecutive_5xx: od.enforcing_consecutive_5xx,
                enforcing_success_rate: None,
                success_rate_minimum_hosts: None,
                success_rate_request_volume: None,
                success_rate_stdev_factor: None,
                consecutive_gateway_failure: None,
                enforcing_consecutive_gateway_failure: None,
                split_external_local_origin_errors: None,
            }
        }),
    }
}

/// Transform route configurations to simplified API definition view
pub fn routes_to_api_summary(route_name: &str, route_spec: &Value) -> Value {
    // Extract virtual hosts and routes
    let virtual_hosts = route_spec.get("virtual_hosts")
        .and_then(|vh| vh.as_array())
        .map(|vhs| vhs.to_vec())
        .unwrap_or_default();

    let mut all_routes = Vec::new();
    let mut domains = Vec::new();

    for vh in &virtual_hosts {
        if let Some(vh_domains) = vh.get("domains").and_then(|d| d.as_array()) {
            for domain in vh_domains {
                if let Some(d) = domain.as_str() {
                    domains.push(d.to_string());
                }
            }
        }

        if let Some(routes) = vh.get("routes").and_then(|r| r.as_array()) {
            all_routes.extend(routes.iter().cloned());
        }
    }

    json!({
        "id": route_name,
        "name": route_name,
        "domains": domains,
        "routeCount": all_routes.len(),
        "routes": all_routes,
        "source": "native_api",
    })
}

/// Check if a cluster represents a Platform API service
pub fn is_platform_service_cluster(cluster_name: &str) -> bool {
    // Platform services typically have a "-cluster" suffix
    cluster_name.ends_with("-cluster") || cluster_name.contains("-service")
}

/// Extract service name from cluster name
pub fn cluster_name_to_service_name(cluster_name: &str) -> String {
    if cluster_name.ends_with("-cluster") {
        cluster_name.trim_end_matches("-cluster").to_string()
    } else if cluster_name.ends_with("-service") {
        cluster_name.to_string()
    } else {
        cluster_name.to_string()
    }
}

/// Check if a route configuration represents a Platform API definition
pub fn is_platform_api_routes(route_name: &str) -> bool {
    // Platform API definitions typically have a "-routes" suffix
    route_name.ends_with("-routes") || route_name.contains("-api-")
}

/// Transform multiple clusters to service list with filtering
pub fn clusters_to_services(clusters: Vec<ClusterSpec>) -> Vec<ServiceResponse> {
    clusters.into_iter()
        .map(|cluster| cluster_to_service(&cluster))
        .collect()
}

/// Create metadata for cross-API tracking
pub fn create_cross_api_metadata(source: &str, original_name: &str) -> Value {
    json!({
        "source": source,
        "original_name": original_name,
        "created_via": source,
        "managed_by": "flowplane",
        "cross_api_visible": true,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

/// Merge Platform API policies into Native API filter configuration
pub fn policies_to_filters(policies: &crate::api::platform_api_definitions::ApiPolicies) -> Value {
    let mut filters = json!({});

    // Rate limiting filter
    if let Some(rate_limit) = &policies.rate_limit {
        filters["envoy.filters.http.ratelimit"] = json!({
            "domain": "flowplane",
            "stage": 0,
            "request_type": "both",
            "timeout": "0.025s",
            "rate_limit_service": {
                "transport_api_version": "V3",
                "grpc_service": {
                    "envoy_grpc": {
                        "cluster_name": "rate_limit_cluster"
                    }
                }
            },
            "descriptors": [{
                "entries": [{
                    "key": "rate_limit",
                    "value": format!("{}/{}", rate_limit.requests, rate_limit.interval)
                }]
            }]
        });
    }

    // CORS filter
    if let Some(cors) = &policies.cors {
        filters["envoy.filters.http.cors"] = json!({
            "allow_origin_string_match": cors.origins.iter().map(|o| {
                json!({"exact": o})
            }).collect::<Vec<_>>(),
            "allow_methods": cors.methods.join(", "),
            "allow_headers": cors.headers.join(", "),
            "allow_credentials": cors.allow_credentials,
            "max_age": cors.max_age.map(|age| age.to_string()),
        });
    }

    // JWT authentication filter
    if let Some(auth) = &policies.authentication {
        if auth.auth_type == "jwt" {
            filters["envoy.filters.http.jwt_authn"] = json!({
                "providers": {
                    "provider": auth.config.clone().unwrap_or_else(|| json!({}))
                },
                "rules": [{
                    "match": {"prefix": "/"},
                    "requires": if auth.required {
                        json!({"provider_name": "provider"})
                    } else {
                        json!({})
                    }
                }]
            });
        }
    }

    filters
}