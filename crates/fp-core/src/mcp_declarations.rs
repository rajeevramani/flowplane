//! Shared MCP static-tool declarations: the descriptor model (name, description, input
//! schema, authz resource/action, risk) for every `cp_*`/`ops_*` control-plane tool.
//!
//! This is the single registry both MCP `tools/list` serving (fp-api) and the REST tool
//! catalog consume, so a listed tool and its enforced authz metadata cannot drift apart.
//! Execution dispatch stays in fp-api: each declaration is bound to its executor there by
//! name, and a bijection test in fp-api pins declaration↔executor completeness.

use fp_domain::authz::{Action, Resource};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRisk {
    Read,
    Mutate,
    Delete,
}

impl ToolRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mutate => "mutate",
            Self::Delete => "delete",
        }
    }
}

/// Declaration of one static control-plane MCP tool. This is metadata only — no executor:
/// dispatch is bound by name in fp-api.
#[derive(Clone, Copy, Debug)]
pub struct StaticToolDecl {
    pub name: &'static str,
    pub description: &'static str,
    pub resource: Resource,
    pub action: Action,
    pub risk: ToolRisk,
    pub input_schema: fn() -> Value,
}

pub const STATIC_TOOL_DECLS: &[StaticToolDecl] = &[
    StaticToolDecl {
        name: "cp_clusters_list",
        description: "List upstream clusters for one team.",
        resource: Resource::Clusters,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_clusters_get",
        description: "Read one upstream cluster by name.",
        resource: Resource::Clusters,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_clusters_create",
        description: "Create an upstream cluster.",
        resource: Resource::Clusters,
        action: Action::Create,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec,
    },
    StaticToolDecl {
        name: "cp_clusters_update",
        description: "Update an upstream cluster using an expected revision.",
        resource: Resource::Clusters,
        action: Action::Update,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec_revision,
    },
    StaticToolDecl {
        name: "cp_clusters_delete",
        description: "Delete an upstream cluster using an expected revision.",
        resource: Resource::Clusters,
        action: Action::Delete,
        risk: ToolRisk::Delete,
        input_schema: schema_named_revision,
    },
    StaticToolDecl {
        name: "cp_route_configs_list",
        description: "List route configs for one team.",
        resource: Resource::RouteConfigs,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_route_configs_get",
        description: "Read one route config by name.",
        resource: Resource::RouteConfigs,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_route_configs_create",
        description: "Create a route config.",
        resource: Resource::RouteConfigs,
        action: Action::Create,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec,
    },
    StaticToolDecl {
        name: "cp_route_configs_update",
        description: "Update a route config using an expected revision.",
        resource: Resource::RouteConfigs,
        action: Action::Update,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec_revision,
    },
    StaticToolDecl {
        name: "cp_route_configs_delete",
        description: "Delete a route config using an expected revision.",
        resource: Resource::RouteConfigs,
        action: Action::Delete,
        risk: ToolRisk::Delete,
        input_schema: schema_named_revision,
    },
    StaticToolDecl {
        name: "cp_listeners_list",
        description: "List listeners for one team.",
        resource: Resource::Listeners,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_listeners_get",
        description: "Read one listener by name.",
        resource: Resource::Listeners,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_listeners_create",
        description: "Create a listener.",
        resource: Resource::Listeners,
        action: Action::Create,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec,
    },
    StaticToolDecl {
        name: "cp_listeners_update",
        description: "Update a listener using an expected revision.",
        resource: Resource::Listeners,
        action: Action::Update,
        risk: ToolRisk::Mutate,
        input_schema: schema_named_spec_revision,
    },
    StaticToolDecl {
        name: "cp_listeners_delete",
        description: "Delete a listener using an expected revision.",
        resource: Resource::Listeners,
        action: Action::Delete,
        risk: ToolRisk::Delete,
        input_schema: schema_named_revision,
    },
    StaticToolDecl {
        name: "cp_apis_list",
        description: "List API definitions for one team.",
        resource: Resource::ApiDefinitions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_apis_get",
        description: "Read one API definition by name.",
        resource: Resource::ApiDefinitions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_apis_status",
        description: "Read publish/spec/tool status for one API definition.",
        resource: Resource::ApiDefinitions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_learning_sessions_list",
        description: "List learning capture sessions for one team.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_learning_sessions_get",
        description: "Read one learning capture session by name or UUID.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_discovery_sessions_list",
        description: "List passive discovery sessions for one team.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_discovery_sessions_get",
        description: "Read one passive discovery session by name or UUID.",
        resource: Resource::LearningSessions,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "ops_xds_status",
        description: "Summarize xDS dataplane and recent NACK status for one team.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_team,
    },
    StaticToolDecl {
        name: "ops_xds_nacks",
        description: "List recent xDS NACK events for one team.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "ops_xds_trace",
        description: "Trace audit/outbox rows by request id, trace id, or path fragment.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_trace,
    },
    StaticToolDecl {
        name: "ops_stats_overview",
        description: "Summarize dataplane request/error telemetry for one team.",
        resource: Resource::Stats,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_team,
    },
    StaticToolDecl {
        name: "cp_secrets_list",
        description: "List secret metadata for one team. Secret values are never returned.",
        resource: Resource::Secrets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_secrets_get",
        description: "Read one secret metadata record. Secret values are never returned.",
        resource: Resource::Secrets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_ai_providers_list",
        description: "List AI providers for one team.",
        resource: Resource::AiProviders,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_ai_providers_get",
        description: "Read one AI provider by name.",
        resource: Resource::AiProviders,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_ai_routes_list",
        description: "List AI routes for one team.",
        resource: Resource::AiRoutes,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_ai_routes_get",
        description: "Read one AI route by name.",
        resource: Resource::AiRoutes,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_ai_budgets_list",
        description: "List AI budgets for one team.",
        resource: Resource::AiBudgets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_list,
    },
    StaticToolDecl {
        name: "cp_ai_budgets_get",
        description: "Read one AI budget by name.",
        resource: Resource::AiBudgets,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_named,
    },
    StaticToolDecl {
        name: "cp_ai_usage",
        description: "Read AI usage summary rows for one team, optionally windowed by \
                      RFC 3339 since/until (half-open [since, until); same semantics as \
                      the REST endpoint).",
        resource: Resource::AiUsage,
        action: Action::Read,
        risk: ToolRisk::Read,
        input_schema: schema_usage,
    },
];

/// Wire name of a dynamic (generated) tool: the `api_` prefix distinguishes generated
/// `api_tools` rows from static `cp_*`/`ops_*` declarations in every listing surface.
pub fn dynamic_tool_name(tool_name: &str) -> String {
    format!("api_{tool_name}")
}

/// Wire description of a dynamic tool ("METHOD /path"), shared by MCP `tools/list` and the
/// REST catalog so the two views cannot drift.
pub fn dynamic_tool_description(method: &str, path: &str) -> String {
    format!("{method} {path}")
}

/// Risk tier every dynamic tool advertises: execution proxies an arbitrary HTTP operation.
pub const DYNAMIC_TOOL_RISK: &str = "mutate";

/// Input schema a dynamic tool advertises: the stored per-operation schema augmented with
/// the invocation envelope (`team` required; `pathParams`/`query`/`headers`/`body` present).
pub fn dynamic_input_schema(schema: &Value) -> Value {
    let mut schema = schema.as_object().cloned().unwrap_or_default();
    schema.insert("type".into(), json!("object"));
    let mut properties = schema
        .remove("properties")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    properties.insert(
        "team".into(),
        json!({ "type": "string", "description": "Team name or UUID" }),
    );
    properties
        .entry("pathParams")
        .or_insert_with(|| json!({ "type": "object" }));
    properties
        .entry("query")
        .or_insert_with(|| json!({ "type": "object" }));
    properties
        .entry("headers")
        .or_insert_with(|| json!({ "type": "object" }));
    properties.entry("body").or_insert_with(|| json!({}));
    schema.insert("properties".into(), Value::Object(properties));
    let mut required = schema
        .remove("required")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if !required.iter().any(|v| v.as_str() == Some("team")) {
        required.push(json!("team"));
    }
    schema.insert("required".into(), Value::Array(required));
    Value::Object(schema)
}

fn schema_team() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" }
        },
        "required": ["team"],
        "additionalProperties": false
    })
}

fn schema_list() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 },
            "offset": { "type": "integer", "minimum": 0, "default": 0 }
        },
        "required": ["team"],
        "additionalProperties": false
    })
}

fn schema_usage() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "since": {
                "type": "string",
                "format": "date-time",
                "description": "RFC 3339 inclusive lower bound of the half-open usage \
                                window [since, until). Omitted = all-time."
            },
            "until": {
                "type": "string",
                "format": "date-time",
                "description": "RFC 3339 exclusive upper bound; omitted = now. With \
                                `since` present the span is capped at 92 days."
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 },
            "offset": { "type": "integer", "minimum": 0, "default": 0 }
        },
        "required": ["team"],
        "additionalProperties": false
    })
}

fn schema_named() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" }
        },
        "required": ["team", "name"],
        "additionalProperties": false
    })
}

fn schema_named_spec() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" },
            "spec": { "type": "object" }
        },
        "required": ["team", "name", "spec"],
        "additionalProperties": false
    })
}

fn schema_named_revision() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" },
            "revision": { "type": "integer" }
        },
        "required": ["team", "name", "revision"],
        "additionalProperties": false
    })
}

fn schema_named_spec_revision() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "name": { "type": "string" },
            "spec": { "type": "object" },
            "revision": { "type": "integer" }
        },
        "required": ["team", "name", "spec", "revision"],
        "additionalProperties": false
    })
}

fn schema_trace() -> Value {
    json!({
        "type": "object",
        "properties": {
            "team": { "type": "string", "description": "Team name or UUID" },
            "requestId": { "type": "string" },
            "traceId": { "type": "string" },
            "path": { "type": "string" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
        },
        "required": ["team"],
        "additionalProperties": false
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn declarations_have_unique_names_descriptions_and_consistent_risk() {
        let mut names = HashSet::new();
        for tool in STATIC_TOOL_DECLS {
            assert!(names.insert(tool.name), "duplicate tool {}", tool.name);
            assert!(
                !tool.description.is_empty(),
                "missing description: {}",
                tool.name
            );
            assert!(matches!(
                (tool.risk, tool.action),
                (ToolRisk::Read, Action::Read)
                    | (ToolRisk::Mutate, Action::Create | Action::Update)
                    | (ToolRisk::Delete, Action::Delete)
            ));
        }
    }

    #[test]
    fn every_input_schema_is_a_closed_object_requiring_team() {
        for tool in STATIC_TOOL_DECLS {
            let schema = (tool.input_schema)();
            assert_eq!(schema["type"], "object", "{}", tool.name);
            assert_eq!(schema["additionalProperties"], false, "{}", tool.name);
            let required: Vec<_> = schema["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{} missing required[]", tool.name))
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            assert!(
                required.contains(&"team"),
                "{} must require team",
                tool.name
            );
        }
    }
}
