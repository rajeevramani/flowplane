//! API lifecycle services (S8.2): API CRUD/status plus config-first OpenAPI import.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::api_lifecycle::{
    ApiDefinition, ApiDefinitionSpec, ApiRouteBindingSpec, ApiToolSpec, HttpMethod, SpecFormat,
    SpecReviewDecision, SpecSourceKind, SpecVersion, SpecVersionInput,
};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::identity::NAME_MAX_LEN;
use fp_domain::{DomainError, DomainResult, ErrorCode, RequestId};
use fp_storage::repos::{api_lifecycle, audit};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct CreateApiInput {
    pub name: String,
    pub definition: ApiDefinitionSpec,
    pub imported_spec: Option<serde_json::Value>,
    pub route_binding_name: Option<String>,
    pub route_binding: Option<ApiRouteBindingSpec>,
}

#[derive(Debug, Clone)]
pub struct ApiStatus {
    pub api: ApiDefinition,
    pub latest_spec: Option<SpecVersion>,
    pub tool_count: i64,
    pub route_binding_count: i64,
}

#[derive(Debug, Clone)]
pub struct SpecReviewInput {
    pub api: String,
    pub version: i64,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PublishSpecResult {
    pub spec: SpecVersion,
    pub tool_count: i64,
}

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::ApiDefinitions, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::ApiDefinitions,
                action,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::ApiDefinitions, action, reason))
        }
    }
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    resource: String,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource,
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

fn actor_type_name(ctx: &PrincipalCtx) -> &'static str {
    match ctx {
        PrincipalCtx::User { .. } => "user",
        PrincipalCtx::Agent { .. } => "agent",
    }
}

pub async fn create_api(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: CreateApiInput,
    request_id: RequestId,
) -> DomainResult<ApiStatus> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::ApiDefinitions)
        .await?;
    let tools = input
        .imported_spec
        .as_ref()
        .map(|spec| tools_from_openapi(&input.name, spec))
        .transpose()?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create api: begin"))?;
    let api =
        api_lifecycle::create_api_definition(&mut tx, team, &input.name, &input.definition).await?;
    if let Some(binding) = &input.route_binding {
        let binding_name = input
            .route_binding_name
            .as_deref()
            .unwrap_or(input.name.as_str());
        api_lifecycle::create_route_binding(&mut tx, team, api.id, binding_name, binding).await?;
    }

    let mut latest_spec = None;
    let mut tool_count = 0_i64;
    if let Some(spec) = input.imported_spec {
        let spec_version = api_lifecycle::create_spec_version(
            &mut tx,
            team,
            api.id,
            &SpecVersionInput {
                source_kind: SpecSourceKind::Imported,
                format: SpecFormat::OpenApi3,
                spec,
            },
        )
        .await?;
        fp_storage::outbox::append(
            &mut tx,
            &DomainEvent::SpecVersionCreated {
                spec_version_id: spec_version.id.as_uuid(),
                api_definition_id: api.id.as_uuid(),
                version: spec_version.version,
            },
            EventScope {
                org_id: Some(team.org_id),
                team_id: Some(team.id),
            },
            trace_context_json(),
        )
        .await?;
        if let Some(tools) = tools {
            for tool in tools {
                api_lifecycle::create_api_tool(
                    &mut tx,
                    team,
                    api.id,
                    spec_version.id,
                    &tool.name,
                    &tool.spec,
                )
                .await?;
                tool_count += 1;
            }
        }
        if tool_count > 0 {
            fp_storage::outbox::append(
                &mut tx,
                &DomainEvent::ApiToolsGenerated {
                    api_definition_id: api.id.as_uuid(),
                    spec_version_id: spec_version.id.as_uuid(),
                    count: tool_count as usize,
                },
                EventScope {
                    org_id: Some(team.org_id),
                    team_id: Some(team.id),
                },
                trace_context_json(),
            )
            .await?;
        }
        latest_spec = Some(spec_version);
    }

    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ApiDefinitionCreated {
            api_definition_id: api.id.as_uuid(),
            name: api.name.clone(),
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
            "api.create",
            format!("apis/{}", api.name),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create api: commit"))?;

    Ok(ApiStatus {
        api,
        latest_spec,
        tool_count,
        route_binding_count: if input.route_binding.is_some() { 1 } else { 0 },
    })
}

pub async fn get_api(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<ApiDefinition> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    api_lifecycle::get_api_definition(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("api", name))
}

pub async fn list_apis(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<ApiDefinition>, i64)> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    api_lifecycle::list_api_definitions(pool, team.id, limit, offset).await
}

pub async fn api_status(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<ApiStatus> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    let api = api_lifecycle::get_api_definition(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("api", name))?;
    status_for_api(pool, team, api).await
}

pub async fn delete_api(
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
        .map_err(crate::services::db_err("delete api: begin"))?;
    let api_id =
        api_lifecycle::delete_api_definition(&mut tx, team.id, name, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ApiDefinitionDeleted {
            api_definition_id: api_id.as_uuid(),
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
        &mutation_audit(ctx, request_id, team, "api.delete", format!("apis/{name}")),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete api: commit"))?;
    Ok(())
}

pub async fn reject_spec_version(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: SpecReviewInput,
    request_id: RequestId,
) -> DomainResult<SpecVersion> {
    authorize(pool, ctx, Action::Update, team, request_id).await?;
    let api = api_lifecycle::get_api_definition(pool, team.id, &input.api)
        .await?
        .ok_or_else(|| DomainError::not_found("api", &input.api))?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("reject spec version: begin"))?;
    let spec =
        api_lifecycle::get_spec_version_for_api_by_version(&mut tx, team.id, api.id, input.version)
            .await?;
    if spec.source_kind != SpecSourceKind::Learned {
        return Err(DomainError::validation(
            "only learned spec versions can be rejected",
        ));
    }
    if api.published_spec_version_id == Some(spec.id) {
        return Err(DomainError::conflict(
            "published spec versions cannot be rejected",
        ));
    }
    let (_, actor_id) = actor_of(ctx);
    api_lifecycle::append_spec_review_event(
        &mut tx,
        team,
        api_lifecycle::SpecReviewEventInsert {
            api_id: api.id,
            spec_version_id: spec.id,
            decision: SpecReviewDecision::Rejected,
            actor_type: actor_type_name(ctx),
            actor_id,
            reason: &input.reason,
            metadata: serde_json::json!({}),
        },
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "api.spec.reject",
            format!("apis/{}/specs/{}", api.name, spec.version),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("reject spec version: commit"))?;
    Ok(spec)
}

pub async fn publish_spec_version(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: SpecReviewInput,
    request_id: RequestId,
) -> DomainResult<PublishSpecResult> {
    authorize(pool, ctx, Action::Update, team, request_id).await?;
    let api = api_lifecycle::get_api_definition(pool, team.id, &input.api)
        .await?
        .ok_or_else(|| DomainError::not_found("api", &input.api))?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("publish spec version: begin"))?;
    let spec =
        api_lifecycle::get_spec_version_for_api_by_version(&mut tx, team.id, api.id, input.version)
            .await?;
    if spec.source_kind != SpecSourceKind::Learned {
        return Err(DomainError::validation(
            "only learned spec versions can be published through the review loop",
        ));
    }
    if api_lifecycle::latest_spec_review_decision(&mut tx, team.id, spec.id).await?
        == Some(SpecReviewDecision::Rejected)
    {
        return Err(DomainError::conflict(
            "rejected spec versions cannot be published",
        ));
    }
    let tools = tools_from_openapi(&api.name, &spec.spec)?;
    api_lifecycle::set_published_spec_version(&mut tx, team.id, api.id, spec.id).await?;
    api_lifecycle::delete_api_tools_for_api(&mut tx, team.id, api.id).await?;
    for tool in &tools {
        api_lifecycle::create_api_tool(&mut tx, team, api.id, spec.id, &tool.name, &tool.spec)
            .await?;
    }
    let (_, actor_id) = actor_of(ctx);
    api_lifecycle::append_spec_review_event(
        &mut tx,
        team,
        api_lifecycle::SpecReviewEventInsert {
            api_id: api.id,
            spec_version_id: spec.id,
            decision: SpecReviewDecision::Published,
            actor_type: actor_type_name(ctx),
            actor_id,
            reason: &input.reason,
            metadata: serde_json::json!({ "tool_count": tools.len() }),
        },
    )
    .await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ApiToolsGenerated {
            api_definition_id: api.id.as_uuid(),
            spec_version_id: spec.id.as_uuid(),
            count: tools.len(),
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
            "api.spec.publish",
            format!("apis/{}/specs/{}", api.name, spec.version),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("publish spec version: commit"))?;
    Ok(PublishSpecResult {
        spec,
        tool_count: tools.len() as i64,
    })
}

async fn status_for_api(
    pool: &PgPool,
    team: TeamRef,
    api: ApiDefinition,
) -> DomainResult<ApiStatus> {
    let latest_spec = api_lifecycle::latest_spec_version(pool, team.id, api.id).await?;
    let tool_count = api_lifecycle::count_api_tools(pool, team.id, api.id).await?;
    let route_binding_count = api_lifecycle::count_route_bindings(pool, team.id, api.id).await?;
    Ok(ApiStatus {
        api,
        latest_spec,
        tool_count,
        route_binding_count,
    })
}

#[derive(Debug)]
struct GeneratedTool {
    name: String,
    spec: ApiToolSpec,
}

fn tools_from_openapi(
    api_name: &str,
    spec: &serde_json::Value,
) -> DomainResult<Vec<GeneratedTool>> {
    let object = spec
        .as_object()
        .ok_or_else(|| DomainError::validation("OpenAPI document must be a JSON object"))?;
    if !object.contains_key("openapi") {
        return Err(DomainError::validation(
            "OpenAPI document must include an openapi version",
        ));
    }
    let paths = object
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| DomainError::validation("OpenAPI document must include a paths object"))?;
    let mut tools = Vec::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (method, operation) in item {
            let Some(method) = parse_openapi_method(method) else {
                continue;
            };
            let operation_id = operation
                .as_object()
                .and_then(|o| o.get("operationId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| fallback_operation_id(method, path));
            let name = generated_tool_name(api_name, &operation_id);
            let tool = ApiToolSpec {
                operation_id,
                method,
                path: path.clone(),
                input_schema: serde_json::json!({}),
                output_schema: serde_json::json!({}),
                enabled: true,
            };
            tool.validate()?;
            tools.push(GeneratedTool { name, spec: tool });
        }
    }
    if tools.is_empty() {
        return Err(DomainError::new(
            ErrorCode::ValidationFailed,
            "OpenAPI document does not contain any HTTP operations",
        )
        .with_hint("include at least one path item with get/post/put/patch/delete/options/head"));
    }
    Ok(tools)
}

fn parse_openapi_method(method: &str) -> Option<HttpMethod> {
    match method {
        "get" => Some(HttpMethod::Get),
        "post" => Some(HttpMethod::Post),
        "put" => Some(HttpMethod::Put),
        "patch" => Some(HttpMethod::Patch),
        "delete" => Some(HttpMethod::Delete),
        "options" => Some(HttpMethod::Options),
        "head" => Some(HttpMethod::Head),
        _ => None,
    }
}

fn fallback_operation_id(method: HttpMethod, path: &str) -> String {
    let suffix = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim_matches(|c| c == '{' || c == '}')
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if suffix.is_empty() {
        method.as_str().to_ascii_lowercase()
    } else {
        format!("{}-{suffix}", method.as_str().to_ascii_lowercase())
    }
}

fn normalize_name_part(value: &str) -> String {
    let mut out = String::new();
    let mut prev_hyphen = false;
    for c in value.chars().flat_map(char::to_lowercase) {
        let mapped = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            Some(c)
        } else if c == '-' || c == '_' || c.is_whitespace() {
            Some('-')
        } else {
            None
        };
        if let Some(c) = mapped {
            if c == '-' {
                if !out.is_empty() && !prev_hyphen {
                    out.push('-');
                    prev_hyphen = true;
                }
            } else {
                out.push(c);
                prev_hyphen = false;
            }
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "operation".into()
    } else {
        trimmed.chars().take(60).collect()
    }
}

fn generated_tool_name(api_name: &str, operation_id: &str) -> String {
    let suffix = normalize_name_part(operation_id);
    let mut name = format!("{api_name}-{suffix}");
    if name.len() > NAME_MAX_LEN {
        name.truncate(NAME_MAX_LEN);
        while name.ends_with('-') {
            name.pop();
        }
    }
    name
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn openapi_operations_generate_stable_tools() {
        let tools = tools_from_openapi(
            "catalog",
            &serde_json::json!({
                "openapi": "3.0.3",
                "info": {"title": "Catalog", "version": "1"},
                "paths": {
                    "/items": {"get": {"operationId": "listItems"}},
                    "/items/{id}": {"post": {}}
                }
            }),
        )
        .expect("tools");
        let names = tools.into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(names, vec!["catalog-listitems", "catalog-post-items-id"]);
    }

    #[test]
    fn openapi_without_operations_is_rejected() {
        let err = tools_from_openapi(
            "empty",
            &serde_json::json!({"openapi": "3.0.3", "paths": {}}),
        )
        .expect_err("must reject");
        assert_eq!(err.code, ErrorCode::ValidationFailed);
    }
}
