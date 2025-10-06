//! Route domain types
//!
//! This module contains pure domain entities for route configurations.
//! These types encapsulate route matching, transformation, and
//! routing logic without any infrastructure dependencies.

/// Path matching strategy for route selection.
///
/// Defines how incoming request paths should be matched against route definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathMatchStrategy {
    /// Exact path match (e.g., "/api/users" matches only "/api/users")
    Exact(String),

    /// Prefix match (e.g., "/api" matches "/api/users", "/api/products")
    Prefix(String),

    /// Regular expression match
    Regex(String),

    /// URI template match (e.g., "/users/{id}")
    Template(String),
}

impl PathMatchStrategy {
    /// Check if this strategy matches the given path.
    ///
    /// Note: Regex matching is not implemented in the domain layer
    /// and should be delegated to infrastructure.
    pub fn matches(&self, path: &str) -> bool {
        match self {
            PathMatchStrategy::Exact(pattern) => path == pattern,
            PathMatchStrategy::Prefix(prefix) => path.starts_with(prefix),
            PathMatchStrategy::Regex(_) => {
                // Regex matching requires external dependencies
                // Implementation should be done in infrastructure layer
                false
            }
            PathMatchStrategy::Template(_) => {
                // Template matching requires parsing logic
                // Implementation should be done in infrastructure layer
                false
            }
        }
    }

    /// Get the pattern string regardless of strategy type
    pub fn pattern(&self) -> &str {
        match self {
            PathMatchStrategy::Exact(s)
            | PathMatchStrategy::Prefix(s)
            | PathMatchStrategy::Regex(s)
            | PathMatchStrategy::Template(s) => s,
        }
    }
}

/// Header matching criteria.
///
/// Defines how request headers should be matched for route selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderMatch {
    /// Header name (case-insensitive in HTTP/1.1, lowercase in HTTP/2)
    pub name: String,

    /// Match criteria
    pub matcher: HeaderMatcher,
}

/// Header matching strategy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderMatcher {
    /// Exact value match
    Exact(String),

    /// Regular expression match
    Regex(String),

    /// Header must be present (any value)
    Present,

    /// Header must be absent
    Absent,
}

/// Query parameter matching criteria
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParameterMatch {
    /// Query parameter name
    pub name: String,

    /// Match criteria
    pub matcher: QueryParameterMatcher,
}

/// Query parameter matching strategy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryParameterMatcher {
    /// Exact value match
    Exact(String),

    /// Regular expression match
    Regex(String),

    /// Parameter must be present (any value)
    Present,
}

/// Complete route matching criteria.
///
/// Combines path, header, and query parameter matching to determine
/// if a request should be routed to this route.
#[derive(Debug, Clone)]
pub struct RouteMatch {
    /// Path matching strategy
    pub path: PathMatchStrategy,

    /// Optional header matches (all must match)
    pub headers: Vec<HeaderMatch>,

    /// Optional query parameter matches (all must match)
    pub query_parameters: Vec<QueryParameterMatch>,

    /// Whether path matching is case-sensitive
    pub case_sensitive: bool,
}

impl RouteMatch {
    /// Create a simple prefix-based route match
    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self {
            path: PathMatchStrategy::Prefix(prefix.into()),
            headers: vec![],
            query_parameters: vec![],
            case_sensitive: true,
        }
    }

    /// Create an exact path route match
    pub fn exact(path: impl Into<String>) -> Self {
        Self {
            path: PathMatchStrategy::Exact(path.into()),
            headers: vec![],
            query_parameters: vec![],
            case_sensitive: true,
        }
    }

    /// Add a header match requirement
    pub fn with_header(mut self, name: impl Into<String>, matcher: HeaderMatcher) -> Self {
        self.headers.push(HeaderMatch { name: name.into(), matcher });
        self
    }

    /// Add a query parameter match requirement
    pub fn with_query_param(
        mut self,
        name: impl Into<String>,
        matcher: QueryParameterMatcher,
    ) -> Self {
        self.query_parameters.push(QueryParameterMatch { name: name.into(), matcher });
        self
    }

    /// Set case sensitivity
    pub fn case_sensitive(mut self, sensitive: bool) -> Self {
        self.case_sensitive = sensitive;
        self
    }
}

/// Path rewrite strategy.
///
/// Defines how the request path should be transformed before
/// forwarding to the upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRewrite {
    /// Replace prefix (e.g., "/api/v1" -> "/v1")
    ReplacePrefix {
        /// The prefix to remove
        prefix: String,
        /// The replacement prefix
        replacement: String,
    },

    /// Regex-based rewrite
    Regex {
        /// Regular expression pattern
        pattern: String,
        /// Substitution string
        substitution: String,
    },

    /// Full path replacement
    ReplaceFull(String),
}

/// Route action defining what to do with matched requests.
///
/// This is a pure domain representation of routing decisions,
/// independent of any specific implementation.
#[derive(Debug, Clone)]
pub struct RouteAction {
    /// Target cluster or upstream
    pub target: RouteTarget,

    /// Optional path rewrite
    pub path_rewrite: Option<PathRewrite>,

    /// Optional timeout in seconds
    pub timeout_seconds: Option<u64>,

    /// Optional retry policy
    pub retry_policy: Option<RetryPolicy>,
}

/// Route target specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    /// Single cluster
    Cluster(String),

    /// Weighted distribution across multiple clusters
    WeightedClusters(Vec<WeightedCluster>),
}

/// Weighted cluster for traffic splitting
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedCluster {
    /// Cluster name
    pub name: String,

    /// Weight (relative to other clusters)
    pub weight: u32,
}

/// Retry policy for failed requests
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of retries
    pub max_retries: u32,

    /// Conditions that trigger retries
    pub retry_on: Vec<RetryCondition>,

    /// Per-retry timeout in seconds
    pub per_try_timeout_seconds: Option<u64>,
}

/// Conditions that trigger request retries
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryCondition {
    /// Retry on 5xx responses
    ServerError,

    /// Retry on connection failures
    ConnectionFailure,

    /// Retry on specific status code
    StatusCode(u16),

    /// Retry on timeout
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_match_exact() {
        let matcher = PathMatchStrategy::Exact("/api/users".to_string());
        assert!(matcher.matches("/api/users"));
        assert!(!matcher.matches("/api/users/123"));
        assert!(!matcher.matches("/api"));
    }

    #[test]
    fn path_match_prefix() {
        let matcher = PathMatchStrategy::Prefix("/api".to_string());
        assert!(matcher.matches("/api"));
        assert!(matcher.matches("/api/users"));
        assert!(matcher.matches("/api/products/123"));
        assert!(!matcher.matches("/users"));
    }

    #[test]
    fn path_match_pattern_extraction() {
        let exact = PathMatchStrategy::Exact("/test".to_string());
        assert_eq!(exact.pattern(), "/test");

        let prefix = PathMatchStrategy::Prefix("/api".to_string());
        assert_eq!(prefix.pattern(), "/api");
    }

    #[test]
    fn route_match_builder_prefix() {
        let route_match = RouteMatch::prefix("/api/v1");

        assert!(matches!(route_match.path, PathMatchStrategy::Prefix(_)));
        assert_eq!(route_match.path.pattern(), "/api/v1");
        assert!(route_match.case_sensitive);
        assert!(route_match.headers.is_empty());
        assert!(route_match.query_parameters.is_empty());
    }

    #[test]
    fn route_match_builder_exact() {
        let route_match = RouteMatch::exact("/health");

        assert!(matches!(route_match.path, PathMatchStrategy::Exact(_)));
        assert_eq!(route_match.path.pattern(), "/health");
    }

    #[test]
    fn route_match_with_header() {
        let route_match =
            RouteMatch::prefix("/api").with_header("X-API-Key", HeaderMatcher::Present);

        assert_eq!(route_match.headers.len(), 1);
        assert_eq!(route_match.headers[0].name, "X-API-Key");
        assert!(matches!(route_match.headers[0].matcher, HeaderMatcher::Present));
    }

    #[test]
    fn route_match_with_query_param() {
        let route_match = RouteMatch::prefix("/search")
            .with_query_param("version", QueryParameterMatcher::Exact("v2".to_string()));

        assert_eq!(route_match.query_parameters.len(), 1);
        assert_eq!(route_match.query_parameters[0].name, "version");
    }

    #[test]
    fn route_match_case_insensitive() {
        let route_match = RouteMatch::prefix("/API").case_sensitive(false);
        assert!(!route_match.case_sensitive);
    }

    #[test]
    fn weighted_cluster_creation() {
        let cluster = WeightedCluster { name: "cluster-a".to_string(), weight: 80 };

        assert_eq!(cluster.name, "cluster-a");
        assert_eq!(cluster.weight, 80);
    }

    #[test]
    fn route_target_single_cluster() {
        let target = RouteTarget::Cluster("backend".to_string());
        assert!(matches!(target, RouteTarget::Cluster(_)));
    }

    #[test]
    fn route_target_weighted_clusters() {
        let target = RouteTarget::WeightedClusters(vec![
            WeightedCluster { name: "backend-v1".to_string(), weight: 90 },
            WeightedCluster { name: "backend-v2".to_string(), weight: 10 },
        ]);

        if let RouteTarget::WeightedClusters(clusters) = target {
            assert_eq!(clusters.len(), 2);
            assert_eq!(clusters[0].weight, 90);
            assert_eq!(clusters[1].weight, 10);
        } else {
            panic!("Expected WeightedClusters");
        }
    }

    #[test]
    fn path_rewrite_prefix() {
        let rewrite = PathRewrite::ReplacePrefix {
            prefix: "/old".to_string(),
            replacement: "/new".to_string(),
        };

        assert!(matches!(rewrite, PathRewrite::ReplacePrefix { .. }));
    }

    #[test]
    fn retry_policy_configuration() {
        let policy = RetryPolicy {
            max_retries: 3,
            retry_on: vec![RetryCondition::ServerError, RetryCondition::Timeout],
            per_try_timeout_seconds: Some(5),
        };

        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.retry_on.len(), 2);
        assert_eq!(policy.per_try_timeout_seconds, Some(5));
    }

    #[test]
    fn route_action_with_all_options() {
        let action = RouteAction {
            target: RouteTarget::Cluster("backend".to_string()),
            path_rewrite: Some(PathRewrite::ReplacePrefix {
                prefix: "/api/v1".to_string(),
                replacement: "/v2".to_string(),
            }),
            timeout_seconds: Some(30),
            retry_policy: Some(RetryPolicy {
                max_retries: 3,
                retry_on: vec![RetryCondition::ServerError],
                per_try_timeout_seconds: Some(10),
            }),
        };

        assert!(matches!(action.target, RouteTarget::Cluster(_)));
        assert!(action.path_rewrite.is_some());
        assert_eq!(action.timeout_seconds, Some(30));
        assert!(action.retry_policy.is_some());
    }

    #[test]
    fn complex_route_match_with_multiple_criteria() {
        let route_match = RouteMatch::prefix("/api")
            .with_header("X-API-Version", HeaderMatcher::Exact("v2".to_string()))
            .with_header("Authorization", HeaderMatcher::Present)
            .with_query_param("format", QueryParameterMatcher::Exact("json".to_string()))
            .case_sensitive(false);

        assert_eq!(route_match.headers.len(), 2);
        assert_eq!(route_match.query_parameters.len(), 1);
        assert!(!route_match.case_sensitive);
    }

    #[test]
    fn header_matcher_variants() {
        let exact = HeaderMatcher::Exact("application/json".to_string());
        let regex = HeaderMatcher::Regex("application/.*".to_string());
        let present = HeaderMatcher::Present;
        let absent = HeaderMatcher::Absent;

        assert!(matches!(exact, HeaderMatcher::Exact(_)));
        assert!(matches!(regex, HeaderMatcher::Regex(_)));
        assert!(matches!(present, HeaderMatcher::Present));
        assert!(matches!(absent, HeaderMatcher::Absent));
    }

    #[test]
    fn query_parameter_matcher_variants() {
        let exact = QueryParameterMatcher::Exact("true".to_string());
        let regex = QueryParameterMatcher::Regex("[0-9]+".to_string());
        let present = QueryParameterMatcher::Present;

        assert!(matches!(exact, QueryParameterMatcher::Exact(_)));
        assert!(matches!(regex, QueryParameterMatcher::Regex(_)));
        assert!(matches!(present, QueryParameterMatcher::Present));
    }

    #[test]
    fn path_rewrite_variants() {
        let prefix_rewrite = PathRewrite::ReplacePrefix {
            prefix: "/old".to_string(),
            replacement: "/new".to_string(),
        };
        let regex_rewrite = PathRewrite::Regex {
            pattern: "^/api/v1/(.*)".to_string(),
            substitution: "/v2/$1".to_string(),
        };
        let full_rewrite = PathRewrite::ReplaceFull("/static".to_string());

        assert!(matches!(prefix_rewrite, PathRewrite::ReplacePrefix { .. }));
        assert!(matches!(regex_rewrite, PathRewrite::Regex { .. }));
        assert!(matches!(full_rewrite, PathRewrite::ReplaceFull(_)));
    }

    #[test]
    fn retry_condition_variants() {
        assert_eq!(RetryCondition::ServerError, RetryCondition::ServerError);
        assert_eq!(RetryCondition::ConnectionFailure, RetryCondition::ConnectionFailure);
        assert_eq!(RetryCondition::StatusCode(503), RetryCondition::StatusCode(503));
        assert_ne!(RetryCondition::StatusCode(503), RetryCondition::StatusCode(502));
        assert_eq!(RetryCondition::Timeout, RetryCondition::Timeout);
    }

    #[test]
    fn weighted_clusters_distribution() {
        let target = RouteTarget::WeightedClusters(vec![
            WeightedCluster { name: "stable".to_string(), weight: 95 },
            WeightedCluster { name: "canary".to_string(), weight: 5 },
        ]);

        if let RouteTarget::WeightedClusters(clusters) = target {
            let total_weight: u32 = clusters.iter().map(|c| c.weight).sum();
            assert_eq!(total_weight, 100);
            assert_eq!(clusters[0].name, "stable");
            assert_eq!(clusters[1].name, "canary");
        }
    }
}
