//! Cluster validation and conversion utilities

use crate::{
    api::error::ApiError, services::ClusterService, storage::ClusterData, xds::ClusterSpec,
};

use super::types::{CreateClusterBody, HealthCheckRequest};

/// Internal struct for cluster configuration parts
#[derive(Debug)]
pub(super) struct ClusterConfigParts {
    pub(super) name: String,
    pub(super) service_name: String,
    pub(super) config: ClusterSpec,
}

/// Convert create cluster request body to cluster config parts
pub(super) fn cluster_parts_from_body(payload: CreateClusterBody) -> ClusterConfigParts {
    let CreateClusterBody {
        team: _,
        name,
        service_name,
        endpoints,
        connect_timeout_seconds,
        use_tls,
        tls_server_name,
        dns_lookup_family,
        lb_policy,
        health_checks,
        circuit_breakers,
        outlier_detection,
        protocol_type,
    } = payload;

    let service_name = service_name.unwrap_or_else(|| name.clone());

    let config = ClusterSpec {
        endpoints: endpoints
            .into_iter()
            .map(|ep| crate::xds::EndpointSpec::Address { host: ep.host, port: ep.port })
            .collect(),
        connect_timeout_seconds,
        use_tls,
        tls_server_name,
        dns_lookup_family,
        lb_policy,
        health_checks: health_checks
            .into_iter()
            .map(|hc| {
                let HealthCheckRequest {
                    r#type,
                    path,
                    host,
                    method,
                    interval_seconds,
                    timeout_seconds,
                    healthy_threshold,
                    unhealthy_threshold,
                    expected_statuses,
                } = hc;

                match r#type.to_lowercase().as_str() {
                    "tcp" => crate::xds::HealthCheckSpec::Tcp {
                        interval_seconds,
                        timeout_seconds,
                        healthy_threshold,
                        unhealthy_threshold,
                    },
                    _ => crate::xds::HealthCheckSpec::Http {
                        path: path.unwrap_or_else(|| "/health".to_string()),
                        host,
                        method,
                        interval_seconds,
                        timeout_seconds,
                        healthy_threshold,
                        unhealthy_threshold,
                        expected_statuses,
                    },
                }
            })
            .collect(),
        circuit_breakers: circuit_breakers.map(|cb| crate::xds::CircuitBreakersSpec {
            default: cb.default.map(|d| crate::xds::CircuitBreakerThresholdsSpec {
                max_connections: d.max_connections,
                max_pending_requests: d.max_pending_requests,
                max_requests: d.max_requests,
                max_retries: d.max_retries,
            }),
            high: cb.high.map(|h| crate::xds::CircuitBreakerThresholdsSpec {
                max_connections: h.max_connections,
                max_pending_requests: h.max_pending_requests,
                max_requests: h.max_requests,
                max_retries: h.max_retries,
            }),
        }),
        outlier_detection: outlier_detection.map(|od| crate::xds::OutlierDetectionSpec {
            consecutive_5xx: od.consecutive_5xx,
            interval_seconds: od.interval_seconds,
            base_ejection_time_seconds: od.base_ejection_time_seconds,
            max_ejection_percent: od.max_ejection_percent,
        }),
        protocol_type,
        ..Default::default()
    };

    ClusterConfigParts { name, service_name, config }
}

/// Convert cluster database data to response DTO
pub(super) fn cluster_response_from_data(
    service: &ClusterService,
    data: ClusterData,
) -> Result<super::types::ClusterResponse, ApiError> {
    let config = service.parse_config(&data).map_err(ApiError::from)?;
    Ok(super::types::ClusterResponse {
        name: data.name,
        team: data.team.unwrap_or_else(|| "unknown".to_string()),
        service_name: data.service_name,
        import_id: data.import_id,
        config,
    })
}
