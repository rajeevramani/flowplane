//! Transactional outbox writer + consumer dispatcher (spec/10 §3.3).
//!
//! Delivery contract: **at-least-once, in order, per consumer.** A consumer's cursor only
//! advances in the same transaction that follows a successful handle; a crash (or kill -9)
//! between handling and commit re-delivers the batch. Handlers must therefore be idempotent.
//! Multi-replica CPs coordinate through `FOR UPDATE SKIP LOCKED` on the cursor row: one
//! replica works, others skip past instead of blocking.

use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{DomainError, DomainResult};
use sqlx::postgres::PgListener;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::future::Future;
use std::time::Duration;
use uuid::Uuid;

/// Append an event within the caller's transaction — the mutation and its event commit or
/// roll back together. `trace_context` carries the W3C trace fields of the originating
/// request (empty object when none).
pub async fn append(
    tx: &mut Transaction<'_, Postgres>,
    event: &DomainEvent,
    scope: EventScope,
    trace_context: serde_json::Value,
) -> DomainResult<()> {
    let payload = serde_json::to_value(event)
        .map_err(|e| DomainError::internal(format!("outbox: serialize event: {e}")))?;
    sqlx::query(
        "INSERT INTO events (id, event_type, org_id, team_id, payload, trace_context) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(event.kind())
    .bind(scope.org_id.map(|o| o.as_uuid()))
    .bind(scope.team_id.map(|t| t.as_uuid()))
    .bind(payload)
    .bind(trace_context)
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("outbox: append: {e}")))?;
    Ok(())
}

/// A stored event as seen by consumers.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub seq: i64,
    pub event: DomainEvent,
    pub scope: EventScope,
    pub trace_context: serde_json::Value,
}

/// Register a consumer cursor (idempotent). Call once before `run_consumer`.
pub async fn register_consumer(pool: &PgPool, consumer: &str) -> DomainResult<()> {
    sqlx::query("INSERT INTO event_cursors (consumer) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(consumer)
        .execute(pool)
        .await
        .map_err(|e| DomainError::internal(format!("outbox: register consumer: {e}")))?;
    Ok(())
}

/// Process at most one batch for `consumer`. Returns the number of events handled.
///
/// The cursor row is locked (SKIP LOCKED) for the duration: if another replica holds it,
/// returns 0 without blocking. The handler runs while the lock is held; the cursor advance
/// commits only after the handler succeeds — a failure (or crash) re-delivers the batch.
pub async fn process_batch<F, Fut>(
    pool: &PgPool,
    consumer: &str,
    batch_size: i64,
    handler: F,
) -> DomainResult<usize>
where
    F: FnOnce(Vec<StoredEvent>) -> Fut,
    Fut: Future<Output = DomainResult<()>>,
{
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("outbox: begin: {e}")))?;

    let Some(cursor_row) = sqlx::query(
        "SELECT last_seq FROM event_cursors WHERE consumer = $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(consumer)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("outbox: lock cursor: {e}")))?
    else {
        // Another replica is processing (or the consumer is unregistered).
        return Ok(0);
    };
    let last_seq: i64 = cursor_row.get("last_seq");

    let rows = sqlx::query(
        "SELECT seq, event_type, org_id, team_id, payload, trace_context FROM events \
         WHERE seq > $1 ORDER BY seq LIMIT $2",
    )
    .bind(last_seq)
    .bind(batch_size)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("outbox: fetch batch: {e}")))?;

    if rows.is_empty() {
        return Ok(0);
    }

    let mut events = Vec::with_capacity(rows.len());
    let mut max_seq = last_seq;
    for row in rows {
        let seq: i64 = row.get("seq");
        max_seq = max_seq.max(seq);
        let payload: serde_json::Value = row.get("payload");
        match serde_json::from_value::<DomainEvent>(payload) {
            Ok(event) => events.push(StoredEvent {
                seq,
                event,
                scope: EventScope {
                    org_id: row.get::<Option<Uuid>, _>("org_id").map(Into::into),
                    team_id: row.get::<Option<Uuid>, _>("team_id").map(Into::into),
                },
                trace_context: row.get("trace_context"),
            }),
            Err(e) => {
                // An unparseable event is a deployment-skew bug: stop the consumer rather
                // than silently skipping (at-least-once also means never-dropped).
                return Err(DomainError::internal(format!(
                    "outbox: event seq={seq} ({}) does not parse: {e} — \
                     is this binary older than the writer?",
                    row.get::<String, _>("event_type")
                )));
            }
        }
    }

    let count = events.len();
    if let Err(e) = handler(events).await {
        // Roll back explicitly so the cursor lock releases before we return: a dropped
        // transaction rolls back asynchronously, and an immediate retry could SKIP LOCKED
        // past a lock that is still being torn down.
        let _ = tx.rollback().await;
        return Err(e);
    }

    sqlx::query("UPDATE event_cursors SET last_seq = $1, updated_at = now() WHERE consumer = $2")
        .bind(max_seq)
        .bind(consumer)
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::internal(format!("outbox: advance cursor: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("outbox: commit: {e}")))?;

    metrics::counter!("fp_outbox_events_handled_total", "consumer" => consumer.to_string())
        .increment(count as u64);
    Ok(count)
}

/// Current lag (events not yet handled) for a consumer — the readiness signal (spec/10 §8a).
pub async fn consumer_lag(pool: &PgPool, consumer: &str) -> DomainResult<i64> {
    let lag: Option<i64> = sqlx::query_scalar(
        "SELECT (SELECT coalesce(max(seq), 0) FROM events) - last_seq \
         FROM event_cursors WHERE consumer = $1",
    )
    .bind(consumer)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("outbox: lag: {e}")))?;
    Ok(lag.unwrap_or(0))
}

/// Long-running consumer loop: LISTEN for wakeups with a poll fallback, drain batches,
/// stop when `shutdown` flips. Handler errors back off and retry (at-least-once).
pub async fn run_consumer<F, Fut>(
    pool: PgPool,
    consumer: &str,
    handler: F,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> DomainResult<()>
where
    F: Fn(Vec<StoredEvent>) -> Fut + Send,
    Fut: Future<Output = DomainResult<()>> + Send,
{
    register_consumer(&pool, consumer).await?;
    let mut listener = PgListener::connect_with(&pool)
        .await
        .map_err(|e| DomainError::internal(format!("outbox: listener: {e}")))?;
    listener
        .listen("fp_events")
        .await
        .map_err(|e| DomainError::internal(format!("outbox: listen: {e}")))?;

    loop {
        if *shutdown.borrow() {
            tracing::info!(consumer, "outbox consumer stopping");
            return Ok(());
        }
        // Drain everything available before sleeping.
        loop {
            match process_batch(&pool, consumer, 100, &handler).await {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => {
                    metrics::counter!("fp_outbox_handler_failures_total",
                        "consumer" => consumer.to_string())
                    .increment(1);
                    tracing::error!(consumer, "outbox handler failed (will retry): {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    break;
                }
            }
        }
        // Sleep until notified, polled (covers missed notifications), or shut down.
        tokio::select! {
            _ = listener.recv() => {},
            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
            _ = shutdown.changed() => {},
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn unique_consumer() -> String {
        format!("test-{}", Uuid::now_v7().simple())
    }

    async fn pool() -> Option<PgPool> {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return None;
        };
        let pool = crate::connect(&url, 4).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");
        Some(pool)
    }

    async fn append_one(pool: &PgPool, name: &str) {
        let mut tx = pool.begin().await.expect("begin");
        append(
            &mut tx,
            &DomainEvent::ClusterUpserted {
                cluster_id: Uuid::now_v7(),
                name: name.into(),
            },
            EventScope {
                org_id: None,
                team_id: None,
            },
            serde_json::json!({}),
        )
        .await
        .expect("append");
        tx.commit().await.expect("commit");
    }

    #[tokio::test]
    async fn events_commit_with_their_transaction_and_roll_back_with_it() {
        let Some(pool) = pool().await else { return };
        let consumer = unique_consumer();
        register_consumer(&pool, &consumer).await.expect("register");
        // Fast-forward past pre-existing events from parallel tests.
        let _ = process_batch(&pool, &consumer, 10_000, |_| async { Ok(()) }).await;

        // Rolled-back transaction: its event must never be delivered.
        let mut tx = pool.begin().await.expect("begin");
        append(
            &mut tx,
            &DomainEvent::ClusterDeleted {
                cluster_id: Uuid::now_v7(),
                name: "ghost".into(),
            },
            EventScope {
                org_id: None,
                team_id: None,
            },
            serde_json::json!({}),
        )
        .await
        .expect("append");
        drop(tx); // rollback

        append_one(&pool, "real").await;

        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let seen2 = seen.clone();
        // Drain in batches until quiet; parallel tests may interleave their own events.
        while process_batch(&pool, &consumer, 100, |events| {
            let seen = seen2.clone();
            async move {
                let mut guard = seen.lock().unwrap_or_else(|p| p.into_inner());
                for e in events {
                    if let DomainEvent::ClusterDeleted { name, .. }
                    | DomainEvent::ClusterUpserted { name, .. } = e.event
                    {
                        guard.push(name);
                    }
                }
                Ok(())
            }
        })
        .await
        .expect("process")
            > 0
        {}

        let names = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(
            names.contains(&"real".to_string()),
            "committed event delivered"
        );
        assert!(
            !names.contains(&"ghost".to_string()),
            "rolled-back event never delivered"
        );
    }

    #[tokio::test]
    async fn failed_handler_redelivers_the_same_batch() {
        let Some(pool) = pool().await else { return };
        let consumer = unique_consumer();
        register_consumer(&pool, &consumer).await.expect("register");
        let _ = process_batch(&pool, &consumer, 10_000, |_| async { Ok(()) }).await;

        append_one(&pool, "must-not-be-lost").await;

        // First attempt: handler crashes (simulates the process dying mid-handle —
        // the cursor-advance transaction never commits).
        let err = process_batch(&pool, &consumer, 100, |_| async {
            Err(DomainError::internal("simulated crash"))
        })
        .await;
        assert!(err.is_err());

        // Retry (or a fresh replica after kill -9): the SAME events arrive again.
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts2 = attempts.clone();
        let handled = process_batch(&pool, &consumer, 100, |events| {
            let attempts = attempts2.clone();
            async move {
                attempts.fetch_add(events.len(), Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .expect("retry succeeds");
        assert!(
            handled >= 1,
            "at-least-once: events redelivered after failure"
        );
        assert!(attempts.load(Ordering::SeqCst) >= 1);

        // After success OUR event is never delivered a third time. (Parallel tests may
        // append unrelated events meanwhile, so we check for our marker, not count == 0.)
        let seen_again = Arc::new(AtomicUsize::new(0));
        let seen_again2 = seen_again.clone();
        process_batch(&pool, &consumer, 10_000, move |events| {
            let seen = seen_again2.clone();
            async move {
                for e in &events {
                    if matches!(&e.event,
                        fp_domain::event::DomainEvent::ClusterUpserted { name, .. }
                            if name == "must-not-be-lost")
                    {
                        seen.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Ok(())
            }
        })
        .await
        .expect("quiet");
        assert_eq!(
            seen_again.load(Ordering::SeqCst),
            0,
            "cursor advanced exactly once"
        );
    }

    #[tokio::test]
    async fn independent_consumers_see_the_full_stream_independently() {
        let Some(pool) = pool().await else { return };
        let (a, b) = (unique_consumer(), unique_consumer());
        register_consumer(&pool, &a).await.expect("a");
        register_consumer(&pool, &b).await.expect("b");
        let _ = process_batch(&pool, &a, 10_000, |_| async { Ok(()) }).await;
        let _ = process_batch(&pool, &b, 10_000, |_| async { Ok(()) }).await;

        append_one(&pool, "fanout").await;
        let got_a = process_batch(&pool, &a, 100, |_| async { Ok(()) })
            .await
            .expect("a");
        let got_b = process_batch(&pool, &b, 100, |_| async { Ok(()) })
            .await
            .expect("b");
        assert!(got_a >= 1, "consumer A sees the event");
        assert!(got_b >= 1, "consumer B sees the same event independently");
        // Parallel tests keep appending; drain until we observe zero lag (bounded).
        let mut caught_up = false;
        for _ in 0..50 {
            if consumer_lag(&pool, &a).await.expect("lag") == 0 {
                caught_up = true;
                break;
            }
            let _ = process_batch(&pool, &a, 10_000, |_| async { Ok(()) }).await;
        }
        assert!(caught_up, "consumer A catches up to zero lag");
    }
}
