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
        statuses.entry(status).or_insert(None);
    }
    let body_sample_count = rows
        .iter()
        .filter(|row| row.request_body.is_some() || row.response_body.is_some())
        .count() as u64;
    let truncated_body_count = rows
        .iter()
        .filter(|row| row.request_body_truncated || row.response_body_truncated)
        .count() as u64;
    LearnedEndpointAggregate {
        key: LearnedEndpointKey {
            host,
            method,
            path_template,
        },
        operation_id: String::new(),
        request_schema: None,
        response_schemas: statuses,
        request_headers: Vec::new(),
        response_headers: Vec::new(),
        confidence: LearnedConfidence {
            score: grouping_confidence(rows.len()),
            sample_count: rows.len() as u64,
            body_sample_count,
            distinct_path_count: rows
                .iter()
                .map(|row| row.path.as_str())
                .collect::<BTreeSet<_>>()
                .len() as u64,
            truncated_body_count,
            dropped_observation_count: 0,
        },
    }
}

fn grouping_confidence(sample_count: usize) -> f64 {
    if sample_count >= 10 {
        1.0
    } else {
        sample_count as f64 / 10.0
    }
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
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
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
}
