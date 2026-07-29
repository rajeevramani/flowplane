//! Discovery session repository.

use fp_domain::api_lifecycle::{ObservationIngest, RawObservation};
use fp_domain::authz::TeamRef;
use fp_domain::{
    DiscoveryObservation, DiscoveryObservationProvenance, DiscoverySession, DiscoverySessionId,
    DiscoverySessionSpec, DiscoverySessionStatus, DomainError, DomainResult, ErrorCode,
    RawObservationId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::types::Uuid;
use sqlx::{PgPool, Postgres, Row, Transaction};

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
    // Idempotent for an already-completed session: ingest can now auto-complete a session when it
    // reaches its target sample count, and `stop_session` must still be able to complete-then-tear
    // down its forwarding resources afterwards. Accept 'capturing' OR 'completed' (preserving the
    // original completed_at); cancelled/failed sessions match nothing and surface NotFound.
    let row = sqlx::query(&format!(
        "UPDATE discovery_sessions \
         SET status = 'completed', completed_at = COALESCE(completed_at, now()), updated_at = now() \
         WHERE team_id = $1 AND (name = $2 OR id = $3) AND status IN ('capturing', 'completed') \
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
    // Lock the session row regardless of status. A single proxied request arrives as two
    // observations — metadata-only then body-only (crates/fp-xds/src/capture.rs). If the metadata
    // half alone reaches target_sample_count and auto-completes the session, the trailing body
    // half must still merge into the existing row; a `status = 'capturing'` lock would reject it
    // with NotFound. New observations are still gated on `capturing` below. Mirrors learning's
    // `get_capture_session_for_update` (crates/fp-storage/src/repos/api_lifecycle.rs).
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
    let existing_raw: Option<RawObservation> = existing.as_ref().map(raw_from_row);

    if let Some(raw) = &existing_raw {
        if raw.method != input.method || raw.path != input.path {
            return Err(DomainError::conflict(
                "discovery observation request_id was already captured with different request metadata",
            ));
        }
    } else {
        // New observation: only a capturing session below its target may accept it. Merges into an
        // existing request (above) are always allowed so a completed session still absorbs the
        // trailing body half.
        if session.status != DiscoverySessionStatus::Capturing {
            return Err(DomainError::conflict(format!(
                "discovery session \"{}\" is {}",
                session.name,
                session.status.as_str()
            ))
            .with_hint("only capturing sessions can accept new observations"));
        }
        if session.sample_count >= i64::from(session.target_sample_count) {
            increment_discovery_drop_count(tx, team.id, session.id).await?;
            return Err(DomainError::new(
                ErrorCode::QuotaExceeded,
                "discovery session has reached its target sample count",
            )
            .with_hint("start a new discovery session for additional samples"));
        }
    }

    // Merge body payloads so byte_count stays correct across the metadata/body split: prefer the
    // incoming value, fall back to what the earlier half stored. Header sanitize/merge is out of
    // scope for this counter fix (pre-existing overwrite behavior retained below).
    let merged_request_body = input
        .request_body
        .clone()
        .or_else(|| existing_raw.as_ref().and_then(|r| r.request_body.clone()));
    let merged_response_body = input
        .response_body
        .clone()
        .or_else(|| existing_raw.as_ref().and_then(|r| r.response_body.clone()));
    let merged_request_bytes = crate::repos::api_lifecycle::merged_body_bytes(
        merged_request_body.as_deref(),
        existing_raw.as_ref().map(|r| r.request_body_bytes),
        input.request_body_bytes,
    );
    let merged_response_bytes = crate::repos::api_lifecycle::merged_body_bytes(
        merged_response_body.as_deref(),
        existing_raw.as_ref().map(|r| r.response_body_bytes),
        input.response_body_bytes,
    );
    let merged_metadata_seen = existing_raw
        .as_ref()
        .map(|r| r.metadata_seen)
        .unwrap_or(false)
        || input.metadata_seen;
    let merged_body_seen =
        existing_raw.as_ref().map(|r| r.body_seen).unwrap_or(false) || input.body_seen;
    let merged_request_truncated = existing_raw
        .as_ref()
        .map(|r| r.request_body_truncated)
        .unwrap_or(false)
        || input.request_body_truncated;
    let merged_response_truncated = existing_raw
        .as_ref()
        .map(|r| r.response_body_truncated)
        .unwrap_or(false)
        || input.response_body_truncated;

    enforce_discovery_quotas(
        tx,
        &session,
        existing_raw.as_ref(),
        &input.path,
        merged_request_bytes + merged_response_bytes,
        &input.request_id,
    )
    .await?;

    let id = existing_raw
        .as_ref()
        .map(|r| r.id)
        .unwrap_or_else(RawObservationId::generate);
    let ttl_days: i32 = 30;
    sqlx::query(
        "INSERT INTO raw_observations \
         (id, team_id, org_id, capture_session_id, request_id, method, path, response_status, \
          request_headers, response_headers, request_body, response_body, request_body_truncated, \
          response_body_truncated, request_body_bytes, response_body_bytes, metadata_seen, body_seen, \
          observed_at, expires_at) \
         VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
                 $14, $15, $16, $17, $18, now() + make_interval(days => $19)) \
         ON CONFLICT (id) DO UPDATE SET \
            response_status = COALESCE(EXCLUDED.response_status, raw_observations.response_status), \
            request_headers = EXCLUDED.request_headers, \
            response_headers = EXCLUDED.response_headers, request_body = EXCLUDED.request_body, \
            response_body = EXCLUDED.response_body, request_body_truncated = EXCLUDED.request_body_truncated, \
            response_body_truncated = EXCLUDED.response_body_truncated, \
            request_body_bytes = EXCLUDED.request_body_bytes, response_body_bytes = EXCLUDED.response_body_bytes, \
            metadata_seen = EXCLUDED.metadata_seen, body_seen = EXCLUDED.body_seen, \
            observed_at = LEAST(raw_observations.observed_at, EXCLUDED.observed_at), updated_at = now()",
    )
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(&input.request_id)
    .bind(&input.method)
    .bind(&input.path)
    .bind(input.response_status)
    .bind(serde_json::Value::Object(input.request_headers.clone()))
    .bind(serde_json::Value::Object(input.response_headers.clone()))
    .bind(&merged_request_body)
    .bind(&merged_response_body)
    .bind(merged_request_truncated)
    .bind(merged_response_truncated)
    .bind(merged_request_bytes)
    .bind(merged_response_bytes)
    .bind(merged_metadata_seen)
    .bind(merged_body_seen)
    .bind(input.observed_at)
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
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(&input.request_id)
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

    update_discovery_counters_incremental(
        tx,
        &session,
        existing_raw.as_ref(),
        &input.path,
        merged_request_bytes + merged_response_bytes,
        &input.request_id,
    )
    .await?;

    observations_for_session(tx, team.id, provenance.discovery_session_id)
        .await?
        .into_iter()
        .find(|row| row.raw.id == id)
        .ok_or_else(|| DomainError::internal("read ingested discovery observation"))
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

/// Whether the discovery session already has an observation on `path` under a *different*
/// request id (used to count distinct paths, excluding the row being upserted).
async fn discovery_path_exists(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session_id: DiscoverySessionId,
    path: &str,
    request_id: &str,
) -> DomainResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM discovery_raw_observations dro \
         JOIN raw_observations ro ON ro.id = dro.raw_observation_id AND ro.team_id = dro.team_id \
         WHERE dro.team_id = $1 AND dro.discovery_session_id = $2 AND ro.path = $3 \
           AND dro.request_id <> $4)",
    )
    .bind(team_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(path)
    .bind(request_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("check discovery observation path: {e}")))
}

/// Enforce byte and distinct-path quotas, incrementing `drop_count` on rejection. The
/// target_sample_count gate lives inline in the caller (only reachable for new observations while
/// the session is still capturing). Mirrors learning's `enforce_observation_quotas`.
async fn enforce_discovery_quotas(
    tx: &mut Transaction<'_, Postgres>,
    session: &DiscoverySession,
    existing: Option<&RawObservation>,
    path: &str,
    merged_body_bytes: i64,
    request_id: &str,
) -> DomainResult<()> {
    let existing_body_bytes = existing
        .map(|row| row.request_body_bytes + row.response_body_bytes)
        .unwrap_or(0);
    let next_byte_count = session.byte_count - existing_body_bytes + merged_body_bytes;
    if next_byte_count > session.max_bytes {
        increment_discovery_drop_count(tx, session.team_id, session.id).await?;
        return Err(DomainError::new(
            ErrorCode::QuotaExceeded,
            "discovery session has reached its raw observation byte limit",
        )
        .with_hint("raise max_bytes or start a narrower discovery session"));
    }
    if existing.is_none() {
        let path_already_present =
            discovery_path_exists(tx, session.team_id, session.id, path, request_id).await?;
        if !path_already_present && session.path_count + 1 > i64::from(session.max_distinct_paths) {
            increment_discovery_drop_count(tx, session.team_id, session.id).await?;
            return Err(DomainError::new(
                ErrorCode::QuotaExceeded,
                "discovery session has reached its distinct path limit",
            )
            .with_hint("raise max_distinct_paths or scope discovery to fewer routes"));
        }
    }
    Ok(())
}

/// Apply sample/byte/path deltas and atomically auto-complete the session when it reaches its
/// target. Mirrors learning's `update_capture_counters_incremental`.
async fn update_discovery_counters_incremental(
    tx: &mut Transaction<'_, Postgres>,
    session: &DiscoverySession,
    existing: Option<&RawObservation>,
    path: &str,
    merged_body_bytes: i64,
    request_id: &str,
) -> DomainResult<()> {
    let existing_body_bytes = existing
        .map(|row| row.request_body_bytes + row.response_body_bytes)
        .unwrap_or(0);
    let sample_delta = if existing.is_some() { 0 } else { 1 };
    let path_delta = if existing.is_some()
        || discovery_path_exists(tx, session.team_id, session.id, path, request_id).await?
    {
        0
    } else {
        1
    };
    let byte_delta = merged_body_bytes - existing_body_bytes;
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
    .bind(session.team_id.as_uuid())
    .bind(session.id.as_uuid())
    .bind(sample_delta)
    .bind(byte_delta)
    .bind(path_delta)
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update discovery counters: {e}")))?;
    Ok(())
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
