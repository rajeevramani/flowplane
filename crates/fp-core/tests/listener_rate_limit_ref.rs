//! S7 exit tests (fpv2-4ht.7): fail-closed listener reference validation + CP domain
//! composition for `global_rate_limit` HTTP filters.
//!
//! These are independent, black-box, adversarial integration tests written from the S7
//! acceptance criteria and the public `fp_core::services::gateway` API only — they do NOT
//! consult the listener-service implementation. They assert the tenant-binding invariant
//! end-to-end: when the built-in RLS path is used, the PERSISTED Envoy filter `domain` must
//! equal exactly `compose_domain(org, team, user_domain)` — the same key the RLS push (S5)
//! uses — and a missing/cross-team service cluster must be rejected before anything is stored.
//!
//! DB-backed; each test self-skips when `FLOWPLANE_TEST_DATABASE_URL` is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::clusters as cluster_svc;
use fp_core::services::gateway as gw;
use fp_core::services::rls_sync::compose_domain;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy, RESERVED_RATE_LIMIT_CLUSTER};
use fp_domain::gateway::filters::{
    GlobalRateLimitConfig, HttpFilterEntry, HttpFilterSpec, RateLimitRequestType,
};
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;
use std::net::{IpAddr, SocketAddr};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Pick a port unlikely to collide across tests (each test uses its own team, so the
/// per-team port-uniqueness constraint does not bite across tests, but distinct ports keep
/// failures legible).
fn port() -> u16 {
    // 1024..=65535; derive from a random nibble of a fresh uuid to spread allocations.
    let raw = uuid::Uuid::now_v7().as_u128();
    let p = 20000u32 + (raw as u32 % 40000);
    p as u16
}

fn cluster_spec(host: &str) -> ClusterSpec {
    ClusterSpec {
        aggregate_clusters: Vec::new(),
        endpoints: vec![Endpoint {
            host: host.into(),
            port: 8080,
            weight: None,
        }],
        lb_policy: LbPolicy::RoundRobin,
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 5,
        use_tls: false,
        upstream_tls: None,
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
        outlier_detection: None,
    }
}

fn hermetic_cluster_policy(
    specs: &[ClusterSpec],
) -> fp_core::services::egress_policy::EgressPolicy {
    let allowed = specs
        .iter()
        .flat_map(|spec| {
            spec.endpoints.iter().filter_map(|endpoint| {
                endpoint
                    .host
                    .parse::<IpAddr>()
                    .ok()
                    .map(|ip| SocketAddr::new(ip, endpoint.port))
            })
        })
        .collect();
    fp_core::services::egress_policy::EgressPolicy::with_allowed(Vec::new(), allowed)
}

/// A global_rate_limit filter entry with explicit domain + service_cluster.
fn grl_entry(user_domain: &str, service_cluster: &str) -> HttpFilterEntry {
    HttpFilterEntry {
        filter: HttpFilterSpec::GlobalRateLimit(GlobalRateLimitConfig {
            domain: user_domain.into(),
            service_cluster: service_cluster.into(),
            timeout_ms: 20,
            failure_mode_deny: false,
            stage: 0,
            request_type: RateLimitRequestType::Both,
            stat_prefix: None,
            enable_x_ratelimit_headers: false,
            disable_x_envoy_ratelimited_header: false,
            rate_limited_status: None,
            status_on_error: None,
        }),
        disabled: false,
    }
}

/// A minimal valid listener spec carrying the given filter chain.
fn listener_spec(filters: Vec<HttpFilterEntry>) -> ListenerSpec {
    ListenerSpec {
        address: "0.0.0.0".into(),
        port: port(),
        public_base_url: None,
        protocol: ListenerProtocol::Http,
        route_config: None,
        http_filters: filters,
        access_logs: Vec::new(),
        tls_context: None,
    }
}

/// Extract the (single) global_rate_limit filter's persisted `domain` from a Listener.
fn persisted_grl_domain(spec: &ListenerSpec) -> String {
    spec.http_filters
        .iter()
        .find_map(|entry| match &entry.filter {
            HttpFilterSpec::GlobalRateLimit(c) => Some(c.domain.clone()),
            _ => None,
        })
        .expect("listener must carry a global_rate_limit filter")
}

struct World {
    pool: PgPool,
    /// Primary team.
    team: TeamRef,
    admin: PrincipalCtx,
    /// A SECOND team in the SAME org, with its own admin.
    team2: TeamRef,
    admin2: PrincipalCtx,
    /// A team in a DIFFERENT org, with its own admin.
    other_org_team: TeamRef,
    other_admin: PrincipalCtx,
}

async fn make_admin(pool: &PgPool, org_id: fp_domain::OrgId) -> PrincipalCtx {
    let sub = unique("sub");
    let user_id = identity::upsert_user_by_subject(pool, &sub, "a@t.test", "A")
        .await
        .expect("user");
    identity::add_org_membership(pool, user_id, org_id, OrgRole::Admin)
        .await
        .expect("membership");
    PrincipalCtx::User {
        user_id,
        platform_admin: false,
        org_selector_required: false,
        org: Some((org_id, OrgRole::Admin)),
        grants: GrantSet::default(),
    }
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Org 1 with two teams.
    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_row = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };
    let team2_row = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team2");
    let team2 = TeamRef {
        id: team2_row.id,
        org_id: org.id,
    };
    let admin = make_admin(&pool, org.id).await;
    let admin2 = make_admin(&pool, org.id).await;

    // A second org with its own team + admin.
    let other_org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("other org");
    let other_team_row = identity::create_team(&pool, other_org.id, &unique("team"), "")
        .await
        .expect("other team");
    let other_org_team = TeamRef {
        id: other_team_row.id,
        org_id: other_org.id,
    };
    let other_admin = make_admin(&pool, other_org.id).await;

    Some(World {
        pool,
        team,
        admin,
        team2,
        admin2,
        other_org_team,
        other_admin,
    })
}

// ---------------------------------------------------------------------------
// AC1: Built-in path, unconfigured -> rejected (ValidationFailed), not persisted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn builtin_path_unconfigured_is_rejected_and_not_persisted() {
    let Some(w) = world().await else { return };
    let name = unique("edge");

    let err = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        listener_spec(vec![grl_entry("checkout", RESERVED_RATE_LIMIT_CLUSTER)]),
        RequestId::generate(),
        false, // rls_grpc NOT configured
    )
    .await
    .expect_err("built-in path with unconfigured RLS must be rejected");
    assert_eq!(
        err.code,
        ErrorCode::ValidationFailed,
        "fail-closed: unconfigured built-in cluster is a 400, not a silent accept"
    );

    // Must NOT be persisted: a subsequent get is NotFound.
    let get_err = gw::get_listener(&w.pool, &w.admin, w.team, &name, RequestId::generate())
        .await
        .expect_err("rejected listener must not exist");
    assert_eq!(get_err.code, ErrorCode::NotFound);

    // And it must not show up in the team's listing.
    let (listed, _total) =
        gw::list_listeners(&w.pool, &w.admin, w.team, 100, 0, RequestId::generate())
            .await
            .expect("list");
    assert!(
        !listed.iter().any(|l| l.name == name),
        "rejected listener leaked into list_listeners"
    );
}

// ---------------------------------------------------------------------------
// AC2: Built-in path, configured -> accepted AND domain composed to the tenant key.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn builtin_path_configured_composes_tenant_domain() {
    let Some(w) = world().await else { return };
    let name = unique("edge");
    let user_domain = "checkout";

    let listener = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        listener_spec(vec![grl_entry(user_domain, RESERVED_RATE_LIMIT_CLUSTER)]),
        RequestId::generate(),
        true, // rls_grpc configured -> CP injects built-in cluster
    )
    .await
    .expect("built-in path with configured RLS must be accepted");

    let expected = compose_domain(w.team.org_id, w.team.id, user_domain);
    let persisted = persisted_grl_domain(&listener.spec);
    assert_eq!(
        persisted, expected,
        "persisted Envoy domain must equal the RLS namespace key (tenant binding)"
    );
    // Tenant binding is non-trivial: the composed value must differ from the raw user input.
    assert_ne!(
        persisted, user_domain,
        "domain must be namespaced, not left as the raw user string"
    );

    // The composition survives a round-trip through storage (re-fetch, not just the returned value).
    let fetched = gw::get_listener(&w.pool, &w.admin, w.team, &name, RequestId::generate())
        .await
        .expect("fetch persisted listener");
    assert_eq!(
        persisted_grl_domain(&fetched.spec),
        expected,
        "the composed domain must be what is durably stored"
    );
}

// ---------------------------------------------------------------------------
// AC3: Two teams (same org + cross-org), same user domain -> distinct namespaces.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn same_user_domain_yields_distinct_namespaces_per_team() {
    let Some(w) = world().await else { return };
    let user_domain = "checkout";

    let name1 = unique("edge");
    let l1 = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name1,
        listener_spec(vec![grl_entry(user_domain, RESERVED_RATE_LIMIT_CLUSTER)]),
        RequestId::generate(),
        true,
    )
    .await
    .expect("team1 listener");

    // Same user domain, DIFFERENT team in the SAME org.
    let name2 = unique("edge");
    let l2 = gw::create_listener(
        &w.pool,
        &w.admin2,
        w.team2,
        &name2,
        listener_spec(vec![grl_entry(user_domain, RESERVED_RATE_LIMIT_CLUSTER)]),
        RequestId::generate(),
        true,
    )
    .await
    .expect("team2 listener");

    // Same user domain, DIFFERENT org entirely.
    let name3 = unique("edge");
    let l3 = gw::create_listener(
        &w.pool,
        &w.other_admin,
        w.other_org_team,
        &name3,
        listener_spec(vec![grl_entry(user_domain, RESERVED_RATE_LIMIT_CLUSTER)]),
        RequestId::generate(),
        true,
    )
    .await
    .expect("other-org listener");

    let d1 = persisted_grl_domain(&l1.spec);
    let d2 = persisted_grl_domain(&l2.spec);
    let d3 = persisted_grl_domain(&l3.spec);

    assert_ne!(
        d1, d2,
        "two teams in the same org must get different namespaces for the same user domain"
    );
    assert_ne!(d1, d3, "different org must get a different namespace");
    assert_ne!(d2, d3, "different org/team must get a different namespace");

    // Each must equal exactly its own composed value (no accidental swaps).
    assert_eq!(d1, compose_domain(w.team.org_id, w.team.id, user_domain));
    assert_eq!(d2, compose_domain(w.team2.org_id, w.team2.id, user_domain));
    assert_eq!(
        d3,
        compose_domain(w.other_org_team.org_id, w.other_org_team.id, user_domain)
    );
}

// ---------------------------------------------------------------------------
// AC4: User-supplied cluster that EXISTS in this team -> accepted, domain verbatim.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn user_cluster_in_team_is_accepted_with_verbatim_domain() {
    let Some(w) = world().await else { return };

    let cluster_name = unique("my-rls");
    let cluster = cluster_spec("10.0.0.5");
    cluster_svc::create_cluster_with_egress_policy(
        &w.pool,
        &w.admin,
        w.team,
        &cluster_name,
        cluster.clone(),
        RequestId::generate(),
        &hermetic_cluster_policy(&[cluster]),
    )
    .await
    .expect("create user rls cluster");

    let user_domain = "checkout";
    let name = unique("edge");
    // rls_grpc DELIBERATELY false: a user-supplied existing cluster does not depend on the
    // built-in RLS being configured.
    let listener = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        listener_spec(vec![grl_entry(user_domain, &cluster_name)]),
        RequestId::generate(),
        false,
    )
    .await
    .expect("user-cluster path must be accepted regardless of rls_grpc_configured");

    let persisted = persisted_grl_domain(&listener.spec);
    assert_eq!(
        persisted, user_domain,
        "a user-supplied cluster leaves the domain VERBATIM — no CP namespacing"
    );
    // Guard against accidental composition: it must NOT equal the composed value.
    assert_ne!(
        persisted,
        compose_domain(w.team.org_id, w.team.id, user_domain),
        "user-cluster path must not compose the tenant namespace"
    );
}

// ---------------------------------------------------------------------------
// AC5: User-supplied cluster missing / in another team -> rejected (NotFound), not persisted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn user_cluster_absent_or_cross_team_is_rejected_not_found() {
    let Some(w) = world().await else { return };

    // (a) A cluster that exists in NO team.
    let ghost = unique("ghost-rls");
    let name_a = unique("edge");
    let err = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name_a,
        listener_spec(vec![grl_entry("checkout", &ghost)]),
        RequestId::generate(),
        true, // even with RLS configured, a named non-existent cluster is a hard NotFound
    )
    .await
    .expect_err("non-existent service cluster must be rejected");
    assert_eq!(
        err.code,
        ErrorCode::NotFound,
        "missing referenced cluster is a 404"
    );
    let get_err = gw::get_listener(&w.pool, &w.admin, w.team, &name_a, RequestId::generate())
        .await
        .expect_err("rejected listener must not exist");
    assert_eq!(get_err.code, ErrorCode::NotFound);

    // (b) A cluster that exists, but only in a DIFFERENT team (team2). Referencing it from
    // team1 must NOT resolve — no cross-team cluster reuse.
    let foreign_cluster = unique("foreign-rls");
    let cluster = cluster_spec("10.0.0.9");
    cluster_svc::create_cluster_with_egress_policy(
        &w.pool,
        &w.admin2,
        w.team2,
        &foreign_cluster,
        cluster.clone(),
        RequestId::generate(),
        &hermetic_cluster_policy(&[cluster]),
    )
    .await
    .expect("create cluster in team2");

    let name_b = unique("edge");
    let err = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team, // team1 referencing team2's cluster
        &name_b,
        listener_spec(vec![grl_entry("checkout", &foreign_cluster)]),
        RequestId::generate(),
        false,
    )
    .await
    .expect_err("cross-team cluster reference must be rejected");
    assert_eq!(
        err.code,
        ErrorCode::NotFound,
        "a cluster owned by another team must not resolve for this team"
    );
    let get_err = gw::get_listener(&w.pool, &w.admin, w.team, &name_b, RequestId::generate())
        .await
        .expect_err("rejected listener must not exist");
    assert_eq!(get_err.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------
// AC6: Update path is enforced identically (not silently exempt).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn update_path_enforces_builtin_rls_gate_and_composes() {
    let Some(w) = world().await else { return };
    let name = unique("edge");

    // Start from a valid listener with NO rate-limit filter.
    let created = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        listener_spec(Vec::new()),
        RequestId::generate(),
        false,
    )
    .await
    .expect("create plain listener");
    assert_eq!(created.version, 1);

    // Build an update spec adding the built-in-path rate-limit filter, but keep the SAME
    // address/port as the created listener (avoid spurious port-collision noise).
    let mut update_spec = created.spec.clone();
    update_spec.http_filters = vec![grl_entry("checkout", RESERVED_RATE_LIMIT_CLUSTER)];

    // Update with RLS unconfigured -> must be rejected (the gate is not update-exempt).
    let err = gw::update_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        update_spec.clone(),
        1,
        RequestId::generate(),
        false,
    )
    .await
    .expect_err("update into the built-in path with unconfigured RLS must be rejected");
    assert_eq!(err.code, ErrorCode::ValidationFailed);

    // The rejected update must not have mutated the stored listener: still version 1, no filter.
    let after_reject = gw::get_listener(&w.pool, &w.admin, w.team, &name, RequestId::generate())
        .await
        .expect("listener still exists");
    assert_eq!(
        after_reject.version, 1,
        "a rejected update must not bump the version"
    );
    assert!(
        after_reject.spec.http_filters.is_empty(),
        "a rejected update must not persist the rate-limit filter"
    );

    // Update with RLS configured -> accepted AND domain composed.
    let updated = gw::update_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        update_spec,
        1,
        RequestId::generate(),
        true,
    )
    .await
    .expect("update with configured RLS must be accepted");
    assert_eq!(updated.version, 2, "accepted update bumps the version");
    assert_eq!(
        persisted_grl_domain(&updated.spec),
        compose_domain(w.team.org_id, w.team.id, "checkout"),
        "the update path composes the tenant domain identically to create"
    );
}

// ---------------------------------------------------------------------------
// AC7: A listener with NO global_rate_limit filter is unaffected by rls_grpc_configured.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn listener_without_rate_limit_filter_ignores_rls_flag() {
    let Some(w) = world().await else { return };

    // rls unconfigured.
    let name_a = unique("plain");
    gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name_a,
        listener_spec(Vec::new()),
        RequestId::generate(),
        false,
    )
    .await
    .expect("plain listener accepted with rls unconfigured");

    // rls configured.
    let name_b = unique("plain");
    gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name_b,
        listener_spec(Vec::new()),
        RequestId::generate(),
        true,
    )
    .await
    .expect("plain listener accepted with rls configured");
}

// ---------------------------------------------------------------------------
// AC8: Read-modify-write round-trip does NOT double-compose the domain.
// Feeding the already-COMPOSED spec back through update must keep the domain at exactly
// compose_domain(org, team, user_domain) — composition is idempotent across a GET→PATCH cycle.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn round_trip_update_does_not_double_compose() {
    let Some(w) = world().await else { return };
    let name = unique("edge");
    let user_domain = "checkout";
    let expected = compose_domain(w.team.org_id, w.team.id, user_domain);

    let created = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        listener_spec(vec![grl_entry(user_domain, RESERVED_RATE_LIMIT_CLUSTER)]),
        RequestId::generate(),
        true,
    )
    .await
    .expect("built-in path listener created");
    // Precondition: the created spec already carries the composed (namespaced) domain.
    assert_eq!(persisted_grl_domain(&created.spec), expected);

    // Feed the RETURNED spec back UNCHANGED — it now contains the composed domain string, so a
    // naive re-composition would produce compose_domain(org, team, "<org>|<team>|checkout").
    let updated = gw::update_listener(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        created.spec.clone(),
        created.version,
        RequestId::generate(),
        true,
    )
    .await
    .expect("re-applying the composed spec must be accepted");

    let after = persisted_grl_domain(&updated.spec);
    assert_eq!(
        after, expected,
        "round-trip update must remain singly-composed, not double-namespaced"
    );
    // Explicitly assert it did NOT double-compose.
    let double = compose_domain(w.team.org_id, w.team.id, &expected);
    assert_ne!(
        after, double,
        "domain must not be re-namespaced on read-modify-write"
    );

    // Durable check: re-fetch confirms the stored value too.
    let fetched = gw::get_listener(&w.pool, &w.admin, w.team, &name, RequestId::generate())
        .await
        .expect("fetch after round-trip");
    assert_eq!(persisted_grl_domain(&fetched.spec), expected);
}
