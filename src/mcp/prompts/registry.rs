//! Prompt Registry
//!
//! Central registry for MCP prompts with lookup and listing capabilities.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{Prompt, PromptArgument, PromptGetResult};
use serde_json::Value;
use std::collections::HashMap;

/// Type alias for prompt rendering function
pub type PromptRenderer = fn(Option<Value>) -> Result<PromptGetResult, McpError>;

/// Prompt entry in the registry
pub struct PromptEntry {
    pub definition: Prompt,
    pub renderer: PromptRenderer,
}

/// Registry for managing prompts
pub struct PromptRegistry {
    prompts: HashMap<String, PromptEntry>,
}

impl PromptRegistry {
    /// Create a new empty prompt registry
    pub fn new() -> Self {
        Self { prompts: HashMap::new() }
    }

    /// Register a prompt with its renderer
    pub fn register(
        &mut self,
        name: String,
        description: Option<String>,
        arguments: Option<Vec<PromptArgument>>,
        renderer: PromptRenderer,
    ) {
        let definition = Prompt { name: name.clone(), description, arguments };
        self.prompts.insert(name, PromptEntry { definition, renderer });
    }

    /// Get a prompt by name
    pub fn get(&self, name: &str) -> Option<&PromptEntry> {
        self.prompts.get(name)
    }

    /// List all registered prompts
    pub fn list_all(&self) -> Vec<Prompt> {
        self.prompts.values().map(|entry| entry.definition.clone()).collect()
    }

    /// Render a prompt with arguments
    pub fn render(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<PromptGetResult, McpError> {
        let entry = self
            .get(name)
            .ok_or_else(|| McpError::InvalidParams(format!("Prompt not found: {}", name)))?;

        (entry.renderer)(arguments)
    }
}

impl Default for PromptRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::{PromptContent, PromptMessage};

    fn test_renderer(_args: Option<Value>) -> Result<PromptGetResult, McpError> {
        Ok(PromptGetResult {
            messages: vec![PromptMessage::User {
                content: PromptContent::Text { text: "Test prompt".to_string() },
            }],
            description: Some("Test description".to_string()),
        })
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = PromptRegistry::new();

        registry.register(
            "test_prompt".to_string(),
            Some("A test prompt".to_string()),
            None,
            test_renderer,
        );

        let entry = registry.get("test_prompt");
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.definition.name, "test_prompt");
        assert_eq!(entry.definition.description, Some("A test prompt".to_string()));
    }

    #[test]
    fn test_registry_get_missing() {
        let registry = PromptRegistry::new();
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn test_registry_list_all() {
        let mut registry = PromptRegistry::new();

        registry.register("prompt1".to_string(), None, None, test_renderer);
        registry.register("prompt2".to_string(), None, None, test_renderer);
        registry.register("prompt3".to_string(), None, None, test_renderer);

        let prompts = registry.list_all();
        assert_eq!(prompts.len(), 3);

        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"prompt1"));
        assert!(names.contains(&"prompt2"));
        assert!(names.contains(&"prompt3"));
    }

    #[test]
    fn test_registry_render() {
        let mut registry = PromptRegistry::new();
        registry.register("test".to_string(), None, None, test_renderer);

        let result = registry.render("test", None);
        assert!(result.is_ok());

        let result = result.unwrap();
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.description, Some("Test description".to_string()));
    }

    #[test]
    fn test_registry_render_missing() {
        let registry = PromptRegistry::new();
        let result = registry.render("missing", None);
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("Prompt not found"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }
}
