//! Schema diffing and breaking change detection
//!
//! This module implements logic to compare two JSON schemas and detect breaking changes.
//! A breaking change is any modification that could cause existing API clients to fail.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Type of breaking change detected
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakingChangeType {
    /// A required field was removed from the schema
    RequiredFieldRemoved,
    /// A field's type changed incompatibly (e.g., string -> integer)
    IncompatibleTypeChange,
    /// A new required field was added without a default value
    RequiredFieldAdded,
    /// An optional field became required
    FieldBecameRequired,
    /// The schema type changed (e.g., object -> array)
    SchemaTypeChanged,
}

/// Details of a breaking change detected between schema versions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BreakingChange {
    /// Type of breaking change
    #[serde(rename = "type")]
    pub change_type: BreakingChangeType,
    /// JSON path to the field that changed (e.g., "$.user.email")
    pub path: String,
    /// Description of the change for human readers
    pub description: String,
    /// Previous value (for type changes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<String>,
    /// New value (for type changes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
}

/// Result of schema comparison
#[derive(Debug, Clone, Default)]
pub struct SchemaDiff {
    /// Breaking changes detected
    pub breaking_changes: Vec<BreakingChange>,
    /// Non-breaking changes (for informational purposes)
    pub non_breaking_changes: Vec<String>,
}

impl SchemaDiff {
    /// Check if there are any breaking changes
    pub fn has_breaking_changes(&self) -> bool {
        !self.breaking_changes.is_empty()
    }
}

/// Compare two schemas and detect breaking changes
///
/// This function recursively compares old and new schemas to identify:
/// - Removed required fields
/// - Incompatible type changes
/// - New required fields
/// - Fields that became required
/// - Schema type changes
pub fn detect_breaking_changes(
    old_schema: &serde_json::Value,
    new_schema: &serde_json::Value,
) -> SchemaDiff {
    let mut diff = SchemaDiff::default();
    compare_schemas(old_schema, new_schema, "$", &mut diff);
    diff
}

/// Recursively compare two schema objects
fn compare_schemas(
    old: &serde_json::Value,
    new: &serde_json::Value,
    path: &str,
    diff: &mut SchemaDiff,
) {
    // Check for schema type changes
    if let (Some(old_type), Some(new_type)) = (get_schema_type(old), get_schema_type(new)) {
        if old_type != new_type {
            // Type changed - this is breaking
            diff.breaking_changes.push(BreakingChange {
                change_type: BreakingChangeType::SchemaTypeChanged,
                path: path.to_string(),
                description: format!("Schema type changed from {} to {}", old_type, new_type),
                old_value: Some(old_type.clone()),
                new_value: Some(new_type.clone()),
            });
            return; // Don't continue if fundamental type changed
        }
    }

    // For object schemas, compare properties and required fields
    if is_object_schema(old) && is_object_schema(new) {
        compare_object_schemas(old, new, path, diff);
    }

    // For array schemas, compare items
    if is_array_schema(old) && is_array_schema(new) {
        if let (Some(old_items), Some(new_items)) = (old.get("items"), new.get("items")) {
            let item_path = format!("{}.items", path);
            compare_schemas(old_items, new_items, &item_path, diff);
        }
    }
}

/// Compare object schemas
fn compare_object_schemas(
    old: &serde_json::Value,
    new: &serde_json::Value,
    path: &str,
    diff: &mut SchemaDiff,
) {
    let old_props = get_properties(old);
    let new_props = get_properties(new);
    let old_required = get_required_fields(old);
    let new_required = get_required_fields(new);

    // Check for removed fields
    for (field_name, old_field_schema) in &old_props {
        let field_path = format!("{}.{}", path, field_name);

        if !new_props.contains_key(field_name) {
            // Field was removed
            if old_required.contains(field_name) {
                // Removed required field - BREAKING
                diff.breaking_changes.push(BreakingChange {
                    change_type: BreakingChangeType::RequiredFieldRemoved,
                    path: field_path.clone(),
                    description: format!("Required field '{}' was removed", field_name),
                    old_value: None,
                    new_value: None,
                });
            } else {
                // Removed optional field - non-breaking
                diff.non_breaking_changes
                    .push(format!("Optional field '{}' removed at {}", field_name, field_path));
            }
        } else {
            // Field exists in both schemas - check for type changes
            let new_field_schema = &new_props[field_name];
            let had_type_change = compare_field_types(
                old_field_schema,
                new_field_schema,
                &field_path,
                field_name,
                diff,
            );

            // Only recursively compare nested schemas if there was no type change
            // (type change already detected at field level)
            if !had_type_change {
                compare_schemas(old_field_schema, new_field_schema, &field_path, diff);
            }
        }
    }

    // Check for new required fields
    for field_name in &new_required {
        if !old_props.contains_key(field_name) {
            // New required field added - BREAKING
            let field_path = format!("{}.{}", path, field_name);
            diff.breaking_changes.push(BreakingChange {
                change_type: BreakingChangeType::RequiredFieldAdded,
                path: field_path,
                description: format!(
                    "New required field '{}' added without default value",
                    field_name
                ),
                old_value: None,
                new_value: None,
            });
        } else if !old_required.contains(field_name) {
            // Field became required - BREAKING
            let field_path = format!("{}.{}", path, field_name);
            diff.breaking_changes.push(BreakingChange {
                change_type: BreakingChangeType::FieldBecameRequired,
                path: field_path,
                description: format!("Optional field '{}' became required", field_name),
                old_value: Some("optional".to_string()),
                new_value: Some("required".to_string()),
            });
        }
    }

    // Check for new optional fields (non-breaking)
    for field_name in new_props.keys() {
        if !old_props.contains_key(field_name) && !new_required.contains(field_name) {
            let field_path = format!("{}.{}", path, field_name);
            diff.non_breaking_changes
                .push(format!("New optional field '{}' added at {}", field_name, field_path));
        }
    }
}

/// Compare field types to detect incompatible changes
/// Returns true if a type change was detected (breaking or not)
fn compare_field_types(
    old_schema: &serde_json::Value,
    new_schema: &serde_json::Value,
    path: &str,
    field_name: &str,
    diff: &mut SchemaDiff,
) -> bool {
    let old_type = get_field_type_str(old_schema);
    let new_type = get_field_type_str(new_schema);

    if old_type != new_type {
        if !are_types_compatible(&old_type, &new_type) {
            // Incompatible type change - BREAKING
            diff.breaking_changes.push(BreakingChange {
                change_type: BreakingChangeType::IncompatibleTypeChange,
                path: path.to_string(),
                description: format!(
                    "Field '{}' type changed incompatibly from {} to {}",
                    field_name, old_type, new_type
                ),
                old_value: Some(old_type),
                new_value: Some(new_type),
            });
        }
        return true; // Type changed (either compatible or incompatible)
    }

    false // No type change
}

/// Get schema type as string
fn get_schema_type(schema: &serde_json::Value) -> Option<String> {
    schema.get("type").and_then(|t| {
        if let Some(s) = t.as_str() {
            Some(s.to_string())
        } else if let Some(obj) = t.as_object() {
            // Handle oneOf type conflicts
            if obj.contains_key("oneof") {
                Some("oneof".to_string())
            } else {
                None
            }
        } else {
            None
        }
    })
}

/// Get field type as a string representation
fn get_field_type_str(schema: &serde_json::Value) -> String {
    if let Some(type_val) = schema.get("type") {
        if let Some(s) = type_val.as_str() {
            return s.to_string();
        } else if let Some(obj) = type_val.as_object() {
            // Handle oneOf (type conflicts)
            if let Some(oneof) = obj.get("oneof") {
                if let Some(types) = oneof.as_array() {
                    let type_names: Vec<String> =
                        types.iter().filter_map(|t| t.as_str()).map(|s| s.to_string()).collect();
                    return format!("oneOf[{}]", type_names.join(", "));
                }
            }
        }
    }
    "unknown".to_string()
}

/// Check if two types are compatible
/// Compatible means: old clients expecting old_type can handle new_type
fn are_types_compatible(old_type: &str, new_type: &str) -> bool {
    // Same type is always compatible
    if old_type == new_type {
        return true;
    }

    // oneOf is compatible if it includes the old type
    if new_type.starts_with("oneOf[") {
        return new_type.contains(old_type);
    }

    // Integer is compatible with number (narrowing)
    if old_type == "number" && new_type == "integer" {
        return true;
    }

    // Other combinations are incompatible
    false
}

/// Check if schema is an object type
fn is_object_schema(schema: &serde_json::Value) -> bool {
    schema.get("type").and_then(|t| t.as_str()) == Some("object")
}

/// Check if schema is an array type
fn is_array_schema(schema: &serde_json::Value) -> bool {
    schema.get("type").and_then(|t| t.as_str()) == Some("array")
}

/// Get properties from an object schema
fn get_properties(schema: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    schema.get("properties").and_then(|p| p.as_object()).cloned().unwrap_or_default()
}

/// Get required fields from an object schema
fn get_required_fields(schema: &serde_json::Value) -> HashSet<String> {
    schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_changes() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id"]
        });

        let diff = detect_breaking_changes(&schema, &schema);
        assert!(!diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 0);
    }

    #[test]
    fn test_required_field_removed() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id", "name"]
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            },
            "required": ["id"]
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 1);
        assert_eq!(diff.breaking_changes[0].change_type, BreakingChangeType::RequiredFieldRemoved);
        assert!(diff.breaking_changes[0].path.contains("name"));
    }

    #[test]
    fn test_optional_field_removed_non_breaking() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "email": {"type": "string"}
            },
            "required": ["id"]
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            },
            "required": ["id"]
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(!diff.has_breaking_changes());
        assert_eq!(diff.non_breaking_changes.len(), 1);
    }

    #[test]
    fn test_incompatible_type_change() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "age": {"type": "string"}
            }
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "age": {"type": "integer"}
            }
        });

        let diff = detect_breaking_changes(&old, &new);

        assert!(diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 1);
        assert_eq!(
            diff.breaking_changes[0].change_type,
            BreakingChangeType::IncompatibleTypeChange
        );
    }

    #[test]
    fn test_required_field_added() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            },
            "required": ["id"]
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "email": {"type": "string"}
            },
            "required": ["id", "email"]
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 1);
        assert_eq!(diff.breaking_changes[0].change_type, BreakingChangeType::RequiredFieldAdded);
    }

    #[test]
    fn test_field_became_required() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id"]
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id", "name"]
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 1);
        assert_eq!(diff.breaking_changes[0].change_type, BreakingChangeType::FieldBecameRequired);
    }

    #[test]
    fn test_schema_type_changed() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            }
        });

        let new = serde_json::json!({
            "type": "array",
            "items": {"type": "integer"}
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 1);
        assert_eq!(diff.breaking_changes[0].change_type, BreakingChangeType::SchemaTypeChanged);
    }

    #[test]
    fn test_optional_field_added_non_breaking() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            },
            "required": ["id"]
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "email": {"type": "string"}
            },
            "required": ["id"]
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(!diff.has_breaking_changes());
        assert_eq!(diff.non_breaking_changes.len(), 1);
    }

    #[test]
    fn test_nested_field_changes() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "age": {"type": "integer"}
                    },
                    "required": ["name", "age"]
                }
            }
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"}
                    },
                    "required": ["name"]
                }
            }
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(diff.has_breaking_changes());
        assert_eq!(diff.breaking_changes.len(), 1);
        assert_eq!(diff.breaking_changes[0].change_type, BreakingChangeType::RequiredFieldRemoved);
        assert!(diff.breaking_changes[0].path.contains("user.age"));
    }

    #[test]
    fn test_compatible_type_widening() {
        // integer -> number is compatible (widening)
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {"type": "number"}
            }
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {"type": "integer"}
            }
        });

        let diff = detect_breaking_changes(&old, &new);
        // This should be non-breaking (narrowing is OK)
        assert!(!diff.has_breaking_changes());
    }

    #[test]
    fn test_oneof_type_compatibility() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {"type": "string"}
            }
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "type": {
                        "oneof": ["string", "integer"]
                    }
                }
            }
        });

        let diff = detect_breaking_changes(&old, &new);
        // oneOf that includes the old type is compatible
        assert!(!diff.has_breaking_changes());
    }

    #[test]
    fn test_multiple_breaking_changes() {
        let old = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "email": {"type": "string"}
            },
            "required": ["id", "name"]
        });

        let new = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "email": {"type": "string"}
            },
            "required": ["id", "email"]
        });

        let diff = detect_breaking_changes(&old, &new);
        assert!(diff.has_breaking_changes());
        // Should detect: required field removed (name), type changed (id), field became required (email)
        assert!(diff.breaking_changes.len() >= 2);
    }
}
