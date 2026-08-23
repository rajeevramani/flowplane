#!/usr/bin/env python3
"""Independent, fail-closed fpv2-d23.7 API lifecycle acceptance gate.

The input is a sanitized ``flowplane.qualification.api-lifecycle/v2`` JSON
projection.  This harness intentionally validates only the approved d23.7
inventory and the lifecycle that was actually observed.  In particular:

* immutable versions are proved by a stable content hash and ETag plus a
  conditional GET returning 304; there is no conditional-update/412 claim;
* imported specs create internal tool rows while unpublished, but do not enter
  the dynamic catalog and cannot use the learned-only rejection transition;
* route plans originate from published discovery specs, not generated tools;
* generated-tool invocation is deferred to fpv2-d23.8;
* publication never changes Envoy routes; only route-plan apply does;
* retained discovery history and retired dataplane tombstones are inert history;
* non-zero ephemeral Tailscale provider residue fails the cleanup gate.

The projection contains fingerprints, counts, status classes, synthetic
references, and timestamps only.  Raw payloads, credentials, private paths, and
private evidence are forbidden.  ``--self-test`` uses synthetic projections and
does not read production implementation or private evidence.
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

SCHEMA = "flowplane.qualification.api-lifecycle/v2"
DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.7/api-lifecycle.json")
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
ETAG = re.compile(r'^"sha256:[0-9a-f]{64}"$')
REF = re.compile(r"^[a-z][a-z0-9_-]{2,63}$")
UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
TEAM_KEYS = ("alpha/payments", "alpha/shared", "beta/payments", "beta/shared")

APPROVED_INVENTORY = {
    "api.definition": "supported",
    "api.route-binding": "supported",
    "api.spec.import": "supported",
    "api.spec.immutable-content-etag": "supported",
    "api.spec.review-reject": "supported",
    "api.spec.publish": "supported",
    "api.generated-tools": "supported",
    "learning.session": "supported",
    "learning.observations": "supported-internal-write-only",
    "learning.spec-generation": "supported",
    "discovery.session-temporary-gateway": "supported",
    "discovery.observations": "supported-internal-write-only",
    "discovery.spec-generation": "supported",
    "route-generation.plan": "supported-dry-run",
    "route-generation.apply": "supported",
    "mcp.generated-tool-execution": "deferred-fpv2-d23.8",
}

PROHIBITED_KEYS = {
    "token", "access_token", "refresh_token", "authorization", "password",
    "secret", "secret_value", "private_key", "raw_body", "raw_log", "raw_path",
    "spec_content", "tool_source", "credential", "cookie", "conditional_update_attempted",
    "invocation_succeeded", "tool_invocation", "private_evidence_path",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~-]+"),
    re.compile(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b"),
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


def boolean(value: Any, name: str, expected: bool) -> None:
    if value is not expected:
        fail(f"{name}: expected {expected!r}, observed {value!r}")


def integer(value: Any, name: str, expected: int | None = None) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        fail(f"{name}: non-negative integer required")
    if expected is not None and value != expected:
        fail(f"{name}: expected {expected}, observed {value}")
    return value


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


def indexed(value: Any, name: str) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(seq(value, name)):
        item = obj(raw, f"{name}[{index}]")
        item_id = text(item.get("id"), f"{name}[{index}].id")
        if not REF.fullmatch(item_id):
            fail(f"{name}[{index}].id: sanitized reference required")
        if item_id in result:
            fail(f"{name}: duplicate id {item_id!r}")
        result[item_id] = item
    return result


def teams_indexed(value: Any, name: str) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(seq(value, name)):
        item = obj(raw, f"{name}[{index}]")
        key = text(item.get("team_key"), f"{name}[{index}].team_key")
        if key not in TEAM_KEYS:
            fail(f"{name}[{index}].team_key: unexpected team {key!r}")
        if key in result:
            fail(f"{name}: duplicate team {key!r}")
        result[key] = item
    if tuple(sorted(result)) != tuple(sorted(TEAM_KEYS)):
        fail(f"{name}: exact four-team fixture required")
    return result


def sha(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not SHA256.fullmatch(candidate):
        fail(f"{name}: sha256 fingerprint required")
    return candidate


def etag(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not ETAG.fullmatch(candidate):
        fail(f"{name}: quoted sha256 ETag required")
    return candidate


def timestamp(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not UTC.fullmatch(candidate):
        fail(f"{name}: second-precision UTC timestamp required")
    return candidate


def exact_keys(item: dict[str, Any], expected: set[str], name: str) -> None:
    if set(item) != expected:
        fail(f"{name}: exact fields required; got {sorted(item)!r}")


def check_run(e: dict[str, Any]) -> None:
    exact_keys(e, {"schema", "run", "inventory", "team_lifecycles", "route_generation", "isolation",
                   "evidence_limitations", "cleanup", "redaction"}, "evidence root")
    equal(e, "schema", SCHEMA)
    run = obj(field(e, "run"), "run")
    exact_keys(run, {"synthetic_only", "rerunnable", "supported_surfaces_only", "direct_database_access",
                     "independent_acceptance_author_read_implementation", "sanitized_projection_contains_private_values", "started_at_utc",
                     "finished_at_utc", "teams"}, "run")
    equal(e, "run.synthetic_only", True)
    equal(e, "run.rerunnable", True)
    equal(e, "run.supported_surfaces_only", True)
    equal(e, "run.direct_database_access", False)
    equal(e, "run.independent_acceptance_author_read_implementation", False)
    equal(e, "run.sanitized_projection_contains_private_values", False)
    started = timestamp(field(e, "run.started_at_utc"), "run.started_at_utc")
    finished = timestamp(field(e, "run.finished_at_utc"), "run.finished_at_utc")
    if finished <= started:
        fail("run: finish must post-date start")
    teams = teams_indexed(field(e, "run.teams"), "run.teams")
    markers: set[str] = set()
    for key, item in teams.items():
        exact_keys(item, {"team_key", "marker", "created_via_supported_api"}, f"run.teams[{key}]")
        marker = text(item.get("marker"), f"run.teams[{key}].marker")
        if len(marker) < 12 or marker in markers:
            fail("run.teams.marker: unique run-scoped marker required")
        markers.add(marker)
        boolean(item.get("created_via_supported_api"), f"run.teams[{key}].created_via_supported_api", True)


def check_inventory(e: dict[str, Any]) -> None:
    rows = seq(field(e, "inventory"), "inventory")
    actual: dict[str, str] = {}
    for index, raw in enumerate(rows):
        row = obj(raw, f"inventory[{index}]")
        exact_keys(row, {"capability", "support_status"}, f"inventory[{index}]")
        capability = text(row.get("capability"), f"inventory[{index}].capability")
        if capability in actual:
            fail(f"inventory: duplicate capability {capability!r}")
        actual[capability] = text(row.get("support_status"), f"inventory[{index}].support_status")
    if actual != APPROVED_INVENTORY:
        missing = sorted(set(APPROVED_INVENTORY) - set(actual))
        extra = sorted(set(actual) - set(APPROVED_INVENTORY))
        wrong = sorted(key for key in set(actual) & set(APPROVED_INVENTORY) if actual[key] != APPROVED_INVENTORY[key])
        fail(f"inventory: exact approved inventory required; missing={missing!r} extra={extra!r} wrong_status={wrong!r}")


def check_immutability(item: Any, name: str) -> None:
    probe = obj(item, name)
    exact_keys(probe, {
        "content_fingerprint_before", "content_fingerprint_after", "etag_before",
        "etag_after", "conditional_get_if_none_match", "conditional_get_http_status",
    }, name)
    before_hash = sha(probe.get("content_fingerprint_before"), f"{name}.content_fingerprint_before")
    after_hash = sha(probe.get("content_fingerprint_after"), f"{name}.content_fingerprint_after")
    before_etag = etag(probe.get("etag_before"), f"{name}.etag_before")
    after_etag = etag(probe.get("etag_after"), f"{name}.etag_after")
    if before_hash != after_hash or before_etag != after_etag:
        fail(f"{name}: immutable version content hash and ETag must remain stable")
    if probe.get("conditional_get_if_none_match") != before_etag or probe.get("conditional_get_http_status") != 304:
        fail(f"{name}: ETag conditional GET 304 evidence required")


def check_route_non_effect(value: Any, name: str) -> None:
    item = obj(value, name)
    exact_keys(item, {"before_fingerprint", "after_fingerprint", "changed_route_count"}, name)
    before = sha(item.get("before_fingerprint"), f"{name}.before_fingerprint")
    after = sha(item.get("after_fingerprint"), f"{name}.after_fingerprint")
    if before != after:
        fail(f"{name}: publishing specs/tools changed Envoy route fingerprint")
    integer(item.get("changed_route_count"), f"{name}.changed_route_count", 0)


def check_internal_observations(value: Any, name: str) -> None:
    item = obj(value, name)
    exact_keys(item, {
        "write_mode", "session_counter_delta", "projected_into_generated_spec",
        "raw_read_surface_available",
    }, name)
    if item.get("write_mode") != "internal-only":
        fail(f"{name}: observations must be internal-only writes")
    if integer(item.get("session_counter_delta"), f"{name}.session_counter_delta") < 1:
        fail(f"{name}: session counter evidence required")
    boolean(item.get("projected_into_generated_spec"), f"{name}.projected_into_generated_spec", True)
    boolean(item.get("raw_read_surface_available"), f"{name}.raw_read_surface_available", False)


def check_lifecycle(e: dict[str, Any]) -> None:
    lifecycles = teams_indexed(field(e, "team_lifecycles"), "team_lifecycles")
    for key, lifecycle in lifecycles.items():
        exact_keys(lifecycle, {"team_key", "learned_v1", "learned_v2", "imported_spec", "discovery"},
                   f"team_lifecycles[{key}]")
        learned_v1 = obj(lifecycle.get("learned_v1"), f"team_lifecycles[{key}].learned_v1")
        exact_keys(learned_v1, {"review_state", "publish_http_status", "published", "immutability"},
                   f"team_lifecycles[{key}].learned_v1")
        if learned_v1.get("review_state") != "rejected" or learned_v1.get("publish_http_status") not in {403, 409}:
            fail(f"team_lifecycles[{key}].learned_v1: rejected version must be denied publication")
        boolean(learned_v1.get("published"), f"team_lifecycles[{key}].learned_v1.published", False)
        check_immutability(learned_v1.get("immutability"), f"team_lifecycles[{key}].learned_v1.immutability")

        learned_v2 = obj(lifecycle.get("learned_v2"), f"team_lifecycles[{key}].learned_v2")
        exact_keys(learned_v2, {"review_state", "published", "generated_tool_rows_present", "dynamic_catalog_present",
                                "observations", "immutability", "publication_route_observation"},
                   f"team_lifecycles[{key}].learned_v2")
        if learned_v2.get("review_state") != "approved":
            fail(f"team_lifecycles[{key}].learned_v2: approved review state required")
        for attr in ("published", "generated_tool_rows_present", "dynamic_catalog_present"):
            boolean(learned_v2.get(attr), f"team_lifecycles[{key}].learned_v2.{attr}", True)
        check_internal_observations(learned_v2.get("observations"), f"team_lifecycles[{key}].learned_v2.observations")
        check_immutability(learned_v2.get("immutability"), f"team_lifecycles[{key}].learned_v2.immutability")
        check_route_non_effect(learned_v2.get("publication_route_observation"), f"team_lifecycles[{key}].learned_v2.publication_route_observation")

        imported = obj(lifecycle.get("imported_spec"), f"team_lifecycles[{key}].imported_spec")
        exact_keys(imported, {"published", "internal_tool_rows_present_immediately", "dynamic_catalog_present",
                              "reject_http_status", "reject_error", "reject_reason", "content_fingerprint",
                              "tool_generation_route_observation"}, f"team_lifecycles[{key}].imported_spec")
        boolean(imported.get("published"), f"team_lifecycles[{key}].imported_spec.published", False)
        boolean(imported.get("internal_tool_rows_present_immediately"), f"team_lifecycles[{key}].imported_spec.internal_tool_rows_present_immediately", True)
        boolean(imported.get("dynamic_catalog_present"), f"team_lifecycles[{key}].imported_spec.dynamic_catalog_present", False)
        if imported.get("reject_http_status") != 400 or imported.get("reject_error") != "validation_failed" or imported.get("reject_reason") != "rejection_is_learned_only":
            fail(f"team_lifecycles[{key}].imported_spec: learned-only rejection must return validation_failed")
        sha(imported.get("content_fingerprint"), f"team_lifecycles[{key}].imported_spec.content_fingerprint")
        check_route_non_effect(imported.get("tool_generation_route_observation"), f"team_lifecycles[{key}].imported_spec.tool_generation_route_observation")

        discovery = obj(lifecycle.get("discovery"), f"team_lifecycles[{key}].discovery")
        exact_keys(discovery, {"temporary_gateway_observed", "session_completed", "session_api_link_retained",
                               "spec_published", "generated_tool_rows_present", "dynamic_catalog_present",
                               "observations", "immutability", "publication_route_observation"},
                   f"team_lifecycles[{key}].discovery")
        for attr in ("temporary_gateway_observed", "session_completed", "spec_published", "generated_tool_rows_present", "dynamic_catalog_present"):
            boolean(discovery.get(attr), f"team_lifecycles[{key}].discovery.{attr}", True)
        boolean(discovery.get("session_api_link_retained"), f"team_lifecycles[{key}].discovery.session_api_link_retained", False)
        check_internal_observations(discovery.get("observations"), f"team_lifecycles[{key}].discovery.observations")
        check_immutability(discovery.get("immutability"), f"team_lifecycles[{key}].discovery.immutability")
        check_route_non_effect(discovery.get("publication_route_observation"), f"team_lifecycles[{key}].discovery.publication_route_observation")


def check_route_generation(e: dict[str, Any]) -> None:
    rows = teams_indexed(field(e, "route_generation"), "route_generation")
    for key, row in rows.items():
        exact_keys(row, {"team_key", "source_kind", "source_spec_published", "dry_run", "apply"},
                   f"route_generation[{key}]")
        if row.get("source_kind") != "discovery_spec" or row.get("source_spec_published") is not True:
            fail(f"route_generation[{key}]: plan must originate from a published discovery spec")
        if "tool_ref" in row or "generated_tool_ref" in row:
            fail(f"route_generation[{key}]: d23.7 route plan cannot originate from a tool")
        dry = obj(row.get("dry_run"), f"route_generation[{key}].dry_run")
        exact_keys(dry, {"plan_returned", "validation_passed", "state_unchanged", "created_resource_count",
                         "successful_traffic_probes", "plan_fingerprint", "planned_resource_refs"},
                   f"route_generation[{key}].dry_run")
        plan = sha(dry.get("plan_fingerprint"), f"route_generation[{key}].dry_run.plan_fingerprint")
        boolean(dry.get("plan_returned"), f"route_generation[{key}].dry_run.plan_returned", True)
        boolean(dry.get("validation_passed"), f"route_generation[{key}].dry_run.validation_passed", True)
        boolean(dry.get("state_unchanged"), f"route_generation[{key}].dry_run.state_unchanged", True)
        integer(dry.get("created_resource_count"), f"route_generation[{key}].dry_run.created_resource_count", 0)
        integer(dry.get("successful_traffic_probes"), f"route_generation[{key}].dry_run.successful_traffic_probes", 0)

        applied = obj(row.get("apply"), f"route_generation[{key}].apply")
        exact_keys(applied, {"plan_fingerprint", "created_resource_refs", "created_resource_count", "route_active",
                             "successful_traffic_probes", "sibling_team_changes", "foreign_org_changes"},
                   f"route_generation[{key}].apply")
        if applied.get("plan_fingerprint") != plan:
            fail(f"route_generation[{key}].apply: must apply the exact dry-run plan")
        expected = sorted(seq(dry.get("planned_resource_refs"), f"route_generation[{key}].dry_run.planned_resource_refs"))
        actual = sorted(seq(applied.get("created_resource_refs"), f"route_generation[{key}].apply.created_resource_refs"))
        if any(not isinstance(ref, str) or not REF.fullmatch(ref) for ref in expected + actual):
            fail(f"route_generation[{key}]: sanitized resource references required")
        if not expected or expected != actual or len(actual) != len(set(actual)):
            fail(f"route_generation[{key}].apply: exact planned resources must be created")
        integer(applied.get("created_resource_count"), f"route_generation[{key}].apply.created_resource_count", len(actual))
        boolean(applied.get("route_active"), f"route_generation[{key}].apply.route_active", True)
        if integer(applied.get("successful_traffic_probes"), f"route_generation[{key}].apply.successful_traffic_probes") < 1:
            fail(f"route_generation[{key}].apply: real traffic success required")
        integer(applied.get("sibling_team_changes"), f"route_generation[{key}].apply.sibling_team_changes", 0)
        integer(applied.get("foreign_org_changes"), f"route_generation[{key}].apply.foreign_org_changes", 0)


def check_isolation(e: dict[str, Any]) -> None:
    isolation = obj(field(e, "isolation"), "isolation")
    exact_keys(isolation, {"teams", "cross_org_probe"}, "isolation")
    teams = teams_indexed(isolation.get("teams"), "isolation.teams")
    for key, item in teams.items():
        exact_keys(item, {"team_key", "own_api_definitions", "foreign_api_definitions",
                          "own_generated_tool_api_ids", "foreign_generated_tool_api_ids",
                          "own_learning_sessions", "own_discovery_sessions",
                          "foreign_listener_ports_active", "foreign_dataplanes"}, f"isolation.teams[{key}]")
        integer(item.get("own_api_definitions"), f"isolation.teams[{key}].own_api_definitions", 3)
        integer(item.get("foreign_api_definitions"), f"isolation.teams[{key}].foreign_api_definitions", 0)
        integer(item.get("own_generated_tool_api_ids"), f"isolation.teams[{key}].own_generated_tool_api_ids", 3)
        integer(item.get("foreign_generated_tool_api_ids"), f"isolation.teams[{key}].foreign_generated_tool_api_ids", 0)
        integer(item.get("own_learning_sessions"), f"isolation.teams[{key}].own_learning_sessions", 2)
        integer(item.get("own_discovery_sessions"), f"isolation.teams[{key}].own_discovery_sessions", 1)
        integer(item.get("foreign_listener_ports_active"), f"isolation.teams[{key}].foreign_listener_ports_active", 0)
        integer(item.get("foreign_dataplanes"), f"isolation.teams[{key}].foreign_dataplanes", 0)
    probe = obj(isolation.get("cross_org_probe"), "isolation.cross_org_probe")
    exact_keys(probe, {"actor_role", "target_org", "http_status", "error", "resource_visible", "state_changed"}, "isolation.cross_org_probe")
    if probe.get("actor_role") != "beta_owner" or probe.get("target_org") != "alpha":
        fail("isolation.cross_org_probe: exact Beta-to-Alpha probe required")
    if probe.get("http_status") != 400 or probe.get("error") != "org_selector_required":
        fail("isolation.cross_org_probe: expected fail-closed selector denial")
    boolean(probe.get("resource_visible"), "isolation.cross_org_probe.resource_visible", False)
    boolean(probe.get("state_changed"), "isolation.cross_org_probe.state_changed", False)


def check_evidence_limitations(e: dict[str, Any]) -> None:
    limitations = indexed(field(e, "evidence_limitations"), "evidence_limitations")
    if set(limitations) != {"alpha_payments_preapply_absence"}:
        fail("evidence_limitations: exact known evidence limitation required")
    item = limitations["alpha_payments_preapply_absence"]
    exact_keys(item, {"id", "team_key", "observed_in_execution_order", "frozen_file_snapshot_available",
                      "snapshot_overwritten_after_apply", "presented_as_frozen_proof", "disposition"},
               "evidence_limitations[alpha_payments_preapply_absence]")
    if item.get("team_key") != "alpha/payments":
        fail("evidence_limitations[alpha_payments_preapply_absence]: exact affected team required")
    boolean(item.get("observed_in_execution_order"), "evidence_limitations[alpha_payments_preapply_absence].observed_in_execution_order", True)
    boolean(item.get("frozen_file_snapshot_available"), "evidence_limitations[alpha_payments_preapply_absence].frozen_file_snapshot_available", False)
    boolean(item.get("snapshot_overwritten_after_apply"), "evidence_limitations[alpha_payments_preapply_absence].snapshot_overwritten_after_apply", True)
    boolean(item.get("presented_as_frozen_proof"), "evidence_limitations[alpha_payments_preapply_absence].presented_as_frozen_proof", False)
    if item.get("disposition") != "explicit_evidence_limitation":
        fail("evidence_limitations[alpha_payments_preapply_absence]: explicit limitation disposition required")


def zero(item: dict[str, Any], attr: str, name: str) -> None:
    integer(item.get(attr), f"{name}.{attr}", 0)


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(field(e, "cleanup"), "cleanup")
    exact_keys(cleanup, {"cascade_exercised_via_supported_api", "teams", "vm_disks_expected",
                         "vm_disks_remaining", "tailscale_provider_residue"}, "cleanup")
    boolean(cleanup.get("cascade_exercised_via_supported_api"), "cleanup.cascade_exercised_via_supported_api", True)
    per_team = teams_indexed(cleanup.get("teams"), "cleanup.teams")
    for key, item in per_team.items():
        exact_keys(item, {"team_key", "api_definitions", "api_bindings", "specs", "generated_tools",
                          "learned_sessions", "generated_runtime_resources", "baseline_runtime_resources",
                          "active_dataplanes", "certificates", "completed_unlinked_discovery_history",
                          "retired_dataplane_tombstones", "retained_history_inert"}, f"cleanup.teams[{key}]")
        for attr in (
            "api_definitions", "api_bindings", "specs", "generated_tools", "learned_sessions",
            "generated_runtime_resources", "baseline_runtime_resources", "active_dataplanes", "certificates",
        ):
            zero(item, attr, f"cleanup.teams[{key}]")
        integer(item.get("completed_unlinked_discovery_history"), f"cleanup.teams[{key}].completed_unlinked_discovery_history", 1)
        integer(item.get("retired_dataplane_tombstones"), f"cleanup.teams[{key}].retired_dataplane_tombstones", 1)
        boolean(item.get("retained_history_inert"), f"cleanup.teams[{key}].retained_history_inert", True)
    integer(cleanup.get("vm_disks_remaining"), "cleanup.vm_disks_remaining", 0)
    integer(cleanup.get("vm_disks_expected"), "cleanup.vm_disks_expected", 4)

    residue = obj(cleanup.get("tailscale_provider_residue"), "cleanup.tailscale_provider_residue")
    exact_keys(residue, {"remaining_count", "records", "status", "cleanup_gate_passed", "acceptance_claim_made"},
               "cleanup.tailscale_provider_residue")
    count = integer(residue.get("remaining_count"), "cleanup.tailscale_provider_residue.remaining_count")
    records_raw = seq(residue.get("records"), "cleanup.tailscale_provider_residue.records")
    records: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(records_raw):
        record = obj(raw, f"cleanup.tailscale_provider_residue.records[{index}]")
        key = text(record.get("team_key"), f"cleanup.tailscale_provider_residue.records[{index}].team_key")
        if key not in TEAM_KEYS or key in records:
            fail("cleanup.tailscale_provider_residue.records: unique known team required")
        records[key] = record
    if count != len(records):
        fail("cleanup.tailscale_provider_residue: count/record mismatch")
    for key, record in records.items():
        exact_keys(record, {"team_key", "ephemeral", "offline", "active"},
                   f"cleanup.tailscale_provider_residue.records[{key}]")
        boolean(record.get("ephemeral"), f"cleanup.tailscale_provider_residue.records[{key}].ephemeral", True)
        boolean(record.get("offline"), f"cleanup.tailscale_provider_residue.records[{key}].offline", True)
        boolean(record.get("active"), f"cleanup.tailscale_provider_residue.records[{key}].active", False)
    if count:
        if residue.get("status") != "unresolved_residue" or residue.get("cleanup_gate_passed") is not False or residue.get("acceptance_claim_made") is not False:
            fail("cleanup.tailscale_provider_residue: residue must be explicit and cannot claim cleanup acceptance")
        fail(f"cleanup: unresolved residue: {count} offline/inactive ephemeral Tailscale provider records remain")
    if residue.get("status") != "clear" or residue.get("cleanup_gate_passed") is not True or residue.get("acceptance_claim_made") is not False:
        fail("cleanup.tailscale_provider_residue: zero residue must be explicitly clear")


def walk(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key.lower() in PROHIBITED_KEYS:
                fail(f"redaction: prohibited/out-of-scope key {path}.{key}")
            walk(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            walk(child, f"{path}[{index}]")
    elif isinstance(value, str):
        for pattern in SECRET_PATTERNS:
            if pattern.search(value):
                fail(f"redaction: prohibited value at {path}")


def check_redaction(e: dict[str, Any]) -> None:
    equal(e, "redaction.sanitized_projection", True)
    equal(e, "redaction.raw_payloads_recorded", False)
    equal(e, "redaction.raw_identifiers_recorded", False)
    equal(e, "redaction.private_paths_recorded", False)
    equal(e, "redaction.scan_after_final_edit", True)
    equal(e, "redaction.undisposed_match_count", 0)
    walk(e)


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.7-RUN": ("sanitized supported-surface four-team run", check_run),
    "FPV2-D23.7-INVENTORY": ("exact approved capability inventory", check_inventory),
    "FPV2-D23.7-LIFECYCLE": ("actual learned/imported/discovery lifecycle and immutable GET evidence", check_lifecycle),
    "FPV2-D23.7-ROUTE-GENERATION": ("discovery-spec dry-run/apply with exact resources and real traffic", check_route_generation),
    "FPV2-D23.7-ISOLATION": ("four-team lifecycle reads, tool/session visibility and cross-org denial", check_isolation),
    "FPV2-D23.7-EVIDENCE-LIMITS": ("explicit alpha/payments pre-apply snapshot limitation", check_evidence_limitations),
    "FPV2-D23.7-CLEANUP": ("cascade cleanup with provider residue fail gate", check_cleanup),
    "FPV2-D23.7-REDACTION": ("strong redaction and d23.8 invocation exclusion", check_redaction),
}


def fingerprint(number: int) -> str:
    return "sha256:" + f"{number:064x}"


def immutability(number: int) -> dict[str, Any]:
    content = fingerprint(number)
    tag = f'"{fingerprint(number + 100)}"'
    return {
        "content_fingerprint_before": content,
        "content_fingerprint_after": content,
        "etag_before": tag,
        "etag_after": tag,
        "conditional_get_if_none_match": tag,
        "conditional_get_http_status": 304,
    }


def no_route_change(number: int) -> dict[str, Any]:
    state = fingerprint(number)
    return {"before_fingerprint": state, "after_fingerprint": state, "changed_route_count": 0}


def fixture() -> dict[str, Any]:
    inventory = [{"capability": capability, "support_status": status} for capability, status in APPROVED_INVENTORY.items()]
    lifecycles: list[dict[str, Any]] = []
    routes: list[dict[str, Any]] = []
    cleanup_teams: list[dict[str, Any]] = []
    for index, key in enumerate(TEAM_KEYS, 1):
        lifecycles.append({
            "team_key": key,
            "learned_v1": {"review_state": "rejected", "publish_http_status": 409, "published": False, "immutability": immutability(index * 10 + 1)},
            "learned_v2": {"review_state": "approved", "published": True, "generated_tool_rows_present": True,
                           "dynamic_catalog_present": True, "immutability": immutability(index * 10 + 2),
                           "observations": {"write_mode": "internal-only", "session_counter_delta": 2,
                                            "projected_into_generated_spec": True, "raw_read_surface_available": False},
                           "publication_route_observation": no_route_change(index * 10 + 3)},
            "imported_spec": {"published": False, "internal_tool_rows_present_immediately": True, "dynamic_catalog_present": False,
                              "reject_http_status": 400, "reject_error": "validation_failed", "reject_reason": "rejection_is_learned_only",
                              "content_fingerprint": fingerprint(index * 10 + 4), "tool_generation_route_observation": no_route_change(index * 10 + 5)},
            "discovery": {"temporary_gateway_observed": True, "session_completed": True, "session_api_link_retained": False,
                          "spec_published": True, "generated_tool_rows_present": True, "dynamic_catalog_present": True,
                          "observations": {"write_mode": "internal-only", "session_counter_delta": 2,
                                           "projected_into_generated_spec": True, "raw_read_surface_available": False},
                          "immutability": immutability(index * 10 + 6), "publication_route_observation": no_route_change(index * 10 + 7)},
        })
        plan = fingerprint(200 + index)
        resources = [f"route_{index}", f"cluster_{index}"]
        routes.append({
            "team_key": key, "source_kind": "discovery_spec", "source_spec_published": True,
            "dry_run": {"plan_returned": True, "validation_passed": True, "state_unchanged": True,
                        "created_resource_count": 0, "successful_traffic_probes": 0,
                        "plan_fingerprint": plan, "planned_resource_refs": resources},
            "apply": {"plan_fingerprint": plan, "created_resource_refs": resources, "created_resource_count": 2,
                      "route_active": True, "successful_traffic_probes": 1, "sibling_team_changes": 0, "foreign_org_changes": 0},
        })
        cleanup_teams.append({
            "team_key": key, "api_definitions": 0, "api_bindings": 0, "specs": 0, "generated_tools": 0,
            "learned_sessions": 0, "generated_runtime_resources": 0, "baseline_runtime_resources": 0,
            "active_dataplanes": 0, "certificates": 0, "completed_unlinked_discovery_history": 1,
            "retired_dataplane_tombstones": 1, "retained_history_inert": True,
        })
    return {
        "schema": SCHEMA,
        "run": {"synthetic_only": True, "rerunnable": True, "supported_surfaces_only": True,
                "direct_database_access": False, "independent_acceptance_author_read_implementation": False,
                "sanitized_projection_contains_private_values": False,
                "started_at_utc": "2026-08-21T00:00:00Z", "finished_at_utc": "2026-08-21T01:00:00Z",
                "teams": [{"team_key": key, "marker": f"fpd237-marker-{index:02d}", "created_via_supported_api": True}
                          for index, key in enumerate(TEAM_KEYS, 1)]},
        "inventory": inventory,
        "team_lifecycles": lifecycles,
        "route_generation": routes,
        "isolation": {"teams": [{"team_key": key, "own_api_definitions": 3, "foreign_api_definitions": 0,
                                   "own_generated_tool_api_ids": 3, "foreign_generated_tool_api_ids": 0,
                                   "own_learning_sessions": 2, "own_discovery_sessions": 1,
                                   "foreign_listener_ports_active": 0, "foreign_dataplanes": 0}
                                  for key in TEAM_KEYS],
                      "cross_org_probe": {"actor_role": "beta_owner", "target_org": "alpha", "http_status": 400,
                                          "error": "org_selector_required", "resource_visible": False, "state_changed": False}},
        "evidence_limitations": [{"id": "alpha_payments_preapply_absence", "team_key": "alpha/payments",
                                  "observed_in_execution_order": True, "frozen_file_snapshot_available": False,
                                  "snapshot_overwritten_after_apply": True, "presented_as_frozen_proof": False,
                                  "disposition": "explicit_evidence_limitation"}],
        "cleanup": {"cascade_exercised_via_supported_api": True, "teams": cleanup_teams,
                    "vm_disks_expected": 4, "vm_disks_remaining": 0,
                    "tailscale_provider_residue": {"remaining_count": 0, "records": [], "status": "clear", "cleanup_gate_passed": True,
                                                      "acceptance_claim_made": False}},
        "redaction": {"sanitized_projection": True, "raw_payloads_recorded": False,
                      "raw_identifiers_recorded": False, "private_paths_recorded": False,
                      "scan_after_final_edit": True, "undisposed_match_count": 0},
    }


def provider_residue(e: dict[str, Any]) -> None:
    residue = e["cleanup"]["tailscale_provider_residue"]
    residue.update(remaining_count=4, status="unresolved_residue", cleanup_gate_passed=False, acceptance_claim_made=False)
    residue["records"] = [{"team_key": key, "ephemeral": True, "offline": True, "active": False} for key in TEAM_KEYS]


def self_test() -> int:
    good = fixture()
    failures: list[str] = []
    for scenario_id, (_, check) in SCENARIOS.items():
        try:
            check(good)
        except ContractFailure as error:
            failures.append(f"valid fixture rejected by {scenario_id}: {error}")

    mutations: list[tuple[str, Callable[[dict[str, Any]], Any], str]] = [
        ("missing fourth team", lambda x: x["run"]["teams"].pop(), "FPV2-D23.7-RUN"),
        ("invented observation read support", lambda x: x["inventory"][8].__setitem__("support_status", "supported"), "FPV2-D23.7-INVENTORY"),
        ("generated tool invocation promoted", lambda x: x["inventory"][-1].__setitem__("support_status", "supported"), "FPV2-D23.7-INVENTORY"),
        ("conditional update claim", lambda x: x["team_lifecycles"][0]["learned_v1"]["immutability"].__setitem__("conditional_update_attempted", True), "FPV2-D23.7-LIFECYCLE"),
        ("immutable hash changed", lambda x: x["team_lifecycles"][0]["learned_v2"]["immutability"].__setitem__("content_fingerprint_after", fingerprint(999)), "FPV2-D23.7-LIFECYCLE"),
        ("imported draft enters catalog", lambda x: x["team_lifecycles"][0]["imported_spec"].__setitem__("dynamic_catalog_present", True), "FPV2-D23.7-LIFECYCLE"),
        ("imported rejection accepted", lambda x: x["team_lifecycles"][0]["imported_spec"].__setitem__("reject_http_status", 200), "FPV2-D23.7-LIFECYCLE"),
        ("raw learning observations exposed", lambda x: x["team_lifecycles"][0]["learned_v2"]["observations"].__setitem__("raw_read_surface_available", True), "FPV2-D23.7-LIFECYCLE"),
        ("discovery projection absent", lambda x: x["team_lifecycles"][0]["discovery"]["observations"].__setitem__("projected_into_generated_spec", False), "FPV2-D23.7-LIFECYCLE"),
        ("publication changes routes", lambda x: x["team_lifecycles"][0]["discovery"]["publication_route_observation"].__setitem__("changed_route_count", 1), "FPV2-D23.7-LIFECYCLE"),
        ("route sourced from tool", lambda x: x["route_generation"][0].__setitem__("tool_ref", "tool_1"), "FPV2-D23.7-ROUTE-GENERATION"),
        ("dry-run mutation", lambda x: x["route_generation"][0]["dry_run"].__setitem__("created_resource_count", 1), "FPV2-D23.7-ROUTE-GENERATION"),
        ("apply resource mismatch", lambda x: x["route_generation"][0]["apply"]["created_resource_refs"].pop(), "FPV2-D23.7-ROUTE-GENERATION"),
        ("foreign API visible", lambda x: x["isolation"]["teams"][0].__setitem__("foreign_api_definitions", 1), "FPV2-D23.7-ISOLATION"),
        ("cross-org denial changed state", lambda x: x["isolation"]["cross_org_probe"].__setitem__("state_changed", True), "FPV2-D23.7-ISOLATION"),
        ("missing real traffic", lambda x: x["route_generation"][0]["apply"].__setitem__("successful_traffic_probes", 0), "FPV2-D23.7-ROUTE-GENERATION"),
        ("pre-apply snapshot overclaimed", lambda x: x["evidence_limitations"][0].__setitem__("presented_as_frozen_proof", True), "FPV2-D23.7-EVIDENCE-LIMITS"),
        ("retained discovery history omitted", lambda x: x["cleanup"]["teams"][0].__setitem__("completed_unlinked_discovery_history", 0), "FPV2-D23.7-CLEANUP"),
        ("VM disk remains", lambda x: x["cleanup"].__setitem__("vm_disks_remaining", 1), "FPV2-D23.7-CLEANUP"),
        ("provider residue", provider_residue, "FPV2-D23.7-CLEANUP"),
        ("provider residue passed silently", lambda x: (provider_residue(x), x["cleanup"]["tailscale_provider_residue"].__setitem__("cleanup_gate_passed", True)), "FPV2-D23.7-CLEANUP"),
        ("tool invocation evidence", lambda x: x.__setitem__("tool_invocation", {"claimed": True}), "FPV2-D23.7-REDACTION"),
        ("private path", lambda x: x.__setitem__("note", "/Users/example/private.json"), "FPV2-D23.7-REDACTION"),
    ]
    for label, mutate, scenario_id in mutations:
        bad = copy.deepcopy(good)
        mutate(bad)
        try:
            SCENARIOS[scenario_id][1](bad)
        except ContractFailure:
            pass
        else:
            failures.append(f"negative mutation did not fail closed: {label}")
    if failures:
        for failure in failures:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        print(f"api lifecycle acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"api lifecycle acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
    return 0


def load_evidence(path: Path) -> dict[str, Any]:
    if not path.is_file():
        fail(f"sanitized evidence absent: {path.resolve()}")
    try:
        return obj(json.loads(path.read_text(encoding="utf-8")), "evidence root")
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"sanitized evidence unreadable: {error}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", help="sanitized api-lifecycle v2 JSON projection")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), help="run one scenario")
    parser.add_argument("--list", action="store_true", help="list scenario IDs")
    parser.add_argument("--self-test", action="store_true", help="run synthetic adversarial harness tests")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list:
        for scenario_id, (description, _) in SCENARIOS.items():
            print(f"{scenario_id}\t{description}")
        return 0
    if args.self_test:
        return self_test()
    path = Path(args.evidence or os.environ.get("FLOWPLANE_API_LIFECYCLE_EVIDENCE", DEFAULT_EVIDENCE))
    selected = [args.scenario] if args.scenario else list(SCENARIOS)
    try:
        evidence = load_evidence(path)
    except ContractFailure as error:
        print(f"api lifecycle acceptance: FAIL: {error}", file=sys.stderr)
        return 1
    failures = 0
    for scenario_id in selected:
        description, check = SCENARIOS[scenario_id]
        try:
            check(evidence)
        except ContractFailure as error:
            failures += 1
            print(f"{scenario_id}: FAIL: {error}", file=sys.stderr)
        else:
            print(f"{scenario_id}: PASS: {description}")
    if failures:
        print(f"api lifecycle acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"api lifecycle acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
