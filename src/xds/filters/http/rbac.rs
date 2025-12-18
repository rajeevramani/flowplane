//! RBAC (Role-Based Access Control) HTTP filter configuration
//!
//! This module provides configuration types for the Envoy RBAC filter,
//! which enforces access control based on policies with permissions and principals.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::rbac::v3::{
    Permission, Policy, Principal, Rbac as RbacRulesProto,
};
use envoy_types::pb::envoy::extensions::filters::http::rbac::v3::{
    Rbac as RbacProto, RbacPerRoute as RbacPerRouteProto,
};
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// Type URLs for RBAC filter configuration
pub const RBAC_TYPE_URL: &str = "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC";
pub const RBAC_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBACPerRoute";

/// RBAC action type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RbacAction {
    /// Allow requests that match the policy
    #[default]
    Allow,
    /// Deny requests that match the policy
    Deny,
    /// Log requests that match the policy (shadow mode)
    Log,
}

impl RbacAction {
    fn to_proto_value(self) -> i32 {
        match self {
            Self::Allow => 0,
            Self::Deny => 1,
            Self::Log => 2,
        }
    }

    fn from_proto_value(value: i32) -> Self {
        match value {
            1 => Self::Deny,
            2 => Self::Log,
            _ => Self::Allow,
        }
    }
}

/// Permission rule for RBAC
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionRule {
    /// Match any request
    Any { any: bool },
    /// Match requests with specific header
    Header {
        name: String,
        #[serde(default)]
        exact_match: Option<String>,
        #[serde(default)]
        prefix_match: Option<String>,
        #[serde(default)]
        suffix_match: Option<String>,
        #[serde(default)]
        present_match: Option<bool>,
    },
    /// Match requests to specific URL path
    UrlPath {
        path: String,
        #[serde(default)]
        ignore_case: bool,
    },
    /// Match requests with specific destination port
    DestinationPort { port: u32 },
    /// Match requests with specific metadata
    Metadata { filter: String, path: Vec<String> },
    /// AND of multiple permissions
    #[schema(no_recursion)]
    AndRules { rules: Vec<PermissionRule> },
    /// OR of multiple permissions
    #[schema(no_recursion)]
    OrRules { rules: Vec<PermissionRule> },
    /// NOT of a permission
    #[schema(no_recursion)]
    NotRule { rule: Box<PermissionRule> },
}

impl Default for PermissionRule {
    fn default() -> Self {
        Self::Any { any: true }
    }
}

impl PermissionRule {
    fn to_proto(&self) -> Result<Permission, crate::Error> {
        use envoy_types::pb::envoy::config::rbac::v3::permission::Rule;
        use envoy_types::pb::envoy::config::route::v3::HeaderMatcher;
        use envoy_types::pb::envoy::r#type::matcher::v3::{PathMatcher, StringMatcher};

        let rule = match self {
            Self::Any { any } => Rule::Any(*any),
            Self::Header {
                name,
                exact_match,
                prefix_match,
                suffix_match,
                present_match,
            } => {
                let header_match_specifier = if let Some(exact) = exact_match {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::ExactMatch(
                            exact.clone(),
                        ),
                    )
                } else if let Some(prefix) = prefix_match {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::PrefixMatch(
                            prefix.clone(),
                        ),
                    )
                } else if let Some(suffix) = suffix_match {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::SuffixMatch(
                            suffix.clone(),
                        ),
                    )
                } else if let Some(present) = present_match {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::PresentMatch(
                            *present,
                        ),
                    )
                } else {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::PresentMatch(
                            true,
                        ),
                    )
                };

                Rule::Header(HeaderMatcher {
                    name: name.clone(),
                    header_match_specifier,
                    invert_match: false,
                    treat_missing_header_as_empty: false,
                })
            }
            Self::UrlPath { path, ignore_case } => Rule::UrlPath(PathMatcher {
                rule: Some(
                    envoy_types::pb::envoy::r#type::matcher::v3::path_matcher::Rule::Path(
                        StringMatcher {
                            match_pattern: Some(
                                envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                    path.clone(),
                                ),
                            ),
                            ignore_case: *ignore_case,
                        },
                    ),
                ),
            }),
            Self::DestinationPort { port } => Rule::DestinationPort(*port),
            Self::Metadata { .. } => {
                // Simplified - metadata matching is complex
                Rule::Any(true)
            }
            Self::AndRules { rules } => {
                let permissions: Result<Vec<_>, _> =
                    rules.iter().map(|r| r.to_proto()).collect();
                Rule::AndRules(envoy_types::pb::envoy::config::rbac::v3::permission::Set {
                    rules: permissions?,
                })
            }
            Self::OrRules { rules } => {
                let permissions: Result<Vec<_>, _> =
                    rules.iter().map(|r| r.to_proto()).collect();
                Rule::OrRules(envoy_types::pb::envoy::config::rbac::v3::permission::Set {
                    rules: permissions?,
                })
            }
            Self::NotRule { rule } => Rule::NotRule(Box::new(rule.to_proto()?)),
        };

        Ok(Permission { rule: Some(rule) })
    }
}

/// Principal rule for RBAC (who is making the request)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PrincipalRule {
    /// Match any principal
    Any { any: bool },
    /// Match authenticated principals
    Authenticated {
        #[serde(default)]
        principal_name: Option<String>,
    },
    /// Match by source IP
    SourceIp { address_prefix: String, prefix_len: u32 },
    /// Match by direct remote IP
    DirectRemoteIp { address_prefix: String, prefix_len: u32 },
    /// Match by request header
    Header {
        name: String,
        #[serde(default)]
        exact_match: Option<String>,
        #[serde(default)]
        prefix_match: Option<String>,
    },
    /// AND of multiple principals
    #[schema(no_recursion)]
    AndIds { ids: Vec<PrincipalRule> },
    /// OR of multiple principals
    #[schema(no_recursion)]
    OrIds { ids: Vec<PrincipalRule> },
    /// NOT of a principal
    #[schema(no_recursion)]
    NotId { id: Box<PrincipalRule> },
}

impl Default for PrincipalRule {
    fn default() -> Self {
        Self::Any { any: true }
    }
}

impl PrincipalRule {
    fn to_proto(&self) -> Result<Principal, crate::Error> {
        use envoy_types::pb::envoy::config::core::v3::CidrRange;
        use envoy_types::pb::envoy::config::rbac::v3::principal::Identifier;
        use envoy_types::pb::envoy::config::route::v3::HeaderMatcher;

        let identifier = match self {
            Self::Any { any } => Identifier::Any(*any),
            Self::Authenticated { principal_name } => {
                Identifier::Authenticated(
                    envoy_types::pb::envoy::config::rbac::v3::principal::Authenticated {
                        principal_name: principal_name.as_ref().map(|name| {
                            envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                                match_pattern: Some(
                                    envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                        name.clone(),
                                    ),
                                ),
                                ignore_case: false,
                            }
                        }),
                    },
                )
            }
            Self::SourceIp {
                address_prefix,
                prefix_len,
            } => Identifier::SourceIp(CidrRange {
                address_prefix: address_prefix.clone(),
                prefix_len: Some(envoy_types::pb::google::protobuf::UInt32Value {
                    value: *prefix_len,
                }),
            }),
            Self::DirectRemoteIp {
                address_prefix,
                prefix_len,
            } => Identifier::DirectRemoteIp(CidrRange {
                address_prefix: address_prefix.clone(),
                prefix_len: Some(envoy_types::pb::google::protobuf::UInt32Value {
                    value: *prefix_len,
                }),
            }),
            Self::Header {
                name,
                exact_match,
                prefix_match,
            } => {
                let header_match_specifier = if let Some(exact) = exact_match {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::ExactMatch(
                            exact.clone(),
                        ),
                    )
                } else if let Some(prefix) = prefix_match {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::PrefixMatch(
                            prefix.clone(),
                        ),
                    )
                } else {
                    Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::PresentMatch(
                            true,
                        ),
                    )
                };

                Identifier::Header(HeaderMatcher {
                    name: name.clone(),
                    header_match_specifier,
                    invert_match: false,
                    treat_missing_header_as_empty: false,
                })
            }
            Self::AndIds { ids } => {
                let principals: Result<Vec<_>, _> = ids.iter().map(|p| p.to_proto()).collect();
                Identifier::AndIds(envoy_types::pb::envoy::config::rbac::v3::principal::Set {
                    ids: principals?,
                })
            }
            Self::OrIds { ids } => {
                let principals: Result<Vec<_>, _> = ids.iter().map(|p| p.to_proto()).collect();
                Identifier::OrIds(envoy_types::pb::envoy::config::rbac::v3::principal::Set {
                    ids: principals?,
                })
            }
            Self::NotId { id } => Identifier::NotId(Box::new(id.to_proto()?)),
        };

        Ok(Principal { identifier: Some(identifier) })
    }
}

/// RBAC policy definition
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RbacPolicy {
    /// Permissions for this policy (what actions are allowed)
    #[serde(default)]
    pub permissions: Vec<PermissionRule>,
    /// Principals for this policy (who can perform the actions)
    #[serde(default)]
    pub principals: Vec<PrincipalRule>,
}

impl Default for RbacPolicy {
    fn default() -> Self {
        Self {
            permissions: vec![PermissionRule::Any { any: true }],
            principals: vec![PrincipalRule::Any { any: true }],
        }
    }
}

/// RBAC rules configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct RbacRulesConfig {
    /// The action to take when a policy matches
    #[serde(default)]
    pub action: RbacAction,
    /// Named policies
    #[serde(default)]
    pub policies: HashMap<String, RbacPolicy>,
}

impl RbacRulesConfig {
    fn to_proto(&self) -> Result<RbacRulesProto, crate::Error> {
        let mut policies = HashMap::new();

        for (name, policy) in &self.policies {
            let permissions: Result<Vec<_>, _> =
                policy.permissions.iter().map(|p| p.to_proto()).collect();
            let principals: Result<Vec<_>, _> =
                policy.principals.iter().map(|p| p.to_proto()).collect();

            policies.insert(
                name.clone(),
                Policy {
                    permissions: permissions?,
                    principals: principals?,
                    condition: None,
                    checked_condition: None,
                    cel_config: None,
                },
            );
        }

        Ok(RbacRulesProto {
            action: self.action.to_proto_value(),
            policies,
            audit_logging_options: None,
        })
    }
}

/// RBAC HTTP filter configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct RbacConfig {
    /// Primary RBAC rules
    #[serde(default)]
    pub rules: Option<RbacRulesConfig>,
    /// Stat prefix for rules
    #[serde(default)]
    pub rules_stat_prefix: Option<String>,
    /// Shadow rules for testing without enforcement
    #[serde(default)]
    pub shadow_rules: Option<RbacRulesConfig>,
    /// Stat prefix for shadow rules
    #[serde(default)]
    pub shadow_rules_stat_prefix: Option<String>,
    /// Track per-rule statistics
    #[serde(default)]
    pub track_per_rule_stats: bool,
}

impl RbacConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.rules.is_none() && self.shadow_rules.is_none() {
            return Err(invalid_config("RBAC filter requires at least rules or shadow_rules"));
        }
        Ok(())
    }

    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let rules = self.rules.as_ref().map(|r| r.to_proto()).transpose()?;

        let shadow_rules = self.shadow_rules.as_ref().map(|r| r.to_proto()).transpose()?;

        let proto = RbacProto {
            rules,
            rules_stat_prefix: self.rules_stat_prefix.clone().unwrap_or_default(),
            shadow_rules,
            shadow_rules_stat_prefix: self.shadow_rules_stat_prefix.clone().unwrap_or_default(),
            track_per_rule_stats: self.track_per_rule_stats,
            matcher: None,
            shadow_matcher: None,
        };

        Ok(any_from_message(RBAC_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &RbacProto) -> Result<Self, crate::Error> {
        // Simplified - full parsing would require reverse conversion
        Ok(Self {
            rules: proto.rules.as_ref().map(|r| RbacRulesConfig {
                action: RbacAction::from_proto_value(r.action),
                policies: HashMap::new(), // Simplified
            }),
            rules_stat_prefix: if proto.rules_stat_prefix.is_empty() {
                None
            } else {
                Some(proto.rules_stat_prefix.clone())
            },
            shadow_rules: proto.shadow_rules.as_ref().map(|r| RbacRulesConfig {
                action: RbacAction::from_proto_value(r.action),
                policies: HashMap::new(), // Simplified
            }),
            shadow_rules_stat_prefix: if proto.shadow_rules_stat_prefix.is_empty() {
                None
            } else {
                Some(proto.shadow_rules_stat_prefix.clone())
            },
            track_per_rule_stats: proto.track_per_rule_stats,
        })
    }
}

/// Per-route RBAC configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum RbacPerRouteConfig {
    /// Disable RBAC for this route
    Disabled { disabled: bool },
    /// Override RBAC rules for this route
    Override {
        #[serde(default)]
        rbac: Option<RbacRulesConfig>,
    },
}

impl Default for RbacPerRouteConfig {
    fn default() -> Self {
        Self::Disabled { disabled: false }
    }
}

impl RbacPerRouteConfig {
    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let proto = match self {
            Self::Disabled { disabled } => {
                if *disabled {
                    // To disable RBAC for a route, we set rbac to None
                    // (absent RBAC policy disables it for this route per Envoy docs)
                    RbacPerRouteProto { rbac: None }
                } else {
                    RbacPerRouteProto::default()
                }
            }
            Self::Override { rbac } => {
                // For per-route override, we wrap the config RBAC rules in an HTTP filter RBAC
                let rbac_proto = rbac
                    .as_ref()
                    .map(|r| {
                        r.to_proto().map(|rules| RbacProto {
                            rules: Some(rules),
                            rules_stat_prefix: String::new(),
                            shadow_rules: None,
                            shadow_rules_stat_prefix: String::new(),
                            track_per_rule_stats: false,
                            matcher: None,
                            shadow_matcher: None,
                        })
                    })
                    .transpose()?;
                RbacPerRouteProto { rbac: rbac_proto }
            }
        };

        Ok(any_from_message(RBAC_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &RbacPerRouteProto) -> Result<Self, crate::Error> {
        match &proto.rbac {
            None => Ok(Self::Disabled { disabled: true }),
            Some(rbac) => Ok(Self::Override {
                rbac: rbac.rules.as_ref().map(|r| RbacRulesConfig {
                    action: RbacAction::from_proto_value(r.action),
                    policies: HashMap::new(), // Simplified
                }),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_rbac_config_fails_validation() {
        let config = RbacConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_allow_all_policy() {
        let mut policies = HashMap::new();
        policies.insert(
            "allow-all".to_string(),
            RbacPolicy {
                permissions: vec![PermissionRule::Any { any: true }],
                principals: vec![PrincipalRule::Any { any: true }],
            },
        );

        let config = RbacConfig {
            rules: Some(RbacRulesConfig { action: RbacAction::Allow, policies }),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, RBAC_TYPE_URL);
    }

    #[test]
    fn test_deny_by_ip() {
        let mut policies = HashMap::new();
        policies.insert(
            "deny-internal".to_string(),
            RbacPolicy {
                permissions: vec![PermissionRule::Any { any: true }],
                principals: vec![PrincipalRule::SourceIp {
                    address_prefix: "10.0.0.0".to_string(),
                    prefix_len: 8,
                }],
            },
        );

        let config = RbacConfig {
            rules: Some(RbacRulesConfig { action: RbacAction::Deny, policies }),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, RBAC_TYPE_URL);
    }

    #[test]
    fn test_per_route_disabled() {
        let config = RbacPerRouteConfig::Disabled { disabled: true };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, RBAC_PER_ROUTE_TYPE_URL);
    }
}
