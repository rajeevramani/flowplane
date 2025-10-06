//! Shared output formatting utilities for CLI commands
//!
//! Provides consistent output formatting across all CLI commands with support
//! for JSON, YAML, and table formats.

use anyhow::{Context, Result};
use serde::Serialize;

/// Output format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Yaml,
    Table,
}

impl OutputFormat {
    /// Parse output format from string
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "yaml" => Ok(OutputFormat::Yaml),
            "table" => Ok(OutputFormat::Table),
            _ => anyhow::bail!(
                "Unsupported output format: '{}'. Use 'json', 'yaml', or 'table'.",
                s
            ),
        }
    }
}

/// Print data in the specified format
pub fn print_output<T: Serialize>(data: &T, format: &str) -> Result<()> {
    let format = OutputFormat::from_str(format)?;
    print_output_format(data, format)
}

/// Print data in the specified OutputFormat
pub fn print_output_format<T: Serialize>(data: &T, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(data),
        OutputFormat::Yaml => print_yaml(data),
        OutputFormat::Table => {
            anyhow::bail!("Table format requires custom implementation per data type")
        }
    }
}

/// Print data as JSON
pub fn print_json<T: Serialize>(data: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(data).context("Failed to serialize to JSON")?;
    println!("{}", json);
    Ok(())
}

/// Print data as YAML
pub fn print_yaml<T: Serialize>(data: &T) -> Result<()> {
    let yaml = serde_yaml::to_string(data).context("Failed to serialize to YAML")?;
    println!("{}", yaml);
    Ok(())
}

/// Truncate string to maximum length with ellipsis
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Print a horizontal separator line
pub fn print_separator(width: usize) {
    println!("{}", "-".repeat(width));
}

/// Print a table header
pub fn print_table_header(columns: &[(&str, usize)]) {
    println!();
    let mut header = String::new();
    for (name, width) in columns {
        header.push_str(&format!("{:<width$} ", name, width = width));
    }
    println!("{}", header.trim());

    let total_width: usize = columns.iter().map(|(_, w)| w + 1).sum();
    print_separator(total_width.saturating_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(OutputFormat::from_str("json").unwrap(), OutputFormat::Json);
        assert_eq!(OutputFormat::from_str("JSON").unwrap(), OutputFormat::Json);
        assert_eq!(OutputFormat::from_str("yaml").unwrap(), OutputFormat::Yaml);
        assert_eq!(OutputFormat::from_str("YAML").unwrap(), OutputFormat::Yaml);
        assert_eq!(OutputFormat::from_str("table").unwrap(), OutputFormat::Table);
        assert!(OutputFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_print_json() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };
        assert!(print_json(&data).is_ok());
    }

    #[test]
    fn test_print_yaml() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };
        assert!(print_yaml(&data).is_ok());
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(truncate("hello", 3), "...");
    }
}
