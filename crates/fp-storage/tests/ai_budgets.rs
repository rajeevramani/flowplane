#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_domain::{AiProviderId, OpenAiTokenUsage, RouteConfigId};
use fp_storage::repos::{ai, identity};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct World {
    pool: PgPool,
    team_a: TeamRef,
    team_b: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 16).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");

    Some(World {
        pool,
        team_a: TeamRef {
            id: team_a.id,
            org_id: org.id,
        },
        team_b: TeamRef {
            id: team_b.id,
            org_id: org.id,
        },
    })
}

async fn insert_provider_and_route(pool: &PgPool, team: TeamRef) -> (AiProviderId, RouteConfigId) {
    let secret_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets \
         (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
         VALUES ($1, $2, $3, $4, '', 'generic_secret', $5, $6, 'default')",
    )
    .bind(secret_id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("ai-key"))
    .bind(Vec::<u8>::from([1_u8]))
    .bind(Vec::<u8>::from([2_u8; 12]))
    .execute(pool)
    .await
    .expect("secret");

    let provider_id = AiProviderId::generate();
    sqlx::query(
        "INSERT INTO ai_providers \
         (id, team_id, org_id, name, kind, base_url, credential_secret_id, auth_header) \
         VALUES ($1, $2, $3, $4, 'openai', 'https://api.openai.example', $5, 'authorization')",
    )
    .bind(provider_id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("provider"))
    .bind(secret_id)
    .execute(pool)
    .await
    .expect("provider");

    let route_config_id = RouteConfigId::generate();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb)",
    )
    .bind(route_config_id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("route-config"))
    .execute(pool)
    .await
    .expect("route config");

    (provider_id, route_config_id)
}

async fn insert_budget(
    pool: &PgPool,
    team: TeamRef,
    provider_id: AiProviderId,
    route_config_id: RouteConfigId,
    limit_units: i64,
) -> Uuid {
    let budget_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO ai_budgets \
         (id, team_id, org_id, name, mode, limit_units, window_seconds, provider_id, route_config_id, prompt_token_weight, completion_token_weight) \
         VALUES ($1, $2, $3, $4, 'enforcing', $5, 3600, $6, $7, 1, 2)",
    )
    .bind(budget_id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("budget"))
    .bind(limit_units)
    .bind(provider_id.as_uuid())
    .bind(route_config_id.as_uuid())
    .execute(pool)
    .await
    .expect("budget");
    budget_id
}

async fn used_units(pool: &PgPool, budget_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT used_units FROM ai_budget_counters WHERE budget_id = $1")
        .bind(budget_id)
        .fetch_one(pool)
        .await
        .expect("used units")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_budget_settlement_is_atomic_and_team_scoped() {
    let Some(w) = world().await else { return };
    let (provider_a, route_a) = insert_provider_and_route(&w.pool, w.team_a).await;
    let (provider_b, route_b) = insert_provider_and_route(&w.pool, w.team_b).await;
    let budget_a = insert_budget(&w.pool, w.team_a, provider_a, route_a, 1_000).await;
    let budget_b = insert_budget(&w.pool, w.team_b, provider_b, route_b, 10).await;

    let per_event_units = 7_i64; // 3 prompt + (2 completion * weight 2)
    let events = 32_i64;
    let mut tasks = Vec::new();
    for _ in 0..events {
        let pool = w.pool.clone();
        tasks.push(tokio::spawn(async move {
            ai::record_usage_event_and_settle_budgets(
                &pool,
                ai::AiUsageEventInsert {
                    team_id: w.team_a.id,
                    route_config_id: route_a,
                    provider_id: provider_a,
                    backend_position: Some(0),
                    usage: OpenAiTokenUsage {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                        total_tokens: 5,
                    },
                },
            )
            .await
            .expect("settle usage");
        }));
    }
    for task in tasks {
        task.await.expect("task");
    }

    assert_eq!(
        used_units(&w.pool, budget_a).await,
        per_event_units * events
    );
    assert_eq!(
        ai::exhausted_enforcing_budget(&w.pool, w.team_a.id, route_a, provider_a)
            .await
            .expect("team a enforcement"),
        None
    );
    assert_eq!(
        sqlx::query("SELECT used_units FROM ai_budget_counters WHERE budget_id = $1")
            .bind(budget_b)
            .fetch_optional(&w.pool)
            .await
            .expect("team b counter")
            .map(|row| row.get::<i64, _>("used_units")),
        None,
        "team A usage must not settle team B counters"
    );
    assert_eq!(
        ai::exhausted_enforcing_budget(&w.pool, w.team_b.id, route_b, provider_b)
            .await
            .expect("team b enforcement"),
        None,
        "team A usage must not exhaust team B budgets"
    );
}
