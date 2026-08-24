#!/usr/bin/env python3
"""Independent, fail-closed fpv2-d23.8 AI/MCP acceptance gate.

Consumes only a sanitized ``flowplane.qualification.ai-mcp/v1`` JSON projection.
The projection is an allowlisted inventory of observations: it must not contain
credentials, provider payloads, raw identifiers, private paths, or private evidence.
``--self-test`` exercises synthetic positive evidence and adversarial mutations.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import re
import sys
from typing import Any, Callable, NoReturn

SCHEMA = "flowplane.qualification.ai-mcp/v1"
DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.8/ai-mcp.json")
TEAMS = ("alpha/payments", "alpha/shared", "beta/payments", "beta/shared")
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
DYNAMIC_API = re.compile(r"^api_", re.I)

APPROVED_INVENTORY = {
    "ai.secret-management": "supported",
    "ai.provider-management": "supported",
    "ai.route-management": "supported",
    "ai.budget-management": "supported",
    "ai.usage-management": "supported",
    "ai.trace-management": "supported",
    "ai.retention-management": "supported",
    "ai.envoy-nonstream": "supported",
    "ai.envoy-finite-stream": "supported",
    "ai.weighted-routing": "supported",
    "ai.priority-failover": "supported",
    "mcp.status-management": "supported",
    "mcp.connection-management": "supported",
    "mcp.catalog-management": "supported",
    "mcp.jsonrpc-initialize-tools-call": "supported-with-limitation/evolving",
    "mcp.dynamic-api-descriptor-execution": "not-mandatory-no-unconditional-support-claim",
}
MANAGEMENT_OPERATIONS = {
    "ai_secret": ["create", "read", "delete", "list"],
    "ai_provider": ["create", "read", "update", "delete", "list"],
    "ai_route": ["create", "read", "update", "delete", "list"],
    "ai_budget": ["create", "read", "update", "delete", "list"],
    "ai_usage": ["read", "list"],
    "ai_trace": ["read", "list"],
    "ai_retention": ["read"],
}
CLEANUP_INVENTORY = {
    "ai_secrets", "ai_providers", "ai_routes", "ai_budgets", "ai_usage_rows",
    "ai_traces", "listeners",
    "active_dataplanes", "certificates", "lima_vms", "tailscale_nodes", "tailscale_keys",
    "fly_machines", "fly_apps", "fly_volumes", "fly_provider_resources",
}
PROHIBITED_KEYS = {
    "secret", "secret_value", "plaintext", "token", "access_token", "refresh_token",
    "authorization", "api_key", "password", "credential", "private_key", "raw_body",
    "raw_payload", "raw_request", "raw_response", "raw_identifier", "provider_body",
    "connection_id", "session_id", "grant_id", "agent_id", "private_evidence_path",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~-]+"),
    re.compile(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b"),
    re.compile(r"(?i)(?:sk|tskey)-[A-Za-z0-9_-]{8,}"),
    re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\Users\\)"),
)


class ContractFailure(AssertionError):
    """The sanitized projection did not prove an acceptance invariant."""


def fail(message: str) -> NoReturn:
    raise ContractFailure(message)


def obj(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{name}: object required")
    return value


def seq(value: Any, name: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{name}: array required")
    return value


def text(value: Any, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        fail(f"{name}: non-empty text required")
    return value


def integer(value: Any, name: str, expected: int | None = None) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        fail(f"{name}: non-negative integer required")
    if expected is not None and value != expected:
        fail(f"{name}: expected {expected}, observed {value}")
    return value


def boolean(value: Any, name: str, expected: bool) -> None:
    if value is not expected:
        fail(f"{name}: expected {expected!r}, observed {value!r}")


def exact_keys(value: dict[str, Any], expected: set[str], name: str) -> None:
    if set(value) != expected:
        fail(f"{name}: exact fields required; got {sorted(value)!r}")


def field(root: dict[str, Any], dotted: str) -> Any:
    value: Any = root
    for part in dotted.split("."):
        value = obj(value, dotted).get(part)
        if value is None:
            fail(f"{dotted}: required")
    return value


def equal(root: dict[str, Any], dotted: str, expected: Any) -> None:
    actual = field(root, dotted)
    if actual != expected:
        fail(f"{dotted}: expected {expected!r}, observed {actual!r}")


def fingerprint(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not SHA256.fullmatch(candidate):
        fail(f"{name}: sha256 fingerprint required")
    return candidate


def timestamp(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not UTC.fullmatch(candidate):
        fail(f"{name}: second-precision UTC timestamp required")
    return candidate


def indexed(value: Any, name: str, key: str = "id") -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(seq(value, name)):
        item = obj(raw, f"{name}[{index}]")
        item_id = text(item.get(key), f"{name}[{index}].{key}")
        if item_id in result:
            fail(f"{name}: duplicate {key} {item_id!r}")
        result[item_id] = item
    return result


def team_index(value: Any, name: str) -> dict[str, dict[str, Any]]:
    rows = indexed(value, name, "team_key")
    if set(rows) != set(TEAMS):
        fail(f"{name}: exact four-team fixture required; got {sorted(rows)!r}")
    return rows


def check_run(e: dict[str, Any]) -> None:
    exact_keys(e, {"schema", "run", "inventory", "teams", "isolation", "mcp", "limitations", "cleanup", "redaction"}, "evidence root")
    equal(e, "schema", SCHEMA)
    run = obj(field(e, "run"), "run")
    exact_keys(run, {"synthetic_only", "rerunnable", "supported_surfaces_only", "direct_database_access", "independent_author_read_implementation", "sanitized_projection_contains_private_values", "started_at_utc", "finished_at_utc", "team_keys"}, "run")
    for key in ("synthetic_only", "rerunnable", "supported_surfaces_only"):
        boolean(run.get(key), f"run.{key}", True)
    for key in ("direct_database_access", "independent_author_read_implementation", "sanitized_projection_contains_private_values"):
        boolean(run.get(key), f"run.{key}", False)
    started = timestamp(run.get("started_at_utc"), "run.started_at_utc")
    finished = timestamp(run.get("finished_at_utc"), "run.finished_at_utc")
    if finished <= started:
        fail("run: finish must post-date start")
    if run.get("team_keys") != list(TEAMS):
        fail("run.team_keys: exact ordered four-team fixture required")


def check_inventory(e: dict[str, Any]) -> None:
    actual: dict[str, str] = {}
    for index, raw in enumerate(seq(field(e, "inventory"), "inventory")):
        row = obj(raw, f"inventory[{index}]")
        exact_keys(row, {"capability", "support_status"}, f"inventory[{index}]")
        capability = text(row.get("capability"), f"inventory[{index}].capability")
        if capability in actual:
            fail(f"inventory: duplicate capability {capability!r}")
        actual[capability] = text(row.get("support_status"), f"inventory[{index}].support_status")
    if actual != APPROVED_INVENTORY:
        missing = sorted(set(APPROVED_INVENTORY) - set(actual))
        extra = sorted(set(actual) - set(APPROVED_INVENTORY))
        wrong = sorted(k for k in set(actual) & set(APPROVED_INVENTORY) if actual[k] != APPROVED_INVENTORY[k])
        fail(f"inventory: exact approved inventory required; missing={missing!r} extra={extra!r} wrong={wrong!r}")


def check_management(team: dict[str, Any], name: str) -> None:
    rows = indexed(team.get("management"), f"{name}.management", "resource_kind")
    if set(rows) != set(MANAGEMENT_OPERATIONS):
        fail(f"{name}.management: exact AI management inventory required")
    for kind, operations in MANAGEMENT_OPERATIONS.items():
        row = rows[kind]
        exact_keys(row, {"resource_kind", "support_status", "observed_operations", "supported_api_only", "foreign_result_count"}, f"{name}.management[{kind}]")
        if row.get("support_status") != "supported" or row.get("observed_operations") != operations:
            fail(f"{name}.management[{kind}]: exact supported operations required")
        boolean(row.get("supported_api_only"), f"{name}.management[{kind}].supported_api_only", True)
        integer(row.get("foreign_result_count"), f"{name}.management[{kind}].foreign_result_count", 0)


def check_traffic(team: dict[str, Any], name: str) -> None:
    probes = obj(team.get("traffic"), f"{name}.traffic")
    exact_keys(probes, {"nonstream", "finite_stream", "malformed_200", "provider_500"}, f"{name}.traffic")
    request_refs: set[str] = set()

    nonstream = obj(probes.get("nonstream"), f"{name}.traffic.nonstream")
    exact_keys(nonstream, {"request_fingerprint", "through_envoy", "real_provider_request", "http_status", "response_marker_matched", "usage_record_count"}, f"{name}.traffic.nonstream")
    request_refs.add(fingerprint(nonstream.get("request_fingerprint"), f"{name}.traffic.nonstream.request_fingerprint"))
    for key in ("through_envoy", "real_provider_request", "response_marker_matched"):
        boolean(nonstream.get(key), f"{name}.traffic.nonstream.{key}", True)
    integer(nonstream.get("http_status"), f"{name}.traffic.nonstream.http_status", 200)
    integer(nonstream.get("usage_record_count"), f"{name}.traffic.nonstream.usage_record_count", 1)

    stream = obj(probes.get("finite_stream"), f"{name}.traffic.finite_stream")
    exact_keys(stream, {"request_fingerprint", "through_envoy", "real_provider_request", "http_status", "chunk_count", "terminal_event_observed", "bounded_completion", "usage_record_count"}, f"{name}.traffic.finite_stream")
    request_refs.add(fingerprint(stream.get("request_fingerprint"), f"{name}.traffic.finite_stream.request_fingerprint"))
    for key in ("through_envoy", "real_provider_request", "terminal_event_observed", "bounded_completion"):
        boolean(stream.get(key), f"{name}.traffic.finite_stream.{key}", True)
    integer(stream.get("http_status"), f"{name}.traffic.finite_stream.http_status", 200)
    if integer(stream.get("chunk_count"), f"{name}.traffic.finite_stream.chunk_count") < 2:
        fail(f"{name}.traffic.finite_stream.chunk_count: finite multi-chunk stream required")
    integer(stream.get("usage_record_count"), f"{name}.traffic.finite_stream.usage_record_count", 1)

    malformed = obj(probes.get("malformed_200"), f"{name}.traffic.malformed_200")
    exact_keys(malformed, {"request_fingerprint", "through_envoy", "provider_http_status", "downstream_http_status", "provider_payload_fingerprint", "downstream_payload_fingerprint", "response_passed_through", "usage_record_count"}, f"{name}.traffic.malformed_200")
    request_refs.add(fingerprint(malformed.get("request_fingerprint"), f"{name}.traffic.malformed_200.request_fingerprint"))
    boolean(malformed.get("through_envoy"), f"{name}.traffic.malformed_200.through_envoy", True)
    integer(malformed.get("provider_http_status"), f"{name}.traffic.malformed_200.provider_http_status", 200)
    integer(malformed.get("downstream_http_status"), f"{name}.traffic.malformed_200.downstream_http_status", 200)
    before = fingerprint(malformed.get("provider_payload_fingerprint"), f"{name}.traffic.malformed_200.provider_payload_fingerprint")
    after = fingerprint(malformed.get("downstream_payload_fingerprint"), f"{name}.traffic.malformed_200.downstream_payload_fingerprint")
    if before != after:
        fail(f"{name}.traffic.malformed_200: response was not passed through unchanged")
    boolean(malformed.get("response_passed_through"), f"{name}.traffic.malformed_200.response_passed_through", True)
    integer(malformed.get("usage_record_count"), f"{name}.traffic.malformed_200.usage_record_count", 0)

    error = obj(probes.get("provider_500"), f"{name}.traffic.provider_500")
    exact_keys(error, {"request_fingerprint", "through_envoy", "provider_http_status", "downstream_http_status", "trace_record_count", "trace_outcome", "trace_provider_status", "usage_record_count"}, f"{name}.traffic.provider_500")
    request_refs.add(fingerprint(error.get("request_fingerprint"), f"{name}.traffic.provider_500.request_fingerprint"))
    boolean(error.get("through_envoy"), f"{name}.traffic.provider_500.through_envoy", True)
    integer(error.get("provider_http_status"), f"{name}.traffic.provider_500.provider_http_status", 500)
    integer(error.get("downstream_http_status"), f"{name}.traffic.provider_500.downstream_http_status", 500)
    integer(error.get("trace_record_count"), f"{name}.traffic.provider_500.trace_record_count", 1)
    if error.get("trace_outcome") != "provider_failure":
        fail(f"{name}.traffic.provider_500.trace_outcome: provider_failure required")
    integer(error.get("trace_provider_status"), f"{name}.traffic.provider_500.trace_provider_status", 500)
    integer(error.get("usage_record_count"), f"{name}.traffic.provider_500.usage_record_count", 0)
    if len(request_refs) != 4:
        fail(f"{name}.traffic: request fingerprints must be unique")


def check_budget(team: dict[str, Any], name: str) -> None:
    budget = obj(team.get("budget"), f"{name}.budget")
    exact_keys(budget, {"shadow", "enforcing"}, f"{name}.budget")
    shadow = obj(budget.get("shadow"), f"{name}.budget.shadow")
    exact_keys(shadow, {"mode", "limit_reached", "http_status", "provider_request_count", "usage_record_count"}, f"{name}.budget.shadow")
    if shadow.get("mode") != "shadow":
        fail(f"{name}.budget.shadow.mode: shadow required")
    boolean(shadow.get("limit_reached"), f"{name}.budget.shadow.limit_reached", True)
    integer(shadow.get("http_status"), f"{name}.budget.shadow.http_status", 200)
    integer(shadow.get("provider_request_count"), f"{name}.budget.shadow.provider_request_count", 1)
    integer(shadow.get("usage_record_count"), f"{name}.budget.shadow.usage_record_count", 1)

    enforcing = obj(budget.get("enforcing"), f"{name}.budget.enforcing")
    exact_keys(enforcing, {"mode", "over_limit", "http_status", "provider_request_count", "usage_record_count", "denial_outcome"}, f"{name}.budget.enforcing")
    if enforcing.get("mode") != "enforcing":
        fail(f"{name}.budget.enforcing.mode: enforcing required")
    boolean(enforcing.get("over_limit"), f"{name}.budget.enforcing.over_limit", True)
    integer(enforcing.get("http_status"), f"{name}.budget.enforcing.http_status", 429)
    integer(enforcing.get("provider_request_count"), f"{name}.budget.enforcing.provider_request_count", 0)
    integer(enforcing.get("usage_record_count"), f"{name}.budget.enforcing.usage_record_count", 0)
    if enforcing.get("denial_outcome") != "budget_exceeded":
        fail(f"{name}.budget.enforcing.denial_outcome: budget_exceeded required")


def check_lifecycle(team: dict[str, Any], name: str) -> None:
    lifecycle = obj(team.get("lifecycle"), f"{name}.lifecycle")
    exact_keys(lifecycle, {"provider_update", "route_model_override", "stale_revision_failures", "referenced_delete_failures"}, f"{name}.lifecycle")
    update = obj(lifecycle.get("provider_update"), f"{name}.lifecycle.provider_update")
    exact_keys(update, {"old_provider_revision", "new_provider_revision", "old_route_revision", "new_route_revision", "dependent_route_bumped"}, f"{name}.lifecycle.provider_update")
    old_provider = integer(update.get("old_provider_revision"), f"{name}.lifecycle.provider_update.old_provider_revision")
    new_provider = integer(update.get("new_provider_revision"), f"{name}.lifecycle.provider_update.new_provider_revision")
    old_route = integer(update.get("old_route_revision"), f"{name}.lifecycle.provider_update.old_route_revision")
    new_route = integer(update.get("new_route_revision"), f"{name}.lifecycle.provider_update.new_route_revision")
    if new_provider != old_provider + 1 or new_route != old_route + 1:
        fail(f"{name}.lifecycle.provider_update: provider and dependent route must each bump once")
    boolean(update.get("dependent_route_bumped"), f"{name}.lifecycle.provider_update.dependent_route_bumped", True)


    override = obj(lifecycle.get("route_model_override"), f"{name}.lifecycle.route_model_override")
    exact_keys(override, {"provider_default_model_fingerprint", "route_override_model_fingerprint", "observed_provider_model_fingerprint", "override_applied"}, f"{name}.lifecycle.route_model_override")
    default = fingerprint(override.get("provider_default_model_fingerprint"), f"{name}.lifecycle.route_model_override.provider_default_model_fingerprint")
    wanted = fingerprint(override.get("route_override_model_fingerprint"), f"{name}.lifecycle.route_model_override.route_override_model_fingerprint")
    observed = fingerprint(override.get("observed_provider_model_fingerprint"), f"{name}.lifecycle.route_model_override.observed_provider_model_fingerprint")
    if wanted == default or observed != wanted:
        fail(f"{name}.lifecycle.route_model_override: distinct override must reach provider")
    boolean(override.get("override_applied"), f"{name}.lifecycle.route_model_override.override_applied", True)

    stale = indexed(lifecycle.get("stale_revision_failures"), f"{name}.lifecycle.stale_revision_failures", "resource_kind")
    if set(stale) != {"ai_route"}:
        fail(f"{name}.lifecycle.stale_revision_failures: live stale-route proof required")
    for kind, row in stale.items():
        exact_keys(row, {"resource_kind", "http_status", "error", "state_unchanged"}, f"{name}.lifecycle.stale_revision_failures[{kind}]")
        integer(row.get("http_status"), f"{name}.lifecycle.stale_revision_failures[{kind}].http_status", 409)
        if row.get("error") != "revision_mismatch":
            fail(f"{name}.lifecycle.stale_revision_failures[{kind}].error: revision_mismatch required")
        boolean(row.get("state_unchanged"), f"{name}.lifecycle.stale_revision_failures[{kind}].state_unchanged", True)

    deletes = indexed(lifecycle.get("referenced_delete_failures"), f"{name}.lifecycle.referenced_delete_failures", "resource_kind")
    if set(deletes) != {"ai_provider", "ai_secret"}:
        fail(f"{name}.lifecycle.referenced_delete_failures: provider and secret required")
    for kind, row in deletes.items():
        exact_keys(row, {"resource_kind", "referenced_by_active_resource", "http_status", "error", "resource_retained"}, f"{name}.lifecycle.referenced_delete_failures[{kind}]")
        boolean(row.get("referenced_by_active_resource"), f"{name}.lifecycle.referenced_delete_failures[{kind}].referenced_by_active_resource", True)
        integer(row.get("http_status"), f"{name}.lifecycle.referenced_delete_failures[{kind}].http_status", 409)
        if row.get("error") != "conflict":
            fail(f"{name}.lifecycle.referenced_delete_failures[{kind}].error: conflict required")
        boolean(row.get("resource_retained"), f"{name}.lifecycle.referenced_delete_failures[{kind}].resource_retained", True)


def check_routing(team: dict[str, Any], name: str) -> None:
    routing = obj(team.get("routing"), f"{name}.routing")
    exact_keys(routing, {"applicable", "reason", "weighted", "priority_failover"}, f"{name}.routing")
    if routing.get("applicable") is False:
        if routing.get("reason") != "deep_probe_alpha_payments_only" or routing.get("weighted") is not None or routing.get("priority_failover") is not None:
            fail(f"{name}.routing: explicit Alpha-only deep-probe boundary required")
        return
    boolean(routing.get("applicable"), f"{name}.routing.applicable", True)
    if routing.get("reason") != "alpha_payments_deep_probe":
        fail(f"{name}.routing.reason: Alpha deep-probe scope required")
    weighted = obj(routing.get("weighted"), f"{name}.routing.weighted")
    exact_keys(weighted, {"attempt_count", "configured_weights", "both_backends_observed", "foreign_backend_count"}, f"{name}.routing.weighted")
    attempts = integer(weighted.get("attempt_count"), f"{name}.routing.weighted.attempt_count")
    if attempts < 20:
        fail(f"{name}.routing.weighted.attempt_count: at least 20 probes required")
    if weighted.get("configured_weights") != {"primary": 1, "secondary": 1}:
        fail(f"{name}.routing.weighted.configured_weights: exact 1/1 fixture required")
    boolean(weighted.get("both_backends_observed"), f"{name}.routing.weighted.both_backends_observed", True)
    integer(weighted.get("foreign_backend_count"), f"{name}.routing.weighted.foreign_backend_count", 0)

    failover = obj(routing.get("priority_failover"), f"{name}.routing.priority_failover")
    exact_keys(failover, {"primary_priority", "secondary_priority", "primary_forced_unavailable", "secondary_success_count", "primary_success_during_outage", "bounded_failover", "foreign_backend_count"}, f"{name}.routing.priority_failover")
    integer(failover.get("primary_priority"), f"{name}.routing.priority_failover.primary_priority", 0)
    integer(failover.get("secondary_priority"), f"{name}.routing.priority_failover.secondary_priority", 1)
    boolean(failover.get("primary_forced_unavailable"), f"{name}.routing.priority_failover.primary_forced_unavailable", True)
    if integer(failover.get("secondary_success_count"), f"{name}.routing.priority_failover.secondary_success_count") < 1:
        fail(f"{name}.routing.priority_failover: secondary success required")
    integer(failover.get("primary_success_during_outage"), f"{name}.routing.priority_failover.primary_success_during_outage", 0)
    boolean(failover.get("bounded_failover"), f"{name}.routing.priority_failover.bounded_failover", True)
    integer(failover.get("foreign_backend_count"), f"{name}.routing.priority_failover.foreign_backend_count", 0)


def check_teams(e: dict[str, Any]) -> None:
    teams = team_index(field(e, "teams"), "teams")
    markers: set[str] = set()
    routing_carriers: list[str] = []
    for key, team in teams.items():
        exact_keys(team, {"team_key", "run_marker_fingerprint", "management", "traffic", "budget", "lifecycle", "routing"}, f"teams[{key}]")
        marker = fingerprint(team.get("run_marker_fingerprint"), f"teams[{key}].run_marker_fingerprint")
        if marker in markers:
            fail("teams.run_marker_fingerprint: globally unique markers required")
        markers.add(marker)
        check_management(team, f"teams[{key}]")
        check_traffic(team, f"teams[{key}]")
        check_budget(team, f"teams[{key}]")
        check_lifecycle(team, f"teams[{key}]")
        check_routing(team, f"teams[{key}]")
        if team["routing"].get("applicable") is True:
            routing_carriers.append(key)
    if routing_carriers != ["alpha/payments"]:
        fail(f"teams.routing: exactly alpha/payments must carry the deep probe, got {routing_carriers!r}")


def check_isolation(e: dict[str, Any]) -> None:
    isolation = obj(field(e, "isolation"), "isolation")
    exact_keys(isolation, {"teams", "beta_to_alpha_denial"}, "isolation")
    for key, row in team_index(isolation.get("teams"), "isolation.teams").items():
        exact_keys(row, {"team_key", "own_ai_resource_count", "foreign_ai_resource_count", "own_usage_count", "foreign_usage_count", "own_trace_count", "foreign_trace_count", "own_listener_count", "foreign_listener_count", "own_dataplane_count", "foreign_dataplane_count"}, f"isolation.teams[{key}]")
        for attr in ("own_ai_resource_count", "own_trace_count", "own_listener_count", "own_dataplane_count"):
            if integer(row.get(attr), f"isolation.teams[{key}].{attr}") < 1:
                fail(f"isolation.teams[{key}].{attr}: own evidence required")
        if integer(row.get("own_usage_count"), f"isolation.teams[{key}].own_usage_count") < 3:
            fail(f"isolation.teams[{key}].own_usage_count: nonstream, stream and shadow usage evidence required")
        for attr in ("foreign_ai_resource_count", "foreign_usage_count", "foreign_trace_count", "foreign_listener_count", "foreign_dataplane_count"):
            integer(row.get(attr), f"isolation.teams[{key}].{attr}", 0)
    denial = obj(isolation.get("beta_to_alpha_denial"), "isolation.beta_to_alpha_denial")
    exact_keys(denial, {"actor", "target", "http_status", "authorized", "resource_visible", "state_changed"}, "isolation.beta_to_alpha_denial")
    if denial.get("actor") != "beta/owner" or denial.get("target") != "alpha/payments":
        fail("isolation.beta_to_alpha_denial: exact Beta-to-Alpha probe required")
    if denial.get("http_status") != 400:
        fail("isolation.beta_to_alpha_denial.http_status: denied/hidden status required")
    for attr in ("authorized", "resource_visible", "state_changed"):
        boolean(denial.get(attr), f"isolation.beta_to_alpha_denial.{attr}", False)


def check_mcp(e: dict[str, Any]) -> None:
    mcp = obj(field(e, "mcp"), "mcp")
    exact_keys(mcp, {"management", "team_sessions", "dynamic_api_claim"}, "mcp")
    management = obj(mcp.get("management"), "mcp.management")
    exact_keys(management, {"status", "connections", "catalog"}, "mcp.management")
    for surface in ("status", "connections", "catalog"):
        item = obj(management.get(surface), f"mcp.management.{surface}")
        exact_keys(item, {"support_status", "supported_api_only", "foreign_result_count"}, f"mcp.management.{surface}")
        if item.get("support_status") != "supported":
            fail(f"mcp.management.{surface}.support_status: supported required")
        boolean(item.get("supported_api_only"), f"mcp.management.{surface}.supported_api_only", True)
        integer(item.get("foreign_result_count"), f"mcp.management.{surface}.foreign_result_count", 0)

    sessions = team_index(mcp.get("team_sessions"), "mcp.team_sessions")
    all_refs: set[str] = set()
    probe_carriers: list[str] = []
    for key, session in sessions.items():
        exact_keys(session, {"team_key", "connection_fingerprint", "identifiers_non_correlatable", "raw_identifiers_recorded", "probe_executed", "reason", "initialize", "call", "same_session_grant_revocation", "same_session_agent_disable"}, f"mcp.team_sessions[{key}]")
        connection = fingerprint(session.get("connection_fingerprint"), f"mcp.team_sessions[{key}].connection_fingerprint")
        if connection in all_refs:
            fail("mcp.team_sessions.connection_fingerprint: per-team identifiers must not correlate")
        all_refs.add(connection)
        boolean(session.get("identifiers_non_correlatable"), f"mcp.team_sessions[{key}].identifiers_non_correlatable", True)
        boolean(session.get("raw_identifiers_recorded"), f"mcp.team_sessions[{key}].raw_identifiers_recorded", False)
        if session.get("probe_executed") is False:
            if session.get("reason") != "deep_probe_alpha_payments_only" or any(session.get(name) is not None for name in ("initialize", "call", "same_session_grant_revocation", "same_session_agent_disable")):
                fail(f"mcp.team_sessions[{key}]: explicit Alpha-only deep-probe boundary required")
            continue
        boolean(session.get("probe_executed"), f"mcp.team_sessions[{key}].probe_executed", True)
        if key != "alpha/payments" or session.get("reason") != "alpha_payments_deep_probe":
            fail(f"mcp.team_sessions[{key}]: only Alpha/Payments may carry the live JSON-RPC probe")
        probe_carriers.append(key)
        initialize = obj(session.get("initialize"), f"mcp.team_sessions[{key}].initialize")
        exact_keys(initialize, {"jsonrpc", "response_id_matched", "session_established"}, f"mcp.team_sessions[{key}].initialize")
        if initialize.get("jsonrpc") != "2.0":
            fail(f"mcp.team_sessions[{key}].initialize.jsonrpc: 2.0 required")
        boolean(initialize.get("response_id_matched"), f"mcp.team_sessions[{key}].initialize.response_id_matched", True)
        boolean(initialize.get("session_established"), f"mcp.team_sessions[{key}].initialize.session_established", True)

        call = obj(session.get("call"), f"mcp.team_sessions[{key}].call")
        exact_keys(call, {"http_status", "jsonrpc_result", "authorized", "foreign_effect_count"}, f"mcp.team_sessions[{key}].call")
        integer(call.get("http_status"), f"mcp.team_sessions[{key}].call.http_status", 200)
        boolean(call.get("jsonrpc_result"), f"mcp.team_sessions[{key}].call.jsonrpc_result", True)
        boolean(call.get("authorized"), f"mcp.team_sessions[{key}].call.authorized", True)
        integer(call.get("foreign_effect_count"), f"mcp.team_sessions[{key}].call.foreign_effect_count", 0)
        for probe_name, action in (("same_session_grant_revocation", "grant_revoked"), ("same_session_agent_disable", "agent_disabled")):
            probe = obj(session.get(probe_name), f"mcp.team_sessions[{key}].{probe_name}")
            exact_keys(probe, {"same_session", "action_committed_before_next_request", "next_request_denied", "next_request_jsonrpc_error", "next_request_effect_count", "denial_reason"}, f"mcp.team_sessions[{key}].{probe_name}")
            boolean(probe.get("same_session"), f"mcp.team_sessions[{key}].{probe_name}.same_session", True)
            boolean(probe.get("action_committed_before_next_request"), f"mcp.team_sessions[{key}].{probe_name}.action_committed_before_next_request", True)
            boolean(probe.get("next_request_denied"), f"mcp.team_sessions[{key}].{probe_name}.next_request_denied", True)
            boolean(probe.get("next_request_jsonrpc_error"), f"mcp.team_sessions[{key}].{probe_name}.next_request_jsonrpc_error", probe_name == "same_session_grant_revocation")
            integer(probe.get("next_request_effect_count"), f"mcp.team_sessions[{key}].{probe_name}.next_request_effect_count", 0)
            if probe.get("denial_reason") != action:
                fail(f"mcp.team_sessions[{key}].{probe_name}.denial_reason: {action} required")
    if probe_carriers != ["alpha/payments"]:
        fail(f"mcp.team_sessions: exactly alpha/payments must carry the JSON-RPC probe, got {probe_carriers!r}")

    claim = obj(mcp.get("dynamic_api_claim"), "mcp.dynamic_api_claim")
    exact_keys(claim, {"support_status", "unconditional_support_claimed", "descriptor_execution_mandatory", "advertised_dynamic_tool_names", "limitation_id"}, "mcp.dynamic_api_claim")
    if claim.get("support_status") != "evolving-not-required":
        fail("mcp.dynamic_api_claim.support_status: evolving-not-required required")
    boolean(claim.get("unconditional_support_claimed"), "mcp.dynamic_api_claim.unconditional_support_claimed", False)
    boolean(claim.get("descriptor_execution_mandatory"), "mcp.dynamic_api_claim.descriptor_execution_mandatory", False)
    names = seq(claim.get("advertised_dynamic_tool_names"), "mcp.dynamic_api_claim.advertised_dynamic_tool_names")
    if any(not isinstance(name, str) or DYNAMIC_API.match(name) for name in names):
        fail("mcp.dynamic_api_claim: unconditional dynamic api_* advertisements are forbidden")
    if claim.get("limitation_id") != "dynamic_api_descriptor_execution":
        fail("mcp.dynamic_api_claim.limitation_id: explicit limitation linkage required")


def check_limitations(e: dict[str, Any]) -> None:
    rows = indexed(field(e, "limitations"), "limitations")
    if set(rows) != {"mcp_jsonrpc_evolving", "dynamic_api_descriptor_execution"}:
        fail("limitations: exact MCP limitation inventory required")
    jsonrpc = rows["mcp_jsonrpc_evolving"]
    exact_keys(jsonrpc, {"id", "support_status", "guaranteed_methods", "revocation_guarantee", "agent_disable_guarantee"}, "limitations[mcp_jsonrpc_evolving]")
    if jsonrpc.get("support_status") != "supported-with-limitation/evolving" or jsonrpc.get("guaranteed_methods") != ["initialize", "tools/call"]:
        fail("limitations[mcp_jsonrpc_evolving]: exact evolving JSON-RPC claim required")
    if jsonrpc.get("revocation_guarantee") != "same_session_next_request_denied" or jsonrpc.get("agent_disable_guarantee") != "same_session_next_request_denied":
        fail("limitations[mcp_jsonrpc_evolving]: same-session next-request guarantees required")
    dynamic = rows["dynamic_api_descriptor_execution"]
    exact_keys(dynamic, {"id", "support_status", "mandatory_for_acceptance", "unconditional_api_star_claim_allowed"}, "limitations[dynamic_api_descriptor_execution]")
    if dynamic.get("support_status") != "evolving-not-required":
        fail("limitations[dynamic_api_descriptor_execution].support_status: evolving-not-required required")
    boolean(dynamic.get("mandatory_for_acceptance"), "limitations[dynamic_api_descriptor_execution].mandatory_for_acceptance", False)
    boolean(dynamic.get("unconditional_api_star_claim_allowed"), "limitations[dynamic_api_descriptor_execution].unconditional_api_star_claim_allowed", False)


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(field(e, "cleanup"), "cleanup")
    exact_keys(cleanup, {"evidence_frozen_before_cleanup", "cleanup_completed", "inventories", "postgresql", "retained_dataplane_tombstones", "retained_mcp_history", "retained_ai_history"}, "cleanup")
    boolean(cleanup.get("evidence_frozen_before_cleanup"), "cleanup.evidence_frozen_before_cleanup", True)
    boolean(cleanup.get("cleanup_completed"), "cleanup.cleanup_completed", True)
    inventories = indexed(cleanup.get("inventories"), "cleanup.inventories", "resource_kind")
    if set(inventories) != CLEANUP_INVENTORY:
        fail(f"cleanup.inventories: exact inventory required; got {sorted(inventories)!r}")
    for kind, row in inventories.items():
        exact_keys(row, {"resource_kind", "active_run_owned_remaining_count", "authoritative_inventory_checked"}, f"cleanup.inventories[{kind}]")
        integer(row.get("active_run_owned_remaining_count"), f"cleanup.inventories[{kind}].active_run_owned_remaining_count", 0)
        boolean(row.get("authoritative_inventory_checked"), f"cleanup.inventories[{kind}].authoritative_inventory_checked", True)
    postgres = obj(cleanup.get("postgresql"), "cleanup.postgresql")
    exact_keys(postgres, {"restored", "memory_mib"}, "cleanup.postgresql")
    boolean(postgres.get("restored"), "cleanup.postgresql.restored", True)
    integer(postgres.get("memory_mib"), "cleanup.postgresql.memory_mib", 256)
    tombstones = obj(cleanup.get("retained_dataplane_tombstones"), "cleanup.retained_dataplane_tombstones")
    exact_keys(tombstones, {"count", "explicit_history", "active", "addressable", "can_serve_traffic"}, "cleanup.retained_dataplane_tombstones")
    integer(tombstones.get("count"), "cleanup.retained_dataplane_tombstones.count")
    boolean(tombstones.get("explicit_history"), "cleanup.retained_dataplane_tombstones.explicit_history", True)
    for attr in ("active", "addressable", "can_serve_traffic"):
        boolean(tombstones.get(attr), f"cleanup.retained_dataplane_tombstones.{attr}", False)
    history = obj(cleanup.get("retained_mcp_history"), "cleanup.retained_mcp_history")
    exact_keys(history, {"agent_count", "disabled_agent_count", "active_agent_count", "revoked_grant_count", "connection_attribution_retained", "authorization_cached"}, "cleanup.retained_mcp_history")
    integer(history.get("agent_count"), "cleanup.retained_mcp_history.agent_count", 2)
    integer(history.get("disabled_agent_count"), "cleanup.retained_mcp_history.disabled_agent_count", 2)
    integer(history.get("active_agent_count"), "cleanup.retained_mcp_history.active_agent_count", 0)
    if integer(history.get("revoked_grant_count"), "cleanup.retained_mcp_history.revoked_grant_count") < 1:
        fail("cleanup.retained_mcp_history: revoked grant evidence required")
    boolean(history.get("connection_attribution_retained"), "cleanup.retained_mcp_history.connection_attribution_retained", True)
    boolean(history.get("authorization_cached"), "cleanup.retained_mcp_history.authorization_cached", False)
    ai_history = obj(cleanup.get("retained_ai_history"), "cleanup.retained_ai_history")
    exact_keys(ai_history, {"usage_rows_retained", "trace_rows_retained", "active_configuration", "contains_secret_values", "retention_ttl_days"}, "cleanup.retained_ai_history")
    boolean(ai_history.get("usage_rows_retained"), "cleanup.retained_ai_history.usage_rows_retained", True)
    boolean(ai_history.get("trace_rows_retained"), "cleanup.retained_ai_history.trace_rows_retained", True)
    boolean(ai_history.get("active_configuration"), "cleanup.retained_ai_history.active_configuration", False)
    boolean(ai_history.get("contains_secret_values"), "cleanup.retained_ai_history.contains_secret_values", False)
    integer(ai_history.get("retention_ttl_days"), "cleanup.retained_ai_history.retention_ttl_days", 30)


def walk(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key.lower() in PROHIBITED_KEYS:
                fail(f"redaction: prohibited key {path}.{key}")
            walk(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            walk(child, f"{path}[{index}]")
    elif isinstance(value, str):
        for pattern in SECRET_PATTERNS:
            if pattern.search(value):
                fail(f"redaction: prohibited value at {path}")


def check_redaction(e: dict[str, Any]) -> None:
    redaction = obj(field(e, "redaction"), "redaction")
    exact_keys(redaction, {"sanitized_projection", "secret_values_recorded", "raw_provider_payloads_recorded", "raw_identifiers_recorded", "private_paths_recorded", "scan_after_final_edit", "undisposed_match_count", "pattern_classes"}, "redaction")
    boolean(redaction.get("sanitized_projection"), "redaction.sanitized_projection", True)
    for attr in ("secret_values_recorded", "raw_provider_payloads_recorded", "raw_identifiers_recorded", "private_paths_recorded"):
        boolean(redaction.get(attr), f"redaction.{attr}", False)
    boolean(redaction.get("scan_after_final_edit"), "redaction.scan_after_final_edit", True)
    integer(redaction.get("undisposed_match_count"), "redaction.undisposed_match_count", 0)
    required = {"ai_credentials", "bearer_tokens", "provider_payloads", "mcp_identifiers", "private_paths", "foreign_identifiers"}
    if set(seq(redaction.get("pattern_classes"), "redaction.pattern_classes")) != required:
        fail("redaction.pattern_classes: exact scan classes required")
    walk(e)


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.8-RUN": ("independent sanitized exact four-team run", check_run),
    "FPV2-D23.8-INVENTORY": ("exact AI/MCP support inventory", check_inventory),
    "FPV2-D23.8-AI": ("AI management, traffic, budgets, lifecycle and routing", check_teams),
    "FPV2-D23.8-ISOLATION": ("four-team AI visibility and Beta-to-Alpha denial", check_isolation),
    "FPV2-D23.8-MCP": ("MCP management and evolving same-session JSON-RPC guarantees", check_mcp),
    "FPV2-D23.8-LIMITATIONS": ("explicit evolving JSON-RPC and dynamic api_* limitations", check_limitations),
    "FPV2-D23.8-CLEANUP": ("zero active run/provider residue and PostgreSQL restore", check_cleanup),
    "FPV2-D23.8-REDACTION": ("recursive secrets, payload, identifier and path redaction", check_redaction),
}


def fp(number: int) -> str:
    return "sha256:" + f"{number:064x}"


def fixture() -> dict[str, Any]:
    teams: list[dict[str, Any]] = []
    isolation: list[dict[str, Any]] = []
    sessions: list[dict[str, Any]] = []
    for index, team_key in enumerate(TEAMS, 1):
        management = [{"resource_kind": kind, "support_status": "supported", "observed_operations": ops, "supported_api_only": True, "foreign_result_count": 0} for kind, ops in MANAGEMENT_OPERATIONS.items()]
        stale = [{"resource_kind": "ai_route", "http_status": 409, "error": "revision_mismatch", "state_unchanged": True}]
        deletes = [{"resource_kind": kind, "referenced_by_active_resource": True, "http_status": 409, "error": "conflict", "resource_retained": True} for kind in ("ai_provider", "ai_secret")]
        teams.append({
            "team_key": team_key, "run_marker_fingerprint": fp(index), "management": management,
            "traffic": {
                "nonstream": {"request_fingerprint": fp(100 + index), "through_envoy": True, "real_provider_request": True, "http_status": 200, "response_marker_matched": True, "usage_record_count": 1},
                "finite_stream": {"request_fingerprint": fp(110 + index), "through_envoy": True, "real_provider_request": True, "http_status": 200, "chunk_count": 3, "terminal_event_observed": True, "bounded_completion": True, "usage_record_count": 1},
                "malformed_200": {"request_fingerprint": fp(120 + index), "through_envoy": True, "provider_http_status": 200, "downstream_http_status": 200, "provider_payload_fingerprint": fp(130 + index), "downstream_payload_fingerprint": fp(130 + index), "response_passed_through": True, "usage_record_count": 0},
                "provider_500": {"request_fingerprint": fp(140 + index), "through_envoy": True, "provider_http_status": 500, "downstream_http_status": 500, "trace_record_count": 1, "trace_outcome": "provider_failure", "trace_provider_status": 500, "usage_record_count": 0},
            },
            "budget": {
                "shadow": {"mode": "shadow", "limit_reached": True, "http_status": 200, "provider_request_count": 1, "usage_record_count": 1},
                "enforcing": {"mode": "enforcing", "over_limit": True, "http_status": 429, "provider_request_count": 0, "usage_record_count": 0, "denial_outcome": "budget_exceeded"},
            },
            "lifecycle": {
                "provider_update": {"old_provider_revision": 1, "new_provider_revision": 2, "old_route_revision": 1, "new_route_revision": 2, "dependent_route_bumped": True},
                "route_model_override": {"provider_default_model_fingerprint": fp(200 + index), "route_override_model_fingerprint": fp(210 + index), "observed_provider_model_fingerprint": fp(210 + index), "override_applied": True},
                "stale_revision_failures": stale, "referenced_delete_failures": deletes,
            },
            "routing": ({"applicable": True, "reason": "alpha_payments_deep_probe",
                "weighted": {"attempt_count": 30, "configured_weights": {"primary": 1, "secondary": 1}, "both_backends_observed": True, "foreign_backend_count": 0},
                "priority_failover": {"primary_priority": 0, "secondary_priority": 1, "primary_forced_unavailable": True, "secondary_success_count": 1, "primary_success_during_outage": 0, "bounded_failover": True, "foreign_backend_count": 0}}
                if index == 1 else {"applicable": False, "reason": "deep_probe_alpha_payments_only", "weighted": None, "priority_failover": None}),
        })
        isolation.append({"team_key": team_key, "own_ai_resource_count": 7, "foreign_ai_resource_count": 0, "own_usage_count": 3, "foreign_usage_count": 0, "own_trace_count": 1, "foreign_trace_count": 0, "own_listener_count": 1, "foreign_listener_count": 0, "own_dataplane_count": 1, "foreign_dataplane_count": 0})
        denial = lambda reason: {"same_session": True, "action_committed_before_next_request": True, "next_request_denied": True, "next_request_jsonrpc_error": True, "next_request_effect_count": 0, "denial_reason": reason}
        session={"team_key": team_key, "connection_fingerprint": fp(300 + index), "identifiers_non_correlatable": True, "raw_identifiers_recorded": False,
                 "probe_executed": index == 1, "reason": "alpha_payments_deep_probe" if index == 1 else "deep_probe_alpha_payments_only",
                 "initialize": None, "call": None, "same_session_grant_revocation": None, "same_session_agent_disable": None}
        if index == 1:
            session.update({"initialize": {"jsonrpc": "2.0", "response_id_matched": True, "session_established": True}, "call": {"http_status": 200, "jsonrpc_result": True, "authorized": True, "foreign_effect_count": 0}, "same_session_grant_revocation": denial("grant_revoked"), "same_session_agent_disable": {**denial("agent_disabled"), "next_request_jsonrpc_error": False}})
        sessions.append(session)
    inventories = [{"resource_kind": kind, "active_run_owned_remaining_count": 0, "authoritative_inventory_checked": True} for kind in sorted(CLEANUP_INVENTORY)]
    return {
        "schema": SCHEMA,
        "run": {"synthetic_only": True, "rerunnable": True, "supported_surfaces_only": True, "direct_database_access": False, "independent_author_read_implementation": False, "sanitized_projection_contains_private_values": False, "started_at_utc": "2026-08-22T01:00:00Z", "finished_at_utc": "2026-08-22T02:00:00Z", "team_keys": list(TEAMS)},
        "inventory": [{"capability": key, "support_status": value} for key, value in APPROVED_INVENTORY.items()],
        "teams": teams,
        "isolation": {"teams": isolation, "beta_to_alpha_denial": {"actor": "beta/owner", "target": "alpha/payments", "http_status": 400, "authorized": False, "resource_visible": False, "state_changed": False}},
        "mcp": {"management": {surface: {"support_status": "supported", "supported_api_only": True, "foreign_result_count": 0} for surface in ("status", "connections", "catalog")}, "team_sessions": sessions, "dynamic_api_claim": {"support_status": "evolving-not-required", "unconditional_support_claimed": False, "descriptor_execution_mandatory": False, "advertised_dynamic_tool_names": [], "limitation_id": "dynamic_api_descriptor_execution"}},
        "limitations": [
            {"id": "mcp_jsonrpc_evolving", "support_status": "supported-with-limitation/evolving", "guaranteed_methods": ["initialize", "tools/call"], "revocation_guarantee": "same_session_next_request_denied", "agent_disable_guarantee": "same_session_next_request_denied"},
            {"id": "dynamic_api_descriptor_execution", "support_status": "evolving-not-required", "mandatory_for_acceptance": False, "unconditional_api_star_claim_allowed": False},
        ],
        "cleanup": {"evidence_frozen_before_cleanup": True, "cleanup_completed": True, "inventories": inventories, "postgresql": {"restored": True, "memory_mib": 256}, "retained_dataplane_tombstones": {"count": 4, "explicit_history": True, "active": False, "addressable": False, "can_serve_traffic": False}, "retained_mcp_history": {"agent_count": 2, "disabled_agent_count": 2, "active_agent_count": 0, "revoked_grant_count": 1, "connection_attribution_retained": True, "authorization_cached": False}, "retained_ai_history": {"usage_rows_retained": True, "trace_rows_retained": True, "active_configuration": False, "contains_secret_values": False, "retention_ttl_days": 30}},
        "redaction": {"sanitized_projection": True, "secret_values_recorded": False, "raw_provider_payloads_recorded": False, "raw_identifiers_recorded": False, "private_paths_recorded": False, "scan_after_final_edit": True, "undisposed_match_count": 0, "pattern_classes": ["ai_credentials", "bearer_tokens", "provider_payloads", "mcp_identifiers", "private_paths", "foreign_identifiers"]},
    }


def remove_routing_probe(e: dict[str, Any]) -> None:
    for team in e["teams"]:
        team["routing"] = {"applicable": False, "reason": "deep_probe_alpha_payments_only", "weighted": None, "priority_failover": None}


def move_routing_probe(e: dict[str, Any]) -> None:
    probe = copy.deepcopy(e["teams"][0]["routing"])
    e["teams"][0]["routing"] = {"applicable": False, "reason": "deep_probe_alpha_payments_only", "weighted": None, "priority_failover": None}
    e["teams"][2]["routing"] = probe


def remove_mcp_probe(e: dict[str, Any]) -> None:
    session = e["mcp"]["team_sessions"][0]
    session.update(probe_executed=False, reason="deep_probe_alpha_payments_only", initialize=None, call=None, same_session_grant_revocation=None, same_session_agent_disable=None)


def self_test() -> int:
    good = fixture()
    failures: list[str] = []
    for scenario, (_, check) in SCENARIOS.items():
        try:
            check(good)
        except ContractFailure as error:
            failures.append(f"valid fixture rejected by {scenario}: {error}")
    mutations: list[tuple[str, Callable[[dict[str, Any]], None], str]] = [
        ("missing fourth team", lambda x: x["teams"].pop(), "FPV2-D23.8-AI"),
        ("inventory overclaim", lambda x: x["inventory"].append({"capability": "mcp.api_star", "support_status": "supported"}), "FPV2-D23.8-INVENTORY"),
        ("foreign management result", lambda x: x["teams"][0]["management"][0].__setitem__("foreign_result_count", 1), "FPV2-D23.8-AI"),
        ("nonstream double usage", lambda x: x["teams"][0]["traffic"]["nonstream"].__setitem__("usage_record_count", 2), "FPV2-D23.8-AI"),
        ("unterminated stream", lambda x: x["teams"][0]["traffic"]["finite_stream"].__setitem__("terminal_event_observed", False), "FPV2-D23.8-AI"),
        ("malformed response transformed", lambda x: x["teams"][0]["traffic"]["malformed_200"].__setitem__("downstream_payload_fingerprint", fp(999)), "FPV2-D23.8-AI"),
        ("malformed response usage", lambda x: x["teams"][0]["traffic"]["malformed_200"].__setitem__("usage_record_count", 1), "FPV2-D23.8-AI"),
        ("missing failure trace", lambda x: x["teams"][0]["traffic"]["provider_500"].__setitem__("trace_record_count", 0), "FPV2-D23.8-AI"),
        ("shadow blocks", lambda x: x["teams"][0]["budget"]["shadow"].__setitem__("http_status", 429), "FPV2-D23.8-AI"),
        ("enforcing reaches provider", lambda x: x["teams"][0]["budget"]["enforcing"].__setitem__("provider_request_count", 1), "FPV2-D23.8-AI"),
        ("dependent route not bumped", lambda x: x["teams"][0]["lifecycle"]["provider_update"].__setitem__("new_route_revision", 4), "FPV2-D23.8-AI"),
        ("override ignored", lambda x: x["teams"][0]["lifecycle"]["route_model_override"].__setitem__("observed_provider_model_fingerprint", fp(201)), "FPV2-D23.8-AI"),
        ("stale write accepted", lambda x: x["teams"][0]["lifecycle"]["stale_revision_failures"][0].__setitem__("http_status", 200), "FPV2-D23.8-AI"),
        ("referenced secret deleted", lambda x: x["teams"][0]["lifecycle"]["referenced_delete_failures"][1].__setitem__("resource_retained", False), "FPV2-D23.8-AI"),
        ("weighted backend unobserved", lambda x: x["teams"][0]["routing"]["weighted"].__setitem__("both_backends_observed", False), "FPV2-D23.8-AI"),
        ("priority failover absent", lambda x: x["teams"][0]["routing"]["priority_failover"].__setitem__("secondary_success_count", 0), "FPV2-D23.8-AI"),
        ("routing probe absent", remove_routing_probe, "FPV2-D23.8-AI"),
        ("routing probe wrong team", move_routing_probe, "FPV2-D23.8-AI"),
        ("foreign trace visible", lambda x: x["isolation"]["teams"][0].__setitem__("foreign_trace_count", 1), "FPV2-D23.8-ISOLATION"),
        ("Beta reads Alpha", lambda x: x["isolation"]["beta_to_alpha_denial"].__setitem__("resource_visible", True), "FPV2-D23.8-ISOLATION"),
        ("correlatable connections", lambda x: x["mcp"]["team_sessions"][1].__setitem__("connection_fingerprint", x["mcp"]["team_sessions"][0]["connection_fingerprint"]), "FPV2-D23.8-MCP"),
        ("grant revocation delayed", lambda x: x["mcp"]["team_sessions"][0]["same_session_grant_revocation"].__setitem__("next_request_denied", False), "FPV2-D23.8-MCP"),
        ("agent disable delayed", lambda x: x["mcp"]["team_sessions"][0]["same_session_agent_disable"].__setitem__("next_request_effect_count", 1), "FPV2-D23.8-MCP"),
        ("MCP probe absent", remove_mcp_probe, "FPV2-D23.8-MCP"),
        ("dynamic api claim", lambda x: x["mcp"]["dynamic_api_claim"].update(unconditional_support_claimed=True, advertised_dynamic_tool_names=["api_orders"]), "FPV2-D23.8-MCP"),
        ("dynamic execution mandatory", lambda x: x["limitations"][1].__setitem__("mandatory_for_acceptance", True), "FPV2-D23.8-LIMITATIONS"),
        ("Fly residue", lambda x: next(v for v in x["cleanup"]["inventories"] if v["resource_kind"] == "fly_provider_resources").__setitem__("active_run_owned_remaining_count", 1), "FPV2-D23.8-CLEANUP"),
        ("VM residue", lambda x: next(v for v in x["cleanup"]["inventories"] if v["resource_kind"] == "lima_vms").__setitem__("active_run_owned_remaining_count", 1), "FPV2-D23.8-CLEANUP"),
        ("PostgreSQL memory not restored", lambda x: x["cleanup"]["postgresql"].__setitem__("memory_mib", 512), "FPV2-D23.8-CLEANUP"),
        ("active tombstone", lambda x: x["cleanup"]["retained_dataplane_tombstones"].__setitem__("active", True), "FPV2-D23.8-CLEANUP"),
        ("secret-shaped key", lambda x: x["redaction"].__setitem__("api_key", "redacted"), "FPV2-D23.8-REDACTION"),
        ("private path", lambda x: x["redaction"]["pattern_classes"].append("/Users/example/private"), "FPV2-D23.8-REDACTION"),
    ]
    for label, mutate, scenario in mutations:
        bad = copy.deepcopy(good)
        mutate(bad)
        try:
            SCENARIOS[scenario][1](bad)
        except ContractFailure:
            pass
        else:
            failures.append(f"negative self-test did not fail closed: {label}")
    if failures:
        for failure in failures:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        print(f"AI/MCP acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"AI/MCP acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
    return 0


def load(path: Path) -> dict[str, Any]:
    try:
        return obj(json.loads(path.read_text(encoding="utf-8")), "evidence root")
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"evidence unreadable: {error}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS))
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list:
        for scenario, (description, _) in SCENARIOS.items():
            print(f"{scenario}\t{description}")
        return 0
    if args.self_test:
        return self_test()
    path = Path(args.evidence or os.environ.get("FLOWPLANE_AI_MCP_EVIDENCE", DEFAULT_EVIDENCE))
    try:
        evidence = load(path)
    except ContractFailure as error:
        print(f"AI/MCP acceptance: FAIL: {error}", file=sys.stderr)
        return 1
    selected = [args.scenario] if args.scenario else list(SCENARIOS)
    failures = 0
    for scenario in selected:
        description, check = SCENARIOS[scenario]
        try:
            check(evidence)
        except ContractFailure as error:
            failures += 1
            print(f"{scenario}: FAIL: {error}", file=sys.stderr)
        else:
            print(f"{scenario}: PASS: {description}")
    if failures:
        print(f"AI/MCP acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"AI/MCP acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
