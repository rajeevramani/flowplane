//! Tests for the VALID_NAME_REGEX utility pattern.

use flowplane::utils::VALID_NAME_REGEX;

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

    // Unicode
    assert!(!VALID_NAME_REGEX.is_match("my_resource_™"));
    assert!(!VALID_NAME_REGEX.is_match("リソース"));

    // Whitespace
    assert!(!VALID_NAME_REGEX.is_match(" resource"));
    assert!(!VALID_NAME_REGEX.is_match("resource "));
    assert!(!VALID_NAME_REGEX.is_match("my resource"));

    // Case sensitivity
    assert!(VALID_NAME_REGEX.is_match("MyResource"));
    assert!(VALID_NAME_REGEX.is_match("myresource"));
    assert!(VALID_NAME_REGEX.is_match("MYRESOURCE"));
}
