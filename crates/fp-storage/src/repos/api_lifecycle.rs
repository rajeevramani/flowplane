//! API lifecycle repositories (S8/D-017).
//!
//! This is intentionally storage-level for S8.1. REST/CLI, OpenAPI parsing, capture
//! sessions, and tool serving build on these records in later S8/S11 slices.

use fp_domain::api_lifecycle::{
    validate_api_name, ApiDefinition, ApiDefinitionSpec, ApiRouteBinding, ApiRouteBindingSpec,
    ApiTool, ApiToolSpec, HttpMethod, RetentionPolicy, RetentionPolicySpec, SpecFormat,
    SpecSourceKind, SpecVersion, SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::{
    ApiDefinitionId, ApiRouteBindingId, ApiToolId, DomainError, DomainResult, ListenerId,
    RetentionPolicyId, RouteConfigId, SpecVersionId, TeamId,
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
