//! Route Hierarchy Synchronization Service
//!
//! This module provides synchronization of virtual hosts and routes
//! from RouteConfig JSON into normalized database tables.

use crate::domain::{RouteConfigId, RouteMatchType, VirtualHostId};
use crate::errors::Result;
use crate::storage::{
    CreateRouteRequest, CreateVirtualHostRequest, DbPool, RouteRepository, VirtualHostRepository,
};
use crate::xds::route::{PathMatch, RouteConfig};
use tracing::{debug, info, instrument};

/// Result of a route hierarchy sync operation.
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Number of virtual hosts created.
    pub virtual_hosts_created: usize,
    /// Number of virtual hosts updated.
    pub virtual_hosts_updated: usize,
    /// Number of virtual hosts deleted.
    pub virtual_hosts_deleted: usize,
    /// Number of routes created.
    pub routes_created: usize,
    /// Number of routes updated.
    pub routes_updated: usize,
    /// Number of routes deleted.
    pub routes_deleted: usize,
}

/// Service for synchronizing route hierarchy from JSON configuration.
#[derive(Debug, Clone)]
pub struct RouteHierarchySyncService {
    virtual_host_repo: VirtualHostRepository,
    route_repo: RouteRepository,
}

impl RouteHierarchySyncService {
    /// Creates a new sync service.
    pub fn new(pool: DbPool) -> Self {
        Self {
            virtual_host_repo: VirtualHostRepository::new(pool.clone()),
            route_repo: RouteRepository::new(pool),
        }
    }

    /// Synchronizes the route hierarchy for a given route configuration.
    ///
    /// This parses the RouteConfig JSON and extracts virtual hosts and routes
    /// into their respective database tables.
    #[instrument(skip(self, config), fields(route_config_id = %route_config_id), name = "sync_route_hierarchy")]
    pub async fn sync(
        &self,
        route_config_id: &RouteConfigId,
        config: &RouteConfig,
    ) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Get existing virtual hosts for this route config
        let existing_vhosts = self.virtual_host_repo.list_by_route_config(route_config_id).await?;
        let existing_vhost_names: std::collections::HashSet<_> =
            existing_vhosts.iter().map(|vh| vh.name.as_str()).collect();

        // Track which virtual hosts we've seen in the config
        let mut seen_vhost_names = std::collections::HashSet::new();

        // Process each virtual host in the config
        for (idx, vhost_config) in config.virtual_hosts.iter().enumerate() {
            seen_vhost_names.insert(vhost_config.name.as_str());

            // Check if this virtual host already exists
            let vhost_id = if existing_vhost_names.contains(vhost_config.name.as_str()) {
                // Get the existing virtual host ID
                let existing = self
                    .virtual_host_repo
                    .get_by_route_config_and_name(route_config_id, &vhost_config.name)
                    .await?;

                // Update if order or domains changed
                if existing.rule_order != idx as i32 || existing.domains != vhost_config.domains {
                    self.virtual_host_repo
                        .update(
                            &existing.id,
                            crate::storage::UpdateVirtualHostRequest {
                                domains: Some(vhost_config.domains.clone()),
                                rule_order: Some(idx as i32),
                            },
                        )
                        .await?;
                    result.virtual_hosts_updated += 1;
                }

                existing.id
            } else {
                // Create new virtual host
                let created = self
                    .virtual_host_repo
                    .create(CreateVirtualHostRequest {
                        route_config_id: route_config_id.clone(),
                        name: vhost_config.name.clone(),
                        domains: vhost_config.domains.clone(),
                        rule_order: idx as i32,
                    })
                    .await?;

                result.virtual_hosts_created += 1;
                created.id
            };

            // Sync routes for this virtual host
            let route_result = self.sync_routes(&vhost_id, &vhost_config.routes).await?;
            result.routes_created += route_result.routes_created;
            result.routes_updated += route_result.routes_updated;
            result.routes_deleted += route_result.routes_deleted;
        }

        // Delete virtual hosts that are no longer in the config
        for existing in &existing_vhosts {
            if !seen_vhost_names.contains(existing.name.as_str()) {
                // CASCADE delete will remove routes and filter attachments
                self.virtual_host_repo.delete(&existing.id).await?;
                result.virtual_hosts_deleted += 1;
            }
        }

        info!(
            route_config_id = %route_config_id,
            vhosts_created = result.virtual_hosts_created,
            vhosts_updated = result.virtual_hosts_updated,
            vhosts_deleted = result.virtual_hosts_deleted,
            routes_created = result.routes_created,
            routes_updated = result.routes_updated,
            routes_deleted = result.routes_deleted,
            "Route hierarchy synchronized"
        );

        Ok(result)
    }

    /// Synchronizes routes for a virtual host.
    async fn sync_routes(
        &self,
        virtual_host_id: &VirtualHostId,
        routes: &[crate::xds::route::RouteRule],
    ) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Get existing routes for this virtual host
        let existing_routes = self.route_repo.list_by_virtual_host(virtual_host_id).await?;
        let existing_route_names: std::collections::HashSet<_> =
            existing_routes.iter().map(|r| r.name.as_str()).collect();

        // Track which routes we've seen
        let mut seen_route_names = std::collections::HashSet::new();

        for (idx, route_config) in routes.iter().enumerate() {
            // Generate a route name if not provided
            let (match_type, path_pattern) = Self::extract_path_match(&route_config.r#match.path);
            let route_name = route_config
                .name
                .clone()
                .unwrap_or_else(|| match_type.generate_rule_name(&path_pattern));

            seen_route_names.insert(route_name.clone());

            if existing_route_names.contains(route_name.as_str()) {
                // Update existing route
                let existing =
                    self.route_repo.get_by_vh_and_name(virtual_host_id, &route_name).await?;

                if existing.path_pattern != path_pattern
                    || existing.match_type != match_type
                    || existing.rule_order != idx as i32
                {
                    self.route_repo
                        .update(
                            &existing.id,
                            crate::storage::UpdateRouteRequest {
                                path_pattern: Some(path_pattern.clone()),
                                match_type: Some(match_type),
                                rule_order: Some(idx as i32),
                            },
                        )
                        .await?;
                    result.routes_updated += 1;
                }
            } else {
                // Create new route
                self.route_repo
                    .create(CreateRouteRequest {
                        virtual_host_id: virtual_host_id.clone(),
                        name: route_name.clone(),
                        path_pattern,
                        match_type,
                        rule_order: idx as i32,
                    })
                    .await?;
                result.routes_created += 1;
            }
        }

        // Delete routes that are no longer in the config
        for existing in &existing_routes {
            if !seen_route_names.contains(existing.name.as_str()) {
                // CASCADE delete will remove filter attachments
                self.route_repo.delete(&existing.id).await?;
                result.routes_deleted += 1;
            }
        }

        debug!(
            virtual_host_id = %virtual_host_id,
            routes_created = result.routes_created,
            routes_updated = result.routes_updated,
            routes_deleted = result.routes_deleted,
            "Routes synchronized"
        );

        Ok(result)
    }

    /// Extracts the match type and path pattern from a PathMatch.
    fn extract_path_match(path_match: &PathMatch) -> (RouteMatchType, String) {
        match path_match {
            PathMatch::Prefix(p) => (RouteMatchType::Prefix, p.clone()),
            PathMatch::Exact(p) => (RouteMatchType::Exact, p.clone()),
            PathMatch::Regex(p) => (RouteMatchType::Regex, p.clone()),
            PathMatch::Template(p) => (RouteMatchType::PathTemplate, p.clone()),
        }
    }

    /// Clears all virtual hosts and routes for a route config.
    /// Used when a route config is deleted - CASCADE handles most of this,
    /// but this can be called explicitly if needed.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "clear_route_hierarchy")]
    pub async fn clear(&self, route_config_id: &RouteConfigId) -> Result<()> {
        let virtual_hosts = self.virtual_host_repo.list_by_route_config(route_config_id).await?;

        for vhost in virtual_hosts {
            // CASCADE delete handles routes and filter attachments
            self.virtual_host_repo.delete(&vhost.id).await?;
        }

        info!(route_config_id = %route_config_id, "Route hierarchy cleared");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::route::{
        PathMatch, RouteActionConfig, RouteConfig, RouteMatchConfig, RouteRule, VirtualHostConfig,
    };
    use std::collections::HashMap;

    fn create_test_config() -> RouteConfig {
        RouteConfig {
            name: "test-route-config".to_string(),
            virtual_hosts: vec![
                VirtualHostConfig {
                    name: "api-vhost".to_string(),
                    domains: vec!["api.example.com".to_string()],
                    routes: vec![
                        RouteRule {
                            name: Some("api-users".to_string()),
                            r#match: RouteMatchConfig {
                                path: PathMatch::Prefix("/api/users".to_string()),
                                headers: None,
                                query_parameters: None,
                            },
                            action: RouteActionConfig::Cluster {
                                name: "users-cluster".to_string(),
                                timeout: None,
                                prefix_rewrite: None,
                                path_template_rewrite: None,
                                retry_policy: None,
                            },
                            typed_per_filter_config: HashMap::new(),
                        },
                        RouteRule {
                            name: None, // Test auto-generated name
                            r#match: RouteMatchConfig {
                                path: PathMatch::Exact("/health".to_string()),
                                headers: None,
                                query_parameters: None,
                            },
                            action: RouteActionConfig::Cluster {
                                name: "health-cluster".to_string(),
                                timeout: None,
                                prefix_rewrite: None,
                                path_template_rewrite: None,
                                retry_policy: None,
                            },
                            typed_per_filter_config: HashMap::new(),
                        },
                    ],
                    typed_per_filter_config: HashMap::new(),
                },
                VirtualHostConfig {
                    name: "web-vhost".to_string(),
                    domains: vec!["www.example.com".to_string(), "example.com".to_string()],
                    routes: vec![RouteRule {
                        name: Some("web-root".to_string()),
                        r#match: RouteMatchConfig {
                            path: PathMatch::Prefix("/".to_string()),
                            headers: None,
                            query_parameters: None,
                        },
                        action: RouteActionConfig::Cluster {
                            name: "web-cluster".to_string(),
                            timeout: None,
                            prefix_rewrite: None,
                            path_template_rewrite: None,
                            retry_policy: None,
                        },
                        typed_per_filter_config: HashMap::new(),
                    }],
                    typed_per_filter_config: HashMap::new(),
                },
            ],
        }
    }

    #[test]
    fn test_extract_path_match() {
        let (match_type, path) =
            RouteHierarchySyncService::extract_path_match(&PathMatch::Prefix("/api".to_string()));
        assert_eq!(match_type, RouteMatchType::Prefix);
        assert_eq!(path, "/api");

        let (match_type, path) =
            RouteHierarchySyncService::extract_path_match(&PathMatch::Exact("/health".to_string()));
        assert_eq!(match_type, RouteMatchType::Exact);
        assert_eq!(path, "/health");

        let (match_type, path) = RouteHierarchySyncService::extract_path_match(&PathMatch::Regex(
            "^/users/[0-9]+$".to_string(),
        ));
        assert_eq!(match_type, RouteMatchType::Regex);
        assert_eq!(path, "^/users/[0-9]+$");

        let (match_type, path) = RouteHierarchySyncService::extract_path_match(
            &PathMatch::Template("/users/{id}".to_string()),
        );
        assert_eq!(match_type, RouteMatchType::PathTemplate);
        assert_eq!(path, "/users/{id}");
    }

    #[test]
    fn test_create_config_structure() {
        let config = create_test_config();
        assert_eq!(config.virtual_hosts.len(), 2);
        assert_eq!(config.virtual_hosts[0].routes.len(), 2);
        assert_eq!(config.virtual_hosts[1].routes.len(), 1);
    }
}
