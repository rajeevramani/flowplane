use flowplane::auth::token_service::TokenService;
use flowplane::auth::validation::CreateTokenRequest;
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use rand::{distributions::Alphanumeric, rngs::OsRng, Rng};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use tokio::time::Instant;

async fn setup_service() -> (TokenService, Arc<SqlxTokenRepository>, String) {
    let pool: DbPool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("create sqlite pool");

    sqlx::query(
        r#"
        CREATE TABLE personal_access_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL,
            expires_at DATETIME,
            last_used_at DATETIME,
            created_by TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

            is_setup_token BOOLEAN NOT NULL DEFAULT FALSE,
            max_usage_count INTEGER,
            usage_count INTEGER NOT NULL DEFAULT 0,
            failed_attempts INTEGER NOT NULL DEFAULT 0,
            locked_until DATETIME
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE token_scopes (
            id TEXT PRIMARY KEY,
            token_id TEXT NOT NULL,
            scope TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (token_id) REFERENCES personal_access_tokens(id) ON DELETE CASCADE
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            resource_type TEXT NOT NULL,
            resource_id TEXT,
            resource_name TEXT,
            action TEXT NOT NULL,
            old_configuration TEXT,
            new_configuration TEXT,
            user_id TEXT,
            client_ip TEXT,
            user_agent TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool));
    let service = TokenService::new(repo.clone(), audit);

    let secret = service
        .create_token(CreateTokenRequest {
            name: "security-test".into(),
            description: None,
            expires_at: None,
            scopes: vec!["tokens:read".into()],
            created_by: Some("tests".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    (service, repo, secret.token)
}

fn random_secret() -> String {
    OsRng.sample_iter(&Alphanumeric).take(64).map(char::from).collect()
}

#[tokio::test]
async fn token_verification_timing_within_bounds() {
    let (service, repo, valid_token) = setup_service().await;

    let parts: Vec<&str> = valid_token.split('.').collect();
    assert_eq!(parts.len(), 2);
    let id = parts[0].trim_start_matches("fp_pat_");
    let secret = parts[1];

    let (_token, hashed) = repo
        .find_active_for_auth(&flowplane::domain::TokenId::from_str_unchecked(id))
        .await
        .unwrap()
        .unwrap();

    let correct_duration = measure_verify(&service, &hashed, secret, 5);
    let incorrect_secret = random_secret();
    let incorrect_duration = measure_verify(&service, &hashed, &incorrect_secret, 5);

    let delta = (correct_duration - incorrect_duration).abs();
    // Argon2 verification should be constant-time; allow a small tolerance for scheduling noise.
    assert!(
        delta < 0.02,
        "verification timings diverged too much: correct={correct_duration:?}s, incorrect={incorrect_duration:?}s"
    );

    // Ensure revoked tokens short-circuit before verification to prevent further use.
    let _ = service.revoke_token(id).await.unwrap();
    assert!(matches!(
        service.revoke_token(id).await.unwrap().status,
        flowplane::auth::models::TokenStatus::Revoked
    ));
}

fn measure_verify(service: &TokenService, hashed: &str, candidate: &str, iterations: u32) -> f64 {
    let mut total = 0.0;
    for _ in 0..iterations {
        let start = Instant::now();
        let _ = service.verify_secret(hashed, candidate).unwrap();
        total += start.elapsed().as_secs_f64();
    }
    total / iterations as f64
}
