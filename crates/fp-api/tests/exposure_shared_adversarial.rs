//! Independent S3 adversarial API/PostgreSQL contracts from approved design AC4–7/12.
//! Only approved contracts/constitution and allowed prior tests were inspected.
//! Every case owns a normally migrated scratch database, including fault triggers.
//! SQLSTATE injection proves translation/rollback, not actual deadlock scheduling.
//! Authorization precedence is observable; DNS/socket activity is not instrumented.
//! FLOWPLANE_TEST_DATABASE_URL is the only DB prerequisite; unset means a visible skip.
//! The connection role must have CREATEDB and ownership rights to drop its database.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use fp_core::dev::DevIssuer;
use fp_domain::{
    authz::{Action, Resource},
    OrgId, OrgRole, TeamId,
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
        let name = format!("aidf_s3_shared_{}", Uuid::now_v7().simple());
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
    org: OrgId,
    team_name: String,
    token: String,
    issuer: Arc<DevIssuer>,
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
            .mint(&subject, "adversarial@test", "S3 contract", 3600)
            .expect("JWT");
        let org = identity::create_org(&pool, &unique("org"), "")
            .await
            .expect("org");
        let team = identity::create_team(&pool, org.id, &unique("team"), "")
            .await
            .expect("team");
        let user =
            identity::upsert_user_by_subject(&pool, &subject, "adversarial@test", "S3 contract")
                .await
                .expect("user");
        identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
            .await
            .expect("admin membership");
        let app = fp_api::build_router(fp_api::AppState {
            pool: pool.clone(),
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            version: "s3-independent-adversarial",
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
            org: org.id,
            team_name: team.name,
            token,
            issuer: Arc::new(issuer),
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
            "events",
            "audit_log",
        ] {
            let filter = if table == "audit_log" {
                " WHERE outcome='success'"
            } else {
                ""
            };
            let rows: Vec<Value> = sqlx::query_scalar(&format!(
                "SELECT to_jsonb(t) FROM {table} t{filter} ORDER BY to_jsonb(t)::text"
            ))
            .fetch_all(&self.pool)
            .await
            .expect("exact owned graph/effects snapshot");
            map.insert(table.into(), json!(rows));
        }
        Value::Object(map)
    }

    async fn actor(&self, grants: &[(Resource, Action)]) -> Self {
        let subject = unique("shared-member");
        let user = identity::upsert_user_by_subject(
            &self.pool,
            &subject,
            "shared@test",
            "Shared contract",
        )
        .await
        .expect("member");
        let org = self.org;
        identity::add_org_membership(&self.pool, user, org, OrgRole::Member)
            .await
            .expect("non-admin membership");
        for &(resource, action) in grants {
            identity::add_grant(&self.pool, user, org, self.team, resource, action, None)
                .await
                .expect("exact team grant");
        }
        let mut caller = self.clone();
        caller.token = self
            .issuer
            .mint(&subject, "shared@test", "Shared contract", 3600)
            .expect("member JWT");
        caller
    }
    fn attach_body(&self, name: &str, listener: &str) -> Value {
        json!({"name":name,"listener":listener,"upstream":"http://127.0.0.1:3001","path":format!("/{name}")})
    }
    async fn attach(&self, name: &str, target: &Value) -> Value {
        let result = self
            .expect(
                "POST",
                "expose",
                Some(self.attach_body(name, target["listener"]["name"].as_str().unwrap())),
                None,
                StatusCode::CREATED,
            )
            .await;
        assert_eq!(result["mode"], "attached");
        assert_eq!(
            result["listener"], target["listener"],
            "listener ID/spec/revision never rewritten"
        );
        assert_eq!(result["route_config"]["id"], target["route_config"]["id"]);
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM exposures e WHERE team_id=$1 AND name=$2")
                .bind(self.team.as_uuid())
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .expect("association");
        for (field, key) in [
            ("cluster_id", "cluster"),
            ("route_config_id", "route_config"),
            ("listener_id", "listener"),
        ] {
            assert_eq!(row[field], result[key]["id"]);
        }
        result
    }
    async fn borrowed(&self) -> Value {
        let upstream = unique("manual-upstream");
        let cluster = self
            .expect(
                "POST",
                "clusters",
                Some(json!({"name":upstream,"spec":cluster_spec()})),
                None,
                StatusCode::CREATED,
            )
            .await;
        let name = unique("manual-config");
        let config = self.expect("POST", "route-configs", Some(json!({"name":name,"spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{"name":"manual","match":{"prefix":{"prefix":"/manual"}},"action":{"cluster":upstream,"timeout_secs":17}}]}]}})), None, StatusCode::CREATED).await;
        let listener = self.expect("POST", "listeners", Some(json!({"name":unique("manual-listener"),"spec":{"address":"0.0.0.0","port":self.socket.local_addr().unwrap().port(),"protocol":"http","route_config":name}})), None, StatusCode::CREATED).await;
        json!({"cluster":cluster,"route_config":config,"listener":listener})
    }
    async fn inject(&self, table: &str, state: &str, deferred: bool, constraint: &str) {
        assert!(["events", "audit_log", "exposures"].contains(&table));
        assert!(["45000", "40P01", "40001", "23503"].contains(&state));
        assert!([
            "",
            "exposures_cluster_fk",
            "exposures_route_config_fk",
            "exposures_listener_fk"
        ]
        .contains(&constraint));
        let declaration = if deferred {
            "CONSTRAINT TRIGGER aidf_fault AFTER INSERT"
        } else {
            "TRIGGER aidf_fault AFTER INSERT"
        };
        let timing = if deferred {
            "DEFERRABLE INITIALLY DEFERRED"
        } else {
            ""
        };
        let success_only = if table == "audit_log" {
            " AND NEW.outcome='success'"
        } else {
            ""
        };
        let sql = format!(
            r#"
            CREATE SEQUENCE aidf_fault_witness;
            CREATE FUNCTION aidf_raise_fault() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
              IF NEW.team_id = '{}'::uuid{success_only} THEN
                PERFORM nextval('aidf_fault_witness');
                RAISE EXCEPTION 'owned scratch injected fault' USING ERRCODE='{state}', CONSTRAINT='{constraint}';
              END IF;
              RETURN NEW;
            END $$;
            CREATE {declaration} ON {table} {timing} FOR EACH ROW EXECUTE FUNCTION aidf_raise_fault();
        "#,
            self.team.as_uuid()
        );
        sqlx::raw_sql(&sql)
            .execute(&self.pool)
            .await
            .expect("owned immediate/deferred fault trigger");
    }
    async fn assert_fault_once(&self) {
        let (called, value): (bool, i64) =
            sqlx::query_as("SELECT is_called,last_value FROM aidf_fault_witness")
                .fetch_one(&self.pool)
                .await
                .expect("nontransactional witness");
        assert!(called, "must reach injected boundary");
        assert_eq!(value, 1, "no hidden automatic retry");
    }
    async fn clear_fault(&self, table: &str) {
        sqlx::raw_sql(&format!("DROP TRIGGER aidf_fault ON {table}; DROP FUNCTION aidf_raise_fault(); DROP SEQUENCE aidf_fault_witness;")).execute(&self.pool).await.expect("remove owned fault");
    }
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
            .bind(Uuid::now_v7()).bind(self.team.as_uuid()).bind(self.org.as_uuid()).bind(&name).bind(status)
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
    let old_audit = before["audit_log"].as_array().unwrap();
    let events: Vec<_> = after["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !old_events.contains(row))
        .collect();
    let audits: Vec<_> = after["audit_log"]
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

async fn fill_quota(f: &Fixture, kind: &str, target: &Value) {
    let mut sockets = Vec::new();
    for index in 0..4096 {
        let spec = match kind {
            "clusters" => cluster_spec(),
            "route-configs" => target["route_config"]["spec"].clone(),
            "listeners" => {
                let socket = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = socket.local_addr().unwrap().port();
                sockets.push(socket);
                json!({"address":"0.0.0.0","port":port,"protocol":"http"})
            }
            _ => panic!("unsupported quota fixture"),
        };
        let (status, body) = f
            .request(
                "POST",
                kind,
                Some(json!({"name":unique("quota"),"spec":spec})),
                None,
            )
            .await;
        if body["code"] == "quota_exceeded" {
            assert!(!status.is_success());
            return;
        }
        assert_eq!(
            status,
            StatusCode::CREATED,
            "quota {kind} fixture {index}: {body}"
        );
    }
    panic!("configure finite {kind} quota within 4096 ordinary creates");
}
async fn quota_case(kind: &'static str) {
    case(move |f| async move {
        let target = f.borrowed().await;
        fill_quota(&f, kind, &target).await;
        let before = f.snapshot().await;
        let name = unique("quota-attach");
        if kind == "clusters" {
            let (status, body) = f
                .request(
                    "POST",
                    "expose",
                    Some(f.attach_body(&name, target["listener"]["name"].as_str().unwrap())),
                    None,
                )
                .await;
            assert!(!status.is_success());
            assert_eq!(body["code"], "quota_exceeded");
            assert_eq!(
                f.snapshot().await,
                before,
                "only cluster quota applies; failure atomic"
            );
        } else {
            f.actor(&attach_grants()).await.attach(&name, &target).await;
            let after = f.snapshot().await;
            assert_eq!(count(&after, "clusters"), count(&before, "clusters") + 1);
            assert_eq!(count(&after, "listeners"), count(&before, "listeners"));
            assert_eq!(
                count(&after, "route_configs"),
                count(&before, "route_configs")
            );
            assert_new_effects(
                &before,
                &after,
                &[("cluster", "create"), ("route_config", "update")],
            );
        }
    })
    .await;
}
#[tokio::test]
async fn cluster_quota_rejects_attach_atomically() {
    quota_case("clusters").await;
}
#[tokio::test]
async fn config_quota_does_not_reject_attach() {
    quota_case("route-configs").await;
}
#[tokio::test]
async fn listener_quota_does_not_reject_attach() {
    quota_case("listeners").await;
}

#[tokio::test]
async fn simultaneous_attachments_and_revision_checked_ordinary_update_never_lose_accepted_writes()
{
    case(|f| async move {
        let target = f.borrowed().await;
        let before = f.snapshot().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(5));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let f = f.clone();
            let target = target.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                let name = unique("concurrent-attach");
                barrier.wait().await;
                let (status, body) = f
                    .request(
                        "POST",
                        "expose",
                        Some(f.attach_body(&name, target["listener"]["name"].as_str().unwrap())),
                        None,
                    )
                    .await;
                (name, status, body)
            }));
        }
        let mut edited = target["route_config"]["spec"].clone();
        edited["virtual_hosts"][0]["routes"][0]["action"]["timeout_secs"] = json!(29);
        // Start from the complete canonical API route, not sparse request JSON:
        // omitted typed defaults are not a lost accepted write. Keep every field
        // and change only the unrelated route's intended identity and matcher.
        let mut ordinary_route =
            target["route_config"]["spec"]["virtual_hosts"][0]["routes"][0].clone();
        ordinary_route["name"] = json!("ordinary-accepted");
        ordinary_route["match"]["prefix"]["prefix"] = json!("/ordinary");
        edited["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .push(ordinary_route);
        barrier.wait().await;
        let suffix = format!(
            "route-configs/{}",
            target["route_config"]["name"].as_str().unwrap()
        );
        let (ordinary_status, ordinary_body) = f
            .request(
                "PATCH",
                &suffix,
                Some(json!({"spec":edited})),
                target["route_config"]["revision"].as_i64(),
            )
            .await;
        assert!(
            [StatusCode::OK, StatusCode::CONFLICT].contains(&ordinary_status),
            "ordinary race: {ordinary_body}"
        );
        let mut accepted = Vec::new();
        for task in tasks {
            let (name, status, body) = task.await.unwrap();
            if status == StatusCode::CREATED {
                assert_eq!(body["mode"], "attached");
                let accepted_routes: Vec<_> = body["route_config"]["spec"]["virtual_hosts"][0]
                    ["routes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|route| route["name"] == name)
                    .collect();
                assert_eq!(
                    accepted_routes.len(),
                    1,
                    "accepted attachment has exactly one complete route"
                );
                accepted.push((name, accepted_routes[0].clone()));
            } else {
                assert_eq!(status, StatusCode::CONFLICT, "{body}");
                assert_conflict(&body);
            }
        }
        assert!(
            !accepted.is_empty(),
            "non-vacuous accepted attachment writes"
        );
        let current = f.expect("GET", &suffix, None, None, StatusCode::OK).await;
        let routes = current["spec"]["virtual_hosts"][0]["routes"]
            .as_array()
            .unwrap();
        for (name, route) in &accepted {
            assert_eq!(
                routes.iter().filter(|r| r["name"] == *name).count(),
                1,
                "accepted route never lost"
            );
            assert!(
                routes.contains(route),
                "complete accepted attachment survives: {name}"
            );
            let association: i64 =
                sqlx::query_scalar("SELECT count(*) FROM exposures WHERE team_id=$1 AND name=$2")
                    .bind(f.team.as_uuid())
                    .bind(name)
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            assert_eq!(association, 1);
        }
        assert_eq!(routes.iter().filter(|r| r["name"] == "manual").count(), 1);
        let expected_final_spec;
        if ordinary_status == StatusCode::OK {
            assert!(
                routes.contains(&edited["virtual_hosts"][0]["routes"][0]),
                "accepted ordinary policy survives attachments"
            );
            assert!(
                routes.contains(&edited["virtual_hosts"][0]["routes"][1]),
                "accepted ordinary unrelated route survives"
            );
            expected_final_spec = current["spec"].clone();
        } else {
            // Ordinary revision-checked PATCH has its own typed 409 contract,
            // distinct from the shortcut's generic transaction conflict.
            assert_eq!(ordinary_body["code"], "revision_mismatch");
            assert!(ordinary_body["hint"]
                .as_str()
                .unwrap()
                .contains("current revision"));
            assert!(ordinary_body["request_id"].as_str().is_some());
            // Explicit caller reread/retry, never a hidden automatic retry.
            let mut spec = current["spec"].clone();
            spec["virtual_hosts"][0]["routes"]
                .as_array_mut()
                .unwrap()
                .push(edited["virtual_hosts"][0]["routes"][1].clone());
            f.expect(
                "PATCH",
                &suffix,
                Some(json!({"spec":spec})),
                current["revision"].as_i64(),
                StatusCode::OK,
            )
            .await;
            expected_final_spec = spec;
        }
        let final_config = f.expect("GET", &suffix, None, None, StatusCode::OK).await;
        assert_eq!(
            final_config["spec"], expected_final_spec,
            "complete accepted policy/routes survive final readback and explicit retry"
        );
        assert_eq!(
            final_config["revision"].as_i64().unwrap(),
            target["route_config"]["revision"].as_i64().unwrap()
                + i64::try_from(accepted.len()).unwrap()
                + 1,
            "exactly one revision per accepted attachment and ordinary update"
        );
        let routes = final_config["spec"]["virtual_hosts"][0]["routes"]
            .as_array()
            .unwrap();
        for (name, route) in &accepted {
            assert_eq!(routes.iter().filter(|r| r["name"] == *name).count(), 1);
            assert!(
                routes.contains(route),
                "complete accepted attachment survives final readback: {name}"
            );
        }
        let after = f.snapshot().await;
        assert_eq!(count(&after, "exposures"), accepted.len());
        let mut verbs = Vec::new();
        for _ in &accepted {
            verbs.extend([("cluster", "create"), ("route_config", "update")]);
        }
        verbs.push(("route_config", "update"));
        assert_new_effects(&before, &after, &verbs);
        assert_eq!(
            count(&after, "clusters"),
            count(&before, "clusters") + count(&after, "exposures"),
            "no rejected-write orphan upstream"
        );
        assert_eq!(after["listeners"], before["listeners"]);
    })
    .await;
}

async fn final_cleanup_grant_case(missing: Resource) {
    case(move |f| async move {
        let original = unique("managed");
        let target = f.expose(&original).await;
        let attached_name = unique("inheritor");
        let attached = f
            .actor(&attach_grants())
            .await
            .attach(&attached_name, &target)
            .await;
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM exposures e WHERE team_id=$1 AND name=$2")
                .bind(f.team.as_uuid())
                .bind(&attached_name)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(row["cleanup_listener"], true);
        assert_eq!(row["cleanup_route_config"], true);
        let original_removed = f.remove(&original, StatusCode::OK).await;
        assert_eq!(original_removed["listener_disposition"], "retained");
        let mut grants = vec![
            (Resource::Listeners, Action::Read),
            (Resource::RouteConfigs, Action::Read),
            (Resource::Clusters, Action::Read),
            (Resource::RouteConfigs, Action::Update),
        ];
        for resource in [
            Resource::Clusters,
            Resource::Listeners,
            Resource::RouteConfigs,
        ] {
            if resource != missing {
                grants.push((resource, Action::Delete));
            }
        }
        let caller = f.actor(&grants).await;
        let before = f.snapshot().await;
        let (status, body) = caller
            .request("DELETE", &format!("expose/{attached_name}"), None, None)
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "inherited provenance is not delete authority: {body}"
        );
        error_envelope(status, &body);
        assert_eq!(f.snapshot().await, before);
        grants.push((missing, Action::Delete));
        let removed = f
            .actor(&grants)
            .await
            .remove(&attached_name, StatusCode::OK)
            .await;
        for key in [
            "cluster_disposition",
            "listener_disposition",
            "route_config_disposition",
        ] {
            assert_eq!(removed[key], "deleted");
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
            assert_eq!(after[table], json!([]), "final inherited cleanup: {table}");
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
        assert_eq!(attached["listener"], target["listener"]);
    })
    .await;
}
#[tokio::test]
async fn inherited_cleanup_requires_final_listener_delete() {
    final_cleanup_grant_case(Resource::Listeners).await;
}
#[tokio::test]
async fn inherited_cleanup_requires_final_config_delete() {
    final_cleanup_grant_case(Resource::RouteConfigs).await;
}
#[tokio::test]
async fn inherited_cleanup_requires_final_cluster_delete() {
    final_cleanup_grant_case(Resource::Clusters).await;
}

fn attach_grants() -> Vec<(Resource, Action)> {
    vec![
        (Resource::Clusters, Action::Create),
        (Resource::Listeners, Action::Read),
        (Resource::RouteConfigs, Action::Read),
        (Resource::RouteConfigs, Action::Update),
    ]
}
fn error_envelope(status: StatusCode, body: &Value) {
    let code = match status {
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::INTERNAL_SERVER_ERROR => "internal",
        _ => {
            assert!(body["code"].is_string());
            body["code"].as_str().unwrap()
        }
    };
    assert_eq!(body["code"], code);
    assert!(body["message"].is_string());
    for key in ["cluster", "listener", "route_config", "spec", "items"] {
        assert!(body.get(key).is_none(), "error leaks product data");
    }
}

#[tokio::test]
async fn exact_attach_grants_need_no_scaffold_create_or_delete_and_preserve_borrowed_policy() {
    case(|f| async move {
        let target = f.borrowed().await;
        let before = f.snapshot().await;
        let caller = f.actor(&attach_grants()).await;
        let name = unique("exact-grants");
        let attached = caller.attach(&name, &target).await;
        assert_eq!(attached["curl_url"], Value::Null, "no guessed endpoint");
        let rows = f.snapshot().await;
        assert_eq!(count(&rows, "exposures"), 1);
        assert_eq!(rows["exposures"][0]["cleanup_listener"], false);
        assert_eq!(rows["exposures"][0]["cleanup_route_config"], false);
        let mut expected = attached["route_config"]["spec"].clone();
        let routes = expected["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap();
        assert_eq!(routes.len(), 2);
        routes.retain(|r| r["name"] != name);
        assert_eq!(
            expected, target["route_config"]["spec"],
            "unrelated policy/order preserved"
        );
        assert_new_effects(
            &before,
            &rows,
            &[("cluster", "create"), ("route_config", "update")],
        );
        let removed = f.remove(&name, StatusCode::OK).await;
        assert_eq!(removed["listener_disposition"], "retained");
        assert_eq!(removed["route_config_disposition"], "retained");
        let retained = f
            .expect(
                "GET",
                &format!(
                    "route-configs/{}",
                    target["route_config"]["name"].as_str().unwrap()
                ),
                None,
                None,
                StatusCode::OK,
            )
            .await;
        assert_eq!(retained["spec"], target["route_config"]["spec"]);
    })
    .await;
}

async fn missing_grant(index: usize) {
    case(move |f| async move {
        let target = f.borrowed().await;
        let mut grants = attach_grants();
        let missing = grants.remove(index);
        let caller = f.actor(&grants).await;
        for upstream in [
            "not-an-upstream-url",
            "http://unresolvable-s3-contract.invalid:8080",
        ] {
            let before = f.snapshot().await;
            let mut body = f.attach_body(
                &unique("denied"),
                target["listener"]["name"].as_str().unwrap(),
            );
            body["upstream"] = json!(upstream);
            let (status, reply) = caller.request("POST", "expose", Some(body), None).await;
            eprintln!(
                "missing={missing:?} upstream={upstream} status={status} code={}",
                reply["code"]
            );
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "auth must precede malformed/unresolvable upstream: {reply}"
            );
            error_envelope(status, &reply);
            assert_eq!(
                f.snapshot().await,
                before,
                "no product/success audit/outbox mutation"
            );
        }
        // No DNS instrumentation: observable error precedence only.
        f.actor(&attach_grants())
            .await
            .attach(&unique("auth-control"), &target)
            .await;
    })
    .await;
}
macro_rules! missing_case {
    ($name:ident,$index:expr) => {
        #[tokio::test]
        async fn $name() {
            missing_grant($index).await;
        }
    };
}
missing_case!(missing_cluster_create_precedes_upstream, 0);
missing_case!(missing_listener_read_precedes_upstream, 1);
missing_case!(missing_config_read_precedes_upstream, 2);
missing_case!(missing_config_update_precedes_upstream, 3);

#[tokio::test]
async fn listener_conflicts_only_with_nonnull_port_or_public_base_and_null_is_omitted() {
    case(|f| async move {
        let target = f.borrowed().await;
        for (key, value) in [
            ("port", json!(f.socket.local_addr().unwrap().port())),
            ("public_base_url", json!("https://gateway.example")),
        ] {
            let before = f.snapshot().await;
            let mut body = f.attach_body(
                &unique("exclusive"),
                target["listener"]["name"].as_str().unwrap(),
            );
            body[key] = value;
            let (status, reply) = f.request("POST", "expose", Some(body), None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {reply}");
            error_envelope(status, &reply);
            assert_eq!(f.snapshot().await, before);
        }
        let mut body = f.attach_body(
            &unique("null-fields"),
            target["listener"]["name"].as_str().unwrap(),
        );
        body["port"] = Value::Null;
        body["public_base_url"] = Value::Null;
        let attached = f
            .expect("POST", "expose", Some(body), None, StatusCode::CREATED)
            .await;
        assert_eq!(attached["mode"], "attached");
        let mut created = f.expose_body(&unique("null-listener"), false);
        created["listener"] = Value::Null;
        // Borrowed scaffold already uses this fixture's port; allocate a fresh owned one.
        let port = TcpListener::bind("127.0.0.1:0").unwrap();
        created["port"] = json!(port.local_addr().unwrap().port());
        let result = f
            .expect("POST", "expose", Some(created), None, StatusCode::CREATED)
            .await;
        assert_eq!(result["mode"], "created");
    })
    .await;
}

async fn hidden_target(table: &'static str) {
    case(move |f| async move {
        let target=f.borrowed().await;
        let key=if table=="listeners" {"listener"} else {"route_config"};
        let id=Uuid::parse_str(target[key]["id"].as_str().unwrap()).unwrap();
        let row: Value=sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1 AND team_id=$2")).bind(id).bind(f.team.as_uuid()).fetch_one(&f.pool).await.expect("established owner schema");
        assert_eq!(row["owner_kind"],"user");
        assert!(row.get("owner_id").is_some(),"fixture schema must support owner id");
        sqlx::query(&format!("UPDATE {table} SET owner_kind='discovery',owner_id=id WHERE id=$1 AND team_id=$2 AND owner_kind='user'")).bind(id).bind(f.team.as_uuid()).execute(&f.pool).await.expect("supported metadata only; no constraint weakening");
        let route=if table=="listeners" {"listeners"} else {"route-configs"};
        f.expect("GET",&format!("{route}/{}",target[key]["name"].as_str().unwrap()),None,None,StatusCode::NOT_FOUND).await;
        let before=f.snapshot().await;
        let (status,body)=f.request("POST","expose",Some(f.attach_body(&unique("hidden-attach"),target["listener"]["name"].as_str().unwrap())),None).await;
        assert_eq!(status,StatusCode::NOT_FOUND,"hidden {table}: {body}");
        error_envelope(status,&body);
        assert_eq!(f.snapshot().await,before);
    }).await;
}
#[tokio::test]
async fn hidden_discovery_listener_is_not_attachable() {
    hidden_target("listeners").await;
}
#[tokio::test]
async fn hidden_discovery_config_is_not_attachable() {
    hidden_target("route_configs").await;
}

#[tokio::test]
async fn foreign_listener_name_is_404_and_foreign_team_without_grants_is_403() {
    case(|f| async move {
        let own = f.borrowed().await;
        let team = identity::create_team(&f.pool, f.org, &unique("foreign-team"), "")
            .await
            .unwrap();
        let mut foreign = f.clone();
        foreign.team = team.id;
        foreign.team_name = team.name;
        foreign.socket = Arc::new(TcpListener::bind("127.0.0.1:0").unwrap());
        let target = foreign.borrowed().await;
        let caller = f.actor(&attach_grants()).await;
        let before = f.snapshot().await;
        let (status, body) = caller
            .request(
                "POST",
                "expose",
                Some(f.attach_body(
                    &unique("foreign-listener"),
                    target["listener"]["name"].as_str().unwrap(),
                )),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        error_envelope(status, &body);
        assert_eq!(f.snapshot().await, before);
        let mut foreign_caller = caller.clone();
        foreign_caller.team = foreign.team;
        foreign_caller.team_name = foreign.team_name.clone();
        let (status, body) = foreign_caller
            .request(
                "POST",
                "expose",
                Some(foreign.attach_body(
                    &unique("foreign-denied"),
                    target["listener"]["name"].as_str().unwrap(),
                )),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        error_envelope(status, &body);
        assert_eq!(f.snapshot().await, before);
        caller.attach(&unique("own-control"), &own).await;
    })
    .await;
}

#[tokio::test]
async fn same_team_listener_never_resolves_its_config_name_in_another_team() {
    case(|f| async move {
        let own=f.borrowed().await;
        let team=identity::create_team(&f.pool,f.org,&unique("foreign-config-team"),"").await.unwrap();
        let mut foreign=f.clone();foreign.team=team.id;foreign.team_name=team.name;
        foreign.socket=Arc::new(TcpListener::bind("127.0.0.1:0").unwrap());
        let other=foreign.borrowed().await;
        // Model an externally stale authored JSON reference without weakening FKs
        // or moving resource ownership. Existing normalized refs remain same-team.
        // This is deliberately not an ordinary supported update success claim.
        let listener_id=Uuid::parse_str(own["listener"]["id"].as_str().unwrap()).unwrap();
        sqlx::query("UPDATE listeners SET spec=jsonb_set(spec,'{route_config}',to_jsonb($1::text)) WHERE id=$2 AND team_id=$3")
            .bind(other["route_config"]["name"].as_str().unwrap()).bind(listener_id).bind(f.team.as_uuid()).execute(&f.pool).await.expect("owned stale JSON fixture");
        let before=f.snapshot().await;
        let (status,body)=f.request("POST","expose",Some(f.attach_body(&unique("foreign-config"),own["listener"]["name"].as_str().unwrap())),None).await;
        assert_eq!(status,StatusCode::NOT_FOUND,"no cross-team config-name oracle: {body}");
        error_envelope(status,&body);
        assert_eq!(f.snapshot().await,before);
    }).await;
}

async fn broad_dependency(kind: &'static str, scope: &'static str) {
    case(move |f| async move {
        let target=f.borrowed().await;
        let dependent=if kind=="capture" { f.seed_capture(&target,scope,"capturing").await } else {
            let name=unique("broad-api");
            let mut binding=json!({"route_config_id":target["route_config"]["id"],"listener_id":target["listener"]["id"]});
            if scope=="vhost" {binding["virtual_host"]=json!("default");}
            f.expect("POST","api-definitions",Some(json!({"name":name,"route_binding":binding,"openapi":{"openapi":"3.0.3","info":{"title":"Broad","version":"1.0.0"},"paths":{"/manual":{"get":{"operationId":"manual"}}}}})),None,StatusCode::CREATED).await;
            name
        };
        let before=f.snapshot().await;
        let (status,body)=f.request("POST","expose",Some(f.attach_body(&unique("protected"),target["listener"]["name"].as_str().unwrap())),None).await;
        assert_eq!(status,StatusCode::CONFLICT,"{kind}/{scope}: {body}");assert_conflict(&body);
        assert!(body.to_string().contains(&dependent),"same-team dependency guidance");
        assert_eq!(f.snapshot().await,before,"broad scope covers even the new route");
        if kind=="capture" {
            f.expect("POST",&format!("learning-sessions/{dependent}/stop"),None,None,StatusCode::OK).await;
            f.attach(&unique("stopped-control"),&target).await;
        }
    }).await;
}
macro_rules! broad_case {
    ($name:ident,$kind:literal,$scope:literal) => {
        #[tokio::test]
        async fn $name() {
            broad_dependency($kind, $scope).await;
        }
    };
}
broad_case!(config_binding_prevents_attach, "binding", "config");
broad_case!(vhost_binding_prevents_attach, "binding", "vhost");
broad_case!(config_active_capture_prevents_attach, "capture", "config");
broad_case!(vhost_active_capture_prevents_attach, "capture", "vhost");

async fn fault_case(
    table: &'static str,
    state: &'static str,
    deferred: bool,
    removal: bool,
    constraint: &'static str,
) {
    case(move |f| async move {
        let target=f.borrowed().await;
        let name=unique("fault");
        if removal {f.attach(&name,&target).await;}
        let before=f.snapshot().await;
        f.inject(table,state,deferred,constraint).await;
        let (status,body)=if removal {f.request("DELETE",&format!("expose/{name}"),None,None).await} else {f.request("POST","expose",Some(f.attach_body(&name,target["listener"]["name"].as_str().unwrap())),None).await};
        eprintln!("fault table={table} state={state} deferred={deferred} removal={removal} status={status} code={}",body["code"]);
        f.assert_fault_once().await;
        assert_eq!(f.snapshot().await,before,"all rows/refs/IDs/revisions/associations/events/success audit roll back");
        let expected=if state=="45000" {StatusCode::INTERNAL_SERVER_ERROR} else {StatusCode::CONFLICT};
        assert_eq!(status,expected,"{body}");error_envelope(status,&body);
        f.clear_fault(table).await;
        if removal {
            let result=f.remove(&name,StatusCode::OK).await;
            assert_eq!(result["listener_disposition"],"retained");
            assert_new_effects(&before,&f.snapshot().await,&[("cluster","delete"),("route_config","update")]);
        } else {
            f.attach(&name,&target).await;
            assert_new_effects(&before,&f.snapshot().await,&[("cluster","create"),("route_config","update")]);
        }
    }).await;
}
macro_rules! fault_test {
    ($name:ident,$table:literal,$state:literal,$late:expr,$remove:expr,$constraint:literal) => {
        #[tokio::test]
        async fn $name() {
            fault_case($table, $state, $late, $remove, $constraint).await;
        }
    };
}
fault_test!(
    attach_events_immediate_persistence,
    "events",
    "45000",
    false,
    false,
    ""
);
fault_test!(
    remove_events_immediate_persistence,
    "events",
    "45000",
    false,
    true,
    ""
);
fault_test!(
    attach_events_commit_persistence,
    "events",
    "45000",
    true,
    false,
    ""
);
fault_test!(
    remove_events_commit_persistence,
    "events",
    "45000",
    true,
    true,
    ""
);
fault_test!(
    attach_events_immediate_deadlock_sqlstate,
    "events",
    "40P01",
    false,
    false,
    ""
);
fault_test!(
    remove_events_immediate_deadlock_sqlstate,
    "events",
    "40P01",
    false,
    true,
    ""
);
fault_test!(
    attach_events_commit_deadlock_sqlstate,
    "events",
    "40P01",
    true,
    false,
    ""
);
fault_test!(
    remove_events_commit_deadlock_sqlstate,
    "events",
    "40P01",
    true,
    true,
    ""
);
fault_test!(
    attach_events_immediate_serialization_sqlstate,
    "events",
    "40001",
    false,
    false,
    ""
);
fault_test!(
    remove_events_immediate_serialization_sqlstate,
    "events",
    "40001",
    false,
    true,
    ""
);
fault_test!(
    attach_events_commit_serialization_sqlstate,
    "events",
    "40001",
    true,
    false,
    ""
);
fault_test!(
    remove_events_commit_serialization_sqlstate,
    "events",
    "40001",
    true,
    true,
    ""
);
fault_test!(
    attach_audit_log_immediate_persistence,
    "audit_log",
    "45000",
    false,
    false,
    ""
);
fault_test!(
    remove_audit_log_immediate_persistence,
    "audit_log",
    "45000",
    false,
    true,
    ""
);
fault_test!(
    attach_audit_log_commit_persistence,
    "audit_log",
    "45000",
    true,
    false,
    ""
);
fault_test!(
    remove_audit_log_commit_persistence,
    "audit_log",
    "45000",
    true,
    true,
    ""
);
fault_test!(
    attach_audit_log_immediate_deadlock_sqlstate,
    "audit_log",
    "40P01",
    false,
    false,
    ""
);
fault_test!(
    remove_audit_log_immediate_deadlock_sqlstate,
    "audit_log",
    "40P01",
    false,
    true,
    ""
);
fault_test!(
    attach_audit_log_commit_deadlock_sqlstate,
    "audit_log",
    "40P01",
    true,
    false,
    ""
);
fault_test!(
    remove_audit_log_commit_deadlock_sqlstate,
    "audit_log",
    "40P01",
    true,
    true,
    ""
);
fault_test!(
    attach_audit_log_immediate_serialization_sqlstate,
    "audit_log",
    "40001",
    false,
    false,
    ""
);
fault_test!(
    remove_audit_log_immediate_serialization_sqlstate,
    "audit_log",
    "40001",
    false,
    true,
    ""
);
fault_test!(
    attach_audit_log_commit_serialization_sqlstate,
    "audit_log",
    "40001",
    true,
    false,
    ""
);
fault_test!(
    remove_audit_log_commit_serialization_sqlstate,
    "audit_log",
    "40001",
    true,
    true,
    ""
);
fault_test!(
    association_insert_cluster_fk_maps_conflict,
    "exposures",
    "23503",
    false,
    false,
    "exposures_cluster_fk"
);
fault_test!(
    association_insert_route_config_fk_maps_conflict,
    "exposures",
    "23503",
    false,
    false,
    "exposures_route_config_fk"
);
fault_test!(
    association_insert_listener_fk_maps_conflict,
    "exposures",
    "23503",
    false,
    false,
    "exposures_listener_fk"
);
