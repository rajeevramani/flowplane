//! Route View Extractor Service
//!
//! This service extracts UI view data from route configuration JSON at runtime.
//! It provides a flattened view of routes without requiring database schema changes.
//!
//! This is part of the "Option A" prototype approach where all UI fields are
//! derived at runtime from existing JSON configuration.

#[allow(unused_imports)]
use chrono::{DateTime, Utc};

use crate::api::dto::route_view::{RouteListStatsDto, RouteListViewDto};
use crate::domain::RouteMatchType;
use crate::storage::repositories::{RouteMetadataData, RouteWithRelatedData};
use crate::storage::{RouteConfigData, RouteData, VirtualHostData};
use crate::xds::route::{RouteActionConfig, RouteConfig, RouteRule};

/// Extracted action fields from route configuration JSON.
#[derive(Debug, Clone, Default)]
pub struct ExtractedActionFields {
    /// Primary upstream cluster
    pub upstream_cluster: Option<String>,
    /// Fallback cluster (for weighted clusters, secondary)
    pub fallback_cluster: Option<String>,
    /// Request timeout in seconds
    pub timeout_seconds: Option<u64>,
    /// Path prefix rewrite rule
    pub prefix_rewrite: Option<String>,
    /// Path template rewrite rule
    pub template_rewrite: Option<String>,
}

/// Service for extracting UI view data from route configuration JSON.
pub struct RouteViewExtractor;

impl RouteViewExtractor {
    /// Extract a flattened route view from normalized data + JSON configuration.
    ///
    /// # Arguments
    /// * `route` - Route data from the normalized `routes` table
    /// * `virtual_host` - Virtual host data from the normalized `virtual_hosts` table
    /// * `route_config` - Route config data containing the JSON configuration
    /// * `mcp_enabled` - Whether MCP is enabled for this route
    /// * `mcp_tool_name` - Name of the MCP tool (if enabled)
    /// * `filter_count` - Number of filters attached to this route
    /// * `metadata` - Optional route metadata (from OpenAPI import or learning)
    ///
    /// # Returns
    /// A `RouteListViewDto` with all fields populated from existing data.
    pub fn extract_route_view(
        route: &RouteData,
        virtual_host: &VirtualHostData,
        route_config: &RouteConfigData,
        mcp_enabled: bool,
        mcp_tool_name: Option<String>,
        filter_count: i32,
        metadata: Option<&RouteMetadataData>,
    ) -> RouteListViewDto {
        // Extract action fields from configuration JSON
        let action_fields = Self::extract_action_fields_from_config(
            &route_config.configuration,
            &virtual_host.name,
            &route.name,
        );

        RouteListViewDto {
            // Identity fields
            route_id: route.id.to_string(),
            route_name: route.name.clone(),
            virtual_host_id: virtual_host.id.to_string(),
            virtual_host_name: virtual_host.name.clone(),
            route_config_id: route_config.id.to_string(),
            route_config_name: route_config.name.clone(),

            // From routes table
            path_pattern: route.path_pattern.clone(),
            match_type: route.match_type,
            rule_order: route.rule_order,

            // From virtual_hosts table
            domains: virtual_host.domains.clone(),

            // Extracted from JSON configuration
            upstream_cluster: action_fields.upstream_cluster,
            fallback_cluster: action_fields.fallback_cluster,
            http_methods: Self::extract_http_methods(metadata),
            timeout_seconds: action_fields.timeout_seconds,
            prefix_rewrite: action_fields.prefix_rewrite,

            // From related tables
            mcp_enabled,
            mcp_tool_name,
            filter_count,

            // From metadata table
            operation_id: metadata.and_then(|m| m.operation_id.clone()),
            summary: metadata.and_then(|m| m.summary.clone()),

            // Timestamps
            created_at: route.created_at,
            updated_at: route.updated_at,
        }
    }

    /// Extract action fields from route configuration JSON.
    ///
    /// Parses the JSON configuration and finds the matching route by virtual host
    /// and route name, then extracts cluster, timeout, and rewrite settings.
    fn extract_action_fields_from_config(
        config_json: &str,
        virtual_host_name: &str,
        route_name: &str,
    ) -> ExtractedActionFields {
        // Parse the configuration JSON
        let config: RouteConfig = match serde_json::from_str(config_json) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    virtual_host = %virtual_host_name,
                    route = %route_name,
                    "Failed to parse route configuration JSON"
                );
                return ExtractedActionFields::default();
            }
        };

        // Find the matching virtual host
        let vh_config = config.virtual_hosts.iter().find(|vh| vh.name == virtual_host_name);

        let vh_config = match vh_config {
            Some(vh) => vh,
            None => {
                tracing::debug!(
                    virtual_host = %virtual_host_name,
                    "Virtual host not found in configuration"
                );
                return ExtractedActionFields::default();
            }
        };

        // Find the matching route
        let route_config = vh_config.routes.iter().find(|r| {
            r.name.as_deref() == Some(route_name)
                || r.name.is_none() && Self::matches_generated_name(r, route_name)
        });

        match route_config {
            Some(r) => Self::extract_from_action(&r.action),
            None => {
                tracing::debug!(
                    route = %route_name,
                    virtual_host = %virtual_host_name,
                    "Route not found in configuration"
                );
                ExtractedActionFields::default()
            }
        }
    }

    /// Check if a route matches a generated name (for routes without explicit names).
    fn matches_generated_name(route: &RouteRule, generated_name: &str) -> bool {
        use crate::xds::route::PathMatch;

        let (match_type, path_pattern) = match &route.r#match.path {
            PathMatch::Prefix(p) => (RouteMatchType::Prefix, p.clone()),
            PathMatch::Exact(p) => (RouteMatchType::Exact, p.clone()),
            PathMatch::Regex(p) => (RouteMatchType::Regex, p.clone()),
            PathMatch::Template(p) => (RouteMatchType::PathTemplate, p.clone()),
        };

        let expected_name = match_type.generate_rule_name(&path_pattern);
        expected_name == generated_name
    }

    /// Extract action fields from a RouteActionConfig.
    fn extract_from_action(action: &RouteActionConfig) -> ExtractedActionFields {
        match action {
            RouteActionConfig::Cluster {
                name,
                timeout,
                prefix_rewrite,
                path_template_rewrite,
                ..
            } => ExtractedActionFields {
                upstream_cluster: Some(name.clone()),
                fallback_cluster: None,
                timeout_seconds: *timeout,
                prefix_rewrite: prefix_rewrite.clone(),
                template_rewrite: path_template_rewrite.clone(),
            },
            RouteActionConfig::WeightedClusters { clusters, .. } => {
                // For weighted clusters, find primary (highest weight) and secondary
                let mut sorted_clusters = clusters.clone();
                sorted_clusters.sort_by(|a, b| b.weight.cmp(&a.weight));

                let primary = sorted_clusters.first().map(|c| c.name.clone());
                let secondary = sorted_clusters.get(1).map(|c| c.name.clone());

                ExtractedActionFields {
                    upstream_cluster: primary,
                    fallback_cluster: secondary,
                    timeout_seconds: None,
                    prefix_rewrite: None,
                    template_rewrite: None,
                }
            }
            RouteActionConfig::Redirect { .. } => {
                // Redirects don't have cluster information
                ExtractedActionFields::default()
            }
        }
    }

    /// Extract HTTP methods from route metadata.
    ///
    /// If metadata contains http_method (from OpenAPI import), use that.
    /// Otherwise, return empty vec (meaning all methods allowed).
    fn extract_http_methods(metadata: Option<&RouteMetadataData>) -> Vec<String> {
        metadata
            .and_then(|m| m.http_method.as_ref())
            .map(|method: &String| vec![method.to_uppercase()])
            .unwrap_or_default()
    }

    /// Compute aggregate statistics for the route list.
    ///
    /// # Arguments
    /// * `total_routes` - Total number of routes
    /// * `total_virtual_hosts` - Total number of virtual hosts
    /// * `total_route_configs` - Total number of route configurations
    /// * `mcp_enabled_count` - Number of routes with MCP enabled
    /// * `unique_clusters` - Number of unique upstream clusters (computed from views)
    /// * `unique_domains` - Number of unique domains (computed from views)
    pub fn compute_stats(
        total_routes: i64,
        total_virtual_hosts: i64,
        total_route_configs: i64,
        mcp_enabled_count: i64,
        unique_clusters: i64,
        unique_domains: i64,
    ) -> RouteListStatsDto {
        RouteListStatsDto {
            total_routes,
            total_virtual_hosts,
            total_route_configs,
            mcp_enabled_count,
            unique_clusters,
            unique_domains,
        }
    }

    /// Compute unique clusters from a list of route views.
    pub fn count_unique_clusters(views: &[RouteListViewDto]) -> i64 {
        use std::collections::HashSet;

        let clusters: HashSet<_> =
            views.iter().filter_map(|v| v.upstream_cluster.as_ref()).collect();

        clusters.len() as i64
    }

    /// Compute unique domains from a list of route views.
    pub fn count_unique_domains(views: &[RouteListViewDto]) -> i64 {
        use std::collections::HashSet;

        let domains: HashSet<_> = views.iter().flat_map(|v| v.domains.iter()).collect();

        domains.len() as i64
    }

    /// Extract a flattened route view from the optimized RouteWithRelatedData struct.
    ///
    /// This method is used with the optimized JOIN query that fetches all related data
    /// in a single database call, avoiding the N+1 query problem.
    ///
    /// # Arguments
    /// * `data` - Route data with all related entities pre-loaded
    /// * `metadata` - Optional route metadata (from OpenAPI import or learning)
    ///
    /// # Returns
    /// A `RouteListViewDto` with all fields populated from the pre-loaded data.
    pub fn extract_from_related_data(
        data: &RouteWithRelatedData,
        metadata: Option<&RouteMetadataData>,
    ) -> RouteListViewDto {
        // Extract action fields from configuration JSON
        let action_fields = Self::extract_action_fields_from_config(
            &data.configuration,
            &data.virtual_host_name,
            &data.route_name,
        );

        RouteListViewDto {
            // Identity fields
            route_id: data.route_id.to_string(),
            route_name: data.route_name.clone(),
            virtual_host_id: data.virtual_host_id.to_string(),
            virtual_host_name: data.virtual_host_name.clone(),
            route_config_id: data.route_config_id.to_string(),
            route_config_name: data.route_config_name.clone(),

            // From routes table
            path_pattern: data.path_pattern.clone(),
            match_type: data.match_type,
            rule_order: data.rule_order,

            // From virtual_hosts table
            domains: data.domains.clone(),

            // Extracted from JSON configuration
            upstream_cluster: action_fields.upstream_cluster,
            fallback_cluster: action_fields.fallback_cluster,
            http_methods: Self::extract_http_methods(metadata),
            timeout_seconds: action_fields.timeout_seconds,
            prefix_rewrite: action_fields.prefix_rewrite,

            // From related tables (pre-loaded)
            mcp_enabled: data.mcp_enabled,
            mcp_tool_name: data.mcp_tool_name.clone(),
            filter_count: data.filter_count,

            // From metadata table
            operation_id: metadata.and_then(|m| m.operation_id.clone()),
            summary: metadata.and_then(|m| m.summary.clone()),

            // Timestamps
            created_at: data.route_created_at,
            updated_at: data.route_updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RouteConfigId, RouteId, VirtualHostId};

    #[allow(dead_code)]
    fn create_test_route_data() -> RouteData {
        RouteData {
            id: RouteId::new(),
            virtual_host_id: VirtualHostId::new(),
            name: "api-users".to_string(),
            path_pattern: "/api/users".to_string(),
            match_type: RouteMatchType::Prefix,
            rule_order: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[allow(dead_code)]
    fn create_test_virtual_host_data(route_config_id: &RouteConfigId) -> VirtualHostData {
        VirtualHostData {
            id: VirtualHostId::new(),
            route_config_id: route_config_id.clone(),
            name: "default".to_string(),
            domains: vec!["api.example.com".to_string()],
            rule_order: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[allow(dead_code)]
    fn create_test_route_config_json() -> String {
        r#"{
            "name": "primary-routes",
            "virtualHosts": [
                {
                    "name": "default",
                    "domains": ["api.example.com"],
                    "routes": [
                        {
                            "name": "api-users",
                            "match": { "path": { "Prefix": "/api/users" } },
                            "action": {
                                "Cluster": {
                                    "name": "users-service",
                                    "timeout": 30,
                                    "prefix_rewrite": "/internal/users"
                                }
                            },
                            "typed_per_filter_config": {}
                        }
                    ],
                    "typed_per_filter_config": {}
                }
            ]
        }"#
        .to_string()
    }

    #[allow(dead_code)]
    fn create_test_route_config_data() -> RouteConfigData {
        RouteConfigData {
            id: RouteConfigId::new(),
            name: "primary-routes".to_string(),
            path_prefix: "/".to_string(),
            cluster_name: "users-service".to_string(),
            configuration: create_test_route_config_json(),
            version: 1,
            source: "native_api".to_string(),
            team: Some("test-team".to_string()),
            import_id: None,
            route_order: None,
            headers: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_extract_from_cluster_action() {
        let action = RouteActionConfig::Cluster {
            name: "users-service".to_string(),
            timeout: Some(30),
            prefix_rewrite: Some("/internal".to_string()),
            path_template_rewrite: None,
            retry_policy: None,
        };

        let fields = RouteViewExtractor::extract_from_action(&action);

        assert_eq!(fields.upstream_cluster, Some("users-service".to_string()));
        assert_eq!(fields.fallback_cluster, None);
        assert_eq!(fields.timeout_seconds, Some(30));
        assert_eq!(fields.prefix_rewrite, Some("/internal".to_string()));
    }

    #[test]
    fn test_extract_from_weighted_clusters_action() {
        use crate::xds::route::WeightedClusterConfig;

        let action = RouteActionConfig::WeightedClusters {
            clusters: vec![
                WeightedClusterConfig {
                    name: "primary".to_string(),
                    weight: 80,
                    typed_per_filter_config: std::collections::HashMap::new(),
                },
                WeightedClusterConfig {
                    name: "canary".to_string(),
                    weight: 20,
                    typed_per_filter_config: std::collections::HashMap::new(),
                },
            ],
            total_weight: Some(100),
        };

        let fields = RouteViewExtractor::extract_from_action(&action);

        assert_eq!(fields.upstream_cluster, Some("primary".to_string()));
        assert_eq!(fields.fallback_cluster, Some("canary".to_string()));
        assert_eq!(fields.timeout_seconds, None);
    }

    #[test]
    fn test_extract_from_redirect_action() {
        let action = RouteActionConfig::Redirect {
            host_redirect: Some("new.example.com".to_string()),
            path_redirect: Some("/new-path".to_string()),
            response_code: Some(301),
        };

        let fields = RouteViewExtractor::extract_from_action(&action);

        assert_eq!(fields.upstream_cluster, None);
        assert_eq!(fields.fallback_cluster, None);
    }

    #[test]
    fn test_count_unique_clusters() {
        let views = vec![
            RouteListViewDto {
                route_id: "1".to_string(),
                route_name: "r1".to_string(),
                virtual_host_id: "vh1".to_string(),
                virtual_host_name: "default".to_string(),
                route_config_id: "rc1".to_string(),
                route_config_name: "config1".to_string(),
                path_pattern: "/api".to_string(),
                match_type: RouteMatchType::Prefix,
                rule_order: 0,
                domains: vec![],
                upstream_cluster: Some("cluster-a".to_string()),
                fallback_cluster: None,
                http_methods: vec![],
                timeout_seconds: None,
                prefix_rewrite: None,
                mcp_enabled: false,
                mcp_tool_name: None,
                filter_count: 0,
                operation_id: None,
                summary: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            RouteListViewDto {
                route_id: "2".to_string(),
                route_name: "r2".to_string(),
                virtual_host_id: "vh1".to_string(),
                virtual_host_name: "default".to_string(),
                route_config_id: "rc1".to_string(),
                route_config_name: "config1".to_string(),
                path_pattern: "/users".to_string(),
                match_type: RouteMatchType::Prefix,
                rule_order: 1,
                domains: vec![],
                upstream_cluster: Some("cluster-a".to_string()), // Same cluster
                fallback_cluster: None,
                http_methods: vec![],
                timeout_seconds: None,
                prefix_rewrite: None,
                mcp_enabled: false,
                mcp_tool_name: None,
                filter_count: 0,
                operation_id: None,
                summary: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            RouteListViewDto {
                route_id: "3".to_string(),
                route_name: "r3".to_string(),
                virtual_host_id: "vh1".to_string(),
                virtual_host_name: "default".to_string(),
                route_config_id: "rc1".to_string(),
                route_config_name: "config1".to_string(),
                path_pattern: "/orders".to_string(),
                match_type: RouteMatchType::Prefix,
                rule_order: 2,
                domains: vec![],
                upstream_cluster: Some("cluster-b".to_string()), // Different cluster
                fallback_cluster: None,
                http_methods: vec![],
                timeout_seconds: None,
                prefix_rewrite: None,
                mcp_enabled: false,
                mcp_tool_name: None,
                filter_count: 0,
                operation_id: None,
                summary: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ];

        let unique_count = RouteViewExtractor::count_unique_clusters(&views);
        assert_eq!(unique_count, 2); // cluster-a and cluster-b
    }

    #[test]
    fn test_count_unique_domains() {
        let views = vec![
            RouteListViewDto {
                route_id: "1".to_string(),
                route_name: "r1".to_string(),
                virtual_host_id: "vh1".to_string(),
                virtual_host_name: "default".to_string(),
                route_config_id: "rc1".to_string(),
                route_config_name: "config1".to_string(),
                path_pattern: "/api".to_string(),
                match_type: RouteMatchType::Prefix,
                rule_order: 0,
                domains: vec!["api.example.com".to_string(), "api.test.com".to_string()],
                upstream_cluster: None,
                fallback_cluster: None,
                http_methods: vec![],
                timeout_seconds: None,
                prefix_rewrite: None,
                mcp_enabled: false,
                mcp_tool_name: None,
                filter_count: 0,
                operation_id: None,
                summary: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            RouteListViewDto {
                route_id: "2".to_string(),
                route_name: "r2".to_string(),
                virtual_host_id: "vh2".to_string(),
                virtual_host_name: "web".to_string(),
                route_config_id: "rc1".to_string(),
                route_config_name: "config1".to_string(),
                path_pattern: "/".to_string(),
                match_type: RouteMatchType::Prefix,
                rule_order: 0,
                domains: vec!["api.example.com".to_string(), "www.example.com".to_string()], // api.example.com is duplicate
                upstream_cluster: None,
                fallback_cluster: None,
                http_methods: vec![],
                timeout_seconds: None,
                prefix_rewrite: None,
                mcp_enabled: false,
                mcp_tool_name: None,
                filter_count: 0,
                operation_id: None,
                summary: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ];

        let unique_count = RouteViewExtractor::count_unique_domains(&views);
        assert_eq!(unique_count, 3); // api.example.com, api.test.com, www.example.com
    }
}
