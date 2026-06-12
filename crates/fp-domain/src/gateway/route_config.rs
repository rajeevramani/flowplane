//! RouteConfig: virtual hosts and route rules (spec/03 §7.2). Route actions reference
//! clusters by name within the same team; the service layer resolves and reference-tracks
//! them so a referenced cluster can never be silently deleted (v1's FK-by-name CASCADE
//! deleted whole route trees — spec/03 §8.6).

use crate::error::{DomainError, DomainResult};
use crate::id::{RouteConfigId, TeamId};
use crate::identity::validate_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_DOMAINS_PER_VHOST: usize = 50;
pub const MAX_VHOSTS: usize = 50;
pub const MAX_ROUTES_PER_VHOST: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteConfig {
    pub id: RouteConfigId,
    pub team_id: TeamId,
    pub name: String,
    pub spec: RouteConfigSpec,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfigSpec {
    pub virtual_hosts: Vec<VirtualHost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualHost {
    pub name: String,
    /// Host-header matches: valid hostnames, `*.suffix` wildcards, or literal `*`.
    pub domains: Vec<String>,
    pub routes: Vec<RouteRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRule {
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: PathMatch,
    pub action: RouteAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathMatch {
    Prefix {
        prefix: String,
    },
    Exact {
        path: String,
    },
    /// URI template, e.g. `/users/{id}` (Envoy uri_template matcher).
    Template {
        template: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteAction {
    /// Target cluster, by name, same team (resolved by the service layer).
    pub cluster: String,
    /// Replace the matched prefix. Never with a Template match (v1 rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_rewrite: Option<String>,
    /// Rewrite using template captures. Only with a Template match (v1 rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_rewrite: Option<String>,
    /// Upstream request timeout in seconds (1–300; default 15).
    #[serde(default = "default_route_timeout")]
    pub timeout_secs: u32,
}

fn default_route_timeout() -> u32 {
    15
}

fn valid_domain(domain: &str) -> bool {
    if domain == "*" {
        return true;
    }
    let body = domain.strip_prefix("*.").unwrap_or(domain);
    !body.is_empty()
        && domain.len() <= 253
        && !body.contains("..")
        && body.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

fn valid_path(label: &str, path: &str) -> DomainResult<()> {
    if !path.starts_with('/') || path.contains("..") || path.len() > 500 || path.contains('\0') {
        return Err(DomainError::validation(format!(
            "{label} must start with '/', contain no '..', and be <= 500 chars"
        )));
    }
    Ok(())
}

impl RouteConfigSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if self.virtual_hosts.is_empty() {
            return Err(DomainError::validation(
                "a route config needs at least one virtual host",
            ));
        }
        if self.virtual_hosts.len() > MAX_VHOSTS {
            return Err(DomainError::validation(format!(
                "at most {MAX_VHOSTS} virtual hosts, got {}",
                self.virtual_hosts.len()
            )));
        }
        let mut vhost_names = HashSet::new();
        for vhost in &self.virtual_hosts {
            validate_name(&vhost.name)?;
            if !vhost_names.insert(vhost.name.as_str()) {
                return Err(DomainError::validation(format!(
                    "duplicate virtual host \"{}\"",
                    vhost.name
                )));
            }
            if vhost.domains.is_empty() || vhost.domains.len() > MAX_DOMAINS_PER_VHOST {
                return Err(DomainError::validation(format!(
                    "virtual host \"{}\" needs 1-{MAX_DOMAINS_PER_VHOST} domains",
                    vhost.name
                )));
            }
            let mut seen = HashSet::new();
            for domain in &vhost.domains {
                if !valid_domain(domain) {
                    return Err(DomainError::validation(format!(
                        "\"{}\" is not a valid domain match",
                        domain
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(64)
                            .collect::<String>()
                    )));
                }
                if !seen.insert(domain.to_ascii_lowercase()) {
                    return Err(DomainError::validation(format!(
                        "duplicate domain \"{domain}\" in virtual host \"{}\"",
                        vhost.name
                    )));
                }
            }
            if vhost.routes.is_empty() || vhost.routes.len() > MAX_ROUTES_PER_VHOST {
                return Err(DomainError::validation(format!(
                    "virtual host \"{}\" needs 1-{MAX_ROUTES_PER_VHOST} routes",
                    vhost.name
                )));
            }
            let mut rule_names = HashSet::new();
            for rule in &vhost.routes {
                validate_name(&rule.name)?;
                if !rule_names.insert(rule.name.as_str()) {
                    return Err(DomainError::validation(format!(
                        "duplicate route \"{}\" in virtual host \"{}\"",
                        rule.name, vhost.name
                    )));
                }
                match &rule.matcher {
                    PathMatch::Prefix { prefix } => valid_path("route prefix", prefix)?,
                    PathMatch::Exact { path } => valid_path("route path", path)?,
                    PathMatch::Template { template } => valid_path("route template", template)?,
                }
                validate_name(&rule.action.cluster)?;
                let is_template = matches!(rule.matcher, PathMatch::Template { .. });
                match (&rule.action.prefix_rewrite, &rule.action.template_rewrite) {
                    (Some(_), Some(_)) => {
                        return Err(DomainError::validation(format!(
                        "route \"{}\": prefix_rewrite and template_rewrite are mutually exclusive",
                        rule.name
                    )))
                    }
                    (Some(rewrite), None) => {
                        if is_template {
                            return Err(DomainError::validation(format!(
                                "route \"{}\": prefix_rewrite cannot be used with a template match",
                                rule.name
                            )));
                        }
                        valid_path("prefix_rewrite", rewrite)?;
                    }
                    (None, Some(rewrite)) => {
                        if !is_template {
                            return Err(DomainError::validation(format!(
                                "route \"{}\": template_rewrite requires a template match",
                                rule.name
                            )));
                        }
                        valid_path("template_rewrite", rewrite)?;
                    }
                    (None, None) => {}
                }
                if rule.action.timeout_secs < 1 || rule.action.timeout_secs > 300 {
                    return Err(DomainError::validation(format!(
                        "route \"{}\": timeout_secs must be 1-300",
                        rule.name
                    )));
                }
            }
        }
        Ok(())
    }

    /// Distinct cluster names referenced by any route action (for reference tracking).
    pub fn referenced_clusters(&self) -> HashSet<&str> {
        self.virtual_hosts
            .iter()
            .flat_map(|vh| vh.routes.iter())
            .map(|r| r.action.cluster.as_str())
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn minimal(cluster: &str) -> RouteConfigSpec {
        RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "all".into(),
                    matcher: PathMatch::Prefix { prefix: "/".into() },
                    action: RouteAction {
                        cluster: cluster.into(),
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                    },
                }],
            }],
        }
    }

    #[test]
    fn minimal_validates_and_reports_references() {
        let spec = minimal("payments");
        assert!(spec.validate().is_ok());
        assert!(spec.referenced_clusters().contains("payments"));
    }

    #[test]
    fn rewrite_rules_enforced() {
        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].action.template_rewrite = Some("/x/{id}".into());
        assert!(
            spec.validate().is_err(),
            "template_rewrite without template match"
        );

        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].matcher = PathMatch::Template {
            template: "/users/{id}".into(),
        };
        spec.virtual_hosts[0].routes[0].action.prefix_rewrite = Some("/v2".into());
        assert!(
            spec.validate().is_err(),
            "prefix_rewrite with template match"
        );

        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].action.prefix_rewrite = Some("/v2".into());
        spec.virtual_hosts[0].routes[0].action.template_rewrite = Some("/x".into());
        assert!(spec.validate().is_err(), "both rewrites at once");
    }

    #[test]
    fn adversarial_domains_and_paths_rejected() {
        let mut spec = minimal("c");
        spec.virtual_hosts[0].domains = vec!["evil domain".into()];
        assert!(spec.validate().is_err());

        let mut spec = minimal("c");
        spec.virtual_hosts[0].domains = vec!["A.example".into(), "a.example".into()];
        assert!(
            spec.validate().is_err(),
            "case-insensitive duplicate domains"
        );

        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].matcher = PathMatch::Prefix {
            prefix: "/../etc".into(),
        };
        assert!(spec.validate().is_err(), "path traversal in prefix");
    }
}
