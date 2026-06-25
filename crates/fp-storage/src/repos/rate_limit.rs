//! Rate-limit repositories (feature fpv2-4ht, slice S1).
//!
//! Every tenant query carries the team predicate in SQL (spec/10 §4) via [`TeamScope`].
//! These tables use SOFT DELETE: `delete_*` writes a `deleted_at` tombstone, and every read
//! filters `deleted_at IS NULL`, so a deleted name can be reused. Mutations require the
//! expected `version` — optimistic concurrency on every mutable row (spec/10 §3.4.4).
//!
//! `descriptors_canonical` is computed by the pure `fp_domain` function, not here — storage
//! only writes what the domain layer canonicalized.

use crate::scope::TeamScope;
use fp_domain::authz::TeamRef;
use fp_domain::rate_limit::{
    RateLimitDomain, RateLimitPolicy, RateLimitPolicySpec, RateLimitTeamOverride,
    RateLimitTeamOverrideSpec, RateLimitUnit,
};
use fp_domain::{
    DomainError, DomainResult, ErrorCode, OrgId, RateLimitDomainId, RateLimitPolicyId,
    RateLimitTeamOverrideId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use std::str::FromStr;
use uuid::Uuid;

fn require_team(scope: TeamScope, what: &str) -> DomainResult<TeamId> {
    scope.team_id().ok_or_else(|| {
        DomainError::internal(format!(
            "platform-admin {what} reads are not a supported path (tenant resource)"
        ))
    })
}

/// A live (not soft-deleted) domain in this team must exist, else the caller is referencing a
/// tombstoned or absent parent. Returns NotFound (cross-tenant absence is indistinguishable).
async fn require_live_domain(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    domain_id: RateLimitDomainId,
) -> DomainResult<()> {
    let exists: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rate_limit_domains WHERE id = $1 AND team_id = $2 AND deleted_at IS NULL",
    )
    .bind(domain_id.as_uuid())
    .bind(team_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("check rate-limit domain liveness: {e}")))?;
    if exists.is_none() {
        return Err(DomainError::not_found(
            "rate-limit domain",
            &domain_id.to_string(),
        ));
    }
    Ok(())
}

/// A live (not soft-deleted) policy in this team must exist (same reasoning as
/// [`require_live_domain`]).
async fn require_live_policy(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    policy_id: RateLimitPolicyId,
) -> DomainResult<()> {
    let exists: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rate_limit_policies WHERE id = $1 AND team_id = $2 AND deleted_at IS NULL",
    )
    .bind(policy_id.as_uuid())
    .bind(team_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("check rate-limit policy liveness: {e}")))?;
    if exists.is_none() {
        return Err(DomainError::not_found(
            "rate-limit policy",
            &policy_id.to_string(),
        ));
    }
    Ok(())
}

// ---- Domains ----------------------------------------------------------------------------

const DOMAIN_COLUMNS: &str = "id, team_id, name, version, created_at, updated_at";

fn domain_from_row(row: &PgRow) -> DomainResult<RateLimitDomain> {
    Ok(RateLimitDomain {
        id: RateLimitDomainId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn create_domain(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
) -> DomainResult<RateLimitDomain> {
    let row = sqlx::query(&format!(
        "INSERT INTO rate_limit_domains (id, team_id, org_id, name) \
         VALUES ($1, $2, $3, $4) RETURNING {DOMAIN_COLUMNS}"
    ))
    .bind(RateLimitDomainId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| dup_or_internal(&e, "rate-limit domain", name, "create rate-limit domain"))?;
    domain_from_row(&row)
}

pub async fn get_domain(
    pool: &PgPool,
    scope: TeamScope,
    name: &str,
) -> DomainResult<Option<RateLimitDomain>> {
    let team_id = require_team(scope, "rate-limit domain")?;
    let row = sqlx::query(&format!(
        "SELECT {DOMAIN_COLUMNS} FROM rate_limit_domains \
         WHERE team_id = $1 AND name = $2 AND deleted_at IS NULL"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get rate-limit domain: {e}")))?;
    row.as_ref().map(domain_from_row).transpose()
}

pub async fn list_domains(
    pool: &PgPool,
    scope: TeamScope,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<RateLimitDomain>, i64)> {
    let team_id = require_team(scope, "rate-limit domain")?;
    let rows = sqlx::query(&format!(
        "SELECT {DOMAIN_COLUMNS} FROM rate_limit_domains \
         WHERE team_id = $1 AND deleted_at IS NULL ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list rate-limit domains: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rate_limit_domains WHERE team_id = $1 AND deleted_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count rate-limit domains: {e}")))?;
    rows.iter()
        .map(domain_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

/// Rename a domain (its only mutable field), with optimistic concurrency.
pub async fn update_domain(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    new_name: &str,
    expected_version: i64,
) -> DomainResult<RateLimitDomain> {
    let row = sqlx::query(&format!(
        "UPDATE rate_limit_domains SET name = $1, version = version + 1, updated_at = now() \
         WHERE team_id = $2 AND name = $3 AND version = $4 AND deleted_at IS NULL \
         RETURNING {DOMAIN_COLUMNS}"
    ))
    .bind(new_name)
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| {
        dup_or_internal(
            &e,
            "rate-limit domain",
            new_name,
            "update rate-limit domain",
        )
    })?;
    match row {
        Some(row) => domain_from_row(&row),
        None => Err(domain_revision_error(tx, team_id, name, expected_version).await),
    }
}

pub async fn delete_domain(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<RateLimitDomainId> {
    let row = sqlx::query(
        "UPDATE rate_limit_domains SET deleted_at = now(), version = version + 1, updated_at = now() \
         WHERE team_id = $1 AND name = $2 AND version = $3 AND deleted_at IS NULL RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete rate-limit domain: {e}")))?;
    match row {
        Some(row) => {
            let domain_id = RateLimitDomainId::from(row.get::<Uuid, _>("id"));
            // Cascade the tombstone: a soft-deleted domain must leave no live policies or
            // overrides behind — otherwise a known domain_id could still list/create children,
            // and sync/enforcement (S5/S4) would push orphaned rows. The hard-delete FK CASCADE
            // does not fire on a soft delete, so we tombstone children explicitly, in the same
            // tx. Overrides first (they reference policies), then the policies themselves.
            sqlx::query(
                "UPDATE rate_limit_team_overrides \
                 SET deleted_at = now(), version = version + 1, updated_at = now() \
                 WHERE team_id = $1 AND deleted_at IS NULL AND policy_id IN \
                   (SELECT id FROM rate_limit_policies WHERE team_id = $1 AND domain_id = $2)",
            )
            .bind(team_id.as_uuid())
            .bind(domain_id.as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                DomainError::internal(format!("cascade delete rate-limit overrides: {e}"))
            })?;
            sqlx::query(
                "UPDATE rate_limit_policies \
                 SET deleted_at = now(), version = version + 1, updated_at = now() \
                 WHERE team_id = $1 AND domain_id = $2 AND deleted_at IS NULL",
            )
            .bind(team_id.as_uuid())
            .bind(domain_id.as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                DomainError::internal(format!("cascade delete rate-limit policies: {e}"))
            })?;
            Ok(domain_id)
        }
        None => Err(domain_revision_error(tx, team_id, name, expected_version).await),
    }
}

async fn domain_revision_error(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainError {
    let current: Result<Option<i64>, _> = sqlx::query_scalar(
        "SELECT version FROM rate_limit_domains WHERE team_id = $1 AND name = $2 AND deleted_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await;
    match current {
        Ok(Some(version)) => {
            revision_mismatch("rate-limit domain", name, version, expected_version)
        }
        Ok(None) => DomainError::not_found("rate-limit domain", name),
        Err(e) => DomainError::internal(format!("rate-limit domain revision recheck: {e}")),
    }
}

// ---- Policies ---------------------------------------------------------------------------

const POLICY_COLUMNS: &str = "id, team_id, domain_id, name, descriptors, descriptors_canonical, \
    requests_per_unit, unit, version, created_at, updated_at";

fn policy_from_row(row: &PgRow) -> DomainResult<RateLimitPolicy> {
    let descriptors: serde_json::Value = row.get("descriptors");
    let descriptors: BTreeMap<String, String> =
        serde_json::from_value(descriptors).map_err(|e| {
            DomainError::internal(format!("rate-limit descriptors in DB do not parse: {e}"))
        })?;
    let requests_per_unit: i64 = row.get("requests_per_unit");
    let unit: String = row.get("unit");
    Ok(RateLimitPolicy {
        id: RateLimitPolicyId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        domain_id: RateLimitDomainId::from(row.get::<Uuid, _>("domain_id")),
        name: row.get("name"),
        spec: RateLimitPolicySpec {
            descriptors,
            requests_per_unit: u64::try_from(requests_per_unit).map_err(|_| {
                DomainError::internal("rate-limit requests_per_unit in DB is negative")
            })?,
            unit: RateLimitUnit::from_str(&unit).map_err(|e| {
                DomainError::internal(format!("rate-limit unit in DB invalid: {e}"))
            })?,
        },
        descriptors_canonical: row.get("descriptors_canonical"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn limit_to_i64(value: u64) -> DomainResult<i64> {
    i64::try_from(value)
        .map_err(|_| DomainError::validation("rate-limit requests_per_unit exceeds storage range"))
}

pub async fn create_policy(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    domain_id: RateLimitDomainId,
    name: &str,
    spec: &RateLimitPolicySpec,
) -> DomainResult<RateLimitPolicy> {
    // Reject creating a policy under a soft-deleted (or absent) domain: the composite FK still
    // points at the tombstoned row, so without this a known domain_id could spawn orphans.
    require_live_domain(tx, team.id, domain_id).await?;
    let descriptors_json = serde_json::to_value(&spec.descriptors)
        .map_err(|e| DomainError::internal(format!("serialize rate-limit descriptors: {e}")))?;
    let canonical = spec.descriptors_canonical();
    let row = sqlx::query(&format!(
        "INSERT INTO rate_limit_policies \
           (id, team_id, org_id, domain_id, name, descriptors, descriptors_canonical, \
            requests_per_unit, unit) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {POLICY_COLUMNS}"
    ))
    .bind(RateLimitPolicyId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(name)
    .bind(descriptors_json)
    .bind(&canonical)
    .bind(limit_to_i64(spec.requests_per_unit)?)
    .bind(spec.unit.as_str())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| policy_dup_or_internal(&e, name))?;
    policy_from_row(&row)
}

pub async fn get_policy(
    pool: &PgPool,
    scope: TeamScope,
    domain_id: RateLimitDomainId,
    name: &str,
) -> DomainResult<Option<RateLimitPolicy>> {
    let team_id = require_team(scope, "rate-limit policy")?;
    let row = sqlx::query(&format!(
        "SELECT {POLICY_COLUMNS} FROM rate_limit_policies \
         WHERE team_id = $1 AND domain_id = $2 AND name = $3 AND deleted_at IS NULL"
    ))
    .bind(team_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get rate-limit policy: {e}")))?;
    row.as_ref().map(policy_from_row).transpose()
}

pub async fn list_policies(
    pool: &PgPool,
    scope: TeamScope,
    domain_id: RateLimitDomainId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<RateLimitPolicy>, i64)> {
    let team_id = require_team(scope, "rate-limit policy")?;
    let rows = sqlx::query(&format!(
        "SELECT {POLICY_COLUMNS} FROM rate_limit_policies \
         WHERE team_id = $1 AND domain_id = $2 AND deleted_at IS NULL \
         ORDER BY name LIMIT $3 OFFSET $4"
    ))
    .bind(team_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list rate-limit policies: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rate_limit_policies \
         WHERE team_id = $1 AND domain_id = $2 AND deleted_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .bind(domain_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count rate-limit policies: {e}")))?;
    rows.iter()
        .map(policy_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn update_policy(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    domain_id: RateLimitDomainId,
    name: &str,
    spec: &RateLimitPolicySpec,
    expected_version: i64,
) -> DomainResult<RateLimitPolicy> {
    let descriptors_json = serde_json::to_value(&spec.descriptors)
        .map_err(|e| DomainError::internal(format!("serialize rate-limit descriptors: {e}")))?;
    let canonical = spec.descriptors_canonical();
    let row = sqlx::query(&format!(
        "UPDATE rate_limit_policies \
         SET descriptors = $1, descriptors_canonical = $2, requests_per_unit = $3, unit = $4, \
             version = version + 1, updated_at = now() \
         WHERE team_id = $5 AND domain_id = $6 AND name = $7 AND version = $8 AND deleted_at IS NULL \
         RETURNING {POLICY_COLUMNS}"
    ))
    .bind(descriptors_json)
    .bind(&canonical)
    .bind(limit_to_i64(spec.requests_per_unit)?)
    .bind(spec.unit.as_str())
    .bind(team_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| policy_dup_or_internal(&e, name))?;
    match row {
        Some(row) => policy_from_row(&row),
        None => Err(policy_revision_error(tx, team_id, domain_id, name, expected_version).await),
    }
}

pub async fn delete_policy(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    domain_id: RateLimitDomainId,
    name: &str,
    expected_version: i64,
) -> DomainResult<RateLimitPolicyId> {
    let row = sqlx::query(
        "UPDATE rate_limit_policies SET deleted_at = now(), version = version + 1, updated_at = now() \
         WHERE team_id = $1 AND domain_id = $2 AND name = $3 AND version = $4 AND deleted_at IS NULL \
         RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete rate-limit policy: {e}")))?;
    match row {
        Some(row) => {
            let policy_id = RateLimitPolicyId::from(row.get::<Uuid, _>("id"));
            // Cascade to the policy's override (same reasoning as delete_domain).
            sqlx::query(
                "UPDATE rate_limit_team_overrides \
                 SET deleted_at = now(), version = version + 1, updated_at = now() \
                 WHERE team_id = $1 AND policy_id = $2 AND deleted_at IS NULL",
            )
            .bind(team_id.as_uuid())
            .bind(policy_id.as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                DomainError::internal(format!("cascade delete rate-limit override: {e}"))
            })?;
            Ok(policy_id)
        }
        None => Err(policy_revision_error(tx, team_id, domain_id, name, expected_version).await),
    }
}

/// Per-team policy count for quota enforcement (S2).
pub async fn count_policies_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM rate_limit_policies WHERE team_id = $1 AND deleted_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count rate-limit policies: {e}")))
}

async fn policy_revision_error(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    domain_id: RateLimitDomainId,
    name: &str,
    expected_version: i64,
) -> DomainError {
    let current: Result<Option<i64>, _> = sqlx::query_scalar(
        "SELECT version FROM rate_limit_policies \
         WHERE team_id = $1 AND domain_id = $2 AND name = $3 AND deleted_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await;
    match current {
        Ok(Some(version)) => {
            revision_mismatch("rate-limit policy", name, version, expected_version)
        }
        Ok(None) => DomainError::not_found("rate-limit policy", name),
        Err(e) => DomainError::internal(format!("rate-limit policy revision recheck: {e}")),
    }
}

// ---- Team overrides ---------------------------------------------------------------------

const OVERRIDE_COLUMNS: &str =
    "id, team_id, policy_id, requests_per_unit, version, created_at, updated_at";

fn override_from_row(row: &PgRow) -> DomainResult<RateLimitTeamOverride> {
    let requests_per_unit: i64 = row.get("requests_per_unit");
    Ok(RateLimitTeamOverride {
        id: RateLimitTeamOverrideId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        policy_id: RateLimitPolicyId::from(row.get::<Uuid, _>("policy_id")),
        spec: RateLimitTeamOverrideSpec {
            requests_per_unit: u64::try_from(requests_per_unit).map_err(|_| {
                DomainError::internal("rate-limit override requests_per_unit in DB is negative")
            })?,
        },
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn create_override(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    policy_id: RateLimitPolicyId,
    spec: &RateLimitTeamOverrideSpec,
) -> DomainResult<RateLimitTeamOverride> {
    // Reject creating an override under a soft-deleted (or absent) policy (same reasoning).
    require_live_policy(tx, team.id, policy_id).await?;
    let row = sqlx::query(&format!(
        "INSERT INTO rate_limit_team_overrides (id, team_id, org_id, policy_id, requests_per_unit) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {OVERRIDE_COLUMNS}"
    ))
    .bind(RateLimitTeamOverrideId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(policy_id.as_uuid())
    .bind(limit_to_i64(spec.requests_per_unit)?)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict("this policy already has a rate-limit override in this team")
                .with_hint("update the existing override instead of creating another")
        }
        _ => DomainError::internal(format!("create rate-limit override: {e}")),
    })?;
    override_from_row(&row)
}

pub async fn get_override(
    pool: &PgPool,
    scope: TeamScope,
    policy_id: RateLimitPolicyId,
) -> DomainResult<Option<RateLimitTeamOverride>> {
    let team_id = require_team(scope, "rate-limit override")?;
    let row = sqlx::query(&format!(
        "SELECT {OVERRIDE_COLUMNS} FROM rate_limit_team_overrides \
         WHERE team_id = $1 AND policy_id = $2 AND deleted_at IS NULL"
    ))
    .bind(team_id.as_uuid())
    .bind(policy_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get rate-limit override: {e}")))?;
    row.as_ref().map(override_from_row).transpose()
}

pub async fn update_override(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    policy_id: RateLimitPolicyId,
    spec: &RateLimitTeamOverrideSpec,
    expected_version: i64,
) -> DomainResult<RateLimitTeamOverride> {
    let row = sqlx::query(&format!(
        "UPDATE rate_limit_team_overrides \
         SET requests_per_unit = $1, version = version + 1, updated_at = now() \
         WHERE team_id = $2 AND policy_id = $3 AND version = $4 AND deleted_at IS NULL \
         RETURNING {OVERRIDE_COLUMNS}"
    ))
    .bind(limit_to_i64(spec.requests_per_unit)?)
    .bind(team_id.as_uuid())
    .bind(policy_id.as_uuid())
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update rate-limit override: {e}")))?;
    match row {
        Some(row) => override_from_row(&row),
        None => Err(override_revision_error(tx, team_id, policy_id, expected_version).await),
    }
}

pub async fn delete_override(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    policy_id: RateLimitPolicyId,
    expected_version: i64,
) -> DomainResult<RateLimitTeamOverrideId> {
    let row = sqlx::query(
        "UPDATE rate_limit_team_overrides SET deleted_at = now(), version = version + 1, updated_at = now() \
         WHERE team_id = $1 AND policy_id = $2 AND version = $3 AND deleted_at IS NULL RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(policy_id.as_uuid())
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete rate-limit override: {e}")))?;
    match row {
        Some(row) => Ok(RateLimitTeamOverrideId::from(row.get::<Uuid, _>("id"))),
        None => Err(override_revision_error(tx, team_id, policy_id, expected_version).await),
    }
}

async fn override_revision_error(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    policy_id: RateLimitPolicyId,
    expected_version: i64,
) -> DomainError {
    let current: Result<Option<i64>, _> = sqlx::query_scalar(
        "SELECT version FROM rate_limit_team_overrides \
         WHERE team_id = $1 AND policy_id = $2 AND deleted_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .bind(policy_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await;
    match current {
        Ok(Some(version)) => revision_mismatch(
            "rate-limit override",
            &policy_id.to_string(),
            version,
            expected_version,
        ),
        Ok(None) => DomainError::not_found("rate-limit override", &policy_id.to_string()),
        Err(e) => DomainError::internal(format!("rate-limit override revision recheck: {e}")),
    }
}

// ---- CP rls_sync reconcile read (PLATFORM-INTERNAL, cross-team) --------------------------

/// One effective policy row for the CP `rls_sync` worker (S5). The effective `requests_per_unit`
/// already folds in any team override.
pub struct SyncPolicyRow {
    pub org_id: OrgId,
    pub team_id: TeamId,
    pub domain: String,
    pub descriptors: BTreeMap<String, String>,
    pub requests_per_unit: u64,
    pub unit: RateLimitUnit,
}

/// Read every team's live policies (with overrides applied) for the reconcile push. This is the
/// one intentionally UNSCOPED read in this module: the sync worker is a platform-internal
/// component that pushes the full set to the RLS, which keys everything by the CP-composed
/// `{org|team|domain}` namespace. It is never reachable from a tenant request path.
pub async fn list_all_for_sync(pool: &PgPool) -> DomainResult<Vec<SyncPolicyRow>> {
    let rows = sqlx::query(
        "SELECT p.org_id, p.team_id, d.name AS domain, p.descriptors, \
                COALESCE(o.requests_per_unit, p.requests_per_unit) AS requests_per_unit, p.unit \
         FROM rate_limit_policies p \
         JOIN rate_limit_domains d ON d.id = p.domain_id AND d.deleted_at IS NULL \
         LEFT JOIN rate_limit_team_overrides o \
           ON o.policy_id = p.id AND o.team_id = p.team_id AND o.deleted_at IS NULL \
         WHERE p.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list rate-limit policies for sync: {e}")))?;
    rows.iter().map(sync_row_from).collect()
}

fn sync_row_from(row: &PgRow) -> DomainResult<SyncPolicyRow> {
    let descriptors: serde_json::Value = row.get("descriptors");
    let descriptors: BTreeMap<String, String> =
        serde_json::from_value(descriptors).map_err(|e| {
            DomainError::internal(format!("rate-limit descriptors in DB do not parse: {e}"))
        })?;
    let requests_per_unit: i64 = row.get("requests_per_unit");
    let unit: String = row.get("unit");
    Ok(SyncPolicyRow {
        org_id: OrgId::from(row.get::<Uuid, _>("org_id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        domain: row.get("domain"),
        descriptors,
        requests_per_unit: u64::try_from(requests_per_unit)
            .map_err(|_| DomainError::internal("rate-limit requests_per_unit in DB is negative"))?,
        unit: RateLimitUnit::from_str(&unit)
            .map_err(|e| DomainError::internal(format!("rate-limit unit in DB invalid: {e}")))?,
    })
}

// ---- Shared error helpers ---------------------------------------------------------------

fn revision_mismatch(kind: &str, handle: &str, current: i64, supplied: i64) -> DomainError {
    DomainError::new(
        ErrorCode::RevisionMismatch,
        format!("{kind} \"{handle}\" is at revision {current}, you supplied {supplied}"),
    )
    .with_hint("re-read the resource and retry with the current revision")
}

fn dup_or_internal(e: &sqlx::Error, kind: &str, handle: &str, ctx: &str) -> DomainError {
    match e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("{kind} \"{handle}\" already exists in this team"))
                .with_hint("choose a different name or update the existing resource")
        }
        _ => DomainError::internal(format!("{ctx}: {e}")),
    }
}

fn policy_dup_or_internal(e: &sqlx::Error, name: &str) -> DomainError {
    match e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            // Either the (domain, name) handle or the (domain, descriptors) match collides.
            DomainError::conflict(format!(
                "a rate-limit policy named \"{name}\" — or one with the same descriptor set — \
                 already exists in this domain"
            ))
            .with_hint(
                "use a different name, or update the policy that already matches these descriptors",
            )
        }
        _ => DomainError::internal(format!("create/update rate-limit policy: {e}")),
    }
}
