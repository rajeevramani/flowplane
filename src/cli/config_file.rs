//! Shared config file loader for CLI commands
//!
//! Supports YAML (.yaml, .yml) and JSON (.json) files for resource creation and updates.

use std::path::Path;

use anyhow::{Context, Result};

/// Help text for file arguments across CLI commands.
pub const FILE_ARG_HELP: &str = "Path to YAML or JSON file with resource spec";

/// Load a config file and parse it as a JSON Value.
///
/// Format is detected by file extension:
/// - `.yaml` / `.yml` -> parsed as YAML
/// - `.json` -> parsed as JSON
///
/// Returns an error for unsupported extensions.
pub fn load_config_file(path: &Path) -> Result<serde_json::Value> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "yaml" | "yml" => serde_yaml::from_str(&contents)
            .with_context(|| format!("Invalid YAML in {}", path.display())),
        "json" => serde_json::from_str(&contents)
            .with_context(|| format!("Invalid JSON in {}", path.display())),
        _ => anyhow::bail!(
            "Unsupported file extension '{}'. Use .yaml, .yml, or .json extension",
            path.display()
        ),
    }
}

/// Strip the `kind` field from a top-level JSON object.
///
/// Scaffold output includes `kind` for `apply -f` compatibility, but `create -f`
/// and `update -f` send directly to the API which doesn't expect it.
pub fn strip_kind_field(body: &mut serde_json::Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("kind");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp_file(ext: &str, content: &str) -> NamedTempFile {
        let suffix = format!(".{ext}");
        let mut f = tempfile::Builder::new().suffix(&suffix).tempfile().expect("create temp file");
        f.write_all(content.as_bytes()).expect("write temp file");
        f
    }

    #[test]
    fn yaml_file_parses() {
        let f = write_temp_file("yaml", "name: test\nport: 8080\n");
        let val = load_config_file(f.path()).expect("should parse yaml");
        assert_eq!(val["name"], "test");
        assert_eq!(val["port"], 8080);
    }

    #[test]
    fn yml_extension_parses() {
        let f = write_temp_file("yml", "enabled: true\n");
        let val = load_config_file(f.path()).expect("should parse yml");
        assert_eq!(val["enabled"], true);
    }

    #[test]
    fn json_file_parses() {
        let f = write_temp_file("json", r#"{"name":"test","port":8080}"#);
        let val = load_config_file(f.path()).expect("should parse json");
        assert_eq!(val["name"], "test");
        assert_eq!(val["port"], 8080);
    }

    #[test]
    fn unsupported_extension_errors() {
        let f = write_temp_file("txt", "name: test\n");
        let err = load_config_file(f.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Use .yaml, .yml, or .json"), "got: {msg}");
    }

    #[test]
    fn invalid_yaml_errors() {
        let f = write_temp_file("yaml", ":\n  - :\n  invalid: [unterminated");
        let err = load_config_file(f.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Invalid YAML"), "got: {msg}");
    }

    #[test]
    fn invalid_json_errors() {
        let f = write_temp_file("json", "{not valid json}");
        let err = load_config_file(f.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Invalid JSON"), "got: {msg}");
    }

    #[test]
    fn nonexistent_file_errors() {
        let path = Path::new("/tmp/nonexistent-flowplane-test-file.yaml");
        let err = load_config_file(path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Failed to read file"), "got: {msg}");
    }

    #[test]
    fn strip_kind_removes_field() {
        let mut val = serde_json::json!({"kind": "Cluster", "name": "test"});
        strip_kind_field(&mut val);
        assert!(val.get("kind").is_none());
        assert_eq!(val["name"], "test");
    }

    #[test]
    fn strip_kind_noop_without_kind() {
        let mut val = serde_json::json!({"name": "test", "port": 80});
        strip_kind_field(&mut val);
        assert_eq!(val["name"], "test");
        assert_eq!(val["port"], 80);
    }

    #[test]
    fn strip_kind_noop_on_non_object() {
        let mut val = serde_json::json!([1, 2, 3]);
        strip_kind_field(&mut val);
        assert_eq!(val, serde_json::json!([1, 2, 3]));

        let mut null_val = serde_json::Value::Null;
        strip_kind_field(&mut null_val);
        assert!(null_val.is_null());
    }
}
