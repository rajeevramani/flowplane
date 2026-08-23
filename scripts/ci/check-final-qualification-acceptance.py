#!/usr/bin/env python3
"""Independent, fail-closed fpv2-d23.12 final qualification acceptance gate.

Consumes only a sanitized ``flowplane.qualification.final/v1`` JSON projection.
The qualification producer owns destructive provider work and evidence capture;
this checker validates the final acceptance contract without reading production
implementation, raw logs, credentials, browser profiles, private backups, or
private evidence. ``--self-test`` uses synthetic data and adversarial mutations.
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

SCHEMA = "flowplane.qualification.final/v1"
DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.12/final-qualification.json")
EXPECTED_FLY_APPS = ("fpq-flowplane-cp", "fpq-flowplane-db", "fpq-flowplane-rls")
EXCLUDED_FLY_APPS = ("fpq-dp-final",)
COVERAGE_IDS = tuple(f"fpv2-d23.{number}" for number in range(1, 12))
EXPECTED_COVERAGE = {
    "fpv2-d23.1": "gate0_evidence",
    "fpv2-d23.2": "capability_inventory",
    "fpv2-d23.3": "secure_topology",
    "fpv2-d23.4": "identity_tenancy",
    "fpv2-d23.5": "xds_sds_isolation",
    "fpv2-d23.6": "gateway_filter_rls",
    "fpv2-d23.7": "api_lifecycle",
    "fpv2-d23.8": "ai_mcp",
    "fpv2-d23.9": "dashboard_ops",
    "fpv2-d23.10": "failure_recovery",
    "fpv2-d23.11": "lifecycle_soak",
}
CLEANUP_KINDS = {
    "fly_apps", "fly_machines", "fly_database_volume", "fly_dedicated_ipv4",
    "cloudflare_dns", "qualification_vm", "product_prefix_rows",
    "tailscale_cp_nodes", "tailscale_rls_nodes", "tailscale_service_records",
    "tailscale_disposable_dp_nodes", "tailscale_keys", "private_backups",
    "local_credentials", "browser_profiles",
}
SEVERITIES = {"blocker", "major", "medium", "minor"}
DISPOSITIONS = {"fixed", "works_as_designed", "release_stop", "accepted_risk", "deferred"}
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REF = re.compile(r"^[a-z][a-z0-9_-]{2,63}$")
ISSUE_REF = re.compile(r"^#[1-9][0-9]*$")
BEAD_REF = re.compile(r"^(?:\.[1-9][0-9]*|fpv2-[a-z0-9]+(?:\.[1-9][0-9]*)?)$")
PROHIBITED_KEYS = {
    "token", "access_token", "refresh_token", "authorization", "password",
    "secret", "secret_value", "private_key", "private_key_pem", "ssh_key",
    "ssh_cert", "certificate_pem", "tailscale_key", "auth_key", "credential",
    "raw_log", "raw_output", "raw_request", "raw_response", "raw_body",
    "provider_payload", "private_evidence_path", "backup_path",
    "browser_profile_path", "source_vault_path", "ip_address",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~+/-]{8,}"),
    re.compile(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b"),
    re.compile(r"(?i)(?:sk|tskey)-[A-Za-z0-9_-]{8,}"),
    re.compile(r"(?i)(?:password|client_secret|api_key|ssh_key)\s*[:=]\s*\S+"),
    re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\\\Users\\\\)"),
    re.compile(r"(?i)postgres(?:ql)?://[^\s]+"),
    re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
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


def number(value: Any, name: str) -> float:
    if not isinstance(value, (int, float)) or isinstance(value, bool) or value < 0:
        fail(f"{name}: non-negative number required")
    return float(value)


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


def timestamp(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not UTC.fullmatch(candidate):
        fail(f"{name}: second-precision UTC timestamp required")
    return candidate


def digest(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not SHA256.fullmatch(candidate):
        fail(f"{name}: sha256 digest required")
    return candidate


def safe_ref(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not REF.fullmatch(candidate):
        fail(f"{name}: sanitized reference required")
    return candidate


def indexed(value: Any, name: str, key: str = "id") -> dict[str, dict[str, Any]]:
    rows: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(seq(value, name)):
        row = obj(raw, f"{name}[{index}]")
        row_key = text(row.get(key), f"{name}[{index}].{key}")
        if row_key in rows:
            fail(f"{name}: duplicate {key} {row_key!r}")
        rows[row_key] = row
    return rows


def check_run_and_gate(e: dict[str, Any]) -> None:
    exact_keys(e, {"schema", "run", "human_gate", "source_protection", "destructive_rebuild",
                   "clean_room", "dns_repair", "bootstrap", "dataplane", "quality_gates",
                   "coverage", "credential_output_incident", "defect_triage", "cleanup",
                   "auth0_cleanup", "cost_summary", "residual_risks", "final_claim", "redaction"},
               "evidence root")
    if e.get("schema") != SCHEMA:
        fail(f"schema: expected {SCHEMA!r}")
    run = obj(e.get("run"), "run")
    exact_keys(run, {"qualification_id", "release", "live_qualification", "sanitized_projection",
                     "independent_author_read_implementation", "private_evidence_read",
                     "supported_surfaces_only", "started_at_utc", "finished_at_utc"}, "run")
    if run.get("qualification_id") != "fpv2-d23.12" or run.get("release") != "3.1.3":
        fail("run: exact fpv2-d23.12 / 3.1.3 qualification required")
    for key in ("live_qualification", "sanitized_projection", "supported_surfaces_only"):
        boolean(run.get(key), f"run.{key}", True)
    for key in ("independent_author_read_implementation", "private_evidence_read"):
        boolean(run.get(key), f"run.{key}", False)
    if timestamp(run.get("finished_at_utc"), "run.finished_at_utc") <= timestamp(run.get("started_at_utc"), "run.started_at_utc"):
        fail("run: finish must post-date start")
    gate = obj(e.get("human_gate"), "human_gate")
    exact_keys(gate, {"destructive_action", "approval_required", "approved", "approver_ref",
                      "approved_at_utc", "approval_preceded_destruction"}, "human_gate")
    if gate.get("destructive_action") != "destroy_and_rebuild_existing_qualification":
        fail("human_gate.destructive_action: exact destructive action required")
    for key in ("approval_required", "approved", "approval_preceded_destruction"):
        boolean(gate.get(key), f"human_gate.{key}", True)
    safe_ref(gate.get("approver_ref"), "human_gate.approver_ref")
    if timestamp(gate.get("approved_at_utc"), "human_gate.approved_at_utc") >= timestamp(run.get("started_at_utc"), "run.started_at_utc"):
        fail("human_gate: approval must pre-date qualification start")


def check_source_protection(e: dict[str, Any]) -> None:
    protection = obj(e.get("source_protection"), "source_protection")
    exact_keys(protection, {"input_freeze_before_destruction", "private_backup_completed",
                            "private_backup_verified", "source_repo_destructive_write_count",
                            "source_vault_destructive_write_count", "source_repositories_protected",
                            "source_vault_protected"}, "source_protection")
    for key in ("input_freeze_before_destruction", "private_backup_completed", "private_backup_verified",
                "source_repositories_protected", "source_vault_protected"):
        boolean(protection.get(key), f"source_protection.{key}", True)
    integer(protection.get("source_repo_destructive_write_count"), "source_protection.source_repo_destructive_write_count", 0)
    integer(protection.get("source_vault_destructive_write_count"), "source_protection.source_vault_destructive_write_count", 0)


def check_destructive_rebuild(e: dict[str, Any]) -> None:
    rebuild = obj(e.get("destructive_rebuild"), "destructive_rebuild")
    exact_keys(rebuild, {"preexisting_apps", "destroyed_after_backup_and_freeze", "rebuilt_apps",
                         "candidate_digest", "deployed_digest", "digest_pinned",
                         "postgresql", "tailscale_keys"}, "destructive_rebuild")
    if rebuild.get("preexisting_apps") != list(EXPECTED_FLY_APPS) or rebuild.get("rebuilt_apps") != list(EXPECTED_FLY_APPS):
        fail(f"destructive_rebuild: exact Fly apps {list(EXPECTED_FLY_APPS)!r} required")
    boolean(rebuild.get("destroyed_after_backup_and_freeze"), "destructive_rebuild.destroyed_after_backup_and_freeze", True)
    expected = digest(rebuild.get("candidate_digest"), "destructive_rebuild.candidate_digest")
    if digest(rebuild.get("deployed_digest"), "destructive_rebuild.deployed_digest") != expected:
        fail("destructive_rebuild: deployed digest must equal pinned candidate digest")
    boolean(rebuild.get("digest_pinned"), "destructive_rebuild.digest_pinned", True)
    postgres = obj(rebuild.get("postgresql"), "destructive_rebuild.postgresql")
    exact_keys(postgres, {"fresh", "version", "memory_mib", "volume_count", "ready"}, "destructive_rebuild.postgresql")
    boolean(postgres.get("fresh"), "destructive_rebuild.postgresql.fresh", True)
    if postgres.get("version") != "18.1":
        fail("destructive_rebuild.postgresql.version: 18.1 required")
    integer(postgres.get("memory_mib"), "destructive_rebuild.postgresql.memory_mib", 256)
    integer(postgres.get("volume_count"), "destructive_rebuild.postgresql.volume_count", 1)
    boolean(postgres.get("ready"), "destructive_rebuild.postgresql.ready", True)
    keys = indexed(rebuild.get("tailscale_keys"), "destructive_rebuild.tailscale_keys", "scope")
    if set(keys) != {"control_plane", "rls", "dataplane"}:
        fail("destructive_rebuild.tailscale_keys: exact CP/RLS/DP key scopes required")
    for scope, row in keys.items():
        name = f"destructive_rebuild.tailscale_keys[{scope}]"
        exact_keys(row, {"scope", "fresh", "tagged", "disposable", "value_recorded"}, name)
        boolean(row.get("fresh"), f"{name}.fresh", True)
        boolean(row.get("tagged"), f"{name}.tagged", scope != "dataplane")
        boolean(row.get("disposable"), f"{name}.disposable", scope == "dataplane")
        boolean(row.get("value_recorded"), f"{name}.value_recorded", False)


def check_clean_room(e: dict[str, Any]) -> None:
    room = obj(e.get("clean_room"), "clean_room")
    exact_keys(room, {"fresh_resources_only", "same_fly_app_names", "old_resource_reuse_count",
                      "control_plane_ready", "database_ready", "rls_ready",
                      "cp_tailscale_tagged", "rls_tailscale_tagged",
                      "unrelated_app_exclusions"}, "clean_room")
    for key in ("fresh_resources_only", "same_fly_app_names", "control_plane_ready", "database_ready",
                "rls_ready", "cp_tailscale_tagged", "rls_tailscale_tagged"):
        boolean(room.get(key), f"clean_room.{key}", True)
    integer(room.get("old_resource_reuse_count"), "clean_room.old_resource_reuse_count", 0)
    if room.get("unrelated_app_exclusions") != list(EXCLUDED_FLY_APPS):
        fail("clean_room.unrelated_app_exclusions: fpq-dp-final must be explicitly excluded")


def check_dns(e: dict[str, Any]) -> None:
    dns = obj(e.get("dns_repair"), "dns_repair")
    exact_keys(dns, {"stale_old_a_discovered", "dedicated_ipv4_allocated", "ipv4_fingerprint",
                     "cloudflare_record_ref", "record_type", "exact_a_record_updated",
                     "observed_target_fingerprint", "public_readiness_passed"}, "dns_repair")
    for key in ("stale_old_a_discovered", "dedicated_ipv4_allocated", "exact_a_record_updated", "public_readiness_passed"):
        boolean(dns.get(key), f"dns_repair.{key}", True)
    expected = digest(dns.get("ipv4_fingerprint"), "dns_repair.ipv4_fingerprint")
    if digest(dns.get("observed_target_fingerprint"), "dns_repair.observed_target_fingerprint") != expected:
        fail("dns_repair: Cloudflare A target must equal the dedicated IPv4")
    safe_ref(dns.get("cloudflare_record_ref"), "dns_repair.cloudflare_record_ref")
    if dns.get("record_type") != "A":
        fail("dns_repair.record_type: exact A record required")


def check_bootstrap_and_dataplane(e: dict[str, Any]) -> None:
    bootstrap = obj(e.get("bootstrap"), "bootstrap")
    exact_keys(bootstrap, {"clean_database", "supported_surface", "direct_database_access",
                           "governance_platform_created", "tenant_org_count", "tenant_team_count",
                           "product_prefix", "runtime_resource_count"}, "bootstrap")
    boolean(bootstrap.get("clean_database"), "bootstrap.clean_database", True)
    if bootstrap.get("supported_surface") != "api_or_cli":
        fail("bootstrap.supported_surface: supported API/CLI required")
    boolean(bootstrap.get("direct_database_access"), "bootstrap.direct_database_access", False)
    boolean(bootstrap.get("governance_platform_created"), "bootstrap.governance_platform_created", True)
    if integer(bootstrap.get("tenant_org_count"), "bootstrap.tenant_org_count") < 1:
        fail("bootstrap.tenant_org_count: at least one tenant organization required")
    if integer(bootstrap.get("tenant_team_count"), "bootstrap.tenant_team_count") < 1:
        fail("bootstrap.tenant_team_count: at least one tenant team required")
    if bootstrap.get("product_prefix") != "fpq12":
        fail("bootstrap.product_prefix: exact fpq12 prefix required")
    integer(bootstrap.get("runtime_resource_count"), "bootstrap.runtime_resource_count", 4)
    dp = obj(e.get("dataplane"), "dataplane")
    exact_keys(dp, {"vm_ref", "fresh", "mount_count", "architecture", "registered",
                    "certificate_bound", "agent_candidate", "envoy_candidate", "xds_private",
                    "xds_real_san", "xds_san_matches_endpoint", "traffic_tag_ref",
                    "tagged_traffic_passed", "agent_health_passed"}, "dataplane")
    safe_ref(dp.get("vm_ref"), "dataplane.vm_ref")
    safe_ref(dp.get("traffic_tag_ref"), "dataplane.traffic_tag_ref")
    for key in ("fresh", "registered", "certificate_bound", "agent_candidate", "envoy_candidate",
                "xds_private", "xds_real_san", "xds_san_matches_endpoint", "tagged_traffic_passed",
                "agent_health_passed"):
        boolean(dp.get(key), f"dataplane.{key}", True)
    integer(dp.get("mount_count"), "dataplane.mount_count", 0)
    if dp.get("architecture") != "x86_64":
        fail("dataplane.architecture: x86_64 required")


def check_quality_and_coverage(e: dict[str, Any]) -> None:
    quality = obj(e.get("quality_gates"), "quality_gates")
    exact_keys(quality, {"workspace", "static_gates"}, "quality_gates")
    workspace = obj(quality.get("workspace"), "quality_gates.workspace")
    exact_keys(workspace, {"selected", "passed", "failed", "ignored", "complete"}, "quality_gates.workspace")
    integer(workspace.get("selected"), "quality_gates.workspace.selected", 1470)
    integer(workspace.get("passed"), "quality_gates.workspace.passed", 1470)
    integer(workspace.get("failed"), "quality_gates.workspace.failed", 0)
    integer(workspace.get("ignored"), "quality_gates.workspace.ignored", 0)
    boolean(workspace.get("complete"), "quality_gates.workspace.complete", True)
    gates = indexed(quality.get("static_gates"), "quality_gates.static_gates")
    if not gates:
        fail("quality_gates.static_gates: at least one static gate required")
    for gate_id, row in gates.items():
        name = f"quality_gates.static_gates[{gate_id}]"
        exact_keys(row, {"id", "command_ref", "exit_status", "passed"}, name)
        safe_ref(gate_id, f"{name}.id")
        safe_ref(row.get("command_ref"), f"{name}.command_ref")
        integer(row.get("exit_status"), f"{name}.exit_status", 0)
        boolean(row.get("passed"), f"{name}.passed", True)
    coverage = indexed(e.get("coverage"), "coverage")
    if set(coverage) != set(COVERAGE_IDS):
        fail("coverage: exact sanitized references for fpv2-d23.1 through fpv2-d23.11 required")
    for coverage_id, row in coverage.items():
        name = f"coverage[{coverage_id}]"
        exact_keys(row, {"id", "evidence_ref", "sanitized", "accepted"}, name)
        if safe_ref(row.get("evidence_ref"), f"{name}.evidence_ref") != EXPECTED_COVERAGE[coverage_id]:
            fail(f"{name}.evidence_ref: exact bead-to-evidence mapping required")
        boolean(row.get("sanitized"), f"{name}.sanitized", True)
        boolean(row.get("accepted"), f"{name}.accepted", True)


def check_incident(e: dict[str, Any]) -> None:
    incident = obj(e.get("credential_output_incident"), "credential_output_incident")
    exact_keys(incident, {"source_qualification", "occurred", "classification", "secret_values_recorded",
                          "fly_ssh_key_rotated", "fly_ssh_certificate_rotated", "remediation_verified"},
               "credential_output_incident")
    if incident.get("source_qualification") != "fpv2-d23.11" or incident.get("classification") != "harness_incident":
        fail("credential_output_incident: exact .11 harness-incident classification required")
    boolean(incident.get("occurred"), "credential_output_incident.occurred", True)
    boolean(incident.get("secret_values_recorded"), "credential_output_incident.secret_values_recorded", False)
    for key in ("fly_ssh_key_rotated", "fly_ssh_certificate_rotated", "remediation_verified"):
        boolean(incident.get(key), f"credential_output_incident.{key}", True)


def check_defects(e: dict[str, Any]) -> None:
    triage = obj(e.get("defect_triage"), "defect_triage")
    exact_keys(triage, {"github_milestone", "milestone_issue_total", "linked_bead_total",
                        "issues", "verified_open_defects", "inventory_complete"}, "defect_triage")
    if triage.get("github_milestone") != "3.1.3":
        fail("defect_triage.github_milestone: exact 3.1.3 milestone required")
    issues = indexed(triage.get("issues"), "defect_triage.issues", "github_ref")
    integer(triage.get("milestone_issue_total"), "defect_triage.milestone_issue_total", len(issues))
    linked_count = sum(row.get("bead_ref") is not None for row in issues.values())
    integer(triage.get("linked_bead_total"), "defect_triage.linked_bead_total", linked_count)
    boolean(triage.get("inventory_complete"), "defect_triage.inventory_complete", True)
    if "#255" not in issues:
        fail("defect_triage.issues: GitHub #255 required")
    open_refs: set[str] = set()
    bead_refs: set[str] = set()
    for github_ref, row in issues.items():
        name = f"defect_triage.issues[{github_ref}]"
        exact_keys(row, {"github_ref", "bead_ref", "state", "severity", "disposition",
                         "human_disposition", "sanitized_evidence_ref"}, name)
        if not ISSUE_REF.fullmatch(github_ref):
            fail(f"{name}.github_ref: sanitized GitHub issue reference required")
        bead_ref = row.get("bead_ref")
        if bead_ref is not None:
            bead_ref = text(bead_ref, f"{name}.bead_ref")
            if not BEAD_REF.fullmatch(bead_ref) or bead_ref in bead_refs:
                fail(f"{name}.bead_ref: distinct linked Bead reference required")
            bead_refs.add(bead_ref)
        severity = row.get("severity")
        disposition = row.get("disposition")
        if severity not in SEVERITIES or disposition not in DISPOSITIONS:
            fail(f"{name}: valid severity and disposition required")
        if row.get("state") not in {"open", "closed"}:
            fail(f"{name}.state: open or closed required")
        safe_ref(row.get("sanitized_evidence_ref"), f"{name}.sanitized_evidence_ref")
        if row.get("state") == "open":
            open_refs.add(github_ref)
            if severity in {"blocker", "major"}:
                fail(f"{name}: unresolved {severity} is a release stop")
            boolean(row.get("human_disposition"), f"{name}.human_disposition", True)
            if disposition not in {"accepted_risk", "deferred"}:
                fail(f"{name}: open medium/minor requires accepted-risk or deferred disposition")
        elif disposition not in {"fixed", "works_as_designed"}:
            fail(f"{name}: closed issue requires fixed or works-as-designed disposition")
    harness = issues["#255"]
    if harness.get("bead_ref") != "fpv2-d23.14" or harness.get("state") != "closed" or harness.get("disposition") != "works_as_designed":
        fail("defect_triage: #255 and Bead .14 must be closed harness-defect works-as-designed")
    verified = indexed(triage.get("verified_open_defects"), "defect_triage.verified_open_defects", "github_ref")
    if set(verified) != open_refs:
        fail("defect_triage.verified_open_defects: exact open issue inventory required")
    for github_ref, row in verified.items():
        name = f"defect_triage.verified_open_defects[{github_ref}]"
        exact_keys(row, {"github_ref", "bead_ref", "severity", "disposition", "sanitized_evidence_ref"}, name)
        issue = issues[github_ref]
        for key in ("bead_ref", "severity", "disposition", "sanitized_evidence_ref"):
            if row.get(key) != issue.get(key):
                fail(f"{name}.{key}: must match milestone triage inventory")


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(e.get("cleanup"), "cleanup")
    exact_keys(cleanup, {"evidence_frozen_before_cleanup", "clean_room_destroyed", "fly_app_names_absent",
                         "excluded_fly_apps_untouched", "dns_record_ref_absent", "qualification_vm_ref_absent",
                         "product_prefix", "product_prefix_remaining_count", "inventories"}, "cleanup")
    for key in ("evidence_frozen_before_cleanup", "clean_room_destroyed"):
        boolean(cleanup.get(key), f"cleanup.{key}", True)
    if cleanup.get("fly_app_names_absent") != list(EXPECTED_FLY_APPS):
        fail("cleanup.fly_app_names_absent: exact CP/DB/RLS names required")
    if cleanup.get("excluded_fly_apps_untouched") != list(EXCLUDED_FLY_APPS):
        fail("cleanup.excluded_fly_apps_untouched: unrelated fpq-dp-final exclusion required")
    if cleanup.get("dns_record_ref_absent") != field(e, "dns_repair.cloudflare_record_ref"):
        fail("cleanup.dns_record_ref_absent: exact qualification DNS record must be absent")
    if cleanup.get("qualification_vm_ref_absent") != field(e, "dataplane.vm_ref"):
        fail("cleanup.qualification_vm_ref_absent: exact clean-room VM must be absent")
    if cleanup.get("product_prefix") != "fpq12":
        fail("cleanup.product_prefix: exact fpq12 prefix required")
    integer(cleanup.get("product_prefix_remaining_count"), "cleanup.product_prefix_remaining_count", 0)
    rows = indexed(cleanup.get("inventories"), "cleanup.inventories", "resource_kind")
    if set(rows) != CLEANUP_KINDS:
        fail(f"cleanup.inventories: exact zero-cleanup inventory required; got {sorted(rows)!r}")
    for kind, row in rows.items():
        name = f"cleanup.inventories[{kind}]"
        exact_keys(row, {"resource_kind", "remaining_count", "verification_class", "authoritative_check"}, name)
        integer(row.get("remaining_count"), f"{name}.remaining_count", 0)
        boolean(row.get("authoritative_check"), f"{name}.authoritative_check", True)
        if row.get("verification_class") != "machine_verified":
            fail(f"{name}.verification_class: machine_verified required")


def check_auth0(e: dict[str, Any]) -> None:
    auth0 = obj(e.get("auth0_cleanup"), "auth0_cleanup")
    exact_keys(auth0, {"tenant_classification", "shared_tenant_human_approved", "preexisting_resources_untouched",
                       "default_audience_restored", "provider_logs_expected", "reduced_isolation_limitation_recorded",
                       "synthetic_api_deleted", "synthetic_app_deleted",
                       "synthetic_user_count", "synthetic_users_deleted", "performed_by_user_in_dashboard",
                       "cli_token_expired", "machine_verified", "classification"}, "auth0_cleanup")
    if auth0.get("tenant_classification") != "shared_free_tenant":
        fail("auth0_cleanup.tenant_classification: approved shared free tenant required")
    for key in ("shared_tenant_human_approved", "preexisting_resources_untouched", "default_audience_restored",
                "provider_logs_expected", "reduced_isolation_limitation_recorded",
                "synthetic_api_deleted", "synthetic_app_deleted",
                "synthetic_users_deleted", "performed_by_user_in_dashboard", "cli_token_expired"):
        boolean(auth0.get(key), f"auth0_cleanup.{key}", True)
    integer(auth0.get("synthetic_user_count"), "auth0_cleanup.synthetic_user_count", 8)
    boolean(auth0.get("machine_verified"), "auth0_cleanup.machine_verified", False)
    if auth0.get("classification") != "user_attested":
        fail("auth0_cleanup.classification: user_attested required; machine verification must not be claimed")


def check_reporting_and_claim(e: dict[str, Any]) -> None:
    cost = obj(e.get("cost_summary"), "cost_summary")
    exact_keys(cost, {"approved_ceiling_usd", "ceiling_evidence_ref", "exact_cost_measured", "estimated_cost_recorded", "resource_counts_complete", "resources"}, "cost_summary")
    if number(cost.get("approved_ceiling_usd"), "cost_summary.approved_ceiling_usd") != 10:
        fail("cost_summary.approved_ceiling_usd: exact Gate 0 US$10 ceiling required")
    if safe_ref(cost.get("ceiling_evidence_ref"), "cost_summary.ceiling_evidence_ref") != "gate0_cost_ceiling":
        fail("cost_summary.ceiling_evidence_ref: Gate 0 ceiling reference required")
    boolean(cost.get("exact_cost_measured"), "cost_summary.exact_cost_measured", False)
    boolean(cost.get("estimated_cost_recorded"), "cost_summary.estimated_cost_recorded", False)
    boolean(cost.get("resource_counts_complete"), "cost_summary.resource_counts_complete", True)
    resources = indexed(cost.get("resources"), "cost_summary.resources", "resource_kind")
    if not resources:
        fail("cost_summary.resources: non-empty resource summary required")
    for kind, row in resources.items():
        name = f"cost_summary.resources[{kind}]"
        exact_keys(row, {"resource_kind", "created_count", "remaining_count", "billable"}, name)
        safe_ref(kind, f"{name}.resource_kind")
        integer(row.get("created_count"), f"{name}.created_count")
        integer(row.get("remaining_count"), f"{name}.remaining_count", 0)
        if not isinstance(row.get("billable"), bool):
            fail(f"{name}.billable: boolean required")
    risks = indexed(e.get("residual_risks"), "residual_risks")
    if not risks:
        fail("residual_risks: explicit non-empty residual-risk inventory required")
    for risk_id, row in risks.items():
        name = f"residual_risks[{risk_id}]"
        exact_keys(row, {"id", "severity", "disposition", "human_disposition", "sanitized_ref"}, name)
        safe_ref(risk_id, f"{name}.id")
        if row.get("severity") not in {"medium", "minor"}:
            fail(f"{name}.severity: only disposed medium/minor residual risk allowed")
        if row.get("disposition") not in {"accepted_risk", "deferred"}:
            fail(f"{name}.disposition: accepted_risk or deferred required")
        boolean(row.get("human_disposition"), f"{name}.human_disposition", True)
        safe_ref(row.get("sanitized_ref"), f"{name}.sanitized_ref")
    claim = obj(e.get("final_claim"), "final_claim")
    exact_keys(claim, {"ready", "verdict", "qualification", "production_shaped", "ha_claimed",
                       "production_grade_claimed", "release_claimed", "tag_claimed",
                       "publication_claimed", "cleanup_complete", "local_cleanup_zero",
                       "source_protection_preserved"}, "final_claim")
    if claim.get("qualification") != "final_qualification_acceptance":
        fail("final_claim.qualification: exact final qualification acceptance required")
    if claim.get("verdict") != "ready_with_limitations":
        fail("final_claim.verdict: exact ready_with_limitations verdict required")
    for key in ("ready", "production_shaped", "cleanup_complete", "local_cleanup_zero", "source_protection_preserved"):
        boolean(claim.get(key), f"final_claim.{key}", True)
    for key in ("ha_claimed", "production_grade_claimed", "release_claimed", "tag_claimed", "publication_claimed"):
        boolean(claim.get(key), f"final_claim.{key}", False)


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
    redaction = obj(e.get("redaction"), "redaction")
    exact_keys(redaction, {"sanitized_projection", "secret_values_recorded", "raw_logs_recorded",
                           "raw_identifiers_recorded", "private_paths_recorded", "ip_values_recorded",
                           "scan_after_final_edit", "undisposed_match_count", "pattern_classes"}, "redaction")
    boolean(redaction.get("sanitized_projection"), "redaction.sanitized_projection", True)
    for key in ("secret_values_recorded", "raw_logs_recorded", "raw_identifiers_recorded",
                "private_paths_recorded", "ip_values_recorded"):
        boolean(redaction.get(key), f"redaction.{key}", False)
    boolean(redaction.get("scan_after_final_edit"), "redaction.scan_after_final_edit", True)
    integer(redaction.get("undisposed_match_count"), "redaction.undisposed_match_count", 0)
    required = {"credentials", "bearer_tokens", "private_keys", "certificate_bodies", "database_urls",
                "private_paths", "raw_identifiers", "raw_logs", "ip_addresses", "browser_profiles"}
    if set(seq(redaction.get("pattern_classes"), "redaction.pattern_classes")) != required:
        fail("redaction.pattern_classes: exact strict scan classes required")
    walk(e)


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.12-RUN-GATE": ("sanitized independent run with prior destructive human approval", check_run_and_gate),
    "FPV2-D23.12-SOURCE-PROTECTION": ("frozen inputs, private backup and protected source repositories/vault", check_source_protection),
    "FPV2-D23.12-DESTRUCTIVE-REBUILD": ("same-name pinned clean-room Fly rebuild with fresh PostgreSQL and Tailscale keys", check_destructive_rebuild),
    "FPV2-D23.12-CLEAN-ROOM": ("fresh CP/DB/RLS resources with unrelated fpq-dp-final excluded", check_clean_room),
    "FPV2-D23.12-DNS": ("stale A discovery, dedicated IPv4 repair and public readiness", check_dns),
    "FPV2-D23.12-BOOTSTRAP-DP": ("supported bootstrap and mount-free certificate-bound real-SAN dataplane traffic", check_bootstrap_and_dataplane),
    "FPV2-D23.12-QUALITY-COVERAGE": ("1470/1470 workspace, static gates and .1-.11 sanitized coverage", check_quality_and_coverage),
    "FPV2-D23.12-INCIDENT": (".11 credential-output harness incident remediated by Fly SSH rotation", check_incident),
    "FPV2-D23.12-DEFECT-TRIAGE": ("complete 3.1.3 GitHub/Beads triage and release-stop policy", check_defects),
    "FPV2-D23.12-CLEANUP": ("machine-verified zero clean-room resources, credentials and profiles", check_cleanup),
    "FPV2-D23.12-AUTH0": ("eight-user approved shared-tenant Auth0 cleanup honestly user-attested", check_auth0),
    "FPV2-D23.12-REPORT-CLAIM": ("costs, residual risks and production-shaped-only readiness claim", check_reporting_and_claim),
    "FPV2-D23.12-REDACTION": ("strict secret, path, identifier, IP, log and browser-profile redaction", check_redaction),
}


def fp(number: int) -> str:
    return "sha256:" + f"{number:064x}"


def fixture() -> dict[str, Any]:
    cleanup = [{"resource_kind": kind, "remaining_count": 0, "verification_class": "machine_verified",
                "authoritative_check": True} for kind in sorted(CLEANUP_KINDS)]
    coverage = [{"id": item, "evidence_ref": EXPECTED_COVERAGE[item], "sanitized": True, "accepted": True}
                for item in COVERAGE_IDS]
    issues = [
        {"github_ref": "#252", "bead_ref": "fpv2-do5", "state": "closed", "severity": "major",
         "disposition": "fixed", "human_disposition": True,
         "sanitized_evidence_ref": "xds_san_candidate_evidence"},
        {"github_ref": "#254", "bead_ref": "fpv2-a09", "state": "closed", "severity": "major",
         "disposition": "fixed", "human_disposition": True,
         "sanitized_evidence_ref": "secret_delete_candidate_evidence"},
        {"github_ref": "#255", "bead_ref": "fpv2-d23.14", "state": "closed", "severity": "major",
         "disposition": "works_as_designed", "human_disposition": True,
         "sanitized_evidence_ref": "harness_triage"},
        {"github_ref": "#253", "bead_ref": "fpv2-jcs", "state": "closed", "severity": "major",
         "disposition": "fixed", "human_disposition": True,
         "sanitized_evidence_ref": "certificate_issuer_candidate_evidence"},
    ]
    return {
        "schema": SCHEMA,
        "run": {"qualification_id": "fpv2-d23.12", "release": "3.1.3", "live_qualification": True,
                "sanitized_projection": True, "independent_author_read_implementation": False,
                "private_evidence_read": False, "supported_surfaces_only": True,
                "started_at_utc": "2026-08-23T02:00:00Z", "finished_at_utc": "2026-08-23T04:00:00Z"},
        "human_gate": {"destructive_action": "destroy_and_rebuild_existing_qualification",
                       "approval_required": True, "approved": True, "approver_ref": "human_approver",
                       "approved_at_utc": "2026-08-23T01:00:00Z", "approval_preceded_destruction": True},
        "source_protection": {"input_freeze_before_destruction": True, "private_backup_completed": True,
                              "private_backup_verified": True, "source_repo_destructive_write_count": 0,
                              "source_vault_destructive_write_count": 0, "source_repositories_protected": True,
                              "source_vault_protected": True},
        "destructive_rebuild": {"preexisting_apps": list(EXPECTED_FLY_APPS),
                                "destroyed_after_backup_and_freeze": True,
                                "rebuilt_apps": list(EXPECTED_FLY_APPS), "candidate_digest": fp(1),
                                "deployed_digest": fp(1), "digest_pinned": True,
                                "postgresql": {"fresh": True, "version": "18.1", "memory_mib": 256,
                                               "volume_count": 1, "ready": True},
                                "tailscale_keys": [
                                    {"scope": "control_plane", "fresh": True, "tagged": True,
                                     "disposable": False, "value_recorded": False},
                                    {"scope": "rls", "fresh": True, "tagged": True,
                                     "disposable": False, "value_recorded": False},
                                    {"scope": "dataplane", "fresh": True, "tagged": False,
                                     "disposable": True, "value_recorded": False}]},
        "clean_room": {"fresh_resources_only": True, "same_fly_app_names": True,
                       "old_resource_reuse_count": 0, "control_plane_ready": True, "database_ready": True,
                       "rls_ready": True, "cp_tailscale_tagged": True, "rls_tailscale_tagged": True,
                       "unrelated_app_exclusions": list(EXCLUDED_FLY_APPS)},
        "dns_repair": {"stale_old_a_discovered": True, "dedicated_ipv4_allocated": True,
                       "ipv4_fingerprint": fp(2), "cloudflare_record_ref": "qualification_dns",
                       "record_type": "A", "exact_a_record_updated": True,
                       "observed_target_fingerprint": fp(2), "public_readiness_passed": True},
        "bootstrap": {"clean_database": True, "supported_surface": "api_or_cli", "direct_database_access": False,
                      "governance_platform_created": True, "tenant_org_count": 1, "tenant_team_count": 1,
                      "product_prefix": "fpq12", "runtime_resource_count": 4},
        "dataplane": {"vm_ref": "qualification_vm", "fresh": True, "mount_count": 0,
                      "architecture": "x86_64", "registered": True, "certificate_bound": True,
                      "agent_candidate": True, "envoy_candidate": True, "xds_private": True,
                      "xds_real_san": True, "xds_san_matches_endpoint": True,
                      "traffic_tag_ref": "traffic_final", "tagged_traffic_passed": True,
                      "agent_health_passed": True},
        "quality_gates": {"workspace": {"selected": 1470, "passed": 1470, "failed": 0,
                                         "ignored": 0, "complete": True},
                          "static_gates": [{"id": "static_contracts", "command_ref": "static_command",
                                            "exit_status": 0, "passed": True}]},
        "coverage": coverage,
        "credential_output_incident": {"source_qualification": "fpv2-d23.11", "occurred": True,
                                       "classification": "harness_incident", "secret_values_recorded": False,
                                       "fly_ssh_key_rotated": True, "fly_ssh_certificate_rotated": True,
                                       "remediation_verified": True},
        "defect_triage": {"github_milestone": "3.1.3", "milestone_issue_total": 4,
                          "linked_bead_total": 4, "issues": issues,
                          "verified_open_defects": [],
                          "inventory_complete": True},
        "cleanup": {"evidence_frozen_before_cleanup": True, "clean_room_destroyed": True,
                    "fly_app_names_absent": list(EXPECTED_FLY_APPS),
                    "excluded_fly_apps_untouched": list(EXCLUDED_FLY_APPS),
                    "dns_record_ref_absent": "qualification_dns",
                    "qualification_vm_ref_absent": "qualification_vm",
                    "product_prefix": "fpq12", "product_prefix_remaining_count": 0,
                    "inventories": cleanup},
        "auth0_cleanup": {"tenant_classification": "shared_free_tenant", "shared_tenant_human_approved": True,
                          "preexisting_resources_untouched": True, "default_audience_restored": True,
                          "provider_logs_expected": True, "reduced_isolation_limitation_recorded": True,
                          "synthetic_api_deleted": True,
                          "synthetic_app_deleted": True, "synthetic_user_count": 8,
                          "synthetic_users_deleted": True, "performed_by_user_in_dashboard": True,
                          "cli_token_expired": True, "machine_verified": False,
                          "classification": "user_attested"},
        "cost_summary": {"approved_ceiling_usd": 10, "ceiling_evidence_ref": "gate0_cost_ceiling",
                         "exact_cost_measured": False, "estimated_cost_recorded": False, "resource_counts_complete": True,
                         "resources": [{"resource_kind": "fly_resources", "created_count": 7,
                                        "remaining_count": 0, "billable": True}]},
        "residual_risks": [{"id": "auth0_user_attestation", "severity": "minor",
                            "disposition": "accepted_risk", "human_disposition": True,
                            "sanitized_ref": "auth0_cleanup_attestation"}],
        "final_claim": {"ready": True, "verdict": "ready_with_limitations", "qualification": "final_qualification_acceptance",
                        "production_shaped": True, "ha_claimed": False, "production_grade_claimed": False,
                        "release_claimed": False, "tag_claimed": False, "publication_claimed": False,
                        "cleanup_complete": True, "local_cleanup_zero": True,
                        "source_protection_preserved": True},
        "redaction": {"sanitized_projection": True, "secret_values_recorded": False,
                      "raw_logs_recorded": False, "raw_identifiers_recorded": False,
                      "private_paths_recorded": False, "ip_values_recorded": False,
                      "scan_after_final_edit": True, "undisposed_match_count": 0,
                      "pattern_classes": ["credentials", "bearer_tokens", "private_keys",
                                          "certificate_bodies", "database_urls", "private_paths",
                                          "raw_identifiers", "raw_logs", "ip_addresses", "browser_profiles"]},
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
        ("human gate missing", lambda x: x["human_gate"].__setitem__("approved", False), "FPV2-D23.12-RUN-GATE"),
        ("approval after start", lambda x: x["human_gate"].__setitem__("approved_at_utc", "2026-08-23T03:00:00Z"), "FPV2-D23.12-RUN-GATE"),
        ("source repo destructively changed", lambda x: x["source_protection"].__setitem__("source_repo_destructive_write_count", 1), "FPV2-D23.12-SOURCE-PROTECTION"),
        ("wrong Fly app name", lambda x: x["destructive_rebuild"]["rebuilt_apps"].__setitem__(0, "fpq-cp-other"), "FPV2-D23.12-DESTRUCTIVE-REBUILD"),
        ("candidate digest drift", lambda x: x["destructive_rebuild"].__setitem__("deployed_digest", fp(9)), "FPV2-D23.12-DESTRUCTIVE-REBUILD"),
        ("wrong PostgreSQL version", lambda x: x["destructive_rebuild"]["postgresql"].__setitem__("version", "17.6"), "FPV2-D23.12-DESTRUCTIVE-REBUILD"),
        ("DP key tagged", lambda x: x["destructive_rebuild"]["tailscale_keys"][2].__setitem__("tagged", True), "FPV2-D23.12-DESTRUCTIVE-REBUILD"),
        ("unrelated DP not excluded", lambda x: x["clean_room"].__setitem__("unrelated_app_exclusions", []), "FPV2-D23.12-CLEAN-ROOM"),
        ("stale DNS not discovered", lambda x: x["dns_repair"].__setitem__("stale_old_a_discovered", False), "FPV2-D23.12-DNS"),
        ("DNS target mismatch", lambda x: x["dns_repair"].__setitem__("observed_target_fingerprint", fp(8)), "FPV2-D23.12-DNS"),
        ("direct DB bootstrap", lambda x: x["bootstrap"].__setitem__("direct_database_access", True), "FPV2-D23.12-BOOTSTRAP-DP"),
        ("runtime resource count drift", lambda x: x["bootstrap"].__setitem__("runtime_resource_count", 3), "FPV2-D23.12-BOOTSTRAP-DP"),
        ("host-mounted VM", lambda x: x["dataplane"].__setitem__("mount_count", 1), "FPV2-D23.12-BOOTSTRAP-DP"),
        ("xDS SAN mismatch", lambda x: x["dataplane"].__setitem__("xds_san_matches_endpoint", False), "FPV2-D23.12-BOOTSTRAP-DP"),
        ("workspace one failure", lambda x: x["quality_gates"]["workspace"].__setitem__("passed", 1469), "FPV2-D23.12-QUALITY-COVERAGE"),
        ("missing .11 coverage", lambda x: x["coverage"].pop(), "FPV2-D23.12-QUALITY-COVERAGE"),
        ("shifted coverage reference", lambda x: x["coverage"][1].__setitem__("evidence_ref", "secure_topology"), "FPV2-D23.12-QUALITY-COVERAGE"),
        ("SSH cert not rotated", lambda x: x["credential_output_incident"].__setitem__("fly_ssh_certificate_rotated", False), "FPV2-D23.12-INCIDENT"),
        ("missing #255", lambda x: x["defect_triage"]["issues"].pop(2), "FPV2-D23.12-DEFECT-TRIAGE"),
        ("#255 still open", lambda x: x["defect_triage"]["issues"][2].__setitem__("state", "open"), "FPV2-D23.12-DEFECT-TRIAGE"),
        ("unresolved major", lambda x: x["defect_triage"]["issues"][0].__setitem__("state", "open"), "FPV2-D23.12-DEFECT-TRIAGE"),
        ("linked count drift", lambda x: x["defect_triage"].__setitem__("linked_bead_total", 2), "FPV2-D23.12-DEFECT-TRIAGE"),
        ("open inventory omitted", lambda x: x["defect_triage"]["issues"][0].update({"state": "open", "severity": "minor", "disposition": "deferred", "human_disposition": True}), "FPV2-D23.12-DEFECT-TRIAGE"),
        ("Fly app remains", lambda x: next(r for r in x["cleanup"]["inventories"] if r["resource_kind"] == "fly_apps").__setitem__("remaining_count", 1), "FPV2-D23.12-CLEANUP"),
        ("DNS only user-attested", lambda x: next(r for r in x["cleanup"]["inventories"] if r["resource_kind"] == "cloudflare_dns").__setitem__("verification_class", "user_attested"), "FPV2-D23.12-CLEANUP"),
        ("local credential remains", lambda x: next(r for r in x["cleanup"]["inventories"] if r["resource_kind"] == "local_credentials").__setitem__("remaining_count", 1), "FPV2-D23.12-CLEANUP"),
        ("exact VM still present", lambda x: x["cleanup"].__setitem__("qualification_vm_ref_absent", "other_vm"), "FPV2-D23.12-CLEANUP"),
        ("product rows remain", lambda x: x["cleanup"].__setitem__("product_prefix_remaining_count", 1), "FPV2-D23.12-CLEANUP"),
        ("Auth0 machine claim", lambda x: x["auth0_cleanup"].__setitem__("machine_verified", True), "FPV2-D23.12-AUTH0"),
        ("wrong Auth0 user count", lambda x: x["auth0_cleanup"].__setitem__("synthetic_user_count", 7), "FPV2-D23.12-AUTH0"),
        ("dedicated Auth0 overclaim", lambda x: x["auth0_cleanup"].__setitem__("tenant_classification", "dedicated_tenant"), "FPV2-D23.12-AUTH0"),
        ("shared-tenant limitation omitted", lambda x: x["auth0_cleanup"].__setitem__("reduced_isolation_limitation_recorded", False), "FPV2-D23.12-AUTH0"),
        ("HA claim", lambda x: x["final_claim"].__setitem__("ha_claimed", True), "FPV2-D23.12-REPORT-CLAIM"),
        ("unqualified ready verdict", lambda x: x["final_claim"].__setitem__("verdict", "ready"), "FPV2-D23.12-REPORT-CLAIM"),
        ("release claim", lambda x: x["final_claim"].__setitem__("release_claimed", True), "FPV2-D23.12-REPORT-CLAIM"),
        ("no residual risk inventory", lambda x: x.__setitem__("residual_risks", []), "FPV2-D23.12-REPORT-CLAIM"),
        ("invented exact cost", lambda x: x["cost_summary"].__setitem__("exact_cost_measured", True), "FPV2-D23.12-REPORT-CLAIM"),
        ("cost ceiling drift", lambda x: x["cost_summary"].__setitem__("approved_ceiling_usd", 11), "FPV2-D23.12-REPORT-CLAIM"),
        ("prohibited credential field", lambda x: x.__setitem__("credential", "redacted"), "FPV2-D23.12-REDACTION"),
        ("secret-shaped value", lambda x: x["cost_summary"].__setitem__("note", "-----BEGIN PRIVATE KEY-----"), "FPV2-D23.12-REDACTION"),
        ("literal IP value", lambda x: x["dns_repair"].__setitem__("note", "192.0.2.1"), "FPV2-D23.12-REDACTION"),
        ("unexpected run field", lambda x: x["run"].__setitem__("extra", True), "FPV2-D23.12-RUN-GATE"),
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
        print(f"final qualification acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"final qualification acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
    return 0


def load(path: Path) -> dict[str, Any]:
    try:
        return obj(json.loads(path.read_text()), "evidence root")
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
        for scenario_id, (description, _) in SCENARIOS.items():
            print(f"{scenario_id}\t{description}")
        return 0
    if args.self_test:
        return self_test()
    path = Path(args.evidence or os.environ.get("FLOWPLANE_FINAL_QUALIFICATION_EVIDENCE", DEFAULT_EVIDENCE))
    try:
        evidence = load(path)
    except ContractFailure as error:
        print(f"final qualification acceptance: FAIL: {error}", file=sys.stderr)
        return 1
    selected = [args.scenario] if args.scenario else list(SCENARIOS)
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
        print(f"final qualification acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"final qualification acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
