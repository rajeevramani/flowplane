//! MCP read-side services: the REST tool catalog (declared registry view with per-tool
//! executability annotation). Read-only — no mutations here.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::mcp_declarations::{
    dynamic_input_schema, dynamic_tool_description, dynamic_tool_name, DYNAMIC_TOOL_RISK,
    STATIC_TOOL_DECLS,
};
use crate::services::{deny_to_error, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{DomainResult, RequestId};
use serde_json::Value;
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCatalogKind {
    Static,
    Dynamic,
}

impl ToolCatalogKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Dynamic => "dynamic",
        }
    }
}

/// One catalog row: a DECLARATION of a tool, not MCP exposure. `executable_by_caller` is
/// authorization AND every serving gate the execution path enforces — a disabled or
/// non-published dynamic row is never executable regardless of grants.
#[derive(Debug, Clone)]
pub struct ToolCatalogRow {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub resource: &'static str,
    pub action: &'static str,
    pub risk: &'static str,
    pub kind: ToolCatalogKind,
    pub enabled: bool,
    pub executable_by_caller: bool,
}

/// The team's MCP tool catalog: every static registry declaration plus the team's dynamic
/// `api_*` tools for currently published spec versions. Catalog read requires
/// `(mcp-tools, read)`. `include_disabled` additionally requires `(mcp-tools, update)` —
/// the same pair gating tool enable/disable — and fails closed (no silent downgrade to the
/// enabled-only view). Per-tool executability is evaluated purely in-process against the
/// grants already on `ctx` (no per-tool DB round-trip); the dynamic part is exactly one
/// team-wide query, and a DB error fails the whole read (no partial catalog).
pub async fn tool_catalog(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    include_disabled: bool,
    request_id: RequestId,
) -> DomainResult<Vec<ToolCatalogRow>> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    if include_disabled {
        authorize(pool, ctx, Action::Update, team, request_id).await?;
    }

    let mut rows: Vec<ToolCatalogRow> = STATIC_TOOL_DECLS
        .iter()
        .map(|decl| static_row(decl, ctx, team))
        .collect();

    let execute_allowed = allowed(ctx, Resource::McpTools, Action::Execute, team);
    let api_tools = if include_disabled {
        fp_storage::repos::api_lifecycle::list_published_api_tools(pool, team.id).await?
    } else {
        fp_storage::repos::api_lifecycle::list_enabled_published_api_tools(pool, team.id).await?
    };
    rows.extend(
        api_tools
            .into_iter()
            .map(|tool| dynamic_row(tool, execute_allowed)),
    );
    Ok(rows)
}

fn static_row(
    decl: &'static crate::mcp_declarations::StaticToolDecl,
    ctx: &PrincipalCtx,
    team: TeamRef,
) -> ToolCatalogRow {
    ToolCatalogRow {
        name: decl.name.to_string(),
        description: decl.description.to_string(),
        input_schema: (decl.input_schema)(),
        resource: decl.resource.as_str(),
        action: decl.action.as_str(),
        risk: decl.risk.as_str(),
        kind: ToolCatalogKind::Static,
        // Static tools have no enable/disable lifecycle: declared = servable.
        enabled: true,
        executable_by_caller: allowed(ctx, decl.resource, decl.action, team),
    }
}

/// Executability composes authorization with the serving gates execution enforces. Callers
/// pass tools from a published-version-scoped query, so the remaining gate is `enabled`
/// (execution resolves enabled+published rows only): a disabled row is never executable.
fn dynamic_row(tool: fp_domain::api_lifecycle::ApiTool, execute_allowed: bool) -> ToolCatalogRow {
    ToolCatalogRow {
        name: dynamic_tool_name(&tool.name),
        description: dynamic_tool_description(tool.method.as_str(), &tool.path),
        input_schema: dynamic_input_schema(&tool.input_schema),
        resource: Resource::McpTools.as_str(),
        action: Action::Execute.as_str(),
        risk: DYNAMIC_TOOL_RISK,
        kind: ToolCatalogKind::Dynamic,
        enabled: tool.enabled,
        executable_by_caller: execute_allowed && tool.enabled,
    }
}

fn allowed(ctx: &PrincipalCtx, resource: Resource, action: Action, team: TeamRef) -> bool {
    matches!(
        check_resource_access(ctx, resource, action, Some(team)),
        Decision::Allow(_)
    )
}

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::McpTools, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::McpTools,
                action,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::McpTools, action, reason))
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::authz::GrantSet;
    use crate::mcp_declarations::STATIC_TOOL_DECLS;
    use fp_domain::api_lifecycle::{ApiTool, HttpMethod};
    use fp_domain::{ApiDefinitionId, ApiToolId, OrgId, OrgRole, SpecVersionId, TeamId, UserId};

    fn member_with(grants: Vec<(Resource, Action, TeamId)>, org_id: OrgId) -> PrincipalCtx {
        PrincipalCtx::User {
            user_id: UserId::generate(),
            platform_admin: false,
            org: Some((org_id, OrgRole::Member)),
            org_selector_required: false,
            grants: GrantSet::new(grants.into_iter().map(|(r, a, t)| (r, a, t, org_id))),
        }
    }

    fn api_tool(enabled: bool) -> ApiTool {
        ApiTool {
            id: ApiToolId::generate(),
            team_id: TeamId::generate(),
            api_definition_id: ApiDefinitionId::generate(),
            spec_version_id: SpecVersionId::generate(),
            name: "orders_list".into(),
            operation_id: "listOrders".into(),
            method: HttpMethod::Get,
            path: "/orders".into(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            enabled,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn static_executability_follows_each_tools_own_authz_pair() {
        let org = OrgId::generate();
        let team = TeamRef {
            id: TeamId::generate(),
            org_id: org,
        };
        let ctx = member_with(vec![(Resource::Clusters, Action::Read, team.id)], org);
        let list = STATIC_TOOL_DECLS
            .iter()
            .find(|d| d.name == "cp_clusters_list")
            .expect("decl");
        let create = STATIC_TOOL_DECLS
            .iter()
            .find(|d| d.name == "cp_clusters_create")
            .expect("decl");
        assert!(static_row(list, &ctx, team).executable_by_caller);
        assert!(!static_row(create, &ctx, team).executable_by_caller);
        // Static rows are always declared-servable.
        assert!(static_row(create, &ctx, team).enabled);
    }

    #[test]
    fn dynamic_executability_requires_authz_and_enabled_serving_gate() {
        // (execute_allowed, enabled) truth table: only (true, true) is executable.
        assert!(dynamic_row(api_tool(true), true).executable_by_caller);
        assert!(!dynamic_row(api_tool(false), true).executable_by_caller);
        assert!(!dynamic_row(api_tool(true), false).executable_by_caller);
        assert!(!dynamic_row(api_tool(false), false).executable_by_caller);
        // Disabled rows stay visible as disabled declarations.
        assert!(!dynamic_row(api_tool(false), true).enabled);
    }

    #[test]
    fn dynamic_rows_use_the_shared_wire_shape() {
        let row = dynamic_row(api_tool(true), true);
        assert_eq!(row.name, "api_orders_list");
        assert_eq!(row.description, "GET /orders");
        assert_eq!(row.kind, ToolCatalogKind::Dynamic);
        assert_eq!(row.resource, Resource::McpTools.as_str());
        assert_eq!(row.action, Action::Execute.as_str());
        assert_eq!(row.risk, "mutate");
        let required: Vec<_> = row.input_schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"team"));
    }
}
