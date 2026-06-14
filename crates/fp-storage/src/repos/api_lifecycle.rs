//! API lifecycle repositories (S8/D-017).
//!
//! This is intentionally storage-level for S8.1. REST/CLI, OpenAPI parsing, capture
//! sessions, and tool serving build on these records in later S8/S11 slices.

use fp_domain::api_lifecycle::{
    validate_api_name, ApiDefinition, ApiDefinitionSpec, ApiRouteBinding, ApiRouteBindingSpec,
    ApiTool, ApiToolSpec, CaptureSession, CaptureSessionSpec, CaptureSessionStatus, HttpMethod,
    RetentionPolicy, RetentionPolicySpec, SpecFormat, SpecSourceKind, SpecVersion,
    SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::{
    ApiDefinitionId, ApiRouteBindingId, ApiToolId, CaptureSessionId, DomainError, DomainResult,
    ErrorCode, ListenerId, RetentionPolicyId, RouteConfigId, SpecVersionId, TeamId,
};
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

fn canonical_hash(value: &serde_json::Value) -> DomainResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| DomainError::internal(format!("serialize api spec for hash: {e}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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
