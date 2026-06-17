use crate::cli::config::{GlobalOptions, OutputFormat};
use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;
use std::fs;

#[derive(Debug)]
pub(crate) struct CliHttpError {
    status: StatusCode,
}

impl CliHttpError {
    pub(crate) fn exit_code(&self) -> i32 {
        exit_code_for_status(self.status)
    }
}

impl fmt::Display for CliHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request failed with status {}", self.status)
    }
}

impl std::error::Error for CliHttpError {}

pub(crate) fn render_error(
    global: &GlobalOptions,
    status: StatusCode,
    request_id: Option<String>,
    text: &str,
) -> anyhow::Error {
    let (envelope, code, message, hint, request_id) = error_envelope(status, request_id, text);
    if matches!(global.format(), OutputFormat::Json) {
        match serde_json::to_string_pretty(&envelope) {
            Ok(json) => eprintln!("{json}"),
            Err(_) => eprintln!("{}", envelope),
        }
    } else {
        eprintln!("error ({code}): {message}");
        if let Some(hint) = hint {
            eprintln!("  -> {hint}");
        }
        if let Some(rid) = request_id {
            eprintln!("  request id: {rid}");
        }
    }
    anyhow::Error::new(CliHttpError { status })
}

fn error_envelope(
    status: StatusCode,
    request_id: Option<String>,
    text: &str,
) -> (Value, String, String, Option<String>, Option<String>) {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| status.as_str())
        .to_string();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(text)
        .to_string();
    let hint = parsed
        .as_ref()
        .and_then(|v| v.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let request_id = request_id.or_else(|| {
        parsed
            .as_ref()
            .and_then(|v| v.get("request_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });

    let mut envelope = parsed.unwrap_or_else(|| {
        serde_json::json!({
            "code": code,
            "message": message,
        })
    });
    if let Value::Object(obj) = &mut envelope {
        obj.entry("code").or_insert(Value::String(code.clone()));
        obj.entry("message")
            .or_insert(Value::String(message.clone()));
        obj.entry("status")
            .or_insert(Value::Number(status.as_u16().into()));
        if let Some(rid) = &request_id {
            obj.entry("request_id")
                .or_insert(Value::String(rid.clone()));
        }
    }
    (envelope, code, message, hint, request_id)
}

pub(crate) fn exit_code_for_status(status: StatusCode) -> i32 {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => 2,
        StatusCode::NOT_FOUND | StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => 3,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => 4,
        StatusCode::TOO_MANY_REQUESTS => 5,
        s if s.is_server_error() => 6,
        _ => 1,
    }
}

pub(crate) fn render(global: &GlobalOptions, value: &Value) -> Result<()> {
    let text = match global.format() {
        OutputFormat::Json => serde_json::to_string_pretty(value)?,
        OutputFormat::Yaml => yaml_like(value, 0),
        OutputFormat::Table | OutputFormat::Wide => table(value),
    };
    if let Some(out) = &global.out {
        fs::write(out, text).with_context(|| format!("write {}", out.display()))?;
    } else {
        println!("{text}");
    }
    Ok(())
}

pub(crate) fn table(value: &Value) -> String {
    if let Some(flattened) = flatten_xds_status(value) {
        return table(&flattened);
    }
    if let Some(flattened) = flatten_ops_trace(value) {
        return table(&flattened);
    }
    if let Some(flattened) = flatten_expose(value) {
        return table(&flattened);
    }
    if let Some(flattened) = flatten_status_row(value) {
        return table(&flattened);
    }
    let rows = if let Some(items) = value.get("items").and_then(Value::as_array) {
        items.clone()
    } else if let Some(items) = value.as_array() {
        items.clone()
    } else {
        vec![value.clone()]
    };
    if rows.is_empty() {
        return "no rows".into();
    }
    let mut columns = BTreeSet::new();
    for row in &rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "spec" | "certificate_pem" | "private_key_pem" | "ca_certificate_pem"
                ) {
                    columns.insert(key.clone());
                }
            }
        }
    }
    let columns = ordered_columns(columns);
    if columns.is_empty() {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    }
    let matrix = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| cell(row.get(c).unwrap_or(&Value::Null)))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let headers = columns
        .iter()
        .map(|c| c.replace('_', " ").to_ascii_uppercase())
        .collect::<Vec<_>>();
    let widths = (0..columns.len())
        .map(|i| {
            std::iter::once(headers[i].len())
                .chain(matrix.iter().map(|row| row[i].len()))
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut out = format_row(&headers, &widths);
    for row in matrix {
        out.push('\n');
        out.push_str(&format_row(&row, &widths));
    }
    out
}

fn ordered_columns(columns: BTreeSet<String>) -> Vec<String> {
    let preferred = [
        "health",
        "name",
        "id",
        "display_name",
        "description",
        "upstream",
        "path",
        "port",
        "curl_url",
        "cluster_name",
        "route_config_name",
        "listener_name",
        "role",
        "email",
        "resource",
        "action",
        "revision",
        "latest_spec_version",
        "latest_spec_source",
        "latest_spec_hash",
        "route_binding_count",
        "tool_count",
        "live_dataplanes",
        "stale_dataplanes",
        "total_requests",
        "total_errors",
        "warming_failures",
        "source",
        "event_type",
        "outcome",
        "surface",
        "request_id",
        "recent_nack_count",
        "config_verified_dataplanes",
        "last_heartbeat_at",
        "occurred_at",
        "created_at",
        "updated_at",
    ];
    let mut ordered = Vec::new();
    for key in preferred {
        if columns.contains(key) {
            ordered.push(key.to_string());
        }
    }
    for key in columns {
        if !ordered.contains(&key) {
            ordered.push(key);
        }
    }
    ordered
}

fn flatten_expose(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    if !(obj.contains_key("cluster")
        && obj.contains_key("route_config")
        && obj.contains_key("listener"))
    {
        return None;
    }
    let mut row = serde_json::Map::new();
    for key in [
        "name",
        "upstream",
        "path",
        "port",
        "curl_url",
        "endpoint_source",
    ] {
        if let Some(value) = obj.get(key) {
            row.insert(key.to_string(), value.clone());
        }
    }
    if let Some(name) = obj
        .get("cluster")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
    {
        row.insert("cluster_name".into(), Value::String(name.into()));
    }
    if let Some(name) = obj
        .get("route_config")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
    {
        row.insert("route_config_name".into(), Value::String(name.into()));
    }
    if let Some(name) = obj
        .get("listener")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
    {
        row.insert("listener_name".into(), Value::String(name.into()));
    }
    Some(Value::Array(vec![Value::Object(row)]))
}

fn flatten_status_row(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    let api = obj.get("api")?.as_object()?;
    if !(obj.contains_key("latest_spec")
        && obj.contains_key("route_binding_count")
        && obj.contains_key("tool_count"))
    {
        return None;
    }

    let mut row = serde_json::Map::new();
    for key in [
        "name",
        "id",
        "display_name",
        "description",
        "revision",
        "created_at",
        "updated_at",
    ] {
        if let Some(value) = api.get(key) {
            row.insert(key.to_string(), value.clone());
        }
    }
    if let Some(spec) = obj.get("latest_spec").and_then(Value::as_object) {
        if let Some(version) = spec.get("version") {
            row.insert("latest_spec_version".into(), version.clone());
        }
        if let Some(source) = spec.get("source_kind") {
            row.insert("latest_spec_source".into(), source.clone());
        }
        if let Some(hash) = spec.get("spec_hash") {
            row.insert("latest_spec_hash".into(), short_hash(hash));
        }
    } else {
        row.insert("latest_spec_version".into(), Value::Null);
    }
    if let Some(count) = obj.get("route_binding_count") {
        row.insert("route_binding_count".into(), count.clone());
    }
    if let Some(count) = obj.get("tool_count") {
        row.insert("tool_count".into(), count.clone());
    }
    Some(Value::Object(row))
}

fn flatten_xds_status(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    if !(obj.contains_key("health")
        && obj.contains_key("total_dataplanes")
        && obj.contains_key("dataplanes"))
    {
        return None;
    }
    let mut row = serde_json::Map::new();
    for key in [
        "health",
        "total_dataplanes",
        "live_dataplanes",
        "stale_dataplanes",
        "config_verified_dataplanes",
        "recent_nack_count",
        "total_requests",
        "total_errors",
        "warming_failures",
    ] {
        if let Some(value) = obj.get(key) {
            row.insert(key.to_string(), value.clone());
        }
    }
    if let Some(latest) = obj.get("latest_nack").and_then(Value::as_object) {
        if let Some(created_at) = latest.get("created_at") {
            row.insert("latest_nack_at".into(), created_at.clone());
        }
        if let Some(node_id) = latest.get("node_id") {
            row.insert("latest_nack_node".into(), node_id.clone());
        }
        if let Some(type_url) = latest.get("type_url") {
            row.insert("latest_nack_type".into(), type_url.clone());
        }
    }
    Some(Value::Array(vec![Value::Object(row)]))
}

fn flatten_ops_trace(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    let audit = obj.get("audit")?.as_array()?;
    let events = obj.get("events")?.as_array()?;
    let mut rows = Vec::with_capacity(audit.len() + events.len());
    for item in audit {
        let Some(item) = item.as_object() else {
            continue;
        };
        let mut row = serde_json::Map::new();
        row.insert("source".into(), Value::String("audit".into()));
        for key in [
            "occurred_at",
            "request_id",
            "surface",
            "action",
            "resource",
            "outcome",
            "actor_label",
        ] {
            if let Some(value) = item.get(key) {
                row.insert(key.to_string(), value.clone());
            }
        }
        rows.push(Value::Object(row));
    }
    for item in events {
        let Some(item) = item.as_object() else {
            continue;
        };
        let mut row = serde_json::Map::new();
        row.insert("source".into(), Value::String("outbox".into()));
        for key in ["occurred_at", "event_type", "seq"] {
            if let Some(value) = item.get(key) {
                row.insert(key.to_string(), value.clone());
            }
        }
        rows.push(Value::Object(row));
    }
    Some(Value::Array(rows))
}

fn short_hash(value: &Value) -> Value {
    value
        .as_str()
        .map(|s| s.chars().take(12).collect::<String>())
        .map(Value::String)
        .unwrap_or_else(|| value.clone())
}

pub(crate) fn format_row(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(i, cell)| format!("{cell:<width$}", width = widths[i]))
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_string()
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        Value::Array(v) => format!("{} items", v.len()),
        Value::Object(_) => "{...}".into(),
    }
}

fn yaml_like(value: &Value, indent: usize) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let pad = " ".repeat(indent);
                match v {
                    Value::Object(_) | Value::Array(_) => {
                        format!("{pad}{k}:\n{}", yaml_like(v, indent + 2))
                    }
                    _ => format!("{pad}{k}: {}", cell(v)),
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                format!(
                    "{}- {}",
                    " ".repeat(indent),
                    yaml_like(v, indent + 2).trim_start()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => cell(value),
    }
}

pub(crate) fn print_mutation_summary(
    global: &GlobalOptions,
    method: &str,
    path: &str,
    value: Option<&Value>,
) -> Result<()> {
    if global.quiet {
        return Ok(());
    }
    if global.dry_run {
        println!("plan: would {} {}", method.to_ascii_lowercase(), path);
        return Ok(());
    }
    let verb = match method {
        "POST" => "created",
        "PATCH" => "updated",
        "DELETE" => "deleted",
        _ => "ok",
    };
    let label = value
        .and_then(resource_label)
        .unwrap_or_else(|| path.trim_start_matches('/').to_string());
    let revision = value
        .and_then(|v| v.get("revision"))
        .and_then(Value::as_i64)
        .map(|r| format!(" (revision {r})"))
        .unwrap_or_default();
    println!("{verb} {label}{revision}");
    Ok(())
}

fn resource_label(value: &Value) -> Option<String> {
    if let Some(cert) = value.get("certificate") {
        return resource_label(cert);
    }
    value
        .get("name")
        .and_then(Value::as_str)
        .map(|name| format!("\"{name}\""))
        .or_else(|| {
            value
                .get("serial_number")
                .and_then(Value::as_str)
                .map(|serial| format!("certificate \"{serial}\""))
        })
        .or_else(|| {
            value
                .get("id")
                .and_then(Value::as_str)
                .map(|id| format!("resource {id}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_statuses_map_to_scriptable_exit_codes() {
        assert_eq!(exit_code_for_status(StatusCode::UNAUTHORIZED), 2);
        assert_eq!(exit_code_for_status(StatusCode::FORBIDDEN), 2);
        assert_eq!(exit_code_for_status(StatusCode::NOT_FOUND), 3);
        assert_eq!(exit_code_for_status(StatusCode::CONFLICT), 3);
        assert_eq!(exit_code_for_status(StatusCode::BAD_REQUEST), 4);
        assert_eq!(exit_code_for_status(StatusCode::TOO_MANY_REQUESTS), 5);
        assert_eq!(exit_code_for_status(StatusCode::INTERNAL_SERVER_ERROR), 6);
    }

    #[test]
    fn error_envelope_preserves_server_json_and_request_id_header() {
        let (value, code, message, hint, request_id) = error_envelope(
            StatusCode::NOT_FOUND,
            Some("req-1".into()),
            r#"{"code":"not_found","message":"missing","hint":"check name"}"#,
        );

        assert_eq!(code, "not_found");
        assert_eq!(message, "missing");
        assert_eq!(hint.as_deref(), Some("check name"));
        assert_eq!(request_id.as_deref(), Some("req-1"));
        assert_eq!(value["code"], "not_found");
        assert_eq!(value["message"], "missing");
        assert_eq!(value["status"], 404);
        assert_eq!(value["request_id"], "req-1");
    }

    #[test]
    fn error_envelope_wraps_plain_text_failures() {
        let (value, code, message, hint, request_id) =
            error_envelope(StatusCode::BAD_GATEWAY, None, "upstream unavailable");

        assert_eq!(code, "502");
        assert_eq!(message, "upstream unavailable");
        assert_eq!(hint, None);
        assert_eq!(request_id, None);
        assert_eq!(value["code"], "502");
        assert_eq!(value["message"], "upstream unavailable");
        assert_eq!(value["status"], 502);
    }
}
