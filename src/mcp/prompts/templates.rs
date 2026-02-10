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

// -----------------------------------------------------------------------------
// Prompt 6: Deploy and Verify API
// -----------------------------------------------------------------------------

pub fn deploy_and_verify_arguments() -> Vec<PromptArgument> {
    vec![
        PromptArgument {
            name: "backend_host".to_string(),
            description: Some("Backend service host address (e.g., 10.0.1.5)".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "backend_port".to_string(),
            description: Some("Backend service port (e.g., 8080)".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "path".to_string(),
            description: Some("Request path to expose (e.g., /api/orders)".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "listen_port".to_string(),
            description: Some("Port for the listener to bind (e.g., 80)".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "team".to_string(),
            description: Some("Team identifier for multi-tenancy".to_string()),
            required: Some(true),
        },
    ]
}

pub fn render_deploy_and_verify(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let backend_host = get_required_arg(&args, "backend_host")?;
    let backend_port = get_required_arg(&args, "backend_port")?;
    let path = get_required_arg(&args, "path")?;
    let listen_port = get_required_arg(&args, "listen_port")?;
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Deploy a backend service at {backend_host}:{backend_port} and expose it at path '{path}' on port {listen_port} for team '{team}'.

Follow this exact workflow — each step uses a specific MCP tool:

## Step 1: Pre-Flight Validation
Call `dev_preflight_check` with:
- path: "{path}"
- listen_port: {listen_port}
- cluster_name: (derive a sensible name from the path)

If any check fails, report the conflict and suggest a resolution before proceeding.

## Step 2: Create Backend (Cluster)
Call `cp_create_cluster` with:
- name: (derive from path, e.g., "orders-svc" for /api/orders)
- endpoints: [{{ "address": "{backend_host}", "port": {backend_port} }}]

## Step 3: Create Route Configuration
Call `cp_create_route_config` with:
- name: (derive from path, e.g., "orders-routes")
- cluster_name: (same as cluster created in step 2)

## Step 4: Create Virtual Host
Call `cp_create_virtual_host` with:
- name: (derive from path, e.g., "orders-vhost")
- domains: ["*"]
- route_config_name: (same as route config from step 3)

## Step 5: Create Route
Call `cp_create_route` with:
- virtual_host_name: (from step 4)
- path: "{path}"
- cluster: (from step 2)

## Step 6: Create Listener (Entry Point)
Call `cp_query_port` to verify port {listen_port} is still available, then:
Call `cp_create_listener` with:
- name: (derive, e.g., "http-ingress")
- address: "0.0.0.0"
- port: {listen_port}
- route_config_name: (from step 3)

## Step 7: Verify Deployment
Call `cp_query_service` with:
- name: (cluster name from step 2)
Confirm endpoints, route configs, and listeners are connected.

## Step 8: Validate Configuration
Call `ops_config_validate` with:
- resource_type: "all"
Confirm no orphan clusters or route configs.

## Step 9: End-to-End Trace
Call `ops_trace_request` with:
- path: "{path}"
- port: {listen_port}
Confirm the request traces from listener → route config → virtual host → route → cluster → endpoints.

Report the complete deployment summary with all resource names created."#,
        backend_host = backend_host,
        backend_port = backend_port,
        path = path,
        listen_port = listen_port,
        team = team,
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Deploy backend at {}:{} on path '{}' port {} and verify end-to-end",
            backend_host, backend_port, path, listen_port
        )),
        messages: vec![PromptMessage::User { content: PromptContent::Text { text: prompt_text } }],
    })
}

// -----------------------------------------------------------------------------
// Prompt 7: Learn and Document API
// -----------------------------------------------------------------------------

pub fn learn_and_document_arguments() -> Vec<PromptArgument> {
    vec![
        PromptArgument {
            name: "route_config_name".to_string(),
            description: Some("Route configuration to observe for API learning".to_string()),
            required: Some(true),
        },
        PromptArgument {
            name: "team".to_string(),
            description: Some("Team identifier for multi-tenancy".to_string()),
            required: Some(true),
        },
    ]
}

pub fn render_learn_and_document(args: Option<Value>) -> Result<PromptGetResult, McpError> {
    let route_config_name = get_required_arg(&args, "route_config_name")?;
    let team = get_required_arg(&args, "team")?;

    let prompt_text = format!(
        r#"Observe live traffic on route config '{route_config_name}' for team '{team}' and generate an OpenAPI specification.

Follow this workflow — each step uses a specific MCP tool:

## Step 1: Discover the Route Config
Call `ops_trace_request` with a known path for this route config to discover:
- Which listener(s) serve it
- The virtual hosts and routes underneath
- The target cluster and endpoints

Then call `ops_topology` with:
- scope: "route_config"
- name: "{route_config_name}"
to see all paths and virtual hosts under this route config.

## Step 2: Check Existing Learning Sessions
Call `cp_list_learning_sessions` with:
- status: "active"
Check if there is already an active learning session for '{route_config_name}'.
If one exists, skip to Step 4.

## Step 3: Start Learning Session
Call `cp_create_learning_session` with:
- route_config_name: "{route_config_name}"
- target_sample_count: 100
Note the session ID for monitoring.

## Step 4: Monitor Progress
Call `cp_get_learning_session` with the session ID.
Report the current sample count and status.
If the session is still active and samples are low, inform the user that more traffic is needed.

## Step 5: Review Learned Schemas
Call `cp_list_aggregated_schemas` with:
- route_config_name: "{route_config_name}"
For each discovered path/method, call `cp_get_aggregated_schema` to get:
- Request schema (fields, types, nested objects)
- Response schema
- Observed status codes
- Sample count

## Step 6: Export OpenAPI Specification
Call `cp_export_schema_openapi` with:
- route_config_name: "{route_config_name}"
- format: "yaml"

## Step 7: Review and Enhance
Review the generated OpenAPI spec and suggest:
- Meaningful descriptions for each endpoint
- Missing field descriptions
- Potential required vs optional field corrections
- Response schema improvements based on status codes
- Security scheme additions if auth headers were observed

Present the final spec to the user with your enhancement recommendations."#,
        route_config_name = route_config_name,
        team = team,
    );

    Ok(PromptGetResult {
        description: Some(format!(
            "Learn API traffic on '{}' and generate OpenAPI specification",
            route_config_name
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

    // ====================================================================
    // Prompt 6: deploy_and_verify tests
    // ====================================================================

    #[test]
    fn test_deploy_and_verify_valid_arguments() {
        let args = Some(json!({
            "backend_host": "10.0.1.5",
            "backend_port": "8080",
            "path": "/api/orders",
            "listen_port": "80",
            "team": "test-team"
        }));
        let result = render_deploy_and_verify(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.description.is_some());
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content: PromptContent::Text { text } } = &prompt.messages[0] {
            assert!(text.contains("10.0.1.5"));
            assert!(text.contains("8080"));
            assert!(text.contains("/api/orders"));
            assert!(text.contains("dev_preflight_check"));
            assert!(text.contains("cp_create_cluster"));
            assert!(text.contains("cp_create_route_config"));
            assert!(text.contains("cp_create_virtual_host"));
            assert!(text.contains("cp_create_route"));
            assert!(text.contains("cp_create_listener"));
            assert!(text.contains("cp_query_service"));
            assert!(text.contains("ops_config_validate"));
            assert!(text.contains("ops_trace_request"));
        } else {
            panic!("Expected user message with text content");
        }
    }

    #[test]
    fn test_deploy_and_verify_missing_backend_host() {
        let args = Some(json!({
            "backend_port": "8080",
            "path": "/api/orders",
            "listen_port": "80",
            "team": "test-team"
        }));
        let result = render_deploy_and_verify(args);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("backend_host"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[test]
    fn test_deploy_and_verify_missing_team() {
        let args = Some(json!({
            "backend_host": "10.0.1.5",
            "backend_port": "8080",
            "path": "/api/orders",
            "listen_port": "80"
        }));
        let result = render_deploy_and_verify(args);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("team"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[test]
    fn test_deploy_and_verify_workflow_steps() {
        let args = Some(json!({
            "backend_host": "192.168.1.100",
            "backend_port": "3000",
            "path": "/api/users",
            "listen_port": "443",
            "team": "prod-team"
        }));
        let result = render_deploy_and_verify(args).unwrap();

        if let PromptMessage::User { content: PromptContent::Text { text } } = &result.messages[0] {
            // Verify all 9 steps are present in order
            assert!(text.contains("Step 1: Pre-Flight Validation"));
            assert!(text.contains("Step 2: Create Backend"));
            assert!(text.contains("Step 3: Create Route Configuration"));
            assert!(text.contains("Step 4: Create Virtual Host"));
            assert!(text.contains("Step 5: Create Route"));
            assert!(text.contains("Step 6: Create Listener"));
            assert!(text.contains("Step 7: Verify Deployment"));
            assert!(text.contains("Step 8: Validate Configuration"));
            assert!(text.contains("Step 9: End-to-End Trace"));

            // Verify args are interpolated
            assert!(text.contains("192.168.1.100"));
            assert!(text.contains("3000"));
            assert!(text.contains("/api/users"));
            assert!(text.contains("443"));
            assert!(text.contains("prod-team"));
        }
    }

    // ====================================================================
    // Prompt 7: learn_and_document tests
    // ====================================================================

    #[test]
    fn test_learn_and_document_valid_arguments() {
        let args = Some(json!({
            "route_config_name": "orders-routes",
            "team": "test-team"
        }));
        let result = render_learn_and_document(args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.description.is_some());
        assert_eq!(prompt.messages.len(), 1);

        if let PromptMessage::User { content: PromptContent::Text { text } } = &prompt.messages[0] {
            assert!(text.contains("orders-routes"));
            assert!(text.contains("test-team"));
            assert!(text.contains("ops_trace_request"));
            assert!(text.contains("ops_topology"));
            assert!(text.contains("cp_list_learning_sessions"));
            assert!(text.contains("cp_create_learning_session"));
            assert!(text.contains("cp_get_learning_session"));
            assert!(text.contains("cp_list_aggregated_schemas"));
            assert!(text.contains("cp_get_aggregated_schema"));
            assert!(text.contains("cp_export_schema_openapi"));
        } else {
            panic!("Expected user message with text content");
        }
    }

    #[test]
    fn test_learn_and_document_missing_route_config() {
        let args = Some(json!({ "team": "test-team" }));
        let result = render_learn_and_document(args);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("route_config_name"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[test]
    fn test_learn_and_document_workflow_steps() {
        let args = Some(json!({
            "route_config_name": "payments-routes",
            "team": "prod-team"
        }));
        let result = render_learn_and_document(args).unwrap();

        if let PromptMessage::User { content: PromptContent::Text { text } } = &result.messages[0] {
            assert!(text.contains("Step 1: Discover the Route Config"));
            assert!(text.contains("Step 2: Check Existing Learning Sessions"));
            assert!(text.contains("Step 3: Start Learning Session"));
            assert!(text.contains("Step 4: Monitor Progress"));
            assert!(text.contains("Step 5: Review Learned Schemas"));
            assert!(text.contains("Step 6: Export OpenAPI Specification"));
            assert!(text.contains("Step 7: Review and Enhance"));

            assert!(text.contains("payments-routes"));
            assert!(text.contains("prod-team"));
        }
    }
}

// =============================================================================
// DEMO SCENARIO INTEGRATION TESTS
//
// These tests call actual MCP tool execute functions in sequence against a
// seeded TestDatabase, verifying the tool sequences from each demo scenario
// produce correct results.
// =============================================================================
#[cfg(test)]
mod demo_integration_tests {
    use crate::mcp::protocol::ContentBlock;
    use crate::mcp::tools::{
        execute_dev_preflight_check, execute_ops_config_validate, execute_ops_topology,
        execute_ops_trace_request, execute_query_service,
    };
    use crate::storage::test_helpers::{seed_reporting_data, TestDatabase, TEAM_A_ID, TEAM_B_ID};
    use serde_json::{json, Value};

    /// Helper: extract JSON output from tool result
    fn extract_json(result: &crate::mcp::protocol::ToolCallResult) -> Value {
        match &result.content[0] {
            ContentBlock::Text { text } => serde_json::from_str(text).expect("valid JSON output"),
            _ => panic!("expected text content block"),
        }
    }

    /// Helper: create a TestDatabase with reporting seed data
    async fn seeded_db(name: &str) -> TestDatabase {
        let db = TestDatabase::new(name).await;
        seed_reporting_data(&db.pool).await;
        db
    }

    // ========================================================================
    // Demo A: "Deploy and Verify an API"
    //
    // Simulates the post-creation verification workflow.
    // Seed data acts as if resources were already created.
    // Steps: preflight (on unused path) → query_service → config_validate → trace_request
    // ========================================================================

    #[tokio::test]
    async fn test_demo_a_deploy_and_verify() {
        let db = seeded_db("demo_a_deploy").await;

        // Step 1: Preflight check on new resources (should all pass)
        let preflight = execute_dev_preflight_check(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({
                "path": "/api/new-service",
                "listen_port": 3000,
                "cluster_name": "new-svc"
            }),
        )
        .await
        .expect("preflight should succeed");
        let pf_json = extract_json(&preflight);
        assert_eq!(pf_json["ready"], true, "preflight should pass for unused resources");

        // Step 7 (from demo): Verify deployment of existing orders-svc
        let service =
            execute_query_service(&db.pool, TEAM_A_ID, None, json!({"name": "orders-svc"}))
                .await
                .expect("query_service should succeed");
        let svc_json = extract_json(&service);
        assert_eq!(svc_json["success"], true);
        assert!(svc_json["cluster"].is_object(), "cluster should be found");

        // Step 8: Validate configuration
        let validate =
            execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({"resource_type": "all"}))
                .await
                .expect("config_validate should succeed");
        let val_json = extract_json(&validate);
        assert_eq!(val_json["success"], true);

        // Step 9: Trace the deployed path
        let trace = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/orders", "port": 8080}),
        )
        .await
        .expect("trace_request should succeed");
        let trace_json = extract_json(&trace);
        assert_eq!(trace_json["success"], true);
        assert!(
            trace_json["match_count"].as_i64().unwrap() > 0,
            "trace should find the orders path"
        );
        assert!(
            !trace_json["matches"].as_array().unwrap().is_empty(),
            "should have at least one match"
        );
    }

    // ========================================================================
    // Demo B: "Diagnose a Broken API"
    //
    // Simulates diagnosing a 404: trace a missing path, inspect topology,
    // and validate config to find orphans.
    // ========================================================================

    #[tokio::test]
    async fn test_demo_b_diagnose_broken_api() {
        let db = seeded_db("demo_b_diagnose").await;

        // Step 1: Trace a path that doesn't exist for team-a
        let trace = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/payments", "port": 8080}),
        )
        .await
        .expect("trace should succeed even for miss");
        let trace_json = extract_json(&trace);
        assert_eq!(trace_json["match_count"], 0, "path should not be found for team-a");
        assert!(trace_json["unmatched_reason"].is_string(), "should provide unmatched_reason");

        // Step 3: View full topology to understand the gateway layout
        let topology = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"scope": "full", "include_details": true}),
        )
        .await
        .expect("topology should succeed");
        let topo_json = extract_json(&topology);
        assert_eq!(topo_json["success"], true);
        assert!(
            topo_json["summary"]["listener_count"].as_i64().unwrap() > 0,
            "should show team-a listeners"
        );

        // Step 4: Validate config to find problems
        let validate =
            execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({"resource_type": "all"}))
                .await
                .expect("config_validate should succeed");
        let val_json = extract_json(&validate);
        assert_eq!(val_json["success"], true);

        // Orphan cluster should be detected in issues array
        let issues = val_json["issues"].as_array().expect("should have issues array");
        let orphan_cluster_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_cluster").collect();
        assert!(!orphan_cluster_issues.is_empty(), "should detect orphan-cluster in team-a");
    }

    // ========================================================================
    // Demo C: "Gateway Health Check"
    //
    // Full audit: topology overview + config validation.
    // (ops_audit_query is tested separately as it requires audit log entries.)
    // ========================================================================

    #[tokio::test]
    async fn test_demo_c_gateway_health_check() {
        let db = seeded_db("demo_c_health").await;

        // Step 1: Full topology view
        let topology = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"scope": "full", "include_details": true}),
        )
        .await
        .expect("topology should succeed");
        let topo_json = extract_json(&topology);
        assert_eq!(topo_json["success"], true);

        let summary = &topo_json["summary"];
        assert!(summary["listener_count"].as_i64().unwrap() >= 1);
        assert!(summary["cluster_count"].as_i64().unwrap() >= 1);
        assert!(summary["route_config_count"].as_i64().unwrap() >= 1);

        // Step 2: Validate all config
        let validate =
            execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({"resource_type": "all"}))
                .await
                .expect("config_validate should succeed");
        let val_json = extract_json(&validate);

        // Should find orphan issues for team-a
        let issues = val_json["issues"].as_array().expect("should have issues");
        let orphan_cluster_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_cluster").collect();
        let orphan_rc_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_route_config").collect();
        assert!(!orphan_cluster_issues.is_empty(), "should detect orphan cluster");
        assert!(!orphan_rc_issues.is_empty(), "should detect orphan route config");

        // Verify team isolation: team-b data should not appear in topology
        let rows = topo_json["rows"].as_array().unwrap();
        for row in rows {
            if let Some(listener) = row.get("listener_name") {
                assert_ne!(
                    listener.as_str().unwrap(),
                    "http-9090",
                    "team-b listener should not appear"
                );
            }
        }
    }

    // ========================================================================
    // Demo D: "Learn and Document API"
    //
    // Tests the diagnostic discovery steps of the learn workflow.
    // Learning session creation and OpenAPI export require xds_state,
    // so this test covers the discovery + topology portion.
    // ========================================================================

    #[tokio::test]
    async fn test_demo_d_discover_before_learning() {
        let db = seeded_db("demo_d_learn").await;

        // Step 1: Trace a known path to discover the route config
        let trace = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/orders", "port": 8080}),
        )
        .await
        .expect("trace should succeed");
        let trace_json = extract_json(&trace);
        assert_eq!(trace_json["success"], true);
        assert!(trace_json["match_count"].as_i64().unwrap() > 0);

        // Verify we can see the route config name in the match
        let matches = trace_json["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        let first = &matches[0];
        assert_eq!(first["route_config_name"].as_str().unwrap(), "orders-routes");

        // Step 1b: View topology scoped to that route config
        let topology = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"scope": "route_config", "name": "orders-routes", "include_details": true}),
        )
        .await
        .expect("topology should succeed");
        let topo_json = extract_json(&topology);
        assert_eq!(topo_json["success"], true);

        // Should show the orders-routes structure
        let rows = topo_json["rows"].as_array().unwrap();
        assert!(!rows.is_empty(), "should have topology rows for orders-routes");
    }

    // ========================================================================
    // Cross-demo: Team isolation across all demo tools
    // ========================================================================

    #[tokio::test]
    async fn test_demo_team_isolation() {
        let db = seeded_db("demo_isolation").await;

        // Team-A should not see team-B's payments path
        let trace_b = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/payments", "port": 9090}),
        )
        .await
        .expect("trace should succeed");
        let trace_b_json = extract_json(&trace_b);
        assert_eq!(
            trace_b_json["match_count"], 0,
            "team-a should not trace team-b's payments path"
        );

        // Team-B should not see team-A's service
        let service_a =
            execute_query_service(&db.pool, TEAM_B_ID, None, json!({"name": "orders-svc"}))
                .await
                .expect("query should succeed");
        let svc_json = extract_json(&service_a);
        assert!(svc_json["cluster"].is_null(), "team-b should not see team-a's orders-svc");

        // Team-B topology should not contain team-A listeners
        let topo_b = execute_ops_topology(
            &db.pool,
            TEAM_B_ID,
            None,
            json!({"scope": "full", "include_details": true}),
        )
        .await
        .expect("topology should succeed");
        let topo_b_json = extract_json(&topo_b);
        let summary = &topo_b_json["summary"];
        // Team-B has 1 listener (http-9090), should NOT have team-A's http-8080
        assert_eq!(summary["listener_count"].as_i64().unwrap(), 1);
    }
}
