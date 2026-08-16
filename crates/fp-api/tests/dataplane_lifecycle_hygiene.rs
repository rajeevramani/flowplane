//! RED REST contract for fpv2-7f3.8 recreation and retired-row discovery.
//!
//! Authored from the approved design/plan without inspecting production implementation. The tests
//! drive only the public router and observe UUIDv7-isolated PostgreSQL rows. They never truncate or
//! globally clean shared state.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::Arc;
use tokio::sync::Barrier;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

fn app(pool: sqlx::PgPool, validator: Arc<fp_core::OidcValidator>) -> axum::Router {
    fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(validator),
        write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        xds_degraded: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    })
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON response")
    }
}

async fn bounded(application: axum::Router, request: Request<Body>) -> axum::response::Response {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        application.oneshot(request),
    )
    .await
    .expect("REST request completed within five seconds")
    .expect("REST response")
}

fn request(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request")
}

fn items(body: &serde_json::Value) -> &[serde_json::Value] {
    body["items"].as_array().expect("dataplane page items")
}

#[test]
fn openapi_documents_include_retired_as_an_optional_boolean_read_filter() {
    let document = serde_json::to_value(fp_api::routes::openapi_document()).expect("OpenAPI JSON");
    let operation = &document["paths"]["/api/v1/teams/{team}/dataplanes"]["get"];
    let parameters = operation["parameters"].as_array().expect("list parameters");
    let include_retired = parameters
        .iter()
        .find(|parameter| parameter["name"] == "include_retired")
        .expect("dataplane list documents include_retired");
    assert_eq!(include_retired["in"], "query");
    assert_ne!(include_retired["required"], true);
    assert_eq!(include_retired["schema"]["type"], "boolean");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authorized_include_retired_lists_lifecycle_metadata_and_recreation_is_new_identity() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 12)
        .await
        .expect("connect real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");

    let issuer = DevIssuer::generate().expect("OIDC issuer");
    let validator = Arc::new(fp_core::OidcValidator::new(issuer.oidc_config()));
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("load JWKS");
    let owner_subject = unique("lifecycle-owner");
    let owner_token = issuer
        .mint(&owner_subject, "owner@test.invalid", "Owner", 600)
        .expect("owner token");
    let outsider_subject = unique("lifecycle-outsider");
    let outsider_token = issuer
        .mint(&outsider_subject, "outsider@test.invalid", "Outsider", 600)
        .expect("outsider token");

    let org = identity::create_org(&pool, &unique("lifecycle-org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("lifecycle-team"), "")
        .await
        .expect("team");
    let sibling = identity::create_team(&pool, org.id, &unique("lifecycle-sibling"), "")
        .await
        .expect("sibling team");
    let owner =
        identity::upsert_user_by_subject(&pool, &owner_subject, "owner@test.invalid", "Owner")
            .await
            .expect("owner user");
    identity::add_org_membership(&pool, owner, org.id, OrgRole::Admin)
        .await
        .expect("owner membership");

    let outsider_org = identity::create_org(&pool, &unique("lifecycle-outsider-org"), "")
        .await
        .expect("outsider org");
    let outsider = identity::upsert_user_by_subject(
        &pool,
        &outsider_subject,
        "outsider@test.invalid",
        "Outsider",
    )
    .await
    .expect("outsider user");
    identity::add_org_membership(&pool, outsider, outsider_org.id, OrgRole::Admin)
        .await
        .expect("outsider membership");

    let application = app(pool.clone(), validator);
    let list_path = format!("/api/v1/teams/{}/dataplanes", team.name);
    let name = unique("recreated-edge");
    let create_body = serde_json::json!({"name": name, "description": "first incarnation"});
    let created = bounded(
        application.clone(),
        request("POST", &list_path, &owner_token, Some(create_body)),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let first = json(created).await;
    let first_id = uuid::Uuid::parse_str(first["id"].as_str().expect("first id")).expect("UUID");
    let first_revision = first["revision"].as_i64().expect("first revision");
    let first_spiffe = format!("spiffe://flowplane.test/dataplane/{first_id}");
    let old_certificate_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
         (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
    )
    .bind(old_certificate_id)
    .bind(team.id.as_uuid())
    .bind(first_id)
    .bind(&first_spiffe)
    .bind(uuid::Uuid::now_v7().simple().to_string())
    .bind(format!(
        "{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    ))
    .execute(&pool)
    .await
    .expect("historical credential fixture");

    let item_path = format!("{list_path}/{name}");
    let retired = bounded(
        application.clone(),
        Request::builder()
            .method("DELETE")
            .uri(&item_path)
            .header("authorization", format!("Bearer {owner_token}"))
            .header("if-match", first_revision.to_string())
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"reason": "hardware replacement"}).to_string(),
            ))
            .expect("retire request"),
    )
    .await;
    assert_eq!(retired.status(), StatusCode::NO_CONTENT);

    let default_get = bounded(
        application.clone(),
        request("GET", &item_path, &owner_token, None),
    )
    .await;
    assert_eq!(default_get.status(), StatusCode::NOT_FOUND);
    let default_list = bounded(
        application.clone(),
        request("GET", &list_path, &owner_token, None),
    )
    .await;
    assert_eq!(default_list.status(), StatusCode::OK);
    let default_body = json(default_list).await;
    assert!(items(&default_body)
        .iter()
        .all(|row| row["id"] != first_id.to_string()));
    let explicit_active_only = bounded(
        application.clone(),
        request(
            "GET",
            &format!("{list_path}?include_retired=false"),
            &owner_token,
            None,
        ),
    )
    .await;
    assert_eq!(explicit_active_only.status(), StatusCode::OK);
    assert_eq!(
        json(explicit_active_only).await,
        default_body,
        "omitted include_retired and include_retired=false must be equivalent"
    );

    let retired_list = bounded(
        application.clone(),
        request(
            "GET",
            &format!("{list_path}?include_retired=true"),
            &owner_token,
            None,
        ),
    )
    .await;
    assert_eq!(retired_list.status(), StatusCode::OK);
    let retired_body = json(retired_list).await;
    let retired_row = items(&retired_body)
        .iter()
        .find(|row| row["id"] == first_id.to_string())
        .expect("authorized history contains retired dataplane");
    assert!(retired_row["retired_at"].is_string(), "{retired_row}");
    assert_eq!(retired_row["retired_reason"], "hardware replacement");
    for forbidden in [
        "private_key",
        "private_key_pem",
        "secret",
        "token",
        "certificate_pem",
    ] {
        assert!(
            retired_row.get(forbidden).is_none(),
            "retired row leaked {forbidden}"
        );
    }

    let recreated = bounded(
        application.clone(),
        request(
            "POST",
            &list_path,
            &owner_token,
            Some(serde_json::json!({"name": name, "description": "replacement"})),
        ),
    )
    .await;
    assert_eq!(recreated.status(), StatusCode::CREATED);
    let replacement = json(recreated).await;
    let replacement_id =
        uuid::Uuid::parse_str(replacement["id"].as_str().expect("replacement id")).expect("UUID");
    assert_ne!(replacement_id, first_id);
    let replacement_spiffe = format!("spiffe://flowplane.test/dataplane/{replacement_id}");
    assert_ne!(replacement_spiffe, first_spiffe);

    let history: (uuid::Uuid, String, bool) = sqlx::query_as(
        "SELECT dataplane_id, spiffe_uri, revoked_at IS NOT NULL \
         FROM proxy_certificates WHERE id = $1",
    )
    .bind(old_certificate_id)
    .fetch_one(&pool)
    .await
    .expect("old credential history retained");
    assert_eq!(history, (first_id, first_spiffe, true));
    assert_ne!(history.0, replacement_id);
    assert_ne!(history.1, replacement_spiffe);

    let all_rows = bounded(
        application.clone(),
        request(
            "GET",
            &format!("{list_path}?include_retired=true"),
            &owner_token,
            None,
        ),
    )
    .await;
    let all_rows = json(all_rows).await;
    assert!(items(&all_rows)
        .iter()
        .any(|row| row["id"] == first_id.to_string()));
    assert!(items(&all_rows)
        .iter()
        .any(|row| row["id"] == replacement_id.to_string()));

    let sibling_rows = bounded(
        application.clone(),
        request(
            "GET",
            &format!(
                "/api/v1/teams/{}/dataplanes?include_retired=true",
                sibling.name
            ),
            &owner_token,
            None,
        ),
    )
    .await;
    assert_eq!(sibling_rows.status(), StatusCode::OK);
    let sibling_rows = json(sibling_rows).await;
    assert!(items(&sibling_rows).iter().all(|row| {
        row["id"] != first_id.to_string() && row["id"] != replacement_id.to_string()
    }));

    let cross_org = bounded(
        application.clone(),
        request(
            "GET",
            &format!("{list_path}?include_retired=true"),
            &outsider_token,
            None,
        ),
    )
    .await;
    assert_eq!(cross_org.status(), StatusCode::NOT_FOUND);
    let cross_body = json(cross_org).await;
    let disclosure = cross_body.to_string();
    assert!(!disclosure.contains(&name));
    assert!(!disclosure.contains(&first_id.to_string()));
    assert!(!disclosure.contains(&replacement_id.to_string()));

    let race_name = unique("recreate-race");
    let retired_race_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dataplanes \
         (id, team_id, org_id, name, retired_at, retired_reason) \
         VALUES ($1, $2, $3, $4, now(), 'prior incarnation')",
    )
    .bind(retired_race_id)
    .bind(team.id.as_uuid())
    .bind(org.id.as_uuid())
    .bind(&race_name)
    .execute(&pool)
    .await
    .expect("retired race fixture");
    let barrier = Arc::new(Barrier::new(2));
    let race = |barrier: Arc<Barrier>| {
        let application = application.clone();
        let token = owner_token.clone();
        let path = list_path.clone();
        let race_name = race_name.clone();
        async move {
            barrier.wait().await;
            bounded(
                application,
                request(
                    "POST",
                    &path,
                    &token,
                    Some(serde_json::json!({"name": race_name, "description": "winner"})),
                ),
            )
            .await
        }
    };
    let (left, right) = tokio::join!(race(Arc::clone(&barrier)), race(Arc::clone(&barrier)));
    let statuses = [left.status(), right.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let active_ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM dataplanes WHERE team_id = $1 AND name = $2 AND retired_at IS NULL",
    )
    .bind(team.id.as_uuid())
    .bind(&race_name)
    .fetch_all(&pool)
    .await
    .expect("active race winner");
    assert_eq!(active_ids.len(), 1, "one stable active winner");
    assert_ne!(active_ids[0], retired_race_id);
}
