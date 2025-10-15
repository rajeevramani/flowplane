//! Tests for static Regex validation with expect() error messages
//!
//! These tests verify that:
//! 1. Static Regex patterns compile correctly at initialization
//! 2. Regex validation provides clear error messages
//! 3. Edge cases are handled properly

use flowplane::auth::validation::{validate_scope, validate_token_name};
use flowplane::utils::VALID_NAME_REGEX;

#[test]
fn test_static_regex_patterns_are_valid() {
    // This test verifies that all static Regex patterns compile successfully
    // If any expect() fails during initialization, this test would panic with a clear message

    // Test NAME_REGEX is accessible and works
    assert!(validate_token_name("valid-token-123").is_ok());
    assert!(validate_token_name("ab").is_err()); // Too short

    // Test SCOPE_REGEX is accessible and works
    assert!(validate_scope("routes:read").is_ok());
    assert!(validate_scope("invalid").is_err());

    // Test VALID_NAME_REGEX is accessible and works
    assert!(VALID_NAME_REGEX.is_match("valid_name"));
    assert!(VALID_NAME_REGEX.is_match("_starts_with_underscore"));
    assert!(!VALID_NAME_REGEX.is_match("123starts-with-number"));
}

#[test]
fn test_token_name_validation_edge_cases() {
    // Valid patterns
    assert!(validate_token_name("abc").is_ok(), "Exactly 3 chars should be valid");
    assert!(validate_token_name(&"a".repeat(64)).is_ok(), "Exactly 64 chars should be valid");
    assert!(
        validate_token_name("my-token_123").is_ok(),
        "Alphanumeric with dashes and underscores"
    );

    // Invalid patterns
    assert!(validate_token_name("ab").is_err(), "Too short (2 chars)");
    assert!(validate_token_name(&"a".repeat(65)).is_err(), "Too long (65 chars)");
    assert!(validate_token_name("my token").is_err(), "Contains space");
    assert!(validate_token_name("my.token").is_err(), "Contains period");
    assert!(validate_token_name("my@token").is_err(), "Contains special char");
    assert!(validate_token_name("").is_err(), "Empty string");
}

#[test]
fn test_scope_validation_patterns() {
    // Valid resource:action scopes
    assert!(validate_scope("routes:read").is_ok());
    assert!(validate_scope("routes:write").is_ok());
    assert!(validate_scope("clusters:read").is_ok());
    assert!(validate_scope("listeners:write").is_ok());
    assert!(validate_scope("api-definitions:read").is_ok());
    assert!(validate_scope("api-definitions:write").is_ok());

    // Valid admin scope
    assert!(validate_scope("admin:all").is_ok());

    // Valid team-scoped scopes
    assert!(validate_scope("team:platform:routes:read").is_ok());
    assert!(validate_scope("team:eng-team:clusters:write").is_ok());
    assert!(validate_scope("team:test-team:api-definitions:read").is_ok());

    // Invalid patterns
    assert!(validate_scope("Routes:Read").is_err(), "Uppercase not allowed");
    assert!(validate_scope("ADMIN:ALL").is_err(), "Uppercase not allowed");
    assert!(validate_scope("routes").is_err(), "Missing action");
    assert!(validate_scope("routes:").is_err(), "Empty action");
    assert!(validate_scope(":read").is_err(), "Empty resource");
    assert!(validate_scope("routes:read:extra").is_err(), "Too many parts for resource scope");
    assert!(validate_scope("team:only-two").is_err(), "Team scope needs 4 parts");
    assert!(validate_scope("team:a:b").is_err(), "Team scope needs 4 parts");
    assert!(validate_scope("team:platform:routes:read:extra").is_err(), "Too many parts");
    assert!(validate_scope("").is_err(), "Empty string");
}

#[test]
fn test_valid_name_regex_patterns() {
    // Valid names
    assert!(VALID_NAME_REGEX.is_match("my_resource"));
    assert!(VALID_NAME_REGEX.is_match("MyResource"));
    assert!(VALID_NAME_REGEX.is_match("my-resource-123"));
    assert!(VALID_NAME_REGEX.is_match("_private"));
    assert!(VALID_NAME_REGEX.is_match("a"));
    assert!(VALID_NAME_REGEX.is_match("A"));
    assert!(VALID_NAME_REGEX.is_match("_"));

    // Invalid names (must start with letter or underscore)
    assert!(!VALID_NAME_REGEX.is_match("123-resource"));
    assert!(!VALID_NAME_REGEX.is_match("1"));
    assert!(!VALID_NAME_REGEX.is_match("-starts-with-dash"));
    assert!(!VALID_NAME_REGEX.is_match(""));

    // Invalid characters
    assert!(!VALID_NAME_REGEX.is_match("my resource")); // space
    assert!(!VALID_NAME_REGEX.is_match("my.resource")); // dot
    assert!(!VALID_NAME_REGEX.is_match("my@resource")); // special char
    assert!(!VALID_NAME_REGEX.is_match("my/resource")); // slash
}

#[test]
fn test_unicode_and_special_characters() {
    // Regex patterns should only accept ASCII alphanumeric
    assert!(validate_token_name("café").is_err(), "Non-ASCII chars not allowed");
    assert!(validate_token_name("token™").is_err(), "Special Unicode not allowed");
    assert!(validate_token_name("トークン").is_err(), "Japanese chars not allowed");

    assert!(validate_scope("routes:読む").is_err(), "Non-ASCII in scope");
    assert!(validate_scope("マイクロサービス:read").is_err(), "Non-ASCII resource name");

    assert!(!VALID_NAME_REGEX.is_match("my_resource_™"));
    assert!(!VALID_NAME_REGEX.is_match("リソース"));
}

#[test]
fn test_boundary_conditions() {
    // Test exact boundary lengths for token names (3-64 chars)
    let min_valid = "abc";
    let max_valid = &"a".repeat(64);
    let too_short = "ab";
    let too_long = &"a".repeat(65);

    assert!(validate_token_name(min_valid).is_ok());
    assert!(validate_token_name(max_valid).is_ok());
    assert!(validate_token_name(too_short).is_err());
    assert!(validate_token_name(too_long).is_err());

    // Test empty strings
    assert!(validate_token_name("").is_err());
    assert!(validate_scope("").is_err());
    assert!(!VALID_NAME_REGEX.is_match(""));
}

#[test]
fn test_case_sensitivity() {
    // Scopes must be lowercase
    assert!(validate_scope("routes:read").is_ok());
    assert!(validate_scope("Routes:Read").is_err());
    assert!(validate_scope("ROUTES:READ").is_err());
    assert!(validate_scope("routes:READ").is_err());

    // Token names can have mixed case
    assert!(validate_token_name("MyToken123").is_ok());
    assert!(validate_token_name("mytoken123").is_ok());
    assert!(validate_token_name("MYTOKEN123").is_ok());

    // VALID_NAME_REGEX accepts both cases
    assert!(VALID_NAME_REGEX.is_match("MyResource"));
    assert!(VALID_NAME_REGEX.is_match("myresource"));
    assert!(VALID_NAME_REGEX.is_match("MYRESOURCE"));
}

#[test]
fn test_special_scope_patterns() {
    // Admin scope variations
    assert!(validate_scope("admin:all").is_ok());
    assert!(validate_scope("admin:ALL").is_err(), "Must be lowercase");
    assert!(validate_scope("Admin:all").is_err(), "Must be lowercase");

    // Hyphenated resource names
    assert!(validate_scope("api-definitions:read").is_ok());
    assert!(validate_scope("api-definitions:write").is_ok());
    assert!(validate_scope("my-custom-resource:read").is_ok());

    // Hyphenated team names
    assert!(validate_scope("team:my-team:routes:read").is_ok());
    assert!(validate_scope("team:eng-platform-team:api-definitions:write").is_ok());
}

#[test]
fn test_whitespace_handling() {
    // Leading/trailing whitespace should fail
    assert!(validate_token_name(" mytoken").is_err());
    assert!(validate_token_name("mytoken ").is_err());
    assert!(validate_token_name(" mytoken ").is_err());

    assert!(validate_scope(" routes:read").is_err());
    assert!(validate_scope("routes:read ").is_err());

    assert!(!VALID_NAME_REGEX.is_match(" resource"));
    assert!(!VALID_NAME_REGEX.is_match("resource "));

    // Internal whitespace should fail
    assert!(validate_token_name("my token").is_err());
    assert!(validate_scope("routes: read").is_err());
    assert!(!VALID_NAME_REGEX.is_match("my resource"));
}
