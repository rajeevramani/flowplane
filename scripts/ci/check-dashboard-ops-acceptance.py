#!/usr/bin/env python3
"""Independent, fail-closed fpv2-d23.9 dashboard/operations acceptance gate.

Consumes only a sanitized ``flowplane.qualification.dashboard-ops/v1`` JSON
projection.  The live producer owns browser/process orchestration; this checker
validates externally observable results without reading production
implementation, raw logs, credentials, private evidence, or browser profiles.
``--self-test`` exercises a synthetic valid projection and adversarial
fail-closed mutations.
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

SCHEMA = "flowplane.qualification.dashboard-ops/v1"
DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.9/dashboard-ops.json")
TEAMS = ("alpha/shared", "beta/shared")
ROUTES = {
    "overview": "/",
    "resources": "/resources",
    "apis": "/apis",
    "learning": "/learning",
    "ai": "/ai",
    "mcp": "/mcp",
    "operations": "/operations",
}
CLI_SURFACES = ("cluster", "stats", "xds", "nack")
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REF = re.compile(r"^[a-z][a-z0-9_-]{2,63}$")
PROHIBITED_KEYS = {
    "token", "access_token", "refresh_token", "authorization", "bearer",
    "password", "secret", "secret_value", "private_key", "cookie", "nonce",
    "raw_nonce", "raw_header", "raw_request", "raw_response", "raw_body",
    "browser_profile_path", "private_evidence_path", "credential",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~+/-]{8,}"),
    re.compile(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b"),
    re.compile(r"(?i)(?:password|client_secret|api_key)\s*[:=]\s*\S+"),
    re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\\\Users\\\\)"),
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


def indexed(value: Any, name: str, key: str) -> dict[str, dict[str, Any]]:
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
        fail(f"{name}: exactly Alpha/Shared and Beta/Shared required; got {sorted(rows)!r}")
    return rows


def check_run(e: dict[str, Any]) -> None:
    exact_keys(e, {"schema", "run", "processes", "isolation", "security", "cli_agreement",
                   "degradation", "browser", "lifecycle", "limitations", "cleanup", "redaction"},
               "evidence root")
    equal(e, "schema", SCHEMA)
    run = obj(field(e, "run"), "run")
    exact_keys(run, {"live_qualification", "rerunnable", "supported_surfaces_only", "direct_database_access",
                     "independent_author_read_implementation", "external_infrastructure_used",
                     "started_at_utc", "finished_at_utc", "fixture_cluster_name", "minimal_cluster_count",
                     "active_dataplanes_observed", "retained_ai_history_may_render",
                     "retained_mcp_history_may_render"}, "run")
    for key in ("live_qualification", "rerunnable", "supported_surfaces_only"):
        boolean(run.get(key), f"run.{key}", True)
    for key in ("direct_database_access", "independent_author_read_implementation", "external_infrastructure_used"):
        boolean(run.get(key), f"run.{key}", False)
    started = timestamp(run.get("started_at_utc"), "run.started_at_utc")
    finished = timestamp(run.get("finished_at_utc"), "run.finished_at_utc")
    if finished <= started:
        fail("run: finish must post-date start")
    text(run.get("fixture_cluster_name"), "run.fixture_cluster_name")
    integer(run.get("minimal_cluster_count"), "run.minimal_cluster_count", 2)
    integer(run.get("active_dataplanes_observed"), "run.active_dataplanes_observed")
    boolean(run.get("retained_ai_history_may_render"), "run.retained_ai_history_may_render", True)
    boolean(run.get("retained_mcp_history_may_render"), "run.retained_mcp_history_may_render", True)


def check_processes_and_routes(e: dict[str, Any]) -> None:
    processes = team_index(field(e, "processes"), "processes")
    ports: set[int] = set()
    nonces: set[str] = set()
    profiles: set[str] = set()
    for team, process in processes.items():
        name = f"processes[{team}]"
        exact_keys(process, {"team_key", "bind_host", "port", "port_ephemeral", "nonce_fingerprint",
                             "browser_profile_ref", "browser_profile_initially_empty", "command_surface",
                             "read_only", "dashboard_started", "routes"}, name)
        if process.get("bind_host") != "127.0.0.1":
            fail(f"{name}.bind_host: loopback IPv4 required")
        port = integer(process.get("port"), f"{name}.port")
        if not 1024 <= port <= 65535 or process.get("port_ephemeral") is not True:
            fail(f"{name}.port: non-privileged ephemeral port attestation required")
        ports.add(port)
        nonces.add(fingerprint(process.get("nonce_fingerprint"), f"{name}.nonce_fingerprint"))
        profile = text(process.get("browser_profile_ref"), f"{name}.browser_profile_ref")
        if not REF.fullmatch(profile):
            fail(f"{name}.browser_profile_ref: sanitized reference required")
        profiles.add(profile)
        boolean(process.get("browser_profile_initially_empty"), f"{name}.browser_profile_initially_empty", True)
        if process.get("command_surface") != "flowplane_dashboard":
            fail(f"{name}.command_surface: supported dashboard CLI required")
        boolean(process.get("read_only"), f"{name}.read_only", True)
        boolean(process.get("dashboard_started"), f"{name}.dashboard_started", True)
        routes = indexed(process.get("routes"), f"{name}.routes", "id")
        if set(routes) != set(ROUTES):
            fail(f"{name}.routes: exact seven-route inventory required")
        for route_id, expected_path in ROUTES.items():
            row = routes[route_id]
            route_name = f"{name}.routes[{route_id}]"
            exact_keys(row, {"id", "path", "shell", "panel"}, route_name)
            if row.get("path") != expected_path:
                fail(f"{route_name}.path: expected {expected_path!r}")
            shell = obj(row.get("shell"), f"{route_name}.shell")
            exact_keys(shell, {"fetched", "http_status", "cache_control"}, f"{route_name}.shell")
            boolean(shell.get("fetched"), f"{route_name}.shell.fetched", True)
            integer(shell.get("http_status"), f"{route_name}.shell.http_status", 200)
            if shell.get("cache_control") != "no-store":
                fail(f"{route_name}.shell.cache_control: no-store required")
            panel = obj(row.get("panel"), f"{route_name}.panel")
            exact_keys(panel, {"applicable", "fetched", "http_status", "outcome", "cache_control"}, f"{route_name}.panel")
            boolean(panel.get("applicable"), f"{route_name}.panel.applicable", True)
            boolean(panel.get("fetched"), f"{route_name}.panel.fetched", True)
            integer(panel.get("http_status"), f"{route_name}.panel.http_status", 200)
            if panel.get("outcome") != "available":
                fail(f"{route_name}.panel.outcome: available required in baseline")
            if panel.get("cache_control") != "no-store":
                fail(f"{route_name}.panel.cache_control: no-store required")
    if len(ports) != 2 or len(nonces) != 2 or len(profiles) != 2:
        fail("processes: ports, nonce fingerprints, and empty browser profiles must be distinct")


def check_isolation(e: dict[str, Any]) -> None:
    isolation = obj(field(e, "isolation"), "isolation")
    exact_keys(isolation, {"overlapping_cluster_name", "views"}, "isolation")
    if isolation.get("overlapping_cluster_name") != field(e, "run.fixture_cluster_name"):
        fail("isolation.overlapping_cluster_name: fixture cluster name mismatch")
    for team, view in team_index(isolation.get("views"), "isolation.views").items():
        name = f"isolation.views[{team}]"
        exact_keys(view, {"team_key", "cluster_name", "observed_revision", "own_cluster_count",
                          "foreign_cluster_count", "foreign_revision_visible", "foreign_team_marker_count"}, name)
        if view.get("cluster_name") != isolation["overlapping_cluster_name"]:
            fail(f"{name}.cluster_name: overlapping name required")
        expected_revision = 1 if team == "alpha/shared" else 2
        integer(view.get("observed_revision"), f"{name}.observed_revision", expected_revision)
        integer(view.get("own_cluster_count"), f"{name}.own_cluster_count", 1)
        integer(view.get("foreign_cluster_count"), f"{name}.foreign_cluster_count", 0)
        boolean(view.get("foreign_revision_visible"), f"{name}.foreign_revision_visible", False)
        integer(view.get("foreign_team_marker_count"), f"{name}.foreign_team_marker_count", 0)


def check_security(e: dict[str, Any]) -> None:
    security = obj(field(e, "security"), "security")
    exact_keys(security, {"processes", "browser_bearer_material_visible", "browser_secret_shaped_value_count"}, "security")
    boolean(security.get("browser_bearer_material_visible"), "security.browser_bearer_material_visible", False)
    integer(security.get("browser_secret_shaped_value_count"), "security.browser_secret_shaped_value_count", 0)
    expected = {
        "nonce_missing": ("GET", "not_found"), "nonce_wrong": ("GET", "not_found"),
        "bad_host": ("GET", "denied"), "bad_origin": ("GET", "denied"),
        "post": ("POST", "denied"),
    }
    for team, row in team_index(security.get("processes"), "security.processes").items():
        exact_keys(row, {"team_key", "probes"}, f"security.processes[{team}]")
        probes = indexed(row.get("probes"), f"security.processes[{team}].probes", "id")
        if set(probes) != set(expected):
            fail(f"security.processes[{team}].probes: exact negative matrix required")
        for probe_id, (method, status_class) in expected.items():
            probe = probes[probe_id]
            name = f"security.processes[{team}].probes[{probe_id}]"
            exact_keys(probe, {"id", "method", "status_class", "denied", "state_changed", "cache_control"}, name)
            if probe.get("method") != method:
                fail(f"{name}.method: expected {method}")
            if probe.get("status_class") != status_class:
                fail(f"{name}.status_class: expected {status_class!r}")
            boolean(probe.get("denied"), f"{name}.denied", True)
            boolean(probe.get("state_changed"), f"{name}.state_changed", False)
            if probe.get("cache_control") != "no-store":
                fail(f"{name}.cache_control: no-store required")


def check_cli_agreement(e: dict[str, Any]) -> None:
    for team, row in team_index(field(e, "cli_agreement"), "cli_agreement").items():
        exact_keys(row, {"team_key", "surfaces"}, f"cli_agreement[{team}]")
        surfaces = indexed(row.get("surfaces"), f"cli_agreement[{team}].surfaces", "id")
        if set(surfaces) != set(CLI_SURFACES):
            fail(f"cli_agreement[{team}].surfaces: cluster/stats/xds/nack required")
        for surface_id, surface in surfaces.items():
            name = f"cli_agreement[{team}].surfaces[{surface_id}]"
            exact_keys(surface, {"id", "supported_cli", "cli_exit_status", "cli_row_count", "panel_row_count",
                                 "cli_projection_fingerprint", "panel_projection_fingerprint"}, name)
            boolean(surface.get("supported_cli"), f"{name}.supported_cli", True)
            integer(surface.get("cli_exit_status"), f"{name}.cli_exit_status", 0)
            cli_count = integer(surface.get("cli_row_count"), f"{name}.cli_row_count")
            integer(surface.get("panel_row_count"), f"{name}.panel_row_count", cli_count)
            cli_fp = fingerprint(surface.get("cli_projection_fingerprint"), f"{name}.cli_projection_fingerprint")
            panel_fp = fingerprint(surface.get("panel_projection_fingerprint"), f"{name}.panel_projection_fingerprint")
            if cli_fp != panel_fp:
                fail(f"{name}: supported CLI and dashboard panel disagree")


def check_degradation(e: dict[str, Any]) -> None:
    degradation = obj(field(e, "degradation"), "degradation")
    exact_keys(degradation, {"control_plane_disconnects", "authentication_cases"}, "degradation")
    disconnects = seq(degradation.get("control_plane_disconnects"), "degradation.control_plane_disconnects")
    if len(disconnects) != 1 or obj(disconnects[0], "degradation.control_plane_disconnects[0]").get("team_key") != "alpha/shared":
        fail("degradation.control_plane_disconnects: exact Alpha/Shared live disconnect required")
    for row in disconnects:
        team = row["team_key"]
        name = f"degradation.control_plane_disconnects[{team}]"
        exact_keys(row, {"team_key", "control_plane_disconnected", "shell_http_status", "own_panel_count",
                         "own_panel_outcome", "other_team_panel_outcome", "independent_degradation"}, name)
        boolean(row.get("control_plane_disconnected"), f"{name}.control_plane_disconnected", True)
        integer(row.get("shell_http_status"), f"{name}.shell_http_status", 200)
        integer(row.get("own_panel_count"), f"{name}.own_panel_count", 3)
        if row.get("own_panel_outcome") != "unavailable" or row.get("other_team_panel_outcome") != "available":
            fail(f"{name}: own panels must degrade unavailable without contaminating other process")
        boolean(row.get("independent_degradation"), f"{name}.independent_degradation", True)
    cases = indexed(degradation.get("authentication_cases"), "degradation.authentication_cases", "id")
    if set(cases) != {"beta-invalid-expired-shaped"}:
        fail("degradation.authentication_cases: exact Beta invalid/expired-shaped live case required")
    for case_id, case in cases.items():
        name = f"degradation.authentication_cases[{case_id}]"
        exact_keys(case, {"id", "team_key", "credential_shape", "shell_http_status", "authentication_partial_rendered",
                          "panel_outcome", "credential_material_visible", "secret_shaped_value_count",
                          "other_team_panel_outcome"}, name)
        if case.get("team_key") != "beta/shared" or case.get("credential_shape") != "invalid-expired-shaped":
            fail(f"{name}: exact Beta invalid/expired-shaped case required")
        integer(case.get("shell_http_status"), f"{name}.shell_http_status", 200)
        boolean(case.get("authentication_partial_rendered"), f"{name}.authentication_partial_rendered", True)
        if case.get("panel_outcome") != "authentication-required" or case.get("other_team_panel_outcome") != "available":
            fail(f"{name}: authentication degradation must be isolated")
        boolean(case.get("credential_material_visible"), f"{name}.credential_material_visible", False)
        integer(case.get("secret_shaped_value_count"), f"{name}.secret_shaped_value_count", 0)


def check_browser(e: dict[str, Any]) -> None:
    profiles: set[str] = set()
    process_profiles = {
        team: row["browser_profile_ref"]
        for team, row in team_index(field(e, "processes"), "processes").items()
    }
    for team, row in team_index(field(e, "browser"), "browser").items():
        name = f"browser[{team}]"
        exact_keys(row, {"team_key", "engine", "real_window", "profile_ref", "profile_initially_empty",
                         "route", "http_status", "observed_title", "flowplane_resources_title_matched"}, name)
        if row.get("engine") != "Google Chrome" or row.get("real_window") is not True:
            fail(f"{name}: real Google Chrome window required")
        profile = text(row.get("profile_ref"), f"{name}.profile_ref")
        if not REF.fullmatch(profile):
            fail(f"{name}.profile_ref: sanitized reference required")
        profiles.add(profile)
        if profile != process_profiles[team]:
            fail(f"{name}.profile_ref: must match the dashboard process profile")
        boolean(row.get("profile_initially_empty"), f"{name}.profile_initially_empty", True)
        if row.get("route") != "/resources":
            fail(f"{name}.route: resources route required")
        integer(row.get("http_status"), f"{name}.http_status", 200)
        title = text(row.get("observed_title"), f"{name}.observed_title")
        if "flowplane" not in title.lower() or "resources" not in title.lower():
            fail(f"{name}.observed_title: Flowplane resources title required")
        boolean(row.get("flowplane_resources_title_matched"), f"{name}.flowplane_resources_title_matched", True)
    if len(profiles) != 2:
        fail("browser: distinct Chrome profiles required")


def check_lifecycle(e: dict[str, Any]) -> None:
    for team, row in team_index(field(e, "lifecycle"), "lifecycle").items():
        name = f"lifecycle[{team}]"
        exact_keys(row, {"team_key", "cli_process_exit_observed", "dashboard_child_exit_observed",
                         "loopback_port_closed", "bounded_shutdown"}, name)
        for key in ("cli_process_exit_observed", "dashboard_child_exit_observed", "loopback_port_closed", "bounded_shutdown"):
            boolean(row.get(key), f"{name}.{key}", True)


def check_limitations(e: dict[str, Any]) -> None:
    rows = indexed(field(e, "limitations"), "limitations", "id")
    if set(rows) != {"live_stale_grant_mutation", "live_cryptographic_expiry", "live_pagination"}:
        fail("limitations: exact stale-grant, expiry and pagination limitations required")
    row = rows["live_stale_grant_mutation"]
    exact_keys(row, {"id", "live_mutation_performed", "automated_suite_covers_grant_transition",
                     "live_invalid_bearer_covers_auth_degradation", "acceptance_claim"}, "limitations[live_stale_grant_mutation]")
    boolean(row.get("live_mutation_performed"), "limitations.live_stale_grant_mutation.live_mutation_performed", False)
    boolean(row.get("automated_suite_covers_grant_transition"), "limitations.live_stale_grant_mutation.automated_suite_covers_grant_transition", True)
    boolean(row.get("live_invalid_bearer_covers_auth_degradation"), "limitations.live_stale_grant_mutation.live_invalid_bearer_covers_auth_degradation", True)
    if row.get("acceptance_claim") != "live_stale_grant_mutation_not_performed":
        fail("limitations.live_stale_grant_mutation.acceptance_claim: explicit non-claim required")
    expiry = rows["live_cryptographic_expiry"]
    exact_keys(expiry, {"id", "live_cryptographically_expired_token_used", "invalid_expired_shaped_token_used", "automated_suite_covers_expired_token", "acceptance_claim"}, "limitations[live_cryptographic_expiry]")
    boolean(expiry.get("live_cryptographically_expired_token_used"), "limitations.live_cryptographic_expiry.live_cryptographically_expired_token_used", False)
    boolean(expiry.get("invalid_expired_shaped_token_used"), "limitations.live_cryptographic_expiry.invalid_expired_shaped_token_used", True)
    boolean(expiry.get("automated_suite_covers_expired_token"), "limitations.live_cryptographic_expiry.automated_suite_covers_expired_token", True)
    if expiry.get("acceptance_claim") != "live_cryptographic_expiry_not_performed":
        fail("limitations.live_cryptographic_expiry.acceptance_claim: explicit non-claim required")
    pagination = rows["live_pagination"]
    exact_keys(pagination, {"id", "live_pagination_performed", "automated_suite_covers_pagination", "design_acceptance_not_claimed_live", "acceptance_claim"}, "limitations[live_pagination]")
    boolean(pagination.get("live_pagination_performed"), "limitations.live_pagination.live_pagination_performed", False)
    boolean(pagination.get("automated_suite_covers_pagination"), "limitations.live_pagination.automated_suite_covers_pagination", True)
    boolean(pagination.get("design_acceptance_not_claimed_live"), "limitations.live_pagination.design_acceptance_not_claimed_live", True)
    if pagination.get("acceptance_claim") != "live_pagination_not_performed":
        fail("limitations.live_pagination.acceptance_claim: explicit non-claim required")


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(field(e, "cleanup"), "cleanup")
    exact_keys(cleanup, {"evidence_frozen_before_cleanup", "minimal_clusters_created", "minimal_clusters_remaining",
                         "dashboard_processes_remaining", "listening_dashboard_ports_remaining",
                         "browser_profiles_remaining", "nonce_url_files_remaining",
                         "external_infrastructure_created", "retained_ai_history_allowed",
                         "retained_mcp_history_allowed"}, "cleanup")
    boolean(cleanup.get("evidence_frozen_before_cleanup"), "cleanup.evidence_frozen_before_cleanup", True)
    integer(cleanup.get("minimal_clusters_created"), "cleanup.minimal_clusters_created", 2)
    for key in ("minimal_clusters_remaining", "dashboard_processes_remaining", "listening_dashboard_ports_remaining",
                "browser_profiles_remaining", "nonce_url_files_remaining",
                "external_infrastructure_created"):
        integer(cleanup.get(key), f"cleanup.{key}", 0)
    boolean(cleanup.get("retained_ai_history_allowed"), "cleanup.retained_ai_history_allowed", True)
    boolean(cleanup.get("retained_mcp_history_allowed"), "cleanup.retained_mcp_history_allowed", True)


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
    exact_keys(redaction, {"sanitized_projection", "raw_payloads_recorded", "raw_identifiers_recorded",
                           "private_paths_recorded", "browser_secrets_recorded", "scan_after_final_edit",
                           "undisposed_match_count", "pattern_classes"}, "redaction")
    boolean(redaction.get("sanitized_projection"), "redaction.sanitized_projection", True)
    for key in ("raw_payloads_recorded", "raw_identifiers_recorded", "private_paths_recorded", "browser_secrets_recorded"):
        boolean(redaction.get(key), f"redaction.{key}", False)
    boolean(redaction.get("scan_after_final_edit"), "redaction.scan_after_final_edit", True)
    integer(redaction.get("undisposed_match_count"), "redaction.undisposed_match_count", 0)
    classes = set(seq(redaction.get("pattern_classes"), "redaction.pattern_classes"))
    required = {"bearer_tokens", "browser_storage", "private_paths", "raw_nonces", "credentials", "private_keys"}
    if not required.issubset(classes):
        fail(f"redaction.pattern_classes: missing {sorted(required - classes)!r}")
    walk(e)


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.9-RUN": ("sanitized local-only live qualification contract", check_run),
    "FPV2-D23.9-PROCESSES-ROUTES": ("two isolated dashboard processes and seven complete route fetches", check_processes_and_routes),
    "FPV2-D23.9-ISOLATION": ("overlapping cluster names with revision and tenant isolation", check_isolation),
    "FPV2-D23.9-SECURITY": ("nonce, Host, Origin, method and cache fail-closed matrix", check_security),
    "FPV2-D23.9-CLI-AGREEMENT": ("cluster, stats, xDS and NACK dashboard/CLI agreement", check_cli_agreement),
    "FPV2-D23.9-DEGRADATION": ("independent control-plane and authentication degradation", check_degradation),
    "FPV2-D23.9-CHROME": ("real Chrome resources-title windows in separate empty profiles", check_browser),
    "FPV2-D23.9-LIFECYCLE": ("dashboard servers die with their CLI processes", check_lifecycle),
    "FPV2-D23.9-LIMITATION": ("explicit stale-grant live-test limitation and disposition", check_limitations),
    "FPV2-D23.9-CLEANUP": ("minimal cluster and local process cleanup without external infrastructure", check_cleanup),
    "FPV2-D23.9-REDACTION": ("strict sanitized projection and browser-secret redaction", check_redaction),
}


def fp(number: int) -> str:
    return "sha256:" + f"{number:064x}"


def fixture() -> dict[str, Any]:
    processes: list[dict[str, Any]] = []
    security: list[dict[str, Any]] = []
    cli: list[dict[str, Any]] = []
    disconnects: list[dict[str, Any]] = []
    browser: list[dict[str, Any]] = []
    lifecycle: list[dict[str, Any]] = []
    auth_cases: list[dict[str, Any]] = []
    for index, team in enumerate(TEAMS, 1):
        routes = []
        for route_id, path in ROUTES.items():
            routes.append({"id": route_id, "path": path,
                           "shell": {"fetched": True, "http_status": 200, "cache_control": "no-store"},
                           "panel": {"applicable": True, "fetched": True, "http_status": 200,
                                     "outcome": "available", "cache_control": "no-store"}})
        processes.append({"team_key": team, "bind_host": "127.0.0.1", "port": 52000 + index,
                          "port_ephemeral": True, "nonce_fingerprint": fp(index),
                          "browser_profile_ref": f"chrome_profile_{index}", "browser_profile_initially_empty": True,
                          "command_surface": "flowplane_dashboard", "read_only": True,
                          "dashboard_started": True, "routes": routes})
        probes = []
        for probe_id, method, status_class in (("nonce_missing", "GET", "not_found"), ("nonce_wrong", "GET", "not_found"),
                                         ("bad_host", "GET", "denied"), ("bad_origin", "GET", "denied"),
                                         ("post", "POST", "denied")):
            probes.append({"id": probe_id, "method": method, "status_class": status_class, "denied": True,
                           "state_changed": False, "cache_control": "no-store"})
        security.append({"team_key": team, "probes": probes})
        surfaces = []
        for offset, surface_id in enumerate(CLI_SURFACES):
            digest = fp(index * 10 + offset)
            count = 1 if surface_id == "cluster" else 0
            surfaces.append({"id": surface_id, "supported_cli": True, "cli_exit_status": 0,
                             "cli_row_count": count, "panel_row_count": count,
                             "cli_projection_fingerprint": digest, "panel_projection_fingerprint": digest})
        cli.append({"team_key": team, "surfaces": surfaces})
        if index == 1:
            disconnects.append({"team_key": team, "control_plane_disconnected": True, "shell_http_status": 200,
                                "own_panel_count": 3, "own_panel_outcome": "unavailable",
                                "other_team_panel_outcome": "available", "independent_degradation": True})
        if index == 2:
            auth_cases.append({"id": "beta-invalid-expired-shaped", "team_key": team, "credential_shape": "invalid-expired-shaped",
                               "shell_http_status": 200, "authentication_partial_rendered": True,
                               "panel_outcome": "authentication-required", "credential_material_visible": False,
                               "secret_shaped_value_count": 0, "other_team_panel_outcome": "available"})
        browser.append({"team_key": team, "engine": "Google Chrome", "real_window": True,
                        "profile_ref": f"chrome_profile_{index}", "profile_initially_empty": True,
                        "route": "/resources", "http_status": 200, "observed_title": "Resources · Flowplane",
                        "flowplane_resources_title_matched": True})
        lifecycle.append({"team_key": team, "cli_process_exit_observed": True,
                          "dashboard_child_exit_observed": True, "loopback_port_closed": True,
                          "bounded_shutdown": True})
    return {
        "schema": SCHEMA,
        "run": {"live_qualification": True, "rerunnable": True, "supported_surfaces_only": True,
                "direct_database_access": False, "independent_author_read_implementation": False,
                "external_infrastructure_used": False, "started_at_utc": "2026-08-22T01:00:00Z",
                "finished_at_utc": "2026-08-22T01:05:00Z", "fixture_cluster_name": "shared-overlap",
                "minimal_cluster_count": 2, "active_dataplanes_observed": 0,
                "retained_ai_history_may_render": True, "retained_mcp_history_may_render": True},
        "processes": processes,
        "isolation": {"overlapping_cluster_name": "shared-overlap", "views": [
            {"team_key": "alpha/shared", "cluster_name": "shared-overlap", "observed_revision": 1,
             "own_cluster_count": 1, "foreign_cluster_count": 0, "foreign_revision_visible": False,
             "foreign_team_marker_count": 0},
            {"team_key": "beta/shared", "cluster_name": "shared-overlap", "observed_revision": 2,
             "own_cluster_count": 1, "foreign_cluster_count": 0, "foreign_revision_visible": False,
             "foreign_team_marker_count": 0}]},
        "security": {"processes": security, "browser_bearer_material_visible": False,
                     "browser_secret_shaped_value_count": 0},
        "cli_agreement": cli,
        "degradation": {"control_plane_disconnects": disconnects, "authentication_cases": auth_cases},
        "browser": browser,
        "lifecycle": lifecycle,
        "limitations": [{"id": "live_stale_grant_mutation", "live_mutation_performed": False,
                         "automated_suite_covers_grant_transition": True,
                         "live_invalid_bearer_covers_auth_degradation": True,
                         "acceptance_claim": "live_stale_grant_mutation_not_performed"},
                        {"id": "live_cryptographic_expiry", "live_cryptographically_expired_token_used": False,
                         "invalid_expired_shaped_token_used": True, "automated_suite_covers_expired_token": True,
                         "acceptance_claim": "live_cryptographic_expiry_not_performed"},
                        {"id": "live_pagination", "live_pagination_performed": False,
                         "automated_suite_covers_pagination": True, "design_acceptance_not_claimed_live": True,
                         "acceptance_claim": "live_pagination_not_performed"}],
        "cleanup": {"evidence_frozen_before_cleanup": True, "minimal_clusters_created": 2,
                    "minimal_clusters_remaining": 0, "dashboard_processes_remaining": 0,
                    "listening_dashboard_ports_remaining": 0, "browser_profiles_remaining": 0,
                    "nonce_url_files_remaining": 0, "external_infrastructure_created": 0,
                    "retained_ai_history_allowed": True, "retained_mcp_history_allowed": True},
        "redaction": {"sanitized_projection": True, "raw_payloads_recorded": False,
                      "raw_identifiers_recorded": False, "private_paths_recorded": False,
                      "browser_secrets_recorded": False, "scan_after_final_edit": True,
                      "undisposed_match_count": 0,
                      "pattern_classes": ["bearer_tokens", "browser_storage", "private_paths", "raw_nonces",
                                          "credentials", "private_keys"]},
    }


def self_test() -> int:
    good = fixture()
    failures: list[str] = []
    for scenario_id, (_, check) in SCENARIOS.items():
        try:
            check(good)
        except ContractFailure as error:
            failures.append(f"valid fixture rejected by {scenario_id}: {error}")
    mutations: list[tuple[str, Callable[[dict[str, Any]], None], str]] = [
        ("extra root field", lambda x: x.__setitem__("unexpected", True), "FPV2-D23.9-RUN"),
        ("external infrastructure", lambda x: x["run"].__setitem__("external_infrastructure_used", True), "FPV2-D23.9-RUN"),
        ("duplicate port", lambda x: x["processes"][1].__setitem__("port", x["processes"][0]["port"]), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("duplicate nonce", lambda x: x["processes"][1].__setitem__("nonce_fingerprint", x["processes"][0]["nonce_fingerprint"]), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("reused browser profile", lambda x: x["processes"][1].__setitem__("browser_profile_ref", x["processes"][0]["browser_profile_ref"]), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("missing route", lambda x: x["processes"][0]["routes"].pop(), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("panel not fetched", lambda x: x["processes"][0]["routes"][0]["panel"].__setitem__("fetched", False), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("shell cacheable", lambda x: x["processes"][0]["routes"][0]["shell"].__setitem__("cache_control", "max-age=60"), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("panel cacheable", lambda x: x["processes"][0]["routes"][0]["panel"].__setitem__("cache_control", "max-age=60"), "FPV2-D23.9-PROCESSES-ROUTES"),
        ("cross-contaminated revision", lambda x: x["isolation"]["views"][0].__setitem__("observed_revision", 2), "FPV2-D23.9-ISOLATION"),
        ("foreign cluster visible", lambda x: x["isolation"]["views"][1].__setitem__("foreign_cluster_count", 1), "FPV2-D23.9-ISOLATION"),
        ("missing nonce accepted", lambda x: x["security"]["processes"][0]["probes"][0].__setitem__("status_class", "success"), "FPV2-D23.9-SECURITY"),
        ("bad origin accepted", lambda x: x["security"]["processes"][0]["probes"][3].__setitem__("denied", False), "FPV2-D23.9-SECURITY"),
        ("POST changed state", lambda x: x["security"]["processes"][0]["probes"][4].__setitem__("state_changed", True), "FPV2-D23.9-SECURITY"),
        ("browser bearer visible", lambda x: x["security"].__setitem__("browser_bearer_material_visible", True), "FPV2-D23.9-SECURITY"),
        ("CLI mismatch", lambda x: x["cli_agreement"][0]["surfaces"][0].__setitem__("panel_projection_fingerprint", fp(999)), "FPV2-D23.9-CLI-AGREEMENT"),
        ("NACK CLI unsupported", lambda x: x["cli_agreement"][0]["surfaces"][3].__setitem__("supported_cli", False), "FPV2-D23.9-CLI-AGREEMENT"),
        ("disconnect kills shell", lambda x: x["degradation"]["control_plane_disconnects"][0].__setitem__("shell_http_status", 503), "FPV2-D23.9-DEGRADATION"),
        ("disconnect contaminates peer", lambda x: x["degradation"]["control_plane_disconnects"][0].__setitem__("other_team_panel_outcome", "unavailable"), "FPV2-D23.9-DEGRADATION"),
        ("auth partial leak", lambda x: x["degradation"]["authentication_cases"][0].__setitem__("credential_material_visible", True), "FPV2-D23.9-DEGRADATION"),
        ("fake Chrome", lambda x: x["browser"][0].__setitem__("real_window", False), "FPV2-D23.9-CHROME"),
        ("wrong resources title", lambda x: x["browser"][0].__setitem__("observed_title", "Dashboard"), "FPV2-D23.9-CHROME"),
        ("browser/process profile mismatch", lambda x: x["browser"][0].__setitem__("profile_ref", "different_profile"), "FPV2-D23.9-CHROME"),
        ("dashboard survives CLI", lambda x: x["lifecycle"][0].__setitem__("dashboard_child_exit_observed", False), "FPV2-D23.9-LIFECYCLE"),
        ("live stale-grant claim", lambda x: x["limitations"][0].__setitem__("live_mutation_performed", True), "FPV2-D23.9-LIMITATION"),
        ("live pagination overclaim", lambda x: x["limitations"][2].__setitem__("live_pagination_performed", True), "FPV2-D23.9-LIMITATION"),
        ("cluster residue", lambda x: x["cleanup"].__setitem__("minimal_clusters_remaining", 1), "FPV2-D23.9-CLEANUP"),
        ("browser profile residue", lambda x: x["cleanup"].__setitem__("browser_profiles_remaining", 1), "FPV2-D23.9-CLEANUP"),
        ("secret-shaped key", lambda x: x.__setitem__("access_token", "forbidden"), "FPV2-D23.9-REDACTION"),
        ("private path", lambda x: x["redaction"]["pattern_classes"].append("/Users/example/private"), "FPV2-D23.9-REDACTION"),
    ]
    for label, mutate, scenario_id in mutations:
        bad = copy.deepcopy(good)
        mutate(bad)
        try:
            SCENARIOS[scenario_id][1](bad)
        except ContractFailure:
            pass
        else:
            failures.append(f"negative self-test did not fail closed: {label}")
    if failures:
        for failure in failures:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        print(f"dashboard ops acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"dashboard ops acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
    return 0


def load_evidence(path: Path) -> dict[str, Any]:
    if not path.is_file():
        fail(f"sanitized dashboard/ops evidence absent: {path.resolve()}")
    try:
        return obj(json.loads(path.read_text(encoding="utf-8")), "evidence root")
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"sanitized dashboard/ops evidence unreadable: {path.resolve()}: {error}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", help="sanitized dashboard/ops JSON projection")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), help="run exactly one independently rerunnable scenario")
    parser.add_argument("--list", action="store_true", help="list scenario IDs and exit")
    parser.add_argument("--self-test", action="store_true", help="run synthetic positive and adversarial checks")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list:
        for scenario_id, (description, _) in SCENARIOS.items():
            print(f"{scenario_id}\t{description}")
        return 0
    if args.self_test:
        return self_test()
    selected = [args.scenario] if args.scenario else list(SCENARIOS)
    configured = os.environ.get("FLOWPLANE_DASHBOARD_OPS_EVIDENCE")
    path = Path(args.evidence or configured or DEFAULT_EVIDENCE)
    try:
        evidence = load_evidence(path)
    except ContractFailure as error:
        for scenario_id in selected:
            print(f"{scenario_id}: FAIL: {error}", file=sys.stderr)
        print(f"dashboard ops acceptance: FAIL ({len(selected)}/{len(selected)} scenarios)", file=sys.stderr)
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
        print(f"dashboard ops acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"dashboard ops acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
