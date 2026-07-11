//! Black-box integration tests for slice fpv2-1hp.4: the write-time egress advisory
//! (FP-DEC-0008) gating (a) the expose shortcut and (b) route-generation plan apply,
//! each with its own mutation label ("expose.create", "route_generation.apply").
//!
//! Spec under test: with `AppState.egress_advisory` ENABLED, an expose whose upstream
//! host is denied is rejected 4xx with NO cluster/route-config/listener created and
//! exactly one rejection audit row (action = "egress_advisory.denied", outcome =
//! denied, resource mentioning the expose name, caller org/team ids, detail.class =
//! "egress_advisory_denied", detail carrying "expose.create", detail.host string,
//! detail.resolved_addresses array). Applying a route-generation plan whose generated
//! cluster host is denied is rejected 4xx with an audit row carrying
//! "route_generation.apply" and the plan stays un-applied. Tenant-private upstreams
//! are accepted; the Default (disabled) policy bypasses the advisory.
//!
//! Written spec-first without reading the implementation. The expose request-body
//! shape comes from api_crud.rs; the approved-spec-version seeding is copied verbatim
//! (mechanism-wise) from crates/fp-core/tests/route_generation.rs, which already seeds
//! via fp_storage::repos::api_lifecycle inside a transaction.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_core::services::egress_advisory::{Cidr, EgressAdvisoryPolicy};
use fp_domain::api_lifecycle::{
    ApiDefinitionSpec, SpecFormat, SpecReviewDecision, SpecSourceKind, SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::OrgRole;
use fp_storage::repos::{api_lifecycle, identity};
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

/// Random high port so parallel tests cannot collide on the listeners
/// unique-(address, port) constraint.
fn unique_port() -> u16 {
    20000 + (uuid::Uuid::new_v4().as_u128() % 20000) as u16
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("body must be JSON")
}

/// Same enabled policy as the sibling advisory test files: one denied address and one
/// denied CIDR. The metadata endpoint and loopback are deliberately NOT listed — those
/// denials must come from the advisory itself.
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
    team_ref: TeamRef,
    team_id: uuid::Uuid,
    org_id: uuid::Uuid,
}

/// Full production-path setup mirroring api_crud.rs. Returns None (skip) when the
/// shared test database is not configured.
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
        .mint(&subject, "egress-expose@test", "Egress Expose", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user =
        identity::upsert_user_by_subject(&pool, &subject, "egress-expose@test", "Egress Expose")
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
        team_ref: TeamRef {
            id: team.id,
            org_id: org.id,
        },
        team_id: team.id.as_uuid(),
        org_id: org.id.as_uuid(),
    })
}

impl Ctx {
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {}", self.token));
        let request = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(Body::from(json.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("request");
        self.app.clone().oneshot(request).await.expect("response")
    }

    async fn expose(&self, name: &str, upstream: &str, port: u16) -> axum::response::Response {
        self.send(
            "POST",
            &format!("/api/v1/teams/{}/expose", self.team_name),
            Some(serde_json::json!({
                "name": name,
                "upstream": upstream,
                "path": "/",
                "port": port
            })),
        )
        .await
    }

    /// The named gateway resource must not exist: item GET 404 and list omission.
    async fn assert_gateway_resource_absent(&self, kind: &str, name: &str) {
        let base = format!("/api/v1/teams/{}/{kind}", self.team_name);
        let response = self.send("GET", &format!("{base}/{name}"), None).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{kind}/{name} must not exist"
        );
        let response = self.send("GET", &base, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_of(response).await;
        let items = body["items"]
            .as_array()
            .or_else(|| body.as_array())
            .expect("list array");
        assert!(
            !items.iter().any(|item| item["name"] == name),
            "{kind}/{name} must not appear in the list"
        );
    }

    /// Exactly one rejection audit row scoped to this test's fresh team, returned for
    /// further label-specific assertions.
    async fn assert_single_denied_audit_for_team(&self) -> (String, serde_json::Value) {
        let rows: Vec<(String, String, Option<uuid::Uuid>, serde_json::Value)> = sqlx::query_as(
            "SELECT resource, outcome, org_id, detail FROM audit_log \
             WHERE action = 'egress_advisory.denied' AND team_id = $1",
        )
        .bind(self.team_id)
        .fetch_all(&self.pool)
        .await
        .expect("audit query");
        assert_eq!(
            rows.len(),
            1,
            "expected exactly one egress_advisory.denied audit row for the team, found {}",
            rows.len()
        );
        let (resource, outcome, org_id, detail) = rows.into_iter().next().unwrap();
        assert_eq!(outcome, "denied", "audit outcome");
        assert_eq!(org_id, Some(self.org_id), "audit org_id");
        assert_eq!(
            detail["class"], "egress_advisory_denied",
            "audit detail.class: {detail}"
        );
        (resource, detail)
    }

    /// No egress_advisory.denied audit row may exist for this test's team.
    async fn assert_no_denied_audit_for_team(&self) {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log \
             WHERE action = 'egress_advisory.denied' AND team_id = $1",
        )
        .bind(self.team_id)
        .fetch_one(&self.pool)
        .await
        .expect("audit count");
        assert_eq!(count, 0, "no egress_advisory.denied audit row expected");
    }

    /// Seed an approved (reviewed) learned spec version whose forwarded upstream is
    /// `host:port` — copied mechanism-for-mechanism from the existing
    /// crates/fp-core/tests/route_generation.rs seeding helper, which already uses
    /// fp_storage::repos::api_lifecycle inside a transaction.
    async fn reviewed_spec_with_upstream(
        &self,
        api_name: &str,
        host: &str,
        port: u16,
    ) -> fp_domain::SpecVersionId {
        let mut tx = self.pool.begin().await.expect("tx");
        let api = api_lifecycle::create_api_definition(
            &mut tx,
            self.team_ref,
            api_name,
            &ApiDefinitionSpec {
                display_name: api_name.into(),
                description: String::new(),
            },
        )
        .await
        .expect("api");
        let spec = api_lifecycle::create_spec_version(
            &mut tx,
            self.team_ref,
            api.id,
            &SpecVersionInput {
                source_kind: SpecSourceKind::Learned,
                format: SpecFormat::OpenApi3,
                spec: serde_json::json!({
                    "openapi": "3.1.0",
                    "info": {"title": api_name, "version": "1.0.0"},
                    "x-flowplane-learning-source": {
                        "observed_host": "api.example.test",
                        "forwarded_upstream_host": host,
                        "forwarded_upstream_port": port,
                        "forwarded_upstream_tls": false
                    },
                    "paths": {
                        "/v1/items/{id}": {
                            "get": {"operationId": "getItem", "responses": {"200": {"description": "ok"}}}
                        }
                    }
                }),
            },
        )
        .await
        .expect("spec");
        api_lifecycle::append_spec_review_event(
            &mut tx,
            self.team_ref,
            api_lifecycle::SpecReviewEventInsert {
                api_id: api.id,
                spec_version_id: spec.id,
                decision: SpecReviewDecision::Reviewed,
                actor_type: "user",
                actor_id: None,
                reason: "test",
                metadata: serde_json::json!({}),
            },
        )
        .await
        .expect("review");
        tx.commit().await.expect("commit");
        spec.id
    }

    /// Create a dry-run route-generation plan over REST and return its response body
    /// (id, status, plan.{cluster_name, route_config_name, listener_name}).
    async fn create_plan(
        &self,
        spec_version_id: fp_domain::SpecVersionId,
        listener_port: u16,
    ) -> serde_json::Value {
        let response = self
            .send(
                "POST",
                &format!("/api/v1/teams/{}/route-generation-plans", self.team_name),
                Some(serde_json::json!({
                    "spec_version_id": spec_version_id.as_uuid(),
                    "listener_port": listener_port
                })),
            )
            .await;
        let status = response.status();
        let body = json_of(response).await;
        assert!(
            status.is_success(),
            "plan create (dry-run) must succeed — the advisory gates APPLY, not the \
             dry-run preview; got {status}: {body}"
        );
        assert_eq!(
            body["status"], "dry_run",
            "fresh plan must be dry_run: {body}"
        );
        assert!(body["id"].is_string(), "plan id in response: {body}");
        body
    }

    async fn apply_plan(&self, plan_id: &str) -> axum::response::Response {
        self.send(
            "POST",
            &format!(
                "/api/v1/teams/{}/route-generation-plans/{plan_id}/apply",
                self.team_name
            ),
            Some(serde_json::json!({})),
        )
        .await
    }
}

/// 400 or 422 only — not 5xx, not 403, and (for plan re-apply) explicitly not 409.
fn assert_validation_rejection(status: StatusCode) {
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422 validation rejection, got {status}"
    );
}

// ---------------------------------------------------------------------------
// Expose shortcut
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enabled_policy_rejects_metadata_upstream_on_expose_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    let name = unique("deny");
    let response = ctx
        .expose(&name, "http://169.254.169.254:80", unique_port())
        .await;
    assert_validation_rejection(response.status());
    let body = json_of(response).await;
    assert!(
        body.is_object(),
        "rejection body must be a JSON object, got {body}"
    );

    // The expose shortcut creates three resources atomically — a rejected expose must
    // leave NONE of them behind (names per the documented expose convention pinned in
    // api_crud.rs: {name}-upstream / {name}-routes / {name}).
    ctx.assert_gateway_resource_absent("clusters", &format!("{name}-upstream"))
        .await;
    ctx.assert_gateway_resource_absent("route-configs", &format!("{name}-routes"))
        .await;
    ctx.assert_gateway_resource_absent("listeners", &name).await;

    let (resource, detail) = ctx.assert_single_denied_audit_for_team().await;
    assert!(
        resource.contains(&name),
        "audit resource {resource:?} must mention the expose name {name}"
    );
    assert!(
        detail.to_string().contains("expose.create"),
        "rejection audit detail must carry the expose.create context: {detail}"
    );
    let host = detail["host"]
        .as_str()
        .unwrap_or_else(|| panic!("detail.host must be a string: {detail}"));
    assert!(
        host.contains("169.254.169.254"),
        "detail.host {host:?} must reference the denied host"
    );
    let resolved = detail["resolved_addresses"]
        .as_array()
        .unwrap_or_else(|| panic!("detail.resolved_addresses must be an array: {detail}"));
    assert!(
        resolved
            .iter()
            .any(|addr| addr.as_str().is_some_and(|a| a.contains("169.254.169.254"))),
        "detail.resolved_addresses {resolved:?} must contain the denied address"
    );
}

#[tokio::test]
async fn enabled_policy_allows_tenant_private_upstream_on_expose() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    let name = unique("allow");
    let port = unique_port();
    let response = ctx.expose(&name, "http://10.1.2.3:8080", port).await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "tenant-private upstream must be accepted by the enabled advisory"
    );
    let body = json_of(response).await;
    assert_eq!(body["cluster"]["name"], format!("{name}-upstream"));
    assert_eq!(body["cluster"]["spec"]["endpoints"][0]["host"], "10.1.2.3");
    assert_eq!(body["route_config"]["name"], format!("{name}-routes"));
    assert_eq!(body["listener"]["name"], name);

    // Persisted, not just echoed.
    let response = ctx
        .send(
            "GET",
            &format!("/api/v1/teams/{}/clusters/{name}-upstream", ctx.team_name),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_of(response).await["spec"]["endpoints"][0]["host"],
        "10.1.2.3"
    );

    ctx.assert_no_denied_audit_for_team().await;
}

#[tokio::test]
async fn disabled_default_policy_bypasses_advisory_on_expose() {
    // Default::default() = advisory disabled: even a metadata-endpoint upstream lands.
    let Some(ctx) = setup(Default::default()).await else {
        return;
    };
    let name = unique("bypass");
    let response = ctx
        .expose(&name, "http://169.254.169.254:80", unique_port())
        .await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "disabled advisory must not block the metadata endpoint"
    );
    let body = json_of(response).await;
    assert_eq!(
        body["cluster"]["spec"]["endpoints"][0]["host"],
        "169.254.169.254"
    );

    let response = ctx
        .send(
            "GET",
            &format!("/api/v1/teams/{}/clusters/{name}-upstream", ctx.team_name),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    ctx.assert_no_denied_audit_for_team().await;
}

// ---------------------------------------------------------------------------
// Route-generation plan apply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enabled_policy_rejects_denied_generated_cluster_on_plan_apply() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Approved spec whose learned upstream — hence the plan's generated cluster host —
    // is the cloud metadata endpoint.
    let spec_id = ctx
        .reviewed_spec_with_upstream(&unique("learned-api"), "169.254.169.254", 80)
        .await;
    let plan = ctx.create_plan(spec_id, unique_port()).await;
    let plan_id = plan["id"].as_str().expect("plan id").to_string();
    let cluster_name = plan["plan"]["cluster_name"]
        .as_str()
        .expect("generated cluster name")
        .to_string();
    let route_config_name = plan["plan"]["route_config_name"]
        .as_str()
        .expect("generated route config name")
        .to_string();
    let listener_name = plan["plan"]["listener_name"]
        .as_str()
        .expect("generated listener name")
        .to_string();

    // Apply must be rejected by the advisory.
    let response = ctx.apply_plan(&plan_id).await;
    assert_validation_rejection(response.status());
    let body = json_of(response).await;
    assert!(
        body.is_object(),
        "rejection body must be a JSON object, got {body}"
    );

    // Exactly one rejection audit row, labelled with the apply mutation and pinned to the
    // plan-specific resource string.
    let (resource, detail) = ctx.assert_single_denied_audit_for_team().await;
    assert_eq!(
        resource,
        format!("route-plans/{plan_id}"),
        "rejection audit resource must name the plan"
    );
    assert!(
        detail.to_string().contains("route_generation.apply"),
        "rejection audit detail must carry the route_generation.apply context: {detail}"
    );

    // Nothing was applied: none of the plan's generated resources exist.
    ctx.assert_gateway_resource_absent("clusters", &cluster_name)
        .await;
    ctx.assert_gateway_resource_absent("route-configs", &route_config_name)
        .await;
    ctx.assert_gateway_resource_absent("listeners", &listener_name)
        .await;

    // The plan must still be dry-run: re-applying is not a 409 already-applied
    // conflict — it hits the advisory again (another 400/422).
    let response = ctx.apply_plan(&plan_id).await;
    assert_ne!(
        response.status(),
        StatusCode::CONFLICT,
        "a rejected apply must not have consumed the plan"
    );
    assert_validation_rejection(response.status());
}

#[tokio::test]
async fn enabled_policy_allows_plan_apply_with_private_upstream() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Same flow with a tenant-private generated cluster host: the enabled advisory
    // must NOT block a legitimate apply.
    let spec_id = ctx
        .reviewed_spec_with_upstream(&unique("learned-api"), "10.1.2.3", 8080)
        .await;
    let plan = ctx.create_plan(spec_id, unique_port()).await;
    let plan_id = plan["id"].as_str().expect("plan id").to_string();
    let cluster_name = plan["plan"]["cluster_name"]
        .as_str()
        .expect("generated cluster name")
        .to_string();

    let response = ctx.apply_plan(&plan_id).await;
    let status = response.status();
    let body = json_of(response).await;
    assert!(
        status.is_success(),
        "apply with an allowed upstream must succeed, got {status}: {body}"
    );
    assert_eq!(
        body["plan"]["status"], "applied",
        "applied plan status: {body}"
    );

    // The generated cluster now exists with the learned upstream host.
    let response = ctx
        .send(
            "GET",
            &format!("/api/v1/teams/{}/clusters/{cluster_name}", ctx.team_name),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_of(response).await["spec"]["endpoints"][0]["host"],
        "10.1.2.3"
    );

    ctx.assert_no_denied_audit_for_team().await;
}
