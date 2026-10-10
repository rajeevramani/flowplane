//! Independent S2 contract tests, authored from the approved exposure-first-use
//! design (AC4/6/7/13/14), not the expose/service/storage implementations.
//! Uses the real router, RS256 bearer middleware, and PostgreSQL. No shared-mode
//! selector, mocks, schema alteration, silent DB skip, or legacy backfill fixture.
//! Unset FLOWPLANE_TEST_DATABASE_URL means a visible skip; configured DB errors fail.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole, TeamId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{future::Future, net::TcpListener, sync::Arc};
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

#[derive(Clone)]
struct Fixture {
    app: Router,
    pool: PgPool,
    issuer: Arc<DevIssuer>,
    org: OrgId,
    team: TeamId,
    team_name: String,
    admin: String,
    // Reserve an ephemeral port for this test's explicit-port fixtures. The
    // in-process router never binds an Envoy socket; holding it avoids collisions
    // with this target's concurrent cases without hard-coded manual ports.
    socket: Arc<TcpListener>,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let url = match std::env::var("FLOWPLANE_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(std::env::VarError::NotPresent) => {
                eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
                return None;
            }
            Err(_) => panic!("invalid FLOWPLANE_TEST_DATABASE_URL (credentials redacted)"),
        };
        let pool = fp_storage::connect(&url, 4).await.unwrap_or_else(|_| {
            panic!(
                "required DB: cannot connect to FLOWPLANE_TEST_DATABASE_URL (credentials redacted)"
            )
        });
        fp_storage::migrate(&pool)
            .await
            .expect("required DB: migrations must succeed");
        let issuer = Arc::new(DevIssuer::generate().expect("issuer"));
        let validator = fp_core::OidcValidator::new(issuer.oidc_config());
        validator
            .load_jwks_json(issuer.jwks_json())
            .await
            .expect("jwks");
        let subject = unique("admin");
        let admin = issuer
            .mint(&subject, "exposure@test", "Exposure contract", 600)
            .expect("mint");
        let org = identity::create_org(&pool, &unique("org"), "")
            .await
            .expect("org");
        let team = identity::create_team(&pool, org.id, &unique("team"), "")
            .await
            .expect("team");
        let user =
            identity::upsert_user_by_subject(&pool, &subject, "exposure@test", "Exposure contract")
                .await
                .expect("user");
        identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
            .await
            .expect("admin membership");
        let app = fp_api::build_router(fp_api::AppState {
            pool: pool.clone(),
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            version: "exposure-contract",
            validator: Some(Arc::new(validator)),
            write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
            xds_readiness: None,
            xds_degraded: None,
            discovery_forwarding_policy: Default::default(),
            egress_advisory: Default::default(),
            rls_repush: None,
            rls_grpc_configured: false,
        });
        Some(Self {
            app,
            pool,
            issuer,
            org: org.id,
            team: team.id,
            team_name: team.name,
            admin,
            socket: Arc::new(TcpListener::bind("127.0.0.1:0").expect("unique port reservation")),
        })
    }

    fn port(&self) -> u16 {
        self.socket.local_addr().expect("reserved address").port()
    }
    fn path(&self, suffix: &str) -> String {
        format!("/api/v1/teams/{}/{suffix}", self.team_name)
    }
    fn expose_body(&self, name: &str) -> Value {
        json!({"name": name, "upstream": "http://127.0.0.1:3001", "path": "/owned",
            "port": self.port(), "public_base_url": "https://gateway.example"})
    }

    async fn request(
        &self,
        token: &str,
        method: &str,
        suffix: &str,
        body: Option<Value>,
        revision: Option<i64>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(self.path(suffix))
            .header("authorization", format!("Bearer {token}"));
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
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("real router response");
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
            serde_json::from_slice(&bytes).expect("JSON response")
        };
        (status, body)
    }

    async fn admin_request(
        &self,
        method: &str,
        suffix: &str,
        body: Option<Value>,
        revision: Option<i64>,
        expected: StatusCode,
    ) -> Value {
        let (status, body) = self
            .request(&self.admin, method, suffix, body, revision)
            .await;
        assert_eq!(status, expected, "{method} {suffix}: {body}");
        body
    }

    async fn create(&self, kind: &str, name: &str, spec: Value) -> Value {
        self.admin_request(
            "POST",
            kind,
            Some(json!({"name":name,"spec":spec})),
            None,
            StatusCode::CREATED,
        )
        .await
    }

    async fn expose(&self, name: &str) -> Value {
        self.admin_request(
            "POST",
            "expose",
            Some(self.expose_body(name)),
            None,
            StatusCode::CREATED,
        )
        .await
    }

    async fn remove(&self, name: &str, expected: StatusCode) -> Value {
        self.admin_request("DELETE", &format!("expose/{name}"), None, None, expected)
            .await
    }

    async fn member(&self, grants: &[(Resource, Action)]) -> String {
        let subject = unique("member");
        let user = identity::upsert_user_by_subject(&self.pool, &subject, "member@test", "Member")
            .await
            .expect("member user");
        identity::add_org_membership(&self.pool, user, self.org, OrgRole::Member)
            .await
            .expect("member membership");
        for &(resource, action) in grants {
            identity::add_grant(
                &self.pool, user, self.org, self.team, resource, action, None,
            )
            .await
            .expect("grant");
        }
        self.issuer
            .mint(&subject, "member@test", "Member", 600)
            .expect("member token")
    }

    // Whole row snapshots include IDs, versions, JSON policy, and timestamps:
    // create-and-compensate is not allowed to leave audit/outbox residue either.
    async fn snapshot(&self) -> Value {
        let mut result = serde_json::Map::new();
        for table in ["clusters", "route_configs", "listeners", "exposures"] {
            let rows: Vec<Value> = sqlx::query_scalar(&format!(
                "SELECT to_jsonb(t) FROM {table} t WHERE team_id = $1 ORDER BY id"
            ))
            .bind(self.team.as_uuid())
            .fetch_all(&self.pool)
            .await
            .expect("scoped product snapshot");
            result.insert(table.into(), json!(rows));
        }
        let events: Vec<Value> =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM events e WHERE team_id = $1 ORDER BY seq")
                .bind(self.team.as_uuid())
                .fetch_all(&self.pool)
                .await
                .expect("outbox snapshot");
        result.insert("events".into(), json!(events));
        // Denial audits are legitimate, but no gateway success action or shortcut
        // success action may be committed by a failed mutation.
        let audits: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_log a WHERE team_id = $1 AND (action LIKE 'cluster.%' OR action LIKE 'route_config.%' OR action LIKE 'listener.%' OR action LIKE 'expose.%') ORDER BY to_jsonb(a)::text")
            .bind(self.team.as_uuid()).fetch_all(&self.pool).await.expect("gateway audit snapshot");
        result.insert("audit".into(), json!(audits));
        Value::Object(result)
    }

    async fn inventories(&self, expected: &[(&str, &str, &Value)]) {
        for kind in ["clusters", "route-configs", "listeners"] {
            let body = self
                .admin_request("GET", kind, None, None, StatusCode::OK)
                .await;
            let wanted: Vec<_> = expected.iter().filter(|(k, _, _)| *k == kind).collect();
            let items = body["items"].as_array().expect("normal inventory items");
            assert_eq!(body["total"], json!(wanted.len()));
            assert_eq!(items.len(), wanted.len());
            for (_, name, view) in wanted {
                let item = items
                    .iter()
                    .find(|item| item["name"] == *name)
                    .expect("visible user-owned resource");
                assert_eq!(item["id"], view["id"]);
                assert_eq!(item["revision"], view["revision"]);
                let get = self
                    .admin_request("GET", &format!("{kind}/{name}"), None, None, StatusCode::OK)
                    .await;
                assert_eq!(&get, *view, "create view must equal ordinary GET");
            }
        }
    }

    async fn association(&self, name: &str) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(e) FROM exposures e WHERE team_id=$1 AND name=$2")
            .bind(self.team.as_uuid())
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .expect("durable exposure identity")
    }

    // Runs even when a spawned assertion panics. Delete only this UUID team's
    // fixtures in FK-safe order. Identities/audit are retained like api_crud's
    // fixture convention; no shared schema, global cleanup, or migration rewind.
    async fn cleanup(&self) {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass('exposures') IS NOT NULL")
            .fetch_one(&self.pool)
            .await
            .expect("cleanup schema check");
        if exists {
            sqlx::query("DELETE FROM exposures WHERE team_id=$1")
                .bind(self.team.as_uuid())
                .execute(&self.pool)
                .await
                .expect("owned associations cleanup");
        }
        for table in ["listeners", "route_configs", "clusters"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE team_id=$1"))
                .bind(self.team.as_uuid())
                .execute(&self.pool)
                .await
                .expect("owned gateway fixture cleanup");
        }
    }
}

async fn case<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let outcome = tokio::spawn(test(fixture.clone())).await;
    fixture.cleanup().await;
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
fn route_spec(cluster: &str, route: &str) -> Value {
    json!({"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{
        "name":route,"match":{"prefix":{"prefix":"/owned"}},"action":{"cluster":cluster}}]}]})
}
fn listener_spec(port: u16, config: &str) -> Value {
    json!({"address":"0.0.0.0","port":port,"protocol":"http","route_config":config})
}

fn assert_error(body: &Value, code: &str) {
    assert_eq!(body["code"], code, "stable error envelope: {body}");
}

fn assert_effects(snapshot: &Value, verb: &str) {
    let event_suffix = if verb == "create" {
        "upserted"
    } else {
        "deleted"
    };
    let events = snapshot["events"].as_array().expect("events");
    let audits = snapshot["audit"].as_array().expect("audit");
    for resource in ["cluster", "route_config", "listener"] {
        assert_eq!(
            events
                .iter()
                .filter(|e| e["event_type"] == format!("{resource}.{event_suffix}"))
                .count(),
            1,
            "exactly one ordinary event per changed gateway row"
        );
        assert_eq!(
            audits
                .iter()
                .filter(|a| a["action"] == format!("{resource}.{verb}"))
                .count(),
            1,
            "exactly one ordinary success audit per changed gateway row"
        );
    }
    assert!(
        !audits
            .iter()
            .any(|a| a["action"].as_str().expect("action").starts_with("expose.")),
        "no new shortcut success action"
    );
}

#[tokio::test]
async fn independent_create_and_remove_publish_exact_graph_and_effects() {
    case(|f| async move {
        let name = unique("owned");
        let created = f.expose(&name).await;
        assert_eq!(created["mode"], "created");
        assert_eq!(created["curl_url"], "https://gateway.example/owned");
        assert_eq!(created["endpoint_source"], "listener.public_base_url");
        for key in ["cluster", "route_config", "listener"] {
            uuid::Uuid::parse_str(created[key]["id"].as_str().expect("response ID"))
                .expect("UUID response ID");
            assert_eq!(created[key]["revision"], 1);
        }
        let cluster = format!("{name}-upstream");
        let config = format!("{name}-routes");
        assert_eq!(
            created["route_config"]["spec"]["virtual_hosts"][0]["routes"][0]["name"],
            name
        );
        f.inventories(&[
            ("clusters", &cluster, &created["cluster"]),
            ("route-configs", &config, &created["route_config"]),
            ("listeners", &name, &created["listener"]),
        ])
        .await;
        let association = f.association(&name).await;
        for (field, key) in [
            ("cluster_id", "cluster"),
            ("route_config_id", "route_config"),
            ("listener_id", "listener"),
        ] {
            assert_eq!(association[field], created[key]["id"]);
        }
        assert_eq!(association["cleanup_listener"], true);
        assert_eq!(association["cleanup_route_config"], true);
        assert_eq!(association["version"], 1);
        let before = f.snapshot().await;
        assert_effects(&before, "create");
        assert_eq!(before["events"].as_array().unwrap().len(), 3);
        assert_eq!(before["audit"].as_array().unwrap().len(), 3);
        // An unrelated ordinary graph in the SAME team must survive exact-ID
        // cleanup. Cross-team-only isolation would not catch overbroad teardown.
        let sibling = unique("sibling");
        let sibling_config_name = format!("{sibling}-routes");
        let sibling_cluster = f.create("clusters", &sibling, cluster_spec()).await;
        let sibling_config = f
            .create(
                "route-configs",
                &sibling_config_name,
                route_spec(&sibling, "manual"),
            )
            .await;
        let sibling_socket = TcpListener::bind("127.0.0.1:0").expect("sibling port");
        let sibling_listener = f
            .create(
                "listeners",
                &sibling,
                listener_spec(
                    sibling_socket.local_addr().unwrap().port(),
                    &sibling_config_name,
                ),
            )
            .await;
        let sibling_before = f.snapshot().await;
        let removed = f.remove(&name, StatusCode::OK).await;
        assert_eq!(removed["cluster_name"], cluster);
        assert_eq!(removed["route_config_name"], config);
        assert_eq!(removed["listener_name"], name);
        for field in [
            "cluster_disposition",
            "route_config_disposition",
            "listener_disposition",
        ] {
            assert_eq!(removed[field], "deleted");
        }
        f.inventories(&[
            ("clusters", &sibling, &sibling_cluster),
            ("route-configs", &sibling_config_name, &sibling_config),
            ("listeners", &sibling, &sibling_listener),
        ])
        .await;
        let after = f.snapshot().await;
        assert_eq!(after["exposures"], json!([]));
        for (table, view) in [
            ("clusters", &sibling_cluster),
            ("route_configs", &sibling_config),
            ("listeners", &sibling_listener),
        ] {
            let original_row = sibling_before[table]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == view["id"])
                .unwrap();
            assert_eq!(
                after[table],
                json!([original_row]),
                "unrelated graph must remain byte-for-byte unchanged"
            );
        }
        assert_effects(&after, "delete");
        assert_eq!(after["events"].as_array().unwrap().len(), 9);
        assert_eq!(after["audit"].as_array().unwrap().len(), 9);
        assert_error(&f.remove(&name, StatusCode::NOT_FOUND).await, "not_found");
        assert_eq!(f.snapshot().await, after, "repeat removal has no effects");
    })
    .await;
}

#[tokio::test]
async fn late_explicit_listener_port_conflict_rolls_back_every_effect() {
    case(|f| async move {
        let occupied = unique("occupied");
        // A valid ordinary listener can be unbound. Its occupied port forces a
        // late scaffold conflict, not an early cluster-name uniqueness failure.
        f.create(
            "listeners",
            &occupied,
            json!({"address":"0.0.0.0","port":f.port(),"protocol":"http"}),
        )
        .await;
        let before = f.snapshot().await;
        let name = unique("late");
        let body = f
            .admin_request(
                "POST",
                "expose",
                Some(f.expose_body(&name)),
                None,
                StatusCode::CONFLICT,
            )
            .await;
        assert_error(&body, "conflict");
        assert_eq!(
            f.snapshot().await,
            before,
            "late failure must not commit cluster/config/association or compensating audit/outbox"
        );
        for suffix in [
            format!("clusters/{name}-upstream"),
            format!("route-configs/{name}-routes"),
            format!("listeners/{name}"),
        ] {
            f.admin_request("GET", &suffix, None, None, StatusCode::NOT_FOUND)
                .await;
        }
    })
    .await;
}

#[tokio::test]
async fn every_missing_create_grant_prevents_earlier_mutations() {
    case(|f| async move {
        let grants = [
            (Resource::Clusters, Action::Create),
            (Resource::RouteConfigs, Action::Create),
            (Resource::Listeners, Action::Create),
        ];
        for missing in 0..grants.len() {
            let allowed: Vec<_> = grants
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != missing)
                .map(|(_, grant)| *grant)
                .collect();
            let token = f.member(&allowed).await;
            let before = f.snapshot().await;
            let (status, body) = f
                .request(
                    &token,
                    "POST",
                    "expose",
                    Some(f.expose_body(&unique("denied"))),
                    None,
                )
                .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "missing {:?}: {body}",
                grants[missing]
            );
            assert_error(&body, "forbidden");
            assert_eq!(
                f.snapshot().await,
                before,
                "all create permissions precede any product writes"
            );
        }
        // Explicit port does NOT require Listeners Read: authenticated positive
        // control using precisely the complete approved mutation grant set.
        let token = f.member(&grants).await;
        let name = unique("exact");
        let (status, body) = f
            .request(&token, "POST", "expose", Some(f.expose_body(&name)), None)
            .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "exact explicit-port grants: {body}"
        );
    })
    .await;
}

#[tokio::test]
async fn every_missing_final_delete_grant_preserves_managed_identity() {
    case(|f| async move {
        let name = unique("delete");
        f.expose(&name).await;
        let reads = [
            (Resource::Clusters, Action::Read),
            (Resource::RouteConfigs, Action::Read),
            (Resource::Listeners, Action::Read),
        ];
        let deletes = [
            (Resource::Clusters, Action::Delete),
            (Resource::RouteConfigs, Action::Delete),
            (Resource::Listeners, Action::Delete),
        ];
        for missing in 0..deletes.len() {
            let mut allowed = reads.to_vec();
            allowed.extend(
                deletes
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != missing)
                    .map(|(_, g)| *g),
            );
            let token = f.member(&allowed).await;
            let before = f.snapshot().await;
            let (status, body) = f
                .request(&token, "DELETE", &format!("expose/{name}"), None, None)
                .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "missing {:?}: {body}",
                deletes[missing]
            );
            assert_error(&body, "forbidden");
            assert_eq!(
                f.snapshot().await,
                before,
                "later delete denial must not erase an earlier resource or association"
            );
        }
        let mut all = reads.to_vec();
        all.extend(deletes);
        let token = f.member(&all).await;
        let (status, body) = f
            .request(&token, "DELETE", &format!("expose/{name}"), None, None)
            .await;
        assert_eq!(status, StatusCode::OK, "exact final-cleanup grants: {body}");
        f.inventories(&[]).await;
    })
    .await;
}

#[tokio::test]
async fn cluster_config_listener_name_collisions_and_duplicate_preserve_ids_versions() {
    // Each collision runs in a fresh team, so no collision case can be masked by
    // an earlier leftover port/resource. Fixture writes use ordinary REST only.
    for collision in ["cluster", "route_config", "listener", "duplicate"] {
        case(move |f| async move {
            let name = unique("collision");
            match collision {
                "cluster" => {
                    f.create("clusters", &format!("{name}-upstream"), cluster_spec())
                        .await;
                }
                "route_config" => {
                    let upstream = unique("manual");
                    f.create("clusters", &upstream, cluster_spec()).await;
                    f.create(
                        "route-configs",
                        &format!("{name}-routes"),
                        route_spec(&upstream, "manual"),
                    )
                    .await;
                }
                "listener" => {
                    f.create(
                        "listeners",
                        &name,
                        json!({"address":"0.0.0.0","port":f.port()}),
                    )
                    .await;
                }
                "duplicate" => {
                    f.expose(&name).await;
                }
                _ => unreachable!(),
            }
            let before = f.snapshot().await;
            let body = f
                .admin_request(
                    "POST",
                    "expose",
                    Some(f.expose_body(&name)),
                    None,
                    StatusCode::CONFLICT,
                )
                .await;
            assert_error(&body, "conflict");
            assert_eq!(
                f.snapshot().await,
                before,
                "{collision} must never replace IDs/versions or append success evidence"
            );
        })
        .await;
    }
}

#[tokio::test]
async fn manual_legacy_matching_names_are_never_shortcut_deleted_or_adopted() {
    case(|f| async move {
        let name = unique("legacy");
        let cluster_name = format!("{name}-upstream");
        let config_name = format!("{name}-routes");
        let cluster = f.create("clusters", &cluster_name, cluster_spec()).await;
        // The old shortcut used route 'all'; ordinary rows are deliberately not
        // inserted into the new association table, regardless of matching names.
        let config = f
            .create(
                "route-configs",
                &config_name,
                route_spec(&cluster_name, "all"),
            )
            .await;
        let listener = f
            .create("listeners", &name, listener_spec(f.port(), &config_name))
            .await;
        let before = f.snapshot().await;
        let body = f.remove(&name, StatusCode::NOT_FOUND).await;
        assert_error(&body, "not_found");
        assert!(!body["hint"]
            .as_str()
            .expect("legacy manual-cleanup hint")
            .is_empty());
        assert_eq!(f.snapshot().await, before, "names do not prove provenance");
        f.inventories(&[
            ("clusters", &cluster_name, &cluster),
            ("route-configs", &config_name, &config),
            ("listeners", &name, &listener),
        ])
        .await;
    })
    .await;
}

#[tokio::test]
async fn surviving_cluster_reference_or_extra_listener_blocks_entire_removal() {
    for dependent in ["cluster-route", "extra-listener"] {
        case(move |f| async move {
            let name = unique("protected");
            let created = f.expose(&name).await;
            let other = unique("dependent");
            if dependent == "cluster-route" {
                f.create(
                    "route-configs",
                    &other,
                    route_spec(&format!("{name}-upstream"), "other"),
                )
                .await;
            } else {
                let socket = TcpListener::bind("127.0.0.1:0").expect("dependent port");
                f.create(
                    "listeners",
                    &other,
                    listener_spec(
                        socket.local_addr().unwrap().port(),
                        &format!("{name}-routes"),
                    ),
                )
                .await;
            }
            let before = f.snapshot().await;
            let body = f.remove(&name, StatusCode::CONFLICT).await;
            assert_error(&body, "conflict");
            assert_eq!(
                f.snapshot().await,
                before,
                "{dependent} must roll back all resources, membership, events and audits"
            );
            assert_eq!(
                f.association(&name).await["cluster_id"],
                created["cluster"]["id"]
            );
        })
        .await;
    }
}

#[tokio::test]
async fn ordinary_deletes_are_exposure_conflicts_and_stale_route_can_be_restored() {
    case(|f| async move {
        let name = unique("restore");
        let created = f.expose(&name).await;
        for (kind, key) in [
            ("clusters", "cluster"),
            ("route-configs", "route_config"),
            ("listeners", "listener"),
        ] {
            let before = f.snapshot().await;
            let body = f
                .admin_request(
                    "DELETE",
                    &format!("{kind}/{}", created[key]["name"].as_str().unwrap()),
                    None,
                    Some(1),
                    StatusCode::CONFLICT,
                )
                .await;
            assert_error(&body, "conflict");
            assert!(
                body.to_string().contains(&name),
                "conflict names owning exposure"
            );
            assert!(body["hint"]
                .as_str()
                .expect("supported cleanup hint")
                .contains("unexpose"));
            assert_eq!(f.snapshot().await, before);
        }
        let config_path = format!("route-configs/{name}-routes");
        let original = created["route_config"]["spec"].clone();
        let mut renamed = original.clone();
        renamed["virtual_hosts"][0]["routes"][0]["name"] = json!("renamed");
        let changed = f
            .admin_request(
                "PATCH",
                &config_path,
                Some(json!({"spec":renamed})),
                Some(1),
                StatusCode::OK,
            )
            .await;
        let before = f.snapshot().await;
        let body = f.remove(&name, StatusCode::CONFLICT).await;
        assert_error(&body, "conflict");
        let hint = body["hint"].as_str().expect("stale restore guidance");
        assert!(!hint.is_empty());
        assert!(
            body.to_string().contains(&name),
            "precise stale identity diagnostic"
        );
        assert_eq!(
            f.snapshot().await,
            before,
            "stale membership must never select a renamed route by names alone"
        );
        f.admin_request(
            "PATCH",
            &config_path,
            Some(json!({"spec":original})),
            Some(changed["revision"].as_i64().unwrap()),
            StatusCode::OK,
        )
        .await;
        f.remove(&name, StatusCode::OK).await;
        f.inventories(&[]).await;
    })
    .await;
}

#[tokio::test]
async fn ordinary_extra_route_is_preserved_with_exact_route_only_removal_grants() {
    case(|f| async move {
        let name = unique("retain");
        let created = f.expose(&name).await;
        let unrelated = unique("unrelated");
        let other_cluster = f.create("clusters", &unrelated, cluster_spec()).await;
        let mut spec = created["route_config"]["spec"].clone();
        let other_route = json!({"name":"genuine-other","match":{"prefix":{"prefix":"/other"}},"action":{"cluster":unrelated,"timeout_secs":17}});
        spec["virtual_hosts"][0]["routes"].as_array_mut().unwrap().push(other_route);
        let path = format!("route-configs/{name}-routes");
        let changed = f.admin_request("PATCH", &path, Some(json!({"spec":spec})), Some(1), StatusCode::OK).await;
        let token = f.member(&[(Resource::Listeners, Action::Read), (Resource::RouteConfigs, Action::Read), (Resource::RouteConfigs, Action::Update), (Resource::Clusters, Action::Read), (Resource::Clusters, Action::Delete)]).await;
        let (status, body) = f.request(&token, "DELETE", &format!("expose/{name}"), None, None).await;
        assert_eq!(status, StatusCode::OK, "route-only branch needs no infrastructure Delete grants: {body}");
        assert_eq!(body["cluster_disposition"], "deleted");
        assert_eq!(body["route_config_disposition"], "retained");
        assert_eq!(body["listener_disposition"], "retained");
        let retained = f.admin_request("GET", &path, None, None, StatusCode::OK).await;
        let mut expected = changed["spec"].clone();
        expected["virtual_hosts"][0]["routes"].as_array_mut().unwrap().remove(0);
        assert_eq!(retained["spec"], expected, "unrelated route and policy must be preserved exactly");
        assert_eq!(retained["id"], created["route_config"]["id"]);
        assert_eq!(retained["revision"].as_i64().unwrap(), changed["revision"].as_i64().unwrap() + 1);
        f.inventories(&[("clusters", &unrelated, &other_cluster), ("route-configs", &format!("{name}-routes"), &retained), ("listeners", &name, &created["listener"])]).await;
        assert_eq!(f.snapshot().await["exposures"], json!([]));
    }).await;
}
