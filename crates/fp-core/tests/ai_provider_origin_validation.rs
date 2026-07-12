//! fpv2-ti2.1 integration coverage: the AI provider `base_url` is validated as
//! a canonical ORIGIN (scheme + host + optional port, nothing else) at both
//! provider create and provider update through the fp-core service layer.
//!
//! - Non-origin inputs (userinfo, path, query, fragment, bad port, non-http(s)
//!   scheme, missing host) are REJECTED with a validation error and leave no
//!   row created / no row updated.
//! - Origin inputs (bare, trailing slash, explicit default port, non-default
//!   port) are ACCEPTED.
//! - Canonicalization is observable in materialized state: a percent-encoded
//!   host spelling in base_url materializes the DECODED canonical host into the
//!   backend cluster's endpoint host and TLS SNI at ROUTE create.
//!
//! Black-box tests written spec-first from the acceptance criteria: they drive
//! `fp_core::services::ai` and observe only the `ai_providers` / `clusters`
//! tables and the service read APIs. Unique org/team/provider/route/secret
//! names (uuid suffix) keep every test parallel-safe against siblings sharing
//! the database. Egress-advisory: `Default::default()` policy = bypass (same
//! as the other AI integration suites), so example/test hosts never hit
//! DNS-resolution rejections.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::ai as ai_svc;
use fp_core::services::secrets::{self as secret_svc, SecretWrite};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{
    AiProvider, AiProviderKind, AiProviderSpec, AiRoute, AiRouteBackend, AiRouteSpec, ErrorCode,
    OrgRole, RequestId, SecretId, SecretSpec,
};
use fp_storage::repos::identity;
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Random listener port. Listener port uniqueness is per team and every test
/// gets a fresh team, so this only needs to avoid collisions within one test.
fn unique_port() -> u16 {
    let b = uuid::Uuid::now_v7().into_bytes();
    20000 + (u16::from_be_bytes([b[14], b[15]]) % 40000)
}

struct World {
    pool: PgPool,
    team: TeamRef,
    admin: PrincipalCtx,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("org-ai-origin"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team-ai-origin"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "admin@example.test", "A")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership");
    Some(World {
        pool,
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
        admin: PrincipalCtx::User {
            user_id: user,
            platform_admin: false,
            org_selector_required: false,
            org: Some((org.id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
    })
}

async fn create_secret(w: &World) -> SecretId {
    let name = unique("ai-key");
    secret_svc::create_secret(
        &w.pool,
        &w.admin,
        w.team,
        SecretWrite {
            name: &name,
            description: "",
            spec: SecretSpec::GenericSecret {
                secret: "b3JpZ2luLXRlc3Q=".into(),
            },
            expires_at: None,
        },
        RequestId::generate(),
    )
    .await
    .expect("create secret")
    .id
}

fn provider_spec(base_url: &str, secret: SecretId) -> AiProviderSpec {
    AiProviderSpec {
        kind: AiProviderKind::OpenaiCompatible,
        base_url: base_url.into(),
        path_prefix: Some("/v1".into()),
        credential_secret_id: secret,
        models: vec!["gpt-5".into()],
        auth_header: "authorization".into(),
    }
}

async fn create_provider(w: &World, prefix: &str, base_url: &str, secret: SecretId) -> AiProvider {
    ai_svc::create_provider(
        &w.pool,
        &w.admin,
        w.team,
        &unique(prefix),
        provider_spec(base_url, secret),
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("create provider")
}

fn backend(provider: &AiProvider) -> AiRouteBackend {
    AiRouteBackend {
        provider_id: provider.id,
        models: vec!["gpt-5".into()],
        model_override: None,
        weight: 1,
        priority: 0,
    }
}

async fn create_route(
    w: &World,
    prefix: &str,
    port: u16,
    backends: Vec<AiRouteBackend>,
) -> AiRoute {
    ai_svc::create_route(
        &w.pool,
        &w.admin,
        w.team,
        &unique(prefix),
        AiRouteSpec {
            listener_port: port,
            path: "/v1/chat/completions".into(),
            backends,
        },
        RequestId::generate(),
    )
    .await
    .expect("create route")
}

/// Provider rows of this team under one exact name — straight from the table,
/// so "no row created" is asserted against storage, not a read API.
async fn provider_row_count(w: &World, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ai_providers WHERE team_id = $1 AND name = $2")
        .bind(w.team.id.as_uuid())
        .bind(name)
        .fetch_one(&w.pool)
        .await
        .expect("ai_providers row count")
}

/// `(base_url, version)` of one provider row, straight from the table.
async fn provider_row(w: &World, name: &str) -> (String, i64) {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT base_url, version FROM ai_providers WHERE team_id = $1 AND name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(name)
    .fetch_one(&w.pool)
    .await
    .expect("ai_providers row")
}

/// Fetch the spec JSON for one cluster row of this team.
async fn cluster_spec(w: &World, name: &str) -> serde_json::Value {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT spec FROM clusters WHERE team_id = $1 AND name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(name)
    .fetch_one(&w.pool)
    .await
    .expect("cluster spec row")
}

/// The non-origin inputs the spec rejects, each with a label for diagnostics.
/// Hosts get a per-call unique spelling where a real host is present so no two
/// tests (or loop iterations) ever share a provider host.
fn rejected_base_urls(host: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "userinfo (credentials belong in the credential secret)",
            format!("https://user:pw@{host}"),
        ),
        ("unparseable port", format!("https://{host}:abc")),
        (
            "path (paths belong in path_prefix)",
            format!("https://{host}/api/v1"),
        ),
        ("query", format!("https://{host}?x=1")),
        ("fragment", format!("https://{host}#f")),
        ("non-http(s) scheme", format!("ftp://{host}")),
        ("missing host", "https://".to_string()),
    ]
}

// AC1 (rejections): every non-origin base_url is rejected with a validation
// error at CREATE (no ai_providers row appears) and at UPDATE (the existing
// row keeps its base_url and version).
#[tokio::test]
async fn non_origin_base_urls_are_rejected_at_create_and_update() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let good_host = format!("{}.example", unique("ai-orig"));
    let good_url = format!("https://{good_host}");

    // An existing, valid provider — the target of the update-rejection half.
    // Capture its STORED row up front so "not updated" is asserted against the
    // exact persisted (possibly canonicalized) form, not the input spelling.
    let existing = create_provider(&w, "prov-upd", &good_url, secret).await;
    let stored_before = provider_row(&w, &existing.name).await;

    for (label, bad_url) in rejected_base_urls(&format!("{}.example", unique("ai-bad"))) {
        // CREATE rejection: validation error, no row created.
        let doomed_name = unique("prov-bad");
        let err = ai_svc::create_provider(
            &w.pool,
            &w.admin,
            w.team,
            &doomed_name,
            provider_spec(&bad_url, secret),
            RequestId::generate(),
            Default::default(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("create must reject {label}: {bad_url}"));
        assert_eq!(
            err.code,
            ErrorCode::ValidationFailed,
            "{label} ({bad_url}) must fail create with ValidationFailed, got: {err}"
        );
        assert_eq!(
            provider_row_count(&w, &doomed_name).await,
            0,
            "{label} ({bad_url}): no ai_providers row created by the rejected create"
        );

        // UPDATE rejection: validation error, row untouched (base_url AND
        // version — a version bump would be a partial update).
        let err = ai_svc::update_provider(
            &w.pool,
            &w.admin,
            w.team,
            &existing.name,
            provider_spec(&bad_url, secret),
            existing.version,
            RequestId::generate(),
            Default::default(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("update must reject {label}: {bad_url}"));
        assert_eq!(
            err.code,
            ErrorCode::ValidationFailed,
            "{label} ({bad_url}) must fail update with ValidationFailed, got: {err}"
        );
        assert_eq!(
            provider_row(&w, &existing.name).await,
            stored_before,
            "{label} ({bad_url}): existing row (base_url, version) unchanged after rejected update"
        );
    }
}

// AC2 (accepted forms): canonical-origin base_urls are accepted — bare origin,
// trailing slash, explicit default port, and non-default port.
#[tokio::test]
async fn origin_base_urls_are_accepted() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host = format!("{}.example", unique("ai-ok"));

    for (label, url) in [
        ("bare origin", format!("https://{host}")),
        ("trailing slash", format!("https://{host}/")),
        ("explicit default port", format!("https://{host}:443")),
        ("non-default port", format!("https://{host}:8443")),
    ] {
        let provider = ai_svc::create_provider(
            &w.pool,
            &w.admin,
            w.team,
            &unique("prov-ok"),
            provider_spec(&url, secret),
            RequestId::generate(),
            Default::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("{label} ({url}) must be accepted, got: {e}"));
        assert_eq!(provider_row_count(&w, &provider.name).await, 1);

        // The accepted origin must also be accepted as an UPDATE target.
        ai_svc::update_provider(
            &w.pool,
            &w.admin,
            w.team,
            &provider.name,
            provider_spec(&url, secret),
            provider.version,
            RequestId::generate(),
            Default::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("{label} ({url}) must be accepted at update, got: {e}"));
    }

    // "https://host" and "https://host/" are the SAME canonical origin. The
    // spec makes canonicalization observable in MATERIALIZED state (the stored
    // base_url may keep the input spelling), so assert equivalence there: a
    // route on each provider materializes byte-identical backend cluster specs.
    let bare = create_provider(&w, "prov-eq-a", &format!("https://{host}"), secret).await;
    let slash = create_provider(&w, "prov-eq-b", &format!("https://{host}/"), secret).await;
    let port = unique_port();
    let bare_route = create_route(&w, "route-eq-a", port, vec![backend(&bare)]).await;
    let slash_route = create_route(&w, "route-eq-b", port + 1, vec![backend(&slash)]).await;
    let bare_spec = cluster_spec(&w, &bare_route.materialized.cluster_names[0]).await;
    let slash_spec = cluster_spec(&w, &slash_route.materialized.cluster_names[0]).await;
    assert_eq!(
        bare_spec, slash_spec,
        "bare and trailing-slash spellings materialize the same canonical cluster spec"
    );
    assert_eq!(
        bare_spec["endpoints"][0]["host"].as_str().expect("host"),
        host,
        "both materialize the canonical host"
    );
}

// AC3 (canonicalization is visible in materialized state): a percent-encoded
// host spelling ("https://%65xample-… " -> "example-…") is accepted, and when
// an AI route uses the provider, the materialized backend cluster's endpoint
// host AND TLS SNI carry the DECODED canonical host — the percent-encoded
// spelling appears nowhere in the cluster spec.
#[tokio::test]
async fn canonical_host_reaches_materialized_cluster_endpoint_and_sni() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let suffix = unique("host");
    let canonical_host = format!("example-{suffix}.test");
    let encoded_spelling = format!("%65xample-{suffix}.test");

    let provider = create_provider(
        &w,
        "prov-canon",
        &format!("https://{encoded_spelling}"),
        secret,
    )
    .await;
    let route = create_route(&w, "route-canon", unique_port(), vec![backend(&provider)]).await;
    assert_eq!(
        route.materialized.cluster_names.len(),
        1,
        "one backend materializes one backend cluster"
    );

    let spec = cluster_spec(&w, &route.materialized.cluster_names[0]).await;
    assert_eq!(
        spec["endpoints"][0]["host"]
            .as_str()
            .expect("endpoint host"),
        canonical_host,
        "materialized endpoint host is the decoded canonical host"
    );
    assert_eq!(spec["use_tls"], serde_json::json!(true), "https => TLS on");
    assert_eq!(
        spec["upstream_tls"]["sni"].as_str().expect("TLS SNI"),
        canonical_host,
        "materialized TLS SNI is the decoded canonical host"
    );
    assert!(
        !serde_json::to_string(&spec)
            .expect("serialize")
            .contains("%65"),
        "the percent-encoded spelling leaks nowhere into the cluster spec"
    );
}
