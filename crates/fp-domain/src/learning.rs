//! S8.6 learning aggregation contract.
//!
//! Domain owns the shape and deterministic OpenAPI output. Core/storage/API decide when and
//! where to run or persist it.

use crate::api_lifecycle::{
    HttpMethod, RawObservation, SpecFormat, SpecSourceKind, SpecVersionInput,
};
use crate::error::{DomainError, DomainResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const DEFAULT_LEARNED_OUTLIER_PATH: &str = "/_flowplane/outliers/{path}";
const REQUIRED_FIELD_THRESHOLD: f64 = 0.8;
const REQUIRED_FIELD_MIN_SAMPLES: u64 = 2;
const LEARNED_HEADER_THRESHOLD: f64 = 0.5;
const LEARNED_HEADER_MIN_SAMPLES: u64 = 2;
const MAX_LEARNED_HEADERS: usize = 16;
const MAX_LEARNED_HEADER_VALUE_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointGroupingConfig {
    pub low_cardinality_literal_limit: usize,
    pub max_path_templates: usize,
}

impl Default for EndpointGroupingConfig {
    fn default() -> Self {
        Self {
            low_cardinality_literal_limit: 3,
            max_path_templates: 500,
        }
    }
}

impl EndpointGroupingConfig {
    pub fn validate(&self) -> DomainResult<()> {
        if !(1..=32).contains(&self.low_cardinality_literal_limit) {
            return Err(DomainError::validation(
                "low_cardinality_literal_limit must be between 1 and 32",
            ));
        }
        if !(1..=10_000).contains(&self.max_path_templates) {
            return Err(DomainError::validation(
                "max_path_templates must be between 1 and 10000",
            ));
        }
        Ok(())
    }
}

pub fn group_observations_by_endpoint(
    observations: &[RawObservation],
    config: EndpointGroupingConfig,
) -> DomainResult<Vec<LearnedEndpointAggregate>> {
    config.validate()?;
    if let Some(first) = observations.first() {
        for observation in observations {
            if observation.team_id != first.team_id
                || observation.capture_session_id != first.capture_session_id
            {
                return Err(DomainError::validation(
                    "learned endpoint grouping requires one team and capture session",
                ));
            }
        }
    }
    let mut contexts: BTreeMap<(Option<String>, &'static str), Vec<&RawObservation>> =
        BTreeMap::new();
    for observation in observations {
        let method = HttpMethod::parse(&observation.method)?;
        contexts
            .entry((observation_host(observation), method.openapi_method()))
            .or_default()
            .push(observation);
    }

    let mut endpoints = Vec::new();
    for ((host, method), rows) in contexts {
        let segment_stats = segment_stats(&rows);
        let mut grouped: BTreeMap<String, Vec<&RawObservation>> = BTreeMap::new();
        for row in rows {
            grouped
                .entry(path_template(row.path.as_str(), &segment_stats, config))
                .or_default()
                .push(row);
        }
        grouped = bucket_path_overflow(grouped, config.max_path_templates);
        for (path_template, rows) in grouped {
            endpoints.push(endpoint_from_group(
                host.clone(),
                parse_openapi_method(method),
                path_template,
                &rows,
            ));
        }
    }

    Ok(endpoints)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedSpecCandidate {
    pub title: String,
    pub version: String,
    #[serde(default)]
    pub endpoints: Vec<LearnedEndpointAggregate>,
}

impl LearnedSpecCandidate {
    pub fn validate(&self) -> DomainResult<()> {
        validate_text("learned spec title", &self.title, 1, 160)?;
        validate_text("learned spec version", &self.version, 1, 64)?;
        if self.endpoints.is_empty() {
            return Err(DomainError::validation(
                "learned spec candidate must contain at least one endpoint",
            ));
        }
        for endpoint in &self.endpoints {
            endpoint.validate()?;
        }
        let mut openapi_operations = BTreeSet::new();
        for endpoint in &self.endpoints {
            let key = (
                endpoint.key.path_template.as_str(),
                endpoint.key.method.openapi_method(),
            );
            if !openapi_operations.insert(key) {
                return Err(DomainError::validation(
                    "learned spec candidate cannot contain duplicate OpenAPI path/method operations",
                )
                .with_hint(
                    "split host-distinct endpoints into separate learned specs or snapshots before rendering OpenAPI",
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_openapi(&self) -> DomainResult<Value> {
        self.validate()?;

        let mut paths = Map::new();
        let mut endpoints = self.endpoints.clone();
        endpoints.sort_by(|a, b| {
            (
                a.key.host.as_deref().unwrap_or(""),
                a.key.method.openapi_method(),
                a.key.path_template.as_str(),
            )
                .cmp(&(
                    b.key.host.as_deref().unwrap_or(""),
                    b.key.method.openapi_method(),
                    b.key.path_template.as_str(),
                ))
        });

        for endpoint in endpoints {
            let path_item = paths
                .entry(endpoint.key.path_template.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            let Value::Object(methods) = path_item else {
                return Err(DomainError::internal("OpenAPI path item was not an object"));
            };
            methods.insert(
                endpoint.key.method.openapi_method().into(),
                endpoint.canonical_operation()?,
            );
        }

        let spec = Value::Object(Map::from_iter([
            ("openapi".into(), Value::String("3.1.0".into())),
            (
                "info".into(),
                Value::Object(Map::from_iter([
                    ("title".into(), Value::String(self.title.clone())),
                    ("version".into(), Value::String(self.version.clone())),
                ])),
            ),
            ("paths".into(), Value::Object(paths)),
        ]));

        Ok(canonicalize_json(spec))
    }

    pub fn spec_version_input(&self) -> DomainResult<SpecVersionInput> {
        let input = SpecVersionInput {
            source_kind: SpecSourceKind::Learned,
            format: SpecFormat::OpenApi3,
            spec: self.canonical_openapi()?,
        };
        input.validate()?;
        Ok(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedEndpointKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub method: HttpMethod,
    pub path_template: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedEndpointAggregate {
    pub key: LearnedEndpointKey,
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub request_schema: Option<Value>,
    #[serde(default)]
    pub response_schemas: BTreeMap<String, Option<Value>>,
    #[serde(default)]
    pub request_headers: Vec<LearnedHeader>,
    #[serde(default)]
    pub response_headers: Vec<LearnedHeader>,
    pub confidence: LearnedConfidence,
}

impl LearnedEndpointAggregate {
    fn validate(&self) -> DomainResult<()> {
        if let Some(host) = &self.key.host {
            validate_text("learned endpoint host", host, 1, 253)?;
        }
        if !self.key.path_template.starts_with('/')
            || self.key.path_template.contains('\0')
            || self.key.path_template.len() > 2048
        {
            return Err(DomainError::validation(
                "learned endpoint path_template must start with / and be at most 2048 characters",
            ));
        }
        if !self.operation_id.is_empty() {
            validate_text("learned endpoint operation_id", &self.operation_id, 1, 200)?;
        }
        if self.response_schemas.is_empty() {
            return Err(DomainError::validation(
                "learned endpoint must contain at least one response status",
            ));
        }
        for status in self.response_schemas.keys() {
            validate_status(status)?;
        }
        for header in self.request_headers.iter().chain(&self.response_headers) {
            header.validate()?;
        }
        self.confidence.validate()
    }

    fn canonical_operation(&self) -> DomainResult<Value> {
        let operation_id = if self.operation_id.is_empty() {
            fallback_operation_id(self.key.method, &self.key.path_template)
        } else {
            self.operation_id.clone()
        };

        let mut operation = Map::new();
        operation.insert("operationId".into(), Value::String(operation_id));

        let mut parameters = Vec::new();
        for parameter in path_parameters(&self.key.path_template) {
            parameters.push(Value::Object(Map::from_iter([
                ("name".into(), Value::String(parameter)),
                ("in".into(), Value::String("path".into())),
                ("required".into(), Value::Bool(true)),
                (
                    "schema".into(),
                    Value::Object(Map::from_iter([(
                        "type".into(),
                        Value::String("string".into()),
                    )])),
                ),
            ])));
        }
        for header in sorted_headers(&self.request_headers) {
            parameters.push(header.parameter_value());
        }
        if !parameters.is_empty() {
            operation.insert("parameters".into(), Value::Array(parameters));
        }

        if let Some(schema) = &self.request_schema {
            operation.insert(
                "requestBody".into(),
                Value::Object(Map::from_iter([
                    ("required".into(), Value::Bool(false)),
                    (
                        "content".into(),
                        json_content(canonicalize_json(schema.clone())),
                    ),
                ])),
            );
        }

        let mut responses = Map::new();
        for (status, schema) in &self.response_schemas {
            let mut response = Map::new();
            response.insert(
                "description".into(),
                Value::String(response_description(status)),
            );
            if let Some(schema) = schema {
                response.insert(
                    "content".into(),
                    json_content(canonicalize_json(schema.clone())),
                );
            }
            responses.insert(status.clone(), Value::Object(response));
        }
        operation.insert("responses".into(), Value::Object(responses));
        operation.insert(
            "x-flowplane-learning".into(),
            serde_json::to_value(&self.confidence)
                .map_err(|e| DomainError::internal(format!("serialize learned confidence: {e}")))?,
        );
        Ok(Value::Object(operation))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedHeader {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

impl LearnedHeader {
    fn validate(&self) -> DomainResult<()> {
        if self.name.is_empty()
            || self.name.len() > 128
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(DomainError::validation(
                "learned header names must be 1-128 chars of ASCII alnum, - or _",
            ));
        }
        Ok(())
    }

    fn parameter_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("name".into(), Value::String(self.name.to_ascii_lowercase())),
            ("in".into(), Value::String("header".into())),
            ("required".into(), Value::Bool(false)),
            (
                "schema".into(),
                self.schema
                    .clone()
                    .map(canonicalize_json)
                    .unwrap_or_else(|| {
                        Value::Object(Map::from_iter([(
                            "type".into(),
                            Value::String("string".into()),
                        )]))
                    }),
            ),
        ]))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedConfidence {
    pub score: f64,
    pub sample_count: u64,
    pub body_sample_count: u64,
    pub distinct_path_count: u64,
    pub truncated_body_count: u64,
    pub dropped_header_count: u64,
    pub dropped_observation_count: u64,
}

impl LearnedConfidence {
    fn validate(&self) -> DomainResult<()> {
        if !(0.0..=1.0).contains(&self.score) {
            return Err(DomainError::validation(
                "learned confidence score must be between 0 and 1",
            ));
        }
        if self.sample_count == 0 {
            return Err(DomainError::validation(
                "learned confidence sample_count must be greater than 0",
            ));
        }
        Ok(())
    }
}

trait OpenApiMethod {
    fn openapi_method(self) -> &'static str;
}

impl OpenApiMethod for HttpMethod {
    fn openapi_method(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
            Self::Options => "options",
            Self::Head => "head",
        }
    }
}

fn validate_text(label: &str, value: &str, min: usize, max: usize) -> DomainResult<()> {
    if value.len() < min || value.len() > max || value.contains('\0') {
        return Err(DomainError::validation(format!(
            "{label} must be {min}-{max} characters and contain no NUL"
        )));
    }
    Ok(())
}

fn validate_status(status: &str) -> DomainResult<()> {
    if status == "default" {
        return Ok(());
    }
    if status.len() == 3 && status.chars().all(|c| c.is_ascii_digit()) {
        return Ok(());
    }
    Err(DomainError::validation(
        "learned response status must be a 3-digit status code or default",
    ))
}

fn parse_openapi_method(method: &str) -> HttpMethod {
    match method {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "patch" => HttpMethod::Patch,
        "delete" => HttpMethod::Delete,
        "options" => HttpMethod::Options,
        "head" => HttpMethod::Head,
        _ => unreachable!("method came from HttpMethod::openapi_method"),
    }
}

fn observation_host(observation: &RawObservation) -> Option<String> {
    header_string(&observation.request_headers, "host")
        .or_else(|| header_string(&observation.request_headers, "authority"))
        .map(|host| host.to_ascii_lowercase())
}

fn header_string(headers: &Value, name: &str) -> Option<String> {
    headers.as_object().and_then(|headers| {
        headers.iter().find_map(|(key, value)| {
            if key.eq_ignore_ascii_case(name) {
                value.as_str().map(str::to_owned)
            } else {
                None
            }
        })
    })
}

fn segment_stats(rows: &[&RawObservation]) -> BTreeMap<usize, BTreeSet<String>> {
    let mut stats = BTreeMap::new();
    for row in rows {
        for (index, segment) in path_segments(&row.path).into_iter().enumerate() {
            stats
                .entry(index)
                .or_insert_with(BTreeSet::new)
                .insert(segment);
        }
    }
    stats
}

fn path_template(
    path: &str,
    stats: &BTreeMap<usize, BTreeSet<String>>,
    config: EndpointGroupingConfig,
) -> String {
    let segments = path_segments(path);
    if segments.is_empty() {
        return "/".into();
    }
    let mut template = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        let distinct = stats.get(&index).map_or(1, BTreeSet::len);
        if is_strong_dynamic_segment(segment) || distinct > config.low_cardinality_literal_limit {
            template.push(parameter_name(&template, index, segment));
        } else {
            template.push(segment.clone());
        }
    }
    format!("/{}", template.join("/"))
}

fn path_segments(path: &str) -> Vec<String> {
    path.split_once('?')
        .map_or(path, |(path, _)| path)
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_strong_dynamic_segment(segment: &str) -> bool {
    is_uuid(segment) || segment.chars().all(|c| c.is_ascii_digit())
}

fn is_uuid(segment: &str) -> bool {
    let parts = segment.split('-').map(str::len).collect::<Vec<_>>();
    parts == [8, 4, 4, 4, 12] && segment.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

fn parameter_name(previous_template: &[String], index: usize, segment: &str) -> String {
    let suffix = if is_strong_dynamic_segment(segment) {
        "Id"
    } else {
        "Param"
    };
    let stem = previous_template
        .iter()
        .rev()
        .find(|segment| !segment.starts_with('{'))
        .map(|segment| singularize(segment))
        .unwrap_or_else(|| format!("param{}", index + 1));
    format!("{{{stem}{suffix}}}")
}

fn singularize(segment: &str) -> String {
    if segment.ends_with("ies") && segment.len() > 3 {
        format!("{}y", &segment[..segment.len() - 3])
    } else if segment.ends_with('s') && !segment.ends_with("ss") && segment.len() > 1 {
        segment[..segment.len() - 1].to_owned()
    } else {
        segment.to_owned()
    }
}

fn bucket_path_overflow(
    grouped: BTreeMap<String, Vec<&RawObservation>>,
    max_path_templates: usize,
) -> BTreeMap<String, Vec<&RawObservation>> {
    if grouped.len() <= max_path_templates {
        return grouped;
    }
    let mut ranked = grouped.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(a_path, a_rows), (b_path, b_rows)| {
        b_rows
            .len()
            .cmp(&a_rows.len())
            .then_with(|| a_path.cmp(b_path))
    });

    let keep = max_path_templates.saturating_sub(1);
    let mut result = BTreeMap::new();
    let mut outliers = Vec::new();
    for (index, (path, rows)) in ranked.into_iter().enumerate() {
        if index < keep {
            result.insert(path, rows);
        } else {
            outliers.extend(rows);
        }
    }
    result.insert(DEFAULT_LEARNED_OUTLIER_PATH.into(), outliers);
    result
}

fn endpoint_from_group(
    host: Option<String>,
    method: HttpMethod,
    path_template: String,
    rows: &[&RawObservation],
) -> LearnedEndpointAggregate {
    let mut statuses = BTreeMap::new();
    for row in rows {
        let status = row
            .response_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "default".into());
        statuses.entry(status).or_insert_with(Vec::new).push(
            row.response_body
                .as_deref()
                .filter(|_| !row.response_body_truncated),
        );
    }
    let response_schemas = statuses
        .into_iter()
        .map(|(status, bodies)| (status, infer_body_schema(bodies.into_iter().flatten())))
        .collect();
    let request_schema = infer_body_schema(
        rows.iter()
            .filter(|row| !row.request_body_truncated)
            .filter_map(|row| row.request_body.as_deref()),
    );
    let body_sample_count = rows
        .iter()
        .filter(|row| row.request_body.is_some() || row.response_body.is_some())
        .count() as u64;
    let truncated_body_count = rows
        .iter()
        .filter(|row| row.request_body_truncated || row.response_body_truncated)
        .count() as u64;
    let (request_headers, dropped_request_headers) = learn_headers(rows, HeaderDirection::Request);
    let (response_headers, dropped_response_headers) =
        learn_headers(rows, HeaderDirection::Response);
    let dropped_header_count = dropped_request_headers.saturating_add(dropped_response_headers);
    LearnedEndpointAggregate {
        key: LearnedEndpointKey {
            host,
            method,
            path_template,
        },
        operation_id: String::new(),
        request_schema,
        response_schemas,
        request_headers,
        response_headers,
        confidence: LearnedConfidence {
            score: endpoint_confidence(
                rows,
                body_sample_count,
                truncated_body_count,
                dropped_header_count,
            ),
            sample_count: rows.len() as u64,
            body_sample_count,
            distinct_path_count: rows
                .iter()
                .map(|row| row.path.as_str())
                .collect::<BTreeSet<_>>()
                .len() as u64,
            truncated_body_count,
            dropped_header_count,
            dropped_observation_count: 0,
        },
    }
}

#[derive(Debug, Clone, Copy)]
enum HeaderDirection {
    Request,
    Response,
}

fn learn_headers(
    rows: &[&RawObservation],
    direction: HeaderDirection,
) -> (Vec<LearnedHeader>, u64) {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut dropped_count = 0_u64;

    for row in rows {
        let headers = match direction {
            HeaderDirection::Request => &row.request_headers,
            HeaderDirection::Response => &row.response_headers,
        };
        let Some(headers) = headers.as_object() else {
            continue;
        };

        let mut seen_in_row = BTreeSet::new();
        for (name, value) in headers {
            let normalized = name.to_ascii_lowercase();
            if is_structural_header(&normalized) {
                continue;
            }
            if !is_safe_learned_header(direction, &normalized)
                || value
                    .as_str()
                    .is_none_or(|v| v.len() > MAX_LEARNED_HEADER_VALUE_LEN)
            {
                dropped_count = dropped_count.saturating_add(1);
                continue;
            }
            if seen_in_row.insert(normalized.clone()) {
                counts
                    .entry(normalized)
                    .and_modify(|count| *count = count.saturating_add(1))
                    .or_insert(1);
            }
        }
    }

    let min_count = LEARNED_HEADER_MIN_SAMPLES
        .max(((rows.len() as f64) * LEARNED_HEADER_THRESHOLD).ceil() as u64);
    let mut learned = counts
        .into_iter()
        .filter(|(_, count)| *count >= min_count)
        .map(|(name, _)| LearnedHeader { name, schema: None })
        .collect::<Vec<_>>();
    if learned.len() > MAX_LEARNED_HEADERS {
        dropped_count = dropped_count.saturating_add((learned.len() - MAX_LEARNED_HEADERS) as u64);
        learned.truncate(MAX_LEARNED_HEADERS);
    }

    (learned, dropped_count)
}

fn is_structural_header(name: &str) -> bool {
    matches!(
        name,
        "host" | "authority" | ":authority" | ":method" | ":path" | ":scheme"
    )
}

fn is_safe_learned_header(direction: HeaderDirection, name: &str) -> bool {
    if is_blocked_header(name) {
        return false;
    }
    match direction {
        HeaderDirection::Request => matches!(
            name,
            "accept"
                | "accept-language"
                | "content-type"
                | "user-agent"
                | "x-account"
                | "x-api-version"
                | "x-client-version"
                | "x-requested-with"
                | "x-tenant"
        ),
        HeaderDirection::Response => matches!(
            name,
            "cache-control"
                | "content-type"
                | "etag"
                | "location"
                | "x-ratelimit-limit"
                | "x-ratelimit-remaining"
                | "x-ratelimit-reset"
        ),
    }
}

fn is_blocked_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-auth-token"
            | "x-csrf-token"
            | "x-session-id"
            | "connection"
            | "content-length"
            | "date"
            | "server"
            | "traceparent"
            | "tracestate"
            | "transfer-encoding"
            | "via"
            | "x-request-id"
    ) || name.starts_with("x-amzn-")
        || name.starts_with("x-b3-")
        || name.starts_with("x-envoy-")
        || name.starts_with("x-forwarded-")
        || name.starts_with("x-trace-")
}

#[derive(Debug, Clone, Default)]
struct JsonSchemaStats {
    total: u64,
    types: BTreeSet<&'static str>,
    object_count: u64,
    properties: BTreeMap<String, JsonSchemaStats>,
    array_items: Option<Box<JsonSchemaStats>>,
}

impl JsonSchemaStats {
    fn observe(&mut self, value: &Value) {
        self.total = self.total.saturating_add(1);
        self.types.insert(json_type(value));
        match value {
            Value::Object(map) => {
                self.object_count = self.object_count.saturating_add(1);
                for (key, value) in map {
                    self.properties
                        .entry(key.clone())
                        .or_default()
                        .observe(value);
                }
            }
            Value::Array(items) => {
                let items_stats = self.array_items.get_or_insert_with(Default::default);
                for item in items {
                    items_stats.observe(item);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    fn schema(&self) -> Value {
        if self.types.len() > 1 {
            return Value::Object(Map::from_iter([(
                "oneOf".into(),
                Value::Array(self.types.iter().map(|kind| type_schema(kind)).collect()),
            )]));
        }
        match self.types.iter().next().copied() {
            Some("object") => self.object_schema(),
            Some("array") => {
                let mut schema = Map::from_iter([("type".into(), Value::String("array".into()))]);
                if let Some(items) = &self.array_items {
                    if items.total > 0 {
                        schema.insert("items".into(), items.schema());
                    }
                }
                Value::Object(schema)
            }
            Some(kind) => type_schema(kind),
            None => Value::Object(Map::new()),
        }
    }

    fn object_schema(&self) -> Value {
        let mut schema = Map::from_iter([("type".into(), Value::String("object".into()))]);
        if !self.properties.is_empty() {
            schema.insert(
                "properties".into(),
                Value::Object(
                    self.properties
                        .iter()
                        .map(|(name, stats)| (name.clone(), stats.schema()))
                        .collect(),
                ),
            );
            let required = self
                .properties
                .iter()
                .filter(|(_, stats)| {
                    self.object_count >= REQUIRED_FIELD_MIN_SAMPLES
                        && (stats.total as f64 / self.object_count as f64)
                            >= REQUIRED_FIELD_THRESHOLD
                })
                .map(|(name, _)| Value::String(name.clone()))
                .collect::<Vec<_>>();
            if !required.is_empty() {
                schema.insert("required".into(), Value::Array(required));
            }
        }
        Value::Object(schema)
    }
}

fn infer_body_schema<'a>(bodies: impl Iterator<Item = &'a str>) -> Option<Value> {
    let mut stats = JsonSchemaStats::default();
    for body in bodies {
        if let Ok(value) = serde_json::from_str::<Value>(body) {
            stats.observe(&value);
        }
    }
    (stats.total > 0).then(|| stats.schema())
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_schema(kind: &str) -> Value {
    Value::Object(Map::from_iter([(
        "type".into(),
        Value::String(kind.to_owned()),
    )]))
}

fn endpoint_confidence(
    rows: &[&RawObservation],
    body_sample_count: u64,
    truncated_body_count: u64,
    dropped_header_count: u64,
) -> f64 {
    let sample_score = (rows.len().min(10) as f64) / 10.0;
    let body_score = if body_sample_count == 0 {
        0.8
    } else {
        body_sample_count as f64 / rows.len() as f64
    };
    let truncation_penalty = truncated_body_count as f64 / rows.len() as f64;
    let header_penalty = (dropped_header_count as f64 / rows.len() as f64).min(1.0);
    ((0.6 * sample_score) + (0.4 * body_score)
        - (0.3 * truncation_penalty)
        - (0.1 * header_penalty))
        .clamp(0.0, 1.0)
}

fn path_parameters(path: &str) -> Vec<String> {
    let mut params = path
        .split('/')
        .filter_map(|segment| {
            segment
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    params.sort();
    params.dedup();
    params
}

fn sorted_headers(headers: &[LearnedHeader]) -> Vec<LearnedHeader> {
    let mut headers = headers.to_vec();
    headers.sort_by_key(|h| h.name.to_ascii_lowercase());
    headers.dedup_by_key(|h| h.name.to_ascii_lowercase());
    headers
}

fn json_content(schema: Value) -> Value {
    Value::Object(Map::from_iter([(
        "application/json".into(),
        Value::Object(Map::from_iter([("schema".into(), schema)])),
    )]))
}

fn response_description(status: &str) -> String {
    match status {
        "204" => "No Content".into(),
        _ => format!("HTTP {status}"),
    }
}

fn fallback_operation_id(method: HttpMethod, path: &str) -> String {
    let suffix = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim_matches(|c| c == '{' || c == '}')
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if suffix.is_empty() {
        method.openapi_method().into()
    } else {
        format!("{}_{suffix}", method.openapi_method())
    }
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_json).collect()),
        Value::Object(map) => {
            let sorted = map
                .into_iter()
                .map(|(k, v)| (k, canonicalize_json(v)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(Map::from_iter(sorted))
        }
        scalar => scalar,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use crate::{CaptureSessionId, RawObservationId, TeamId};
    use chrono::Utc;

    #[test]
    fn canonical_openapi_is_stable_for_shuffled_aggregates() {
        let a = candidate(vec![orders_post(), users_get()]);
        let b = candidate(vec![users_get(), orders_post()]);

        let a_json = serde_json::to_string(&a.canonical_openapi().unwrap()).unwrap();
        let b_json = serde_json::to_string(&b.canonical_openapi().unwrap()).unwrap();
        assert_eq!(a_json, b_json);
        assert!(a_json.find("/orders").unwrap() < a_json.find("/users/{userId}").unwrap());
    }

    #[test]
    fn canonical_openapi_sorts_headers_and_statuses() {
        let spec = candidate(vec![users_get()])
            .canonical_openapi()
            .expect("canonical spec");
        let json = serde_json::to_string(&spec).unwrap();

        assert!(json.find("\"200\"").unwrap() < json.find("\"404\"").unwrap());
        assert!(json.find("x-account").unwrap() < json.find("x-requested-with").unwrap());
    }

    #[test]
    fn learned_candidate_outputs_valid_spec_version_input() {
        let input = candidate(vec![users_get()])
            .spec_version_input()
            .expect("spec input");
        assert_eq!(input.source_kind, SpecSourceKind::Learned);
        assert_eq!(input.format, SpecFormat::OpenApi3);
        assert!(input.validate().is_ok());
    }

    #[test]
    fn grouping_keeps_static_paths_and_detects_id_params() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation(team, session, "GET", "/users/123", "api.example.test"),
                observation(team, session, "GET", "/users/456", "api.example.test"),
                observation(team, session, "GET", "/health", "api.example.test"),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let paths = grouped
            .iter()
            .map(|endpoint| endpoint.key.path_template.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/health", "/users/{userId}"]);
    }

    #[test]
    fn grouping_preserves_low_cardinality_literal_segments() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation(team, session, "GET", "/reports/daily", "api.example.test"),
                observation(team, session, "GET", "/reports/weekly", "api.example.test"),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let paths = grouped
            .iter()
            .map(|endpoint| endpoint.key.path_template.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/reports/daily", "/reports/weekly"]);
    }

    #[test]
    fn grouping_preserves_stable_alphanumeric_version_segments() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation(team, session, "GET", "/v1/users/1", "api.example.test"),
                observation(team, session, "GET", "/v2/users/2", "api.example.test"),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let paths = grouped
            .iter()
            .map(|endpoint| endpoint.key.path_template.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/v1/users/{userId}", "/v2/users/{userId}"]);
    }

    #[test]
    fn grouping_templates_high_cardinality_alphanumeric_segments() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation(team, session, "GET", "/assets/a1", "api.example.test"),
                observation(team, session, "GET", "/assets/b2", "api.example.test"),
                observation(team, session, "GET", "/assets/c3", "api.example.test"),
                observation(team, session, "GET", "/assets/d4", "api.example.test"),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let paths = grouped
            .iter()
            .map(|endpoint| endpoint.key.path_template.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/assets/{assetParam}"]);
    }

    #[test]
    fn grouping_infers_json_schema_without_requiring_sparse_optional_fields() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation_with_bodies(
                    team,
                    session,
                    "POST",
                    "/users",
                    "api.example.test",
                    Some(r#"{"email":"a@example.test","nickname":"a"}"#),
                    Some(r#"{"id":1,"email":"a@example.test"}"#),
                    false,
                    false,
                ),
                observation_with_bodies(
                    team,
                    session,
                    "POST",
                    "/users",
                    "api.example.test",
                    Some(r#"{"email":"b@example.test"}"#),
                    Some(r#"{"id":2,"email":"b@example.test"}"#),
                    false,
                    false,
                ),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let request = grouped[0].request_schema.as_ref().expect("request schema");
        assert_eq!(
            request.pointer("/properties/email/type"),
            Some(&Value::String("string".into()))
        );
        assert_eq!(
            request.pointer("/properties/nickname/type"),
            Some(&Value::String("string".into()))
        );
        assert_eq!(
            request.pointer("/required"),
            Some(&serde_json::json!(["email"]))
        );
    }

    #[test]
    fn grouping_infers_mixed_arrays_and_nullability() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation_with_bodies(
                    team,
                    session,
                    "POST",
                    "/events",
                    "api.example.test",
                    Some(r#"{"tags":["a",1],"memo":null}"#),
                    Some(r#"{"ok":true}"#),
                    false,
                    false,
                ),
                observation_with_bodies(
                    team,
                    session,
                    "POST",
                    "/events",
                    "api.example.test",
                    Some(r#"{"tags":["b"],"memo":"later"}"#),
                    Some(r#"{"ok":true}"#),
                    false,
                    false,
                ),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let request = grouped[0].request_schema.as_ref().expect("request schema");
        assert_eq!(
            request.pointer("/properties/tags/type"),
            Some(&Value::String("array".into()))
        );
        assert_eq!(
            request.pointer("/properties/tags/items/oneOf"),
            Some(&serde_json::json!([{"type":"integer"},{"type":"string"}]))
        );
        assert_eq!(
            request.pointer("/properties/memo/oneOf"),
            Some(&serde_json::json!([{"type":"null"},{"type":"string"}]))
        );
    }

    #[test]
    fn grouping_excludes_truncated_bodies_from_schema_and_confidence() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation_with_bodies(
                    team,
                    session,
                    "POST",
                    "/uploads",
                    "api.example.test",
                    Some(r#"{"name":"ok"}"#),
                    Some(r#"{"accepted":true}"#),
                    false,
                    false,
                ),
                observation_with_bodies(
                    team,
                    session,
                    "POST",
                    "/uploads",
                    "api.example.test",
                    Some(r#"{"name":"broken""#),
                    Some(r#"{"accepted":true"#),
                    true,
                    true,
                ),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let endpoint = &grouped[0];
        assert_eq!(endpoint.confidence.truncated_body_count, 1);
        assert_eq!(
            endpoint
                .request_schema
                .as_ref()
                .and_then(|s| s.pointer("/properties/name/type")),
            Some(&Value::String("string".into()))
        );
    }

    #[test]
    fn grouping_schema_output_is_stable_for_shuffled_observations() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let rows = vec![
            observation_with_bodies(
                team,
                session,
                "POST",
                "/users",
                "api.example.test",
                Some(r#"{"email":"a@example.test","admin":true}"#),
                Some(r#"{"id":1}"#),
                false,
                false,
            ),
            observation_with_bodies(
                team,
                session,
                "POST",
                "/users",
                "api.example.test",
                Some(r#"{"email":"b@example.test","admin":false}"#),
                Some(r#"{"id":2}"#),
                false,
                false,
            ),
        ];
        let mut shuffled = rows.clone();
        shuffled.reverse();

        let a = group_observations_by_endpoint(&rows, EndpointGroupingConfig::default())
            .expect("group observations");
        let b = group_observations_by_endpoint(&shuffled, EndpointGroupingConfig::default())
            .expect("group observations");
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
    }

    #[test]
    fn grouped_schema_candidate_validates_as_spec_version_input() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let endpoints = group_observations_by_endpoint(
            &[observation_with_bodies(
                team,
                session,
                "POST",
                "/users",
                "api.example.test",
                Some(r#"{"email":"a@example.test"}"#),
                Some(r#"{"id":1}"#),
                false,
                false,
            )],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let input = candidate(endpoints)
            .spec_version_input()
            .expect("spec input");
        assert!(input.validate().is_ok());
    }

    #[test]
    fn grouping_excludes_auth_and_volatile_headers() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let mut first = observation(team, session, "GET", "/users", "api.example.test");
        first.request_headers = serde_json::json!({
            "host": "api.example.test",
            "authorization": "Bearer secret",
            "x-request-id": "req-1",
            "x-account": "acct_1"
        });
        let mut second = observation(team, session, "GET", "/users", "api.example.test");
        second.request_headers = serde_json::json!({
            "host": "api.example.test",
            "Authorization": "Bearer other",
            "traceparent": "00-00000000000000000000000000000000-0000000000000000-01",
            "X-Account": "acct_1"
        });

        let grouped =
            group_observations_by_endpoint(&[first, second], EndpointGroupingConfig::default())
                .expect("group observations");

        let endpoint = &grouped[0];
        assert_eq!(
            endpoint
                .request_headers
                .iter()
                .map(|header| header.name.as_str())
                .collect::<Vec<_>>(),
            vec!["x-account"]
        );
        assert_eq!(endpoint.confidence.dropped_header_count, 4);
    }

    #[test]
    fn grouping_requires_frequency_before_learning_headers() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let mut sparse = observation(team, session, "GET", "/users", "api.example.test");
        sparse.request_headers = serde_json::json!({
            "host": "api.example.test",
            "x-client-version": "2026.06"
        });

        let grouped = group_observations_by_endpoint(
            &[
                sparse,
                observation(team, session, "GET", "/users", "api.example.test"),
                observation(team, session, "GET", "/users", "api.example.test"),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        assert!(grouped[0].request_headers.is_empty());
    }

    #[test]
    fn grouping_caps_header_output_and_records_flood_drops() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let mut headers =
            Map::from_iter([("host".into(), Value::String("api.example.test".into()))]);
        for index in 0..64 {
            headers.insert(format!("x-noise-{index}"), Value::String("junk".into()));
        }
        let mut first = observation(team, session, "GET", "/users", "api.example.test");
        first.request_headers = Value::Object(headers.clone());
        let mut second = observation(team, session, "GET", "/users", "api.example.test");
        second.request_headers = Value::Object(headers);

        let grouped =
            group_observations_by_endpoint(&[first, second], EndpointGroupingConfig::default())
                .expect("group observations");

        let endpoint = &grouped[0];
        assert!(endpoint.request_headers.len() <= MAX_LEARNED_HEADERS);
        assert_eq!(endpoint.confidence.dropped_header_count, 128);
        assert!(endpoint.confidence.score < 0.68);
    }

    #[test]
    fn grouping_learns_headers_in_deterministic_order() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let mut first = observation(team, session, "GET", "/users", "api.example.test");
        first.request_headers = serde_json::json!({
            "host": "api.example.test",
            "x-tenant": "tenant_1",
            "accept-language": "en-AU",
            "content-type": "application/json"
        });
        let mut second = observation(team, session, "GET", "/users", "api.example.test");
        second.request_headers = serde_json::json!({
            "host": "api.example.test",
            "Content-Type": "application/json",
            "Accept-Language": "en-AU",
            "X-Tenant": "tenant_1"
        });

        let grouped =
            group_observations_by_endpoint(&[first, second], EndpointGroupingConfig::default())
                .expect("group observations");

        assert_eq!(
            grouped[0]
                .request_headers
                .iter()
                .map(|header| header.name.as_str())
                .collect::<Vec<_>>(),
            vec!["accept-language", "content-type", "x-tenant"]
        );
        let spec = candidate(grouped).canonical_openapi().expect("openapi");
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.find("accept-language").unwrap() < json.find("content-type").unwrap());
        assert!(json.find("content-type").unwrap() < json.find("x-tenant").unwrap());
    }

    #[test]
    fn grouping_keeps_hosts_separate() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation(team, session, "GET", "/users/123", "api.example.test"),
                observation(team, session, "GET", "/users/123", "admin.example.test"),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect("group observations");

        let hosts = grouped
            .iter()
            .map(|endpoint| endpoint.key.host.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            hosts,
            vec![Some("admin.example.test"), Some("api.example.test")]
        );
    }

    #[test]
    fn grouping_buckets_path_explosion() {
        let team = TeamId::generate();
        let session = CaptureSessionId::generate();
        let grouped = group_observations_by_endpoint(
            &[
                observation(team, session, "GET", "/alpha", "api.example.test"),
                observation(team, session, "GET", "/bravo", "api.example.test"),
                observation(team, session, "GET", "/charlie", "api.example.test"),
            ],
            EndpointGroupingConfig {
                low_cardinality_literal_limit: 3,
                max_path_templates: 2,
            },
        )
        .expect("group observations");

        let paths = grouped
            .iter()
            .map(|endpoint| endpoint.key.path_template.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/_flowplane/outliers/{path}", "/alpha"]);
    }

    #[test]
    fn grouping_rejects_mixed_team_observations() {
        let session = CaptureSessionId::generate();
        let err = group_observations_by_endpoint(
            &[
                observation(
                    TeamId::generate(),
                    session,
                    "GET",
                    "/users/123",
                    "api.example.test",
                ),
                observation(
                    TeamId::generate(),
                    session,
                    "GET",
                    "/users/456",
                    "api.example.test",
                ),
            ],
            EndpointGroupingConfig::default(),
        )
        .expect_err("mixed teams rejected");
        assert!(err.message.contains("one team and capture session"));
    }

    #[test]
    fn duplicate_path_method_operations_are_rejected() {
        let mut duplicate = users_get();
        duplicate.key.host = Some("api.example.test".into());

        let err = candidate(vec![users_get(), duplicate])
            .canonical_openapi()
            .expect_err("duplicate operation is rejected");
        assert!(err.message.contains("duplicate OpenAPI path/method"));
    }

    #[test]
    fn host_distinct_path_method_collisions_are_rejected() {
        let mut other_host = users_get();
        other_host.key.host = Some("admin.example.test".into());

        let err = candidate(vec![users_get(), other_host])
            .canonical_openapi()
            .expect_err("host-flattened operation collision is rejected");
        assert!(err.message.contains("duplicate OpenAPI path/method"));
    }

    fn candidate(endpoints: Vec<LearnedEndpointAggregate>) -> LearnedSpecCandidate {
        LearnedSpecCandidate {
            title: "Learned Catalog".into(),
            version: "1.0.0".into(),
            endpoints,
        }
    }

    fn confidence() -> LearnedConfidence {
        LearnedConfidence {
            score: 0.82,
            sample_count: 12,
            body_sample_count: 10,
            distinct_path_count: 2,
            truncated_body_count: 1,
            dropped_header_count: 0,
            dropped_observation_count: 0,
        }
    }

    fn users_get() -> LearnedEndpointAggregate {
        LearnedEndpointAggregate {
            key: LearnedEndpointKey {
                host: Some("api.example.test".into()),
                method: HttpMethod::Get,
                path_template: "/users/{userId}".into(),
            },
            operation_id: "".into(),
            request_schema: None,
            response_schemas: BTreeMap::from([
                (
                    "404".into(),
                    Some(
                        serde_json::json!({"type": "object", "properties": {"error": {"type": "string"}}}),
                    ),
                ),
                (
                    "200".into(),
                    Some(
                        serde_json::json!({"properties": {"id": {"type": "string"}, "email": {"type": "string"}}, "type": "object"}),
                    ),
                ),
            ]),
            request_headers: vec![
                LearnedHeader {
                    name: "X-Requested-With".into(),
                    schema: None,
                },
                LearnedHeader {
                    name: "x-account".into(),
                    schema: None,
                },
            ],
            response_headers: vec![],
            confidence: confidence(),
        }
    }

    fn orders_post() -> LearnedEndpointAggregate {
        LearnedEndpointAggregate {
            key: LearnedEndpointKey {
                host: Some("api.example.test".into()),
                method: HttpMethod::Post,
                path_template: "/orders".into(),
            },
            operation_id: "createOrder".into(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: BTreeMap::from([(
                "201".into(),
                Some(serde_json::json!({"type": "object"})),
            )]),
            request_headers: vec![],
            response_headers: vec![],
            confidence: confidence(),
        }
    }

    fn observation(
        team_id: TeamId,
        capture_session_id: CaptureSessionId,
        method: &str,
        path: &str,
        host: &str,
    ) -> RawObservation {
        let now = Utc::now();
        RawObservation {
            id: RawObservationId::generate(),
            team_id,
            capture_session_id,
            request_id: format!("{method}-{path}-{host}"),
            method: method.into(),
            path: path.into(),
            response_status: Some(200),
            request_headers: serde_json::json!({ "host": host }),
            response_headers: serde_json::json!({}),
            request_body: None,
            response_body: None,
            request_body_truncated: false,
            response_body_truncated: false,
            request_body_bytes: 0,
            response_body_bytes: 0,
            metadata_seen: true,
            body_seen: false,
            observed_at: now,
            updated_at: now,
            created_at: now,
        }
    }

    fn observation_with_bodies(
        team_id: TeamId,
        capture_session_id: CaptureSessionId,
        method: &str,
        path: &str,
        host: &str,
        request_body: Option<&str>,
        response_body: Option<&str>,
        request_body_truncated: bool,
        response_body_truncated: bool,
    ) -> RawObservation {
        let mut observation = observation(team_id, capture_session_id, method, path, host);
        observation.request_body = request_body.map(str::to_owned);
        observation.response_body = response_body.map(str::to_owned);
        observation.request_body_truncated = request_body_truncated;
        observation.response_body_truncated = response_body_truncated;
        observation.request_body_bytes = request_body.map_or(0, |body| body.len() as i64);
        observation.response_body_bytes = response_body.map_or(0, |body| body.len() as i64);
        observation.body_seen = request_body.is_some() || response_body.is_some();
        observation
    }
}
