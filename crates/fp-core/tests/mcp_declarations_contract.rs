//! Black-box contract tests for the public `fp_core::mcp_declarations` registry
//! (bead fpv2-zl8.1 acceptance criteria).
//!
//! These tests derive all expectations from the public API surface
//! (`STATIC_TOOL_DECLS`, `StaticToolDecl`, `ToolRisk`) — no counts or entry
//! lists are hardcoded except the three anchor tools named in the spec.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use fp_core::mcp_declarations::{StaticToolDecl, ToolRisk, STATIC_TOOL_DECLS};
use fp_domain::authz::{Action, Resource};
use serde_json::Value;

/// Look up an anchor tool by name, failing loudly if it is missing.
fn find(name: &str) -> &'static StaticToolDecl {
    STATIC_TOOL_DECLS
        .iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("anchor tool {name:?} missing from STATIC_TOOL_DECLS"))
}

#[test]
fn registry_is_non_empty() {
    assert!(
        !STATIC_TOOL_DECLS.is_empty(),
        "STATIC_TOOL_DECLS must contain the static MCP control-plane tool registry"
    );
}

#[test]
fn every_name_is_unique() {
    let mut seen: HashSet<&str> = HashSet::new();
    for decl in STATIC_TOOL_DECLS {
        assert!(
            seen.insert(decl.name),
            "duplicate tool name in STATIC_TOOL_DECLS: {:?}",
            decl.name
        );
    }
    assert_eq!(
        seen.len(),
        STATIC_TOOL_DECLS.len(),
        "unique-name count must equal registry length"
    );
}

#[test]
fn every_name_is_non_empty_with_cp_or_ops_prefix() {
    for decl in STATIC_TOOL_DECLS {
        assert!(!decl.name.is_empty(), "tool with empty name in registry");
        assert!(
            decl.name.starts_with("cp_") || decl.name.starts_with("ops_"),
            "tool name {:?} must start with \"cp_\" or \"ops_\"",
            decl.name
        );
        // A bare prefix ("cp_" / "ops_") is not a real name.
        assert!(
            decl.name != "cp_" && decl.name != "ops_",
            "tool name {:?} is only a prefix",
            decl.name
        );
    }
}

#[test]
fn every_description_is_non_empty() {
    for decl in STATIC_TOOL_DECLS {
        assert!(
            !decl.description.trim().is_empty(),
            "tool {:?} has an empty description",
            decl.name
        );
    }
}

#[test]
fn every_input_schema_is_a_closed_object_requiring_team() {
    for decl in STATIC_TOOL_DECLS {
        let schema: Value = (decl.input_schema)();
        let obj = schema.as_object().unwrap_or_else(|| {
            panic!(
                "tool {:?}: input_schema() must return a JSON object",
                decl.name
            )
        });

        // "type": "object"
        assert_eq!(
            obj.get("type").and_then(Value::as_str),
            Some("object"),
            "tool {:?}: schema must declare \"type\": \"object\", got {:?}",
            decl.name,
            obj.get("type")
        );

        // Closed object: "additionalProperties": false
        assert_eq!(
            obj.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "tool {:?}: schema must set \"additionalProperties\": false, got {:?}",
            decl.name,
            obj.get("additionalProperties")
        );

        // "required" must be an array of strings containing "team".
        let required = obj
            .get("required")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "tool {:?}: schema must have a \"required\" array, got {:?}",
                    decl.name,
                    obj.get("required")
                )
            });
        for entry in required {
            assert!(
                entry.is_string(),
                "tool {:?}: non-string entry in \"required\": {entry:?}",
                decl.name
            );
        }
        assert!(
            required.iter().any(|v| v.as_str() == Some("team")),
            "tool {:?}: \"required\" must contain \"team\", got {required:?}",
            decl.name
        );

        // A closed schema with a required key that has no matching property is
        // unsatisfiable — every required key must be declared in "properties".
        let properties = obj
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or_else(|| {
                panic!(
                    "tool {:?}: schema must have a \"properties\" object, got {:?}",
                    decl.name,
                    obj.get("properties")
                )
            });
        assert!(
            properties.contains_key("team"),
            "tool {:?}: \"properties\" must declare \"team\"",
            decl.name
        );
        for key in required.iter().filter_map(Value::as_str) {
            assert!(
                properties.contains_key(key),
                "tool {:?}: required key {key:?} is absent from \"properties\" of a closed \
                 (additionalProperties: false) schema — unsatisfiable",
                decl.name
            );
        }
    }
}

#[test]
fn input_schema_is_deterministic_per_tool() {
    // The schema fn is the tools/list source of truth; it must not vary call-to-call.
    for decl in STATIC_TOOL_DECLS {
        assert_eq!(
            (decl.input_schema)(),
            (decl.input_schema)(),
            "tool {:?}: input_schema() must be deterministic",
            decl.name
        );
    }
}

#[test]
fn risk_maps_to_action_for_every_entry() {
    for decl in STATIC_TOOL_DECLS {
        match decl.risk {
            ToolRisk::Read => assert_eq!(
                decl.action,
                Action::Read,
                "tool {:?}: risk Read must pair with Action::Read, got {:?}",
                decl.name,
                decl.action
            ),
            ToolRisk::Mutate => assert!(
                matches!(decl.action, Action::Create | Action::Update),
                "tool {:?}: risk Mutate must pair with Action::Create or Action::Update, got {:?}",
                decl.name,
                decl.action
            ),
            ToolRisk::Delete => assert_eq!(
                decl.action,
                Action::Delete,
                "tool {:?}: risk Delete must pair with Action::Delete, got {:?}",
                decl.name,
                decl.action
            ),
        }
    }
}

#[test]
fn action_maps_back_to_risk_for_every_entry() {
    // Reverse direction of the mapping: the action alone determines the risk tier.
    for decl in STATIC_TOOL_DECLS {
        let expected = match decl.action {
            Action::Read => ToolRisk::Read,
            Action::Create | Action::Update => ToolRisk::Mutate,
            Action::Delete => ToolRisk::Delete,
            other => panic!(
                "tool {:?}: action {other:?} has no risk tier under the fpv2-zl8.1 contract \
                 (Read↔Read, Mutate↔Create/Update, Delete↔Delete)",
                decl.name
            ),
        };
        assert_eq!(
            decl.risk, expected,
            "tool {:?}: action {:?} must carry risk {:?}, got {:?}",
            decl.name, decl.action, expected, decl.risk
        );
    }
}

#[test]
fn anchor_cp_clusters_list_has_exact_authz_metadata() {
    let decl = find("cp_clusters_list");
    assert_eq!(decl.resource, Resource::Clusters);
    assert_eq!(decl.action, Action::Read);
    assert_eq!(decl.risk, ToolRisk::Read);
    assert_eq!(decl.risk.as_str(), "read");
}

#[test]
fn anchor_cp_secrets_get_has_exact_authz_metadata() {
    let decl = find("cp_secrets_get");
    assert_eq!(decl.resource, Resource::Secrets);
    assert_eq!(decl.action, Action::Read);
}

#[test]
fn anchor_cp_ai_usage_has_exact_authz_metadata() {
    let decl = find("cp_ai_usage");
    assert_eq!(decl.resource, Resource::AiUsage);
    assert_eq!(decl.action, Action::Read);
}

#[test]
fn tool_risk_as_str_returns_spec_wire_strings() {
    assert_eq!(ToolRisk::Read.as_str(), "read");
    assert_eq!(ToolRisk::Mutate.as_str(), "mutate");
    assert_eq!(ToolRisk::Delete.as_str(), "delete");
}
