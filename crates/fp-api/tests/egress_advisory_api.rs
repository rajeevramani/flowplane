//! Black-box integration tests for the write-time egress advisory (FP-DEC-0008).
//!
//! Spec under test: when `AppState.egress_advisory` is ENABLED, cluster create/update
//! requests whose endpoint hosts resolve to the cloud metadata endpoint, loopback, a
//! policy-denied CIDR, or a policy-denied address are REJECTED with a 4xx validation
//! error, the mutation is not persisted, and a rejection audit row
//! (action = "egress_advisory.denied", outcome = denied, detail.class =
//! "egress_advisory_denied") is written anyway. Arbitrary public and tenant-private
//! hosts are accepted — there is no blanket private-range denial. When the policy is
//! `Default` (disabled), the advisory is bypassed entirely.
//!
//! These tests were written from the acceptance criteria without reading the
//! implementation. They exercise the real router + middleware + Postgres, and assert
//! audit rows via direct SQL scoped by unique cluster names (parallel-safe).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_core::services::egress_advisory::{Cidr, EgressAdvisoryPolicy};
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::IpAddr;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("rejection body must be JSON")
}

/// The policy used by every enabled-advisory test: denies one explicit address
/// (203.0.113.9, TEST-NET-3) and one CIDR (the CGNAT range 100.64.0.0/10). Everything
/// the advisory must reject unconditionally (metadata endpoint, loopback) is NOT listed
/// here — those denials must come from the advisory itself, not this test's policy.
fn enabled_policy() -> EgressAdvisoryPolicy {
    EgressAdvisoryPolicy::new(
        true,
        vec!["203.0.113.9".parse::<IpAddr>().expect("denied addr")],
        vec!["100.64.0.0/10".parse::<Cidr>().expect("denied cidr")],
    )
}

struct Ctx {
    app: axum::Router,
    pool: sqlx::PgPool,
    token: String,
    team_name: String,
    team_id: uuid::Uuid,
    org_id: uuid::Uuid,
}

/// Full production-path setup mirroring api_crud.rs: real RS256 dev-issuer token, real
/// OIDC validation, fresh org/team per test. Returns None (skip) when the shared test
/// database is not configured.
async fn setup(egress_advisory: EgressAdvisoryPolicy) -> Option<Ctx> {
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
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "egress@test", "Egress", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "egress@test", "Egress")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let query_pool = pool.clone();
    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory,
        rls_repush: None,
        rls_grpc_configured: false,
    });

    Some(Ctx {
        app,
        pool: query_pool,
        token,
        team_name: team.name.clone(),
        team_id: team.id.as_uuid(),
        org_id: org.id.as_uuid(),
    })
}

impl Ctx {
    fn clusters_base(&self) -> String {
        format!("/api/v1/teams/{}/clusters", self.team_name)
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        revision: Option<i64>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {}", self.token));
        if let Some(revision) = revision {
            builder = builder.header("if-match", revision.to_string());
        }
        let request = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(Body::from(json.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("request");
        self.app.clone().oneshot(request).await.expect("response")
    }

    /// POST a cluster whose single endpoint is `host`. Returns the response.
    async fn create_cluster(&self, name: &str, host: &str) -> axum::response::Response {
        self.send(
            "POST",
            &self.clusters_base(),
            Some(serde_json::json!({
                "name": name,
                "spec": {"endpoints": [{"host": host, "port": 443}]}
            })),
            None,
        )
        .await
    }

    /// The cluster must not exist: item GET is 404 and the list omits it.
    async fn assert_cluster_absent(&self, name: &str) {
        let response = self
            .send(
                "GET",
                &format!("{}/{name}", self.clusters_base()),
                None,
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "rejected cluster {name} must not be retrievable"
        );

        let response = self.send("GET", &self.clusters_base(), None, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_of(response).await;
        let items = body["items"].as_array().expect("items array");
        assert!(
            !items.iter().any(|item| item["name"] == name),
            "rejected cluster {name} must not appear in the list"
        );
    }

    /// Exactly one rejection audit row must exist for this cluster, with the contract
    /// fields: action egress_advisory.denied, outcome denied, resource mentioning the
    /// cluster name, the caller's org/team ids, and detail.class set. Returns detail.
    async fn assert_denied_audit(&self, cluster: &str) -> serde_json::Value {
        let rows: Vec<(
            String,
            Option<uuid::Uuid>,
            Option<uuid::Uuid>,
            serde_json::Value,
        )> = sqlx::query_as(
            "SELECT resource, org_id, team_id, detail FROM audit_log \
                 WHERE action = 'egress_advisory.denied' AND resource LIKE $1",
        )
        .bind(format!("%{cluster}%"))
        .fetch_all(&self.pool)
        .await
        .expect("audit query");
        assert_eq!(
            rows.len(),
            1,
            "expected exactly one egress_advisory.denied audit row for {cluster}, found {}",
            rows.len()
        );

        let outcome: String = sqlx::query_scalar(
            "SELECT outcome FROM audit_log \
             WHERE action = 'egress_advisory.denied' AND resource LIKE $1",
        )
        .bind(format!("%{cluster}%"))
        .fetch_one(&self.pool)
        .await
        .expect("audit outcome");
        assert_eq!(outcome, "denied", "audit outcome for {cluster}");

        let (resource, org_id, team_id, detail) = rows.into_iter().next().unwrap();
        assert!(
            resource.contains(cluster),
            "audit resource {resource:?} must mention cluster {cluster}"
        );
        assert_eq!(org_id, Some(self.org_id), "audit org_id for {cluster}");
        assert_eq!(team_id, Some(self.team_id), "audit team_id for {cluster}");
        assert_eq!(
            detail["class"], "egress_advisory_denied",
            "audit detail.class for {cluster}: {detail}"
        );
        // Evidence contract: every rejection record carries the authored hostname and the
        // FULL resolved-address set (empty array for resolution failures).
        assert!(
            detail["host"].is_string(),
            "audit detail.host must carry the authored hostname: {detail}"
        );
        assert!(
            detail["resolved_addresses"].is_array(),
            "audit detail.resolved_addresses must carry the full resolution result: {detail}"
        );
        detail
    }

    /// No egress_advisory.denied audit row may exist for this cluster.
    async fn assert_no_denied_audit(&self, cluster: &str) {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log \
             WHERE action = 'egress_advisory.denied' AND resource LIKE $1",
        )
        .bind(format!("%{cluster}%"))
        .fetch_one(&self.pool)
        .await
        .expect("audit count");
        assert_eq!(
            count, 0,
            "no egress_advisory.denied audit row expected for {cluster}"
        );
    }
}

/// The advisory contract allows 400 or 422 for a validation rejection — nothing else.
/// In particular NOT 5xx (a denial is not a server error) and NOT 403 (it is a
/// validation outcome, not an authz outcome).
fn assert_validation_rejection(status: StatusCode) {
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422 validation rejection, got {status}"
    );
}

/// Reject + audit + not-persisted, for one denied host on the create path.
async fn assert_create_denied(ctx: &Ctx, host: &str) {
    let cluster = unique("deny");
    let response = ctx.create_cluster(&cluster, host).await;
    assert_validation_rejection(response.status());
    // Rejection body must be a JSON error object (json_of panics on non-JSON).
    let body = json_of(response).await;
    assert!(
        body.is_object(),
        "rejection body must be a JSON object, got {body}"
    );
    ctx.assert_cluster_absent(&cluster).await;
    ctx.assert_denied_audit(&cluster).await;
}

#[tokio::test]
async fn enabled_policy_rejects_cloud_metadata_endpoint_on_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // 169.254.169.254 is not in the test policy's denied lists — the advisory itself
    // must know the metadata endpoint.
    assert_create_denied(&ctx, "169.254.169.254").await;
}

#[tokio::test]
async fn enabled_policy_rejects_loopback_ip_and_localhost_on_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Loopback literal.
    assert_create_denied(&ctx, "127.0.0.1").await;
    // Hostname that resolves to loopback — must be rejected via resolution, not just
    // literal matching.
    assert_create_denied(&ctx, "localhost").await;
}

#[tokio::test]
async fn enabled_policy_rejects_policy_denied_cidr_and_addr_on_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Inside the policy's denied CIDR 100.64.0.0/10 (but not equal to its base).
    assert_create_denied(&ctx, "100.64.12.34").await;
    // Exactly the policy's denied address.
    assert_create_denied(&ctx, "203.0.113.9").await;
}

#[tokio::test]
async fn enabled_policy_rejects_unresolvable_hostname_on_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // RFC 6761 reserves .invalid: syntactically valid, guaranteed not to resolve, and
    // no external DNS dependency for the failure.
    assert_create_denied(&ctx, "does-not-resolve.invalid").await;
}

#[tokio::test]
async fn enabled_policy_allows_public_and_tenant_private_hosts() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Public address plus two RFC 1918 tenant-private addresses: the advisory must NOT
    // implement a blanket private-range denial. None of these are in the denied
    // addr/CIDR lists.
    for host in ["93.184.216.34", "10.1.2.3", "192.168.1.7"] {
        let cluster = unique("allow");
        let response = ctx.create_cluster(&cluster, host).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "host {host} must be accepted by the enabled advisory"
        );
        assert_eq!(json_of(response).await["revision"], 1);

        let response = ctx
            .send(
                "GET",
                &format!("{}/{cluster}", ctx.clusters_base()),
                None,
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "accepted cluster for {host} must be persisted"
        );
        let body = json_of(response).await;
        assert_eq!(body["spec"]["endpoints"][0]["host"], host);

        ctx.assert_no_denied_audit(&cluster).await;
    }
}

#[tokio::test]
async fn disabled_default_policy_bypasses_advisory_on_create() {
    // Default::default() = advisory disabled: even the metadata endpoint is accepted.
    let Some(ctx) = setup(Default::default()).await else {
        return;
    };
    let cluster = unique("bypass");
    let response = ctx.create_cluster(&cluster, "169.254.169.254").await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "disabled advisory must not block the metadata endpoint"
    );

    let response = ctx
        .send(
            "GET",
            &format!("{}/{cluster}", ctx.clusters_base()),
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["spec"]["endpoints"][0]["host"], "169.254.169.254");

    ctx.assert_no_denied_audit(&cluster).await;
}

#[tokio::test]
async fn enabled_policy_rejects_denied_host_on_update_and_preserves_revision() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    let cluster = unique("upd");

    // Create with an allowed public host.
    let response = ctx.create_cluster(&cluster, "93.184.216.34").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(json_of(response).await["revision"], 1);

    // PATCH to the metadata endpoint with the correct revision: the advisory (not a
    // revision conflict) must reject it.
    let item = format!("{}/{cluster}", ctx.clusters_base());
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({
                "spec": {"endpoints": [{"host": "169.254.169.254", "port": 443}]}
            })),
            Some(1),
        )
        .await;
    assert_validation_rejection(response.status());
    let body = json_of(response).await;
    assert!(
        body.is_object(),
        "rejection body must be a JSON object, got {body}"
    );

    // The mutation must not have landed: same revision, same original endpoint.
    let response = ctx.send("GET", &item, None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(
        body["revision"], 1,
        "rejected update must not bump revision"
    );
    assert_eq!(
        body["spec"]["endpoints"][0]["host"], "93.184.216.34",
        "rejected update must not change the spec"
    );

    // The successful create wrote no denied row, so the single denied row for this
    // cluster is the update rejection; its detail carries the cluster.update context.
    let detail = ctx.assert_denied_audit(&cluster).await;
    assert!(
        detail.to_string().contains("cluster.update"),
        "update-rejection audit detail must carry the cluster.update context: {detail}"
    );

    // And the rejected PATCH must not have consumed the revision: retrying with
    // If-Match 1 and an allowed host still succeeds.
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({
                "spec": {"endpoints": [{"host": "10.1.2.3", "port": 8080}]}
            })),
            Some(1),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "allowed update at the original revision must still succeed after a rejected one"
    );
    assert_eq!(json_of(response).await["revision"], 2);
}
