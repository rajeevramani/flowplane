//! CLI Integration Tests
//!
//! Comprehensive integration tests for all CLI commands including:
//! - Authentication methods and precedence
//! - OpenAPI import management
//! - Native resource management (clusters, listeners, routes)
//! - Configuration file handling
//! - Error handling and edge cases
//!
//! These tests start a real Flowplane server and execute CLI commands
//! against it to verify end-to-end functionality.

#[path = "cli_integration/support.rs"]
mod support;

#[path = "cli_integration/test_auth_methods.rs"]
mod test_auth_methods;

#[path = "cli_integration/test_native_commands.rs"]
mod test_native_commands;

#[path = "cli_integration/test_config_commands.rs"]
mod test_config_commands;

#[path = "cli_integration/test_error_handling.rs"]
mod test_error_handling;
