use crate::validation::requests::{
    ValidatedCreateClusterRequest, ValidatedEndpointRequest, ValidatedHealthCheckRequest,
    ValidatedUpdateClusterRequest,
};
use crate::xds::cluster;

impl From<ValidatedCreateClusterRequest> for cluster::ClusterConfig {
    fn from(validated: ValidatedCreateClusterRequest) -> Self {
        Self {
            name: validated.name,
            endpoints: validated.endpoints.into_iter().map(Into::into).collect(),
            load_balancing_policy: validated
                .lb_policy
                .map(|policy| match policy.as_str() {
                    "ROUND_ROBIN" => cluster::LoadBalancingPolicy::RoundRobin,
                    "LEAST_REQUEST" => cluster::LoadBalancingPolicy::LeastRequest,
                    "RANDOM" => cluster::LoadBalancingPolicy::Random,
                    "PASS_THROUGH" => cluster::LoadBalancingPolicy::PassThrough,
                    _ => cluster::LoadBalancingPolicy::RoundRobin,
                })
                .unwrap_or(cluster::LoadBalancingPolicy::RoundRobin),
            connect_timeout: validated.connect_timeout_seconds,
            health_checks: validated.health_check.map(|hc| vec![hc.into()]),
        }
    }
}

impl From<ValidatedUpdateClusterRequest> for cluster::ClusterConfig {
    fn from(validated: ValidatedUpdateClusterRequest) -> Self {
        Self {
            name: String::new(),
            endpoints: validated.endpoints.into_iter().map(Into::into).collect(),
            load_balancing_policy: validated
                .lb_policy
                .map(|policy| match policy.as_str() {
                    "ROUND_ROBIN" => cluster::LoadBalancingPolicy::RoundRobin,
                    "LEAST_REQUEST" => cluster::LoadBalancingPolicy::LeastRequest,
                    "RANDOM" => cluster::LoadBalancingPolicy::Random,
                    "PASS_THROUGH" => cluster::LoadBalancingPolicy::PassThrough,
                    _ => cluster::LoadBalancingPolicy::RoundRobin,
                })
                .unwrap_or(cluster::LoadBalancingPolicy::RoundRobin),
            connect_timeout: validated.connect_timeout_seconds,
            health_checks: validated.health_check.map(|hc| vec![hc.into()]),
        }
    }
}

impl From<ValidatedEndpointRequest> for cluster::EndpointConfig {
    fn from(validated: ValidatedEndpointRequest) -> Self {
        Self {
            address: validated.address,
            port: validated.port,
            weight: validated.weight,
        }
    }
}

impl From<ValidatedHealthCheckRequest> for cluster::HealthCheckConfig {
    fn from(validated: ValidatedHealthCheckRequest) -> Self {
        Self {
            timeout: validated.timeout_seconds,
            interval: validated.interval_seconds,
            healthy_threshold: validated.healthy_threshold,
            unhealthy_threshold: validated.unhealthy_threshold,
            path: validated.path,
        }
    }
}

impl From<cluster::ClusterConfig> for ValidatedCreateClusterRequest {
    fn from(cluster_config: cluster::ClusterConfig) -> Self {
        Self {
            name: cluster_config.name,
            endpoints: cluster_config.endpoints.into_iter().map(Into::into).collect(),
            lb_policy: Some(match cluster_config.load_balancing_policy {
                cluster::LoadBalancingPolicy::RoundRobin => "ROUND_ROBIN".to_string(),
                cluster::LoadBalancingPolicy::LeastRequest => "LEAST_REQUEST".to_string(),
                cluster::LoadBalancingPolicy::Random => "RANDOM".to_string(),
                cluster::LoadBalancingPolicy::PassThrough => "PASS_THROUGH".to_string(),
            }),
            connect_timeout_seconds: cluster_config.connect_timeout,
            health_check: cluster_config
                .health_checks
                .and_then(|checks| checks.into_iter().next())
                .map(Into::into),
            circuit_breaker: None,
        }
    }
}

impl From<cluster::EndpointConfig> for ValidatedEndpointRequest {
    fn from(endpoint_config: cluster::EndpointConfig) -> Self {
        Self {
            address: endpoint_config.address,
            port: endpoint_config.port,
            weight: endpoint_config.weight,
        }
    }
}

impl From<cluster::HealthCheckConfig> for ValidatedHealthCheckRequest {
    fn from(health_check_config: cluster::HealthCheckConfig) -> Self {
        Self {
            timeout_seconds: health_check_config.timeout,
            interval_seconds: health_check_config.interval,
            healthy_threshold: health_check_config.healthy_threshold,
            unhealthy_threshold: health_check_config.unhealthy_threshold,
            path: health_check_config.path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_conversion() {
        let validated_request = ValidatedCreateClusterRequest {
            name: "test-cluster".to_string(),
            endpoints: vec![ValidatedEndpointRequest {
                address: "127.0.0.1".to_string(),
                port: 8080,
                weight: Some(100),
            }],
            lb_policy: Some("ROUND_ROBIN".to_string()),
            connect_timeout_seconds: Some(5),
            health_check: Some(ValidatedHealthCheckRequest {
                timeout_seconds: 5,
                interval_seconds: 10,
                healthy_threshold: 2,
                unhealthy_threshold: 3,
                path: Some("/health".to_string()),
            }),
            circuit_breaker: None,
        };

        let cluster_config: cluster::ClusterConfig = validated_request.into();

        assert_eq!(cluster_config.name, "test-cluster");
        assert_eq!(cluster_config.endpoints.len(), 1);
        assert_eq!(cluster_config.endpoints[0].address, "127.0.0.1");
        assert_eq!(cluster_config.endpoints[0].port, 8080);
        assert_eq!(cluster_config.endpoints[0].weight, Some(100));
        assert!(matches!(
            cluster_config.load_balancing_policy,
            cluster::LoadBalancingPolicy::RoundRobin
        ));
        assert_eq!(cluster_config.connect_timeout, Some(5));
        assert!(cluster_config.health_checks.is_some());
    }
}
