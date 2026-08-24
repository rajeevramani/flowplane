#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_domain::secret::{SecretReference, SecretType};
use fp_domain::{AiProviderKind, AiProviderSpec, ErrorCode, SecretId};
use fp_storage::repos::{ai, identity, secret_refs};
use sqlx::{PgPool, Row};
use std::time::Duration;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn team(pool: &PgPool, prefix: &str) -> TeamRef {
    let org = identity::create_org(pool, &unique(&format!("{prefix}-org")), "")
        .await
        .expect("org");
    let team = identity::create_team(pool, org.id, &unique(&format!("{prefix}-team")), "")
        .await
        .expect("team");
    TeamRef {
        id: team.id,
        org_id: org.id,
    }
}

async fn secret(pool: &PgPool, team: TeamRef, name: &str, secret_type: SecretType) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets \
         (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
         VALUES ($1, $2, $3, $4, '', $5, $6, $7, 'test')",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(secret_type.as_str())
    .bind(vec![1_u8])
    .bind(vec![0_u8; 12])
    .execute(pool)
    .await
    .expect("secret");
    id
}

async fn listener(pool: &PgPool, team: TeamRef, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO listeners (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(serde_json::json!({"address":"127.0.0.1","port":18443}))
    .execute(pool)
    .await
    .expect("listener");
    id
}

async fn cluster(pool: &PgPool, team: TeamRef, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO clusters (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(serde_json::json!({"endpoints":[]}))
    .execute(pool)
    .await
    .expect("cluster");
    id
}

#[tokio::test]
async fn listener_refs_are_same_team_restrict_secret_and_cascade_with_owner() {
    let Some(pool) = pool().await else { return };
    let team_a = team(&pool, "refs-a").await;
    let team_b = team(&pool, "refs-b").await;
    let name = unique("shared-cert");
    let secret_a = secret(&pool, team_a, &name, SecretType::TlsCertificate).await;
    let secret_b = secret(&pool, team_b, &name, SecretType::TlsCertificate).await;
    let listener_a = listener(&pool, team_a, &unique("listener")).await;

    let cross_team = sqlx::query(
        "INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage) \
         VALUES ($1, $2, $3, 'tls_certificate')",
    )
    .bind(listener_a)
    .bind(team_a.id.as_uuid())
    .bind(secret_b)
    .execute(&pool)
    .await
    .expect_err("cross-team secret ref must fail");
    assert_eq!(
        cross_team
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503")
    );

    sqlx::query(
        "INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage) \
         VALUES ($1, $2, $3, 'tls_certificate')",
    )
    .bind(listener_a)
    .bind(team_a.id.as_uuid())
    .bind(secret_a)
    .execute(&pool)
    .await
    .expect("same-team ref");

    let restricted = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(secret_a)
        .execute(&pool)
        .await
        .expect_err("referenced secret delete must be restricted");
    assert_eq!(
        restricted
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503")
    );

    sqlx::query("DELETE FROM listeners WHERE id = $1")
        .bind(listener_a)
        .execute(&pool)
        .await
        .expect("delete owner");
    let refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM listener_secret_refs WHERE listener_id = $1")
            .bind(listener_a)
            .fetch_one(&pool)
            .await
            .expect("count refs");
    assert_eq!(refs, 0);
    sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(secret_a)
        .execute(&pool)
        .await
        .expect("delete unreferenced secret");
}

#[tokio::test]
async fn key_share_resolution_serializes_delete_without_blocking_other_teams() {
    let Some(pool) = pool().await else { return };
    let team_a = team(&pool, "race-a").await;
    let team_b = team(&pool, "race-b").await;
    let name = unique("race-cert");
    let secret_a = secret(&pool, team_a, &name, SecretType::TlsCertificate).await;
    secret(&pool, team_b, &name, SecretType::TlsCertificate).await;
    let listener_a = listener(&pool, team_a, &unique("race-listener")).await;

    let reference = SecretReference {
        name: &name,
        required_type: SecretType::TlsCertificate,
        usage: "tls_certificate",
    };
    let mut writer = pool.begin().await.expect("writer tx");
    let resolved = secret_refs::resolve(&mut writer, team_a.id, &[reference])
        .await
        .expect("resolve A");
    secret_refs::replace_listener(&mut writer, team_a.id, listener_a, &resolved)
        .await
        .expect("insert ref A");

    let mut delete = pool.begin().await.expect("delete tx");
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *delete)
        .await
        .expect("lock timeout");
    let blocked = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(secret_a)
        .execute(&mut *delete)
        .await
        .expect_err("delete must wait behind key-share writer");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    delete.rollback().await.expect("rollback blocked delete");

    let mut other_team = pool.begin().await.expect("other-team tx");
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *other_team)
        .await
        .expect("other lock timeout");
    let other_references = [reference];
    let other_ref = secret_refs::resolve(&mut other_team, team_b.id, &other_references);
    tokio::time::timeout(Duration::from_secs(1), other_ref)
        .await
        .expect("different-team resolution must not block")
        .expect("resolve B");
    other_team.rollback().await.expect("rollback B");

    writer.commit().await.expect("commit writer");
    let restricted = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(secret_a)
        .execute(&pool)
        .await
        .expect_err("committed ref must restrict delete");
    assert_eq!(
        restricted
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503")
    );
}

#[tokio::test]
async fn cluster_and_ai_writers_serialize_delete_and_delete_first_fails_closed() {
    let Some(pool) = pool().await else { return };
    let team = team(&pool, "other-races").await;

    let ca_name = unique("cluster-ca");
    let ca = secret(
        &pool,
        team,
        &ca_name,
        SecretType::CertificateValidationContext,
    )
    .await;
    let cluster_id = cluster(&pool, team, &unique("race-cluster")).await;
    let ca_reference = SecretReference {
        name: &ca_name,
        required_type: SecretType::CertificateValidationContext,
        usage: "validation_context",
    };
    let mut cluster_writer = pool.begin().await.expect("cluster writer");
    let resolved = secret_refs::resolve(&mut cluster_writer, team.id, &[ca_reference])
        .await
        .expect("resolve cluster CA");
    secret_refs::replace_cluster(&mut cluster_writer, team.id, cluster_id, &resolved)
        .await
        .expect("insert cluster ref");
    let mut ca_delete = pool.begin().await.expect("CA delete");
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *ca_delete)
        .await
        .expect("CA timeout");
    let blocked = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(ca)
        .execute(&mut *ca_delete)
        .await
        .expect_err("cluster writer must serialize delete");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    ca_delete.rollback().await.expect("rollback CA delete");
    cluster_writer
        .commit()
        .await
        .expect("commit cluster writer");
    let restricted = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(ca)
        .execute(&pool)
        .await
        .expect_err("cluster ref must restrict delete");
    assert_eq!(
        restricted
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503")
    );

    let ai_secret = secret(&pool, team, &unique("ai-race"), SecretType::GenericSecret).await;
    let mut ai_writer = pool.begin().await.expect("AI writer");
    sqlx::query(
        "INSERT INTO ai_providers \
         (id, team_id, org_id, name, kind, base_url, credential_secret_id) \
         VALUES ($1, $2, $3, $4, 'openai', 'https://example.test', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("race-provider"))
    .bind(ai_secret)
    .execute(&mut *ai_writer)
    .await
    .expect("AI provider writer");
    let mut ai_delete = pool.begin().await.expect("AI delete");
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *ai_delete)
        .await
        .expect("AI timeout");
    let blocked = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(ai_secret)
        .execute(&mut *ai_delete)
        .await
        .expect_err("AI writer must serialize delete");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    ai_delete.rollback().await.expect("rollback AI delete");
    ai_writer.commit().await.expect("commit AI writer");
    let restricted = sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(ai_secret)
        .execute(&pool)
        .await
        .expect_err("AI FK must restrict delete");
    assert_eq!(
        restricted
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503")
    );

    let deleted_name = unique("delete-first");
    let deleted_secret = secret(&pool, team, &deleted_name, SecretType::TlsCertificate).await;
    let mut deleter = pool.begin().await.expect("delete-first tx");
    sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(deleted_secret)
        .execute(&mut *deleter)
        .await
        .expect("delete first");
    let mut waiting_writer = pool.begin().await.expect("waiting writer");
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *waiting_writer)
        .await
        .expect("waiting timeout");
    let deleted_reference = [SecretReference {
        name: &deleted_name,
        required_type: SecretType::TlsCertificate,
        usage: "tls_certificate",
    }];
    let err = secret_refs::resolve(&mut waiting_writer, team.id, &deleted_reference)
        .await
        .expect_err("writer must not pass a concurrent delete");
    assert_eq!(err.code, fp_domain::ErrorCode::Internal);
    waiting_writer
        .rollback()
        .await
        .expect("rollback waiting writer");
    deleter.commit().await.expect("commit delete first");
    let mut after_delete = pool.begin().await.expect("after-delete tx");
    let err = secret_refs::resolve(&mut after_delete, team.id, &deleted_reference)
        .await
        .expect_err("committed delete must resolve as absent");
    assert_eq!(err.code, fp_domain::ErrorCode::NotFound);
    after_delete
        .rollback()
        .await
        .expect("rollback after-delete");
}

#[tokio::test]
async fn ai_provider_fk_loser_maps_to_secret_not_found_on_create_and_update() {
    let Some(pool) = pool().await else { return };
    let team = team(&pool, "ai-fk-map").await;
    let deleted = SecretId::from(
        secret(
            &pool,
            team,
            &unique("deleted-ai-key"),
            SecretType::GenericSecret,
        )
        .await,
    );
    sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(deleted.as_uuid())
        .execute(&pool)
        .await
        .expect("delete credential before writer");
    let spec = |secret_id| AiProviderSpec {
        kind: AiProviderKind::OpenaiCompatible,
        base_url: "https://example.test".into(),
        path_prefix: Some("/v1".into()),
        credential_secret_id: secret_id,
        models: vec!["model".into()],
        auth_header: "authorization".into(),
        auth_scheme: None,
    };
    let mut create_tx = pool.begin().await.expect("create tx");
    let err = ai::create(&mut create_tx, team, &unique("provider"), &spec(deleted))
        .await
        .expect_err("deleted credential create must fail");
    assert_eq!(err.code, ErrorCode::NotFound);
    create_tx.rollback().await.expect("rollback failed create");

    let original = SecretId::from(
        secret(
            &pool,
            team,
            &unique("original-ai-key"),
            SecretType::GenericSecret,
        )
        .await,
    );
    let replacement = SecretId::from(
        secret(
            &pool,
            team,
            &unique("replacement-ai-key"),
            SecretType::GenericSecret,
        )
        .await,
    );
    let provider_name = unique("provider-update");
    let mut create_tx = pool.begin().await.expect("provider create tx");
    let provider = ai::create(&mut create_tx, team, &provider_name, &spec(original))
        .await
        .expect("provider create");
    create_tx.commit().await.expect("provider create commit");
    sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(replacement.as_uuid())
        .execute(&pool)
        .await
        .expect("delete replacement before update");
    let mut update_tx = pool.begin().await.expect("update tx");
    let err = ai::update(
        &mut update_tx,
        team.id,
        &provider_name,
        &spec(replacement),
        provider.version,
    )
    .await
    .expect_err("deleted replacement update must fail");
    assert_eq!(err.code, ErrorCode::NotFound);
    update_tx.rollback().await.expect("rollback failed update");
}

#[tokio::test]
async fn team_delete_with_gateway_refs_and_ai_credential_is_fk_safe() {
    let Some(pool) = pool().await else { return };
    let team = team(&pool, "delete-team").await;
    let ai_secret = secret(&pool, team, &unique("ai-key"), SecretType::GenericSecret).await;
    let cert = secret(
        &pool,
        team,
        &unique("server-cert"),
        SecretType::TlsCertificate,
    )
    .await;
    let ca = secret(
        &pool,
        team,
        &unique("upstream-ca"),
        SecretType::CertificateValidationContext,
    )
    .await;
    let listener_id = listener(&pool, team, &unique("listener")).await;
    let cluster_id = cluster(&pool, team, &unique("cluster")).await;
    sqlx::query(
        "INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage) \
         VALUES ($1, $2, $3, 'tls_certificate')",
    )
    .bind(listener_id)
    .bind(team.id.as_uuid())
    .bind(cert)
    .execute(&pool)
    .await
    .expect("listener ref");
    sqlx::query(
        "INSERT INTO cluster_secret_refs (cluster_id, team_id, secret_id, usage) \
         VALUES ($1, $2, $3, 'validation_context')",
    )
    .bind(cluster_id)
    .bind(team.id.as_uuid())
    .bind(ca)
    .execute(&pool)
    .await
    .expect("cluster ref");
    sqlx::query(
        "INSERT INTO ai_providers \
         (id, team_id, org_id, name, kind, base_url, credential_secret_id) \
         VALUES ($1, $2, $3, $4, 'openai', 'https://example.test', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("provider"))
    .bind(ai_secret)
    .execute(&pool)
    .await
    .expect("provider");

    let mut blocked = pool.begin().await.expect("blocked team delete tx");
    let err = identity::delete_team_tx(&mut blocked, team.id)
        .await
        .expect_err("gateway owners must block team delete");
    assert_eq!(err.code, fp_domain::ErrorCode::Conflict);
    blocked.rollback().await.expect("rollback blocked delete");

    sqlx::query("DELETE FROM listeners WHERE id = $1")
        .bind(listener_id)
        .execute(&pool)
        .await
        .expect("delete listener owner");
    sqlx::query("DELETE FROM clusters WHERE id = $1")
        .bind(cluster_id)
        .execute(&pool)
        .await
        .expect("delete cluster owner");

    let mut tx = pool.begin().await.expect("team delete tx");
    identity::delete_team_tx(&mut tx, team.id)
        .await
        .expect("team delete must order cascades safely");
    tx.commit().await.expect("commit team delete");

    let remains: i64 = sqlx::query("SELECT count(*) FROM teams WHERE id = $1")
        .bind(team.id.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("team count")
        .get(0);
    assert_eq!(remains, 0);
}
