//! API lifecycle repositories (S8/D-017).
//!
//! This is intentionally storage-level for S8.1. REST/CLI, OpenAPI parsing, capture
//! sessions, and tool serving build on these records in later S8/S11 slices.

use fp_domain::api_lifecycle::{
    validate_api_name, ApiDefinition, ApiDefinitionSpec, ApiRouteBinding, ApiRouteBindingSpec,
    ApiTool, ApiToolSpec, CaptureSession, CaptureSessionSpec, CaptureSessionStatus, HttpMethod,
    ObservationIngest, RawObservation, RetentionPolicy, RetentionPolicySpec, SpecFormat,
    SpecSourceKind, SpecVersion, SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::{
    ApiDefinitionId, ApiRouteBindingId, ApiToolId, CaptureSessionId, DomainError, DomainResult,
    ErrorCode, ListenerId, RawObservationId, RetentionPolicyId, RouteConfigId, SpecVersionId,
    TeamId,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const API_COLUMNS: &str =
    "id, team_id, name, display_name, description, version, created_at, updated_at";
const BINDING_COLUMNS: &str = "id, team_id, api_definition_id, route_config_id, listener_id, \
    name, virtual_host, route, created_at";
const SPEC_COLUMNS: &str = "id, team_id, api_definition_id, version, source_kind, format, spec, \
    spec_hash, created_at";
const TOOL_COLUMNS: &str = "id, team_id, api_definition_id, spec_version_id, name, operation_id, \
    method, path, input_schema, output_schema, enabled, created_at, updated_at";
const RETENTION_COLUMNS: &str = "id, team_id, api_definition_id, name, raw_observation_ttl_days, \
    max_spec_versions, created_at, updated_at";
const CAPTURE_SESSION_COLUMNS: &str = "id, team_id, name, status, api_definition_id, \
    route_config_id, listener_id, virtual_host, route, target_sample_count, max_duration_seconds, \
    max_bytes, max_distinct_paths, sample_count, byte_count, path_count, drop_count, started_at, \
    completed_at, cancelled_at, updated_at, created_at";
const RAW_OBSERVATION_COLUMNS: &str = "id, team_id, capture_session_id, request_id, method, path, \
    response_status, request_headers, response_headers, request_body, response_body, \
    request_body_truncated, response_body_truncated, request_body_bytes, response_body_bytes, \
    metadata_seen, body_seen, observed_at, updated_at, created_at";
const DEFAULT_RAW_OBSERVATION_TTL_DAYS: i32 = 30;

fn map_unique(e: sqlx::Error, kind: &str, name: &str) -> DomainError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            return DomainError::conflict(format!("{kind} \"{name}\" already exists in this team"))
                .with_hint("choose a different name or update the existing resource");
        }
    }
    DomainError::internal(format!("write {kind}: {e}"))
}

fn api_from_row(row: &PgRow) -> ApiDefinition {
    ApiDefinition {
        id: ApiDefinitionId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn binding_from_row(row: &PgRow) -> ApiRouteBinding {
    ApiRouteBinding {
        id: ApiRouteBindingId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        api_definition_id: ApiDefinitionId::from(row.get::<Uuid, _>("api_definition_id")),
        route_config_id: RouteConfigId::from(row.get::<Uuid, _>("route_config_id")),
        listener_id: row
            .get::<Option<Uuid>, _>("listener_id")
            .map(ListenerId::from),
        name: row.get("name"),
        virtual_host: row.get("virtual_host"),
        route: row.get("route"),
        created_at: row.get("created_at"),
    }
}

fn spec_from_row(row: &PgRow) -> DomainResult<SpecVersion> {
    Ok(SpecVersion {
        id: SpecVersionId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        api_definition_id: ApiDefinitionId::from(row.get::<Uuid, _>("api_definition_id")),
        version: row.get("version"),
        source_kind: SpecSourceKind::parse(&row.get::<String, _>("source_kind"))?,
        format: SpecFormat::parse(&row.get::<String, _>("format"))?,
        spec: row.get("spec"),
        spec_hash: row.get("spec_hash"),
        created_at: row.get("created_at"),
    })
}

fn tool_from_row(row: &PgRow) -> DomainResult<ApiTool> {
    Ok(ApiTool {
        id: ApiToolId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        api_definition_id: ApiDefinitionId::from(row.get::<Uuid, _>("api_definition_id")),
        spec_version_id: SpecVersionId::from(row.get::<Uuid, _>("spec_version_id")),
        name: row.get("name"),
        operation_id: row.get("operation_id"),
        method: HttpMethod::parse(&row.get::<String, _>("method"))?,
        path: row.get("path"),
        input_schema: row.get("input_schema"),
        output_schema: row.get("output_schema"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn retention_from_row(row: &PgRow) -> RetentionPolicy {
    RetentionPolicy {
        id: RetentionPolicyId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        api_definition_id: row
            .get::<Option<Uuid>, _>("api_definition_id")
            .map(ApiDefinitionId::from),
        name: row.get("name"),
        raw_observation_ttl_days: row.get("raw_observation_ttl_days"),
        max_spec_versions: row.get("max_spec_versions"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn capture_session_from_row(row: &PgRow) -> DomainResult<CaptureSession> {
    Ok(CaptureSession {
        id: CaptureSessionId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        status: CaptureSessionStatus::parse(&row.get::<String, _>("status"))?,
        api_definition_id: row
            .get::<Option<Uuid>, _>("api_definition_id")
            .map(ApiDefinitionId::from),
        route_config_id: row
            .get::<Option<Uuid>, _>("route_config_id")
            .map(RouteConfigId::from),
        listener_id: row
            .get::<Option<Uuid>, _>("listener_id")
            .map(ListenerId::from),
        virtual_host: row.get("virtual_host"),
        route: row.get("route"),
        target_sample_count: row.get("target_sample_count"),
        max_duration_seconds: row.get("max_duration_seconds"),
        max_bytes: row.get("max_bytes"),
        max_distinct_paths: row.get("max_distinct_paths"),
        sample_count: row.get("sample_count"),
        byte_count: row.get("byte_count"),
        path_count: row.get("path_count"),
        drop_count: row.get("drop_count"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        cancelled_at: row.get("cancelled_at"),
        updated_at: row.get("updated_at"),
        created_at: row.get("created_at"),
    })
}

fn raw_observation_from_row(row: &PgRow) -> RawObservation {
    RawObservation {
        id: RawObservationId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        capture_session_id: CaptureSessionId::from(row.get::<Uuid, _>("capture_session_id")),
        request_id: row.get("request_id"),
        method: row.get("method"),
        path: row.get("path"),
        response_status: row.get("response_status"),
        request_headers: row.get("request_headers"),
        response_headers: row.get("response_headers"),
        request_body: row.get("request_body"),
        response_body: row.get("response_body"),
        request_body_truncated: row.get("request_body_truncated"),
        response_body_truncated: row.get("response_body_truncated"),
        request_body_bytes: row.get("request_body_bytes"),
        response_body_bytes: row.get("response_body_bytes"),
        metadata_seen: row.get("metadata_seen"),
        body_seen: row.get("body_seen"),
        observed_at: row.get("observed_at"),
        updated_at: row.get("updated_at"),
        created_at: row.get("created_at"),
    }
}

fn canonical_hash(value: &serde_json::Value) -> DomainResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| DomainError::internal(format!("serialize api spec for hash: {e}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn body_bytes(value: Option<&str>) -> i64 {
    value.map_or(0, |body| body.len() as i64)
}

fn sanitize_headers(headers: &Map<String, Value>) -> Value {
    const REDACTED_HEADERS: &[&str] = &[
        "authorization",
        "proxy-authorization",
        "x-api-key",
        "x-auth-token",
        "cookie",
        "set-cookie",
    ];
    const DROPPED_HEADERS: &[&str] = &[
        "connection",
        "content-length",
        "date",
        "server",
        "traceparent",
        "tracestate",
        "x-b3-sampled",
        "x-b3-spanid",
        "x-b3-traceid",
        "x-envoy-attempt-count",
        "x-envoy-decorator-operation",
        "x-envoy-expected-rq-timeout-ms",
        "x-envoy-internal",
        "x-forwarded-client-cert",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-request-id",
    ];
    let mut out = Map::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if DROPPED_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if REDACTED_HEADERS.contains(&lower.as_str()) {
            out.insert(name.clone(), Value::String("[REDACTED]".to_string()));
        } else {
            out.insert(name.clone(), value.clone());
        }
    }
    Value::Object(out)
}

fn merge_headers(existing: Value, incoming: Value) -> Value {
    match incoming {
        Value::Object(map) if map.is_empty() => existing,
        value => value,
    }
}

async fn ensure_api_in_team(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM api_definitions WHERE team_id = $1 AND id = $2)",
    )
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("resolve api definition: {e}")))?;
    if exists {
        Ok(())
    } else {
        Err(DomainError::validation(
            "api definition does not exist in this team",
        ))
    }
}

async fn lock_api_in_team(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<()> {
    let row: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM api_definitions WHERE team_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock api definition: {e}")))?;
    if row.is_some() {
        Ok(())
    } else {
        Err(DomainError::validation(
            "api definition does not exist in this team",
        ))
    }
}

async fn ensure_route_scope_in_team(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    spec: &ApiRouteBindingSpec,
) -> DomainResult<()> {
    let route_config_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM route_configs WHERE team_id = $1 AND id = $2)",
    )
    .bind(team_id.as_uuid())
    .bind(spec.route_config_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("resolve route config: {e}")))?;
    if !route_config_exists {
        return Err(DomainError::validation(
            "route binding references a route config that does not exist in this team",
        ));
    }
    if let Some(listener_id) = spec.listener_id {
        let listener_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM listeners WHERE team_id = $1 AND id = $2)",
        )
        .bind(team_id.as_uuid())
        .bind(listener_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("resolve listener: {e}")))?;
        if !listener_exists {
            return Err(DomainError::validation(
                "route binding references a listener that does not exist in this team",
            ));
        }
    }
    Ok(())
}

async fn ensure_capture_route_scope_in_team(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    listener_id: Option<ListenerId>,
) -> DomainResult<()> {
    let spec = ApiRouteBindingSpec {
        route_config_id,
        listener_id,
        virtual_host: None,
        route: None,
    };
    ensure_route_scope_in_team(tx, team_id, &spec).await
}

pub async fn create_api_definition(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &ApiDefinitionSpec,
) -> DomainResult<ApiDefinition> {
    validate_api_name(name)?;
    spec.validate()?;
    let row = sqlx::query(&format!(
        "INSERT INTO api_definitions (id, team_id, org_id, name, display_name, description) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {API_COLUMNS}"
    ))
    .bind(ApiDefinitionId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(&spec.display_name)
    .bind(&spec.description)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "api", name))?;
    Ok(api_from_row(&row))
}

pub async fn get_api_definition(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<ApiDefinition>> {
    let row = sqlx::query(&format!(
        "SELECT {API_COLUMNS} FROM api_definitions WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get api: {e}")))?;
    Ok(row.as_ref().map(api_from_row))
}

pub async fn get_api_definition_by_id(
    pool: &PgPool,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<Option<ApiDefinition>> {
    let row = sqlx::query(&format!(
        "SELECT {API_COLUMNS} FROM api_definitions WHERE team_id = $1 AND id = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get api by id: {e}")))?;
    Ok(row.as_ref().map(api_from_row))
}

pub async fn list_api_definitions(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<ApiDefinition>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {API_COLUMNS} FROM api_definitions WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list apis: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM api_definitions WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count apis: {e}")))?;
    Ok((rows.iter().map(api_from_row).collect(), total))
}

pub async fn delete_api_definition(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<ApiDefinitionId> {
    let row = sqlx::query(
        "DELETE FROM api_definitions WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete api: {e}")))?;
    match row {
        Some(row) => Ok(ApiDefinitionId::from(row.get::<Uuid, _>("id"))),
        None => {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM api_definitions WHERE team_id = $1 AND name = $2",
            )
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("delete api: recheck: {e}")))?;
            Err(match current {
                Some(version) => DomainError::new(
                    ErrorCode::RevisionMismatch,
                    format!(
                        "api \"{name}\" is at revision {version}, you supplied {expected_version}"
                    ),
                )
                .with_hint("re-read the API and retry with the current revision"),
                None => DomainError::not_found("api", name),
            })
        }
    }
}

pub async fn create_route_binding(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    api_id: ApiDefinitionId,
    name: &str,
    spec: &ApiRouteBindingSpec,
) -> DomainResult<ApiRouteBinding> {
    validate_api_name(name)?;
    spec.validate()?;
    lock_api_in_team(tx, team.id, api_id).await?;
    ensure_route_scope_in_team(tx, team.id, spec).await?;
    let row = sqlx::query(&format!(
        "INSERT INTO api_route_bindings \
         (id, team_id, org_id, api_definition_id, route_config_id, listener_id, name, virtual_host, route) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {BINDING_COLUMNS}"
    ))
    .bind(ApiRouteBindingId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(api_id.as_uuid())
    .bind(spec.route_config_id.as_uuid())
    .bind(spec.listener_id.map(|id| id.as_uuid()))
    .bind(name)
    .bind(&spec.virtual_host)
    .bind(&spec.route)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "api route binding", name))?;
    Ok(binding_from_row(&row))
}

pub async fn create_spec_version(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    api_id: ApiDefinitionId,
    input: &SpecVersionInput,
) -> DomainResult<SpecVersion> {
    input.validate()?;
    lock_api_in_team(tx, team.id, api_id).await?;
    let next_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM spec_versions \
         WHERE team_id = $1 AND api_definition_id = $2",
    )
    .bind(team.id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("allocate spec version: {e}")))?;
    let spec_hash = canonical_hash(&input.spec)?;
    let row = sqlx::query(&format!(
        "INSERT INTO spec_versions \
         (id, team_id, org_id, api_definition_id, version, source_kind, format, spec, spec_hash) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {SPEC_COLUMNS}"
    ))
    .bind(SpecVersionId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(api_id.as_uuid())
    .bind(next_version)
    .bind(input.source_kind.as_str())
    .bind(input.format.as_str())
    .bind(&input.spec)
    .bind(spec_hash)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "spec version", &next_version.to_string()))?;
    spec_from_row(&row)
}

pub async fn create_api_tool(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    api_id: ApiDefinitionId,
    spec_version_id: SpecVersionId,
    name: &str,
    spec: &ApiToolSpec,
) -> DomainResult<ApiTool> {
    validate_api_name(name)?;
    spec.validate()?;
    ensure_api_in_team(tx, team.id, api_id).await?;
    let spec_belongs_to_api: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM spec_versions \
         WHERE team_id = $1 AND api_definition_id = $2 AND id = $3)",
    )
    .bind(team.id.as_uuid())
    .bind(api_id.as_uuid())
    .bind(spec_version_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("resolve spec version: {e}")))?;
    if !spec_belongs_to_api {
        return Err(DomainError::validation(
            "api tool references a spec version that does not belong to this API",
        ));
    }
    let row = sqlx::query(&format!(
        "INSERT INTO api_tools \
         (id, team_id, org_id, api_definition_id, spec_version_id, name, operation_id, method, path, input_schema, output_schema, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {TOOL_COLUMNS}"
    ))
    .bind(ApiToolId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(api_id.as_uuid())
    .bind(spec_version_id.as_uuid())
    .bind(name)
    .bind(&spec.operation_id)
    .bind(spec.method.as_str())
    .bind(&spec.path)
    .bind(&spec.input_schema)
    .bind(&spec.output_schema)
    .bind(spec.enabled)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "api tool", name))?;
    tool_from_row(&row)
}

pub async fn latest_spec_version(
    pool: &PgPool,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<Option<SpecVersion>> {
    let row = sqlx::query(&format!(
        "SELECT {SPEC_COLUMNS} FROM spec_versions \
         WHERE team_id = $1 AND api_definition_id = $2 ORDER BY version DESC LIMIT 1"
    ))
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("latest spec version: {e}")))?;
    row.as_ref().map(spec_from_row).transpose()
}

pub async fn count_api_tools(
    pool: &PgPool,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM api_tools WHERE team_id = $1 AND api_definition_id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count api tools: {e}")))
}

pub async fn count_route_bindings(
    pool: &PgPool,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM api_route_bindings WHERE team_id = $1 AND api_definition_id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count api route bindings: {e}")))
}

pub async fn list_route_bindings_for_api(
    pool: &PgPool,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<Vec<ApiRouteBinding>> {
    let rows = sqlx::query(&format!(
        "SELECT {BINDING_COLUMNS} FROM api_route_bindings \
         WHERE team_id = $1 AND api_definition_id = $2 ORDER BY name"
    ))
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list api route bindings: {e}")))?;
    Ok(rows.iter().map(binding_from_row).collect())
}

pub async fn list_api_tools(
    pool: &PgPool,
    team_id: TeamId,
    api_id: ApiDefinitionId,
) -> DomainResult<Vec<ApiTool>> {
    let rows = sqlx::query(&format!(
        "SELECT {TOOL_COLUMNS} FROM api_tools \
         WHERE team_id = $1 AND api_definition_id = $2 ORDER BY name"
    ))
    .bind(team_id.as_uuid())
    .bind(api_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list api tools: {e}")))?;
    rows.iter().map(tool_from_row).collect()
}

pub async fn create_retention_policy(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &RetentionPolicySpec,
) -> DomainResult<RetentionPolicy> {
    validate_api_name(name)?;
    spec.validate()?;
    if let Some(api_id) = spec.api_definition_id {
        ensure_api_in_team(tx, team.id, api_id).await?;
    }
    let row = sqlx::query(&format!(
        "INSERT INTO api_retention_policies \
         (id, team_id, org_id, api_definition_id, name, raw_observation_ttl_days, max_spec_versions) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {RETENTION_COLUMNS}"
    ))
    .bind(RetentionPolicyId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(spec.api_definition_id.map(|id| id.as_uuid()))
    .bind(name)
    .bind(spec.raw_observation_ttl_days)
    .bind(spec.max_spec_versions)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "api retention policy", name))?;
    Ok(retention_from_row(&row))
}

pub async fn create_capture_session(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &CaptureSessionSpec,
) -> DomainResult<CaptureSession> {
    validate_api_name(name)?;
    spec.validate()?;
    if let Some(api_id) = spec.api_definition_id {
        ensure_api_in_team(tx, team.id, api_id).await?;
    }
    if let Some(route_config_id) = spec.route_config_id {
        ensure_capture_route_scope_in_team(tx, team.id, route_config_id, spec.listener_id).await?;
    }
    let row = sqlx::query(&format!(
        "INSERT INTO capture_sessions \
         (id, team_id, org_id, name, status, api_definition_id, route_config_id, listener_id, \
          virtual_host, route, target_sample_count, max_duration_seconds, max_bytes, \
          max_distinct_paths) \
         VALUES ($1, $2, $3, $4, 'capturing', $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         RETURNING {CAPTURE_SESSION_COLUMNS}"
    ))
    .bind(CaptureSessionId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec.api_definition_id.map(|id| id.as_uuid()))
    .bind(spec.route_config_id.map(|id| id.as_uuid()))
    .bind(spec.listener_id.map(|id| id.as_uuid()))
    .bind(&spec.virtual_host)
    .bind(&spec.route)
    .bind(spec.target_sample_count)
    .bind(spec.max_duration_seconds)
    .bind(spec.max_bytes)
    .bind(spec.max_distinct_paths)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "learning session", name))?;
    capture_session_from_row(&row)
}

pub async fn get_capture_session(
    pool: &PgPool,
    team_id: TeamId,
    handle: &str,
) -> DomainResult<Option<CaptureSession>> {
    let id = Uuid::parse_str(handle).ok();
    let row = sqlx::query(&format!(
        "SELECT {CAPTURE_SESSION_COLUMNS} FROM capture_sessions \
         WHERE team_id = $1 AND (name = $2 OR id = $3)"
    ))
    .bind(team_id.as_uuid())
    .bind(handle)
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get learning session: {e}")))?;
    row.as_ref().map(capture_session_from_row).transpose()
}

pub async fn list_capture_sessions(
    pool: &PgPool,
    team_id: TeamId,
    status: Option<CaptureSessionStatus>,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<CaptureSession>, i64)> {
    let status = status.map(|s| s.as_str().to_string());
    let rows = sqlx::query(&format!(
        "SELECT {CAPTURE_SESSION_COLUMNS} FROM capture_sessions \
         WHERE team_id = $1 AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC, name LIMIT $3 OFFSET $4"
    ))
    .bind(team_id.as_uuid())
    .bind(status.as_deref())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list learning sessions: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM capture_sessions \
         WHERE team_id = $1 AND ($2::text IS NULL OR status = $2)",
    )
    .bind(team_id.as_uuid())
    .bind(status.as_deref())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count learning sessions: {e}")))?;
    Ok((
        rows.iter()
            .map(capture_session_from_row)
            .collect::<DomainResult<Vec<_>>>()?,
        total,
    ))
}

pub async fn list_capturing_capture_sessions(
    pool: &PgPool,
    team_id: TeamId,
) -> DomainResult<Vec<CaptureSession>> {
    let rows = sqlx::query(&format!(
        "SELECT {CAPTURE_SESSION_COLUMNS} FROM capture_sessions \
         WHERE team_id = $1 AND status = 'capturing' ORDER BY created_at, name"
    ))
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list active learning sessions: {e}")))?;
    rows.iter().map(capture_session_from_row).collect()
}

pub async fn transition_capture_session(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    handle: &str,
    status: CaptureSessionStatus,
) -> DomainResult<CaptureSession> {
    let current = get_capture_session_for_update(tx, team_id, handle).await?;
    if current.status.terminal() {
        return Err(DomainError::conflict(format!(
            "learning session \"{}\" is already {}",
            current.name,
            current.status.as_str()
        ))
        .with_hint("start a new learning session for additional capture"));
    }
    if current.status == status {
        return Ok(current);
    }
    let row = sqlx::query(&format!(
        "UPDATE capture_sessions SET \
            status = $3, \
            completed_at = CASE WHEN $3 = 'completed' THEN now() ELSE completed_at END, \
            cancelled_at = CASE WHEN $3 = 'cancelled' THEN now() ELSE cancelled_at END, \
            updated_at = now() \
         WHERE team_id = $1 AND id = $2 RETURNING {CAPTURE_SESSION_COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(current.id.as_uuid())
    .bind(status.as_str())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("transition learning session: {e}")))?;
    capture_session_from_row(&row)
}

pub async fn validate_capture_ingest_binding(
    pool: &PgPool,
    team_id: TeamId,
    session_id: CaptureSessionId,
    api_definition_id: Option<ApiDefinitionId>,
    route_config_id: RouteConfigId,
    listener_id: Option<ListenerId>,
) -> DomainResult<CaptureSession> {
    let session = get_capture_session(pool, team_id, &session_id.to_string())
        .await?
        .ok_or_else(|| DomainError::not_found("learning session", &session_id.to_string()))?;
    if session.status != CaptureSessionStatus::Capturing {
        return Err(DomainError::conflict(format!(
            "learning session \"{}\" is {}",
            session.name,
            session.status.as_str()
        ))
        .with_hint("only capturing sessions can accept observations"));
    }
    if let Some(session_api_id) = session.api_definition_id {
        if api_definition_id != Some(session_api_id) {
            return Err(DomainError::not_found(
                "learning session binding",
                &session_id.to_string(),
            ));
        }
        let binding_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM api_route_bindings \
             WHERE team_id = $1 AND api_definition_id = $2 AND route_config_id = $3 \
             AND (listener_id IS NULL OR listener_id = $4))",
        )
        .bind(team_id.as_uuid())
        .bind(session_api_id.as_uuid())
        .bind(route_config_id.as_uuid())
        .bind(listener_id.map(|id| id.as_uuid()))
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("validate learning api binding: {e}")))?;
        if !binding_exists {
            return Err(DomainError::not_found(
                "learning session binding",
                &session_id.to_string(),
            ));
        }
        return Ok(session);
    }
    if session.route_config_id != Some(route_config_id) {
        return Err(DomainError::not_found(
            "learning session binding",
            &session_id.to_string(),
        ));
    }
    if session.listener_id.is_some() && session.listener_id != listener_id {
        return Err(DomainError::not_found(
            "learning session binding",
            &session_id.to_string(),
        ));
    }
    Ok(session)
}

pub async fn ingest_raw_observation(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    session_id: CaptureSessionId,
    api_definition_id: Option<ApiDefinitionId>,
    route_config_id: RouteConfigId,
    listener_id: Option<ListenerId>,
    input: &ObservationIngest,
) -> DomainResult<RawObservation> {
    input.validate()?;
    let session = get_capture_session_for_update(tx, team.id, &session_id.to_string()).await?;
    let existing =
        get_raw_observation_for_update(tx, team.id, session.id, &input.request_id).await?;
    validate_locked_capture_ingest_binding(
        tx,
        &session,
        team.id,
        api_definition_id,
        route_config_id,
        listener_id,
        existing.is_some(),
    )
    .await?;
    if existing.is_none() {
        reject_expired_session(tx, &session).await?;
    }
    if let Some(existing) = &existing {
        if existing.method != input.method || existing.path != input.path {
            return Err(DomainError::conflict(
                "observation request_id was already captured with different request metadata",
            )
            .with_hint("use a stable unique request id for each proxied request"));
        }
    } else if session.sample_count >= i64::from(session.target_sample_count) {
        increment_capture_drop_count(tx, team.id, session.id).await?;
        return Err(DomainError::new(
            ErrorCode::QuotaExceeded,
            "learning session has reached its target sample count",
        )
        .with_hint("start a new learning session for additional samples"));
    }

    let incoming_request_headers = sanitize_headers(&input.request_headers);
    let incoming_response_headers = sanitize_headers(&input.response_headers);
    let merged = match existing {
        Some(existing) => RawObservation {
            response_status: input.response_status.or(existing.response_status),
            request_headers: merge_headers(existing.request_headers, incoming_request_headers),
            response_headers: merge_headers(existing.response_headers, incoming_response_headers),
            request_body: input
                .request_body
                .clone()
                .or_else(|| existing.request_body.clone()),
            response_body: input
                .response_body
                .clone()
                .or_else(|| existing.response_body.clone()),
            request_body_truncated: existing.request_body_truncated || input.request_body_truncated,
            response_body_truncated: existing.response_body_truncated
                || input.response_body_truncated,
            metadata_seen: existing.metadata_seen || input.metadata_seen,
            body_seen: existing.body_seen || input.body_seen,
            observed_at: existing.observed_at.min(input.observed_at),
            ..existing
        },
        None => RawObservation {
            id: RawObservationId::generate(),
            team_id: team.id,
            capture_session_id: session.id,
            request_id: input.request_id.clone(),
            method: input.method.clone(),
            path: input.path.clone(),
            response_status: input.response_status,
            request_headers: incoming_request_headers,
            response_headers: incoming_response_headers,
            request_body: input.request_body.clone(),
            response_body: input.response_body.clone(),
            request_body_truncated: input.request_body_truncated,
            response_body_truncated: input.response_body_truncated,
            request_body_bytes: 0,
            response_body_bytes: 0,
            metadata_seen: input.metadata_seen,
            body_seen: input.body_seen,
            observed_at: input.observed_at,
            updated_at: input.observed_at,
            created_at: input.observed_at,
        },
    };
    let merged_request_bytes = body_bytes(merged.request_body.as_deref());
    let merged_response_bytes = body_bytes(merged.response_body.as_deref());
    let existing = existing_row_exists(tx, team.id, session.id, &input.request_id).await?;
    enforce_observation_quotas(
        tx,
        &session,
        existing,
        &merged.path,
        merged_request_bytes + merged_response_bytes,
        &input.request_id,
    )
    .await?;

    let ttl_days = raw_observation_ttl_days(tx, team.id, api_definition_id).await?;
    let row = sqlx::query(&format!(
        "INSERT INTO raw_observations \
         (id, team_id, org_id, capture_session_id, request_id, method, path, response_status, \
          request_headers, response_headers, request_body, response_body, \
          request_body_truncated, response_body_truncated, request_body_bytes, \
          response_body_bytes, metadata_seen, body_seen, observed_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, \
                 $16, $17, $18, $19, now() + make_interval(days => $20)) \
         ON CONFLICT (team_id, capture_session_id, request_id) DO UPDATE SET \
            response_status = EXCLUDED.response_status, \
            request_headers = EXCLUDED.request_headers, \
            response_headers = EXCLUDED.response_headers, \
            request_body = EXCLUDED.request_body, \
            response_body = EXCLUDED.response_body, \
            request_body_truncated = EXCLUDED.request_body_truncated, \
            response_body_truncated = EXCLUDED.response_body_truncated, \
            request_body_bytes = EXCLUDED.request_body_bytes, \
            response_body_bytes = EXCLUDED.response_body_bytes, \
            metadata_seen = EXCLUDED.metadata_seen, \
            body_seen = EXCLUDED.body_seen, \
            observed_at = LEAST(raw_observations.observed_at, EXCLUDED.observed_at), \
            updated_at = now() \
         RETURNING {RAW_OBSERVATION_COLUMNS}"
    ))
    .bind(merged.id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(session.id.as_uuid())
    .bind(&merged.request_id)
    .bind(&merged.method)
    .bind(&merged.path)
    .bind(merged.response_status)
    .bind(&merged.request_headers)
    .bind(&merged.response_headers)
    .bind(&merged.request_body)
    .bind(&merged.response_body)
    .bind(merged.request_body_truncated)
    .bind(merged.response_body_truncated)
    .bind(merged_request_bytes)
    .bind(merged_response_bytes)
    .bind(merged.metadata_seen)
    .bind(merged.body_seen)
    .bind(merged.observed_at)
    .bind(ttl_days)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("ingest raw observation: {e}")))?;
    recalculate_capture_counters(tx, team.id, session.id).await?;
    Ok(raw_observation_from_row(&row))
}

async fn get_capture_session_for_update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    handle: &str,
) -> DomainResult<CaptureSession> {
    let id = Uuid::parse_str(handle).ok();
    let row = sqlx::query(&format!(
        "SELECT {CAPTURE_SESSION_COLUMNS} FROM capture_sessions \
         WHERE team_id = $1 AND (name = $2 OR id = $3) FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(handle)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock learning session: {e}")))?;
    row.as_ref()
        .map(capture_session_from_row)
        .transpose()?
        .ok_or_else(|| DomainError::not_found("learning session", handle))
}

async fn validate_locked_capture_ingest_binding(
    tx: &mut Transaction<'_, Postgres>,
    session: &CaptureSession,
    team_id: TeamId,
    api_definition_id: Option<ApiDefinitionId>,
    route_config_id: RouteConfigId,
    listener_id: Option<ListenerId>,
    allow_existing_observation_merge: bool,
) -> DomainResult<()> {
    if session.status != CaptureSessionStatus::Capturing && !allow_existing_observation_merge {
        return Err(DomainError::conflict(format!(
            "learning session \"{}\" is {}",
            session.name,
            session.status.as_str()
        ))
        .with_hint("only capturing sessions can accept observations"));
    }
    if let Some(session_api_id) = session.api_definition_id {
        if api_definition_id != Some(session_api_id) {
            return Err(DomainError::not_found(
                "learning session binding",
                &session.id.to_string(),
            ));
        }
        let binding_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM api_route_bindings \
             WHERE team_id = $1 AND api_definition_id = $2 AND route_config_id = $3 \
             AND (listener_id IS NULL OR listener_id = $4))",
        )
        .bind(team_id.as_uuid())
        .bind(session_api_id.as_uuid())
        .bind(route_config_id.as_uuid())
        .bind(listener_id.map(|id| id.as_uuid()))
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("validate learning api binding: {e}")))?;
        if !binding_exists {
            return Err(DomainError::not_found(
                "learning session binding",
                &session.id.to_string(),
            ));
        }
        return Ok(());
    }
    if session.route_config_id != Some(route_config_id) {
        return Err(DomainError::not_found(
            "learning session binding",
            &session.id.to_string(),
        ));
    }
    if session.listener_id.is_some() && session.listener_id != listener_id {
        return Err(DomainError::not_found(
            "learning session binding",
            &session.id.to_string(),
        ));
    }
    Ok(())
}

async fn reject_expired_session(
    tx: &mut Transaction<'_, Postgres>,
    session: &CaptureSession,
) -> DomainResult<()> {
    if session.max_duration_seconds.is_none() {
        return Ok(());
    }
    let expired: Option<Uuid> = sqlx::query_scalar(
        "UPDATE capture_sessions SET status = 'completed', completed_at = COALESCE(completed_at, now()), \
         drop_count = drop_count + 1, updated_at = now() \
         WHERE team_id = $1 AND id = $2 \
           AND max_duration_seconds IS NOT NULL \
           AND now() > started_at + (max_duration_seconds * interval '1 second') \
         RETURNING id",
    )
    .bind(session.team_id.as_uuid())
    .bind(session.id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("expire learning session: {e}")))?;
    if expired.is_none() {
        return Ok(());
    }
    Err(DomainError::conflict(format!(
        "learning session \"{}\" has reached its max duration",
        session.name
    ))
    .with_hint("start a new learning session for additional samples"))
}

async fn get_raw_observation_for_update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: CaptureSessionId,
    request_id: &str,
) -> DomainResult<Option<RawObservation>> {
    let row = sqlx::query(&format!(
        "SELECT {RAW_OBSERVATION_COLUMNS} FROM raw_observations \
         WHERE team_id = $1 AND capture_session_id = $2 AND request_id = $3 FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock raw observation: {e}")))?;
    Ok(row.as_ref().map(raw_observation_from_row))
}

async fn existing_row_exists(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: CaptureSessionId,
    request_id: &str,
) -> DomainResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM raw_observations \
         WHERE team_id = $1 AND capture_session_id = $2 AND request_id = $3)",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(request_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("check raw observation existence: {e}")))
}

async fn raw_observation_ttl_days(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    api_definition_id: Option<ApiDefinitionId>,
) -> DomainResult<i32> {
    let ttl_days: Option<i32> = sqlx::query_scalar(
        "SELECT raw_observation_ttl_days FROM api_retention_policies \
         WHERE team_id = $1 AND (api_definition_id = $2 OR api_definition_id IS NULL) \
         ORDER BY api_definition_id IS NULL, created_at DESC LIMIT 1",
    )
    .bind(team_id.as_uuid())
    .bind(api_definition_id.map(|id| id.as_uuid()))
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("resolve raw observation ttl: {e}")))?;
    Ok(ttl_days.unwrap_or(DEFAULT_RAW_OBSERVATION_TTL_DAYS))
}

async fn enforce_observation_quotas(
    tx: &mut Transaction<'_, Postgres>,
    session: &CaptureSession,
    existing: bool,
    path: &str,
    merged_body_bytes: i64,
    request_id: &str,
) -> DomainResult<()> {
    let (sample_count_without_request, bytes_without_request, path_already_present): (i64, i64, bool) =
        sqlx::query_as(
            "SELECT \
                count(*) FILTER (WHERE request_id <> $3), \
                COALESCE(sum(request_body_bytes + response_body_bytes) FILTER (WHERE request_id <> $3), 0)::bigint, \
                EXISTS(SELECT 1 FROM raw_observations \
                    WHERE team_id = $1 AND capture_session_id = $2 AND path = $4 AND request_id <> $3) \
             FROM raw_observations WHERE team_id = $1 AND capture_session_id = $2",
        )
        .bind(session.team_id.as_uuid())
        .bind(session.id.as_uuid())
        .bind(request_id)
        .bind(path)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("count raw observations for quota: {e}")))?;

    let next_sample_count = sample_count_without_request + 1;
    let next_byte_count = bytes_without_request + merged_body_bytes;
    if !existing && next_sample_count > i64::from(session.target_sample_count) {
        increment_capture_drop_count(tx, session.team_id, session.id).await?;
        return Err(DomainError::new(
            ErrorCode::QuotaExceeded,
            "learning session has reached its target sample count",
        )
        .with_hint("start a new learning session for additional samples"));
    }
    if next_byte_count > session.max_bytes {
        increment_capture_drop_count(tx, session.team_id, session.id).await?;
        return Err(DomainError::new(
            ErrorCode::QuotaExceeded,
            "learning session has reached its raw observation byte limit",
        )
        .with_hint("raise max_bytes or start a narrower learning session"));
    }
    if !existing
        && !path_already_present
        && session.path_count + 1 > i64::from(session.max_distinct_paths)
    {
        increment_capture_drop_count(tx, session.team_id, session.id).await?;
        return Err(DomainError::new(
            ErrorCode::QuotaExceeded,
            "learning session has reached its distinct path limit",
        )
        .with_hint("raise max_distinct_paths or scope capture to fewer routes"));
    }
    Ok(())
}

async fn increment_capture_drop_count(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: CaptureSessionId,
) -> DomainResult<()> {
    sqlx::query(
        "UPDATE capture_sessions SET drop_count = drop_count + 1, updated_at = now() \
         WHERE team_id = $1 AND id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("increment learning drop count: {e}")))?;
    Ok(())
}

async fn recalculate_capture_counters(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: CaptureSessionId,
) -> DomainResult<()> {
    sqlx::query(
        "WITH counts AS ( \
            SELECT count(*)::bigint AS sample_count, \
                   COALESCE(sum(request_body_bytes + response_body_bytes), 0)::bigint AS byte_count, \
                   count(DISTINCT path)::bigint AS path_count \
            FROM raw_observations WHERE team_id = $1 AND capture_session_id = $2 \
         ) \
         UPDATE capture_sessions SET \
            sample_count = counts.sample_count, \
            byte_count = counts.byte_count, \
            path_count = counts.path_count, \
            status = CASE \
                WHEN status = 'capturing' AND counts.sample_count >= target_sample_count \
                    THEN 'completed' \
                ELSE status \
            END, \
            completed_at = CASE \
                WHEN status = 'capturing' AND counts.sample_count >= target_sample_count \
                    THEN COALESCE(completed_at, now()) \
                ELSE completed_at \
            END, \
            updated_at = now() \
         FROM counts WHERE capture_sessions.team_id = $1 AND capture_sessions.id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("recalculate learning counters: {e}")))?;
    Ok(())
}
