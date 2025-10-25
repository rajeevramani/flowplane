//! Path normalization module for API schema aggregation
//!
//! This module provides functionality to detect and normalize path parameters in API endpoints,
//! converting literal paths like `/users/123` into parameterized templates like `/users/{id}`.
//!
//! ## Supported Parameter Types
//!
//! - **Numeric IDs**: `/users/123` → `/users/{id}`
//! - **UUIDs**: `/orders/550e8400-e29b-41d4-a716-446655440000` → `/orders/{id}`
//! - **Alphanumeric codes**: `/products/ABC123` → `/products/{code}`
//! - **Composite paths**: `/users/123/orders/456` → `/users/{userId}/orders/{orderId}`
//!
//! ## Design Goals
//!
//! - Avoid false positives by carefully distinguishing literals from parameters
//! - Support configurable parameter naming conventions
//! - Provide consistent normalization across the application
//! - Handle edge cases gracefully (mixed literal/parameterized paths)

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Configuration for path normalization behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathNormalizationConfig {
    /// Enable path normalization (default: true)
    pub enabled: bool,

    /// Minimum segment length to consider for parameterization (default: 1)
    /// Prevents very short segments like "v1", "api" from being parameterized
    pub min_param_length: usize,

    /// Maximum segment length to consider for parameterization (default: 100)
    /// Prevents extremely long segments from being parameterized
    pub max_param_length: usize,
}

impl Default for PathNormalizationConfig {
    fn default() -> Self {
        Self { enabled: true, min_param_length: 1, max_param_length: 100 }
    }
}

/// Types of path parameters that can be detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterType {
    /// Numeric ID (integers)
    NumericId,
    /// UUID (v4 format)
    Uuid,
    /// Alphanumeric code (mixed letters and numbers)
    AlphanumericCode,
    /// Date (YYYY-MM-DD format)
    Date,
    /// Timestamp (Unix timestamp or ISO8601)
    Timestamp,
}

impl ParameterType {
    /// Get the default placeholder name for this parameter type
    fn default_placeholder(&self) -> &'static str {
        match self {
            ParameterType::NumericId => "id",
            ParameterType::Uuid => "id",
            ParameterType::AlphanumericCode => "code",
            ParameterType::Date => "date",
            ParameterType::Timestamp => "timestamp",
        }
    }
}

/// Compiled regex patterns for parameter detection
struct RegexPatterns {
    /// UUID pattern (loose matching for v4 UUIDs)
    uuid: Regex,
    /// Numeric ID pattern (pure numbers)
    numeric_id: Regex,
    /// Alphanumeric code pattern (mixed letters and numbers, at least 2 characters)
    alphanumeric_code: Regex,
    /// Date pattern (YYYY-MM-DD)
    date: Regex,
    /// Unix timestamp (10+ digit number)
    timestamp: Regex,
}

/// Get compiled regex patterns (singleton)
fn get_patterns() -> &'static RegexPatterns {
    static PATTERNS: OnceLock<RegexPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        RegexPatterns {
            // UUID v4 pattern: 8-4-4-4-12 hex characters
            uuid: Regex::new(
                r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
            )
            .expect("UUID regex compilation failed"),

            // Numeric ID: pure digits
            numeric_id: Regex::new(r"^\d+$").expect("Numeric ID regex compilation failed"),

            // Alphanumeric code: mix of letters and numbers (at least one of each)
            // Must have at least 2 characters total
            // We'll validate this separately since regex crate doesn't support lookahead
            alphanumeric_code: Regex::new(r"^[a-zA-Z0-9]{2,}$")
                .expect("Alphanumeric code regex compilation failed"),

            // Date: YYYY-MM-DD
            date: Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("Date regex compilation failed"),

            // Unix timestamp: 10+ digits (covers timestamps from 2001 onwards)
            timestamp: Regex::new(r"^\d{10,}$").expect("Timestamp regex compilation failed"),
        }
    })
}

/// Check if a segment looks like a common literal value that shouldn't be parameterized
fn is_common_literal(segment: &str) -> bool {
    // Version patterns: v1, v2, v1.0, v2.1, etc.
    if segment.starts_with('v') && segment.len() <= 5 {
        let rest = &segment[1..];
        // Check if rest is version-like (digits and dots)
        if rest.chars().all(|c| c.is_numeric() || c == '.') {
            return true;
        }
    }

    // Short codes with hyphens (project identifiers like "team-1", "proj-2")
    // BUT: Exclude date patterns (YYYY-MM-DD) which are 10 characters with 2 hyphens
    if segment.contains('-') && segment.len() <= 10 {
        // Check if it's a date pattern (YYYY-MM-DD)
        let parts: Vec<&str> = segment.split('-').collect();
        if parts.len() == 3 {
            // If all parts are numeric, this might be a date (will be caught by date pattern)
            if parts.iter().all(|p| p.chars().all(char::is_numeric)) {
                return false; // Let date pattern handle it
            }
        }
        return true;
    }

    // API namespace keywords that should stay literal
    let keywords = ["api", "v1", "v2", "v3", "admin", "public", "private"];
    if keywords.contains(&segment) {
        return true;
    }

    false
}

/// Detect the parameter type for a given path segment
fn detect_parameter_type(segment: &str, config: &PathNormalizationConfig) -> Option<ParameterType> {
    // Check length constraints
    if segment.len() < config.min_param_length || segment.len() > config.max_param_length {
        return None;
    }

    // Skip common literals
    if is_common_literal(segment) {
        return None;
    }

    let patterns = get_patterns();

    // Check in order of specificity (most specific first)
    if patterns.uuid.is_match(segment) {
        return Some(ParameterType::Uuid);
    }

    if patterns.date.is_match(segment) {
        return Some(ParameterType::Date);
    }

    if patterns.timestamp.is_match(segment) {
        return Some(ParameterType::Timestamp);
    }

    // Check alphanumeric code - must have both letters and numbers
    // Exclude short patterns (length < 5) to avoid false positives
    if segment.len() >= 5 && patterns.alphanumeric_code.is_match(segment) {
        let has_letter = segment.chars().any(|c| c.is_alphabetic());
        let has_digit = segment.chars().any(|c| c.is_numeric());
        if has_letter && has_digit {
            return Some(ParameterType::AlphanumericCode);
        }
    }

    if patterns.numeric_id.is_match(segment) {
        return Some(ParameterType::NumericId);
    }

    None
}

/// Generate a contextual parameter name based on the preceding segment
fn generate_parameter_name(param_type: ParameterType, previous_segment: Option<&str>) -> String {
    // If there's a preceding segment (like "users"), use it for context
    if let Some(prev) = previous_segment {
        // Remove trailing 's' for common plural nouns
        let singular =
            if prev.ends_with('s') && prev.len() > 1 { &prev[..prev.len() - 1] } else { prev };

        // Combine with parameter type's default suffix
        match param_type {
            ParameterType::NumericId | ParameterType::Uuid => {
                format!("{}Id", singular)
            }
            ParameterType::AlphanumericCode => {
                format!("{}Code", singular)
            }
            ParameterType::Date => {
                format!("{}Date", singular)
            }
            ParameterType::Timestamp => {
                format!("{}Timestamp", singular)
            }
        }
    } else {
        // Fallback to default placeholder
        param_type.default_placeholder().to_string()
    }
}

/// Normalize a single API path by detecting and replacing parameter segments
///
/// # Examples
///
/// ```
/// use flowplane::services::path_normalizer::{normalize_path, PathNormalizationConfig};
///
/// let config = PathNormalizationConfig::default();
///
/// assert_eq!(
///     normalize_path("/users/123", &config),
///     "/users/{userId}"
/// );
///
/// assert_eq!(
///     normalize_path("/orders/550e8400-e29b-41d4-a716-446655440000", &config),
///     "/orders/{orderId}"
/// );
///
/// assert_eq!(
///     normalize_path("/products/ABC123", &config),
///     "/products/{productCode}"
/// );
/// ```
pub fn normalize_path(path: &str, config: &PathNormalizationConfig) -> String {
    if !config.enabled {
        return path.to_string();
    }

    // Split path into segments
    let segments: Vec<&str> = path.split('/').collect();

    let mut normalized_segments = Vec::with_capacity(segments.len());
    let mut previous_segment: Option<&str> = None;

    for segment in segments {
        if segment.is_empty() {
            // Preserve empty segments (leading/trailing slashes)
            normalized_segments.push(segment.to_string());
            continue;
        }

        // Try to detect if this segment is a parameter
        if let Some(param_type) = detect_parameter_type(segment, config) {
            // Generate contextual parameter name
            let param_name = generate_parameter_name(param_type, previous_segment);
            normalized_segments.push(format!("{{{}}}", param_name));
        } else {
            // Keep literal segment
            normalized_segments.push(segment.to_string());
            previous_segment = Some(segment);
        }
    }

    normalized_segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> PathNormalizationConfig {
        PathNormalizationConfig::default()
    }

    #[test]
    fn test_detect_numeric_id() {
        let config = default_config();
        assert_eq!(detect_parameter_type("123", &config), Some(ParameterType::NumericId));
        assert_eq!(detect_parameter_type("0", &config), Some(ParameterType::NumericId));
        assert_eq!(detect_parameter_type("999999", &config), Some(ParameterType::NumericId));
    }

    #[test]
    fn test_detect_uuid() {
        let config = default_config();
        assert_eq!(
            detect_parameter_type("550e8400-e29b-41d4-a716-446655440000", &config),
            Some(ParameterType::Uuid)
        );
        assert_eq!(
            detect_parameter_type("123e4567-e89b-12d3-a456-426614174000", &config),
            Some(ParameterType::Uuid)
        );
    }

    #[test]
    fn test_detect_alphanumeric_code() {
        let config = default_config();
        assert_eq!(detect_parameter_type("ABC123", &config), Some(ParameterType::AlphanumericCode));
        assert_eq!(detect_parameter_type("prod1", &config), Some(ParameterType::AlphanumericCode));
        // Pure letters should not match
        assert_eq!(detect_parameter_type("ABC", &config), None);
        // Pure numbers covered by numeric_id
        assert_eq!(detect_parameter_type("123", &config), Some(ParameterType::NumericId));
    }

    #[test]
    fn test_detect_date() {
        let config = default_config();
        assert_eq!(detect_parameter_type("2024-10-25", &config), Some(ParameterType::Date));
        assert_eq!(detect_parameter_type("2023-01-01", &config), Some(ParameterType::Date));
    }

    #[test]
    fn test_normalize_simple_paths() {
        let config = default_config();

        assert_eq!(normalize_path("/users/123", &config), "/users/{userId}");

        assert_eq!(normalize_path("/products/ABC123", &config), "/products/{productCode}");

        assert_eq!(normalize_path("/api/v1/users", &config), "/api/v1/users");
    }

    #[test]
    fn test_normalize_uuid_paths() {
        let config = default_config();

        assert_eq!(
            normalize_path("/orders/550e8400-e29b-41d4-a716-446655440000", &config),
            "/orders/{orderId}"
        );
    }

    #[test]
    fn test_normalize_composite_paths() {
        let config = default_config();

        assert_eq!(
            normalize_path("/users/123/orders/456", &config),
            "/users/{userId}/orders/{orderId}"
        );

        // Hyphenated identifiers are treated as literals to avoid false positives
        // Pure numeric IDs are still parameterized
        assert_eq!(
            normalize_path("/teams/team-1/projects/proj-2/tasks/789", &config),
            "/teams/team-1/projects/proj-2/tasks/{taskId}"
        );
    }

    #[test]
    fn test_normalize_mixed_paths() {
        let config = default_config();

        // Path with both literals and parameters
        assert_eq!(
            normalize_path("/api/v1/users/123/profile", &config),
            "/api/v1/users/{userId}/profile"
        );
    }

    #[test]
    fn test_normalize_edge_cases() {
        let config = default_config();

        // Empty path
        assert_eq!(normalize_path("", &config), "");

        // Root path
        assert_eq!(normalize_path("/", &config), "/");

        // No parameters
        assert_eq!(normalize_path("/api/v1/health", &config), "/api/v1/health");

        // Trailing slash
        assert_eq!(normalize_path("/users/123/", &config), "/users/{userId}/");
    }

    #[test]
    fn test_normalization_disabled() {
        let mut config = default_config();
        config.enabled = false;

        assert_eq!(normalize_path("/users/123", &config), "/users/123");
    }

    #[test]
    fn test_length_constraints() {
        let mut config = default_config();
        config.min_param_length = 3;

        // "12" is too short
        assert_eq!(normalize_path("/users/12", &config), "/users/12");

        // "123" meets minimum
        assert_eq!(normalize_path("/users/123", &config), "/users/{userId}");
    }

    #[test]
    fn test_actual_flowplane_paths() {
        let config = default_config();

        // Real paths from the database issue
        assert_eq!(normalize_path("/anything/users/101", &config), "/anything/users/{userId}");

        assert_eq!(normalize_path("/anything/users/102", &config), "/anything/users/{userId}");

        assert_eq!(normalize_path("/anything/users/103", &config), "/anything/users/{userId}");

        // All should normalize to the same pattern
        let paths = vec![
            "/anything/users/101",
            "/anything/users/102",
            "/anything/users/103",
            "/anything/users/104",
            "/anything/users/105",
        ];

        let normalized: Vec<String> = paths.iter().map(|p| normalize_path(p, &config)).collect();

        // All should be identical
        assert_eq!(normalized.len(), 5);
        assert!(normalized.iter().all(|n| n == "/anything/users/{userId}"));
    }
}
