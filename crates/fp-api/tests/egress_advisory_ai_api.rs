//! Black-box integration tests for slice fpv2-1hp.3: the write-time egress advisory
//! (FP-DEC-0008) gating AI provider create/update on the provider's `base_url` host.
//!
//! Spec under test: when `AppState.egress_advisory` is ENABLED, an AI provider whose
//! `base_url` host is the cloud metadata endpoint or falls inside a policy-denied CIDR
//! is REJECTED with a 4xx validation error, the provider is not persisted, and exactly
//! one rejection audit row is written (action = "egress_advisory.denied", outcome =
//! denied, resource mentioning the provider name, caller org/team ids, detail.class =
//! "egress_advisory_denied", detail.host string, detail.resolved_addresses array).
//! Public and tenant-private hosts are accepted. `Default` policy = bypass.
//!
//! Written spec-first from the acceptance criteria without reading the implementation.
//! Request-body conventions (AiProviderSpec shape, secret prerequisite) come from the
//! public domain contract (fp-domain/src/ai.rs) and existing AI tests in api_crud.rs.

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
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("body must be JSON")
}

/// Same enabled policy as the cluster-path tests: one denied address (203.0.113.9)
/// and one denied CIDR (CGNAT 100.64.0.0/10). Metadata endpoint and loopback are
/// deliberately NOT listed — those denials must come from the advisory itself.
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
    /// A real team secret: AiProviderSpec.credential_secret_id is required.
    secret_id: String,
}

/// Full production-path setup mirroring api_crud.rs: RS256 dev-issuer token through
/// real OIDC validation, fresh org/team per test, plus one team secret so provider
/// specs can reference a valid credential_secret_id. Returns None (skip) when the
/// shared test database is not configured.
async fn setup(egress_advisory: EgressAdvisoryPolicy) -> Option<Ctx> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    std::env::set_var(
        "FLOWPLANE_SECRET_ENCRYPTION_KEY",
        "12345678901234567890123456789012",
    );
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
        .mint(&subject, "egress-ai@test", "Egress AI", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "egress-ai@test", "Egress AI")
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

    let mut ctx = Ctx {
        app,
        pool: query_pool,
        token,
        team_name: team.name.clone(),
        team_id: team.id.as_uuid(),
        org_id: org.id.as_uuid(),
        secret_id: String::new(),
    };

    let response = ctx
        .send(
            "POST",
            &format!("/api/v1/teams/{}/secrets", ctx.team_name),
            Some(serde_json::json!({
                "name": unique("secret"),
                "description": "ai credential",
                "spec": {"type": "generic_secret", "secret": "aGVsbG8="}
            })),
            None,
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "test-prerequisite secret must be creatable"
    );
    ctx.secret_id = json_of(response).await["id"]
        .as_str()
        .expect("secret id")
        .to_string();

    Some(ctx)
}

impl Ctx {
    fn providers_base(&self) -> String {
        format!("/api/v1/teams/{}/ai/providers", self.team_name)
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

    /// AiProviderSpec (fp-domain/src/ai.rs): kind + base_url + credential_secret_id
    /// required; path_prefix optional (base_url must be scheme://host:port, no path).
    fn provider_spec(&self, base_url: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "openai-compatible",
            "base_url": base_url,
            "path_prefix": "/v1",
            "credential_secret_id": self.secret_id,
            "models": ["gpt-5-mini"]
        })
    }

    async fn create_provider(&self, name: &str, base_url: &str) -> axum::response::Response {
        self.send(
            "POST",
            &self.providers_base(),
            Some(serde_json::json!({
                "name": name,
                "spec": self.provider_spec(base_url)
            })),
            None,
        )
        .await
    }

    /// The provider must not exist: item GET is 404 and the list omits it.
    async fn assert_provider_absent(&self, name: &str) {
        let response = self
            .send(
                "GET",
                &format!("{}/{name}", self.providers_base()),
                None,
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "rejected provider {name} must not be retrievable"
        );

        let response = self.send("GET", &self.providers_base(), None, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_of(response).await;
        // Accept either list envelope shape ({"items": [...]} or a bare array) —
        // both existing list surfaces occur in this API.
        let items = body["items"]
            .as_array()
            .or_else(|| body.as_array())
            .expect("provider list array");
        assert!(
            !items.iter().any(|item| item["name"] == name),
            "rejected provider {name} must not appear in the list"
        );
    }

    /// Exactly one rejection audit row for this provider, with the fpv2-1hp.3 contract
    /// fields. `denied_host` is the offending base_url host (an IP literal in these
    /// tests), pinned inside detail.host and detail.resolved_addresses.
    async fn assert_denied_audit(&self, provider: &str, denied_host: &str) -> serde_json::Value {
        type AuditRow = (
            String,
            String,
            Option<uuid::Uuid>,
            Option<uuid::Uuid>,
            serde_json::Value,
        );
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT resource, outcome, org_id, team_id, detail FROM audit_log \
                 WHERE action = 'egress_advisory.denied' AND resource LIKE $1",
        )
        .bind(format!("%{provider}%"))
        .fetch_all(&self.pool)
        .await
        .expect("audit query");
        assert_eq!(
            rows.len(),
            1,
            "expected exactly one egress_advisory.denied audit row for {provider}, found {}",
            rows.len()
        );
        let (resource, outcome, org_id, team_id, detail) = rows.into_iter().next().unwrap();
        assert_eq!(outcome, "denied", "audit outcome for {provider}");
        assert!(
            resource.contains(provider),
            "audit resource {resource:?} must mention provider {provider}"
        );
        assert_eq!(org_id, Some(self.org_id), "audit org_id for {provider}");
        assert_eq!(team_id, Some(self.team_id), "audit team_id for {provider}");
        assert_eq!(
            detail["class"], "egress_advisory_denied",
            "audit detail.class for {provider}: {detail}"
        );
        let host = detail["host"]
            .as_str()
            .unwrap_or_else(|| panic!("detail.host must be a string: {detail}"));
        assert!(
            host.contains(denied_host),
            "detail.host {host:?} must reference the denied host {denied_host}"
        );
        let resolved = detail["resolved_addresses"]
            .as_array()
            .unwrap_or_else(|| panic!("detail.resolved_addresses must be an array: {detail}"));
        assert!(
            resolved
                .iter()
                .any(|addr| addr.as_str().is_some_and(|a| a.contains(denied_host))),
            "detail.resolved_addresses {resolved:?} must contain {denied_host}"
        );
        detail
    }

    /// No egress_advisory.denied audit row may exist for this provider.
    async fn assert_no_denied_audit(&self, provider: &str) {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log \
             WHERE action = 'egress_advisory.denied' AND resource LIKE $1",
        )
        .bind(format!("%{provider}%"))
        .fetch_one(&self.pool)
        .await
        .expect("audit count");
        assert_eq!(
            count, 0,
            "no egress_advisory.denied audit row expected for {provider}"
        );
    }
}

/// 400 or 422 only — not 5xx (a denial is not a server error), not 403 (it is a
/// validation outcome, not an authz outcome).
fn assert_validation_rejection(status: StatusCode) {
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422 validation rejection, got {status}"
    );
}

/// Reject + audit + not-persisted, for one denied base_url on the create path.
async fn assert_create_denied(ctx: &Ctx, base_url: &str, denied_host: &str) {
    let provider = unique("deny");
    let response = ctx.create_provider(&provider, base_url).await;
    assert_validation_rejection(response.status());
    let body = json_of(response).await;
    assert!(
        body.is_object(),
        "rejection body must be a JSON object, got {body}"
    );
    ctx.assert_provider_absent(&provider).await;
    ctx.assert_denied_audit(&provider, denied_host).await;
}

#[tokio::test]
async fn enabled_policy_rejects_metadata_endpoint_on_provider_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Not in the test policy's denied lists — the advisory itself must know the
    // cloud metadata endpoint.
    assert_create_denied(&ctx, "http://169.254.169.254:80", "169.254.169.254").await;
}

#[tokio::test]
async fn enabled_policy_rejects_policy_denied_cidr_on_provider_create() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Inside the policy's denied CIDR 100.64.0.0/10 (not equal to its base address).
    assert_create_denied(&ctx, "http://100.64.12.34:8080", "100.64.12.34").await;
}

#[tokio::test]
async fn enabled_policy_allows_public_and_tenant_private_provider_hosts() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    // Public plus RFC 1918 tenant-private: no blanket private-range denial, and
    // neither host is in the policy's denied lists.
    for base_url in ["http://93.184.216.34:8080", "http://10.1.2.3:8080"] {
        let provider = unique("allow");
        let response = ctx.create_provider(&provider, base_url).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "base_url {base_url} must be accepted by the enabled advisory"
        );
        let body = json_of(response).await;
        assert_eq!(body["revision"], 1);
        assert_eq!(body["spec"]["base_url"], base_url);

        let response = ctx
            .send(
                "GET",
                &format!("{}/{provider}", ctx.providers_base()),
                None,
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "accepted provider for {base_url} must be persisted"
        );
        assert_eq!(json_of(response).await["spec"]["base_url"], base_url);

        ctx.assert_no_denied_audit(&provider).await;
    }
}

#[tokio::test]
async fn disabled_default_policy_bypasses_advisory_on_provider_create() {
    // Default::default() = advisory disabled: even the metadata endpoint is accepted.
    let Some(ctx) = setup(Default::default()).await else {
        return;
    };
    let provider = unique("bypass");
    let response = ctx
        .create_provider(&provider, "http://169.254.169.254:80")
        .await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "disabled advisory must not block the metadata endpoint"
    );

    let response = ctx
        .send(
            "GET",
            &format!("{}/{provider}", ctx.providers_base()),
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_of(response).await["spec"]["base_url"],
        "http://169.254.169.254:80"
    );

    ctx.assert_no_denied_audit(&provider).await;
}

#[tokio::test]
async fn enabled_policy_rejects_denied_host_on_provider_update_and_preserves_revision() {
    let Some(ctx) = setup(enabled_policy()).await else {
        return;
    };
    let provider = unique("upd");
    let allowed_base_url = "http://10.1.2.3:8080";

    // Create with an allowed tenant-private host.
    let response = ctx.create_provider(&provider, allowed_base_url).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(json_of(response).await["revision"], 1);

    // PATCH to the metadata endpoint with the correct revision: the advisory (not a
    // revision conflict) must reject it.
    let item = format!("{}/{provider}", ctx.providers_base());
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({
                "spec": ctx.provider_spec("http://169.254.169.254:80")
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

    // The mutation must not have landed: same revision, same original base_url.
    let response = ctx.send("GET", &item, None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(
        body["revision"], 1,
        "rejected update must not bump revision"
    );
    assert_eq!(
        body["spec"]["base_url"], allowed_base_url,
        "rejected update must not change the spec"
    );

    // The successful create wrote no denied row, so the single denied row for this
    // provider is the update rejection; its detail carries the ai_provider.update
    // context and the standard host/resolved_addresses fields.
    let detail = ctx.assert_denied_audit(&provider, "169.254.169.254").await;
    assert!(
        detail.to_string().contains("ai_provider.update"),
        "update-rejection audit detail must carry the ai_provider.update context: {detail}"
    );

    // The rejected PATCH must not have consumed the revision: retrying with If-Match 1
    // and an allowed base_url still succeeds.
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({
                "spec": ctx.provider_spec("http://93.184.216.34:8080")
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
