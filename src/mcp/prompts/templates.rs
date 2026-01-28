//! Built-in Prompt Templates
//!
//! Defines Flowplane-specific prompts for common operations and troubleshooting.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{PromptArgument, PromptContent, PromptGetResult, PromptMessage};
use serde_json::Value;

/// Helper to extract required string argument
fn get_required_arg(args: &Option<Value>, name: &str) -> Result<String, McpError> {
    args.as_ref()
        .and_then(|v| v.get(name))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| McpError::InvalidParams(format!("Missing required argument: {}", name)))
}

// -----------------------------------------------------------------------------
// Prompt 1: Debug Route Configuration
// -----------------------------------------------------------------------------

pub fn debug_route_arguments() -> Vec<PromptArgument> {
    vec![
        PromptArgument {
            name: "route_name".to_string(),
            description: Some("Name of the route to debug".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "team".to_string(),
            description: Some("Team identifier for multi-tenancy".to_string()),
            required: Some(true),
        },
    ]
}

pub fn render_debug_route(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let route_name = get_required_arg(&args, "route_name")?;
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Analyze the route configuration for route '{}' in team '{}' and identify any issues.

Please perform the following checks:

1. **Route Configuration**:
   - Verify the route exists and is properly configured
   - Check path patterns for correctness and conflicts
   - Validate HTTP methods and match types
   - Review rule ordering (lower order = higher priority)

2. **Virtual Host Integration**:
   - Confirm the route is attached to the correct virtual host
   - Check domain patterns and TLS settings on the virtual host

3. **Filter Attachments**:
   - List all filters attached to this route
   - Verify filter order and configuration
   - Check for authentication, rate limiting, or transformation filters

4. **Cluster Connectivity**:
   - Identify the target cluster for this route
   - Verify cluster health and endpoint availability
   - Check load balancing configuration

5. **Common Issues**:
   - Path pattern conflicts with other routes
   - Missing or misconfigured filters
   - Incorrect rule_order causing unexpected routing
   - Cluster endpoint health issues

Use the following tools to gather information:
- `cp_list_routes` with route_config filter
- `cp_get_listener` to check virtual host configuration
- `cp_list_filters` to identify attached filters
- `cp_get_cluster` to verify cluster health

Provide a summary of findings and actionable recommendations."#,
        route_name, team
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Diagnostic prompt for analyzing route '{}' configuration issues",
            route_name
        )),
        messages: vec![PromptMessage::User { content: PromptContent::Text { text: prompt_text } }],
    })
}

// -----------------------------------------------------------------------------
// Prompt 2: Explain Filter Configuration
// -----------------------------------------------------------------------------

pub fn explain_filter_arguments() -> Vec<PromptArgument> {
    vec![
        PromptArgument {
            name: "filter_name".to_string(),
            description: Some("Name of the filter to explain".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "team".to_string(),
            description: Some("Team identifier for multi-tenancy".to_string()),
            required: Some(true),
        },
    ]
}

pub fn render_explain_filter(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let filter_name = get_required_arg(&args, "filter_name")?;
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Explain the filter '{}' in team '{}' and its behavior in the request/response pipeline.

Please provide:

1. **Filter Overview**:
   - Filter type (authentication, rate limiting, transformation, etc.)
   - Current configuration and parameters
   - Enabled/disabled status

2. **Behavior Analysis**:
   - What this filter does to incoming requests
   - What this filter does to outgoing responses
   - How it modifies headers, body, or metadata
   - Any conditional logic or rules

3. **Attachment Points**:
   - Where this filter is attached (listeners, routes, clusters)
   - Filter execution order in the chain
   - Interaction with other filters

4. **Configuration Details**:
   - Key configuration parameters and their values
   - Default vs custom settings
   - Environment-specific configurations

5. **Performance Impact**:
   - Expected latency added by this filter
   - Resource usage (CPU, memory)
   - Scaling considerations

Use `cp_get_filter` to retrieve the filter configuration and provide a clear, actionable explanation suitable for both developers and operators."#,
        filter_name, team
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Explanation prompt for filter '{}' configuration and behavior",
            filter_name
        )),
        messages: vec![PromptMessage::User { content: PromptContent::Text { text: prompt_text } }],
    })
}

// -----------------------------------------------------------------------------
// Prompt 3: Troubleshoot Cluster Connectivity
// -----------------------------------------------------------------------------

pub fn troubleshoot_cluster_arguments() -> Vec<PromptArgument> {
    vec![
        PromptArgument {
            name: "cluster_name".to_string(),
            description: Some("Name of the cluster to troubleshoot".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "team".to_string(),
            description: Some("Team identifier for multi-tenancy".to_string()),
            required: Some(true),
        },
    ]
}

pub fn render_troubleshoot_cluster(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let cluster_name = get_required_arg(&args, "cluster_name")?;
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Diagnose connectivity and health issues for cluster '{}' in team '{}'.

Please perform the following diagnostic steps:

1. **Cluster Configuration**:
   - Verify cluster exists and is properly configured
   - Check load balancing policy (ROUND_ROBIN, LEAST_REQUEST, etc.)
   - Review connection timeout and retry settings

2. **Endpoint Health**:
   - List all endpoints in this cluster
   - Check health check configuration (if enabled)
   - Identify any unhealthy or unreachable endpoints
   - Verify endpoint addresses and ports

3. **TLS Configuration**:
   - Check if TLS is required for upstream connections
   - Verify TLS certificates and validation settings
   - Review SNI configuration

4. **Circuit Breaker & Outlier Detection**:
   - Check if circuit breaker is configured
   - Review outlier detection settings
   - Identify any ejected endpoints

5. **Traffic Routing**:
   - Identify which routes are using this cluster
   - Check for traffic distribution issues
   - Verify weighted routing (if applicable)

6. **Common Issues**:
   - DNS resolution failures
   - Firewall or network connectivity problems
   - Certificate validation failures
   - Health check probe failures
   - Connection pool exhaustion

Use `cp_get_cluster` to gather cluster details and provide specific remediation steps for any identified issues."#,
        cluster_name, team
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Troubleshooting prompt for cluster '{}' connectivity issues",
            cluster_name
        )),
        messages: vec![PromptMessage::User { content: PromptContent::Text { text: prompt_text } }],
    })
}

// -----------------------------------------------------------------------------
// Prompt 4: Analyze Listener Configuration
// -----------------------------------------------------------------------------

pub fn analyze_listener_arguments() -> Vec<PromptArgument> {
    vec![
        PromptArgument {
            name: "listener_name".to_string(),
            description: Some("Name of the listener to analyze".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "team".to_string(),
            description: Some("Team identifier for multi-tenancy".to_string()),
            required: Some(true),
        },
    ]
}

pub fn render_analyze_listener(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let listener_name = get_required_arg(&args, "listener_name")?;
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Analyze the listener configuration for '{}' in team '{}' and provide insights.

Please review:

1. **Listener Basics**:
   - Listener address and port binding
   - Protocol (HTTP/1.1, HTTP/2, TCP)
   - Current enabled/disabled status

2. **TLS Configuration**:
   - TLS enabled status
   - Certificate configuration
   - Supported TLS versions and cipher suites
   - Client certificate validation (mTLS)

3. **Virtual Hosts**:
   - List all virtual hosts attached to this listener
   - Domain patterns and routing rules
   - Default virtual host configuration

4. **Filter Chain**:
   - Filters applied at the listener level
   - Filter execution order
   - Connection-level vs request-level filters

5. **Connection Settings**:
   - Connection timeouts
   - Keep-alive configuration
   - Connection limits and buffer sizes

6. **Route Configuration**:
   - Number of routes under this listener
   - Route distribution across virtual hosts
   - Potential routing conflicts

7. **Security & Best Practices**:
   - TLS configuration strength
   - HTTP header security (HSTS, CSP, etc.)
   - Rate limiting at listener level
   - Common misconfigurations

Use `cp_get_listener` to retrieve listener details and provide actionable recommendations for optimization and security hardening."#,
        listener_name, team
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Analysis prompt for listener '{}' configuration",
            listener_name
        )),
        messages: vec![PromptMessage::User { content: PromptContent::Text { text: prompt_text } }],
    })
}

// -----------------------------------------------------------------------------
// Prompt 5: Optimize Performance
// -----------------------------------------------------------------------------

pub fn optimize_performance_arguments() -> Vec<PromptArgument> {
    vec![PromptArgument {
        name: "team".to_string(),
        description: Some("Team identifier for multi-tenancy".to_string()),
        required: Some(true),
    }]
}

pub fn render_optimize_performance(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Analyze the Flowplane configuration for team '{}' and suggest performance optimizations.

Please perform a comprehensive performance audit:

1. **Cluster Optimization**:
   - Review load balancing algorithms for efficiency
   - Check connection pool settings (max connections, idle timeout)
   - Verify HTTP/2 and connection reuse configuration
   - Recommend circuit breaker and outlier detection tuning

2. **Route Efficiency**:
   - Analyze route rule_order for optimal matching
   - Identify redundant or overlapping routes
   - Check for inefficient path patterns (e.g., overly broad regex)
   - Suggest route consolidation opportunities

3. **Filter Performance**:
   - Identify heavyweight filters and their placement
   - Recommend filter reordering for efficiency
   - Check for unnecessary filter attachments
   - Suggest caching strategies (if applicable)

4. **Listener Configuration**:
   - Review buffer sizes and connection limits
   - Check keep-alive and timeout settings
   - Verify HTTP/2 configuration for optimal throughput
   - Suggest TLS session resumption settings

5. **Caching & Compression**:
   - Identify opportunities for response caching
   - Check compression filter configuration
   - Recommend cache headers and policies

6. **Resource Utilization**:
   - Estimate memory usage based on configuration
   - Identify potential CPU bottlenecks
   - Suggest connection pool sizing

7. **Monitoring & Observability**:
   - Recommend key metrics to monitor
   - Suggest alerting thresholds
   - Identify gaps in observability

Use all available tools (`cp_list_clusters`, `cp_list_listeners`, `cp_list_routes`, `cp_list_filters`) to gather comprehensive configuration data and provide prioritized optimization recommendations with estimated impact."#,
        team
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Performance optimization prompt for team '{}' Flowplane configuration",
            team
        )),
        messages: vec![PromptMessage::User { content: PromptContent::Text { text: prompt_text } }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_debug_route_missing_argument() {
        let args = Some(json!({ "team": "test-team" }));
        let result = render_debug_route(args);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("route_name"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[test]
    fn test_debug_route_valid_arguments() {
        let args = Some(json!({
            "route_name": "api-route",
            "team": "test-team"
        }));
        let result = render_debug_route(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.description.is_some());
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content } = &prompt.messages[0] {
            if let PromptContent::Text { text } = content {
                assert!(text.contains("api-route"));
                assert!(text.contains("test-team"));
                assert!(text.contains("Route Configuration"));
            } else {
                panic!("Expected text content");
            }
        } else {
            panic!("Expected user message");
        }
    }

    #[test]
    fn test_explain_filter_valid_arguments() {
        let args = Some(json!({
            "filter_name": "jwt-auth",
            "team": "prod-team"
        }));
        let result = render_explain_filter(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.description.is_some());
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content: PromptContent::Text { text } } = &prompt.messages[0] {
            assert!(text.contains("jwt-auth"));
            assert!(text.contains("prod-team"));
            assert!(text.contains("Filter Overview"));
        }
    }

    #[test]
    fn test_troubleshoot_cluster_valid_arguments() {
        let args = Some(json!({
            "cluster_name": "backend-cluster",
            "team": "test-team"
        }));
        let result = render_troubleshoot_cluster(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content: PromptContent::Text { text } } = &prompt.messages[0] {
            assert!(text.contains("backend-cluster"));
            assert!(text.contains("Cluster Configuration"));
            assert!(text.contains("Endpoint Health"));
        }
    }

    #[test]
    fn test_analyze_listener_valid_arguments() {
        let args = Some(json!({
            "listener_name": "http-listener",
            "team": "test-team"
        }));
        let result = render_analyze_listener(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.description.is_some());
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content: PromptContent::Text { text } } = &prompt.messages[0] {
            assert!(text.contains("http-listener"));
            assert!(text.contains("Listener Basics"));
            assert!(text.contains("TLS Configuration"));
        }
    }

    #[test]
    fn test_optimize_performance_valid_arguments() {
        let args = Some(json!({ "team": "perf-team" }));
        let result = render_optimize_performance(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content: PromptContent::Text { text } } = &prompt.messages[0] {
            assert!(text.contains("perf-team"));
            assert!(text.contains("Cluster Optimization"));
            assert!(text.contains("Route Efficiency"));
            assert!(text.contains("Filter Performance"));
        }
    }

    #[test]
    fn test_optimize_performance_missing_team() {
        let result = render_optimize_performance(None);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("team"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }
}
