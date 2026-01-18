//! MCP Prompts Module
//!
//! Provides templated prompts for common Flowplane operations and troubleshooting workflows.

pub mod registry;
pub mod templates;

use crate::mcp::error::McpError;
use crate::mcp::protocol::{Prompt, PromptGetResult};
use registry::PromptRegistry;
use serde_json::Value;
use std::sync::OnceLock;

static PROMPT_REGISTRY: OnceLock<PromptRegistry> = OnceLock::new();

/// Initialize and return the global prompt registry
fn get_registry() -> &'static PromptRegistry {
    PROMPT_REGISTRY.get_or_init(|| {
        let mut registry = PromptRegistry::new();

        // Register debug_route prompt
        registry.register(
            "debug_route".to_string(),
            Some("Analyze route configuration issues and provide diagnostic insights".to_string()),
            Some(templates::debug_route_arguments()),
            templates::render_debug_route,
        );

        // Register explain_filter prompt
        registry.register(
            "explain_filter".to_string(),
            Some(
                "Explain filter configuration and behavior in the request/response pipeline"
                    .to_string(),
            ),
            Some(templates::explain_filter_arguments()),
            templates::render_explain_filter,
        );

        // Register troubleshoot_cluster prompt
        registry.register(
            "troubleshoot_cluster".to_string(),
            Some("Diagnose cluster connectivity and health issues".to_string()),
            Some(templates::troubleshoot_cluster_arguments()),
            templates::render_troubleshoot_cluster,
        );

        // Register analyze_listener prompt
        registry.register(
            "analyze_listener".to_string(),
            Some(
                "Analyze listener configuration and provide optimization recommendations"
                    .to_string(),
            ),
            Some(templates::analyze_listener_arguments()),
            templates::render_analyze_listener,
        );

        // Register optimize_performance prompt
        registry.register(
            "optimize_performance".to_string(),
            Some("Suggest performance optimizations for Flowplane configuration".to_string()),
            Some(templates::optimize_performance_arguments()),
            templates::render_optimize_performance,
        );

        registry
    })
}

/// Get all available prompts
///
/// Returns a vector of all registered prompt definitions.
pub fn get_all_prompts() -> Vec<Prompt> {
    get_registry().list_all()
}

/// Get a specific prompt by name and render it with arguments
///
/// # Arguments
///
/// * `name` - Name of the prompt to retrieve
/// * `arguments` - Optional JSON value containing prompt arguments
///
/// # Returns
///
/// Result containing the rendered prompt or an error if the prompt is not found
/// or arguments are invalid.
pub fn get_prompt(name: &str, arguments: Option<Value>) -> Result<PromptGetResult, McpError> {
    get_registry().render(name, arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_get_all_prompts() {
        let prompts = get_all_prompts();
        assert_eq!(prompts.len(), 5);

        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"debug_route"));
        assert!(names.contains(&"explain_filter"));
        assert!(names.contains(&"troubleshoot_cluster"));
        assert!(names.contains(&"analyze_listener"));
        assert!(names.contains(&"optimize_performance"));
    }

    #[test]
    fn test_get_prompt_debug_route() {
        let args = Some(json!({
            "route_name": "test-route",
            "team": "test-team"
        }));

        let result = get_prompt("debug_route", args);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.description.is_some());
        assert!(!prompt.messages.is_empty());
    }

    #[test]
    fn test_get_prompt_explain_filter() {
        let args = Some(json!({
            "filter_name": "test-filter",
            "team": "test-team"
        }));

        let result = get_prompt("explain_filter", args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_prompt_not_found() {
        let result = get_prompt("unknown_prompt", None);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("Prompt not found"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[test]
    fn test_get_prompt_missing_required_arg() {
        let args = Some(json!({ "team": "test-team" })); // Missing route_name

        let result = get_prompt("debug_route", args);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("route_name"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[test]
    fn test_prompts_have_descriptions() {
        let prompts = get_all_prompts();

        for prompt in prompts {
            assert!(
                prompt.description.is_some(),
                "Prompt '{}' should have a description",
                prompt.name
            );
        }
    }

    #[test]
    fn test_prompts_have_arguments() {
        let prompts = get_all_prompts();

        for prompt in prompts {
            assert!(prompt.arguments.is_some(), "Prompt '{}' should have arguments", prompt.name);

            let args = prompt.arguments.as_ref().unwrap();
            assert!(!args.is_empty(), "Prompt '{}' should have at least one argument", prompt.name);

            // All our prompts require 'team' argument
            assert!(
                args.iter().any(|a| a.name == "team"),
                "Prompt '{}' should have 'team' argument",
                prompt.name
            );
        }
    }
}
