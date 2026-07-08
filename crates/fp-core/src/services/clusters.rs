//! Cluster service: the ONLY mutation path (spec/10 §2). Every mutation is one
//! transaction containing the row change, its domain event (outbox), and its audit entry —
//! they commit together or not at all.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::egress_policy::{EgressPolicy, EgressValidation};
use crate::services::filesystem_path_policy::FilesystemPathPolicy;
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::gateway::cluster::{
    validate_cluster_name, Cluster, ClusterSpec, Endpoint, UpstreamTlsConfig,
};
use fp_domain::{DomainResult, RequestId};
use fp_storage::repos::{audit, clusters};
use fp_storage::scope::TeamScope;
use sqlx::PgPool;
use std::net::IpAddr;

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::Clusters, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::Clusters,
                action,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::Clusters, action, reason))
        }
    }
}

pub async fn create_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    request_id: RequestId,
) -> DomainResult<Cluster> {
    let policy = EgressPolicy::from_process_config().await;
    let file_policy = FilesystemPathPolicy::from_process_config()?;
    create_cluster_with_policies(
        pool,
        ctx,
        team,
        name,
        spec,
        request_id,
        &policy,
        &file_policy,
    )
    .await
}

pub async fn create_cluster_with_egress_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    request_id: RequestId,
    policy: &EgressPolicy,
) -> DomainResult<Cluster> {
    let file_policy = FilesystemPathPolicy::from_process_config()?;
    create_cluster_with_policies(
        pool,
        ctx,
        team,
        name,
        spec,
        request_id,
        policy,
        &file_policy,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_cluster_with_policies(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    request_id: RequestId,
    policy: &EgressPolicy,
    file_policy: &FilesystemPathPolicy,
) -> DomainResult<Cluster> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    validate_cluster_name(name)?;
    spec.validate()?;
    let (spec, egress) = materialize_pinned_cluster_spec(spec, policy).await?;
    validate_cluster_filesystem_paths(&spec, file_policy)?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::Clusters).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create cluster: begin"))?;
    let cluster = clusters::create(&mut tx, team, name, &spec).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ClusterUpserted {
            cluster_id: cluster.id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "cluster.create", name, &egress),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create cluster: commit"))?;
    Ok(cluster)
}

pub async fn get_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<Cluster> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    clusters::get(pool, TeamScope::Team(team.id), name)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("cluster", name))
}

pub async fn list_clusters(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<Cluster>, i64)> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    clusters::list(pool, TeamScope::Team(team.id), limit, offset).await
}

pub async fn update_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<Cluster> {
    let policy = EgressPolicy::from_process_config().await;
    let file_policy = FilesystemPathPolicy::from_process_config()?;
    update_cluster_with_policies(
        pool,
        ctx,
        team,
        name,
        spec,
        expected_version,
        request_id,
        &policy,
        &file_policy,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_cluster_with_egress_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    expected_version: i64,
    request_id: RequestId,
    policy: &EgressPolicy,
) -> DomainResult<Cluster> {
    let file_policy = FilesystemPathPolicy::from_process_config()?;
    update_cluster_with_policies(
        pool,
        ctx,
        team,
        name,
        spec,
        expected_version,
        request_id,
        policy,
        &file_policy,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_cluster_with_policies(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    expected_version: i64,
    request_id: RequestId,
    policy: &EgressPolicy,
    file_policy: &FilesystemPathPolicy,
) -> DomainResult<Cluster> {
    authorize(pool, ctx, Action::Update, team, request_id).await?;
    spec.validate()?;
    let (spec, egress) = materialize_pinned_cluster_spec(spec, policy).await?;
    validate_cluster_filesystem_paths(&spec, file_policy)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update cluster: begin"))?;
    let cluster = clusters::update(&mut tx, team.id, name, &spec, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ClusterUpserted {
            cluster_id: cluster.id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "cluster.update", name, &egress),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update cluster: commit"))?;
    Ok(cluster)
}

fn validate_cluster_filesystem_paths(
    spec: &ClusterSpec,
    policy: &FilesystemPathPolicy,
) -> DomainResult<()> {
    if let Some(UpstreamTlsConfig {
        ca_cert_file: Some(path),
        ..
    }) = &spec.upstream_tls
    {
        policy.validate_path("upstream_tls.ca_cert_file", path)?;
    }
    Ok(())
}

pub(crate) async fn materialize_pinned_cluster_spec(
    mut spec: ClusterSpec,
    policy: &EgressPolicy,
) -> DomainResult<(ClusterSpec, EgressValidation)> {
    let mut validation = EgressValidation::default();
    let mut pinned = Vec::new();
    let should_preserve_tls_hostname = spec.use_tls || spec.upstream_tls.is_some();
    let authored_endpoints = spec.endpoints.clone();
    for endpoint in &authored_endpoints {
        let endpoint_validation = policy
            .validate_host_port(&endpoint.host, endpoint.port, "cluster endpoint")
            .await?;
        preserve_tls_hostname_intent(&mut spec, &endpoint.host, should_preserve_tls_hostname);
        if validation.allowlist_match.is_none() {
            validation.allowlist_match = endpoint_validation.allowlist_match;
        }
        validation
            .resolved_ips
            .extend(endpoint_validation.resolved_ips.iter().copied());
        pinned.extend(
            endpoint_validation
                .resolved_ips
                .into_iter()
                .map(|ip| Endpoint {
                    host: ip.to_string(),
                    port: endpoint.port,
                    weight: endpoint.weight,
                }),
        );
    }
    validation.resolved_ips.sort();
    validation.resolved_ips.dedup();
    spec.endpoints = pinned;
    Ok((spec, validation))
}

#[cfg(test)]
async fn validate_cluster_egress(
    spec: &ClusterSpec,
    policy: &EgressPolicy,
) -> DomainResult<EgressValidation> {
    materialize_pinned_cluster_spec(spec.clone(), policy)
        .await
        .map(|(_, validation)| validation)
}

fn preserve_tls_hostname_intent(spec: &mut ClusterSpec, host: &str, enabled: bool) {
    if !enabled || host.parse::<IpAddr>().is_ok() {
        return;
    }
    match &mut spec.upstream_tls {
        Some(tls) => {
            if tls.sni.is_none() {
                tls.sni = Some(host.to_string());
                tls.auto_sni_san_validation = true;
            }
        }
        None => {
            spec.upstream_tls = Some(UpstreamTlsConfig {
                sni: Some(host.to_string()),
                validation_context_sds_secret_name: None,
                ca_cert_file: None,
                auto_sni_san_validation: true,
                insecure_skip_verify: false,
            });
        }
    }
}

pub async fn delete_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(pool, ctx, Action::Delete, team, request_id).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete cluster: begin"))?;
    // Referenced clusters cannot be deleted (no silent cascade — spec/10 §3.4.1); the
    // error lists dependents so the operator knows exactly what to unwind.
    let dependents =
        fp_storage::repos::gateway::route_configs_referencing_cluster(&mut tx, team.id, name)
            .await?;
    if !dependents.is_empty() {
        return Err(fp_domain::DomainError::conflict(format!(
            "cluster \"{name}\" is referenced by route configs: {}",
            dependents.join(", ")
        ))
        .with_hint("update or delete those route configs first"));
    }
    let cluster_id = clusters::delete(&mut tx, team.id, name, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ClusterDeleted {
            cluster_id: cluster_id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "cluster.delete",
            name,
            &EgressValidation::default(),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete cluster: commit"))?;
    Ok(())
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    name: &str,
    egress: &EgressValidation,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource: format!("clusters/{name}"),
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: egress.audit_detail(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_domain::gateway::cluster::{Endpoint, LbPolicy};

    fn spec(host: &str, port: u16) -> ClusterSpec {
        ClusterSpec {
            aggregate_clusters: Vec::new(),
            endpoints: vec![Endpoint {
                host: host.into(),
                port,
                weight: None,
            }],
            lb_policy: LbPolicy::RoundRobin,
            least_request: None,
            ring_hash: None,
            maglev: None,
            dns_lookup_family: None,
            connect_timeout_secs: 5,
            use_tls: false,
            upstream_tls: None,
            protocol: None,
            health_checks: None,
            circuit_breakers: None,
            outlier_detection: None,
        }
    }

    #[tokio::test]
    async fn cluster_egress_validation_rejects_denied_endpoint_before_storage_inputs() {
        validate_cluster_egress(&spec("10.0.0.10", 8080), &EgressPolicy::default())
            .await
            .expect_err("private cluster endpoint denied");
    }

    #[tokio::test]
    async fn materialized_cluster_pins_all_validated_ips_deterministically() {
        let policy = EgressPolicy::with_static_hosts(
            Vec::new(),
            Vec::new(),
            vec![(
                "api.example.test".into(),
                443,
                vec![
                    "203.0.113.20".parse().unwrap(),
                    "203.0.113.10".parse().unwrap(),
                ],
            )],
        );
        let mut spec = spec("api.example.test", 443);
        spec.use_tls = true;

        let (pinned, validation) = materialize_pinned_cluster_spec(spec, &policy)
            .await
            .expect("pin cluster");

        assert_eq!(
            pinned
                .endpoints
                .iter()
                .map(|endpoint| endpoint.host.as_str())
                .collect::<Vec<_>>(),
            vec!["203.0.113.10", "203.0.113.20"]
        );
        assert_eq!(
            validation
                .resolved_ips
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["203.0.113.10", "203.0.113.20"]
        );
        let tls = pinned.upstream_tls.expect("hostname TLS intent preserved");
        assert_eq!(tls.sni.as_deref(), Some("api.example.test"));
        assert!(tls.auto_sni_san_validation);
    }

    #[tokio::test]
    async fn materialized_cluster_keeps_explicit_sni() {
        let policy = EgressPolicy::with_static_hosts(
            Vec::new(),
            Vec::new(),
            vec![(
                "api.example.test".into(),
                443,
                vec!["203.0.113.10".parse().unwrap()],
            )],
        );
        let mut spec = spec("api.example.test", 443);
        spec.upstream_tls = Some(UpstreamTlsConfig {
            sni: Some("origin.example.test".into()),
            validation_context_sds_secret_name: None,
            ca_cert_file: None,
            auto_sni_san_validation: false,
            insecure_skip_verify: false,
        });

        let (pinned, _) = materialize_pinned_cluster_spec(spec, &policy)
            .await
            .expect("pin cluster");

        let tls = pinned.upstream_tls.expect("upstream tls");
        assert_eq!(tls.sni.as_deref(), Some("origin.example.test"));
        assert!(!tls.auto_sni_san_validation);
    }

    #[test]
    fn cluster_filesystem_validation_rejects_ca_file_by_default() {
        let mut spec = spec("api.example.com", 443);
        spec.use_tls = true;
        spec.upstream_tls = Some(UpstreamTlsConfig {
            sni: Some("api.example.com".into()),
            validation_context_sds_secret_name: None,
            ca_cert_file: Some("/etc/tenant-ca.pem".into()),
            auto_sni_san_validation: false,
            insecure_skip_verify: false,
        });
        validate_cluster_filesystem_paths(&spec, &FilesystemPathPolicy::disabled())
            .expect_err("tenant CA file rejected by default");
    }

    #[test]
    fn cluster_filesystem_validation_accepts_sds_reference_by_default() {
        let mut spec = spec("api.example.com", 443);
        spec.use_tls = true;
        spec.upstream_tls = Some(UpstreamTlsConfig {
            sni: Some("api.example.com".into()),
            validation_context_sds_secret_name: Some("tenant-ca".into()),
            ca_cert_file: None,
            auto_sni_san_validation: true,
            insecure_skip_verify: false,
        });
        validate_cluster_filesystem_paths(&spec, &FilesystemPathPolicy::disabled())
            .expect("SDS reference accepted");
    }
}
