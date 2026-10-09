//! Independent remaining S2/B3 retry, structured SQLSTATE and auth-order contracts.
//! Only approved design/OpenAPI and allowed existing integration fixtures were read.
//! Real router/JWT/owned normally migrated PostgreSQL; no production implementation reads.
//! FLOWPLANE_TEST_DATABASE_URL unset visibly skips; a configured prerequisite must fail
//! rather than silently skip. Compilation provisions nothing; parent owns runtime RED.
//! Synthetic deferred SQLSTATE triggers never modify the configured base database.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use fp_core::dev::DevIssuer;
use fp_domain::{
    authz::{Action, Resource},
    OrgRole, TeamId,
};
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
        let name = format!("aidf_s2_retry_auth_{}", Uuid::now_v7().simple());
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
    denied_tokens: Vec<String>,
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
        let mut denied_tokens = Vec::new();
        for missing in [
            Resource::Clusters,
            Resource::RouteConfigs,
            Resource::Listeners,
        ] {
            let subject = unique("missing-create");
            let user =
                identity::upsert_user_by_subject(&pool, &subject, "denied@test", "Missing Create")
                    .await
                    .expect("member user");
            identity::add_org_membership(&pool, user, org.id, OrgRole::Member)
                .await
                .expect("non-admin member");
            for resource in [
                Resource::Clusters,
                Resource::RouteConfigs,
                Resource::Listeners,
            ] {
                identity::add_grant(&pool, user, org.id, team.id, resource, Action::Read, None)
                    .await
                    .expect("team read grant");
                if resource != missing {
                    identity::add_grant(
                        &pool,
                        user,
                        org.id,
                        team.id,
                        resource,
                        Action::Create,
                        None,
                    )
                    .await
                    .expect("other team create grant");
                }
            }
            denied_tokens.push(
                issuer
                    .mint(&subject, "denied@test", "Missing Create", 3600)
                    .expect("member JWT"),
            );
        }
        let app = fp_api::build_router(fp_api::AppState {
            pool: pool.clone(),
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            version: "s2-independent-retry-auth",
            validator: Some(Arc::new(validator)),
            write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1_000_000)),
            xds_readiness: None,
            xds_degraded: None,
            discovery_forwarding_policy: Default::default(),
            egress_advisory: fp_core::services::egress_advisory::EgressAdvisoryPolicy::new(
                true,
                Vec::new(),
                Vec::new(),
            ),
            rls_repush: None,
            rls_grpc_configured: false,
        });
        Self {
            app,
            pool,
            team: team.id,
            denied_tokens,
            team_name: team.name,
            token,
            socket: Arc::new(TcpListener::bind("127.0.0.1:0").expect("owned explicit port")),
        }
    }
    fn expose_body(&self, name: &str, auto: bool) -> Value {
        let mut body = json!({"name":name,"upstream":"http://8.8.8.8:3001","path":"/owned","public_base_url":"https://gateway.example"});
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

    // Synthetic SQLSTATEs test shortcut translation, not actual concurrency scheduling.
    // Immediate statement triggers reach the reused helper boundary; the
    // nontransactional sequence witnesses faults despite transaction rollback.
    async fn inject(&self, table: &str, operation: &str, state: &str, constraint: &str) {
        assert!([
            "clusters",
            "route_configs",
            "listeners",
            "audit_log",
            "events"
        ]
        .contains(&table));
        assert!(["INSERT", "DELETE"].contains(&operation));
        assert!(["40P01", "40001", "23503"].contains(&state));
        assert!([
            "",
            "exposures_cluster_fk",
            "exposures_route_config_fk",
            "exposures_listener_fk"
        ]
        .contains(&constraint));
        let row = if operation == "DELETE" { "OLD" } else { "NEW" };
        let sql = format!(
            r#"
            CREATE SEQUENCE aidf_fault_witness;
            CREATE FUNCTION aidf_raise_fault() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
              IF {row}.team_id = '{}'::uuid THEN
                PERFORM nextval('aidf_fault_witness');
                RAISE EXCEPTION 'owned scratch injected fault'
                  USING ERRCODE='{state}', CONSTRAINT='{constraint}';
              END IF;
              RETURN {row};
            END $$;
            CREATE TRIGGER aidf_fault AFTER {operation} ON {table}
            FOR EACH ROW EXECUTE FUNCTION aidf_raise_fault();
        "#,
            self.team.as_uuid()
        );
        sqlx::raw_sql(&sql)
            .execute(&self.pool)
            .await
            .expect("owned deferred fault trigger");
    }
    async fn assert_fault(&self) {
        let fired: bool = sqlx::query_scalar("SELECT is_called FROM aidf_fault_witness")
            .fetch_one(&self.pool)
            .await
            .expect("nontransactional fault witness");
        assert!(fired, "request must reach injected boundary");
    }
    async fn clear_fault(&self, table: &str) {
        sqlx::raw_sql(&format!("DROP TRIGGER aidf_fault ON {table}; DROP FUNCTION aidf_raise_fault(); DROP SEQUENCE aidf_fault_witness;"))
            .execute(&self.pool).await.expect("remove owned fault for positive control");
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
fn assert_conflict(body: &Value) {
    assert_eq!(body["code"], "conflict", "stable conflict envelope: {body}");
}
fn count(snapshot: &Value, table: &str) -> usize {
    snapshot[table].as_array().unwrap().len()
}
fn assert_new_effects(before: &Value, after: &Value, verbs: &[(&str, &str)]) {
    let old_events = before["events"].as_array().unwrap();
    let old_audit = before["audit"].as_array().unwrap();
    let events: Vec<_> = after["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !old_events.contains(row))
        .collect();
    let audits: Vec<_> = after["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !old_audit.contains(row))
        .collect();
    assert_eq!(
        events.len(),
        verbs.len(),
        "no orphan/compensating outbox effects"
    );
    assert_eq!(
        audits.len(),
        verbs.len(),
        "no orphan/shortcut success audits"
    );
    for &(resource, verb) in verbs {
        let event = if verb == "delete" {
            "deleted"
        } else {
            "upserted"
        };
        assert_eq!(
            events
                .iter()
                .filter(|row| row["event_type"] == format!("{resource}.{event}"))
                .count(),
            verbs
                .iter()
                .filter(|&&pair| pair == (resource, verb))
                .count()
        );
        assert_eq!(
            audits
                .iter()
                .filter(|row| row["action"] == format!("{resource}.{verb}"))
                .count(),
            verbs
                .iter()
                .filter(|&&pair| pair == (resource, verb))
                .count()
        );
    }
}

// Each fault is its own database/test, so a first RED cannot hide later matrix rows.
async fn create_fault(state: &'static str, table: &'static str) {
    case(move |f| async move {
        let before = f.snapshot().await;
        let name = unique("fault-create");
        f.inject(table, "INSERT", state, "").await;
        let (status, body) = f
            .request("POST", "expose", Some(f.expose_body(&name, false)), None)
            .await;
        eprintln!(
            "create fault case SQLSTATE={state} boundary={table} status={status} code={}",
            body["code"]
        );
        f.assert_fault().await;
        assert_eq!(
            f.snapshot().await,
            before,
            "{state}/{table}: exact rollback"
        );
        f.assert_absent(&name).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "shortcut SQLSTATE {state}/{table}: {body}"
        );
        assert_conflict(&body);
        // The same helper fault on ordinary CRUD keeps its historical 500/redaction.
        let ordinary = f
            .expect(
                "POST",
                "clusters",
                Some(json!({
                    "name": unique("ordinary-fault"),
                    "spec": {"endpoints": [{"host": "8.8.8.8", "port": 3001}]}
                })),
                None,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await;
        assert_eq!(ordinary["code"], "internal");
        assert!(ordinary.get("details").is_none_or(Value::is_null));
        assert_eq!(
            f.snapshot().await,
            before,
            "ordinary failed mutation also rolls back"
        );
        f.clear_fault(table).await;
        let created = f.expose(&name).await;
        f.assert_visible(&created).await;
        let after = f.snapshot().await;
        assert_new_effects(
            &before,
            &after,
            &[
                ("cluster", "create"),
                ("route_config", "create"),
                ("listener", "create"),
            ],
        );
        f.remove(&name, StatusCode::OK).await;
    })
    .await;
}
macro_rules! create_fault_case {
    ($name:ident, $state:literal, $table:literal) => {
        #[tokio::test]
        async fn $name() {
            create_fault($state, $table).await;
        }
    };
}
create_fault_case!(
    deadlock_reused_cluster_insert_is_atomic_conflict,
    "40P01",
    "clusters"
);
create_fault_case!(
    serialization_reused_cluster_insert_is_atomic_conflict,
    "40001",
    "clusters"
);
create_fault_case!(
    deadlock_success_audit_insert_is_atomic_conflict,
    "40P01",
    "audit_log"
);
create_fault_case!(
    serialization_success_audit_insert_is_atomic_conflict,
    "40001",
    "audit_log"
);
create_fault_case!(deadlock_outbox_insert_is_atomic_conflict, "40P01", "events");
create_fault_case!(
    serialization_outbox_insert_is_atomic_conflict,
    "40001",
    "events"
);

async fn delete_fault(table: &'static str, constraint: &'static str) {
    case(move |f| async move {
        let name = unique("fault-delete");
        let created = f.expose(&name).await;
        let before = f.snapshot().await;
        f.inject(table, "DELETE", "23503", constraint).await;
        let (status, body) = f
            .request("DELETE", &format!("expose/{name}"), None, None)
            .await;
        eprintln!(
            "delete fault case constraint={constraint} boundary={table} status={status} code={}",
            body["code"]
        );
        f.assert_fault().await;
        assert_eq!(
            f.snapshot().await,
            before,
            "named FK rollback includes association, revisions, refs, success effects"
        );
        f.assert_visible(&created).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "scoped named FK {constraint}: {body}"
        );
        assert_conflict(&body);
        f.clear_fault(table).await;
        f.remove(&name, StatusCode::OK).await;
        let after = f.snapshot().await;
        for table in [
            "clusters",
            "route_configs",
            "listeners",
            "exposures",
            "route_config_cluster_refs",
            "listener_route_config_refs",
        ] {
            assert_eq!(after[table], json!([]), "successful delete {table}");
        }
        assert_new_effects(
            &before,
            &after,
            &[
                ("cluster", "delete"),
                ("route_config", "delete"),
                ("listener", "delete"),
            ],
        );
    })
    .await;
}
#[tokio::test]
async fn named_cluster_fk_shortcut_delete_is_atomic_conflict() {
    delete_fault("clusters", "exposures_cluster_fk").await;
}
#[tokio::test]
async fn named_route_config_fk_shortcut_delete_is_atomic_conflict() {
    delete_fault("route_configs", "exposures_route_config_fk").await;
}
#[tokio::test]
async fn named_listener_fk_shortcut_delete_is_atomic_conflict() {
    delete_fault("listeners", "exposures_listener_fk").await;
}

#[tokio::test]
async fn hidden_discovery_listener_collision_retries_without_orphan_graph_or_effects() {
    case(|f| async move {
        let hidden = unique("hidden-port");
        let listener = f.expect("POST", "listeners", Some(json!({"name":hidden,
            "spec":{"address":"0.0.0.0","port":10000,"protocol":"http"}})), None, StatusCode::CREATED).await;
        // Ordinary public creation supplies every required field. Inspect its owned
        // persisted schema before changing ONLY metadata; never relax constraints.
        let id = Uuid::parse_str(listener["id"].as_str().unwrap()).unwrap();
        let persisted: Value = sqlx::query_scalar("SELECT to_jsonb(l) FROM listeners l WHERE id=$1 AND team_id=$2")
            .bind(id).bind(f.team.as_uuid()).fetch_one(&f.pool).await.expect("ordinary fixture schema");
        assert_eq!(persisted["owner_kind"], "user", "fixture must expose established ownership metadata");
        let changed = sqlx::query("UPDATE listeners SET owner_kind='discovery', owner_id=id WHERE id=$1 AND team_id=$2 AND owner_kind='user'")
            .bind(id).bind(f.team.as_uuid()).execute(&f.pool).await.expect("normally migrated schema must permit system metadata; no constraint weakening");
        assert_eq!(changed.rows_affected(), 1);
        f.expect("GET", &format!("listeners/{hidden}"), None, None, StatusCode::NOT_FOUND).await;
        let listing = f.expect("GET", "listeners", None, None, StatusCode::OK).await;
        let items = listing["items"].as_array().or_else(|| listing.as_array()).expect("public listener list");
        assert!(!items.iter().any(|item| item["id"] == listener["id"]), "allocator's public list must hide collision");
        let before = f.snapshot().await;
        let name = unique("retry");
        let created = f.expect("POST", "expose", Some(f.expose_body(&name, true)), None, StatusCode::CREATED).await;
        assert_eq!(created["mode"], "created");
        assert_eq!(created["listener"]["spec"]["port"], 10001, "skip occupied first auto-port");
        f.assert_visible(&created).await;
        let after = f.snapshot().await;
        for table in ["clusters", "route_configs", "listeners", "exposures", "route_config_cluster_refs", "listener_route_config_refs"] {
            assert_eq!(count(&after, table), count(&before, table)+1, "exactly one graph, no retry orphan: {table}");
            for old in before[table].as_array().unwrap() {
                assert!(after[table].as_array().unwrap().contains(old), "collision fixture unchanged: {table}");
            }
        }
        let association = &after["exposures"][0];
        for (column, key) in [("cluster_id", "cluster"), ("route_config_id", "route_config"), ("listener_id", "listener")] {
            assert_eq!(association[column], created[key]["id"]);
        }
        assert_new_effects(&before, &after, &[("cluster", "create"), ("route_config", "create"), ("listener", "create")]);
        f.remove(&name, StatusCode::OK).await;
        let cleaned = f.snapshot().await;
        for table in ["clusters", "route_configs", "listeners", "exposures", "route_config_cluster_refs", "listener_route_config_refs"] {
            assert_eq!(cleaned[table], before[table], "cleanup leaves only original hidden fixture: {table}");
        }
    }).await;
}

#[tokio::test]
async fn each_missing_create_grant_denies_before_upstream_validation_or_advisory() {
    case(|f| async move {
        // Successful authenticated admin control proves the enabled advisory/router
        // fixture is usable, without relying on external DNS for baseline traffic.
        let control = unique("auth-control");
        f.expose(&control).await;
        f.remove(&control, StatusCode::OK).await;
        for (index, token) in f.denied_tokens.iter().enumerate() {
            let mut caller = f.clone();
            caller.token = token.clone();
            // Read is granted, isolating missing Create from team/selector denial.
            caller
                .expect("GET", "listeners", None, None, StatusCode::OK)
                .await;
            for upstream in [
                "http://unresolvable-s2-contract.invalid:8080",
                "not-an-upstream-url",
            ] {
                let before = f.snapshot().await;
                let name = unique("auth-denied");
                let mut request = f.expose_body(&name, true);
                request["upstream"] = json!(upstream);
                let (status, body) = caller.request("POST", "expose", Some(request), None).await;
                eprintln!(
                    "missing Create index={index} upstream={upstream} status={status} code={}",
                    body["code"]
                );
                assert_eq!(
                    f.snapshot().await,
                    before,
                    "denial has no product/outbox/success-audit writes"
                );
                f.assert_absent(&name).await;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "authorization must precede upstream validation/advisory: {body}"
                );
                assert_eq!(body["code"], "forbidden");
                assert!(body["message"].is_string());
                for key in ["cluster", "route_config", "listener", "items", "spec"] {
                    assert!(
                        body.get(key).is_none(),
                        "denial must not return product data"
                    );
                }
            }
        }
        // These are response-precedence and persistence assertions. DNS/socket
        // activity is not instrumented: this test makes no network-observation claim.
    })
    .await;
}
