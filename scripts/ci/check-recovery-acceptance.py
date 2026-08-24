#!/usr/bin/env python3
"""Independent, fail-closed fpv2-d23.10 recovery acceptance gate.

Consumes only a sanitized ``flowplane.qualification.recovery/v1`` JSON
projection. The live producer owns fault injection and recovery; this checker
validates the externally observable contract without reading implementation,
raw logs, credentials, private evidence, or provider state. ``--self-test``
exercises a valid synthetic projection and adversarial fail-closed mutations.
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

SCHEMA = "flowplane.qualification.recovery/v1"
DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.10/recovery.json")
TEAMS = ("alpha/payments", "alpha/shared", "beta/payments", "beta/shared")
PROCESS_EFFECTS = {
    "envoy": (False, True, False),
    "agent": (True, True, True),
    "upstream": (False, False, True),
}
SUITES = {
    "xds_nack_certificate": (11, 11, 114),
    "api_auth_mcp": (30, 30, 217),
}
LIMITATIONS = {
    "tailscale_traffic_survival": ("topology-inapplicable", None),
    "shared_auth0_outage": ("not-live", None),
    "destructive_token_expiry_clock_manipulation": ("not-live", None),
    "host_laptop_wide_network_disruption": ("not-live", None),
    "malformed_nack_certificate_beyond_focused_suites": ("focused-suite-only", None),
    "rls_policy_behavior": ("carry-forward", "fpv2-d23.6"),
    "ai_provider_recovery": ("carry-forward", "fpv2-d23.8"),
    "database_agent_health_during_stop": ("not-measured", None),
    "dashboard_readiness_during_faults": ("not-live", None),
}
CLEANUP_RESOURCES = {"run_resources", "vms", "tailscale_nodes", "local_credentials"}
PROHIBITED_KEYS = {
    "token", "access_token", "refresh_token", "authorization", "password", "secret",
    "secret_value", "private_key", "certificate_pem", "tailscale_key", "auth_key",
    "credential", "raw_request", "raw_response", "raw_body", "raw_log", "raw_identifier",
    "private_evidence_path", "provider_payload",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~+/-]{8,}"),
    re.compile(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b"),
    re.compile(r"(?i)(?:sk|tskey)-[A-Za-z0-9_-]{8,}"),
    re.compile(r"(?i)(?:password|client_secret|api_key)\s*[:=]\s*\S+"),
    re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\\\Users\\\\)"),
    re.compile(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b"),
    re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b"),
)
UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REF = re.compile(r"^[a-z][a-z0-9_-]{2,63}$")
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")


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


def timestamp(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not UTC.fullmatch(candidate):
        fail(f"{name}: second-precision UTC timestamp required")
    return candidate


def safe_ref(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not REF.fullmatch(candidate):
        fail(f"{name}: sanitized reference required")
    return candidate


def fingerprint(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not SHA256.fullmatch(candidate):
        fail(f"{name}: sha256 fingerprint required")
    return candidate


def indexed(value: Any, name: str, key: str = "id") -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(seq(value, name)):
        item = obj(raw, f"{name}[{index}]")
        item_key = text(item.get(key), f"{name}[{index}].{key}")
        if item_key in result:
            fail(f"{name}: duplicate {key} {item_key!r}")
        result[item_key] = item
    return result


def team_index(value: Any, name: str) -> dict[str, dict[str, Any]]:
    rows = indexed(value, name, "team_key")
    if set(rows) != set(TEAMS):
        fail(f"{name}: exact Alpha/Beta x Payments/Shared fixture required; got {sorted(rows)!r}")
    return rows


def team_observations(value: Any, name: str, *, traffic: bool, agent: bool) -> None:
    for team, row in team_index(value, name).items():
        row_name = f"{name}[{team}]"
        exact_keys(row, {"team_key", "lkg_traffic_available", "agent_healthy"}, row_name)
        boolean(row.get("lkg_traffic_available"), f"{row_name}.lkg_traffic_available", traffic)
        boolean(row.get("agent_healthy"), f"{row_name}.agent_healthy", agent)


def check_run(e: dict[str, Any]) -> None:
    exact_keys(e, {"schema", "run", "baseline", "control_plane", "database", "tailscale",
                   "local_process_faults", "rls", "focused_suites", "limitations", "cleanup", "redaction"},
               "evidence root")
    equal(e, "schema", SCHEMA)
    run = obj(field(e, "run"), "run")
    exact_keys(run, {"live_qualification", "rerunnable", "sanitized_projection", "supported_surfaces_only",
                     "direct_database_access", "independent_author_read_implementation", "started_at_utc",
                     "finished_at_utc", "team_keys"}, "run")
    for key in ("live_qualification", "rerunnable", "sanitized_projection", "supported_surfaces_only"):
        boolean(run.get(key), f"run.{key}", True)
    for key in ("direct_database_access", "independent_author_read_implementation"):
        boolean(run.get(key), f"run.{key}", False)
    started = timestamp(run.get("started_at_utc"), "run.started_at_utc")
    finished = timestamp(run.get("finished_at_utc"), "run.finished_at_utc")
    if finished <= started:
        fail("run: finish must post-date start")
    if run.get("team_keys") != list(TEAMS):
        fail("run.team_keys: exact ordered four-team fixture required")


def check_baseline(e: dict[str, Any]) -> None:
    refs: set[str] = set()
    identities: set[str] = set()
    for team, row in team_index(field(e, "baseline"), "baseline").items():
        name = f"baseline[{team}]"
        exact_keys(row, {"team_key", "vm_ref", "fresh_vm", "certificate_bound", "node_identity_fingerprint",
                         "config_fingerprint", "traffic_available", "agent_healthy"}, name)
        refs.add(safe_ref(row.get("vm_ref"), f"{name}.vm_ref"))
        identities.add(fingerprint(row.get("node_identity_fingerprint"), f"{name}.node_identity_fingerprint"))
        fingerprint(row.get("config_fingerprint"), f"{name}.config_fingerprint")
        for key in ("fresh_vm", "certificate_bound", "traffic_available", "agent_healthy"):
            boolean(row.get(key), f"{name}.{key}", True)
    if len(refs) != 4 or len(identities) != 4:
        fail("baseline: four distinct fresh VM references and node identities required")


def check_control_plane(e: dict[str, Any]) -> None:
    cp = obj(field(e, "control_plane"), "control_plane")
    exact_keys(cp, {"live_service_stop", "machine_ref_before", "machine_ref_after", "during_stop",
                    "initial_restart", "human_rotation", "recovery"}, "control_plane")
    boolean(cp.get("live_service_stop"), "control_plane.live_service_stop", True)
    before = safe_ref(cp.get("machine_ref_before"), "control_plane.machine_ref_before")
    after = safe_ref(cp.get("machine_ref_after"), "control_plane.machine_ref_after")
    if before != after:
        fail("control_plane: recovery must use the exact same machine")
    team_observations(cp.get("during_stop"), "control_plane.during_stop", traffic=True, agent=False)
    restart = obj(cp.get("initial_restart"), "control_plane.initial_restart")
    exact_keys(restart, {"attempted_on_same_machine", "recovered", "failure_reason", "classification"}, "control_plane.initial_restart")
    boolean(restart.get("attempted_on_same_machine"), "control_plane.initial_restart.attempted_on_same_machine", True)
    boolean(restart.get("recovered"), "control_plane.initial_restart.recovered", False)
    if restart.get("failure_reason") != "staged_tailscale_key_invalid":
        fail("control_plane.initial_restart.failure_reason: staged invalid Tailscale key required")
    if restart.get("classification") != "environment_staging":
        fail("control_plane.initial_restart.classification: environment/staging classification required")
    rotation = obj(cp.get("human_rotation"), "control_plane.human_rotation")
    exact_keys(rotation, {"performed", "rotated_key_scopes", "other_credentials_rotated"}, "control_plane.human_rotation")
    boolean(rotation.get("performed"), "control_plane.human_rotation.performed", True)
    if rotation.get("rotated_key_scopes") != ["control_plane", "rls"]:
        fail("control_plane.human_rotation.rotated_key_scopes: only CP/RLS keys may rotate")
    boolean(rotation.get("other_credentials_rotated"), "control_plane.human_rotation.other_credentials_rotated", False)
    recovery = obj(cp.get("recovery"), "control_plane.recovery")
    exact_keys(recovery, {"same_machine", "replacement_used", "control_plane_ready", "teams"}, "control_plane.recovery")
    boolean(recovery.get("same_machine"), "control_plane.recovery.same_machine", True)
    boolean(recovery.get("replacement_used"), "control_plane.recovery.replacement_used", False)
    boolean(recovery.get("control_plane_ready"), "control_plane.recovery.control_plane_ready", True)
    team_observations(recovery.get("teams"), "control_plane.recovery.teams", traffic=True, agent=True)


def check_database(e: dict[str, Any]) -> None:
    db = obj(field(e, "database"), "database")
    exact_keys(db, {"live_same_machine_stop", "machine_ref_before", "machine_ref_after", "during_stop", "recovery"}, "database")
    boolean(db.get("live_same_machine_stop"), "database.live_same_machine_stop", True)
    if safe_ref(db.get("machine_ref_before"), "database.machine_ref_before") != safe_ref(db.get("machine_ref_after"), "database.machine_ref_after"):
        fail("database: recovery must use the exact same machine")
    during = obj(db.get("during_stop"), "database.during_stop")
    exact_keys(during, {"control_plane_readiness_degraded", "teams"}, "database.during_stop")
    boolean(during.get("control_plane_readiness_degraded"), "database.during_stop.control_plane_readiness_degraded", True)
    for team, row in team_index(during.get("teams"), "database.during_stop.teams").items():
        name = f"database.during_stop.teams[{team}]"
        exact_keys(row, {"team_key", "lkg_traffic_available"}, name)
        boolean(row.get("lkg_traffic_available"), f"{name}.lkg_traffic_available", True)
    recovery = obj(db.get("recovery"), "database.recovery")
    exact_keys(recovery, {"same_machine", "replacement_used", "database_ready", "control_plane_ready"}, "database.recovery")
    for key in ("same_machine", "database_ready", "control_plane_ready"):
        boolean(recovery.get(key), f"database.recovery.{key}", True)
    boolean(recovery.get("replacement_used"), "database.recovery.replacement_used", False)


def check_tailscale(e: dict[str, Any]) -> None:
    ts = obj(field(e, "tailscale"), "tailscale")
    exact_keys(ts, {"live_one_vm_down_up", "team_key", "vm_ref_before", "vm_ref_after", "node_identity_before",
                    "node_identity_after", "agent_degraded_while_down", "agent_healthy_after_up", "replacement_used",
                    "auth_reenrollment_used", "controlled_upstream_used_same_vm_tailnet_ip", "traffic_survival_claimed", "limitation_id"}, "tailscale")
    boolean(ts.get("live_one_vm_down_up"), "tailscale.live_one_vm_down_up", True)
    if ts.get("team_key") not in TEAMS:
        fail("tailscale.team_key: one of the exact fixture teams required")
    if safe_ref(ts.get("vm_ref_before"), "tailscale.vm_ref_before") != safe_ref(ts.get("vm_ref_after"), "tailscale.vm_ref_after"):
        fail("tailscale: same VM must recover")
    if fingerprint(ts.get("node_identity_before"), "tailscale.node_identity_before") != fingerprint(ts.get("node_identity_after"), "tailscale.node_identity_after"):
        fail("tailscale: same node identity must recover")
    for key in ("agent_degraded_while_down", "agent_healthy_after_up", "controlled_upstream_used_same_vm_tailnet_ip"):
        boolean(ts.get(key), f"tailscale.{key}", True)
    for key in ("replacement_used", "auth_reenrollment_used", "traffic_survival_claimed"):
        boolean(ts.get(key), f"tailscale.{key}", False)
    if ts.get("limitation_id") != "tailscale_traffic_survival":
        fail("tailscale.limitation_id: explicit topology-inapplicable limitation required")


def check_process_faults(e: dict[str, Any]) -> None:
    faults = obj(field(e, "local_process_faults"), "local_process_faults")
    exact_keys(faults, {"live_faults", "team_key", "vm_ref", "faults"}, "local_process_faults")
    boolean(faults.get("live_faults"), "local_process_faults.live_faults", True)
    if faults.get("team_key") not in TEAMS:
        fail("local_process_faults.team_key: fixture team required")
    vm_ref = safe_ref(faults.get("vm_ref"), "local_process_faults.vm_ref")
    rows = indexed(faults.get("faults"), "local_process_faults.faults", "process")
    if set(rows) != set(PROCESS_EFFECTS):
        fail("local_process_faults.faults: exact Envoy/agent/upstream matrix required")
    for process, (traffic, degraded, envoy_live) in PROCESS_EFFECTS.items():
        row = rows[process]
        name = f"local_process_faults.faults[{process}]"
        exact_keys(row, {"process", "live_service_stop", "traffic_available_during_stop", "agent_degraded_during_stop",
                         "envoy_live_during_stop", "transient_systemd_unit", "new_pid_required", "exact_pids_recorded", "same_vm_ref",
                         "same_config", "same_identity", "replacement_used", "recovered"}, name)
        boolean(row.get("live_service_stop"), f"{name}.live_service_stop", True)
        boolean(row.get("traffic_available_during_stop"), f"{name}.traffic_available_during_stop", traffic)
        boolean(row.get("agent_degraded_during_stop"), f"{name}.agent_degraded_during_stop", degraded)
        boolean(row.get("envoy_live_during_stop"), f"{name}.envoy_live_during_stop", envoy_live)
        boolean(row.get("transient_systemd_unit"), f"{name}.transient_systemd_unit", True)
        boolean(row.get("new_pid_required"), f"{name}.new_pid_required", True)
        boolean(row.get("exact_pids_recorded"), f"{name}.exact_pids_recorded", False)
        if safe_ref(row.get("same_vm_ref"), f"{name}.same_vm_ref") != vm_ref:
            fail(f"{name}.same_vm_ref: exact fault VM required")
        for key in ("same_config", "same_identity", "recovered"):
            boolean(row.get(key), f"{name}.{key}", True)
        boolean(row.get("replacement_used"), f"{name}.replacement_used", False)


def check_rls(e: dict[str, Any]) -> None:
    rls = obj(field(e, "rls"), "rls")
    exact_keys(rls, {"live_same_machine_service_stop", "machine_ref_before", "machine_ref_after", "replacement_used",
                     "service_recovered", "non_rls_during_stop", "policy_behavior_live_tested", "policy_behavior_source"}, "rls")
    boolean(rls.get("live_same_machine_service_stop"), "rls.live_same_machine_service_stop", True)
    if safe_ref(rls.get("machine_ref_before"), "rls.machine_ref_before") != safe_ref(rls.get("machine_ref_after"), "rls.machine_ref_after"):
        fail("rls: service must recover on same machine")
    boolean(rls.get("replacement_used"), "rls.replacement_used", False)
    boolean(rls.get("service_recovered"), "rls.service_recovered", True)
    for team, row in team_index(rls.get("non_rls_during_stop"), "rls.non_rls_during_stop").items():
        name = f"rls.non_rls_during_stop[{team}]"
        exact_keys(row, {"team_key", "lkg_traffic_available", "control_plane_ready"}, name)
        boolean(row.get("lkg_traffic_available"), f"{name}.lkg_traffic_available", True)
        boolean(row.get("control_plane_ready"), f"{name}.control_plane_ready", True)
    boolean(rls.get("policy_behavior_live_tested"), "rls.policy_behavior_live_tested", False)
    if rls.get("policy_behavior_source") != "fpv2-d23.6":
        fail("rls.policy_behavior_source: explicit fpv2-d23.6 carry-forward required")


def check_suites(e: dict[str, Any]) -> None:
    rows = indexed(field(e, "focused_suites"), "focused_suites", "id")
    if set(rows) != set(SUITES):
        fail("focused_suites: exact xDS/NACK/certificate and API/auth/MCP suites required")
    for suite_id, (selected, passed, skipped) in SUITES.items():
        row = rows[suite_id]
        name = f"focused_suites[{suite_id}]"
        exact_keys(row, {"id", "focused_selector", "selected_count", "passed_count", "failed_count", "skipped_by_selector",
                         "full_suite_count_claimed", "evidence_class"}, name)
        boolean(row.get("focused_selector"), f"{name}.focused_selector", True)
        integer(row.get("selected_count"), f"{name}.selected_count", selected)
        integer(row.get("passed_count"), f"{name}.passed_count", passed)
        integer(row.get("failed_count"), f"{name}.failed_count", 0)
        integer(row.get("skipped_by_selector"), f"{name}.skipped_by_selector", skipped)
        boolean(row.get("full_suite_count_claimed"), f"{name}.full_suite_count_claimed", False)
        if row.get("evidence_class") != "focused":
            fail(f"{name}.evidence_class: focused evidence required")


def check_limitations(e: dict[str, Any]) -> None:
    rows = indexed(field(e, "limitations"), "limitations")
    if set(rows) != set(LIMITATIONS):
        fail(f"limitations: exact non-live/carry-forward inventory required; got {sorted(rows)!r}")
    for limitation_id, (status, source) in LIMITATIONS.items():
        row = rows[limitation_id]
        name = f"limitations[{limitation_id}]"
        exact_keys(row, {"id", "live_tested", "status", "carry_forward_source", "acceptance_claimed_live", "explicit_limitation"}, name)
        boolean(row.get("live_tested"), f"{name}.live_tested", False)
        if row.get("status") != status or row.get("carry_forward_source") != source:
            fail(f"{name}: expected status/source {status!r}/{source!r}")
        boolean(row.get("acceptance_claimed_live"), f"{name}.acceptance_claimed_live", False)
        boolean(row.get("explicit_limitation"), f"{name}.explicit_limitation", True)


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(field(e, "cleanup"), "cleanup")
    exact_keys(cleanup, {"evidence_frozen_before_cleanup", "cleanup_completed", "inventories", "postgresql",
                         "services_ready", "retained_tombstones", "retained_history"}, "cleanup")
    boolean(cleanup.get("evidence_frozen_before_cleanup"), "cleanup.evidence_frozen_before_cleanup", True)
    boolean(cleanup.get("cleanup_completed"), "cleanup.cleanup_completed", True)
    inventories = indexed(cleanup.get("inventories"), "cleanup.inventories", "resource_kind")
    if set(inventories) != CLEANUP_RESOURCES:
        fail("cleanup.inventories: exact run resource/VM/Tailscale/local credential inventory required")
    for kind, row in inventories.items():
        name = f"cleanup.inventories[{kind}]"
        exact_keys(row, {"resource_kind", "remaining_count", "authoritative_inventory_checked"}, name)
        integer(row.get("remaining_count"), f"{name}.remaining_count", 0)
        boolean(row.get("authoritative_inventory_checked"), f"{name}.authoritative_inventory_checked", True)
    postgres = obj(cleanup.get("postgresql"), "cleanup.postgresql")
    exact_keys(postgres, {"restored", "memory_mib"}, "cleanup.postgresql")
    boolean(postgres.get("restored"), "cleanup.postgresql.restored", True)
    integer(postgres.get("memory_mib"), "cleanup.postgresql.memory_mib", 256)
    services = obj(cleanup.get("services_ready"), "cleanup.services_ready")
    exact_keys(services, {"control_plane", "rls", "database"}, "cleanup.services_ready")
    for service in ("control_plane", "rls", "database"):
        boolean(services.get(service), f"cleanup.services_ready.{service}", True)
    for key in ("retained_tombstones", "retained_history"):
        retained = obj(cleanup.get(key), f"cleanup.{key}")
        exact_keys(retained, {"explicitly_retained", "inert", "active", "addressable", "can_serve_traffic"}, f"cleanup.{key}")
        boolean(retained.get("explicitly_retained"), f"cleanup.{key}.explicitly_retained", True)
        boolean(retained.get("inert"), f"cleanup.{key}.inert", True)
        for attr in ("active", "addressable", "can_serve_traffic"):
            boolean(retained.get(attr), f"cleanup.{key}.{attr}", False)


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
    exact_keys(redaction, {"sanitized_projection", "secret_values_recorded", "raw_logs_recorded", "raw_identifiers_recorded",
                           "private_paths_recorded", "scan_after_final_edit", "undisposed_match_count", "pattern_classes"}, "redaction")
    boolean(redaction.get("sanitized_projection"), "redaction.sanitized_projection", True)
    for key in ("secret_values_recorded", "raw_logs_recorded", "raw_identifiers_recorded", "private_paths_recorded"):
        boolean(redaction.get(key), f"redaction.{key}", False)
    boolean(redaction.get("scan_after_final_edit"), "redaction.scan_after_final_edit", True)
    integer(redaction.get("undisposed_match_count"), "redaction.undisposed_match_count", 0)
    required = {"credentials", "tailscale_keys", "bearer_tokens", "private_keys", "certificate_bodies", "private_paths", "raw_identifiers"}
    if set(seq(redaction.get("pattern_classes"), "redaction.pattern_classes")) != required:
        fail("redaction.pattern_classes: exact scan classes required")
    walk(e)


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.10-RUN": ("independent sanitized exact four-team live run", check_run),
    "FPV2-D23.10-BASELINE": ("fresh certificate-bound VMs with baseline traffic and agent health", check_baseline),
    "FPV2-D23.10-CP": ("LKG continuity and same-machine CP recovery after scoped key rotation", check_control_plane),
    "FPV2-D23.10-DB": ("LKG continuity and same-machine database recovery", check_database),
    "FPV2-D23.10-TAILSCALE": ("same-node Tailscale recovery with topology-inapplicable traffic limitation", check_tailscale),
    "FPV2-D23.10-PROCESSES": ("live Envoy, agent and upstream process fault matrix", check_process_faults),
    "FPV2-D23.10-RLS": ("same-machine RLS recovery and non-RLS continuity", check_rls),
    "FPV2-D23.10-SUITES": ("focused xDS/NACK/certificate and API/auth/MCP evidence", check_suites),
    "FPV2-D23.10-LIMITATIONS": ("explicit non-live and carry-forward boundaries", check_limitations),
    "FPV2-D23.10-CLEANUP": ("zero active resources, restored database and inert retained history", check_cleanup),
    "FPV2-D23.10-REDACTION": ("strict secret, raw identifier and private path redaction", check_redaction),
}


def fp(number: int) -> str:
    return "sha256:" + f"{number:064x}"


def observations(traffic: bool, agent: bool) -> list[dict[str, Any]]:
    return [{"team_key": team, "lkg_traffic_available": traffic, "agent_healthy": agent} for team in TEAMS]


def fixture() -> dict[str, Any]:
    baseline = [
        {"team_key": team, "vm_ref": f"vm_{index}", "fresh_vm": True, "certificate_bound": True,
         "node_identity_fingerprint": fp(index), "config_fingerprint": fp(10 + index),
         "traffic_available": True, "agent_healthy": True}
        for index, team in enumerate(TEAMS, 1)
    ]
    process_faults = []
    for index, (process, (traffic, degraded, envoy_live)) in enumerate(PROCESS_EFFECTS.items(), 1):
        process_faults.append({"process": process, "live_service_stop": True,
                               "traffic_available_during_stop": traffic, "agent_degraded_during_stop": degraded,
                               "envoy_live_during_stop": envoy_live, "transient_systemd_unit": True,
                               "new_pid_required": True, "exact_pids_recorded": False, "same_vm_ref": "vm_1",
                               "same_config": True, "same_identity": True, "replacement_used": False, "recovered": True})
    non_rls = [{"team_key": team, "lkg_traffic_available": True, "control_plane_ready": True} for team in TEAMS]
    focused = [{"id": suite_id, "focused_selector": True, "selected_count": selected, "passed_count": passed,
                "failed_count": 0, "skipped_by_selector": skipped, "full_suite_count_claimed": False,
                "evidence_class": "focused"} for suite_id, (selected, passed, skipped) in SUITES.items()]
    limitations = [{"id": limitation_id, "live_tested": False, "status": status,
                    "carry_forward_source": source, "acceptance_claimed_live": False, "explicit_limitation": True}
                   for limitation_id, (status, source) in LIMITATIONS.items()]
    inventories = [{"resource_kind": kind, "remaining_count": 0, "authoritative_inventory_checked": True}
                   for kind in sorted(CLEANUP_RESOURCES)]
    inert = {"explicitly_retained": True, "inert": True, "active": False, "addressable": False, "can_serve_traffic": False}
    return {
        "schema": SCHEMA,
        "run": {"live_qualification": True, "rerunnable": True, "sanitized_projection": True,
                "supported_surfaces_only": True, "direct_database_access": False,
                "independent_author_read_implementation": False, "started_at_utc": "2026-08-22T01:00:00Z",
                "finished_at_utc": "2026-08-22T02:00:00Z", "team_keys": list(TEAMS)},
        "baseline": baseline,
        "control_plane": {"live_service_stop": True, "machine_ref_before": "cp_machine", "machine_ref_after": "cp_machine",
                          "during_stop": observations(True, False),
                          "initial_restart": {"attempted_on_same_machine": True, "recovered": False,
                                              "failure_reason": "staged_tailscale_key_invalid",
                                              "classification": "environment_staging"},
                          "human_rotation": {"performed": True, "rotated_key_scopes": ["control_plane", "rls"],
                                             "other_credentials_rotated": False},
                          "recovery": {"same_machine": True, "replacement_used": False, "control_plane_ready": True,
                                       "teams": observations(True, True)}},
        "database": {"live_same_machine_stop": True, "machine_ref_before": "db_machine", "machine_ref_after": "db_machine",
                     "during_stop": {"control_plane_readiness_degraded": True,
                                     "teams": [{"team_key": team, "lkg_traffic_available": True} for team in TEAMS]},
                     "recovery": {"same_machine": True, "replacement_used": False, "database_ready": True,
                                  "control_plane_ready": True}},
        "tailscale": {"live_one_vm_down_up": True, "team_key": "alpha/payments", "vm_ref_before": "vm_1",
                      "vm_ref_after": "vm_1", "node_identity_before": fp(1), "node_identity_after": fp(1),
                      "agent_degraded_while_down": True, "agent_healthy_after_up": True, "replacement_used": False,
                      "auth_reenrollment_used": False,
                      "controlled_upstream_used_same_vm_tailnet_ip": True, "traffic_survival_claimed": False,
                      "limitation_id": "tailscale_traffic_survival"},
        "local_process_faults": {"live_faults": True, "team_key": "alpha/payments", "vm_ref": "vm_1", "faults": process_faults},
        "rls": {"live_same_machine_service_stop": True, "machine_ref_before": "rls_machine", "machine_ref_after": "rls_machine",
                "replacement_used": False, "service_recovered": True, "non_rls_during_stop": non_rls,
                "policy_behavior_live_tested": False, "policy_behavior_source": "fpv2-d23.6"},
        "focused_suites": focused,
        "limitations": limitations,
        "cleanup": {"evidence_frozen_before_cleanup": True, "cleanup_completed": True, "inventories": inventories,
                    "postgresql": {"restored": True, "memory_mib": 256},
                    "services_ready": {"control_plane": True, "rls": True, "database": True},
                    "retained_tombstones": dict(inert), "retained_history": dict(inert)},
        "redaction": {"sanitized_projection": True, "secret_values_recorded": False, "raw_logs_recorded": False,
                      "raw_identifiers_recorded": False, "private_paths_recorded": False, "scan_after_final_edit": True,
                      "undisposed_match_count": 0, "pattern_classes": ["credentials", "tailscale_keys", "bearer_tokens",
                          "private_keys", "certificate_bodies", "private_paths", "raw_identifiers"]},
    }


def self_test() -> int:
    good = fixture()
    failures: list[str] = []
    for scenario, (_, check) in SCENARIOS.items():
        try:
            check(good)
        except ContractFailure as error:
            failures.append(f"valid fixture rejected by {scenario}: {error}")
    mutations: list[tuple[str, Callable[[dict[str, Any]], None], str]] = [
        ("extra root field", lambda x: x.__setitem__("unexpected", True), "FPV2-D23.10-RUN"),
        ("missing team", lambda x: x["baseline"].pop(), "FPV2-D23.10-BASELINE"),
        ("non-fresh VM", lambda x: x["baseline"][0].__setitem__("fresh_vm", False), "FPV2-D23.10-BASELINE"),
        ("certificate not bound", lambda x: x["baseline"][1].__setitem__("certificate_bound", False), "FPV2-D23.10-BASELINE"),
        ("CP traffic lost", lambda x: x["control_plane"]["during_stop"][0].__setitem__("lkg_traffic_available", False), "FPV2-D23.10-CP"),
        ("CP agent not degraded", lambda x: x["control_plane"]["during_stop"][0].__setitem__("agent_healthy", True), "FPV2-D23.10-CP"),
        ("CP replacement", lambda x: x["control_plane"].__setitem__("machine_ref_after", "cp_replacement"), "FPV2-D23.10-CP"),
        ("initial CP restart succeeds", lambda x: x["control_plane"]["initial_restart"].__setitem__("recovered", True), "FPV2-D23.10-CP"),
        ("CP failure misclassified", lambda x: x["control_plane"]["initial_restart"].__setitem__("classification", "product"), "FPV2-D23.10-CP"),
        ("extra key rotation", lambda x: x["control_plane"]["human_rotation"].__setitem__("other_credentials_rotated", True), "FPV2-D23.10-CP"),
        ("DB readiness stays ready", lambda x: x["database"]["during_stop"].__setitem__("control_plane_readiness_degraded", False), "FPV2-D23.10-DB"),
        ("DB replacement", lambda x: x["database"].__setitem__("machine_ref_after", "db_replacement"), "FPV2-D23.10-DB"),
        ("Tailscale identity replaced", lambda x: x["tailscale"].__setitem__("node_identity_after", fp(999)), "FPV2-D23.10-TAILSCALE"),
        ("Tailscale re-enrollment hidden", lambda x: x["tailscale"].__setitem__("auth_reenrollment_used", True), "FPV2-D23.10-TAILSCALE"),
        ("Tailscale traffic overclaim", lambda x: x["tailscale"].__setitem__("traffic_survival_claimed", True), "FPV2-D23.10-TAILSCALE"),
        ("Envoy traffic survives", lambda x: x["local_process_faults"]["faults"][0].__setitem__("traffic_available_during_stop", True), "FPV2-D23.10-PROCESSES"),
        ("agent stop loses LKG", lambda x: x["local_process_faults"]["faults"][1].__setitem__("traffic_available_during_stop", False), "FPV2-D23.10-PROCESSES"),
        ("upstream stop kills Envoy", lambda x: x["local_process_faults"]["faults"][2].__setitem__("envoy_live_during_stop", False), "FPV2-D23.10-PROCESSES"),
        ("exact PID overclaim", lambda x: x["local_process_faults"]["faults"][0].__setitem__("exact_pids_recorded", True), "FPV2-D23.10-PROCESSES"),
        ("process identity replaced", lambda x: x["local_process_faults"]["faults"][1].__setitem__("same_identity", False), "FPV2-D23.10-PROCESSES"),
        ("RLS replacement", lambda x: x["rls"].__setitem__("machine_ref_after", "rls_replacement"), "FPV2-D23.10-RLS"),
        ("RLS policy overclaim", lambda x: x["rls"].__setitem__("policy_behavior_live_tested", True), "FPV2-D23.10-RLS"),
        ("xDS focused count wrong", lambda x: x["focused_suites"][0].__setitem__("passed_count", 10), "FPV2-D23.10-SUITES"),
        ("API skip count wrong", lambda x: x["focused_suites"][1].__setitem__("skipped_by_selector", 0), "FPV2-D23.10-SUITES"),
        ("full suite overclaim", lambda x: x["focused_suites"][0].__setitem__("full_suite_count_claimed", True), "FPV2-D23.10-SUITES"),
        ("Auth0 live overclaim", lambda x: next(v for v in x["limitations"] if v["id"] == "shared_auth0_outage").__setitem__("live_tested", True), "FPV2-D23.10-LIMITATIONS"),
        ("wrong AI carry-forward", lambda x: next(v for v in x["limitations"] if v["id"] == "ai_provider_recovery").__setitem__("carry_forward_source", "fpv2-d23.9"), "FPV2-D23.10-LIMITATIONS"),
        ("VM residue", lambda x: next(v for v in x["cleanup"]["inventories"] if v["resource_kind"] == "vms").__setitem__("remaining_count", 1), "FPV2-D23.10-CLEANUP"),
        ("DB memory not restored", lambda x: x["cleanup"]["postgresql"].__setitem__("memory_mib", 512), "FPV2-D23.10-CLEANUP"),
        ("RLS not ready", lambda x: x["cleanup"]["services_ready"].__setitem__("rls", False), "FPV2-D23.10-CLEANUP"),
        ("active tombstone", lambda x: x["cleanup"]["retained_tombstones"].__setitem__("active", True), "FPV2-D23.10-CLEANUP"),
        ("secret-shaped key", lambda x: x["redaction"].__setitem__("access_token", "forbidden"), "FPV2-D23.10-REDACTION"),
        ("private path", lambda x: x["redaction"]["pattern_classes"].append("/Users/example/private"), "FPV2-D23.10-REDACTION"),
        ("email identifier", lambda x: x["redaction"]["pattern_classes"].append("operator@example.com"), "FPV2-D23.10-REDACTION"),
        ("UUID identifier", lambda x: x["redaction"]["pattern_classes"].append("123e4567-e89b-12d3-a456-426614174000"), "FPV2-D23.10-REDACTION"),
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
        print(f"recovery acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"recovery acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
    return 0


def load(path: Path) -> dict[str, Any]:
    try:
        return obj(json.loads(path.read_text(encoding="utf-8")), "evidence root")
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"sanitized recovery evidence unreadable: {path.resolve()}: {error}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", help="sanitized recovery JSON projection")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), help="run exactly one independently rerunnable scenario")
    parser.add_argument("--list", action="store_true", help="list scenario IDs and exit")
    parser.add_argument("--self-test", action="store_true", help="run synthetic positive and adversarial checks")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list:
        for scenario, (description, _) in SCENARIOS.items():
            print(f"{scenario}\t{description}")
        return 0
    if args.self_test:
        return self_test()
    path = Path(args.evidence or os.environ.get("FLOWPLANE_RECOVERY_EVIDENCE", DEFAULT_EVIDENCE))
    try:
        evidence = load(path)
    except ContractFailure as error:
        print(f"recovery acceptance: FAIL: {error}", file=sys.stderr)
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
        print(f"recovery acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"recovery acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
