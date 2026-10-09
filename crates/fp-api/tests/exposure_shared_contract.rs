//! Independent S3 router/PostgreSQL contracts from approved design lines 44–106.
//! Only approved specs and existing test fixtures informed this target; no
//! producer implementation reads. Each test owns a migrated scratch database
//! (CREATEDB required) and dynamic sockets. Unset canonical DB configuration
//! visibly skips; configured failures fail. Public upstream literals are metadata
//! fixtures, not evidence of upstream traffic. Runtime qualification is external.
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
        let name = format!("aidf_shared_{}", Uuid::now_v7().simple());
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
            version: "s3-shared-contract",
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
        let mut body = json!({"name":name,"upstream":"http://8.8.8.8:8080","path":"/owned","public_base_url":"https://gateway.example"});
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
    async fn attach(&self, name: &str, listener: &str, path: &str) -> Value {
        self.expect(
            "POST",
            "expose",
            Some(attach_body(name, listener, path)),
            None,
            StatusCode::CREATED,
        )
        .await
    }
    async fn association(&self, name: &str) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(e) FROM exposures e WHERE team_id=$1 AND name=$2")
            .bind(self.team.as_uuid())
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .expect("association")
    }
    async fn rejected_attach(&self, label: &str, body: Value, status: StatusCode) {
        let before = self.snapshot().await;
        let (actual, error) = self.request("POST", "expose", Some(body), None).await;
        assert_eq!(actual, status, "shared case {label}: {error}");
        assert_error(
            &error,
            if status == StatusCode::CONFLICT {
                "conflict"
            } else {
                "validation_failed"
            },
        );
        assert_eq!(
            self.snapshot().await,
            before,
            "{label}: exact rows/revisions/refs/audit/outbox unchanged"
        );
    }
}
fn attach_body(name: &str, listener: &str, path: &str) -> Value {
    json!({"name":name,"listener":listener,"upstream":"http://8.8.8.8:8080","path":path})
}
fn cluster_spec() -> Value {
    json!({"endpoints":[{"host":"8.8.8.8","port":8080}]})
}
fn config_spec(routes: Vec<Value>) -> Value {
    json!({"virtual_hosts":[{"name":"default","domains":["*"],"routes":routes,
        "rate_limits":[{"stage":1,"actions":[{"type":"generic_key","descriptor_value":"preserved","descriptor_key":"scope"}]}],"filter_overrides":[]}]})
}
fn listener_spec(port: u16, config: &str) -> Value {
    json!({"address":"0.0.0.0","port":port,"protocol":"http","route_config":config,
        "public_base_url":"https://gateway.example","access_logs":[{"path":"/var/log/flowplane/shared.log","text_format":"%REQ(:METHOD)% %RESPONSE_CODE%\\n"}]})
}
fn exact_route(name: &str, cluster: &str, path: &str) -> Value {
    json!({"name":name,"match":{"exact":{"path":path}},"action":{"cluster":cluster,"timeout_secs":17}})
}
fn name(view: &Value) -> &str {
    view["name"].as_str().expect("name")
}
fn routes(spec: &Value) -> &Vec<Value> {
    spec["virtual_hosts"][0]["routes"]
        .as_array()
        .expect("routes")
}
fn disposition(body: &Value, infrastructure: &str) {
    assert_eq!(body["cluster_disposition"], "deleted");
    assert_eq!(body["route_config_disposition"], infrastructure);
    assert_eq!(body["listener_disposition"], infrastructure);
}
fn unchanged_rows(before: &Value, after: &Value, tables: &[&str]) {
    for table in tables {
        assert_eq!(before[*table], after[*table], "unchanged {table}");
    }
}
struct Manual {
    cluster: Value,
    config: Value,
    listener: Value,
}
impl Fixture {
    async fn manual(&self, spec: Value) -> Manual {
        let cluster = self.create("clusters", "ordinary", cluster_spec()).await;
        let config = self.create("route-configs", "manual-routes", spec).await;
        let listener = self
            .create(
                "listeners",
                "manual",
                listener_spec(self.socket.local_addr().unwrap().port(), "manual-routes"),
            )
            .await;
        Manual {
            cluster,
            config,
            listener,
        }
    }
    async fn retained(&self, exposure: &str, listener: &Value, current: &Value) -> Value {
        let mut expected = current["spec"].clone();
        let rs = expected["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap();
        let index = rs
            .iter()
            .position(|r| r["name"] == exposure)
            .expect("owned route slot");
        rs.remove(index);
        let before = self.snapshot().await;
        let result = self.remove(exposure, StatusCode::OK).await;
        disposition(&result, "retained");
        assert_eq!(result["listener_name"], listener["name"]);
        assert_eq!(result["route_config_name"], current["name"]);
        let config = self.get(&format!("route-configs/{}", name(current))).await;
        assert_eq!(
            config["spec"], expected,
            "exact survivor policy and ordering"
        );
        assert_eq!(config["id"], current["id"]);
        assert_eq!(revision(&config), revision(current) + 1);
        assert_eq!(
            self.get(&format!("listeners/{}", name(listener))).await,
            *listener
        );
        let after = self.snapshot().await;
        unchanged_rows(
            &before,
            &after,
            &["listeners", "listener_route_config_refs"],
        );
        assert_new_effects(
            &before,
            &after,
            &[("route_config", "update"), ("cluster", "delete")],
        );
        assert!(!after["exposures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == exposure));
        assert!(!after["clusters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == format!("{exposure}-upstream")));
        config
    }
}

#[tokio::test]
async fn prefix_insertion_exact_exceptions_preserve_policy_endpoint_and_fanout() {
    case(|f| async move {
        let manual = f
            .manual(config_spec(vec![
                exact_route("exception", "ordinary", "/api/health"),
                ordinary_route("narrow", "ordinary", "/api/private"),
                ordinary_route("wide", "ordinary", "/api"),
                ordinary_route("fallback", "ordinary", "/"),
            ]))
            .await;
        // Both listeners reference the same authoritative config. No assertion
        // here claims actual xDS delivery or upstream/backend traffic.
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let second = f
            .create(
                "listeners",
                "second",
                listener_spec(socket.local_addr().unwrap().port(), "manual-routes"),
            )
            .await;
        let mut current = manual.config.clone();
        for (exposure, path, index) in [
            ("attached-deep", "/api/v2", 2),
            ("attached-narrow", "/api/private/admin", 1),
            ("attached-adjacent", "/apix", 4),
        ] {
            let before = f.snapshot().await;
            let created = f.attach(exposure, "manual", path).await;
            assert_eq!(created["mode"], "attached");
            assert_eq!(
                created["curl_url"],
                format!("https://gateway.example{path}")
            );
            assert_eq!(created["endpoint_source"], "listener.public_base_url");
            assert_eq!(created["listener"], manual.listener);
            assert_eq!(created["route_config"]["id"], current["id"]);
            assert_eq!(revision(&created["route_config"]), revision(&current) + 1);
            assert_eq!(revision(&created["cluster"]), 1);
            let added = routes(&created["route_config"]["spec"])[index].clone();
            assert_eq!(added["name"], exposure);
            assert_eq!(added["match"], json!({"prefix":{"prefix":path}}));
            assert_eq!(added["action"]["cluster"], format!("{exposure}-upstream"));
            let mut expected = current["spec"].clone();
            expected["virtual_hosts"][0]["routes"]
                .as_array_mut()
                .unwrap()
                .insert(index, added);
            assert_eq!(
                created["route_config"]["spec"], expected,
                "only one slot added; no longest-prefix resort"
            );
            let association = f.association(exposure).await;
            assert_eq!(association["cleanup_listener"], false);
            assert_eq!(association["cleanup_route_config"], false);
            assert_eq!(association["listener_id"], manual.listener["id"]);
            assert_eq!(association["route_config_id"], current["id"]);
            assert_eq!(association["cluster_id"], created["cluster"]["id"]);
            assert_eq!(association["virtual_host"], "default");
            assert_eq!(association["route_name"], exposure);
            assert_eq!(association["version"], 1);
            let after = f.snapshot().await;
            unchanged_rows(
                &before,
                &after,
                &["listeners", "listener_route_config_refs"],
            );
            assert_new_effects(
                &before,
                &after,
                &[("cluster", "create"), ("route_config", "update")],
            );
            assert_eq!(f.get("listeners/second").await, second);
            assert_eq!(
                second["spec"]["route_config"],
                manual.listener["spec"]["route_config"]
            );
            assert_eq!(
                f.get("route-configs/manual-routes").await,
                created["route_config"]
            );
            current = created["route_config"].clone();
        }
        assert_eq!(
            routes(&current["spec"])[0],
            routes(&manual.config["spec"])[0],
            "earlier Exact exception retains precedence"
        );
        for exposure in ["attached-narrow", "attached-deep", "attached-adjacent"] {
            current = f.retained(exposure, &manual.listener, &current).await;
        }
        assert_eq!(current["spec"], manual.config["spec"]);
        assert_eq!(f.get("clusters/ordinary").await, manual.cluster);
    })
    .await;
}

#[tokio::test]
async fn append_without_wider_prefix_and_shared_root_are_deterministic() {
    case(|f| async move {
        let manual = f
            .manual(config_spec(vec![
                exact_route("exception", "ordinary", "/health"),
                ordinary_route("unrelated", "ordinary", "/else"),
            ]))
            .await;
        let a = f.attach("append", "manual", "/api").await;
        assert_eq!(
            &routes(&a["route_config"]["spec"])[..2],
            routes(&manual.config["spec"]).as_slice()
        );
        assert_eq!(routes(&a["route_config"]["spec"])[2]["name"], "append");
        let root = f.attach("root", "manual", "/").await;
        assert_eq!(
            &routes(&root["route_config"]["spec"])[..3],
            routes(&a["route_config"]["spec"]).as_slice()
        );
        assert_eq!(routes(&root["route_config"]["spec"])[3]["name"], "root");
        f.rejected_attach(
            "equal root",
            attach_body("root-collision", "manual", "/"),
            StatusCode::CONFLICT,
        )
        .await;
    })
    .await;
}

#[tokio::test]
async fn route_name_equal_prefix_exact_path_and_cluster_collisions_are_atomic() {
    for collision in ["name", "prefix", "exact", "cluster", "association"] {
        case(move |f| async move {
            let route = if collision == "exact" {
                exact_route("existing", "ordinary", "/owned")
            } else {
                ordinary_route(
                    if collision == "name" {
                        "attached"
                    } else {
                        "existing"
                    },
                    "ordinary",
                    if collision == "prefix" {
                        "/owned"
                    } else {
                        "/other"
                    },
                )
            };
            f.manual(config_spec(vec![route])).await;
            if collision == "cluster" {
                f.create("clusters", "attached-upstream", cluster_spec())
                    .await;
            }
            if collision == "association" {
                f.attach("attached", "manual", "/first").await;
            }
            f.rejected_attach(
                collision,
                attach_body("attached", "manual", "/owned"),
                StatusCode::CONFLICT,
            )
            .await;
        })
        .await;
    }
}

#[tokio::test]
async fn unsupported_shape_matrix_rejects_without_mutation() {
    for shape in [
        "http2",
        "https",
        "unbound",
        "domains",
        "domain-list",
        "multi-vhost",
        "headers",
        "query",
        "regex",
        "template",
    ] {
        case(move |f| async move {
            let mut route=ordinary_route("ordinary","ordinary","/other");
            match shape {
                "headers"=>route["headers"]=json!([{"name":"x-api-version","type":"exact","value":"2"}]),
                "query"=>route["query_parameters"]=json!([{"name":"preview","type":"present","value":true}]),
                "regex"=>route["match"]=json!({"regex":{"pattern":"^/v[0-9]+/items$"}}),
                "template"=>route["match"]=json!({"template":{"template":"/users/{id}"}}),
                _=>{},
            }
            let mut spec=config_spec(vec![route]);
            if shape=="domains" { spec["virtual_hosts"][0]["domains"]=json!(["api.example.test"]); }
            if shape=="domain-list" { spec["virtual_hosts"][0]["domains"]=json!(["*","api.example.test"]); }
            if shape=="multi-vhost" { let mut other=spec["virtual_hosts"][0].clone();other["name"]=json!("second");other["domains"]=json!(["second.example.test"]);spec["virtual_hosts"].as_array_mut().unwrap().push(other); }
            let manual=f.manual(spec).await;
            if ["http2","https","unbound"].contains(&shape) {
                let mut spec=manual.listener["spec"].clone();
                if shape=="unbound" {spec["route_config"]=Value::Null;} else {spec["protocol"]=json!(shape);}
                if shape=="https" {spec["tls_context"]=json!({"cert_chain_file":"/certs/server.crt","private_key_file":"/certs/server.key"});}
                f.patch("listeners/manual",spec,&manual.listener).await;
            }
            let status=if ["headers","query","regex","template"].contains(&shape) {StatusCode::CONFLICT} else {StatusCode::BAD_REQUEST};
            f.rejected_attach(shape,attach_body("denied","manual","/owned"),status).await;
        }).await;
    }
}

#[tokio::test]
async fn listener_input_conflicts_nulls_and_generated_name_bounds() {
    case(|f| async move {
        f.manual(config_spec(vec![ordinary_route(
            "ordinary", "ordinary", "/other",
        )]))
        .await;
        for field in ["port", "public_base_url"] {
            let mut body = attach_body("denied", "manual", "/owned");
            body[field] = if field == "port" {
                json!(f.socket.local_addr().unwrap().port())
            } else {
                json!("https://override.example")
            };
            f.rejected_attach(field, body, StatusCode::BAD_REQUEST)
                .await;
        }
        for length in [92, 94, 100] {
            let exposure = "n".repeat(length);
            f.rejected_attach(
                "generated name bound",
                attach_body(&exposure, "manual", "/owned"),
                StatusCode::BAD_REQUEST,
            )
            .await;
        }
        let mut body = attach_body(&"n".repeat(91), "manual", "/owned");
        body["port"] = Value::Null;
        body["public_base_url"] = Value::Null;
        let created = f
            .expect("POST", "expose", Some(body), None, StatusCode::CREATED)
            .await;
        assert_eq!(name(&created["cluster"]).len(), 100);
        assert_eq!(created["mode"], "attached");
    })
    .await;
}

#[tokio::test]
async fn managed_original_first_inherits_cleanup_and_final_policy_scaffold_is_deleted() {
    managed_order(true).await;
}
#[tokio::test]
async fn managed_attached_first_preserves_original_and_final_scaffold_is_deleted() {
    managed_order(false).await;
}
async fn managed_order(original_first: bool) {
    case(move |f| async move {
        let original = f.expose("original").await;
        let attached = f.attach("attached", "original", "/attached").await;
        assert_eq!(attached["listener"], original["listener"]);
        let inherited = f.association("attached").await;
        assert_eq!(inherited["cleanup_listener"], true);
        assert_eq!(inherited["cleanup_route_config"], true);
        let mut policy = attached["route_config"]["spec"].clone();
        policy["virtual_hosts"][0]["rate_limits"] =
            config_spec(vec![])["virtual_hosts"][0]["rate_limits"].clone();
        for route in policy["virtual_hosts"][0]["routes"].as_array_mut().unwrap() {
            route["action"]["timeout_secs"] = json!(23);
        }
        let current = f
            .patch(
                "route-configs/original-routes",
                policy,
                &attached["route_config"],
            )
            .await;
        let mut spec = original["listener"]["spec"].clone();
        spec["access_logs"] = json!([{"path":"/var/log/flowplane/edited.log"}]);
        let listener = f
            .patch("listeners/original", spec, &original["listener"])
            .await;
        let (first, last) = if original_first {
            ("original", "attached")
        } else {
            ("attached", "original")
        };
        let retained = f.retained(first, &listener, &current).await;
        assert_eq!(routes(&retained["spec"]).len(), 1);
        assert_eq!(routes(&retained["spec"])[0]["name"], last);
        let surviving = f.association(last).await;
        assert_eq!(surviving["cleanup_listener"], true);
        assert_eq!(surviving["cleanup_route_config"], true);
        let before = f.snapshot().await;
        disposition(&f.remove(last, StatusCode::OK).await, "deleted");
        let after = f.snapshot().await;
        for table in [
            "clusters",
            "route_configs",
            "listeners",
            "exposures",
            "route_config_cluster_refs",
            "listener_route_config_refs",
        ] {
            assert_eq!(after[table], json!([]), "final managed {table}");
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
async fn borrowed_last_route_requires_genuine_replacement_and_retains_manual_scaffold() {
    case(|f| async move {
        let manual = f
            .manual(config_spec(vec![ordinary_route(
                "genuine",
                "ordinary",
                "/ordinary",
            )]))
            .await;
        let attached = f.attach("borrowed", "manual", "/owned").await;
        let mut sole = attached["route_config"]["spec"].clone();
        sole["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["name"] == "borrowed");
        let current = f
            .patch(
                "route-configs/manual-routes",
                sole,
                &attached["route_config"],
            )
            .await;
        let before = f.snapshot().await;
        let error = f.remove("borrowed", StatusCode::CONFLICT).await;
        assert_error(&error, "conflict");
        assert!(
            error["hint"].as_str().is_some_and(|s| !s.is_empty()),
            "genuine-route remediation guidance"
        );
        assert_eq!(f.snapshot().await, before);
        let mut restored = current["spec"].clone();
        restored["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .push(routes(&manual.config["spec"])[0].clone());
        let updated = f
            .patch("route-configs/manual-routes", restored, &current)
            .await;
        let result = f.retained("borrowed", &manual.listener, &updated).await;
        assert_eq!(result["spec"], manual.config["spec"]);
        assert_eq!(f.get("clusters/ordinary").await, manual.cluster);
    })
    .await;
}

#[tokio::test]
async fn http_tls_filters_access_logs_and_unconfigured_endpoint_are_preserved() {
    case(|f| async move {
        let manual=f.manual(config_spec(vec![ordinary_route("ordinary","ordinary","/other")])).await;
        let mut spec=manual.listener["spec"].clone();
        spec["public_base_url"]=Value::Null;
        spec["tls_context"]=json!({"cert_chain_file":"/certs/server.crt","private_key_file":"/certs/server.key"});
        spec["http_filters"]=json!([{"filter":{"type":"global_rate_limit","domain":"flowplane","service_cluster":"ordinary","timeout_ms":50,"failure_mode_deny":true,"stage":1,"request_type":"external","stat_prefix":"shared_rls","enable_x_ratelimit_headers":true,"disable_x_envoy_ratelimited_header":true,"rate_limited_status":429,"status_on_error":503}}]);
        let listener=f.patch("listeners/manual",spec,&manual.listener).await;
        let before=f.snapshot().await;
        let mut request=attach_body("default-root","manual","/");
        request.as_object_mut().unwrap().remove("path");
        let created=f.expect("POST","expose",Some(request),None,StatusCode::CREATED).await;
        assert_eq!(created["mode"],"attached");
        assert!(created.get("curl_url").is_none(),"unconfigured curl_url omitted; no host guessing");
        assert_eq!(created["endpoint_source"],"unconfigured");
        assert_eq!(created["listener"],listener,"HTTP HCM with TLS accepted without listener rewrite");
        assert_eq!(routes(&created["route_config"]["spec"])[1]["match"],json!({"prefix":{"prefix":"/"}}));
        let after=f.snapshot().await;
        unchanged_rows(&before,&after,&["listeners","listener_route_config_refs"]);
        assert_new_effects(&before,&after,&[("cluster","create"),("route_config","update")]);
        let retained=f.retained("default-root",&listener,&created["route_config"]).await;
        assert_eq!(retained["spec"],manual.config["spec"]);
    }).await;
}

#[tokio::test]
async fn managed_fanout_last_route_conflicts_then_genuine_route_permits_retention() {
    case(|f| async move {
        let original = f.expose("original").await;
        let attached = f.attach("attached", "original", "/attached").await;
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let second = f
            .create(
                "listeners",
                "second",
                listener_spec(socket.local_addr().unwrap().port(), "original-routes"),
            )
            .await;
        let current = f
            .retained("original", &original["listener"], &attached["route_config"])
            .await;
        let before = f.snapshot().await;
        let error = f.remove("attached", StatusCode::CONFLICT).await;
        assert_error(&error, "conflict");
        assert!(error["hint"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(
            f.snapshot().await,
            before,
            "extra listener prevents destructive final cleanup"
        );
        let ordinary = f.create("clusters", "ordinary", cluster_spec()).await;
        let mut spec = current["spec"].clone();
        spec["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .push(ordinary_route("genuine", "ordinary", "/genuine"));
        let changed = f
            .patch("route-configs/original-routes", spec, &current)
            .await;
        let retained = f
            .retained("attached", &original["listener"], &changed)
            .await;
        assert_eq!(routes(&retained["spec"]).len(), 1);
        assert_eq!(routes(&retained["spec"])[0]["name"], "genuine");
        assert_eq!(f.get("listeners/second").await, second);
        assert_eq!(f.get("clusters/ordinary").await, ordinary);
        assert_eq!(f.snapshot().await["exposures"], json!([]));
    })
    .await;
}

#[tokio::test]
async fn second_manual_listener_borrows_exact_pair_without_managed_cleanup_rights() {
    case(|f| async move {
        let original = f.expose("original").await;
        assert_eq!(original["mode"], "created");
        let owner = f.association("original").await;
        assert_eq!(owner["cleanup_listener"], true);
        assert_eq!(owner["cleanup_route_config"], true);
        assert_eq!(owner["listener_id"], original["listener"]["id"]);
        assert_eq!(owner["route_config_id"], original["route_config"]["id"]);
        assert_eq!(owner["cluster_id"], original["cluster"]["id"]);

        // A shared configuration alone is not the exact listener/config pair.
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let second = f
            .create(
                "listeners",
                "second-manual",
                listener_spec(socket.local_addr().unwrap().port(), "original-routes"),
            )
            .await;
        assert_ne!(second["id"], original["listener"]["id"]);
        assert_eq!(
            second["spec"]["route_config"],
            original["route_config"]["name"]
        );
        assert_eq!(revision(&second), 1);
        let before = f.snapshot().await;
        let attached = f.attach("borrowed", "second-manual", "/borrowed").await;
        assert_eq!(attached["mode"], "attached");
        assert_eq!(attached["listener"], second);
        assert_eq!(
            attached["route_config"]["id"],
            original["route_config"]["id"]
        );
        assert_eq!(
            revision(&attached["route_config"]),
            revision(&original["route_config"]) + 1
        );
        assert_eq!(revision(&attached["cluster"]), 1);
        let mut expected = original["route_config"]["spec"].clone();
        let added = routes(&attached["route_config"]["spec"])[1].clone();
        assert_eq!(added["name"], "borrowed");
        assert_eq!(added["match"], json!({"prefix":{"prefix":"/borrowed"}}));
        assert_eq!(added["action"]["cluster"], attached["cluster"]["name"]);
        expected["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .push(added);
        assert_eq!(attached["route_config"]["spec"], expected);
        let borrowed = f.association("borrowed").await;
        assert_eq!(borrowed["cleanup_listener"], false);
        assert_eq!(borrowed["cleanup_route_config"], false);
        assert_eq!(borrowed["listener_id"], second["id"]);
        assert_eq!(borrowed["route_config_id"], original["route_config"]["id"]);
        assert_eq!(borrowed["cluster_id"], attached["cluster"]["id"]);
        assert_eq!(borrowed["virtual_host"], "default");
        assert_eq!(borrowed["route_name"], "borrowed");
        assert_eq!(borrowed["version"], 1);
        let after = f.snapshot().await;
        unchanged_rows(
            &before,
            &after,
            &["listeners", "listener_route_config_refs"],
        );
        assert_new_effects(
            &before,
            &after,
            &[("cluster", "create"), ("route_config", "update")],
        );
        assert_eq!(f.association("original").await, owner);
        assert_eq!(f.get("listeners/original").await, original["listener"]);
        assert_eq!(
            f.get("route-configs/original-routes").await,
            attached["route_config"]
        );

        let mut policy = attached["route_config"]["spec"].clone();
        policy["virtual_hosts"][0]["rate_limits"] =
            config_spec(vec![])["virtual_hosts"][0]["rate_limits"].clone();
        let configured = f
            .patch(
                "route-configs/original-routes",
                policy,
                &attached["route_config"],
            )
            .await;
        assert_eq!(configured["id"], original["route_config"]["id"]);
        assert_eq!(
            revision(&configured),
            revision(&attached["route_config"]) + 1
        );
        let current = f
            .retained("original", &original["listener"], &configured)
            .await;
        assert_eq!(routes(&current["spec"]).len(), 1);
        assert_eq!(routes(&current["spec"])[0]["name"], "borrowed");
        assert_eq!(
            f.association("borrowed").await,
            borrowed,
            "no cleanup-rights transfer"
        );
        assert_eq!(f.get("listeners/second-manual").await, second);
        assert_eq!(
            f.get("clusters/borrowed-upstream").await,
            attached["cluster"]
        );
        assert_eq!(f.snapshot().await["exposures"], json!([borrowed]));

        let before = f.snapshot().await;
        let error = f.remove("borrowed", StatusCode::CONFLICT).await;
        assert_error(&error, "conflict");
        assert!(
            error["hint"].as_str().is_some_and(|s| !s.is_empty()),
            "genuine replacement route remediation"
        );
        assert_eq!(
            f.snapshot().await,
            before,
            "last borrowed route conflict is fully atomic"
        );
        assert_eq!(f.get("route-configs/original-routes").await, current);

        let ordinary = f.create("clusters", "ordinary", cluster_spec()).await;
        let mut spec = current["spec"].clone();
        spec["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .push(ordinary_route("genuine", "ordinary", "/genuine"));
        let before = f.snapshot().await;
        let updated = f
            .patch("route-configs/original-routes", spec, &current)
            .await;
        assert_eq!(updated["id"], current["id"]);
        assert_eq!(revision(&updated), revision(&current) + 1);
        let genuine = routes(&updated["spec"])[1].clone();
        assert_eq!(genuine["name"], "genuine");
        assert_eq!(genuine["match"], json!({"prefix":{"prefix":"/genuine"}}));
        assert_eq!(genuine["action"]["cluster"], ordinary["name"]);
        assert_eq!(genuine["action"]["timeout_secs"], 17);
        let mut without_replacement = updated["spec"].clone();
        without_replacement["virtual_hosts"][0]["routes"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(
            without_replacement, current["spec"],
            "ordinary PATCH preserves all other policy"
        );
        let after = f.snapshot().await;
        unchanged_rows(
            &before,
            &after,
            &[
                "clusters",
                "listeners",
                "exposures",
                "listener_route_config_refs",
            ],
        );
        assert_new_effects(&before, &after, &[("route_config", "update")]);

        let retained = f.retained("borrowed", &second, &updated).await;
        assert_eq!(retained["id"], original["route_config"]["id"]);
        assert_eq!(routes(&retained["spec"]), &vec![genuine]);
        assert_eq!(
            retained["spec"]["virtual_hosts"][0]["rate_limits"],
            configured["spec"]["virtual_hosts"][0]["rate_limits"]
        );
        assert_eq!(f.get("listeners/original").await, original["listener"]);
        assert_eq!(f.get("listeners/second-manual").await, second);
        assert_eq!(f.get("clusters/ordinary").await, ordinary);
        let remaining = f.snapshot().await;
        assert_eq!(remaining["exposures"], json!([]));
        assert_eq!(remaining["clusters"].as_array().unwrap().len(), 1);
        assert_eq!(remaining["route_configs"].as_array().unwrap().len(), 1);
        assert_eq!(remaining["listeners"].as_array().unwrap().len(), 2);

        // No inferred shortcut ownership: explicit dependency-ordered cleanup
        // uses the exact surviving identities and their current revisions.
        for (path, view, resource) in [
            ("listeners/second-manual", &second, "listener"),
            ("listeners/original", &original["listener"], "listener"),
            ("route-configs/original-routes", &retained, "route_config"),
            ("clusters/ordinary", &ordinary, "cluster"),
        ] {
            assert_eq!(f.get(path).await, *view);
            let before = f.snapshot().await;
            let body = f
                .expect(
                    "DELETE",
                    path,
                    None,
                    Some(revision(view)),
                    StatusCode::NO_CONTENT,
                )
                .await;
            assert_eq!(body, Value::Null);
            let error = f
                .expect("GET", path, None, None, StatusCode::NOT_FOUND)
                .await;
            assert_error(&error, "not_found");
            assert_new_effects(&before, &f.snapshot().await, &[(resource, "delete")]);
        }
        let final_state = f.snapshot().await;
        for table in [
            "clusters",
            "route_configs",
            "listeners",
            "exposures",
            "route_config_cluster_refs",
            "listener_route_config_refs",
        ] {
            assert_eq!(final_state[table], json!([]), "ordinary cleanup {table}");
        }
    })
    .await;
}

#[tokio::test]
async fn explicit_legacy_attachment_is_borrowed_and_legacy_identity_is_not_adopted() {
    case(|f| async move {
        let cluster = f
            .create("clusters", "legacy-upstream", cluster_spec())
            .await;
        let config = f
            .create(
                "route-configs",
                "legacy-routes",
                config_spec(vec![ordinary_route("all", "legacy-upstream", "/")]),
            )
            .await;
        let listener = f
            .create(
                "listeners",
                "legacy",
                listener_spec(f.socket.local_addr().unwrap().port(), "legacy-routes"),
            )
            .await;
        let before = f.snapshot().await;
        let error = f.remove("legacy", StatusCode::NOT_FOUND).await;
        assert_error(&error, "not_found");
        assert!(error["hint"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(f.snapshot().await, before);
        let attached = f.attach("new", "legacy", "/new").await;
        let association = f.association("new").await;
        assert_eq!(association["cleanup_listener"], false);
        assert_eq!(association["cleanup_route_config"], false);
        assert_eq!(
            f.snapshot().await["exposures"].as_array().unwrap().len(),
            1,
            "no legacy backfill/adoption"
        );
        let retained = f
            .retained("new", &listener, &attached["route_config"])
            .await;
        assert_eq!(retained["spec"], config["spec"]);
        assert_eq!(f.get("clusters/legacy-upstream").await, cluster);
    })
    .await;
}
