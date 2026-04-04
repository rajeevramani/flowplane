//! Agent RBAC filter injection for listeners with external route grants.
//!
//! Builds Envoy RBAC filter configurations from `grants` rows of type `route`,
//! enabling per-agent, per-route, per-method access control at the Envoy data plane.
//!
//! # Architecture
//!
//! 1. For each listener, query `grants` joined with `routes`, `users`, and the
//!    route → vhost → route_config → listener_route_configs chain.
//! 2. Build a `RbacConfig` with one policy per grant (principal = x-flowplane-sub header,
//!    permission = path + optional method OR-rule).
//! 3. If any grants exist, inject the RBAC filter into the listener's HCM chain.
//!    JWT sub forwarding (`x-flowplane-sub` header) is expected to be set up by the
//!    existing JWT filter attached to the listener via the normal filter attachment flow.

use std::collections::HashMap;

use crate::xds::filters::any_from_message;
use crate::xds::filters::http::jwt_auth::{
    JwtClaimToHeaderConfig, JwtJwksSourceConfig, JwtProviderConfig, RemoteJwksConfig,
    RemoteJwksHttpUriConfig,
};
use crate::xds::filters::http::rbac::{
    PermissionRule, PrincipalRule, RbacAction, RbacConfig, RbacPolicy, RbacRulesConfig,
};
use crate::xds::helpers::ListenerModifier;
use crate::xds::resources::BuiltResource;
use crate::Result;
use envoy_types::pb::envoy::config::core::v3::{data_source, DataSource};
use envoy_types::pb::envoy::extensions::filters::http::lua::v3::Lua as LuaFilter;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType as HttpFilterConfigType;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter;
use tracing::{debug, warn};

/// A route grant joined with the agent's Zitadel sub and route path data,
/// required for generating the xDS RBAC filter configuration.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentGrantWithRoute {
    pub agent_id: String,
    /// The agent's Zitadel `sub` claim, forwarded by JWT filter as `x-flowplane-sub`.
    pub agent_zitadel_sub: String,
    pub route_id: String,
    /// The route path pattern matched by RBAC.
    pub route_path: String,
    /// HTTP methods allowed for this grant (NULL = all methods allowed).
    pub allowed_methods: Option<Vec<String>>,
}

/// Build an Envoy RBAC HTTP filter config from a set of route grants.
///
/// Returns `None` when `grants` is empty (no RBAC filter needed).
///
/// Each grant becomes one named policy:
/// - Principal: exact match on `x-flowplane-sub` header (forwarded JWT sub claim)
/// - Permission: route path match AND (optionally) method OR-rule
pub fn build_rbac_config_for_listener(grants: &[AgentGrantWithRoute]) -> Option<RbacConfig> {
    if grants.is_empty() {
        return None;
    }

    let mut policies = HashMap::new();

    for grant in grants {
        let policy_name = format!("agent-{}-route-{}", grant.agent_id, grant.route_id);

        // Principal: match the agent's Zitadel sub forwarded as a request header
        let principal = PrincipalRule::Header {
            name: "x-flowplane-sub".to_string(),
            exact_match: Some(grant.agent_zitadel_sub.clone()),
            prefix_match: None,
        };

        // Permission: route path, optionally combined with an HTTP method restriction
        let path_permission =
            PermissionRule::UrlPath { path: grant.route_path.clone(), ignore_case: false };

        let permission = match &grant.allowed_methods {
            Some(methods) if !methods.is_empty() => {
                let method_rules: Vec<PermissionRule> = methods
                    .iter()
                    .map(|m| PermissionRule::Header {
                        name: ":method".to_string(),
                        exact_match: Some(m.clone()),
                        prefix_match: None,
                        suffix_match: None,
                        present_match: None,
                    })
                    .collect();

                PermissionRule::AndRules {
                    rules: vec![path_permission, PermissionRule::OrRules { rules: method_rules }],
                }
            }
            _ => {
                // No method restriction: match any HTTP method on this path
                path_permission
            }
        };

        policies.insert(
            policy_name,
            RbacPolicy { permissions: vec![permission], principals: vec![principal] },
        );
    }

    Some(RbacConfig {
        rules: Some(RbacRulesConfig { action: RbacAction::Allow, policies }),
        ..Default::default()
    })
}

/// Build a JWT provider configuration that forwards the `sub` claim as `x-flowplane-sub`.
///
/// This is used when generating JWT filter configurations for listeners that serve
/// routes with `exposure = 'external'`. The forwarded header is then used by the
/// agent RBAC filter to identify which agent is making the request.
///
/// # Arguments
///
/// * `issuer` - JWT issuer URL (e.g. Zitadel base URL)
/// * `audience` - Expected audience claim in the JWT
/// * `jwks_cluster` - Envoy cluster name for JWKS endpoint resolution
pub fn build_jwt_provider_for_agent_auth(
    issuer: &str,
    audience: &str,
    jwks_cluster: &str,
) -> JwtProviderConfig {
    JwtProviderConfig {
        issuer: Some(issuer.to_string()),
        audiences: vec![audience.to_string()],
        claim_to_headers: vec![JwtClaimToHeaderConfig {
            claim_name: "sub".to_string(),
            header_name: "x-flowplane-sub".to_string(),
        }],
        payload_in_metadata: Some("jwt_payload".to_string()),
        jwks: JwtJwksSourceConfig::Remote(RemoteJwksConfig {
            http_uri: RemoteJwksHttpUriConfig {
                uri: format!("{}/oauth/v2/keys", issuer),
                cluster: jwks_cluster.to_string(),
                timeout_ms: 5000,
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Load all `route`-type grants for a specific listener's routes from the database.
///
/// The join chain traces: grants → users → routes → virtual_hosts →
/// route_configs → listener_route_configs → listeners (by name).
pub async fn load_route_grants_for_listener(
    listener_name: &str,
    pool: &crate::storage::DbPool,
) -> Vec<AgentGrantWithRoute> {
    let rows: Vec<AgentGrantWithRoute> = sqlx::query_as(
        "SELECT g.principal_id AS agent_id, \
                u.zitadel_sub   AS agent_zitadel_sub, \
                g.route_id, \
                r.path_pattern  AS route_path, \
                g.allowed_methods \
         FROM grants g \
         JOIN users u   ON u.id  = g.principal_id \
         JOIN routes r  ON r.id  = g.route_id \
         JOIN virtual_hosts vh  ON vh.id = r.virtual_host_id \
         JOIN route_configs rc  ON rc.id = vh.route_config_id \
         JOIN listener_route_configs lrc ON lrc.route_config_id = rc.id \
         JOIN listeners l ON l.id = lrc.listener_id \
         WHERE g.grant_type = 'route' \
           AND r.exposure     = 'external' \
           AND l.name         = $1 \
           AND (g.expires_at IS NULL OR g.expires_at > NOW())",
    )
    .bind(listener_name)
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| {
        warn!(
            listener = %listener_name,
            error = %e,
            "Failed to load route grants for listener"
        );
        Vec::new()
    });

    rows
}

/// Build an Envoy Lua HTTP filter that strips `x-flowplane-sub` from incoming requests.
///
/// This filter must be placed at the start of the HCM filter chain — before the JWT filter —
/// so that a client-supplied `x-flowplane-sub` header cannot spoof the JWT-derived value.
/// The JWT filter will re-set the header with the validated `sub` claim after this runs.
pub fn build_lua_header_sanitizer_filter() -> HttpFilter {
    let lua = LuaFilter {
        default_source_code: Some(DataSource {
            specifier: Some(data_source::Specifier::InlineString(
                "function envoy_on_request(handle)\n  handle:headers():remove(\"x-flowplane-sub\")\nend\n"
                    .to_string(),
            )),
            watched_directory: None,
        }),
        ..Default::default()
    };

    let lua_any =
        any_from_message("type.googleapis.com/envoy.extensions.filters.http.lua.v3.Lua", &lua);

    HttpFilter {
        name: "envoy.filters.http.lua.sub-sanitizer".to_string(),
        config_type: Some(HttpFilterConfigType::TypedConfig(lua_any)),
        disabled: false,
        is_optional: false,
    }
}

/// Inject agent RBAC filters into built listener resources that have active route grants.
///
/// For each listener, queries the database for route grants, builds an RBAC config,
/// and injects it into the listener's HCM filter chain (before the router filter).
/// Listeners without grants are left unchanged.
pub async fn inject_agent_rbac_filters(
    built_listeners: &mut [BuiltResource],
    pool: &crate::storage::DbPool,
) -> Result<()> {
    for built in built_listeners.iter_mut() {
        let grants = load_route_grants_for_listener(&built.name, pool).await;

        let rbac_config = match build_rbac_config_for_listener(&grants) {
            Some(cfg) => cfg,
            None => {
                debug!(listener = %built.name, "No route grants for listener, skipping RBAC injection");
                continue;
            }
        };

        // Convert RbacConfig to Envoy Any proto
        let rbac_any = match rbac_config.to_any() {
            Ok(any) => any,
            Err(e) => {
                warn!(
                    listener = %built.name,
                    error = %e,
                    "Failed to convert RBAC config to Envoy Any, skipping"
                );
                continue;
            }
        };

        let rbac_filter = HttpFilter {
            name: "envoy.filters.http.rbac".to_string(),
            config_type: Some(
                envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(rbac_any),
            ),
            disabled: false,
            is_optional: false,
        };

        let mut modifier = match ListenerModifier::decode(&built.resource.value, &built.name) {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    listener = %built.name,
                    error = %e,
                    "Failed to decode listener for RBAC injection"
                );
                continue;
            }
        };

        match modifier.add_filter_before_router(rbac_filter, false) {
            Ok(_) => {}
            Err(e) => {
                warn!(
                    listener = %built.name,
                    error = %e,
                    "Failed to inject RBAC filter into listener"
                );
                continue;
            }
        }

        // Inject Lua sanitizer at position 0 (before JWT filter) to strip any
        // client-supplied x-flowplane-sub header before JWT validation sets it.
        let lua_filter = build_lua_header_sanitizer_filter();
        match modifier.for_each_hcm(|hcm, _| {
            hcm.http_filters.insert(0, lua_filter.clone());
            Ok(true)
        }) {
            Ok(_) => {}
            Err(e) => {
                warn!(
                    listener = %built.name,
                    error = %e,
                    "Failed to inject Lua header sanitizer into listener"
                );
                continue;
            }
        }

        if let Some(encoded) = modifier.finish_if_modified() {
            built.resource.value = encoded;
            tracing::info!(
                listener = %built.name,
                grant_count = grants.len(),
                "Injected agent RBAC filter into listener"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grant(
        agent_id: &str,
        zitadel_sub: &str,
        route_id: &str,
        route_path: &str,
        allowed_methods: Option<Vec<String>>,
    ) -> AgentGrantWithRoute {
        AgentGrantWithRoute {
            agent_id: agent_id.to_string(),
            agent_zitadel_sub: zitadel_sub.to_string(),
            route_id: route_id.to_string(),
            route_path: route_path.to_string(),
            allowed_methods,
        }
    }

    #[test]
    fn test_build_rbac_empty_grants_returns_none() {
        let result = build_rbac_config_for_listener(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_rbac_single_grant_no_methods() {
        let grants = vec![make_grant("agent-1", "sub-abc", "route-1", "/api/v1", None)];
        let result = build_rbac_config_for_listener(&grants).unwrap();

        let rules = result.rules.unwrap();
        assert_eq!(rules.action, RbacAction::Allow);
        assert_eq!(rules.policies.len(), 1);

        let policy = rules.policies.values().next().unwrap();
        assert_eq!(policy.principals.len(), 1);
        assert_eq!(policy.permissions.len(), 1);

        // Principal must be a header match on x-flowplane-sub
        match &policy.principals[0] {
            PrincipalRule::Header { name, exact_match, .. } => {
                assert_eq!(name, "x-flowplane-sub");
                assert_eq!(exact_match.as_deref(), Some("sub-abc"));
            }
            other => panic!("Expected Header principal, got {:?}", other),
        }

        // Permission must be a UrlPath match (no method restriction)
        match &policy.permissions[0] {
            PermissionRule::UrlPath { path, .. } => {
                assert_eq!(path, "/api/v1");
            }
            other => panic!("Expected UrlPath permission, got {:?}", other),
        }
    }

    #[test]
    fn test_build_rbac_grant_with_methods() {
        let grants = vec![make_grant(
            "agent-2",
            "sub-xyz",
            "route-2",
            "/api/orders",
            Some(vec!["GET".to_string(), "POST".to_string()]),
        )];
        let result = build_rbac_config_for_listener(&grants).unwrap();

        let rules = result.rules.unwrap();
        let policy = rules.policies.values().next().unwrap();

        // Permission should be AndRules(UrlPath, OrRules(GET, POST))
        match &policy.permissions[0] {
            PermissionRule::AndRules { rules } => {
                assert_eq!(rules.len(), 2);
                match &rules[0] {
                    PermissionRule::UrlPath { path, .. } => assert_eq!(path, "/api/orders"),
                    other => panic!("Expected UrlPath, got {:?}", other),
                }
                match &rules[1] {
                    PermissionRule::OrRules { rules: method_rules } => {
                        assert_eq!(method_rules.len(), 2);
                    }
                    other => panic!("Expected OrRules for methods, got {:?}", other),
                }
            }
            other => panic!("Expected AndRules permission, got {:?}", other),
        }
    }

    #[test]
    fn test_build_rbac_multiple_grants_distinct_policies() {
        let grants = vec![
            make_grant("agent-1", "sub-a", "route-1", "/api/v1", None),
            make_grant("agent-2", "sub-b", "route-2", "/api/v2", Some(vec!["GET".to_string()])),
        ];
        let result = build_rbac_config_for_listener(&grants).unwrap();
        assert_eq!(result.rules.unwrap().policies.len(), 2);
    }

    #[test]
    fn test_validate_exposure_in_route_repo() {
        use crate::storage::repositories::route::validate_exposure;
        assert!(validate_exposure("internal").is_ok());
        assert!(validate_exposure("external").is_ok());
        assert!(validate_exposure("public").is_err());
        assert!(validate_exposure("").is_err());
    }

    #[test]
    fn test_lua_sanitizer_filter_has_correct_name() {
        let filter = build_lua_header_sanitizer_filter();
        assert_eq!(filter.name, "envoy.filters.http.lua.sub-sanitizer");
        assert!(!filter.disabled);
        assert!(!filter.is_optional);
    }

    #[test]
    fn test_lua_sanitizer_filter_has_typed_config_with_lua_type_url() {
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType;

        let filter = build_lua_header_sanitizer_filter();
        match &filter.config_type {
            Some(ConfigType::TypedConfig(any)) => {
                assert!(
                    any.type_url.contains("lua"),
                    "Expected Lua type URL, got: {}",
                    any.type_url
                );
                assert!(!any.value.is_empty(), "Lua filter proto should be non-empty");
            }
            _ => panic!("Expected TypedConfig on Lua filter"),
        }
    }

    #[test]
    fn test_lua_sanitizer_filter_code_removes_x_flowplane_sub() {
        use envoy_types::pb::envoy::extensions::filters::http::lua::v3::Lua;
        use prost::Message;
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType;

        let filter = build_lua_header_sanitizer_filter();
        if let Some(ConfigType::TypedConfig(any)) = &filter.config_type {
            let lua = Lua::decode(&any.value[..]).expect("should decode Lua proto");
            let source = lua.default_source_code.expect("default_source_code must be set");
            if let Some(data_source::Specifier::InlineString(code)) = source.specifier {
                assert!(
                    code.contains("x-flowplane-sub"),
                    "Lua code must remove x-flowplane-sub header"
                );
                assert!(code.contains("remove"), "Lua code must call remove() on the header");
            } else {
                panic!("Expected InlineString specifier");
            }
        } else {
            panic!("Expected TypedConfig");
        }
    }

    #[test]
    fn test_lua_sanitizer_injected_before_rbac_when_grants_present() {
        use crate::xds::helpers::ListenerModifier;
        use envoy_types::pb::envoy::config::core::v3::{
            address::Address as AddressType, Address, SocketAddress,
        };
        use envoy_types::pb::envoy::config::listener::v3::{Filter, FilterChain, Listener};
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::{
            HttpConnectionManager, HttpFilter as HcmFilter,
        };
        use envoy_types::pb::google::protobuf::Any as PbAny;
        use prost::Message;

        // Build a minimal listener with JWT + router (typical state before RBAC injection)
        let jwt_filter = HcmFilter {
            name: "envoy.filters.http.jwt_authn".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };
        let router_filter = HcmFilter {
            name: "envoy.filters.http.router".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };
        let hcm = HttpConnectionManager {
            stat_prefix: "test".to_string(),
            http_filters: vec![jwt_filter, router_filter],
            ..Default::default()
        };
        let hcm_any = PbAny {
            type_url: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager".to_string(),
            value: hcm.encode_to_vec(),
        };
        let filter = Filter {
            name: "envoy.filters.network.http_connection_manager".to_string(),
            config_type: Some(
                envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(
                    hcm_any,
                ),
            ),
        };
        let listener = Listener {
            name: "test-listener".to_string(),
            filter_chains: vec![FilterChain { filters: vec![filter], ..Default::default() }],
            address: Some(Address {
                address: Some(AddressType::SocketAddress(SocketAddress {
                    address: "0.0.0.0".to_string(),
                    ..Default::default()
                })),
            }),
            ..Default::default()
        };
        let bytes = listener.encode_to_vec();

        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        // Simulate RBAC injection (before router)
        let rbac_filter = HcmFilter {
            name: "envoy.filters.http.rbac".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };
        modifier.add_filter_before_router(rbac_filter, false).unwrap();

        // Simulate Lua injection at position 0
        let lua_filter = build_lua_header_sanitizer_filter();
        modifier
            .for_each_hcm(|hcm, _| {
                hcm.http_filters.insert(0, lua_filter.clone());
                Ok(true)
            })
            .unwrap();

        let encoded = modifier.finish_if_modified().unwrap();
        let decoded = Listener::decode(&encoded[..]).unwrap();
        if let Some(
            envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(tc),
        ) = &decoded.filter_chains[0].filters[0].config_type
        {
            let hcm = HttpConnectionManager::decode(&tc.value[..]).unwrap();
            // Expected order: Lua, JWT, RBAC, router
            assert_eq!(hcm.http_filters.len(), 4);
            assert_eq!(hcm.http_filters[0].name, "envoy.filters.http.lua.sub-sanitizer");
            assert_eq!(hcm.http_filters[1].name, "envoy.filters.http.jwt_authn");
            assert_eq!(hcm.http_filters[2].name, "envoy.filters.http.rbac");
            assert_eq!(hcm.http_filters[3].name, "envoy.filters.http.router");
        } else {
            panic!("Expected TypedConfig on HCM filter");
        }
    }
}
