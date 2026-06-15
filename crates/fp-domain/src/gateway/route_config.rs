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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteConfigSpec {
    pub virtual_hosts: Vec<VirtualHost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualHost {
    pub name: String,
    /// Host-header matches: valid hostnames, `*.suffix` wildcards, or literal `*`.
    pub domains: Vec<String>,
    pub routes: Vec<RouteRule>,
    /// Virtual-host descriptor generators for the global RLS filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rate_limits: Vec<RateLimitDefinition>,
    /// Per-vhost filter behavior (S5.8); a route-level override wins over the vhost's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_overrides: Vec<crate::gateway::filters::FilterOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteRule {
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: PathMatch,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_parameters: Vec<QueryParameterMatch>,
    pub action: RouteAction,
    /// Per-route filter behavior (S5.8); wins over the vhost-level override.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_overrides: Vec<crate::gateway::filters::FilterOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Full-path RE2 safe regex match.
    Regex {
        pattern: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteAction {
    /// Target cluster, by name, same team (resolved by the service layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    /// Weighted target clusters, by name, same team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weighted_clusters: Option<Vec<WeightedClusterTarget>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<RedirectAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_response: Option<DirectResponseAction>,
    /// Replace the matched prefix. Never with a Template match (v1 rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_rewrite: Option<String>,
    /// Rewrite using template captures. Only with a Template match (v1 rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_rewrite: Option<String>,
    /// Upstream request timeout in seconds (1–300; default 15).
    #[serde(default = "default_route_timeout")]
    pub timeout_secs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<RetryPolicy>,
    /// Route descriptor generators for the global RLS filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rate_limits: Vec<RateLimitDefinition>,
}

fn default_route_timeout() -> u32 {
    15
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WeightedClusterTarget {
    pub cluster: String,
    pub weight: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DirectResponseAction {
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Envoy retry-on tokens, comma-separated (for example `5xx,connect-failure`).
    pub retry_on: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_try_timeout_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retriable_status_codes: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitDefinition {
    /// Envoy RLS stage, 0-10. Omitted means stage 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_key: Option<String>,
    pub actions: Vec<RateLimitAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RateLimitAction {
    RequestHeaders {
        header_name: String,
        descriptor_key: String,
        #[serde(default)]
        skip_if_absent: bool,
    },
    GenericKey {
        descriptor_value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        descriptor_key: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RedirectAction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_redirect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme_redirect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https_redirect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_redirect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_rewrite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_code: Option<RedirectResponseCode>,
    #[serde(default)]
    pub strip_query: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RedirectResponseCode {
    MovedPermanently,
    Found,
    SeeOther,
    TemporaryRedirect,
    PermanentRedirect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HeaderMatch {
    pub name: String,
    #[serde(default)]
    pub invert_match: bool,
    #[serde(flatten)]
    pub matcher: HeaderValueMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HeaderValueMatch {
    Present { value: bool },
    Exact { value: String },
    Prefix { value: String },
    Suffix { value: String },
    Contains { value: String },
    Regex { pattern: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryParameterMatch {
    pub name: String,
    #[serde(flatten)]
    pub matcher: QueryValueMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryValueMatch {
    Present { value: bool },
    Exact { value: String },
    Prefix { value: String },
    Suffix { value: String },
    Contains { value: String },
    Regex { pattern: String },
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

fn valid_token(label: &str, value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\0')
    {
        return Err(DomainError::validation(format!(
            "{label} must be 1-128 non-whitespace, non-control characters"
        )));
    }
    Ok(())
}

fn valid_path(label: &str, path: &str) -> DomainResult<()> {
    if !path.starts_with('/') || path.contains("..") || path.len() > 500 || path.contains('\0') {
        return Err(DomainError::validation(format!(
            "{label} must start with '/', contain no '..', and be <= 500 chars"
        )));
    }
    Ok(())
}

fn valid_match_value(label: &str, value: &str) -> DomainResult<()> {
    if value.is_empty() || value.len() > 500 || value.contains('\0') {
        return Err(DomainError::validation(format!(
            "{label} must be 1-500 chars and contain no NUL"
        )));
    }
    Ok(())
}

fn valid_regex(label: &str, pattern: &str) -> DomainResult<()> {
    if pattern.is_empty() || pattern.len() > 500 || pattern.contains('\0') {
        return Err(DomainError::validation(format!(
            "{label} must be 1-500 chars and contain no NUL"
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
            validate_rate_limits(&vhost.rate_limits)?;
            crate::gateway::filters::validate_filter_overrides(&vhost.filter_overrides)?;
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
                    PathMatch::Regex { pattern } => valid_regex("route regex", pattern)?,
                }
                let mut header_names = HashSet::new();
                for header in &rule.headers {
                    validate_header_match(header)?;
                    if !header_names.insert(header.name.to_ascii_lowercase()) {
                        return Err(DomainError::validation(format!(
                            "duplicate header matcher \"{}\" in route \"{}\"",
                            header.name, rule.name
                        )));
                    }
                }
                let mut query_names = HashSet::new();
                for query in &rule.query_parameters {
                    validate_query_match(query)?;
                    if !query_names.insert(query.name.as_str()) {
                        return Err(DomainError::validation(format!(
                            "duplicate query matcher \"{}\" in route \"{}\"",
                            query.name, rule.name
                        )));
                    }
                }
                validate_action(&rule.action, &rule.matcher, &rule.name)?;
                crate::gateway::filters::validate_filter_overrides(&rule.filter_overrides)?;
            }
        }
        Ok(())
    }

    /// Distinct cluster names referenced by any route action (for reference tracking).
    pub fn referenced_clusters(&self) -> HashSet<&str> {
        self.virtual_hosts
            .iter()
            .flat_map(|vh| vh.routes.iter())
            .flat_map(|r| r.action.referenced_clusters())
            .collect()
    }
}

fn validate_header_match(header: &HeaderMatch) -> DomainResult<()> {
    valid_token("header matcher name", &header.name)?;
    match &header.matcher {
        HeaderValueMatch::Present { .. } => {}
        HeaderValueMatch::Exact { value }
        | HeaderValueMatch::Prefix { value }
        | HeaderValueMatch::Suffix { value }
        | HeaderValueMatch::Contains { value } => valid_match_value("header matcher value", value)?,
        HeaderValueMatch::Regex { pattern } => valid_regex("header matcher regex", pattern)?,
    }
    Ok(())
}

fn validate_query_match(query: &QueryParameterMatch) -> DomainResult<()> {
    valid_token("query matcher name", &query.name)?;
    match &query.matcher {
        QueryValueMatch::Present { .. } => {}
        QueryValueMatch::Exact { value }
        | QueryValueMatch::Prefix { value }
        | QueryValueMatch::Suffix { value }
        | QueryValueMatch::Contains { value } => valid_match_value("query matcher value", value)?,
        QueryValueMatch::Regex { pattern } => valid_regex("query matcher regex", pattern)?,
    }
    Ok(())
}

fn validate_action(
    action: &RouteAction,
    matcher: &PathMatch,
    route_name: &str,
) -> DomainResult<()> {
    let terminal_actions = usize::from(action.cluster.is_some())
        + usize::from(action.weighted_clusters.is_some())
        + usize::from(action.redirect.is_some())
        + usize::from(action.direct_response.is_some());
    if terminal_actions != 1 {
        return Err(DomainError::validation(format!(
            "route \"{route_name}\": exactly one of cluster, weighted_clusters, redirect, or direct_response is required"
        )));
    }
    if let Some(cluster) = &action.cluster {
        validate_name(cluster)?;
    }
    if let Some(weighted) = &action.weighted_clusters {
        validate_weighted_clusters(weighted)?;
    }
    if let Some(redirect) = &action.redirect {
        validate_redirect(redirect)?;
        if action.prefix_rewrite.is_some()
            || action.template_rewrite.is_some()
            || action.retry_policy.is_some()
            || !action.rate_limits.is_empty()
        {
            return Err(DomainError::validation(format!(
                "route \"{route_name}\": redirect cannot combine with route rewrites, retry_policy, or rate_limits"
            )));
        }
    }
    if let Some(direct) = &action.direct_response {
        validate_direct_response(direct)?;
        if action.prefix_rewrite.is_some()
            || action.template_rewrite.is_some()
            || action.retry_policy.is_some()
            || !action.rate_limits.is_empty()
        {
            return Err(DomainError::validation(format!(
                "route \"{route_name}\": direct_response cannot combine with route rewrites, retry_policy, or rate_limits"
            )));
        }
    }
    let is_template = matches!(matcher, PathMatch::Template { .. });
    match (&action.prefix_rewrite, &action.template_rewrite) {
        (Some(_), Some(_)) => {
            return Err(DomainError::validation(format!(
            "route \"{route_name}\": prefix_rewrite and template_rewrite are mutually exclusive",
        )))
        }
        (Some(rewrite), None) => {
            if is_template {
                return Err(DomainError::validation(format!(
                    "route \"{route_name}\": prefix_rewrite cannot be used with a template match",
                )));
            }
            valid_path("prefix_rewrite", rewrite)?;
        }
        (None, Some(rewrite)) => {
            if !is_template {
                return Err(DomainError::validation(format!(
                    "route \"{route_name}\": template_rewrite requires a template match",
                )));
            }
            valid_path("template_rewrite", rewrite)?;
        }
        (None, None) => {}
    }
    if action.timeout_secs < 1 || action.timeout_secs > 300 {
        return Err(DomainError::validation(format!(
            "route \"{route_name}\": timeout_secs must be 1-300",
        )));
    }
    if let Some(retry) = &action.retry_policy {
        validate_retry_policy(retry, action.timeout_secs)?;
    }
    validate_rate_limits(&action.rate_limits)?;
    Ok(())
}

fn validate_rate_limits(rate_limits: &[RateLimitDefinition]) -> DomainResult<()> {
    if rate_limits.len() > 32 {
        return Err(DomainError::validation(
            "rate_limits must contain at most 32 definitions",
        ));
    }
    for limit in rate_limits {
        if let Some(stage) = limit.stage {
            if stage > 10 {
                return Err(DomainError::validation(
                    "rate_limits.stage must be in 0..=10",
                ));
            }
        }
        if let Some(disable_key) = &limit.disable_key {
            valid_token("rate_limits.disable_key", disable_key)?;
        }
        if limit.actions.is_empty() || limit.actions.len() > 16 {
            return Err(DomainError::validation(
                "rate_limits.actions must contain 1-16 actions",
            ));
        }
        for action in &limit.actions {
            validate_rate_limit_action(action)?;
        }
    }
    Ok(())
}

fn validate_direct_response(action: &DirectResponseAction) -> DomainResult<()> {
    if action.status < 100 || action.status > 599 {
        return Err(DomainError::validation(
            "direct_response.status must be an HTTP status code",
        ));
    }
    if action.body.as_ref().is_some_and(|body| body.len() > 4096) {
        return Err(DomainError::validation(
            "direct_response.body must be <= 4096 bytes",
        ));
    }
    Ok(())
}

fn validate_rate_limit_action(action: &RateLimitAction) -> DomainResult<()> {
    match action {
        RateLimitAction::RequestHeaders {
            header_name,
            descriptor_key,
            ..
        } => {
            valid_token("rate_limits.request_headers.header_name", header_name)?;
            valid_token("rate_limits.request_headers.descriptor_key", descriptor_key)?;
        }
        RateLimitAction::GenericKey {
            descriptor_value,
            descriptor_key,
        } => {
            valid_match_value("rate_limits.generic_key.descriptor_value", descriptor_value)?;
            if let Some(descriptor_key) = descriptor_key {
                valid_token("rate_limits.generic_key.descriptor_key", descriptor_key)?;
            }
        }
    }
    Ok(())
}

fn validate_weighted_clusters(weighted: &[WeightedClusterTarget]) -> DomainResult<()> {
    if weighted.is_empty() || weighted.len() > 32 {
        return Err(DomainError::validation(
            "weighted_clusters must contain 1-32 targets",
        ));
    }
    let mut names = HashSet::new();
    let mut total = 0_u64;
    for target in weighted {
        validate_name(&target.cluster)?;
        if !names.insert(target.cluster.as_str()) {
            return Err(DomainError::validation(format!(
                "duplicate weighted cluster \"{}\"",
                target.cluster
            )));
        }
        if target.weight == 0 || target.weight > 10_000 {
            return Err(DomainError::validation(
                "weighted cluster weights must be 1-10000",
            ));
        }
        total += u64::from(target.weight);
    }
    if total > 10_000 {
        return Err(DomainError::validation(
            "weighted cluster total weight must be <= 10000",
        ));
    }
    Ok(())
}

fn validate_retry_policy(retry: &RetryPolicy, route_timeout_secs: u32) -> DomainResult<()> {
    if retry.retry_on.is_empty()
        || retry.retry_on.len() > 200
        || retry
            .retry_on
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\0')
    {
        return Err(DomainError::validation(
            "retry_policy.retry_on must be comma-separated non-whitespace tokens",
        ));
    }
    if let Some(num_retries) = retry.num_retries {
        if num_retries == 0 || num_retries > 10 {
            return Err(DomainError::validation(
                "retry_policy.num_retries must be 1-10",
            ));
        }
    }
    if let Some(per_try) = retry.per_try_timeout_secs {
        if per_try == 0 || per_try > route_timeout_secs {
            return Err(DomainError::validation(
                "retry_policy.per_try_timeout_secs must be 1..timeout_secs",
            ));
        }
    }
    for status in &retry.retriable_status_codes {
        if !(100..600).contains(status) {
            return Err(DomainError::validation(
                "retry_policy.retriable_status_codes must be HTTP status codes 100-599",
            ));
        }
    }
    Ok(())
}

fn validate_redirect(redirect: &RedirectAction) -> DomainResult<()> {
    let mutations = usize::from(redirect.host_redirect.is_some())
        + usize::from(redirect.scheme_redirect.is_some())
        + usize::from(redirect.https_redirect.is_some())
        + usize::from(redirect.path_redirect.is_some())
        + usize::from(redirect.prefix_rewrite.is_some());
    if mutations == 0 {
        return Err(DomainError::validation(
            "redirect must change host, scheme, or path",
        ));
    }
    if let Some(host) = &redirect.host_redirect {
        valid_token("redirect host", host)?;
    }
    if let Some(scheme) = &redirect.scheme_redirect {
        if scheme != "http" && scheme != "https" {
            return Err(DomainError::validation(
                "redirect scheme_redirect must be http or https",
            ));
        }
    }
    if redirect.scheme_redirect.is_some() && redirect.https_redirect.is_some() {
        return Err(DomainError::validation(
            "redirect scheme_redirect and https_redirect are mutually exclusive",
        ));
    }
    let path_actions = usize::from(redirect.path_redirect.is_some())
        + usize::from(redirect.prefix_rewrite.is_some());
    if path_actions > 1 {
        return Err(DomainError::validation(
            "redirect path_redirect and prefix_rewrite are mutually exclusive",
        ));
    }
    if let Some(path) = &redirect.path_redirect {
        valid_path("redirect path_redirect", path)?;
    }
    if let Some(prefix) = &redirect.prefix_rewrite {
        valid_path("redirect prefix_rewrite", prefix)?;
    }
    Ok(())
}

impl RouteAction {
    pub fn referenced_clusters(&self) -> impl Iterator<Item = &str> {
        self.cluster.iter().map(String::as_str).chain(
            self.weighted_clusters
                .iter()
                .flatten()
                .map(|target| target.cluster.as_str()),
        )
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
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: RouteAction {
                        cluster: Some(cluster.into()),
                        weighted_clusters: None,
                        redirect: None,
                        direct_response: None,
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                        retry_policy: None,
                        rate_limits: Vec::new(),
                    },
                    filter_overrides: Vec::new(),
                }],
                rate_limits: Vec::new(),
                filter_overrides: Vec::new(),
            }],
        }
    }

    #[test]
    fn filter_override_rules_enforced() {
        use crate::gateway::filters::FilterOverride;
        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].filter_overrides = vec![FilterOverride::Disable {
            filter_type: "health_check".into(),
        }];
        assert!(spec.validate().is_err(), "health_check is listener-only");

        let mut spec = minimal("c");
        spec.virtual_hosts[0].filter_overrides = vec![
            FilterOverride::Disable {
                filter_type: "cors".into(),
            },
            FilterOverride::Cors(crate::gateway::filters::CorsConfig {
                allow_origin: vec![crate::gateway::filters::OriginMatcher::Exact {
                    value: "https://a.example".into(),
                }],
                allow_methods: vec![],
                allow_headers: vec![],
                expose_headers: vec![],
                max_age_seconds: None,
                allow_credentials: false,
            }),
        ];
        assert!(
            spec.validate().is_err(),
            "two overrides for one filter type in a scope"
        );
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

    #[test]
    fn expanded_route_options_validate_and_report_references() {
        let mut spec = minimal("primary");
        spec.virtual_hosts[0].routes[0].matcher = PathMatch::Regex {
            pattern: "^/v[0-9]+/items$".into(),
        };
        spec.virtual_hosts[0].routes[0].headers = vec![
            HeaderMatch {
                name: "x-api-version".into(),
                invert_match: false,
                matcher: HeaderValueMatch::Exact { value: "2".into() },
            },
            HeaderMatch {
                name: "x-debug".into(),
                invert_match: true,
                matcher: HeaderValueMatch::Present { value: true },
            },
        ];
        spec.virtual_hosts[0].routes[0].query_parameters = vec![QueryParameterMatch {
            name: "preview".into(),
            matcher: QueryValueMatch::Present { value: true },
        }];
        spec.virtual_hosts[0].routes[0].action.cluster = None;
        spec.virtual_hosts[0].routes[0].action.weighted_clusters = Some(vec![
            WeightedClusterTarget {
                cluster: "primary".into(),
                weight: 80,
            },
            WeightedClusterTarget {
                cluster: "canary".into(),
                weight: 20,
            },
        ]);
        spec.virtual_hosts[0].routes[0].action.retry_policy = Some(RetryPolicy {
            retry_on: "5xx,connect-failure".into(),
            num_retries: Some(2),
            per_try_timeout_secs: Some(3),
            retriable_status_codes: vec![502, 503],
        });

        assert!(spec.validate().is_ok());
        let refs = spec.referenced_clusters();
        assert!(refs.contains("primary"));
        assert!(refs.contains("canary"));
    }

    #[test]
    fn flattened_header_and_query_matchers_deserialize_from_api_json() {
        let spec: RouteConfigSpec = serde_json::from_value(serde_json::json!({
            "virtual_hosts": [{
                "name": "default",
                "domains": ["*"],
                "routes": [{
                    "name": "items",
                    "match": {"regex": {"pattern": "^/v[0-9]+/items$"}},
                    "headers": [{"name": "x-api-version", "type": "exact", "value": "2"}],
                    "query_parameters": [{"name": "preview", "type": "present", "value": true}],
                    "action": {"cluster": "primary"}
                }]
            }]
        }))
        .expect("deserialize api route config");
        spec.validate().expect("validate api route config");
        let route = &spec.virtual_hosts[0].routes[0];
        assert!(matches!(
            route.headers[0].matcher,
            HeaderValueMatch::Exact { ref value } if value == "2"
        ));
        assert!(matches!(
            route.query_parameters[0].matcher,
            QueryValueMatch::Present { value: true }
        ));
    }

    #[test]
    fn ambiguous_or_lossy_route_options_rejected() {
        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].action.weighted_clusters =
            Some(vec![WeightedClusterTarget {
                cluster: "canary".into(),
                weight: 1,
            }]);
        assert!(spec.validate().is_err(), "two terminal actions");

        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].headers = vec![
            HeaderMatch {
                name: "x-mode".into(),
                invert_match: false,
                matcher: HeaderValueMatch::Exact { value: "a".into() },
            },
            HeaderMatch {
                name: "X-Mode".into(),
                invert_match: false,
                matcher: HeaderValueMatch::Exact { value: "b".into() },
            },
        ];
        assert!(spec.validate().is_err(), "duplicate header matcher");

        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].action = RouteAction {
            cluster: None,
            weighted_clusters: None,
            redirect: Some(RedirectAction {
                host_redirect: None,
                scheme_redirect: Some("ftp".into()),
                https_redirect: None,
                path_redirect: None,
                prefix_rewrite: None,
                response_code: None,
                strip_query: false,
            }),
            direct_response: None,
            prefix_rewrite: None,
            template_rewrite: None,
            timeout_secs: 15,
            retry_policy: None,
            rate_limits: Vec::new(),
        };
        assert!(spec.validate().is_err(), "invalid redirect scheme");

        let mut spec = minimal("c");
        spec.virtual_hosts[0].routes[0].action = RouteAction {
            cluster: None,
            weighted_clusters: None,
            redirect: Some(RedirectAction {
                host_redirect: None,
                scheme_redirect: None,
                https_redirect: None,
                path_redirect: None,
                prefix_rewrite: None,
                response_code: Some(RedirectResponseCode::Found),
                strip_query: true,
            }),
            direct_response: None,
            prefix_rewrite: None,
            template_rewrite: None,
            timeout_secs: 15,
            retry_policy: None,
            rate_limits: Vec::new(),
        };
        assert!(spec.validate().is_err(), "no-op redirect");
    }
}
