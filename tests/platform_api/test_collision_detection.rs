use flowplane::platform_api::validation::{ensure_domain_available, ensure_route_available};
use flowplane::storage::{
    ApiDefinitionRepository, CreateApiDefinitionRequest, CreateApiRouteRequest,
};

use super::support::setup_platform_api_app;

#[tokio::test]
async fn detecting_domain_collision_for_different_team() {
    let app = setup_platform_api_app().await;
    let repo = ApiDefinitionRepository::new(app.pool.clone());

    let created = repo
        .create_definition(CreateApiDefinitionRequest {
            team: "payments".into(),
            domain: "payments.flowplane.dev".into(),
            listener_isolation: false,
            target_listeners: None,
            tls_config: None,
            metadata: None,
        })
        .await
        .expect("create definition");

    let collision = ensure_domain_available(&repo, "billing", "payments.flowplane.dev").await;
    assert!(collision.is_err(), "collision should be detected across teams");

    // same team should also raise collision
    let same_team = ensure_domain_available(&repo, "payments", "payments.flowplane.dev").await;
    assert!(same_team.is_err(), "collision for same team should be blocked");

    // non-conflicting domain should pass
    ensure_domain_available(&repo, "payments", "other.flowplane.dev")
        .await
        .expect("unique domain allowed");

    // route collision detection setup
    repo.create_route(CreateApiRouteRequest {
        api_definition_id: created.id.clone(),
        match_type: "prefix".into(),
        match_value: "/v1/".into(),
        case_sensitive: true,
        rewrite_prefix: None,
        rewrite_regex: None,
        rewrite_substitution: None,
        upstream_targets: serde_json::json!({
            "targets": [
                { "name": "payments-backend", "endpoint": "payments.svc.cluster.local:8443" }
            ]
        }),
        timeout_seconds: Some(3),
        override_config: None,
        deployment_note: None,
        route_order: 0,
    })
    .await
    .expect("seed route");

    let route_conflict = ensure_route_available(&repo, &created.id, "prefix", "/v1/").await;
    assert!(route_conflict.is_err(), "matching prefix should be rejected");

    ensure_route_available(&repo, &created.id, "prefix", "/v2/")
        .await
        .expect("distinct prefix should be accepted");
}
