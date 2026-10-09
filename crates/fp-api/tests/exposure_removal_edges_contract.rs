//! Independent S2 removal-edge contracts from approved design AC4/6/12/14.
//! Only public REST/generated schemas and the two approved black-box fixtures
//! informed this target. No implementation reads. Runtime requires canonical
//! FLOWPLANE_TEST_DATABASE_URL and CREATEDB; each case owns its database/port.
//! Unset configuration explicitly skips; any configured setup failure panics.
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
        let name = format!("aidf_removal_edges_{}", Uuid::now_v7().simple());
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
            version: "s2-removal-edges",
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

fn assert_error(body: &Value, code: &str) {
    assert_eq!(body["code"], code, "stable error envelope: {body}");
    for key in ["message", "request_id"] {
        assert!(
            body[key].as_str().is_some_and(|s| !s.is_empty()),
            "{key}: {body}"
        );
    }
    if let Some(hint) = body.get("hint") {
        assert!(hint.is_null() || hint.is_string());
    }
    assert!(
        body.get("data").is_none(),
        "errors never contain success data"
    );
}
fn revision(view: &Value) -> i64 {
    view["revision"].as_i64().expect("revision")
}
fn ordinary_route(name: &str, cluster: &str, prefix: &str) -> Value {
    json!({"name":name,"match":{"prefix":{"prefix":prefix}},"action":{"cluster":cluster,"timeout_secs":17}})
}
impl Fixture {
    async fn get(&self, path: &str) -> Value {
        self.expect("GET", path, None, None, StatusCode::OK).await
    }
    async fn patch(&self, path: &str, spec: Value, current: &Value) -> Value {
        self.expect(
            "PATCH",
            path,
            Some(json!({"spec":spec})),
            Some(revision(current)),
            StatusCode::OK,
        )
        .await
    }
    async fn other_cluster(&self) -> Value {
        self.expect(
            "POST",
            "clusters",
            Some(json!({"name":unique("ordinary"),"spec":cluster_spec()})),
            None,
            StatusCode::CREATED,
        )
        .await
    }
    async fn multi_vhost(&self, name: &str, created: &Value) -> (Value, Value) {
        let cluster = self.other_cluster().await;
        let upstream = cluster["name"].as_str().unwrap();
        let sibling = json!({"name":unique("sibling"),"domains":["sibling.example"],"routes":[ordinary_route("first",upstream,"/first"),ordinary_route("second",upstream,"/second")],"rate_limits":[{"actions":[{"type":"generic_key","descriptor_value":"sibling-policy"}]}]});
        let preceding = json!({"name":unique("preceding"),"domains":["preceding.example"],"routes":[ordinary_route("before",upstream,"/before")]});
        let mut spec = created["route_config"]["spec"].clone();
        // Put the owned target in the middle: removal must preserve vhost and route order.
        let target = spec["virtual_hosts"][0].clone();
        spec["virtual_hosts"] = json!([preceding, target, sibling]);
        let updated = self
            .patch(
                &format!("route-configs/{name}-routes"),
                spec,
                &created["route_config"],
            )
            .await;
        (cluster, updated)
    }
    async fn conflict_unchanged(&self, name: &str) {
        let before = self.snapshot().await;
        let error = self.remove(name, StatusCode::CONFLICT).await;
        assert_error(&error, "conflict");
        assert_eq!(
            self.snapshot().await,
            before,
            "no row/revision/ref/success-audit/outbox mutation"
        );
    }
    async fn retained_removal(
        &self,
        name: &str,
        created: &Value,
        current: &Value,
        expected: Value,
    ) {
        let before = self.snapshot().await;
        let removed = self.remove(name, StatusCode::OK).await;
        assert_eq!(removed["cluster_disposition"], "deleted");
        assert_eq!(removed["route_config_disposition"], "retained");
        assert_eq!(removed["listener_disposition"], "retained");
        let config = self.get(&format!("route-configs/{name}-routes")).await;
        assert_eq!(
            config["spec"], expected,
            "exact surviving policies and array order"
        );
        assert_eq!(config["id"], created["route_config"]["id"]);
        assert_eq!(revision(&config), revision(current) + 1);
        assert_eq!(
            self.get(&format!("listeners/{name}")).await,
            created["listener"]
        );
        let after = self.snapshot().await;
        assert_eq!(after["exposures"], json!([]));
        assert_new_effects(
            &before,
            &after,
            &[("route_config", "update"), ("cluster", "delete")],
        );
    }
}

#[tokio::test]
async fn empty_target_vhost_removed_preserving_sibling_specs_and_order() {
    case(|f| async move {
        let name = unique("empty-target");
        let created = f.expose(&name).await;
        let (other, current) = f.multi_vhost(&name, &created).await;
        let mut expected = current["spec"].clone();
        expected["virtual_hosts"].as_array_mut().unwrap().remove(1);
        f.retained_removal(&name, &created, &current, expected)
            .await;
        assert_eq!(
            f.get(&format!("clusters/{}", other["name"].as_str().unwrap()))
                .await,
            other
        );
    })
    .await;
}

#[tokio::test]
async fn empty_target_vhost_policy_binding_and_active_capture_prevent_removal() {
    for protection in ["policy", "binding", "capture"] {
        case(move |f| async move {
            let name=unique("protected-vhost"); let created=f.expose(&name).await;
            let (_,mut current)=f.multi_vhost(&name,&created).await;
            let path=format!("route-configs/{name}-routes");
            let clean=current["spec"].clone();
            let mut api=None; let mut capture=None;
            match protection {
                "policy" => {
                    let mut spec=clean.clone();
                    spec["virtual_hosts"][1]["rate_limits"]=json!([{"actions":[{"type":"generic_key","descriptor_value":"owned-vhost-policy"}]}]);
                    current=f.patch(&path,spec,&current).await;
                }
                "binding" => {
                    let api_name=unique("vhost-api");
                    let imported=f.expect("POST","api-definitions",Some(json!({"name":api_name,"route_binding":{"route_config_id":created["route_config"]["id"],"listener_id":created["listener"]["id"],"virtual_host":"default"},"openapi":{"openapi":"3.0.3","info":{"title":"Bound","version":"1.0.0"},"paths":{"/owned":{"get":{"operationId":"owned"}}}}})),None,StatusCode::CREATED).await;
                    api=Some((api_name,imported));
                }
                "capture" => { capture=Some(f.seed_capture(&created,"vhost","capturing").await); }
                _=>unreachable!(),
            }
            f.conflict_unchanged(&name).await;
            if protection=="policy" { current=f.patch(&path,clean,&current).await; }
            if let Some((api_name,imported))=api {
                f.expect("DELETE",&format!("api-definitions/{api_name}"),None,Some(revision(&imported["api"])),StatusCode::NO_CONTENT).await;
            }
            if let Some(capture)=capture {
                let stopped=f.expect("POST",&format!("learning-sessions/{capture}/stop"),None,None,StatusCode::OK).await;
                assert_eq!(stopped["status"],"completed");
            }
            let mut expected=current["spec"].clone(); expected["virtual_hosts"].as_array_mut().unwrap().remove(1);
            f.retained_removal(&name,&created,&current,expected).await;
        }).await;
    }
}

#[tokio::test]
async fn generated_names_over_one_hundred_fail_before_any_product_write() {
    case(|f| async move {
        // 92+9 exceeds cluster bound only; 94+7 also exceeds config bound.
        for length in [92, 94, 100] {
            let id = unique("n");
            let name = format!("{id}{}", "x".repeat(length - id.len()));
            assert_eq!(name.len(), length);
            let before = f.snapshot().await;
            let body = f
                .expect(
                    "POST",
                    "expose",
                    Some(f.expose_body(&name, false)),
                    None,
                    StatusCode::BAD_REQUEST,
                )
                .await;
            assert_error(&body, "validation_failed");
            assert_eq!(
                f.snapshot().await,
                before,
                "generated length {length}: prevalidation has no writes"
            );
        }
        let id = unique("boundary");
        let name = format!("{id}{}", "x".repeat(91 - id.len()));
        let before = f.snapshot().await;
        let created = f.expose(&name).await;
        assert_eq!(created["cluster"]["name"].as_str().unwrap().len(), 100);
        assert_new_effects(
            &before,
            &f.snapshot().await,
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

#[tokio::test]
async fn concurrent_ordinary_update_and_removal_never_accept_lost_writes() {
    case(|f| async move {
        let name = unique("concurrent");
        let created = f.expose(&name).await;
        let (_, current) = f.multi_vhost(&name, &created).await;
        let path = format!("route-configs/{name}-routes");
        let mut update = current["spec"].clone();
        let extra = ordinary_route(
            "concurrent-added",
            update["virtual_hosts"][2]["routes"][0]["action"]["cluster"]
                .as_str()
                .unwrap(),
            "/concurrent",
        );
        update["virtual_hosts"][2]["routes"]
            .as_array_mut()
            .unwrap()
            .push(extra.clone());
        let before = f.snapshot().await;
        // A shared start barrier plus authoritative revision assertions permits
        // either winner, but NEVER accepts a successful stale overwrite.
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let left = f.clone();
        let left_barrier = barrier.clone();
        let left_path = path.clone();
        let old_revision = revision(&current);
        let patch = tokio::spawn(async move {
            left_barrier.wait().await;
            left.request(
                "PATCH",
                &left_path,
                Some(json!({"spec":update})),
                Some(old_revision),
            )
            .await
        });
        let right = f.clone();
        let right_name = name.clone();
        let remove = tokio::spawn(async move {
            barrier.wait().await;
            right
                .request("DELETE", &format!("expose/{right_name}"), None, None)
                .await
        });
        let (ps, pb) = patch.await.expect("ordinary request task");
        let (rs, rb) = remove.await.expect("removal task");
        for (status, body) in [(ps, &pb), (rs, &rb)] {
            assert!(
                status == StatusCode::OK || status == StatusCode::CONFLICT,
                "explicit success or concurrency conflict: {status} {body}"
            );
            if status == StatusCode::CONFLICT {
                let code = body["code"].as_str().unwrap();
                assert!(code == "conflict" || code == "revision_mismatch");
                assert_error(body, code);
            }
        }
        assert!(
            ps == StatusCode::OK || rs == StatusCode::OK,
            "at least one mutation must make progress"
        );
        let mut expected = current["spec"].clone();
        if ps == StatusCode::OK {
            assert_eq!(revision(&pb), old_revision + 1);
            expected["virtual_hosts"][2]["routes"]
                .as_array_mut()
                .unwrap()
                .push(extra);
        }
        if rs == StatusCode::OK {
            expected["virtual_hosts"].as_array_mut().unwrap().remove(1);
        }
        let final_view = f.get(&path).await;
        assert_eq!(
            final_view["spec"], expected,
            "every accepted change survives; rejected changes do not commit"
        );
        assert_eq!(
            revision(&final_view),
            old_revision + i64::from(ps == StatusCode::OK) + i64::from(rs == StatusCode::OK)
        );
        let after = f.snapshot().await;
        let mut verbs = Vec::new();
        if ps == StatusCode::OK {
            verbs.push(("route_config", "update"));
        }
        if rs == StatusCode::OK {
            verbs.extend([("route_config", "update"), ("cluster", "delete")]);
        }
        assert_new_effects(&before, &after, &verbs);
        if rs == StatusCode::CONFLICT {
            f.remove(&name, StatusCode::OK).await;
        }
        assert_eq!(f.snapshot().await["exposures"], json!([]));
        assert_eq!(
            f.get(&format!("listeners/{name}")).await,
            created["listener"]
        );
    })
    .await;
}

#[tokio::test]
async fn stale_rerouted_weighted_redirect_and_detached_listener_restore_then_remove() {
    for stale in [
        "rerouted",
        "weighted",
        "redirect",
        "detached",
        "different-config",
    ] {
        case(move |f| async move {
            let name=unique("stale"); let created=f.expose(&name).await;
            let other=f.other_cluster().await; let upstream=other["name"].as_str().unwrap();
            let (path,original,changed)=if stale=="detached" || stale=="different-config" {
                let path=format!("listeners/{name}"); let original=created["listener"]["spec"].clone(); let mut spec=original.clone();
                if stale=="detached" { spec.as_object_mut().unwrap().remove("route_config"); }
                else {
                    let config_name=unique("alternate");
                    f.expect("POST","route-configs",Some(json!({"name":config_name,"spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[ordinary_route("alternate",upstream,"/alternate")]}]}})),None,StatusCode::CREATED).await;
                    spec["route_config"]=json!(config_name);
                }
                let changed=f.patch(&path,spec,&created["listener"]).await; (path,original,changed)
            } else {
                let path=format!("route-configs/{name}-routes"); let original=created["route_config"]["spec"].clone(); let mut spec=original.clone();
                spec["virtual_hosts"][0]["routes"][0]["action"]=match stale {
                    "rerouted"=>json!({"cluster":upstream}),
                    "weighted"=>json!({"weighted_clusters":[{"cluster":format!("{name}-upstream"),"weight":50},{"cluster":upstream,"weight":50}]}),
                    "redirect"=>json!({"redirect":{"https_redirect":true,"response_code":"MOVED_PERMANENTLY"}}),
                    _=>unreachable!(),
                };
                let changed=f.patch(&path,spec,&created["route_config"]).await; (path,original,changed)
            };
            let before=f.snapshot().await;
            let error=f.remove(&name,StatusCode::CONFLICT).await; assert_error(&error,"conflict");
            assert!(error["hint"].as_str().is_some_and(|s| !s.is_empty()),"restore guidance: {error}");
            assert!(error.to_string().contains(&name),"identify stale exposure");
            assert_eq!(f.snapshot().await,before,"{stale} preserves every resource and effect");
            f.patch(&path,original,&changed).await;
            let before=f.snapshot().await; let removed=f.remove(&name,StatusCode::OK).await;
            for key in ["cluster_disposition","route_config_disposition","listener_disposition"] { assert_eq!(removed[key],"deleted"); }
            let after=f.snapshot().await; assert_eq!(after["exposures"],json!([]));
            assert_new_effects(&before,&after,&[("cluster","delete"),("route_config","delete"),("listener","delete")]);
            assert_eq!(f.get(&format!("clusters/{upstream}")).await,other);
        }).await;
    }
}
