//! S2 reference-inventory contracts from approved design lines 94/103/106.
//! Black-box real scoped router, RS256 auth and PostgreSQL; no production reads.
//! JSON fixture shapes follow the public v2 OpenAPI baseline (3.1.4); new removal
//! expectations come from the approved S2 design, not that historical baseline.
//! Only FLOWPLANE_TEST_DATABASE_URL is recognized. Unset skips with a notice;
//! configured failures fail. Each runtime case owns a migrated CREATEDB database.
//! Compilation provisions nothing. No raw SQL fabricates gateway references.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use fp_core::dev::DevIssuer;
use fp_domain::{OrgRole, TeamId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use serde_json::{json, Value};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::{future::Future, net::TcpListener, str::FromStr, sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7().simple())
}

// A control pool connected to the configured database is used ONLY for CREATE /
// DROP DATABASE. All fixture data, migrations and triggers use the owned database.
struct Scratch {
    control: PgPool,
    name: String,
    options: PgConnectOptions,
}
impl Scratch {
    async fn new() -> Option<Self> {
        let url = match std::env::var("FLOWPLANE_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(std::env::VarError::NotPresent) => {
                eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
                return None;
            }
            Err(_) => panic!("invalid FLOWPLANE_TEST_DATABASE_URL (credentials redacted)"),
        };
        let options = PgConnectOptions::from_str(&url)
            .unwrap_or_else(|_| panic!("invalid test database URL (credentials redacted)"));
        let control = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap_or_else(|_| {
                panic!("cannot connect to configured PostgreSQL (credentials redacted)")
            });
        let name = format!("aidf_s2_inventory_{}", Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&control)
            .await
            .expect("test role needs CREATEDB");
        Some(Self {
            control,
            name: name.clone(),
            options: options.database(&name),
        })
    }
    async fn drop_database(self) {
        // FORCE closes connections even if fixture setup or a spawned assertion
        // panicked. Never terminate sessions in the configured/shared database.
        sqlx::query(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
            .execute(&self.control)
            .await
            .expect("drop owned AIDF scratch database");
        self.control.close().await;
    }
}

#[derive(Clone)]
struct Fixture {
    app: Router,
    pool: PgPool,
    team: TeamId,
    team_name: String,
    token: String,
    socket: Arc<TcpListener>,
}
impl Fixture {
    async fn new(options: PgConnectOptions) -> Self {
        let pool = PgPoolOptions::new()
            .max_connections(12)
            .connect_with(options)
            .await
            .unwrap_or_else(|_| {
                panic!("cannot connect to owned scratch PostgreSQL (credentials redacted)")
            });
        fp_storage::migrate(&pool).await.expect("normal migrations");
        let issuer = DevIssuer::generate().expect("issuer");
        let validator = fp_core::OidcValidator::new(issuer.oidc_config());
        validator
            .load_jwks_json(issuer.jwks_json())
            .await
            .expect("JWKS");
        let subject = unique("admin");
        let token = issuer
            .mint(&subject, "adversarial@test", "S2 contract", 3600)
            .expect("JWT");
        let org = identity::create_org(&pool, &unique("org"), "")
            .await
            .expect("org");
        let team = identity::create_team(&pool, org.id, &unique("team"), "")
            .await
            .expect("team");
        let user =
            identity::upsert_user_by_subject(&pool, &subject, "adversarial@test", "S2 contract")
                .await
                .expect("user");
        identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
            .await
            .expect("admin membership");
        let app = fp_api::build_router(fp_api::AppState {
            pool: pool.clone(),
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            version: "s2-independent-adversarial",
            validator: Some(Arc::new(validator)),
            write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1_000_000)),
            xds_readiness: None,
            xds_degraded: None,
            discovery_forwarding_policy: Default::default(),
            egress_advisory: Default::default(),
            rls_repush: None,
            rls_grpc_configured: false,
        });
        Self {
            app,
            pool,
            team: team.id,
            team_name: team.name,
            token,
            socket: Arc::new(TcpListener::bind("127.0.0.1:0").expect("owned explicit port")),
        }
    }
    fn expose_body(&self, name: &str, auto: bool) -> Value {
        let mut body = json!({"name":name,"upstream":"http://127.0.0.1:3001","path":"/owned","public_base_url":"https://gateway.example"});
        if !auto {
            body["port"] = json!(self.socket.local_addr().unwrap().port());
        }
        body
    }
    async fn request(
        &self,
        method: &str,
        suffix: &str,
        body: Option<Value>,
        revision: Option<i64>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/api/v1/teams/{}/{suffix}", self.team_name))
            .header("authorization", format!("Bearer {}", self.token));
        if let Some(revision) = revision {
            builder = builder.header("if-match", revision.to_string());
        }
        let request = match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("request");
        let response =
            tokio::time::timeout(Duration::from_secs(30), self.app.clone().oneshot(request))
                .await
                .expect("router request deadline (including lock wait)")
                .expect("real router");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("JSON")
        };
        (status, body)
    }
    async fn expect(
        &self,
        method: &str,
        suffix: &str,
        body: Option<Value>,
        revision: Option<i64>,
        expected: StatusCode,
    ) -> Value {
        let (status, body) = self.request(method, suffix, body, revision).await;
        assert_eq!(status, expected, "{method} {suffix}: {body}");
        body
    }
    async fn expose(&self, name: &str) -> Value {
        self.expect(
            "POST",
            "expose",
            Some(self.expose_body(name, false)),
            None,
            StatusCode::CREATED,
        )
        .await
    }
    async fn remove(&self, name: &str, expected: StatusCode) -> Value {
        self.expect("DELETE", &format!("expose/{name}"), None, None, expected)
            .await
    }
    // Exact snapshots include normalized refs, IDs, revisions, associations,
    // dependent rows and all committed success audit/outbox effects. Failure or
    // denial audit is permitted by design, never misclassified as success.
    async fn snapshot(&self) -> Value {
        let mut map = serde_json::Map::new();
        for table in [
            "clusters",
            "route_configs",
            "listeners",
            "exposures",
            "route_config_cluster_refs",
            "listener_route_config_refs",
            "api_definitions",
            "api_route_bindings",
            "capture_sessions",
            "ai_budgets",
        ] {
            let rows: Vec<Value> = sqlx::query_scalar(&format!(
                "SELECT to_jsonb(t) FROM {table} t WHERE team_id=$1 ORDER BY to_jsonb(t)::text"
            ))
            .bind(self.team.as_uuid())
            .fetch_all(&self.pool)
            .await
            .expect("scoped row snapshot");
            map.insert(table.into(), json!(rows));
        }
        let events: Vec<Value> =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM events e WHERE team_id=$1 ORDER BY seq")
                .bind(self.team.as_uuid())
                .fetch_all(&self.pool)
                .await
                .expect("outbox");
        let audits: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_log a WHERE team_id=$1 AND outcome='success' ORDER BY to_jsonb(a)::text")
            .bind(self.team.as_uuid()).fetch_all(&self.pool).await.expect("success audit");
        map.insert("events".into(), json!(events));
        map.insert("audit".into(), json!(audits));
        Value::Object(map)
    }
    async fn assert_absent(&self, name: &str) {
        for suffix in [
            format!("clusters/{name}-upstream"),
            format!("route-configs/{name}-routes"),
            format!("listeners/{name}"),
        ] {
            self.expect("GET", &suffix, None, None, StatusCode::NOT_FOUND)
                .await;
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM exposures WHERE team_id=$1 AND name=$2")
                .bind(self.team.as_uuid())
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .expect("association count");
        assert_eq!(count, 0);
    }
    async fn assert_visible(&self, created: &Value) {
        for (kind, key) in [
            ("clusters", "cluster"),
            ("route-configs", "route_config"),
            ("listeners", "listener"),
        ] {
            let view = self
                .expect(
                    "GET",
                    &format!("{kind}/{}", created[key]["name"].as_str().unwrap()),
                    None,
                    None,
                    StatusCode::OK,
                )
                .await;
            assert_eq!(
                view, created[key],
                "exact ID/revision/spec must survive rollback"
            );
        }
    }

    async fn create(&self, kind: &str, name: &str, spec: Value) -> Value {
        self.expect(
            "POST",
            kind,
            Some(json!({"name":name,"spec":spec})),
            None,
            StatusCode::CREATED,
        )
        .await
    }
    async fn assert_blocked(&self, name: &str, created: &Value, label: &str) {
        let before = self.snapshot().await;
        let body = self.remove(name, StatusCode::CONFLICT).await;
        assert_eq!(body["code"], "conflict", "{label}: {body}");
        assert_eq!(self.snapshot().await, before,
            "{label}: exact graph, refs, revisions, association, outbox and success audits unchanged");
        self.assert_visible(created).await;
    }
    async fn assert_cleaned(&self, name: &str) {
        let removed = self.remove(name, StatusCode::OK).await;
        for field in [
            "cluster_disposition",
            "route_config_disposition",
            "listener_disposition",
        ] {
            assert_eq!(
                removed[field], "deleted",
                "final managed cleanup: {removed}"
            );
        }
        self.assert_absent(name).await;
    }
}

// Setup also runs inside the joined task: a setup/assertion panic still drops the
// owned database. No runtime resources are provisioned by compilation.
async fn case<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(scratch) = Scratch::new().await else {
        return;
    };
    let options = scratch.options.clone();
    let outcome = tokio::spawn(async move {
        let fixture = Fixture::new(options).await;
        test(fixture.clone()).await;
        fixture.pool.close().await;
    })
    .await;
    scratch.drop_database().await;
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("contract task cancelled");
    }
}
// Each filter lives on a separate ordinary listener, not the scaffold that will
// be deleted. It has no route_config reference: only this filter can block the
// upstream's cleanup. PATCH removes that reference while retaining its owner.
async fn filter_case(kind: &'static str) {
    case(move |f| async move {
        let name = unique("exposed");
        let created = f.expose(&name).await;
        let upstream = created["cluster"]["name"].as_str().unwrap();
        let filter = match kind {
            "ext_authz" => json!({"type":kind,"cluster":upstream}),
            "jwt_auth" => json!({"type":kind,"providers":{"issuer":{
                "issuer":"https://issuer.example", "jwks":{"source":"remote",
                "uri":"https://issuer.example/.well-known/jwks.json","cluster":upstream}}}}),
            "global_rate_limit" => json!({"type":kind,"domain":"inventory-contract",
                "service_cluster":upstream}),
            _ => unreachable!(),
        };
        let dependent = unique("filter-owner");
        let socket = TcpListener::bind("127.0.0.1:0").expect("independent listener port");
        let listener = f
            .create(
                "listeners",
                &dependent,
                json!({"address":"0.0.0.0",
            "port":socket.local_addr().unwrap().port(),"protocol":"http",
            "http_filters":[{"filter":filter}]}),
            )
            .await;
        let path = format!("listeners/{dependent}");
        let get = f.expect("GET", &path, None, None, StatusCode::OK).await;
        assert_eq!(
            get, listener,
            "ordinary API persisted the real filter fixture"
        );
        let persisted = &get["spec"]["http_filters"][0]["filter"];
        assert_eq!(persisted["type"], kind);
        let reference = match kind {
            "ext_authz" => &persisted["cluster"],
            "jwt_auth" => {
                assert_eq!(persisted["providers"]["issuer"]["jwks"]["source"], "remote");
                &persisted["providers"]["issuer"]["jwks"]["cluster"]
            }
            "global_rate_limit" => &persisted["service_cluster"],
            _ => unreachable!(),
        };
        assert_eq!(
            reference, upstream,
            "public filter representation must retain the upstream reference"
        );
        f.assert_blocked(&name, &created, kind).await;
        let mut spec = listener["spec"].clone();
        spec["http_filters"] = json!([]);
        let changed = f
            .expect(
                "PATCH",
                &path,
                Some(json!({"spec":spec})),
                Some(listener["revision"].as_i64().unwrap()),
                StatusCode::OK,
            )
            .await;
        assert_eq!(changed["id"], listener["id"]);
        assert_eq!(
            changed["revision"].as_i64().unwrap(),
            listener["revision"].as_i64().unwrap() + 1
        );
        // Empty optional filter chains are omitted from the public representation.
        assert!(changed["spec"]
            .get("http_filters")
            .is_none_or(|v| v == &json!([])));
        f.assert_cleaned(&name).await;
        assert_eq!(
            f.expect("GET", &path, None, None, StatusCode::OK).await,
            changed,
            "unrelated surviving listener is not rewritten or deleted"
        );
    })
    .await;
}

#[tokio::test]
async fn surviving_ext_authz_cluster_blocks_atomic_unexpose_until_filter_removed() {
    filter_case("ext_authz").await;
}
#[tokio::test]
async fn surviving_jwt_remote_jwks_cluster_blocks_atomic_unexpose_until_filter_removed() {
    filter_case("jwt_auth").await;
}
#[tokio::test]
async fn surviving_global_rate_limit_service_cluster_blocks_atomic_unexpose_until_filter_removed() {
    filter_case("global_rate_limit").await;
}
#[tokio::test]
async fn surviving_aggregate_cluster_blocks_atomic_unexpose_until_ordinary_delete() {
    case(|f| async move {
        let name = unique("exposed");
        let created = f.expose(&name).await;
        let dependent = unique("aggregate");
        let aggregate = f
            .create(
                "clusters",
                &dependent,
                json!({"endpoints":[],
            "aggregate_clusters":[created["cluster"]["name"]]}),
            )
            .await;
        let path = format!("clusters/{dependent}");
        assert_eq!(
            f.expect("GET", &path, None, None, StatusCode::OK).await,
            aggregate
        );
        assert_eq!(
            aggregate["spec"]["aggregate_clusters"],
            json!([created["cluster"]["name"]])
        );
        f.assert_blocked(&name, &created, "aggregate cluster").await;
        f.expect(
            "DELETE",
            &path,
            None,
            Some(aggregate["revision"].as_i64().unwrap()),
            StatusCode::NO_CONTENT,
        )
        .await;
        f.expect("GET", &path, None, None, StatusCode::NOT_FOUND)
            .await;
        f.assert_cleaned(&name).await;
    })
    .await;
}
#[tokio::test]
async fn ai_budget_config_reference_blocks_final_managed_cleanup_until_ordinary_delete() {
    case(|f| async move {
        let name = unique("exposed");
        let created = f.expose(&name).await;
        let dependent = unique("budget");
        let budget = f
            .create(
                "ai/budgets",
                &dependent,
                json!({"mode":"shadow",
            "limit_units":10000,"window_seconds":3600,
            "route_config_id":created["route_config"]["id"]}),
            )
            .await;
        assert_eq!(
            budget["spec"]["route_config_id"],
            created["route_config"]["id"]
        );
        f.assert_blocked(&name, &created, "AI budget config FK")
            .await;
        let path = format!("ai/budgets/{dependent}");
        f.expect(
            "DELETE",
            &path,
            None,
            Some(budget["revision"].as_i64().unwrap()),
            StatusCode::NO_CONTENT,
        )
        .await;
        f.expect("GET", &path, None, None, StatusCode::NOT_FOUND)
            .await;
        f.assert_cleaned(&name).await;
    })
    .await;
}
