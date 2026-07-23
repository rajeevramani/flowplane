//! Acceptance criterion 1 of the grant referential-integrity feature, asserted through the
//! **product path** rather than the schema:
//!
//!   Removing a user from an org deletes that user's grants for that org, verified by querying
//!   `user_grants` after `DELETE /api/v1/orgs/{org}/members/{user}`, and the user's grants in a
//!   *different* org are untouched.
//!
//! A storage-level test already proves the foreign key cascades. That is necessary but not
//! sufficient: it says nothing about whether authorization, org resolution, the last-owner
//! guard and the service mutation actually reach the cascade. This test drives the real HTTP
//! endpoint through the real router with a real bearer token, so a regression that broke the
//! route, the authz, or the service wiring would fail here even with the FK intact.
//!
//! Parallel-safe (constitution invariant 18): every org, team and user is uuid-suffixed and
//! unique to the test; every assertion is scoped to this test's own rows — never a global
//! count; no fixed port. Skipped (with a notice) when FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::identity;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ORG_SELECTOR_HEADER: &str = "x-flowplane-org";

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct Env {
    app: axum::Router,
    issuer: DevIssuer,
    pool: PgPool,
}

async fn env() -> Option<Env> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    Some(Env { app, issuer, pool })
}

/// Create a user, add them to `org_id` with `role`, and mint a bearer token for them.
async fn user_with_org_role(env: &Env, org_id: OrgId, role: OrgRole) -> (UserId, String) {
    let subject = unique("sub");
    let email = format!("{}@test", unique("user"));
    let user = identity::upsert_user_by_subject(&env.pool, &subject, &email, "Test User")
        .await
        .expect("user");
    identity::add_org_membership(&env.pool, user, org_id, role)
        .await
        .expect("org membership");
    let token = env
        .issuer
        .mint(&subject, &email, "Test User", 600)
        .expect("mint");
    (user, token)
}

/// Count this user's grants in this org. Always scoped to the fixture's own rows.
async fn grants_in_org(pool: &PgPool, user: UserId, org: OrgId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM user_grants WHERE user_id = $1 AND org_id = $2")
        .bind(user.as_uuid())
        .bind(org.as_uuid())
        .fetch_one(pool)
        .await
        .expect("count grants")
}

async fn grant_on_team(pool: &PgPool, user: UserId, org: OrgId, team: TeamId) {
    identity::add_grant(
        pool,
        user,
        org,
        team,
        Resource::Clusters,
        Action::Read,
        None,
    )
    .await
    .expect("grant");
}

#[tokio::test]
async fn removing_an_org_member_revokes_their_grants_in_that_org_only() {
    let Some(env) = env().await else { return };

    // Two independent orgs; the victim is a member of both and holds one grant in each.
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&env.pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&env.pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("team b");

    let (victim, _) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;
    identity::add_org_membership(&env.pool, victim, org_b.id, OrgRole::Member)
        .await
        .expect("second membership");
    grant_on_team(&env.pool, victim, org_a.id, team_a.id).await;
    grant_on_team(&env.pool, victim, org_b.id, team_b.id).await;

    // An owner of org A performs the removal — the real authorization path, not a raw delete.
    let (_, admin_token) = user_with_org_role(&env, org_a.id, OrgRole::Owner).await;

    assert_eq!(grants_in_org(&env.pool, victim, org_a.id).await, 1);
    assert_eq!(grants_in_org(&env.pool, victim, org_b.id).await, 1);

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/orgs/{}/members/{}",
                    org_a.id.as_uuid(),
                    victim.as_uuid()
                ))
                .header("authorization", format!("Bearer {admin_token}"))
                .header(ORG_SELECTOR_HEADER, org_a.id.as_uuid().to_string())
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "org owner must be able to remove a member of their own org"
    );

    assert_eq!(
        grants_in_org(&env.pool, victim, org_a.id).await,
        0,
        "removing the membership through the API must revoke that org's grants"
    );
    assert_eq!(
        grants_in_org(&env.pool, victim, org_b.id).await,
        1,
        "grants in a DIFFERENT org must be untouched — the cascade must not over-delete"
    );

    // The membership itself is gone, so the revocation is not an artefact of a failed call.
    let memberships: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_memberships WHERE user_id = $1 AND org_id = $2",
    )
    .bind(victim.as_uuid())
    .bind(org_a.id.as_uuid())
    .fetch_one(&env.pool)
    .await
    .expect("count memberships");
    assert_eq!(memberships, 0, "membership must be gone");

    // And the audit trail records WHY the authority disappeared. FP-DEC-0016 rests on this row
    // being the evidence for the cascaded revocation, so its absence would invalidate the
    // decision, not merely reduce observability.
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'org.member.remove' \
         AND org_id = $1 AND resource = $2",
    )
    .bind(org_a.id.as_uuid())
    .bind(format!("users/{victim}"))
    .fetch_one(&env.pool)
    .await
    .expect("count audit");
    assert_eq!(
        audited, 1,
        "the removal must be audited exactly once for this victim in this org"
    );
}
