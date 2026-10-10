//! Independent S2 exposure scope contracts: approved design AC4/AC7, constitution 6/18.
//! Production handlers/services/repository bodies were not inspected. Only approved
//! contracts, existing integration fixtures and SQL schema informed this target.
//! Real router + RS256 + PostgreSQL; no S3 listener selector or mocks.
//! Unset FLOWPLANE_TEST_DATABASE_URL means a visible skip. Otherwise runtime creates
//! aidf_s2_scope_<uuid>; role needs CREATEDB and DROP ownership. Setup and
//! assertions run in a joined task so their panics still drop the owned database.
//! Compilation provisions nothing. The parent owns runtime execution/DB permission.
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
            .unwrap_or_else(|_| panic!("invalid PostgreSQL URL (credentials redacted)"));
        let control = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap_or_else(|_| panic!("cannot connect to PostgreSQL (credentials redacted)"));
        let name = format!("aidf_s2_scope_{}", Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&control)
            .await
            .expect("runtime role needs CREATEDB");
        Some(Self {
            control,
            options: options.database(&name),
            name,
        })
    }
    async fn cleanup(self) {
        sqlx::query(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
            .execute(&self.control)
            .await
            .expect("drop only owned scope database");
        self.control.close().await;
    }
}

#[derive(Clone)]
struct Scope {
    org: OrgId,
    org_name: String,
    team: TeamId,
    team_name: String,
    socket: Arc<TcpListener>,
}
impl Scope {
    fn body(&self, name: &str) -> Value {
        json!({"name":name, "upstream":"http://127.0.0.1:3001", "path":"/scope",
            "port":self.socket.local_addr().unwrap().port(),
            "public_base_url":"https://scope.example"})
    }
    fn team_uuid(&self) -> String {
        self.team.as_uuid().to_string()
    }
    fn org_uuid(&self) -> String {
        self.org.as_uuid().to_string()
    }
}

struct Fixture {
    app: Router,
    pool: PgPool,
    issuer: DevIssuer,
}
#[derive(Clone, Copy)]
enum ExpectedDenial {
    Forbidden,
    OutsideOrg,
    OrgSelectorRequired,
}

struct Reply {
    status: StatusCode,
    body: Value,
    request_id: Uuid,
}
impl Reply {
    fn expect(self, expected: StatusCode) -> Value {
        assert_eq!(self.status, expected, "router contract: {}", self.body);
        self.body
    }
    fn denied(self, expected: ExpectedDenial) {
        // Exact status/code pairs, never diagnostic/message matching. Selector
        // failure is distinct from authorization; 404 requires an outside-org target.
        let code = match (expected, self.status) {
            (ExpectedDenial::OrgSelectorRequired, StatusCode::BAD_REQUEST) => {
                "org_selector_required"
            }
            (ExpectedDenial::Forbidden | ExpectedDenial::OutsideOrg, StatusCode::FORBIDDEN) => {
                "forbidden"
            }
            (ExpectedDenial::OutsideOrg, StatusCode::NOT_FOUND) => "not_found",
            (_, status) => panic!("expected scope denial, got {status}: {}", self.body),
        };
        assert!(self.body.is_object());
        assert_eq!(self.body["code"], code);
        assert!(self.body["message"].is_string());
        // REST error contract declares hint optional (docs/reference/rest-api.md).
        if let Some(hint) = self.body.get("hint") {
            assert!(hint.is_string());
        }
        assert_eq!(self.body["request_id"], self.request_id.to_string());
        for key in ["items", "cluster", "route_config", "listener", "spec"] {
            assert!(
                self.body.get(key).is_none(),
                "denial must not carry product data"
            );
        }
    }
}
impl Fixture {
    async fn new(options: PgConnectOptions) -> Self {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .unwrap_or_else(|_| panic!("cannot connect to owned database (credentials redacted)"));
        fp_storage::migrate(&pool).await.expect("normal migrations");
        let issuer = DevIssuer::generate().expect("RS256 issuer");
        let validator = fp_core::OidcValidator::new(issuer.oidc_config());
        validator
            .load_jwks_json(issuer.jwks_json())
            .await
            .expect("real JWKS");
        let app = fp_api::build_router(fp_api::AppState {
            pool: pool.clone(),
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            version: "s2-independent-scope",
            validator: Some(Arc::new(validator)),
            write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1_000_000)),
            xds_readiness: None,
            xds_degraded: None,
            discovery_forwarding_policy: Default::default(),
            egress_advisory: Default::default(),
            rls_repush: None,
            rls_grpc_configured: false,
        });
        Self { app, pool, issuer }
    }
    async fn scope(&self, existing_org: Option<&Scope>, team_name: &str) -> Scope {
        let (org, org_name) = if let Some(scope) = existing_org {
            (scope.org, scope.org_name.clone())
        } else {
            let org = identity::create_org(&self.pool, &unique("org"), "")
                .await
                .expect("org");
            (org.id, org.name)
        };
        let team = identity::create_team(&self.pool, org, team_name, "")
            .await
            .expect("team");
        Scope {
            org,
            org_name,
            team: team.id,
            team_name: team.name,
            socket: Arc::new(TcpListener::bind("127.0.0.1:0").expect("unique explicit port")),
        }
    }
    async fn principal(&self, scopes: &[&Scope], role: OrgRole, grant_gateway: bool) -> String {
        let subject = unique("scope-user");
        let user =
            identity::upsert_user_by_subject(&self.pool, &subject, "scope@test", "Scope contract")
                .await
                .expect("user");
        let mut orgs = Vec::new();
        for scope in scopes {
            if !orgs.contains(&scope.org) {
                identity::add_org_membership(&self.pool, user, scope.org, role)
                    .await
                    .expect("org membership");
                orgs.push(scope.org);
            }
            if grant_gateway {
                // Only team-specific gateway grants. No grant-management, org,
                // platform-admin or blanket grant may accidentally authorize B.
                for resource in [
                    Resource::Clusters,
                    Resource::RouteConfigs,
                    Resource::Listeners,
                ] {
                    for action in [Action::Create, Action::Read, Action::Delete] {
                        identity::add_grant(
                            &self.pool, user, scope.org, scope.team, resource, action, None,
                        )
                        .await
                        .expect("team-specific gateway grant");
                    }
                }
            }
        }
        self.issuer
            .mint(&subject, "scope@test", "Scope contract", 3600)
            .expect("RS256 token")
    }
    async fn request(
        &self,
        token: &str,
        scope: (Option<&str>, &str),
        endpoint: (&str, &str),
        body: Option<Value>,
        revision: Option<i64>,
    ) -> Reply {
        let (selector, team) = scope;
        let (method, suffix) = endpoint;
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/api/v1/teams/{team}/{suffix}"))
            .header("authorization", format!("Bearer {token}"));
        if let Some(selector) = selector {
            builder = builder.header("x-flowplane-org", selector);
        }
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
                .expect("bounded router request")
                .expect("real router");
        let status = response.status();
        let request_id = Uuid::parse_str(
            response
                .headers()
                .get("x-request-id")
                .expect("request ID header")
                .to_str()
                .unwrap(),
        )
        .expect("request UUID");
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
        Reply {
            status,
            body,
            request_id,
        }
    }
    async fn expose(&self, token: &str, scope: &Scope, name: &str) -> Value {
        let created = self
            .request(
                token,
                (Some(&scope.org_name), &scope.team_name),
                ("POST", "expose"),
                Some(scope.body(name)),
                None,
            )
            .await
            .expect(StatusCode::CREATED);
        assert_eq!(created["mode"], "created");
        self.association(scope, name, &created).await;
        created
    }
    async fn association(&self, scope: &Scope, name: &str, created: &Value) {
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM exposures e WHERE team_id=$1 AND name=$2")
                .bind(scope.team.as_uuid())
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .expect("owned association");
        assert_eq!(row["org_id"], scope.org_uuid());
        assert_eq!(row["team_id"], scope.team_uuid());
        for (field, key) in [
            ("cluster_id", "cluster"),
            ("route_config_id", "route_config"),
            ("listener_id", "listener"),
        ] {
            assert_eq!(row[field], created[key]["id"]);
        }
    }
    async fn visible(&self, token: &str, scope: &Scope, created: &Value) {
        for (kind, key) in [
            ("clusters", "cluster"),
            ("route-configs", "route_config"),
            ("listeners", "listener"),
        ] {
            let suffix = format!("{kind}/{}", created[key]["name"].as_str().unwrap());
            let view = self
                .request(
                    token,
                    (Some(&scope.org_name), &scope.team_name),
                    ("GET", &suffix),
                    None,
                    None,
                )
                .await
                .expect(StatusCode::OK);
            assert_eq!(
                view, created[key],
                "selected scope resolves exact ID/revision/spec"
            );
            let by_team_uuid = self
                .request(
                    token,
                    (Some(&scope.org_name), &scope.team_uuid()),
                    ("GET", &suffix),
                    None,
                    None,
                )
                .await
                .expect(StatusCode::OK);
            assert_eq!(by_team_uuid, created[key], "own team UUID positive control");
            let list = self
                .request(
                    token,
                    (Some(&scope.org_name), &scope.team_name),
                    ("GET", kind),
                    None,
                    None,
                )
                .await
                .expect(StatusCode::OK);
            let expected = vec![created[key].clone()];
            assert_eq!(list["items"], json!(expected));
            assert_eq!(
                list["total"],
                json!(expected.len()),
                "fixture-derived inventory total"
            );
        }
    }
    async fn remove(&self, token: &str, scope: &Scope, name: &str) {
        let removed = self
            .request(
                token,
                (Some(&scope.org_name), &scope.team_name),
                ("DELETE", &format!("expose/{name}")),
                None,
                None,
            )
            .await
            .expect(StatusCode::OK);
        for key in [
            "cluster_disposition",
            "route_config_disposition",
            "listener_disposition",
        ] {
            assert_eq!(removed[key], "deleted");
        }
        for suffix in resource_paths(name) {
            let reply = self
                .request(
                    token,
                    (Some(&scope.org_name), &scope.team_name),
                    ("GET", &suffix),
                    None,
                    None,
                )
                .await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND);
            assert_eq!(reply.body["code"], "not_found");
        }
        let row: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM exposures WHERE team_id=$1 AND name=$2")
                .bind(scope.team.as_uuid())
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .expect("association absence");
        assert!(row.is_none());
    }
    // Snapshot complete persisted graph and effects, including normalized refs,
    // full timestamps/versions and null-org/team effects. This database belongs
    // ONLY to this case, so full-database equality is parallel-safe. Denial audits
    // are legitimate; every success audit (including an erroneous expose.* one)
    // and every outbox row must remain exactly equal on denial.
    async fn snapshot(&self, team: Option<TeamId>) -> Value {
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
            let mut predicates = Vec::new();
            if team.is_some() {
                predicates.push("team_id=$1");
            }
            if table == "audit_log" {
                predicates.push("outcome='success'");
            }
            let filter = if predicates.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", predicates.join(" AND "))
            };
            let query =
                format!("SELECT to_jsonb(t) FROM {table} t{filter} ORDER BY to_jsonb(t)::text");
            let rows: Vec<Value> = if let Some(team) = team {
                sqlx::query_scalar(&query)
                    .bind(team.as_uuid())
                    .fetch_all(&self.pool)
                    .await
            } else {
                sqlx::query_scalar(&query).fetch_all(&self.pool).await
            }
            .expect("exact graph/success-effects snapshot");
            map.insert(table.into(), json!(rows));
        }
        Value::Object(map)
    }
    async fn reject_resource_creates(
        &self,
        token: &str,
        selector: Option<&str>,
        team: &str,
        created: &Value,
        expected: ExpectedDenial,
    ) {
        // Copy accepted gateway specs from the real successful scaffold. A fresh
        // port prevents a uniqueness conflict from disguising authorization.
        let socket = TcpListener::bind("127.0.0.1:0").expect("denied create port");
        for (kind, key) in [
            ("clusters", "cluster"),
            ("route-configs", "route_config"),
            ("listeners", "listener"),
        ] {
            let mut spec = created[key]["spec"].clone();
            if kind == "listeners" {
                spec["port"] = json!(socket.local_addr().unwrap().port());
            }
            self.reject_unchanged(
                token,
                (selector, team),
                ("POST", kind),
                Some(json!({"name": unique("denied-resource"), "spec": spec})),
                None,
                expected,
            )
            .await;
        }
    }
    async fn reject_unchanged(
        &self,
        token: &str,
        scope: (Option<&str>, &str),
        endpoint: (&str, &str),
        body: Option<Value>,
        revision: Option<i64>,
        expected: ExpectedDenial,
    ) {
        let (selector, team) = scope;
        let (method, suffix) = endpoint;
        let before = self.snapshot(None).await;
        let reply = self
            .request(token, (selector, team), (method, suffix), body, revision)
            .await;
        eprintln!(
            "scope probe: selector={selector:?} team={team} {method} {suffix} -> {} code={}",
            reply.status, reply.body["code"]
        );
        reply.denied(expected);
        assert_eq!(self.snapshot(None).await,before,"{method} {suffix}: denial preserves exact graph, success audit and outbox across ALL scopes");
    }
}
fn resource_paths(name: &str) -> Vec<String> {
    vec![
        format!("clusters/{name}-upstream"),
        format!("route-configs/{name}-routes"),
        format!("listeners/{name}"),
    ]
}
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
        test(Fixture::new(options).await).await;
    })
    .await;
    scratch.cleanup().await;
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("scope contract task cancelled");
    }
}

#[tokio::test]
async fn identical_exposure_names_in_different_teams_never_collide_or_cross_delete() {
    // Both removal directions matter: catches lookup/cleanup choosing the first
    // matching name instead of the explicit team. Different explicit ports keep
    // the assertion about identity, not port allocation (which is not S3).
    for first in [0, 1] {
        case(move |f| async move {
            let a = f.scope(None, &unique("team-a")).await;
            let b = f.scope(Some(&a), &unique("team-b")).await;
            let token = f.principal(&[&a], OrgRole::Admin, false).await;
            let name = unique("same");
            let left = f.expose(&token, &a, &name).await;
            let a_before = f.snapshot(Some(a.team)).await;
            let right = f.expose(&token, &b, &name).await;
            assert_eq!(
                f.snapshot(Some(a.team)).await,
                a_before,
                "B create never rewrites A"
            );
            for key in ["cluster", "route_config", "listener"] {
                assert_ne!(left[key]["id"], right[key]["id"]);
            }
            f.visible(&token, &a, &left).await;
            f.visible(&token, &b, &right).await;
            let (removed, survivor, view) = if first == 0 {
                (&a, &b, &right)
            } else {
                (&b, &a, &left)
            };
            let survivor_before = f.snapshot(Some(survivor.team)).await;
            f.remove(&token, removed, &name).await;
            assert_eq!(
                f.snapshot(Some(survivor.team)).await,
                survivor_before,
                "other team's complete graph/effects unchanged"
            );
            f.visible(&token, survivor, view).await;
            f.association(survivor, &name, view).await;
            f.remove(&token, survivor, &name).await;
        })
        .await;
    }
}

#[tokio::test]
async fn team_a_gateway_grants_cannot_create_read_or_delete_team_b_exposure_resources() {
    case(|f| async move {
        let a = f.scope(None, &unique("team-a")).await;
        let b = f.scope(Some(&a), &unique("team-b")).await;
        let admin = f.principal(&[&a], OrgRole::Admin, false).await;
        let member = f.principal(&[&a], OrgRole::Member, true).await;
        let name = unique("scope");
        let own = f.expose(&member, &a, &name).await; // non-vacuous Create control
        f.visible(&member, &a, &own).await; // exact Read/list controls
        let foreign = f.expose(&admin, &b, &name).await;
        f.visible(&admin, &b, &foreign).await; // B exists and is same-org visible
                                               // Same-org B is not genuinely invisible; ONLY 403 is acceptable, by
                                               // both supported team address forms. No GET expose endpoint is invented.
        for address in [&b.team_name, b.team_uuid().as_str()] {
            f.reject_resource_creates(
                &member,
                Some(&a.org_name),
                address,
                &foreign,
                ExpectedDenial::Forbidden,
            )
            .await;
            f.reject_unchanged(
                &member,
                (Some(&a.org_name), address),
                ("POST", "expose"),
                Some(b.body(&unique("denied"))),
                None,
                ExpectedDenial::Forbidden,
            )
            .await;
            f.reject_unchanged(
                &member,
                (Some(&a.org_name), address),
                ("DELETE", &format!("expose/{name}")),
                None,
                None,
                ExpectedDenial::Forbidden,
            )
            .await;
            for (kind, key) in [
                ("clusters", "cluster"),
                ("route-configs", "route_config"),
                ("listeners", "listener"),
            ] {
                let path = format!("{kind}/{}", foreign[key]["name"].as_str().unwrap());
                f.reject_unchanged(
                    &member,
                    (Some(&a.org_name), address),
                    ("GET", kind),
                    None,
                    None,
                    ExpectedDenial::Forbidden,
                )
                .await;
                f.reject_unchanged(
                    &member,
                    (Some(&a.org_name), address),
                    ("GET", &path),
                    None,
                    None,
                    ExpectedDenial::Forbidden,
                )
                .await;
                f.reject_unchanged(
                    &member,
                    (Some(&a.org_name), address),
                    ("DELETE", &path),
                    None,
                    Some(foreign[key]["revision"].as_i64().unwrap()),
                    ExpectedDenial::Forbidden,
                )
                .await;
            }
        }
        f.visible(&member, &a, &own).await;
        f.remove(&member, &a, &name).await; // exact Read/Delete control after denials
        f.visible(&admin, &b, &foreign).await;
    })
    .await;
}

#[tokio::test]
async fn multi_org_member_selects_only_authorized_org_for_identical_team_and_exposure_names() {
    case(|f| async move {
        let team_name = unique("same-team");
        let a = f.scope(None, &team_name).await;
        let b = f.scope(None, &team_name).await;
        let member = f.principal(&[&a, &b], OrgRole::Member, true).await;
        let name = unique("same-exposure");
        let left = f.expose(&member, &a, &name).await;
        let a_before = f.snapshot(Some(a.team)).await;
        let right = f.expose(&member, &b, &name).await;
        assert_eq!(f.snapshot(Some(a.team)).await, a_before);
        for key in ["cluster", "route_config", "listener"] {
            assert_ne!(left[key]["id"], right[key]["id"]);
        }
        for (scope, created) in [(&a, &left), (&b, &right)] {
            f.visible(&member, scope, created).await;
            // Name and UUID org selectors must select the same exact graph.
            for path in resource_paths(&name) {
                let by_name = f
                    .request(
                        &member,
                        (Some(&scope.org_name), &team_name),
                        ("GET", &path),
                        None,
                        None,
                    )
                    .await
                    .expect(StatusCode::OK);
                let by_uuid = f
                    .request(
                        &member,
                        (Some(&scope.org_uuid()), &team_name),
                        ("GET", &path),
                        None,
                        None,
                    )
                    .await
                    .expect(StatusCode::OK);
                assert_eq!(by_uuid, by_name);
            }
        }
        let b_before = f.snapshot(Some(b.team)).await;
        f.remove(&member, &a, &name).await;
        assert_eq!(
            f.snapshot(Some(b.team)).await,
            b_before,
            "chosen A cannot erase same names in B"
        );
        f.visible(&member, &b, &right).await;
        // Exercise selected-org UUID for mutation, not only GET.
        let a_after_removal = f.snapshot(Some(a.team)).await;
        let removed = f
            .request(
                &member,
                (Some(&b.org_uuid()), &team_name),
                ("DELETE", &format!("expose/{name}")),
                None,
                None,
            )
            .await
            .expect(StatusCode::OK);
        for key in [
            "cluster_disposition",
            "route_config_disposition",
            "listener_disposition",
        ] {
            assert_eq!(removed[key], "deleted");
        }
        assert_eq!(
            f.snapshot(Some(a.team)).await,
            a_after_removal,
            "B UUID cleanup preserves A effects"
        );
        let graph = f.snapshot(Some(b.team)).await;
        for table in [
            "clusters",
            "route_configs",
            "listeners",
            "exposures",
            "route_config_cluster_refs",
            "listener_route_config_refs",
        ] {
            assert_eq!(graph[table], json!([]), "B cleanup {table}");
        }
    })
    .await;
}

#[tokio::test]
async fn nonmember_org_header_and_foreign_team_uuid_never_escape_scope() {
    case(|f| async move {
        let team_name = unique("same-team");
        let a = f.scope(None, &team_name).await;
        let b = f.scope(None, &team_name).await;
        let member = f.principal(&[&a], OrgRole::Member, true).await;
        let b_admin = f.principal(&[&b], OrgRole::Admin, false).await;
        let name = unique("same-exposure");
        let own = f.expose(&member, &a, &name).await;
        let foreign = f.expose(&b_admin, &b, &name).await;
        f.visible(&member, &a, &own).await;
        f.visible(&b_admin, &b, &foreign).await;
        // REST D-014: exactly one tenant membership is inferred without a header.
        // Prove the baseline before testing a foreign UUID under implicit selection.
        for (kind, key) in [
            ("clusters", "cluster"),
            ("route-configs", "route_config"),
            ("listeners", "listener"),
        ] {
            let path = format!("{kind}/{}", own[key]["name"].as_str().unwrap());
            for address in [&a.team_name, a.team_uuid().as_str()] {
                let view = f
                    .request(&member, (None, address), ("GET", &path), None, None)
                    .await
                    .expect(StatusCode::OK);
                assert_eq!(view, own[key], "implicit own-org positive control");
            }
        }
        let b_uuid = b.team_uuid();
        // Foreign selector by both forms, paired with same-name team AND foreign
        // UUID; foreign UUID under own/inferred org cannot bypass active scope.
        // REST selector policy + errors.md and agent_read_scope's team-name
        // convention: names need a resolved active org (400/org_selector_required).
        // UUID attacks retain strict 403/404; never admit selector errors globally.
        let b_org_uuid = b.org_uuid();
        let attempts = [
            (
                Some(b.org_name.as_str()),
                team_name.as_str(),
                ExpectedDenial::OrgSelectorRequired,
            ),
            (
                Some(b_org_uuid.as_str()),
                team_name.as_str(),
                ExpectedDenial::OrgSelectorRequired,
            ),
            (
                Some(b.org_name.as_str()),
                b_uuid.as_str(),
                ExpectedDenial::OutsideOrg,
            ),
            (
                Some(b_org_uuid.as_str()),
                b_uuid.as_str(),
                ExpectedDenial::OutsideOrg,
            ),
            (
                Some(a.org_name.as_str()),
                b_uuid.as_str(),
                ExpectedDenial::OutsideOrg,
            ),
            (None, b_uuid.as_str(), ExpectedDenial::OutsideOrg),
        ];
        for (selector, address, expected) in attempts {
            f.reject_resource_creates(&member, selector, address, &foreign, expected)
                .await;
            f.reject_unchanged(
                &member,
                (selector, address),
                ("POST", "expose"),
                Some(b.body(&unique("foreign-denied"))),
                None,
                expected,
            )
            .await;
            f.reject_unchanged(
                &member,
                (selector, address),
                ("DELETE", &format!("expose/{name}")),
                None,
                None,
                expected,
            )
            .await;
            for (kind, key) in [
                ("clusters", "cluster"),
                ("route-configs", "route_config"),
                ("listeners", "listener"),
            ] {
                let path = format!("{kind}/{}", foreign[key]["name"].as_str().unwrap());
                f.reject_unchanged(
                    &member,
                    (selector, address),
                    ("GET", kind),
                    None,
                    None,
                    expected,
                )
                .await;
                f.reject_unchanged(
                    &member,
                    (selector, address),
                    ("GET", &path),
                    None,
                    None,
                    expected,
                )
                .await;
                f.reject_unchanged(
                    &member,
                    (selector, address),
                    ("DELETE", &path),
                    None,
                    Some(foreign[key]["revision"].as_i64().unwrap()),
                    expected,
                )
                .await;
            }
        }
        f.visible(&member, &a, &own).await;
        let foreign_before = f.snapshot(Some(b.team)).await;
        f.remove(&member, &a, &name).await;
        assert_eq!(f.snapshot(Some(b.team)).await, foreign_before);
        f.visible(&b_admin, &b, &foreign).await;
    })
    .await;
}
