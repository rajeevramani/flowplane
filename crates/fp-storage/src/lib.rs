//! Flowplane storage layer.
//!
//! Owns the PostgreSQL pool, embedded migrations, and (from S3) the repositories and the
//! event outbox. Repository methods will require a `TeamScope` — there is no unscoped query
//! API outside the platform-admin module (spec/10 §4).

pub mod seed;

use fp_domain::{DomainError, DomainResult};
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

/// Embedded migrations, applied in order; forward-only (spec/10 §10).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Install a rustls crypto provider if none is set. Idempotent. Required because the
/// dependency graph links both `ring` (sqlx, axum-server) and `aws-lc-rs` (reqwest) — with
/// two providers present rustls has NO default until one is chosen, and Postgres TLS
/// negotiation fails with a misleading pool timeout.
pub fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // A racing install by another thread is fine — someone won; both are usable.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Connect a pool with sane timeouts. Fails with `unavailable` + hint, never hangs forever.
pub async fn connect(database_url: &str, max_connections: u32) -> DomainResult<PgPool> {
    ensure_crypto_provider();
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
        .map_err(|e| {
            DomainError::unavailable(format!("cannot connect to PostgreSQL: {e}"))
                .with_hint("verify FLOWPLANE_DATABASE_URL and that PostgreSQL is reachable")
        })
}

/// Apply pending migrations. Forward-only; safe to run on every boot.
pub async fn migrate(pool: &PgPool) -> DomainResult<()> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| DomainError::internal(format!("database migration failed: {e}")))
}

/// Readiness probe: cheap round-trip proving the database answers queries.
pub async fn ping(pool: &PgPool) -> DomainResult<()> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|e| DomainError::unavailable(format!("database ping failed: {e}")))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn test_db_url() -> Option<String> {
        std::env::var("FLOWPLANE_TEST_DATABASE_URL").ok()
    }

    #[tokio::test]
    async fn migrations_apply_and_ping_succeeds_on_real_postgres() {
        let Some(url) = test_db_url() else {
            // Integration environments must set FLOWPLANE_TEST_DATABASE_URL; CI always does.
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = connect(&url, 2)
            .await
            .expect("test database must be reachable");
        assert!(
            migrate(&pool).await.is_ok(),
            "migrations must apply cleanly"
        );
        assert!(
            migrate(&pool).await.is_ok(),
            "migrations must be idempotent on re-run"
        );
        assert!(ping(&pool).await.is_ok());
    }

    #[tokio::test]
    async fn connect_to_unreachable_host_fails_with_unavailable() {
        let err = connect("postgres://nobody@127.0.0.1:1/none", 1)
            .await
            .expect_err("must not connect");
        assert_eq!(err.code, fp_domain::ErrorCode::Unavailable);
    }
}
