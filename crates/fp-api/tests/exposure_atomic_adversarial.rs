//! Independent approved-contract S2 tests (AC6/7/12): real router, JWT and PostgreSQL.
//! No expose handler/core service/storage repository implementation was inspected.
//! Every case owns a uniquely named, normally migrated AIDF database; schema-changing
//! failure injection never touches the configured base database. No S3 selector.
//! FLOWPLANE_TEST_DATABASE_URL is the only DB prerequisite; unset means a visible skip.
//! The connection role must have CREATEDB and ownership rights to drop its database.
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
        let name = format!("aidf_s2_exposure_{}", Uuid::now_v7().simple());
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
    org: Uuid,
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
            org: org.id.as_uuid(),
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
    // Deferred constraint trigger fails at transaction commit, AFTER the graph,
    // normalized refs, association and audits have been staged. A sequence is a
    // nontransactional witness proving injection fired even though rows roll back.
    async fn inject_late_outbox_failure(&self, suffix: &str) {
        let sql = format!(
            r#"
            CREATE SEQUENCE aidf_outbox_failure_witness;
            CREATE FUNCTION aidf_fail_outbox_commit() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
              IF NEW.team_id = '{}'::uuid AND NEW.event_type IN
                 ('cluster.{suffix}', 'route_config.{suffix}', 'listener.{suffix}') THEN
                PERFORM nextval('aidf_outbox_failure_witness');
                RAISE EXCEPTION 'AIDF owned scratch late outbox failure' USING ERRCODE='45000';
              END IF;
              RETURN NEW;
            END $$;
            CREATE CONSTRAINT TRIGGER aidf_outbox_commit_failure AFTER INSERT ON events
            DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION aidf_fail_outbox_commit();
        "#,
            self.team.as_uuid()
        );
        sqlx::raw_sql(&sql)
            .execute(&self.pool)
            .await
            .expect("scratch-local deferred outbox trigger");
    }
    async fn assert_injection_fired(&self) {
        let fired: bool = sqlx::query_scalar("SELECT is_called FROM aidf_outbox_failure_witness")
            .fetch_one(&self.pool)
            .await
            .expect("nontransactional failure witness");
        assert!(
            fired,
            "failure must reach the late outbox boundary, not fail earlier"
        );
    }
    async fn remove_injection(&self) {
        sqlx::raw_sql("DROP TRIGGER aidf_outbox_commit_failure ON events; DROP FUNCTION aidf_fail_outbox_commit(); DROP SEQUENCE aidf_outbox_failure_witness;")
            .execute(&self.pool).await.expect("remove scratch trigger for positive control");
    }
    async fn add_genuine_route(&self, name: &str, created: &Value) -> (Value, Value) {
        let other = unique("unrelated");
        let cluster = self
            .expect(
                "POST",
                "clusters",
                Some(json!({"name":other,"spec":cluster_spec()})),
                None,
                StatusCode::CREATED,
            )
            .await;
        let mut spec = created["route_config"]["spec"].clone();
        spec["virtual_hosts"][0]["routes"].as_array_mut().unwrap().push(json!({"name":"genuine-other","match":{"prefix":{"prefix":"/other"}},"action":{"cluster":other,"timeout_secs":17}}));
        let config = self
            .expect(
                "PATCH",
                &format!("route-configs/{name}-routes"),
                Some(json!({"spec":spec})),
                Some(created["route_config"]["revision"].as_i64().unwrap()),
                StatusCode::OK,
            )
            .await;
        (cluster, config)
    }
    // Schema-backed fixture only: these are real FK-constrained persisted capture
    // rows, not fake services. Completed/Cancelled/Failed are pre-existing history.
    // This does not invent a production capture deletion/retention surface.
    async fn seed_capture(&self, created: &Value, scope: &str, status: &str) -> String {
        let name = unique("capture");
        let config = Uuid::parse_str(created["route_config"]["id"].as_str().unwrap()).unwrap();
        let listener = Uuid::parse_str(created["listener"]["id"].as_str().unwrap()).unwrap();
        let vhost = if scope == "config" {
            None
        } else {
            Some("default")
        };
        let route = if scope == "route" {
            Some(created["listener"]["name"].as_str().unwrap())
        } else {
            None
        };
        sqlx::query("INSERT INTO capture_sessions (id,team_id,org_id,name,status,route_config_id,listener_id,virtual_host,route,target_sample_count,max_bytes,max_distinct_paths) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,25,4096,20)")
            .bind(Uuid::now_v7()).bind(self.team.as_uuid()).bind(self.org).bind(&name).bind(status)
            .bind(config).bind(listener).bind(vhost).bind(route).execute(&self.pool).await.expect("real direct capture fixture");
        name
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
fn cluster_spec() -> Value {
    json!({"endpoints":[{"host":"127.0.0.1","port":3001}]})
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

#[tokio::test]
async fn late_outbox_create_failure_rolls_back_graph_association_refs_and_success_effects() {
    case(|f| async move {
        let before = f.snapshot().await;
        f.inject_late_outbox_failure("upserted").await;
        let name = unique("late-create");
        let (status, body) = f
            .request("POST", "expose", Some(f.expose_body(&name, false)), None)
            .await;
        assert!(
            status.is_server_error(),
            "injected persistence error must fail: {status} {body}"
        );
        f.assert_injection_fired().await;
        assert_eq!(
            f.snapshot().await,
            before,
            "commit-time outbox failure must roll back EVERY effect"
        );
        f.assert_absent(&name).await;
        f.remove_injection().await;
        let created = f.expose(&name).await;
        f.assert_visible(&created).await;
        assert_new_effects(
            &before,
            &f.snapshot().await,
            &[
                ("cluster", "create"),
                ("route_config", "create"),
                ("listener", "create"),
            ],
        );
    })
    .await;
}

#[tokio::test]
async fn late_outbox_final_removal_failure_preserves_exact_identity_revisions_and_effects() {
    case(|f| async move {
        let name = unique("late-delete");
        let created = f.expose(&name).await;
        let before = f.snapshot().await;
        f.inject_late_outbox_failure("deleted").await;
        let (status, body) = f
            .request("DELETE", &format!("expose/{name}"), None, None)
            .await;
        assert!(
            status.is_server_error(),
            "injected removal error must fail: {status} {body}"
        );
        f.assert_injection_fired().await;
        assert_eq!(
            f.snapshot().await,
            before,
            "retain exact resource/association rows and pre-existing audit/outbox"
        );
        f.assert_visible(&created).await;
        f.remove_injection().await;
        let removed = f.remove(&name, StatusCode::OK).await;
        for field in [
            "cluster_disposition",
            "route_config_disposition",
            "listener_disposition",
        ] {
            assert_eq!(removed[field], "deleted");
        }
        f.assert_absent(&name).await;
        assert_new_effects(
            &before,
            &f.snapshot().await,
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
async fn concurrent_independent_auto_ports_allocate_unique_complete_graphs_without_orphans() {
    case(|f| async move {
        let before = f.snapshot().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let f = f.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                let name = unique("auto");
                barrier.wait().await;
                let (status, body) = f
                    .request("POST", "expose", Some(f.expose_body(&name, true)), None)
                    .await;
                (name, status, body)
            }));
        }
        let mut ports = std::collections::HashSet::new();
        for task in tasks {
            let (name, status, created) = task.await.expect("concurrent request task");
            assert_eq!(
                status,
                StatusCode::CREATED,
                "serialized auto allocation: {created}"
            );
            assert_eq!(created["mode"], "created");
            assert!(
                ports.insert(
                    created["listener"]["spec"]["port"]
                        .as_u64()
                        .expect("allocated port")
                ),
                "duplicate auto port"
            );
            f.assert_visible(&created).await;
            let association: Value = sqlx::query_scalar(
                "SELECT to_jsonb(e) FROM exposures e WHERE team_id=$1 AND name=$2",
            )
            .bind(f.team.as_uuid())
            .bind(name)
            .fetch_one(&f.pool)
            .await
            .expect("committed association");
            for (field, key) in [
                ("cluster_id", "cluster"),
                ("route_config_id", "route_config"),
                ("listener_id", "listener"),
            ] {
                assert_eq!(association[field], created[key]["id"]);
            }
        }
        let after = f.snapshot().await;
        for table in [
            "clusters",
            "route_configs",
            "listeners",
            "exposures",
            "route_config_cluster_refs",
            "listener_route_config_refs",
        ] {
            assert_eq!(
                count(&after, table),
                8,
                "one complete graph per success, no orphan: {table}"
            );
        }
        let verbs: Vec<_> = (0..8)
            .flat_map(|_| {
                [
                    ("cluster", "create"),
                    ("route_config", "create"),
                    ("listener", "create"),
                ]
            })
            .collect();
        assert_new_effects(&before, &after, &verbs);
    })
    .await;
}

// Discover the effective quota through ordinary authenticated HTTP creation,
// rather than reading implementation defaults or mutating process-global config.
// Bound to 4096 fixtures; a larger/disabled quota is an explicit harness failure,
// never a silent skip. Ports outside the usual auto range avoid masking quota.
async fn fill_to_quota(f: &Fixture, kind: &str) -> Value {
    let dependency = unique("quota-upstream");
    if kind == "route-configs" {
        f.expect(
            "POST",
            "clusters",
            Some(json!({"name":dependency,"spec":cluster_spec()})),
            None,
            StatusCode::CREATED,
        )
        .await;
    }
    let mut last = None;
    for index in 0..4096_u16 {
        let name = unique("quota-seed");
        let spec = match kind {
            "clusters" => cluster_spec(),
            "route-configs" => {
                json!({"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{"name":"manual","match":{"prefix":{"prefix":"/other"}},"action":{"cluster":dependency}}]}]})
            }
            "listeners" => json!({"address":"0.0.0.0","port":20000+index,"protocol":"http"}),
            _ => panic!("unknown fixture kind"),
        };
        let (status, body) = f
            .request("POST", kind, Some(json!({"name":name,"spec":spec})), None)
            .await;
        if body["code"] == "quota_exceeded" {
            assert!(!status.is_success(), "quota is an error");
            return last.expect("fixture needs a positive quota for boundary concurrency");
        }
        assert_eq!(
            status,
            StatusCode::CREATED,
            "quota fixture {kind} {index}: {body}"
        );
        last = Some(body);
    }
    panic!("effective {kind} quota not reached within 4096 ordinary creates; configure finite test quotas");
}

#[tokio::test]
async fn each_independent_gateway_quota_failure_prevents_all_shortcut_mutations() {
    for kind in ["clusters", "route-configs", "listeners"] {
        case(move |f| async move {
            fill_to_quota(&f,kind).await;
            let before = f.snapshot().await;
            let name = unique("quota-denied");
            let (status,body) = f.request("POST","expose",Some(f.expose_body(&name,true)),None).await;
            assert!(!status.is_success(), "quota must reject shortcut: {body}");
            assert_eq!(body["code"],"quota_exceeded", "{kind} quota must be the rejecting boundary");
            assert_eq!(f.snapshot().await,before,"{kind} quota cannot leave earlier resources, refs, associations or success effects");
            f.assert_absent(&name).await;
        }).await;
    }
}

#[tokio::test]
async fn concurrent_shortcuts_at_each_quota_boundary_commit_only_one_complete_graph() {
    for kind in ["clusters", "route-configs", "listeners"] {
        case(move |f| async move {
            let last = fill_to_quota(&f, kind).await;
            f.expect(
                "DELETE",
                &format!("{kind}/{}", last["name"].as_str().unwrap()),
                None,
                Some(last["revision"].as_i64().unwrap()),
                StatusCode::NO_CONTENT,
            )
            .await;
            let before = f.snapshot().await;
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let mut tasks = Vec::new();
            for _ in 0..2 {
                let f = f.clone();
                let barrier = barrier.clone();
                tasks.push(tokio::spawn(async move {
                    let name = unique("boundary");
                    barrier.wait().await;
                    let (status, body) = f
                        .request("POST", "expose", Some(f.expose_body(&name, true)), None)
                        .await;
                    (name, status, body)
                }));
            }
            let mut success = 0;
            for task in tasks {
                let (name, status, body) = task.await.expect("boundary request");
                if status == StatusCode::CREATED {
                    success += 1;
                    f.assert_visible(&body).await;
                } else {
                    assert_eq!(
                        body["code"], "quota_exceeded",
                        "serialized {kind} loser must see committed quota: {status} {body}"
                    );
                    f.assert_absent(&name).await;
                }
            }
            assert_eq!(success, 1, "exactly one remaining quota slot");
            let after = f.snapshot().await;
            for table in [
                "clusters",
                "route_configs",
                "listeners",
                "exposures",
                "route_config_cluster_refs",
                "listener_route_config_refs",
            ] {
                assert_eq!(
                    count(&after, table),
                    count(&before, table) + 1,
                    "no partial loser graph / quota overshoot: {table}"
                );
                for row in before[table].as_array().unwrap() {
                    assert!(
                        after[table].as_array().unwrap().contains(row),
                        "pre-existing {table} rows untouched"
                    );
                }
            }
            assert_new_effects(
                &before,
                &after,
                &[
                    ("cluster", "create"),
                    ("route_config", "create"),
                    ("listener", "create"),
                ],
            );
        })
        .await;
    }
}

#[tokio::test]
async fn live_api_bindings_at_config_vhost_and_route_scope_block_route_only_removal() {
    for scope in ["config", "vhost", "route"] {
        case(move |f| async move {
            let name = unique("bound"); let created = f.expose(&name).await;
            f.add_genuine_route(&name,&created).await;
            let api = unique("live-api");
            let mut binding = json!({"route_config_id":created["route_config"]["id"],"listener_id":created["listener"]["id"]});
            if scope != "config" { binding["virtual_host"] = json!("default"); }
            if scope == "route" { binding["route"] = json!(name); }
            let imported = f.expect("POST","api-definitions",Some(json!({"name":api,"route_binding":binding,"openapi":{"openapi":"3.0.3","info":{"title":"Bound","version":"1.0.0"},"paths":{"/owned":{"get":{"operationId":"owned"}}}}})),None,StatusCode::CREATED).await;
            // A terminal API-based session does not retire its live binding.
            let session = unique("api-session");
            f.expect("POST","learning-sessions",Some(json!({"name":session,"api":api,"target_sample_count":25,"max_bytes":4096,"max_distinct_paths":20})),None,StatusCode::CREATED).await;
            let stopped = f.expect("POST",&format!("learning-sessions/{session}/stop"),None,None,StatusCode::OK).await;
            assert_eq!(stopped["status"],"completed");
            let before = f.snapshot().await;
            assert_eq!(count(&before,"api_route_bindings"),1,"real live binding");
            let body = f.remove(&name,StatusCode::CONFLICT).await; assert_conflict(&body);
            assert_eq!(f.snapshot().await,before,"{scope} binding blocks route-only removal even after capture stops");
            assert!(body.to_string().contains(&api),"same-team dependent API diagnostic");
            // Existing revision-checked API deletion is the supported remediation;
            // no invented standalone binding or capture-session delete endpoint.
            f.expect("DELETE",&format!("api-definitions/{api}"),None,Some(imported["api"]["revision"].as_i64().unwrap()),StatusCode::NO_CONTENT).await;
            f.remove(&name,StatusCode::OK).await;
        }).await;
    }
}

#[tokio::test]
async fn active_direct_captures_at_config_vhost_and_route_scope_block_route_only_removal() {
    for scope in ["config", "vhost", "route"] {
        case(move |f| async move {
            let name = unique("active");
            let created = f.expose(&name).await;
            f.add_genuine_route(&name, &created).await;
            let capture = f.seed_capture(&created, scope, "capturing").await;
            let before = f.snapshot().await;
            let body = f.remove(&name, StatusCode::CONFLICT).await;
            assert_conflict(&body);
            assert!(
                body.to_string().contains(&capture),
                "same-team active dependent diagnostic"
            );
            assert_eq!(
                f.snapshot().await,
                before,
                "{scope} active capture retains exact rows and effects"
            );
            let stopped = f
                .expect(
                    "POST",
                    &format!("learning-sessions/{capture}/stop"),
                    None,
                    None,
                    StatusCode::OK,
                )
                .await;
            assert_eq!(stopped["status"], "completed");
            f.remove(&name, StatusCode::OK).await;
        })
        .await;
    }
}

#[tokio::test]
async fn terminal_direct_capture_history_blocks_final_cleanup_but_allows_genuine_route_only_removal(
) {
    for terminal in ["completed", "cancelled", "failed"] {
        case(move |f| async move {
            let name = unique("history");
            let created = f.expose(&name).await;
            f.seed_capture(&created, "route", terminal).await;
            let before = f.snapshot().await;
            let body = f.remove(&name, StatusCode::CONFLICT).await;
            assert_conflict(&body);
            assert!(
                body["hint"].as_str().is_some_and(|hint| !hint.is_empty()),
                "historical FK conflict needs remediation guidance"
            );
            assert_eq!(
                f.snapshot().await,
                before,
                "{terminal} history retains final scaffold and association"
            );
            f.assert_visible(&created).await;
            let (other, changed) = f.add_genuine_route(&name, &created).await;
            let before_route_only = f.snapshot().await;
            let removed = f.remove(&name, StatusCode::OK).await;
            assert_eq!(removed["cluster_disposition"], "deleted");
            assert_eq!(removed["route_config_disposition"], "retained");
            assert_eq!(removed["listener_disposition"], "retained");
            let retained = f
                .expect(
                    "GET",
                    &format!("route-configs/{name}-routes"),
                    None,
                    None,
                    StatusCode::OK,
                )
                .await;
            let mut spec = changed["spec"].clone();
            spec["virtual_hosts"][0]["routes"]
                .as_array_mut()
                .unwrap()
                .remove(0);
            assert_eq!(
                retained["spec"], spec,
                "genuine unrelated route/policy preserved exactly"
            );
            assert_eq!(retained["id"], created["route_config"]["id"]);
            assert_eq!(
                retained["revision"].as_i64().unwrap(),
                changed["revision"].as_i64().unwrap() + 1
            );
            let listener = f
                .expect(
                    "GET",
                    &format!("listeners/{name}"),
                    None,
                    None,
                    StatusCode::OK,
                )
                .await;
            assert_eq!(
                listener, created["listener"],
                "historical listener FK is retained untouched"
            );
            let after = f.snapshot().await;
            assert_eq!(
                after["capture_sessions"], before_route_only["capture_sessions"],
                "terminal history is never deleted/rewritten"
            );
            assert_eq!(after["exposures"], json!([]));
            assert_eq!(count(&after, "clusters"), 1);
            assert_eq!(after["clusters"][0]["id"], other["id"]);
            f.expect(
                "GET",
                &format!("clusters/{name}-upstream"),
                None,
                None,
                StatusCode::NOT_FOUND,
            )
            .await;
            assert_new_effects(
                &before_route_only,
                &after,
                &[("route_config", "update"), ("cluster", "delete")],
            );
        })
        .await;
    }
}
