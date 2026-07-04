//! Shadow-budget read-only evaluation query (feature ai-gateway-e2e-trace, slice s3).
//!
//! Parallel-safe: every test creates its own uniquely named org/team and its own budget
//! rows; nothing assumes global row counts.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::{AiProviderId, RouteConfigId, TeamId};
use fp_storage::repos::{ai, identity};
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    org_id: Uuid,
    team_a: TeamId,
    team_b: TeamId,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("org-shadow"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-shadow-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-shadow-b"), "")
        .await
        .expect("team b");
    Some(World {
        pool,
        org_id: org.id.as_uuid(),
        team_a: team_a.id,
        team_b: team_b.id,
    })
}

/// Insert a real ai_providers row (with its credential secret) so budgets can scope to it.
async fn seed_provider(w: &World, team: TeamId) -> Uuid {
    let secret_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets \
         (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
         VALUES ($1, $2, $3, $4, '', 'generic_secret', $5, $6, 'default')",
    )
    .bind(secret_id)
    .bind(team.as_uuid())
    .bind(w.org_id)
    .bind(unique("ai-key"))
    .bind(Vec::<u8>::from([1_u8]))
    .bind(Vec::<u8>::from([2_u8; 12]))
    .execute(&w.pool)
    .await
    .expect("secret");

    let provider_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO ai_providers \
         (id, team_id, org_id, name, kind, base_url, credential_secret_id, auth_header) \
         VALUES ($1, $2, $3, $4, 'openai', 'https://api.openai.example', $5, 'authorization')",
    )
    .bind(provider_id)
    .bind(team.as_uuid())
    .bind(w.org_id)
    .bind(unique("provider"))
    .bind(secret_id)
    .execute(&w.pool)
    .await
    .expect("provider");
    provider_id
}

/// Insert a real route_configs row so budgets can scope to it.
async fn seed_route_config(w: &World, team: TeamId) -> Uuid {
    let route_config_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb)",
    )
    .bind(route_config_id)
    .bind(team.as_uuid())
    .bind(w.org_id)
    .bind(unique("route-config"))
    .execute(&w.pool)
    .await
    .expect("route config");
    route_config_id
}

/// Insert an ai_budgets row directly (NULL provider/route scope unless given) with an
/// optional current-window counter.
#[allow(clippy::too_many_arguments)]
async fn seed_budget(
    w: &World,
    team: TeamId,
    name: &str,
    mode: &str,
    limit_units: i64,
    provider_id: Option<Uuid>,
    route_config_id: Option<Uuid>,
    used_units: Option<i64>,
) {
    let budget_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO ai_budgets \
         (id, team_id, org_id, name, mode, limit_units, window_seconds, provider_id, route_config_id, prompt_token_weight, completion_token_weight) \
         VALUES ($1, $2, $3, $4, $5, $6, 3600, $7, $8, 1, 1)",
    )
    .bind(budget_id)
    .bind(team.as_uuid())
    .bind(w.org_id)
    .bind(name)
    .bind(mode)
    .bind(limit_units)
    .bind(provider_id)
    .bind(route_config_id)
    .execute(&w.pool)
    .await
    .expect("budget row");
    if let Some(used) = used_units {
        sqlx::query(
            "INSERT INTO ai_budget_counters (budget_id, team_id, window_start, used_units) \
             VALUES ($1, $2, to_timestamp(floor(extract(epoch FROM now()) / 3600) * 3600), $3)",
        )
        .bind(budget_id)
        .bind(team.as_uuid())
        .bind(used)
        .execute(&w.pool)
        .await
        .expect("counter row");
    }
}

#[tokio::test]
async fn evaluate_shadow_budgets_reports_matching_shadow_standings_read_only() {
    let Some(w) = world().await else { return };
    let provider_id = seed_provider(&w, w.team_a).await;
    let other_provider = seed_provider(&w, w.team_a).await;
    let route_config_id = Uuid::now_v7();

    // Exhausted team-wide shadow budget (NULL scope matches everything).
    seed_budget(
        &w,
        w.team_a,
        "shadow-over",
        "shadow",
        5,
        None,
        None,
        Some(9),
    )
    .await;
    // Shadow budget with headroom, provider-scoped to the queried provider.
    seed_budget(
        &w,
        w.team_a,
        "shadow-under",
        "shadow",
        100,
        Some(provider_id),
        None,
        Some(3),
    )
    .await;
    // Shadow budget with no counter yet: used_units must read as 0, not be dropped.
    seed_budget(&w, w.team_a, "shadow-unused", "shadow", 7, None, None, None).await;
    // Exhausted ENFORCING budget: never part of the shadow evaluation.
    seed_budget(
        &w,
        w.team_a,
        "enforcing-over",
        "enforcing",
        1,
        None,
        None,
        Some(5),
    )
    .await;
    // Shadow budget scoped to a different provider: filtered out.
    seed_budget(
        &w,
        w.team_a,
        "shadow-other-provider",
        "shadow",
        5,
        Some(other_provider),
        None,
        Some(9),
    )
    .await;
    // Another team's exhausted shadow budget: invisible.
    seed_budget(
        &w,
        w.team_b,
        "shadow-b-over",
        "shadow",
        1,
        None,
        None,
        Some(5),
    )
    .await;

    let evaluations = ai::evaluate_shadow_budgets(
        &w.pool,
        w.team_a,
        RouteConfigId::from(route_config_id),
        AiProviderId::from(provider_id),
    )
    .await
    .expect("shadow evaluation");
    assert_eq!(
        evaluations,
        vec![
            ai::ShadowBudgetEvaluation {
                name: "shadow-over".into(),
                used_units: 9,
                limit_units: 5,
            },
            ai::ShadowBudgetEvaluation {
                name: "shadow-under".into(),
                used_units: 3,
                limit_units: 100,
            },
            ai::ShadowBudgetEvaluation {
                name: "shadow-unused".into(),
                used_units: 0,
                limit_units: 7,
            },
        ],
        "matching shadow budgets in name order, enforcing/foreign/other-scope excluded"
    );

    // Read-only: evaluation must not create or mutate any counter row.
    let counters: Vec<(String, i64)> = sqlx::query_as(
        "SELECT b.name, c.used_units FROM ai_budget_counters c \
         JOIN ai_budgets b ON b.id = c.budget_id WHERE c.team_id = $1 ORDER BY b.name",
    )
    .bind(w.team_a.as_uuid())
    .fetch_all(&w.pool)
    .await
    .expect("counters");
    assert_eq!(
        counters,
        vec![
            ("enforcing-over".to_string(), 5),
            ("shadow-other-provider".to_string(), 9),
            ("shadow-over".to_string(), 9),
            ("shadow-under".to_string(), 3),
        ],
        "counters unchanged and none created by the read-only evaluation"
    );
}

#[tokio::test]
async fn evaluate_shadow_budgets_honors_route_config_scope() {
    let Some(w) = world().await else { return };
    let provider_id = Uuid::now_v7();
    let route_config_id = seed_route_config(&w, w.team_a).await;
    let other_route_config = seed_route_config(&w, w.team_a).await;

    seed_budget(
        &w,
        w.team_a,
        "shadow-this-route",
        "shadow",
        5,
        None,
        Some(route_config_id),
        Some(6),
    )
    .await;
    seed_budget(
        &w,
        w.team_a,
        "shadow-other-route",
        "shadow",
        5,
        None,
        Some(other_route_config),
        Some(6),
    )
    .await;

    let evaluations = ai::evaluate_shadow_budgets(
        &w.pool,
        w.team_a,
        RouteConfigId::from(route_config_id),
        AiProviderId::from(provider_id),
    )
    .await
    .expect("shadow evaluation");
    assert_eq!(
        evaluations,
        vec![ai::ShadowBudgetEvaluation {
            name: "shadow-this-route".into(),
            used_units: 6,
            limit_units: 5,
        }],
        "route-config-scoped shadow budgets match only their own route config"
    );
}
