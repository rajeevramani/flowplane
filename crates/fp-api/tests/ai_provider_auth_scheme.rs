//! Black-box integration tests for slice fpv2-crv.1: the OPTIONAL `auth_scheme` string
//! field on `AiProviderSpec` as pure at-rest configuration — stored, validated,
//! losslessly round-tripped over REST and the MCP static CP tools, and documented in
//! the generated OpenAPI contract.
//!
//! Spec under test (written spec-first from the acceptance criteria, without reading
//! the implementation):
//! - Valid values are RFC 7235 auth-scheme tokens: 1+ chars from the tchar set
//!   [A-Za-z0-9!#$%&'*+.^_|~-] plus the backtick (e.g. "Bearer", "bearer", "DPoP", "Token").
//! - Invalid values (empty string, any whitespace, non-token chars) are rejected with
//!   REST 400 and the same domain-validation error envelope other AiProviderSpec
//!   validation failures (e.g. a bad `base_url`) produce.
//! - An omitted field is valid (None) and must be ABSENT from the spec JSON returned
//!   by reads — not null, not empty.
//! - Create/GET/PATCH (If-Match revision convention) round-trip the field losslessly,
//!   including removal (PATCH a spec without the field -> subsequent reads omit it).
//! - The generated OpenAPI document exposes `auth_scheme` on the AiProviderSpec schema.
//! - The MCP static tools `cp_ai_providers_list` / `cp_ai_providers_get` serialize the
//!   spec identically to REST (field present when set, absent when not).
//!
//! Conventions (route shape, auth setup, secret prerequisite, If-Match usage) mirror
//! the existing AI-provider tests in api_crud.rs and egress_advisory_ai_api.rs; the
//! MCP calling convention mirrors mcp_static_tools.rs. DB-backed tests use the shared
//! external PostgreSQL and skip themselves when FLOWPLANE_TEST_DATABASE_URL is unset.
//! Parallel-safety (invariant 18): unique per-test org/team/resource names, no fixed
//! ports, no global row counts.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
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

struct Ctx {
    app: axum::Router,
    token: String,
    team_name: String,
    /// A real team secret: AiProviderSpec.credential_secret_id is required.
    secret_id: String,
}

/// Full production-path setup mirroring api_crud.rs: RS256 dev-issuer token through
/// real OIDC validation, fresh org/team per test, plus one team secret so provider
/// specs can reference a valid credential_secret_id. Returns None (skip) when the
/// shared test database is not configured.
async fn setup() -> Option<Ctx> {
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
        .mint(&subject, "auth-scheme@test", "Auth Scheme", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "auth-scheme@test", "Auth Scheme")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
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

    let mut ctx = Ctx {
        app,
        token,
        team_name: team.name.clone(),
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

    /// A known-valid AiProviderSpec (per api_crud.rs), with `auth_scheme` spliced in
    /// when given. `auth_scheme: None` deliberately OMITS the key entirely.
    fn provider_spec(&self, auth_scheme: Option<&str>) -> serde_json::Value {
        let mut spec = serde_json::json!({
            "kind": "openai-compatible",
            "base_url": "https://llm.example",
            "path_prefix": "/v1",
            "credential_secret_id": self.secret_id,
            "models": ["gpt-5-mini"]
        });
        if let Some(scheme) = auth_scheme {
            spec["auth_scheme"] = serde_json::Value::String(scheme.to_string());
        }
        spec
    }

    async fn create_provider(
        &self,
        name: &str,
        auth_scheme: Option<&str>,
    ) -> axum::response::Response {
        self.send(
            "POST",
            &self.providers_base(),
            Some(serde_json::json!({
                "name": name,
                "spec": self.provider_spec(auth_scheme)
            })),
            None,
        )
        .await
    }

    async fn get_provider_spec(&self, name: &str) -> serde_json::Value {
        let response = self
            .send(
                "GET",
                &format!("{}/{name}", self.providers_base()),
                None,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "GET provider {name}");
        let body = json_of(response).await;
        assert!(
            body["spec"].is_object(),
            "provider {name} body must carry a spec object: {body}"
        );
        body["spec"].clone()
    }
}

/// The `auth_scheme` key must be ABSENT from a spec JSON object — not null, not "".
fn assert_auth_scheme_absent(spec: &serde_json::Value, ctx: &str) {
    let obj = spec
        .as_object()
        .unwrap_or_else(|| panic!("{ctx}: spec must be a JSON object, got {spec}"));
    assert!(
        !obj.contains_key("auth_scheme"),
        "{ctx}: auth_scheme must be ABSENT (not null/empty) from the spec, got {spec}"
    );
}

fn content_type_is_json(response: &axum::response::Response) -> bool {
    response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

// =============================================================================================
// OpenAPI contract — no DB required.
// =============================================================================================

#[test]
fn openapi_ai_provider_spec_schema_documents_optional_auth_scheme() {
    let doc = fp_api::routes::openapi_document();
    let json = serde_json::to_value(&doc).expect("doc");
    let schema = json["components"]["schemas"]
        .get("AiProviderSpec")
        .unwrap_or_else(|| {
            panic!(
                "OpenAPI components.schemas must contain AiProviderSpec; got schema keys: {:?}",
                json["components"]["schemas"]
                    .as_object()
                    .map(|o| o.keys().collect::<Vec<_>>())
            )
        });
    let properties = schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("AiProviderSpec schema must have properties: {schema}"));
    assert!(
        properties.contains_key("auth_scheme"),
        "AiProviderSpec OpenAPI schema must document the auth_scheme property; got: {:?}",
        properties.keys().collect::<Vec<_>>()
    );
    // The field is optional: it must not be listed as required.
    if let Some(required) = schema["required"].as_array() {
        assert!(
            !required.iter().any(|r| r == "auth_scheme"),
            "auth_scheme is OPTIONAL and must not be in AiProviderSpec.required: {required:?}"
        );
    }
}

// =============================================================================================
// REST: lossless round-trip.
// =============================================================================================

#[tokio::test]
async fn auth_scheme_round_trips_on_create_get_and_list() {
    let Some(ctx) = setup().await else {
        return;
    };
    let name = unique("prov");

    let response = ctx.create_provider(&name, Some("Bearer")).await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create with auth_scheme=Bearer must succeed"
    );
    let body = json_of(response).await;
    assert_eq!(
        body["spec"]["auth_scheme"], "Bearer",
        "create response must echo auth_scheme: {body}"
    );
    assert_eq!(body["revision"], 1);

    // GET round-trips the exact value.
    let spec = ctx.get_provider_spec(&name).await;
    assert_eq!(
        spec["auth_scheme"], "Bearer",
        "GET must round-trip auth_scheme losslessly: {spec}"
    );

    // The list serialization carries it too (same serialization as the item GET).
    let response = ctx.send("GET", &ctx.providers_base(), None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    let items = body["items"]
        .as_array()
        .or_else(|| body.as_array())
        .unwrap_or_else(|| panic!("provider list must be an array or {{items: []}}: {body}"));
    let listed = items
        .iter()
        .find(|item| item["name"] == name.as_str())
        .unwrap_or_else(|| panic!("provider {name} must appear in the list"));
    assert_eq!(
        listed["spec"]["auth_scheme"], "Bearer",
        "list must serialize auth_scheme identically to GET: {listed}"
    );
}

#[tokio::test]
async fn omitted_auth_scheme_is_valid_and_absent_from_reads() {
    let Some(ctx) = setup().await else {
        return;
    };
    let name = unique("prov");

    let response = ctx.create_provider(&name, None).await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create WITHOUT auth_scheme must succeed (omitted = valid None)"
    );
    let body = json_of(response).await;
    assert_auth_scheme_absent(&body["spec"], "create response");

    let spec = ctx.get_provider_spec(&name).await;
    assert_auth_scheme_absent(&spec, "GET after create-without-field");

    // And the list serialization omits it too.
    let response = ctx.send("GET", &ctx.providers_base(), None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    let items = body["items"]
        .as_array()
        .or_else(|| body.as_array())
        .unwrap_or_else(|| panic!("provider list must be an array or {{items: []}}: {body}"));
    let listed = items
        .iter()
        .find(|item| item["name"] == name.as_str())
        .unwrap_or_else(|| panic!("provider {name} must appear in the list"));
    assert_auth_scheme_absent(&listed["spec"], "list item");
}

#[tokio::test]
async fn valid_rfc7235_token_variants_are_accepted_and_round_trip() {
    let Some(ctx) = setup().await else {
        return;
    };
    // RFC 7235 auth-scheme = RFC 7230 token: 1+ tchars. Case is preserved, not
    // normalized (lossless round-trip).
    for scheme in [
        "Bearer",
        "bearer",
        "DPoP",
        "Token",
        "B",                 // single tchar
        "123",               // digits are tchars
        "X-Custom.Scheme_9", // ., _, - are tchars
        "!#$%&'*+.^_`|~-",   // every non-alphanumeric tchar at once
    ] {
        let name = unique("ok");
        let response = ctx.create_provider(&name, Some(scheme)).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "auth_scheme {scheme:?} is a valid RFC 7235 token and must be accepted"
        );
        let spec = ctx.get_provider_spec(&name).await;
        assert_eq!(
            spec["auth_scheme"],
            serde_json::Value::String(scheme.to_string()),
            "auth_scheme {scheme:?} must round-trip byte-for-byte"
        );
    }
}

// =============================================================================================
// REST: invalid values -> 400 with the domain validation error envelope.
// =============================================================================================

#[tokio::test]
async fn invalid_auth_scheme_values_are_rejected_with_validation_envelope() {
    let Some(ctx) = setup().await else {
        return;
    };

    // Baseline: the envelope existing AiProviderSpec domain validation produces — a
    // create with a structurally invalid base_url. Invalid auth_scheme values must be
    // rejected with the SAME status and error code.
    let baseline_name = unique("badurl");
    let response = ctx
        .send(
            "POST",
            &ctx.providers_base(),
            Some(serde_json::json!({
                "name": baseline_name,
                "spec": {
                    "kind": "openai-compatible",
                    "base_url": "not-a-url",
                    "credential_secret_id": ctx.secret_id,
                    "models": ["gpt-5-mini"]
                }
            })),
            None,
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "baseline bad base_url must be a 400 domain validation error"
    );
    assert!(
        content_type_is_json(&response),
        "validation error must have a JSON content-type"
    );
    let baseline = json_of(response).await;
    let baseline_code = baseline["code"]
        .as_str()
        .unwrap_or_else(|| panic!("validation envelope must carry a string code: {baseline}"))
        .to_string();

    for scheme in [
        "",         // empty string
        " Bearer",  // leading space
        "Bearer ",  // trailing space
        "Bea rer",  // interior space
        "Bear\ter", // tab
        "Bearer\n", // newline
        "Bearer:",  // colon (the very delimiter after a scheme)
        "Bea;rer",  // semicolon
        "Bearér",   // non-ASCII
        "Bear(er",  // parenthesis (RFC 7230 separator)
        "Bearer/",  // slash
        "Bearer,",  // comma
        "Be@rer",   // at-sign
        "\"Bearer\"", // double quotes (note: the single quote ' IS a tchar, so 'Bearer'
                    // would be VALID and deliberately does not appear in this table)
    ] {
        let name = unique("bad");
        let response = ctx.create_provider(&name, Some(scheme)).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "auth_scheme {scheme:?} is not an RFC 7235 token and must be rejected with 400"
        );
        assert!(
            content_type_is_json(&response),
            "rejection for {scheme:?} must have a JSON content-type"
        );
        let body = json_of(response).await;
        assert_eq!(
            body["code"].as_str(),
            Some(baseline_code.as_str()),
            "auth_scheme {scheme:?} must produce the same validation error code as other \
             AiProviderSpec domain validation failures ({baseline_code}); got: {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "rejection for {scheme:?} must carry a non-empty message: {body}"
        );

        // The rejected provider must not have been persisted.
        let response = ctx
            .send(
                "GET",
                &format!("{}/{name}", ctx.providers_base()),
                None,
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "provider {name} with invalid auth_scheme {scheme:?} must not be persisted"
        );
    }
}

// =============================================================================================
// REST: PATCH (If-Match) — change, removal, and an adversarial empty-string PATCH.
// =============================================================================================

#[tokio::test]
async fn patch_round_trips_auth_scheme_change_and_removal() {
    let Some(ctx) = setup().await else {
        return;
    };
    let name = unique("prov");

    let response = ctx.create_provider(&name, Some("Bearer")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(json_of(response).await["revision"], 1);
    let item = format!("{}/{name}", ctx.providers_base());

    // PATCH to another valid token at the current revision.
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({ "spec": ctx.provider_spec(Some("DPoP")) })),
            Some(1),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK, "PATCH auth_scheme=DPoP");
    let body = json_of(response).await;
    assert_eq!(body["revision"], 2);
    assert_eq!(body["spec"]["auth_scheme"], "DPoP");
    let spec = ctx.get_provider_spec(&name).await;
    assert_eq!(
        spec["auth_scheme"], "DPoP",
        "GET after PATCH must show the new value: {spec}"
    );

    // PATCH a spec WITHOUT the field: the field is removed, and reads omit it.
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({ "spec": ctx.provider_spec(None) })),
            Some(2),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "PATCH with a spec omitting auth_scheme must succeed (omitted = None)"
    );
    let body = json_of(response).await;
    assert_eq!(body["revision"], 3);
    assert_auth_scheme_absent(&body["spec"], "PATCH-removal response");
    let spec = ctx.get_provider_spec(&name).await;
    assert_auth_scheme_absent(&spec, "GET after PATCH-removal");
}

#[tokio::test]
async fn patch_with_empty_auth_scheme_is_rejected_and_state_is_preserved() {
    let Some(ctx) = setup().await else {
        return;
    };
    let name = unique("prov");

    let response = ctx.create_provider(&name, Some("Bearer")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(json_of(response).await["revision"], 1);
    let item = format!("{}/{name}", ctx.providers_base());

    // Adversarial: PATCH tries auth_scheme: "" at the correct revision.
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({ "spec": ctx.provider_spec(Some("")) })),
            Some(1),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "PATCH with auth_scheme=\"\" must be rejected as a validation error"
    );
    assert!(
        content_type_is_json(&response),
        "rejection must have a JSON content-type"
    );
    let body = json_of(response).await;
    assert!(
        body["code"].as_str().is_some_and(|c| !c.is_empty()),
        "rejection must carry the error envelope code: {body}"
    );

    // The mutation must not have landed: same revision, original value intact.
    let response = ctx.send("GET", &item, None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(
        body["revision"], 1,
        "rejected PATCH must not bump the revision: {body}"
    );
    assert_eq!(
        body["spec"]["auth_scheme"], "Bearer",
        "rejected PATCH must not change the stored spec: {body}"
    );

    // The rejected PATCH must not have consumed the revision: a valid PATCH at
    // If-Match 1 still succeeds.
    let response = ctx
        .send(
            "PATCH",
            &item,
            Some(serde_json::json!({ "spec": ctx.provider_spec(Some("Token")) })),
            Some(1),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "valid PATCH at the original revision must still succeed after a rejected one"
    );
    assert_eq!(json_of(response).await["spec"]["auth_scheme"], "Token");
}

// =============================================================================================
// MCP contract parity: cp_ai_providers_list / cp_ai_providers_get.
// =============================================================================================

fn mcp_request(token: &str, session: Option<&str>, body: serde_json::Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

async fn mcp_initialize(app: axum::Router, token: &str) -> String {
    let response = app
        .oneshot(mcp_request(
            token,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }),
        ))
        .await
        .expect("initialize");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session")
        .to_string()
}

async fn mcp_tools_call(
    app: axum::Router,
    token: &str,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = app
        .oneshot(mcp_request(
            token,
            Some(session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }),
        ))
        .await
        .expect("tools/call");
    assert_eq!(response.status(), StatusCode::OK);
    json_of(response).await
}

#[tokio::test]
async fn mcp_static_tools_serialize_auth_scheme_identically_to_rest() {
    let Some(ctx) = setup().await else {
        return;
    };

    // Two providers via REST: one with auth_scheme, one without.
    let with_scheme = unique("with");
    let without_scheme = unique("without");
    let response = ctx.create_provider(&with_scheme, Some("Bearer")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = ctx.create_provider(&without_scheme, None).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    // REST reference serializations (parity target).
    let rest_with = ctx.get_provider_spec(&with_scheme).await;
    let rest_without = ctx.get_provider_spec(&without_scheme).await;

    let session = mcp_initialize(ctx.app.clone(), &ctx.token).await;

    // cp_ai_providers_get: field present when set — same serialization as REST.
    let got = mcp_tools_call(
        ctx.app.clone(),
        &ctx.token,
        &session,
        "cp_ai_providers_get",
        serde_json::json!({ "team": ctx.team_name, "name": with_scheme }),
    )
    .await;
    assert_eq!(
        got["result"]["isError"], false,
        "cp_ai_providers_get({with_scheme}) must succeed: {got}"
    );
    let spec = &got["result"]["structuredContent"]["spec"];
    assert_eq!(
        spec["auth_scheme"], "Bearer",
        "MCP get must include auth_scheme when set: {got}"
    );
    assert_eq!(
        spec, &rest_with,
        "MCP get must serialize the spec identically to REST"
    );

    // cp_ai_providers_get: field absent when not set.
    let got = mcp_tools_call(
        ctx.app.clone(),
        &ctx.token,
        &session,
        "cp_ai_providers_get",
        serde_json::json!({ "team": ctx.team_name, "name": without_scheme }),
    )
    .await;
    assert_eq!(
        got["result"]["isError"], false,
        "cp_ai_providers_get({without_scheme}) must succeed: {got}"
    );
    let spec = &got["result"]["structuredContent"]["spec"];
    assert_auth_scheme_absent(spec, "MCP cp_ai_providers_get (unset)");
    assert_eq!(
        spec, &rest_without,
        "MCP get must serialize the unset-field spec identically to REST"
    );

    // cp_ai_providers_list: both providers, each serialized as over REST.
    let listed = mcp_tools_call(
        ctx.app.clone(),
        &ctx.token,
        &session,
        "cp_ai_providers_list",
        serde_json::json!({ "team": ctx.team_name }),
    )
    .await;
    assert_eq!(
        listed["result"]["isError"], false,
        "cp_ai_providers_list must succeed: {listed}"
    );
    let content = &listed["result"]["structuredContent"];
    let items = content["items"]
        .as_array()
        .or_else(|| content.as_array())
        .unwrap_or_else(|| panic!("list structuredContent must carry items: {listed}"));
    let with_item = items
        .iter()
        .find(|item| item["name"] == with_scheme.as_str())
        .unwrap_or_else(|| panic!("{with_scheme} must be in the MCP list"));
    assert_eq!(
        with_item["spec"]["auth_scheme"], "Bearer",
        "MCP list must include auth_scheme when set: {with_item}"
    );
    let without_item = items
        .iter()
        .find(|item| item["name"] == without_scheme.as_str())
        .unwrap_or_else(|| panic!("{without_scheme} must be in the MCP list"));
    assert_auth_scheme_absent(&without_item["spec"], "MCP cp_ai_providers_list (unset)");
}
