//! One-shot bootstrap (spec/08a §2.2.10): first-boot token → platform org + first admin.
//!
//! Flow: the operator supplies a bootstrap token to an uninitialized server (env / file); the
//! server stores only its SHA-256 hash (24 h expiry) and **never logs the plaintext** (#113).
//! The operator calls `POST /api/v1/bootstrap/initialize` with it; in ONE transaction the
//! platform org is created and marked, the admin user is provisioned for their OIDC subject, the
//! owner membership lands, the token is consumed, and the audit row commits. Replays fail closed.
//!
//! A legacy generate-and-log path (`issue_token_if_uninitialized`) remains available only behind
//! an explicit local-only opt-in; `operator_seeded` on `bootstrap_tokens` records which path a
//! token came from so an operator seed can invalidate prior unused legacy tokens while a
//! divergent live operator token across replicas fails closed.

use crate::repos::audit::{ActorType, AuditEntry, Outcome, Surface};
use fp_domain::{DomainError, DomainResult, ErrorCode, OrgId, RequestId, UserId};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Fixed advisory-lock key serializing concurrent `initialize` calls (see below).
const BOOTSTRAP_LOCK_KEY: i64 = 0x666c_6f77_626f_6f74; // "flowboot"

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// The instance is initialized once a platform org is designated.
pub async fn is_initialized(pool: &PgPool) -> DomainResult<bool> {
    let marker: Option<String> =
        sqlx::query_scalar("SELECT value FROM instance_meta WHERE key = 'platform_org_id'")
            .fetch_optional(pool)
            .await
            .map_err(|e| DomainError::internal(format!("bootstrap status: {e}")))?;
    Ok(marker.is_some())
}

/// LEGACY, local-only: generate a fresh bootstrap token if (and only if) the instance is
/// uninitialized, returning the plaintext once for logging. This path leaks the token to logs and
/// is gated behind an explicit opt-in (`FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN`); production seeds
/// the token with [`seed_token_hash_if_uninitialized`] instead. The generated row is marked
/// `operator_seeded = false` so a later operator seed can invalidate it.
pub async fn issue_token_if_uninitialized(pool: &PgPool) -> DomainResult<Option<String>> {
    if is_initialized(pool).await? {
        return Ok(None);
    }
    let token = format!(
        "fpboot_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    sqlx::query(
        "INSERT INTO bootstrap_tokens (id, token_hash, expires_at, operator_seeded) \
         VALUES ($1, $2, now() + interval '24 hours', false)",
    )
    .bind(Uuid::now_v7())
    .bind(hash_token(&token))
    .execute(pool)
    .await
    .map_err(|e| DomainError::internal(format!("issue bootstrap token: {e}")))?;
    Ok(Some(token))
}

/// Outcome of [`seed_token_hash_if_uninitialized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// The operator token's hash was newly seeded.
    Seeded,
    /// This exact token is already the live seed (same token across replicas / re-run). No-op.
    Idempotent,
    /// The instance is already initialized; nothing to seed.
    AlreadyInitialized,
    /// A *different* live operator-seeded token already exists — divergent replica configuration.
    /// Fail closed; the existing token is left untouched.
    Conflict,
}

/// Seed the SHA-256 hash of an **operator-supplied** bootstrap token when the instance is
/// uninitialized — the plaintext is never logged (#113). Serialized with [`initialize`] via the
/// same advisory lock so seeds and the consuming initialize cannot interleave across replicas.
///
/// Semantics (locked in #113):
/// - already initialized → [`SeedOutcome::AlreadyInitialized`] (no row written);
/// - same token already live → [`SeedOutcome::Idempotent`];
/// - a *different* live operator-seeded token exists → [`SeedOutcome::Conflict`] (fail closed,
///   never clobber a peer replica's token);
/// - otherwise invalidate prior unused **legacy** tokens (`operator_seeded = false`) and seed ours
///   → [`SeedOutcome::Seeded`].
pub async fn seed_token_hash_if_uninitialized(
    pool: &PgPool,
    token: &str,
) -> DomainResult<SeedOutcome> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap seed: begin: {e}")))?;

    // Same advisory lock as `initialize`: seed and initialize mutate the same trust boundary, so
    // they must serialize across all connections/replicas.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(BOOTSTRAP_LOCK_KEY)
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap seed: lock: {e}")))?;

    let initialized: Option<String> =
        sqlx::query_scalar("SELECT value FROM instance_meta WHERE key = 'platform_org_id'")
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DomainError::internal(format!("bootstrap seed: check init: {e}")))?;
    if initialized.is_some() {
        return Ok(SeedOutcome::AlreadyInitialized);
    }

    let h = hash_token(token);

    // This exact token is already live AND operator-seeded → idempotent (same operator token across
    // replicas / re-run). A same-hash *legacy* row is intentionally excluded so it falls through to
    // the invalidate-and-promote path below and becomes operator-seeded.
    let mine_live: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM bootstrap_tokens \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() AND operator_seeded = true",
    )
    .bind(&h)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap seed: probe self: {e}")))?;
    if mine_live.is_some() {
        tx.commit()
            .await
            .map_err(|e| DomainError::internal(format!("bootstrap seed: commit: {e}")))?;
        return Ok(SeedOutcome::Idempotent);
    }

    // A different live *operator-seeded* token → divergent replica config. Fail closed; do not
    // invalidate it. (Legacy `operator_seeded = false` tokens do not trigger this.)
    let peer: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM bootstrap_tokens \
         WHERE token_hash <> $1 AND used_at IS NULL AND expires_at > now() \
           AND operator_seeded = true LIMIT 1",
    )
    .bind(&h)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap seed: probe peers: {e}")))?;
    if peer.is_some() {
        return Ok(SeedOutcome::Conflict); // tx rolls back on drop
    }

    // Invalidate prior unused LEGACY tokens (generate-and-log boots), then seed ours.
    sqlx::query(
        "UPDATE bootstrap_tokens SET used_at = now() \
         WHERE used_at IS NULL AND operator_seeded = false",
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap seed: invalidate legacy: {e}")))?;

    // Insert (or refresh an expired same-hash row) as the live operator seed.
    sqlx::query(
        "INSERT INTO bootstrap_tokens (id, token_hash, expires_at, operator_seeded) \
         VALUES ($1, $2, now() + interval '24 hours', true) \
         ON CONFLICT (token_hash) DO UPDATE \
            SET used_at = NULL, expires_at = now() + interval '24 hours', operator_seeded = true",
    )
    .bind(Uuid::now_v7())
    .bind(&h)
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap seed: insert: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap seed: commit: {e}")))?;
    Ok(SeedOutcome::Seeded)
}

/// Consume a bootstrap token and initialize the platform: platform org + first admin.
/// Atomic and single-use: the token row is locked and marked used inside the transaction.
pub async fn initialize(
    pool: &PgPool,
    token: &str,
    org_name: &str,
    org_display_name: &str,
    admin_subject: &str,
    admin_email: &str,
    request_id: RequestId,
) -> DomainResult<(OrgId, UserId)> {
    fp_domain::validate_name(org_name)?;
    let denied = || {
        DomainError::new(
            ErrorCode::Unauthorized,
            "invalid, expired, or used bootstrap token",
        )
        .with_hint("supply a valid bootstrap token to an uninitialized instance and retry")
    };

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap: begin: {e}")))?;

    // Serialize the whole critical section: a transaction-scoped advisory lock means only
    // one initialize runs at a time across all connections. Without it, two concurrent
    // callers with two different valid tokens both pass the "already initialized?" check
    // (the FOR UPDATE below locks nothing when the marker row does not yet exist) and both
    // commit — producing two orgs and a silently-lost platform marker. The lock is released
    // automatically on commit/rollback. Key is an arbitrary fixed constant for this purpose.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(BOOTSTRAP_LOCK_KEY)
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap: lock: {e}")))?;

    // Idempotency guard inside the (now serialized) transaction: only one initialize wins.
    let already: Option<String> =
        sqlx::query_scalar("SELECT value FROM instance_meta WHERE key = 'platform_org_id'")
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DomainError::internal(format!("bootstrap: check: {e}")))?;
    if already.is_some() {
        return Err(DomainError::conflict(
            "this instance is already initialized",
        ));
    }

    // Consume the token: single UPDATE that only matches unused, unexpired rows.
    let consumed = sqlx::query(
        "UPDATE bootstrap_tokens SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(hash_token(token))
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap: consume token: {e}")))?;
    if consumed.rows_affected() != 1 {
        return Err(denied());
    }

    let org_id = OrgId::generate();
    sqlx::query("INSERT INTO organizations (id, name, display_name) VALUES ($1, $2, $3)")
        .bind(org_id.as_uuid())
        .bind(org_name)
        .bind(org_display_name)
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap: create org: {e}")))?;

    sqlx::query(
        "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(org_id.as_uuid().to_string())
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap: mark platform org: {e}")))?;

    let user_id = UserId::generate();
    let row = sqlx::query(
        "INSERT INTO users (id, subject, email, name) VALUES ($1, $2, $3, 'Platform Admin') \
         ON CONFLICT (subject) DO UPDATE SET email = EXCLUDED.email RETURNING id",
    )
    .bind(user_id.as_uuid())
    .bind(admin_subject)
    .bind(admin_email)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap: admin user: {e}")))?;
    let user_id = UserId::from(row.get::<Uuid, _>("id"));

    sqlx::query(
        "INSERT INTO org_memberships (id, user_id, org_id, role) \
         VALUES (gen_random_uuid(), $1, $2, 'owner')",
    )
    .bind(user_id.as_uuid())
    .bind(org_id.as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("bootstrap: membership: {e}")))?;

    crate::repos::audit::record_in_tx(
        &mut tx,
        &AuditEntry {
            request_id: Some(request_id),
            actor_type: ActorType::System,
            actor_id: None,
            actor_label: admin_subject.to_string(),
            surface: Surface::Rest,
            action: "bootstrap.initialize".into(),
            resource: format!("organizations/{org_name}"),
            org_id: Some(org_id),
            team_id: None,
            outcome: Outcome::Success,
            detail: serde_json::json!({ "admin_subject": admin_subject }),
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("bootstrap: commit: {e}")))?;
    tracing::info!(
        org = org_name,
        "instance bootstrapped; platform org designated"
    );
    Ok((org_id, user_id))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn unique(prefix: &str) -> String {
        format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
    }

    #[tokio::test]
    async fn bootstrap_is_one_shot_and_token_is_single_use() {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = crate::connect(&url, 4).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");

        // This test owns instance-level state; serialize against parallel siblings via an
        // advisory lock and clean the marker afterwards.
        sqlx::query("SELECT pg_advisory_lock(420001)")
            .execute(&pool)
            .await
            .expect("lock");
        sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
            .execute(&pool)
            .await
            .expect("clean");

        let token = issue_token_if_uninitialized(&pool)
            .await
            .expect("issue")
            .expect("uninitialized");
        assert!(token.starts_with("fpboot_"));

        // Wrong token: denied.
        let err = initialize(
            &pool,
            "fpboot_wrong",
            &unique("plat"),
            "",
            "sub-x",
            "",
            RequestId::generate(),
        )
        .await
        .expect_err("wrong token");
        assert_eq!(err.code, ErrorCode::Unauthorized);

        // Right token: succeeds, marks initialized.
        let org_name = unique("platform");
        let admin_subject = unique("sub-admin");
        let (org_id, admin) = initialize(
            &pool,
            &token,
            &org_name,
            "Platform",
            &admin_subject,
            "a@p.test",
            RequestId::generate(),
        )
        .await
        .expect("initialize");
        assert!(is_initialized(&pool).await.expect("status"));

        // Replay with the same token: denied (consumed). Second init: conflict.
        let err = initialize(
            &pool,
            &token,
            &unique("again"),
            "",
            "sub-y",
            "",
            RequestId::generate(),
        )
        .await
        .expect_err("replay");
        assert!(matches!(
            err.code,
            ErrorCode::Conflict | ErrorCode::Unauthorized
        ));

        // No further tokens issued once initialized.
        assert!(issue_token_if_uninitialized(&pool)
            .await
            .expect("issue")
            .is_none());

        // The admin loads as platform admin through the standard principal loader.
        let loaded = crate::repos::identity::load_principal(&pool, &admin_subject)
            .await
            .expect("load")
            .expect("exists");
        assert!(
            loaded.platform_admin,
            "bootstrap admin must be platform admin"
        );
        assert_eq!(loaded.user_id, admin);
        assert!(
            loaded.memberships.iter().any(|(id, _)| *id == org_id),
            "bootstrap admin is a member of the platform org"
        );
        assert_eq!(loaded.platform_org_id, Some(org_id));

        // Cleanup so other instance-level tests (and dev seeding) see a clean slate.
        sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
            .execute(&pool)
            .await
            .expect("clean");
        sqlx::query("SELECT pg_advisory_unlock(420001)")
            .execute(&pool)
            .await
            .expect("unlock");
    }

    #[tokio::test]
    async fn concurrent_initialize_with_two_valid_tokens_yields_exactly_one_platform_org() {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = crate::connect(&url, 8).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");

        // Serialize against the other instance-level test (shared instance_meta state).
        sqlx::query("SELECT pg_advisory_lock(420001)")
            .execute(&pool)
            .await
            .expect("lock");
        sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
            .execute(&pool)
            .await
            .expect("clean");

        // Two distinct, valid tokens (issued before initialization).
        let t1 = issue_token_if_uninitialized(&pool)
            .await
            .expect("issue1")
            .expect("uninit");
        let t2 = issue_token_if_uninitialized(&pool)
            .await
            .expect("issue2")
            .expect("uninit");

        // Race two initialize calls. Exactly one must win; the other must be rejected — never
        // two platform orgs (the bug the advisory lock closes).
        let (org_a, sub_a) = (unique("plat-a"), unique("sub-a"));
        let (org_b, sub_b) = (unique("plat-b"), unique("sub-b"));
        let (a, b) = tokio::join!(
            initialize(
                &pool,
                &t1,
                &org_a,
                "A",
                &sub_a,
                "a@p.test",
                RequestId::generate(),
            ),
            initialize(
                &pool,
                &t2,
                &org_b,
                "B",
                &sub_b,
                "b@p.test",
                RequestId::generate(),
            ),
        );
        let wins = [a.is_ok(), b.is_ok()].iter().filter(|x| **x).count();
        assert_eq!(wins, 1, "exactly one initialize may win the race");

        // And the database agrees: a single platform_org_id marker exists.
        let markers: i64 =
            sqlx::query_scalar("SELECT count(*) FROM instance_meta WHERE key = 'platform_org_id'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(markers, 1, "exactly one platform org marker");

        sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
            .execute(&pool)
            .await
            .expect("clean");
        sqlx::query("SELECT pg_advisory_unlock(420001)")
            .execute(&pool)
            .await
            .expect("unlock");
    }

    #[tokio::test]
    async fn operator_seed_semantics() {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = crate::connect(&url, 4).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");

        // Owns instance-level state; serialize against parallel siblings and start from a clean slate.
        sqlx::query("SELECT pg_advisory_lock(420001)")
            .execute(&pool)
            .await
            .expect("lock");
        let clean = |pool: PgPool| async move {
            sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
                .execute(&pool)
                .await
                .expect("clean meta");
            sqlx::query("DELETE FROM bootstrap_tokens")
                .execute(&pool)
                .await
                .expect("clean tokens");
        };
        clean(pool.clone()).await;

        let live_count = |pool: PgPool, operator: bool| async move {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM bootstrap_tokens \
                 WHERE used_at IS NULL AND expires_at > now() AND operator_seeded = $1",
            )
            .bind(operator)
            .fetch_one(&pool)
            .await
            .expect("count");
            n
        };

        let t1 = "operator-token-aaaaaaaaaaaaaaaaaaaaaaaa"; // >= 32 chars
        let t2 = "operator-token-bbbbbbbbbbbbbbbbbbbbbbbb";

        // Fresh seed.
        assert_eq!(
            seed_token_hash_if_uninitialized(&pool, t1)
                .await
                .expect("seed"),
            SeedOutcome::Seeded
        );
        assert_eq!(
            live_count(pool.clone(), true).await,
            1,
            "one live operator token"
        );

        // Same token again → idempotent, still exactly one live row.
        assert_eq!(
            seed_token_hash_if_uninitialized(&pool, t1)
                .await
                .expect("seed"),
            SeedOutcome::Idempotent
        );
        assert_eq!(live_count(pool.clone(), true).await, 1);

        // A different live operator token → fail closed; t1 untouched.
        assert_eq!(
            seed_token_hash_if_uninitialized(&pool, t2)
                .await
                .expect("seed"),
            SeedOutcome::Conflict
        );
        assert_eq!(
            live_count(pool.clone(), true).await,
            1,
            "conflict must not clobber t1"
        );

        // Invalidate prior unused LEGACY token on a successful operator seed.
        clean(pool.clone()).await;
        let legacy = issue_token_if_uninitialized(&pool)
            .await
            .expect("issue legacy")
            .expect("uninitialized");
        assert!(legacy.starts_with("fpboot_"));
        assert_eq!(
            live_count(pool.clone(), false).await,
            1,
            "legacy token live"
        );
        assert_eq!(
            seed_token_hash_if_uninitialized(&pool, t1)
                .await
                .expect("seed"),
            SeedOutcome::Seeded
        );
        assert_eq!(
            live_count(pool.clone(), false).await,
            0,
            "legacy invalidated"
        );
        assert_eq!(
            live_count(pool.clone(), true).await,
            1,
            "operator token live"
        );

        // Supplying the live LEGACY token itself as the operator token promotes it: Seeded, and the
        // row becomes operator_seeded = true (so a later divergent peer fails closed, not clobbers).
        clean(pool.clone()).await;
        let legacy = issue_token_if_uninitialized(&pool)
            .await
            .expect("issue legacy")
            .expect("uninitialized");
        assert_eq!(
            seed_token_hash_if_uninitialized(&pool, &legacy)
                .await
                .expect("seed"),
            SeedOutcome::Seeded
        );
        assert_eq!(
            live_count(pool.clone(), false).await,
            0,
            "no legacy row remains"
        );
        assert_eq!(
            live_count(pool.clone(), true).await,
            1,
            "promoted to operator-seeded"
        );

        // Already initialized → no-op, no new token row.
        clean(pool.clone()).await;
        sqlx::query("INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1)")
            .bind(Uuid::now_v7().to_string())
            .execute(&pool)
            .await
            .expect("mark initialized");
        assert_eq!(
            seed_token_hash_if_uninitialized(&pool, t1)
                .await
                .expect("seed"),
            SeedOutcome::AlreadyInitialized
        );
        assert_eq!(
            live_count(pool.clone(), true).await,
            0,
            "no token seeded once initialized"
        );

        clean(pool.clone()).await;
        sqlx::query("SELECT pg_advisory_unlock(420001)")
            .execute(&pool)
            .await
            .expect("unlock");
    }
}
