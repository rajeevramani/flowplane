use crate::cli::config::{GlobalOptions, OutputFormat};
use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;
use std::fs;

/// A CLI error whose structured envelope has already been rendered to stderr (CLI-R-30).
///
/// It carries only the resolved process exit code (CLI-R-31); `main` downcasts to this type
/// and exits with `exit_code()` without re-printing, so no raw `{err:?}` trailer leaks.
#[derive(Debug)]
pub(crate) struct CliError {
    exit_code: i32,
}

impl CliError {
    pub(crate) fn new(exit_code: i32) -> Self {
        Self { exit_code }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        self.exit_code
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "command failed (exit {})", self.exit_code)
    }
}

impl std::error::Error for CliError {}

pub(crate) fn render_error(
    global: &GlobalOptions,
    status: StatusCode,
    request_id: Option<String>,
    text: &str,
) -> anyhow::Error {
    let env = error_envelope(status, request_id, text);
    emit_error_envelope(global, &env);
    anyhow::Error::new(CliError::new(exit_code_for_status(status)))
}

/// Render a revision conflict (CLI-R-47): the same structured error envelope as any HTTP
/// failure, enriched so it names BOTH the revision the client attempted (`attempted_revision`)
/// and the server's current one (carried in the server `message`), plus a recovery hint.
pub(crate) fn render_revision_conflict(
    global: &GlobalOptions,
    status: StatusCode,
    request_id: Option<String>,
    text: &str,
    attempted: i64,
) -> anyhow::Error {
    let env = revision_conflict_envelope(status, request_id, text, attempted);
    emit_error_envelope(global, &env);
    anyhow::Error::new(CliError::new(exit_code_for_status(status)))
}

/// Pure builder for the revision-conflict envelope (CLI-R-47): the standard error envelope
/// plus `attempted_revision` and a recovery hint. The server's current revision rides along
/// in the existing `message`/body.
fn revision_conflict_envelope(
    status: StatusCode,
    request_id: Option<String>,
    text: &str,
    attempted: i64,
) -> Value {
    let mut env = error_envelope(status, request_id, text);
    if let Value::Object(obj) = &mut env {
        obj.insert("attempted_revision".into(), Value::Number(attempted.into()));
        obj.insert(
            "hint".into(),
            Value::String(format!(
                "stale revision: you sent revision {attempted}, but the resource has since \
                 changed (see message for the current revision); re-read it, or omit \
                 --revision to let the CLI read-modify-write"
            )),
        );
    }
    env
}

/// Render a transport/network failure (connection refused, DNS, TLS, timeout) through the
/// same structured envelope as HTTP errors (CLI-R-30): no raw `eprintln!("{err:?}")`.
/// Transport failures are always `retryable: true` and exit `7` (CLI-R-31/32).
pub(crate) fn render_transport_error(
    global: &GlobalOptions,
    err: &reqwest::Error,
) -> anyhow::Error {
    let code = if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connection_failed"
    } else {
        "transport_error"
    };
    let env = serde_json::json!({
        "code": code,
        "message": err.to_string(),
        "retryable": true,
    });
    emit_error_envelope(global, &env);
    anyhow::Error::new(CliError::new(7))
}

/// Print an already-built error envelope to **stderr** (CLI-R-30): JSON under `-o json`,
/// YAML under `-o yaml`, otherwise a compact prose form. stdout is left untouched.
pub(crate) fn emit_error_envelope(global: &GlobalOptions, env: &Value) {
    match global.format() {
        OutputFormat::Json => match serde_json::to_string_pretty(env) {
            Ok(json) => eprintln!("{json}"),
            Err(_) => eprintln!("{env}"),
        },
        OutputFormat::Yaml => eprintln!("{}", yaml_like(env, 0)),
        OutputFormat::Table | OutputFormat::Wide => {
            let field = |key: &str| env.get(key).and_then(Value::as_str);
            let code = field("code").unwrap_or("error");
            let message = field("message").unwrap_or("request failed");
            eprintln!("error ({code}): {message}");
            if let Some(hint) = field("hint") {
                eprintln!("  -> {hint}");
            }
            if let Some(rid) = field("request_id") {
                eprintln!("  request id: {rid}");
            }
        }
    }
}

fn error_envelope(status: StatusCode, request_id: Option<String>, text: &str) -> Value {
    // Only a JSON *object* error body is adopted as the envelope base; a string/array/number
    // (or non-JSON text) body falls back to a freshly-built object so the contract shape
    // (CLI-R-30) holds for every HTTP error regardless of what the server/proxy returned.
    let parsed = serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object);
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
    // CLI-R-33: synthesize the login hint on 401 when the server omits one; 403 keeps the
    // server-supplied hint (which already names the (resource, action) via deny_to_error).
    let hint = parsed
        .as_ref()
        .and_then(|v| v.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            (status == StatusCode::UNAUTHORIZED)
                .then(|| "run `flowplane auth login` to authenticate".to_string())
        });
    let request_id = request_id.or_else(|| {
        parsed
            .as_ref()
            .and_then(|v| v.get("request_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });

    let mut envelope = parsed.unwrap_or_else(|| serde_json::json!({}));
    if let Value::Object(obj) = &mut envelope {
        obj.entry("code").or_insert(Value::String(code.clone()));
        obj.entry("message")
            .or_insert(Value::String(message.clone()));
        obj.entry("status")
            .or_insert(Value::Number(status.as_u16().into()));
        // CLI-R-32: classify transient (429/5xx) vs terminal failures.
        obj.insert("retryable".into(), Value::Bool(is_retryable(status)));
        if let Some(hint) = &hint {
            obj.entry("hint").or_insert(Value::String(hint.clone()));
        }
        if let Some(rid) = &request_id {
            obj.entry("request_id")
                .or_insert(Value::String(rid.clone()));
        }
    }
    envelope
}

/// The human message for a non-success HTTP response (object `message`, else the body text,
/// else the status), for callers that aggregate failures instead of rendering each one.
pub(crate) fn error_message(status: StatusCode, text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| {
            if text.trim().is_empty() {
                status.to_string()
            } else {
                text.to_string()
            }
        })
}

/// Build the aggregate error envelope for an `apply` partial/total failure (CLI-R-30): one
/// structured document on stderr listing each failed resource, instead of per-subrequest
/// prints or a raw `{err:?}` trailer.
pub(crate) fn apply_error_envelope(failures: Vec<Value>) -> Value {
    serde_json::json!({
        "code": "apply_failed",
        "message": format!("{} resource(s) failed to apply", failures.len()),
        "retryable": false,
        "failures": failures,
    })
}

/// Whether a failed HTTP status is worth retrying (CLI-R-32): rate-limit and server errors
/// are transient; 4xx terminal. Transport failures are handled in `render_transport_error`.
pub(crate) fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Map an HTTP status to the CLI's scriptable exit code (CLI-R-31): `3` auth (401/403),
/// `4` not-found/conflict/precondition (404/409/412), `5` validation (400/422), `6`
/// rate-limited (429), `7` server (5xx), `1` otherwise. Usage errors (`2`) are clap-native;
/// transport failures (`7`) are assigned in `render_transport_error`.
pub(crate) fn exit_code_for_status(status: StatusCode) -> i32 {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => 3,
        StatusCode::NOT_FOUND | StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => 4,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => 5,
        StatusCode::TOO_MANY_REQUESTS => 6,
        s if s.is_server_error() => 7,
        _ => 1,
    }
}

/// JSON output-contract version (CLI-R-15). Integer, independent of the product semver;
/// bumped only on a breaking change to the `-o json`/`-o yaml` envelope shape.
pub(crate) const SCHEMA_VERSION: u64 = 1;

/// Wrap a payload in the Option-A typed envelope `{ schemaVersion, kind, data }` (CLI-R-15).
pub(crate) fn envelope(kind: &str, data: &Value) -> Value {
    serde_json::json!({
        "schemaVersion": SCHEMA_VERSION,
        "kind": kind,
        "data": data,
    })
}

/// Project a payload to only the requested field names (CLI-R-51). Applied to the resource
/// data: a list (array, or an object wrapping an `items` array) projects every item; a
/// single object keeps only the requested keys; scalars are returned unchanged.
pub(crate) fn project_fields(value: &Value, fields: &[String]) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|v| project_one(v, fields)).collect()),
        Value::Object(map) if map.get("items").map(Value::is_array).unwrap_or(false) => {
            let mut out = map.clone();
            if let Some(Value::Array(items)) = out.get_mut("items") {
                *items = items.iter().map(|v| project_one(v, fields)).collect();
            }
            Value::Object(out)
        }
        Value::Object(_) => project_one(value, fields),
        other => other.clone(),
    }
}

fn project_one(value: &Value, fields: &[String]) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for key in fields {
                if let Some(v) = map.get(key) {
                    out.insert(key.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Authoritative envelope `kind` for a response (CLI-R-15 / invariant 13). This is the single
/// source of truth the renderer uses — NOT free URL inference. Non-collection / action
/// endpoints (whose path tail is not a `{collection}/{id}` pair) get an explicit kind via
/// [`kind_override`]; every true REST collection resolves through [`derive_kind`] (corrected
/// singularization). The kind-parity test (`cli_s7_coverage.rs`) pins every envelope-emitting
/// command against this resolver so a wrong/truncated/id-shaped kind fails CI.
pub(crate) fn resolve_kind(path: &str, value: &Value) -> String {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    if let Some(kind) = kind_override(clean) {
        let is_list = value.is_array() || value.get("items").map(Value::is_array).unwrap_or(false);
        return if is_list {
            format!("{kind}List")
        } else {
            kind.to_string()
        };
    }
    derive_kind(clean, value)
}

/// Authoritative kind for endpoints whose path tail is an *action* (a mutation sub-route) or a
/// *singleton view* rather than a `{collection}/{id}` pair — where singularizing the trailing
/// path segment yields a wrong/truncated kind (`mcp/status` → `statu`) or the resource name
/// (`expose/{name}` → `local`). Ordered; first match wins. Returns the singular base kind (the
/// caller appends `List` for list payloads). Everything not matched here is a true REST
/// collection resolved by [`derive_kind`]; the parity test forbids a wrong kind for any endpoint.
fn kind_override(path: &str) -> Option<&'static str> {
    // Mutation actions: a body-returning DELETE/POST/PATCH sub-route (or expose/unexpose) is a
    // mutation, not a resource read — it carries the stable `mutationResult` kind (matches the
    // body-less mutation path in client.rs).
    const ACTION_TAILS: &[&str] = &[
        "/expose",
        "/rotate",
        "/stop",
        "/disable",
        "/enable",
        "/revoke",
        "/reject",
        "/publish",
        "/apply",
        "/force-repush",
        "/rotate-token",
    ];
    if path.contains("/expose/") || ACTION_TAILS.iter().any(|t| path.ends_with(t)) {
        return Some("mutationResult");
    }
    // Singleton / aggregate GET views (would mis-singularize to `statu`, `stat`, `op`, or the
    // governing collection).
    if path.ends_with("/mcp/status") {
        return Some("mcpStatus");
    }
    if path.ends_with("/xds/status") {
        return Some("xdsStatus");
    }
    if path.ends_with("/stats/overview") {
        return Some("statsOverview");
    }
    if path.ends_with("/ops/trace") {
        return Some("trace");
    }
    if path.ends_with("/ai/trace") {
        return Some("aiTrace");
    }
    if path.ends_with("/ai/retention") {
        return Some("aiRetention");
    }
    if path.ends_with("/envoy-config") {
        return Some("envoyConfig");
    }
    if path.ends_with("/telemetry") {
        return Some("dataplaneTelemetry");
    }
    if path.ends_with("/override") {
        return Some("rateLimitOverride");
    }
    // `…/api-definitions/{name}/status` only (mcp/xds status tails handled above); scoped so a
    // future singleton `…/status` endpoint cannot silently inherit this kind.
    if path.contains("/api-definitions/") && path.ends_with("/status") {
        return Some("apiDefinitionStatus");
    }
    None
}

/// Derive a stable envelope `kind` from the REST path and response shape (CLI-R-15).
/// Singular for an object (`cluster`), `…List` for a collection (`clusterList`).
///
/// The governing collection is chosen from an **explicit known-collection set** (not by guessing
/// which path segment "looks plural") — so an item read resolves to its resource (`…/clusters/x`
/// → `cluster`) even when the id itself looks plural (`…/clusters/aliases` is still `cluster`,
/// not `aliase`). `?query` is stripped first; lists get the `List` suffix. An unmapped path falls
/// back to the trailing segment — the parity test forbids that for known endpoints.
pub(crate) fn derive_kind(path: &str, value: &Value) -> String {
    // Explicit REST collection segments the CLI addresses. The kind is the singular of the
    // nearest-to-end entry found here, so dynamic ids (which are never in this set) are skipped.
    const COLLECTIONS: &[&str] = &[
        "clusters",
        "listeners",
        "route-configs",
        "route-generation-plans",
        "secrets",
        "api-definitions",
        "specs",
        "spec-versions",
        "spec-version",
        "providers",
        "routes",
        "budgets",
        "rate-limit-domains",
        "policies",
        "connections",
        "tools",
        "dataplanes",
        "proxy-certificates",
        "learning-sessions",
        "learning-discovery-sessions",
        "orgs",
        "teams",
        "members",
        "grants",
        "agents",
        "nacks",
    ];
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .skip_while(|s| matches!(*s, "api" | "v1"))
        .collect();
    // Drop a leading `teams/{team}` or `orgs/{org}` scoping pair when a resource follows it,
    // so the (plural) scope segment is never mistaken for the resource noun.
    if segments.len() > 2 && matches!(segments[0], "teams" | "orgs") {
        segments.drain(0..2);
    }
    let is_list = value.is_array() || value.get("items").map(Value::is_array).unwrap_or(false);
    // Nearest-to-end KNOWN collection; else the trailing segment (forbidden for known endpoints
    // by the parity test).
    let noun = segments
        .iter()
        .rev()
        .find(|s| COLLECTIONS.contains(s))
        .or_else(|| segments.last())
        .copied()
        .unwrap_or("result");
    let base = singularize(noun);
    if is_list {
        format!("{base}List")
    } else {
        base
    }
}

/// Convert a plural resource segment to a camelCase singular kind (`route-configs` →
/// `routeConfig`, `policies` → `policy`). Non-plural words ending in `s` (`status`, `ss`,
/// `us`) are left intact so `status` does not become `statu` (Obs-2 / fpv2-86m.1).
fn singularize(segment: &str) -> String {
    let camel = to_lower_camel(segment);
    if let Some(stem) = camel.strip_suffix("ies") {
        format!("{stem}y")
    } else if camel.ends_with("ss") || camel.ends_with("us") {
        // `status`, `bonus`, `…ss` collections are not plurals — never truncate them.
        camel
    } else if let Some(stem) = camel.strip_suffix('s') {
        stem.to_string()
    } else {
        camel
    }
}

fn to_lower_camel(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut upper_next = false;
    for ch in segment.chars() {
        if ch == '-' || ch == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

pub(crate) fn render(global: &GlobalOptions, kind: &str, value: &Value) -> Result<()> {
    // CLI-R-51: `--fields` projects inside `data` (per item for lists); the envelope
    // metadata (schemaVersion/kind) is added afterwards and always survives.
    let projected;
    let value = if global.fields.is_empty() {
        value
    } else {
        projected = project_fields(value, &global.fields);
        &projected
    };
    let text = match global.format() {
        OutputFormat::Json => serde_json::to_string_pretty(&envelope(kind, value))?,
        OutputFormat::Yaml => yaml_like(&envelope(kind, value), 0),
        OutputFormat::Table | OutputFormat::Wide => table_styled(value, global.use_color()),
    };
    if let Some(out) = &global.out {
        fs::write(out, text).with_context(|| format!("write {}", out.display()))?;
    } else {
        println!("{text}");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn table(value: &Value) -> String {
    table_styled(value, false)
}

pub(crate) fn table_styled(value: &Value, color: bool) -> String {
    if let Some(flattened) = flatten_xds_status(value) {
        return table_styled(&flattened, color);
    }
    if let Some(flattened) = flatten_ops_trace(value) {
        return table_styled(&flattened, color);
    }
    if let Some(flattened) = flatten_expose(value) {
        return table_styled(&flattened, color);
    }
    if let Some(flattened) = flatten_status_row(value) {
        return table_styled(&flattened, color);
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
    let header_row = format_row(&headers, &widths);
    let mut out = if color {
        format!("\x1b[1m{header_row}\x1b[0m")
    } else {
        header_row
    };
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
        "transport",
        "preferred_protocol_version",
        "active_sessions",
        "static_tool_count",
        "dynamic_enabled_tool_count",
        "api_invocation_mode",
        "connection_id",
        "principal_kind",
        "sse",
        "age_seconds",
        "idle_seconds",
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn http_statuses_map_to_scriptable_exit_codes() {
        // CLI-R-31 0–7 table: auth=3, not-found/conflict/precondition=4, validation=5,
        // rate-limited=6, server=7, generic=1.
        assert_eq!(exit_code_for_status(StatusCode::UNAUTHORIZED), 3);
        assert_eq!(exit_code_for_status(StatusCode::FORBIDDEN), 3);
        assert_eq!(exit_code_for_status(StatusCode::NOT_FOUND), 4);
        assert_eq!(exit_code_for_status(StatusCode::CONFLICT), 4);
        assert_eq!(exit_code_for_status(StatusCode::PRECONDITION_FAILED), 4);
        assert_eq!(exit_code_for_status(StatusCode::BAD_REQUEST), 5);
        assert_eq!(exit_code_for_status(StatusCode::UNPROCESSABLE_ENTITY), 5);
        assert_eq!(exit_code_for_status(StatusCode::TOO_MANY_REQUESTS), 6);
        assert_eq!(exit_code_for_status(StatusCode::INTERNAL_SERVER_ERROR), 7);
        assert_eq!(exit_code_for_status(StatusCode::BAD_GATEWAY), 7);
        assert_eq!(exit_code_for_status(StatusCode::IM_A_TEAPOT), 1);
    }

    #[test]
    fn retryable_classifies_transient_vs_terminal() {
        assert!(is_retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!is_retryable(StatusCode::NOT_FOUND));
        assert!(!is_retryable(StatusCode::BAD_REQUEST));
        assert!(!is_retryable(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn error_envelope_preserves_server_json_and_request_id_header() {
        let value = error_envelope(
            StatusCode::NOT_FOUND,
            Some("req-1".into()),
            r#"{"code":"not_found","message":"missing","hint":"check name"}"#,
        );

        assert_eq!(value["code"], "not_found");
        assert_eq!(value["message"], "missing");
        assert_eq!(value["hint"], "check name");
        assert_eq!(value["status"], 404);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["retryable"], false);
    }

    #[test]
    fn error_envelope_wraps_plain_text_failures_as_retryable_server_error() {
        let value = error_envelope(StatusCode::BAD_GATEWAY, None, "upstream unavailable");

        assert_eq!(value["code"], "502");
        assert_eq!(value["message"], "upstream unavailable");
        assert_eq!(value["status"], 502);
        assert_eq!(value["retryable"], true);
        assert!(value.get("hint").is_none());
        assert!(value.get("request_id").is_none());
    }

    #[test]
    fn error_envelope_wraps_non_object_json_body_in_a_fresh_object() {
        // A server/proxy returning a JSON array/string/number must still yield the contract
        // object shape, not the raw value (CLI-R-30).
        for body in [r#"["boom"]"#, r#""boom""#, "42", "true"] {
            let value = error_envelope(StatusCode::BAD_GATEWAY, None, body);
            assert!(
                value.is_object(),
                "body {body:?} must produce an object envelope"
            );
            assert_eq!(value["status"], 502);
            assert_eq!(value["retryable"], true);
            assert!(value.get("code").is_some());
            assert!(value.get("message").is_some());
        }
    }

    #[test]
    fn revision_conflict_envelope_names_both_revisions() {
        // Server 409 body carries the current revision in its message; the CLI adds the
        // attempted revision + a recovery hint (CLI-R-47).
        let env = revision_conflict_envelope(
            StatusCode::CONFLICT,
            Some("req-9".into()),
            r#"{"code":"conflict","message":"revision mismatch: current revision is 5"}"#,
            3,
        );
        assert_eq!(env["status"], 409);
        assert_eq!(env["attempted_revision"], 3);
        assert!(
            env["message"]
                .as_str()
                .unwrap()
                .contains("current revision is 5"),
            "server's current revision must survive in the message: {env}"
        );
        assert!(
            env["hint"].as_str().unwrap().contains("revision 3"),
            "hint must name the attempted revision: {env}"
        );
        // 409 maps to exit 4 (not-found/conflict/precondition).
        assert_eq!(exit_code_for_status(StatusCode::CONFLICT), 4);
    }

    #[test]
    fn error_envelope_synthesizes_login_hint_on_401_without_server_hint() {
        let value = error_envelope(StatusCode::UNAUTHORIZED, None, "unauthorized");
        assert_eq!(value["status"], 401);
        assert_eq!(value["retryable"], false);
        assert_eq!(value["hint"], "run `flowplane auth login` to authenticate");

        // A server-supplied hint is preserved, not overwritten.
        let value = error_envelope(
            StatusCode::UNAUTHORIZED,
            None,
            r#"{"message":"nope","hint":"use a fresh token"}"#,
        );
        assert_eq!(value["hint"], "use a fresh token");
    }

    #[test]
    fn envelope_wraps_payload_with_integer_schema_version_and_kind() {
        let data = serde_json::json!({ "name": "demo", "revision": 3 });
        let env = envelope("cluster", &data);
        assert_eq!(env["schemaVersion"], Value::Number(SCHEMA_VERSION.into()));
        assert!(
            env["schemaVersion"].is_u64(),
            "schemaVersion must be an integer"
        );
        assert_eq!(env["kind"], "cluster");
        assert_eq!(env["data"], data);
        // Exactly the three contract keys, nothing leaked at the top level.
        let keys: BTreeSet<&str> = env
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(["schemaVersion", "kind", "data"]));
    }

    #[test]
    fn derive_kind_singular_for_object_list_for_collection() {
        let obj = serde_json::json!({ "name": "c1" });
        let arr = serde_json::json!([{ "name": "c1" }]);
        let items = serde_json::json!({ "items": [{ "name": "c1" }] });

        // item read: trailing id ignored, collection precedes it.
        assert_eq!(
            derive_kind("/api/v1/teams/payments/clusters/c1", &obj),
            "cluster"
        );
        // collection list / create.
        assert_eq!(
            derive_kind("/api/v1/teams/payments/clusters", &arr),
            "clusterList"
        );
        assert_eq!(
            derive_kind("/api/v1/teams/payments/clusters", &items),
            "clusterList"
        );
        assert_eq!(
            derive_kind("/api/v1/teams/payments/clusters", &obj),
            "cluster"
        );
        // top-level collections.
        assert_eq!(derive_kind("/api/v1/orgs/acme/teams", &arr), "teamList");
        assert_eq!(derive_kind("/api/v1/orgs", &arr), "orgList");
        assert_eq!(derive_kind("/api/v1/orgs/acme", &obj), "org");
        // hyphenated/`ies` plurals singularize to camelCase.
        assert_eq!(
            derive_kind("/api/v1/teams/p/route-configs/r1", &obj),
            "routeConfig"
        );
        assert_eq!(
            derive_kind("/api/v1/teams/p/rate-limit/d/policies", &arr),
            "policyList"
        );
        // query strings are stripped, not leaked into the kind.
        assert_eq!(
            derive_kind("/api/v1/teams/p/ai/usage?from=2026-01-01", &obj),
            "usage"
        );
        assert_eq!(
            derive_kind("/api/v1/teams/p/learning-sessions?status=active", &arr),
            "learningSessionList"
        );
        // action sub-routes resolve to the governing collection, not the verb.
        assert_eq!(
            derive_kind("/api/v1/teams/p/secrets/s1/rotate", &obj),
            "secret"
        );
    }

    #[test]
    fn singularize_does_not_truncate_non_plurals() {
        // `status` is not a plural — must not become `statu` (Obs-2).
        assert_eq!(singularize("status"), "status");
        // genuine plurals still singularize.
        assert_eq!(singularize("clusters"), "cluster");
        assert_eq!(singularize("policies"), "policy");
        assert_eq!(singularize("route-configs"), "routeConfig");
        // `…ss` words are left intact.
        assert_eq!(singularize("address"), "address");
    }

    #[test]
    fn resolve_kind_fixes_status_expose_and_singleton_views() {
        let obj = serde_json::json!({ "ok": true });
        let arr = serde_json::json!([{ "x": 1 }]);
        // status views never truncate to `statu`.
        assert_eq!(
            resolve_kind("/api/v1/teams/p/mcp/status", &obj),
            "mcpStatus"
        );
        assert_eq!(
            resolve_kind("/api/v1/teams/p/xds/status", &obj),
            "xdsStatus"
        );
        assert_eq!(
            resolve_kind("/api/v1/teams/p/api-definitions/d1/status", &obj),
            "apiDefinitionStatus"
        );
        // unexpose returns a body — a mutation action, never the resource name.
        assert_eq!(
            resolve_kind("/api/v1/teams/p/expose/local", &obj),
            "mutationResult"
        );
        assert_eq!(
            resolve_kind("/api/v1/teams/p/expose", &obj),
            "mutationResult"
        );
        // other body-returning mutation actions are mutationResult too.
        assert_eq!(
            resolve_kind("/api/v1/teams/p/secrets/s1/rotate", &obj),
            "mutationResult"
        );
        assert_eq!(
            resolve_kind("/api/v1/teams/p/api-definitions/d1/specs/3/publish", &obj),
            "mutationResult"
        );
        // singleton views get a stable kind, not a mis-singularized path tail.
        assert_eq!(
            resolve_kind("/api/v1/teams/p/stats/overview", &obj),
            "statsOverview"
        );
        assert_eq!(resolve_kind("/api/v1/teams/p/ops/trace", &obj), "trace");
        // ai/usage is already a clean singular — no override, no truncation.
        assert_eq!(
            resolve_kind("/api/v1/teams/p/ai/usage?from=x", &obj),
            "usage"
        );
        // true collections still resolve correctly through the resolver.
        assert_eq!(
            resolve_kind("/api/v1/teams/p/clusters", &arr),
            "clusterList"
        );
        assert_eq!(resolve_kind("/api/v1/teams/p/clusters/c1", &obj), "cluster");
    }

    #[test]
    fn resolve_kind_parity_across_every_cli_endpoint() {
        // Exhaustive contract pin (invariant 13). EVERY REST endpoint the CLI calls is checked:
        // an INVARIANT pass asserts no endpoint yields a malformed/truncated/id-shaped kind, and
        // an EXACT pass pins the authoritative value for the confident set (collections,
        // singleton views, mutation actions, and the previously-broken endpoints). A new endpoint
        // added without a kind decision will surface here as a malformed/loose kind.
        let obj = serde_json::json!({ "name": "x" });

        // INVARIANT pass — exhaustive and drift-proof: iterate the *canonical* CLI endpoint
        // registry (`cli_endpoint_templates`), so a new endpoint is automatically checked here.
        // Every endpoint must yield a well-formed lowerCamel kind (no hyphen/slash/`statu`/id).
        for path in crate::cli::cli_endpoint_templates() {
            let got = resolve_kind(path, &obj);
            let base = got.strip_suffix("List").unwrap_or(&got);
            assert!(
                !base.is_empty()
                    && base.chars().next().unwrap().is_ascii_lowercase()
                    && base.chars().all(|c| c.is_ascii_alphanumeric()),
                "kind {got:?} for {path} is malformed (not lowerCamel, or hyphen/slash/id-shaped)"
            );
            assert_ne!(got, "statu", "{path} truncated to statu");
        }

        // Exact authoritative values for the confident set.
        let exact: &[(&str, &str)] = &[
            ("/api/v1/teams/p/clusters/c1", "cluster"),
            ("/api/v1/teams/p/listeners/l1", "listener"),
            ("/api/v1/teams/p/route-configs/r1", "routeConfig"),
            ("/api/v1/teams/p/secrets/s1", "secret"),
            ("/api/v1/teams/p/api-definitions/a1", "apiDefinition"),
            ("/api/v1/teams/p/dataplanes/d1", "dataplane"),
            ("/api/v1/teams/p/rate-limit-domains/d", "rateLimitDomain"),
            ("/api/v1/orgs/acme", "org"),
            ("/api/v1/teams/t1", "team"),
            ("/api/v1/teams/t1/grants/g", "grant"),
            ("/api/v1/auth/whoami", "whoami"),
            // singleton views
            ("/api/v1/teams/p/mcp/status", "mcpStatus"),
            ("/api/v1/teams/p/xds/status", "xdsStatus"),
            (
                "/api/v1/teams/p/api-definitions/a1/status",
                "apiDefinitionStatus",
            ),
            ("/api/v1/teams/p/stats/overview", "statsOverview"),
            ("/api/v1/teams/p/ops/trace", "trace"),
            ("/api/v1/teams/p/ai/trace", "aiTrace"),
            ("/api/v1/teams/p/ai/retention", "aiRetention"),
            ("/api/v1/teams/p/ai/usage", "usage"),
            ("/api/v1/teams/p/dataplanes/d1/envoy-config", "envoyConfig"),
            (
                "/api/v1/teams/p/dataplanes/d1/telemetry",
                "dataplaneTelemetry",
            ),
            (
                "/api/v1/teams/p/rate-limit-domains/d/policies/x/override",
                "rateLimitOverride",
            ),
            // mutation actions
            ("/api/v1/teams/p/expose", "mutationResult"),
            ("/api/v1/teams/p/expose/local", "mutationResult"),
            ("/api/v1/teams/p/secrets/s1/rotate", "mutationResult"),
            (
                "/api/v1/teams/p/api-definitions/a1/specs/3/publish",
                "mutationResult",
            ),
            (
                "/api/v1/teams/p/proxy-certificates/sn/revoke",
                "mutationResult",
            ),
            ("/api/v1/admin/rls/force-repush", "mutationResult"),
            ("/api/v1/teams/p/learning-sessions/s/stop", "mutationResult"),
            // spec-version sub-resources resolve to the spec version, not the parent session.
            (
                "/api/v1/teams/p/learning-sessions/sess/spec-version",
                "specVersion",
            ),
        ];
        for (path, expected) in exact {
            assert_eq!(&resolve_kind(path, &obj), expected, "kind for {path}");
        }
    }

    #[test]
    fn project_fields_keeps_only_requested_keys_inside_data() {
        let fields = vec!["name".to_string(), "revision".to_string()];
        // single object
        let obj = serde_json::json!({ "name": "c1", "revision": 3, "service_name": "svc" });
        assert_eq!(
            project_fields(&obj, &fields),
            serde_json::json!({ "name": "c1", "revision": 3 })
        );
        // array (per item)
        let arr = serde_json::json!([
            { "name": "a", "revision": 1, "x": 9 },
            { "name": "b", "revision": 2, "x": 8 }
        ]);
        assert_eq!(
            project_fields(&arr, &fields),
            serde_json::json!([{ "name": "a", "revision": 1 }, { "name": "b", "revision": 2 }])
        );
        // object wrapping an `items` array
        let items =
            serde_json::json!({ "items": [{ "name": "a", "revision": 1, "x": 9 }], "total": 1 });
        let projected = project_fields(&items, &fields);
        assert_eq!(
            projected["items"],
            serde_json::json!([{ "name": "a", "revision": 1 }])
        );
        // a requested key that is absent is simply omitted (no null injected)
        let missing = project_fields(&obj, &["name".to_string(), "nope".to_string()]);
        assert_eq!(missing, serde_json::json!({ "name": "c1" }));
    }

    #[test]
    fn table_styled_emits_no_ansi_when_color_disabled() {
        let value = serde_json::json!([{ "name": "demo", "revision": 1 }]);
        let plain = table_styled(&value, false);
        assert!(!plain.contains('\x1b'), "no-color table must be ANSI-free");
        let colored = table_styled(&value, true);
        assert!(colored.contains("\x1b[1m"), "colored header should be bold");
        assert!(colored.contains("\x1b[0m"), "colored header should reset");
    }
}
