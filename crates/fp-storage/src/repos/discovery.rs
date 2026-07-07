//! Discovery session repository.

use fp_domain::api_lifecycle::{ObservationIngest, RawObservation};
use fp_domain::authz::TeamRef;
use fp_domain::{
    DiscoveryObservation, DiscoveryObservationProvenance, DiscoverySession, DiscoverySessionId,
    DiscoverySessionSpec, DiscoverySessionStatus, DomainError, DomainResult, RawObservationId,
    TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::types::Uuid;
use sqlx::{PgPool, Postgres, Row, Transaction};

use super::observation_ingest::{
    decide_observation_quota, merge_headers, merged_body_bytes, sanitize_headers,
    ObservationQuotaChange, ObservationQuotaState,
};

const COLUMNS: &str = "id, team_id, name, status, listener_port, upstream_host, upstream_port, \
    upstream_tls, validated_upstream_ip, validated_upstream_port, cluster_name, \
    route_config_name, listener_name, target_sample_count, max_duration_seconds, max_bytes, \
    max_distinct_paths, sample_count, byte_count, path_count, drop_count, started_at, \
    completed_at, cancelled_at, updated_at, created_at";
const RAW_COLUMNS: &str = "ro.id, ro.team_id, ro.capture_session_id, ro.request_id, ro.method, ro.path, \
    ro.response_status, ro.request_headers, ro.response_headers, ro.request_body, ro.response_body, \
    ro.request_body_truncated, ro.response_body_truncated, ro.request_body_bytes, \
    ro.response_body_bytes, ro.metadata_seen, ro.body_seen, ro.observed_at, ro.updated_at, ro.created_at";
const DISCOVERY_RAW_COLUMNS: &str = "dro.discovery_session_id, dro.discovery_listener_id, \
    dro.observed_host, dro.observed_sni, dro.route_matched, dro.forwarded_upstream_host, \
    dro.forwarded_upstream_port, dro.forwarded_upstream_ip, dro.forwarded_upstream_tls";

pub struct DiscoverySessionInsert<'a> {
    pub id: DiscoverySessionId,
    pub name: &'a str,
    pub spec: &'a DiscoverySessionSpec,
    pub validated_upstream_ip: &'a str,
    pub cluster_name: &'a str,
    pub route_config_name: &'a str,
    pub listener_name: &'a str,
}

fn from_row(row: &PgRow) -> DomainResult<DiscoverySession> {
    let status: String = row.get("status");
    Ok(DiscoverySession {
        id: DiscoverySessionId::from(row.get::<uuid::Uuid, _>("id")),
        team_id: TeamId::from(row.get::<uuid::Uuid, _>("team_id")),
        name: row.get("name"),
        status: DiscoverySessionStatus::parse(&status)?,
        listener_port: row.get("listener_port"),
        upstream_host: row.get("upstream_host"),
        upstream_port: row.get("upstream_port"),
        upstream_tls: row.get("upstream_tls"),
        validated_upstream_ip: row.get("validated_upstream_ip"),
        validated_upstream_port: row.get("validated_upstream_port"),
        cluster_name: row.get("cluster_name"),
        route_config_name: row.get("route_config_name"),
        listener_name: row.get("listener_name"),
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

fn raw_from_row(row: &PgRow) -> RawObservation {
    RawObservation {
        id: RawObservationId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        capture_session_id: row
            .get::<Option<Uuid>, _>("capture_session_id")
            .map(fp_domain::CaptureSessionId::from),
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

fn observation_from_row(row: &PgRow) -> DiscoveryObservation {
    DiscoveryObservation {
        raw: raw_from_row(row),
        provenance: DiscoveryObservationProvenance {
            discovery_session_id: DiscoverySessionId::from(
                row.get::<Uuid, _>("discovery_session_id"),
            ),
            discovery_listener_id: fp_domain::ListenerId::from(
                row.get::<Uuid, _>("discovery_listener_id"),
            ),
            observed_host: row.get("observed_host"),
            observed_sni: row.get("observed_sni"),
            route_matched: row.get("route_matched"),
            forwarded_upstream_host: row.get("forwarded_upstream_host"),
            forwarded_upstream_port: row.get("forwarded_upstream_port"),
            forwarded_upstream_ip: row.get("forwarded_upstream_ip"),
            forwarded_upstream_tls: row.get("forwarded_upstream_tls"),
        },
    }
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    insert: DiscoverySessionInsert<'_>,
) -> DomainResult<DiscoverySession> {
    let row = sqlx::query(&format!(
        "INSERT INTO discovery_sessions \
         (id, team_id, org_id, name, status, listener_port, upstream_host, upstream_port, \
          upstream_tls, validated_upstream_ip, validated_upstream_port, cluster_name, \
          route_config_name, listener_name, target_sample_count, max_duration_seconds, \
          max_bytes, max_distinct_paths) \
         VALUES ($1, $2, $3, $4, 'capturing', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
         RETURNING {COLUMNS}"
    ))
    .bind(insert.id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(insert.name)
    .bind(insert.spec.listener_port)
    .bind(&insert.spec.upstream_host)
    .bind(insert.spec.upstream_port)
    .bind(insert.spec.upstream_tls)
    .bind(insert.validated_upstream_ip)
    .bind(insert.spec.upstream_port)
    .bind(insert.cluster_name)
    .bind(insert.route_config_name)
    .bind(insert.listener_name)
    .bind(insert.spec.target_sample_count)
    .bind(insert.spec.max_duration_seconds)
    .bind(insert.spec.max_bytes)
    .bind(insert.spec.max_distinct_paths)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!(
                "discovery session \"{}\" already exists in this team",
                insert.name
            ))
        }
        _ => DomainError::internal(format!("create discovery session: {e}")),
    })?;
    from_row(&row)
}

pub async fn get(
    pool: &PgPool,
    team_id: TeamId,
    session: &str,
) -> DomainResult<Option<DiscoverySession>> {
    let id = uuid::Uuid::parse_str(session).ok();
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM discovery_sessions \
         WHERE team_id = $1 AND (name = $2 OR id = $3)"
    ))
    .bind(team_id.as_uuid())
    .bind(session)
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get discovery session: {e}")))?;
    row.as_ref().map(from_row).transpose()
}

pub async fn list(
    pool: &PgPool,
    team_id: TeamId,
    status: Option<DiscoverySessionStatus>,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<DiscoverySession>, i64)> {
    let status = status.map(DiscoverySessionStatus::as_str);
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM discovery_sessions \
         WHERE team_id = $1 AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC, name LIMIT $3 OFFSET $4"
    ))
    .bind(team_id.as_uuid())
    .bind(status)
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list discovery sessions: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discovery_sessions \
         WHERE team_id = $1 AND ($2::text IS NULL OR status = $2)",
    )
    .bind(team_id.as_uuid())
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count discovery sessions: {e}")))?;
    rows.iter()
        .map(from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session: &str,
) -> DomainResult<DiscoverySession> {
    let id = uuid::Uuid::parse_str(session).ok();
    let row = sqlx::query(&format!(
        "UPDATE discovery_sessions \
         SET status = 'completed', completed_at = now(), updated_at = now() \
         WHERE team_id = $1 AND (name = $2 OR id = $3) AND status = 'capturing' \
         RETURNING {COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(session)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("complete discovery session: {e}")))?;
    match row {
        Some(row) => from_row(&row),
        None => Err(DomainError::not_found("discovery session", session)),
    }
}

pub async fn ingest_raw_observation(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    input: &ObservationIngest,
    provenance: &DiscoveryObservationProvenance,
) -> DomainResult<DiscoveryObservation> {
    input.validate()?;
    let session =
        get_session_for_update(tx, team.id, &provenance.discovery_session_id.to_string()).await?;

    let existing = sqlx::query(&format!(
        "SELECT {RAW_COLUMNS}, {DISCOVERY_RAW_COLUMNS} \
         FROM discovery_raw_observations dro \
         JOIN raw_observations ro ON ro.id = dro.raw_observation_id AND ro.team_id = dro.team_id \
         WHERE dro.team_id = $1 AND dro.discovery_session_id = $2 AND dro.request_id = $3 \
         FOR UPDATE"
    ))
    .bind(team.id.as_uuid())
    .bind(provenance.discovery_session_id.as_uuid())
    .bind(&input.request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock discovery raw observation: {e}")))?;
    let existing_raw = existing.as_ref().map(raw_from_row);
    if let Some(raw) = &existing_raw {
        if raw.method != input.method || raw.path != input.path {
            return Err(DomainError::conflict(
                "discovery observation request_id was already captured with different request metadata",
            ));
        }
    } else if session.status != DiscoverySessionStatus::Capturing
        && session.sample_count < i64::from(session.target_sample_count)
    {
        return Err(DomainError::conflict(format!(
            "discovery session \"{}\" is {}",
            session.name,
            session.status.as_str()
        ))
        .with_hint("only capturing discovery sessions can accept new observations"));
    }

    let incoming_request_headers = sanitize_headers(&input.request_headers);
    let incoming_response_headers = sanitize_headers(&input.response_headers);
    let merged = match existing_raw.as_ref() {
        Some(existing) => RawObservation {
            response_status: input.response_status.or(existing.response_status),
            request_headers: merge_headers(
                existing.request_headers.clone(),
                incoming_request_headers,
            ),
            response_headers: merge_headers(
                existing.response_headers.clone(),
                incoming_response_headers,
            ),
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
            ..existing.clone()
        },
        None => RawObservation {
            id: RawObservationId::generate(),
            team_id: team.id,
            capture_session_id: None,
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
    let merged_request_bytes = merged_body_bytes(
        merged.request_body.as_deref(),
        existing_raw.as_ref().map(|row| row.request_body_bytes),
        input.request_body_bytes,
    );
    let merged_response_bytes = merged_body_bytes(
        merged.response_body.as_deref(),
        existing_raw.as_ref().map(|row| row.response_body_bytes),
        input.response_body_bytes,
    );
    let path_already_present = existing_raw.is_some()
        || discovery_observation_path_exists(
            tx,
            team.id,
            provenance.discovery_session_id,
            &merged.path,
            &input.request_id,
        )
        .await?;
    let quota_change = match decide_observation_quota(
        ObservationQuotaState {
            sample_count: session.sample_count,
            target_sample_count: session.target_sample_count,
            byte_count: session.byte_count,
            max_bytes: session.max_bytes,
            path_count: session.path_count,
            max_distinct_paths: session.max_distinct_paths,
        },
        existing_raw.is_some(),
        existing_raw
            .as_ref()
            .map(|row| row.request_body_bytes + row.response_body_bytes),
        merged_request_bytes + merged_response_bytes,
        path_already_present,
    ) {
        Ok(change) => change,
        Err(reason) => {
            increment_discovery_drop_count(tx, team.id, session.id).await?;
            return Err(reason.into_error("discovery"));
        }
    };

    let ttl_days: i32 = 30;
    sqlx::query(
        "INSERT INTO raw_observations \
         (id, team_id, org_id, capture_session_id, request_id, method, path, response_status, \
          request_headers, response_headers, request_body, response_body, request_body_truncated, \
          response_body_truncated, request_body_bytes, response_body_bytes, metadata_seen, body_seen, \
          observed_at, expires_at) \
         VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
                 COALESCE($14, 0), COALESCE($15, 0), $16, $17, $18, now() + make_interval(days => $19)) \
         ON CONFLICT (id) DO UPDATE SET \
            response_status = EXCLUDED.response_status, request_headers = EXCLUDED.request_headers, \
            response_headers = EXCLUDED.response_headers, request_body = EXCLUDED.request_body, \
            response_body = EXCLUDED.response_body, request_body_truncated = EXCLUDED.request_body_truncated, \
            response_body_truncated = EXCLUDED.response_body_truncated, \
            request_body_bytes = EXCLUDED.request_body_bytes, response_body_bytes = EXCLUDED.response_body_bytes, \
            metadata_seen = EXCLUDED.metadata_seen, body_seen = EXCLUDED.body_seen, \
            observed_at = LEAST(raw_observations.observed_at, EXCLUDED.observed_at), updated_at = now()",
    )
    .bind(merged.id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
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
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("ingest discovery raw observation: {e}")))?;
    sqlx::query(
        "INSERT INTO discovery_raw_observations \
         (raw_observation_id, team_id, request_id, discovery_session_id, discovery_listener_id, \
          observed_host, observed_sni, route_matched, forwarded_upstream_host, \
          forwarded_upstream_port, forwarded_upstream_ip, forwarded_upstream_tls) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         ON CONFLICT (team_id, discovery_session_id, request_id) DO UPDATE SET \
            observed_host = EXCLUDED.observed_host, observed_sni = EXCLUDED.observed_sni, \
            route_matched = EXCLUDED.route_matched, forwarded_upstream_host = EXCLUDED.forwarded_upstream_host, \
            forwarded_upstream_port = EXCLUDED.forwarded_upstream_port, \
            forwarded_upstream_ip = EXCLUDED.forwarded_upstream_ip, \
            forwarded_upstream_tls = EXCLUDED.forwarded_upstream_tls",
    )
    .bind(merged.id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(&merged.request_id)
    .bind(provenance.discovery_session_id.as_uuid())
    .bind(provenance.discovery_listener_id.as_uuid())
    .bind(&provenance.observed_host)
    .bind(&provenance.observed_sni)
    .bind(provenance.route_matched)
    .bind(&provenance.forwarded_upstream_host)
    .bind(provenance.forwarded_upstream_port)
    .bind(&provenance.forwarded_upstream_ip)
    .bind(provenance.forwarded_upstream_tls)
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("upsert discovery provenance: {e}")))?;
    update_discovery_counters(tx, team.id, session.id, quota_change).await?;

    observations_for_session(tx, team.id, provenance.discovery_session_id)
        .await?
        .into_iter()
        .find(|row| row.raw.id == merged.id)
        .ok_or_else(|| DomainError::internal("read ingested discovery observation"))
}

pub async fn completed_observations_for_update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session: &str,
) -> DomainResult<(DiscoverySession, Vec<DiscoveryObservation>)> {
    let session_row = get_session_for_update(tx, team_id, session).await?;
    if session_row.status != DiscoverySessionStatus::Completed {
        return Err(DomainError::conflict(format!(
            "discovery session \"{}\" is {}",
            session_row.name,
            session_row.status.as_str()
        )));
    }
    let observations = observations_for_session(tx, team_id, session_row.id).await?;
    Ok((session_row, observations))
}

async fn get_session_for_update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    handle: &str,
) -> DomainResult<DiscoverySession> {
    let id = Uuid::parse_str(handle).ok();
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM discovery_sessions \
         WHERE team_id = $1 AND (name = $2 OR id = $3) FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(handle)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock discovery session: {e}")))?;
    row.as_ref()
        .map(from_row)
        .transpose()?
        .ok_or_else(|| DomainError::not_found("discovery session", handle))
}

async fn observations_for_session(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: DiscoverySessionId,
) -> DomainResult<Vec<DiscoveryObservation>> {
    let rows = sqlx::query(&format!(
        "SELECT {RAW_COLUMNS}, {DISCOVERY_RAW_COLUMNS} \
         FROM discovery_raw_observations dro \
         JOIN raw_observations ro ON ro.id = dro.raw_observation_id AND ro.team_id = dro.team_id \
         WHERE dro.team_id = $1 AND dro.discovery_session_id = $2 \
         ORDER BY dro.observed_host, dro.observed_sni, ro.observed_at, ro.id"
    ))
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("list discovery observations: {e}")))?;
    Ok(rows.iter().map(observation_from_row).collect())
}

async fn discovery_observation_path_exists(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: DiscoverySessionId,
    path: &str,
    request_id: &str,
) -> DomainResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS( \
            SELECT 1 FROM discovery_raw_observations dro \
            JOIN raw_observations ro ON ro.id = dro.raw_observation_id AND ro.team_id = dro.team_id \
            WHERE dro.team_id = $1 AND dro.discovery_session_id = $2 \
              AND ro.path = $3 AND dro.request_id <> $4)",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(path)
    .bind(request_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("check discovery observation path: {e}")))
}

async fn increment_discovery_drop_count(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: DiscoverySessionId,
) -> DomainResult<()> {
    sqlx::query(
        "UPDATE discovery_sessions SET drop_count = drop_count + 1, updated_at = now() \
         WHERE team_id = $1 AND id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("increment discovery drop count: {e}")))?;
    Ok(())
}

async fn update_discovery_counters(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: DiscoverySessionId,
    change: ObservationQuotaChange,
) -> DomainResult<()> {
    sqlx::query(
        "UPDATE discovery_sessions SET \
            sample_count = sample_count + $3, \
            byte_count = byte_count + $4, \
            path_count = path_count + $5, \
            status = CASE \
                WHEN status = 'capturing' AND sample_count + $3 >= target_sample_count \
                    THEN 'completed' \
                ELSE status \
            END, \
            completed_at = CASE \
                WHEN status = 'capturing' AND sample_count + $3 >= target_sample_count \
                    THEN COALESCE(completed_at, now()) \
                ELSE completed_at \
            END, \
            updated_at = now() \
         WHERE team_id = $1 AND id = $2",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(change.sample_delta)
    .bind(change.byte_delta)
    .bind(change.path_delta)
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update discovery counters: {e}")))?;
    Ok(())
}
