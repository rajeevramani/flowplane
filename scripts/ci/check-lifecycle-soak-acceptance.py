#!/usr/bin/env python3
"""Independent, fail-closed fpv2-d23.11 lifecycle/soak acceptance gate.

Consumes only a sanitized ``flowplane.qualification.lifecycle-soak/v1`` JSON
projection. The live producer owns provider orchestration, backups, migration,
and traffic generation. This checker validates the acceptance contract without
reading production implementation, raw logs, credentials, private evidence, or
provider state. ``--self-test`` exercises a valid synthetic projection and
adversarial fail-closed mutations.
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

SCHEMA = "flowplane.qualification.lifecycle-soak/v1"
DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.11/lifecycle-soak.json")
TEAMS = ("alpha/payments", "alpha/shared", "beta/payments", "beta/shared")
BACKUP_INPUTS = {"database", "kek_keyring", "api_tls", "xds_tls", "issuer_tls", "ca"}
CLEANUP_KINDS = {
    "fpq11_product_resources", "logical_databases", "isolated_fly_cp_apps",
    "isolated_fly_db_apps", "isolated_fly_volumes", "vm_disks", "proxies",
    "local_credentials", "local_backups",
}
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REF = re.compile(r"^[a-z][a-z0-9_-]{2,63}$")
PROHIBITED_KEYS = {
    "token", "access_token", "refresh_token", "authorization", "password",
    "secret", "secret_value", "private_key", "private_key_pem",
    "private_key_hash", "private_key_digest", "kek", "kek_value",
    "ssh_key", "ssh_cert", "certificate_pem", "ca_certificate_pem",
    "credential", "raw_log", "raw_output", "raw_request", "raw_response",
    "raw_body", "private_evidence_path", "backup_path", "provider_payload",
}
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~+/-]{8,}"),
    re.compile(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b"),
    re.compile(r"(?i)(?:sk|tskey)-[A-Za-z0-9_-]{8,}"),
    re.compile(r"(?i)(?:password|client_secret|api_key|ssh_key|kek)\s*[:=]\s*\S+"),
    re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\\\Users\\\\)"),
    re.compile(r"(?i)postgres(?:ql)?://[^\s]+"),
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


def check_run_and_artifacts(e: dict[str, Any]) -> None:
    exact_keys(e, {"schema", "run", "artifacts", "primary_backup", "primary_restore",
                   "sds_recovery", "upgrade", "fixture", "rehearsals", "final_soak",
                   "limitations", "credential_output_incident", "cleanup", "redaction"},
               "evidence root")
    if e.get("schema") != SCHEMA:
        fail(f"schema: expected {SCHEMA!r}")
    run = obj(e.get("run"), "run")
    exact_keys(run, {"live_qualification", "rerunnable", "sanitized_projection",
                     "independent_author_read_implementation", "direct_primary_database_access",
                     "started_at_utc", "finished_at_utc"}, "run")
    for key in ("live_qualification", "rerunnable", "sanitized_projection"):
        boolean(run.get(key), f"run.{key}", True)
    for key in ("independent_author_read_implementation", "direct_primary_database_access"):
        boolean(run.get(key), f"run.{key}", False)
    if timestamp(run.get("finished_at_utc"), "run.finished_at_utc") <= timestamp(run.get("started_at_utc"), "run.started_at_utc"):
        fail("run: finish must post-date start")
    artifacts = obj(e.get("artifacts"), "artifacts")
    exact_keys(artifacts, {"prior_release", "candidate"}, "artifacts")
    prior = obj(artifacts.get("prior_release"), "artifacts.prior_release")
    exact_keys(prior, {"release", "platform", "architecture", "published_archive",
                       "immutable_source", "expected_archive_digest", "observed_archive_digest",
                       "digest_verified_before_use"}, "artifacts.prior_release")
    if prior.get("release") != "3.1.2" or prior.get("platform") != "linux" or prior.get("architecture") != "amd64":
        fail("artifacts.prior_release: immutable published v3.1.2 Linux amd64 required")
    for key in ("published_archive", "immutable_source", "digest_verified_before_use"):
        boolean(prior.get(key), f"artifacts.prior_release.{key}", True)
    expected = digest(prior.get("expected_archive_digest"), "artifacts.prior_release.expected_archive_digest")
    if digest(prior.get("observed_archive_digest"), "artifacts.prior_release.observed_archive_digest") != expected:
        fail("artifacts.prior_release: archive digest mismatch")
    candidate = obj(artifacts.get("candidate"), "artifacts.candidate")
    exact_keys(candidate, {"release", "fly_image_digest", "deployed_image_digest", "digest_pinned"}, "artifacts.candidate")
    if candidate.get("release") != "3.1.3":
        fail("artifacts.candidate.release: 3.1.3 required")
    pinned = digest(candidate.get("fly_image_digest"), "artifacts.candidate.fly_image_digest")
    if digest(candidate.get("deployed_image_digest"), "artifacts.candidate.deployed_image_digest") != pinned:
        fail("artifacts.candidate: deployed Fly image must equal pinned digest")
    boolean(candidate.get("digest_pinned"), "artifacts.candidate.digest_pinned", True)


def check_primary_backup(e: dict[str, Any]) -> None:
    backup = obj(e.get("primary_backup"), "primary_backup")
    exact_keys(backup, {"source", "private", "byte_count", "catalog_object_count", "value_free_manifest",
                        "active_kek_count", "retired_kek_count", "inputs"}, "primary_backup")
    if backup.get("source") != "primary_postgresql":
        fail("primary_backup.source: primary PostgreSQL required")
    boolean(backup.get("private"), "primary_backup.private", True)
    if integer(backup.get("byte_count"), "primary_backup.byte_count") <= 0:
        fail("primary_backup.byte_count: positive count required")
    if integer(backup.get("catalog_object_count"), "primary_backup.catalog_object_count") <= 0:
        fail("primary_backup.catalog_object_count: positive count required")
    boolean(backup.get("value_free_manifest"), "primary_backup.value_free_manifest", True)
    if integer(backup.get("active_kek_count"), "primary_backup.active_kek_count") <= 0:
        fail("primary_backup.active_kek_count: at least one active KEK required")
    integer(backup.get("retired_kek_count"), "primary_backup.retired_kek_count")
    inputs = indexed(backup.get("inputs"), "primary_backup.inputs")
    if set(inputs) != BACKUP_INPUTS:
        fail(f"primary_backup.inputs: exact value-free input inventory required; got {sorted(inputs)!r}")
    for input_id, row in inputs.items():
        name = f"primary_backup.inputs[{input_id}]"
        exact_keys(row, {"id", "present", "value_recorded", "private_key_hash_recorded",
                         "certificate_public_key_matches_private_key"}, name)
        boolean(row.get("present"), f"{name}.present", True)
        boolean(row.get("value_recorded"), f"{name}.value_recorded", False)
        boolean(row.get("private_key_hash_recorded"), f"{name}.private_key_hash_recorded", False)
        match = row.get("certificate_public_key_matches_private_key")
        if input_id in {"api_tls", "xds_tls", "issuer_tls"}:
            boolean(match, f"{name}.certificate_public_key_matches_private_key", True)
        elif match is not None:
            fail(f"{name}.certificate_public_key_matches_private_key: null required when not TLS keypair")


def check_primary_restore(e: dict[str, Any]) -> None:
    restore = obj(e.get("primary_restore"), "primary_restore")
    exact_keys(restore, {"fresh_isolated_fly_database", "separate_from_primary", "backup_restored",
                         "candidate_inputs_applied", "candidate_started", "candidate_ready",
                         "primary_unchanged"}, "primary_restore")
    for key in restore:
        boolean(restore.get(key), f"primary_restore.{key}", True)


def check_sds_recovery(e: dict[str, Any]) -> None:
    sds = obj(e.get("sds_recovery"), "sds_recovery")
    exact_keys(sds, {"isolated_synthetic_fixture", "backup_restored", "snapshot_materialized",
                     "source_key_coverage", "cases"}, "sds_recovery")
    for key in ("isolated_synthetic_fixture", "backup_restored", "snapshot_materialized"):
        boolean(sds.get(key), f"sds_recovery.{key}", True)
    coverage = obj(sds.get("source_key_coverage"), "sds_recovery.source_key_coverage")
    exact_keys(coverage, {"active_source_key_existed", "active_source_key_recovery_proven",
                          "retired_source_key_existed", "retired_source_key_recovery_proven",
                          "limitation_id"}, "sds_recovery.source_key_coverage")
    boolean(coverage.get("active_source_key_existed"), "sds_recovery.source_key_coverage.active_source_key_existed", True)
    boolean(coverage.get("active_source_key_recovery_proven"), "sds_recovery.source_key_coverage.active_source_key_recovery_proven", True)
    retired_existed = coverage.get("retired_source_key_existed")
    if not isinstance(retired_existed, bool):
        fail("sds_recovery.source_key_coverage.retired_source_key_existed: boolean required")
    if retired_existed:
        boolean(coverage.get("retired_source_key_recovery_proven"), "sds_recovery.source_key_coverage.retired_source_key_recovery_proven", True)
        if coverage.get("limitation_id") is not None:
            fail("sds_recovery.source_key_coverage.limitation_id: null when retired key recovery proven")
    else:
        boolean(coverage.get("retired_source_key_recovery_proven"), "sds_recovery.source_key_coverage.retired_source_key_recovery_proven", False)
        if coverage.get("limitation_id") != "retired_source_key_unavailable":
            fail("sds_recovery.source_key_coverage: honest retired-source limitation required; do not fabricate")
    cases = indexed(sds.get("cases"), "sds_recovery.cases")
    if set(cases) != {"mismatched_kek", "missing_kek", "correct_bundle"}:
        fail("sds_recovery.cases: exact mismatch/missing/recovery sequence required")
    for case_id, expected_error in (("mismatched_kek", "undecryptable_secret"), ("missing_kek", "missing_kek")):
        row = cases[case_id]
        name = f"sds_recovery.cases[{case_id}]"
        exact_keys(row, {"id", "control_plane_available", "skipped_secret_count", "other_secret_failure_count",
                         "error_class", "error_redacted", "secret_value_recorded", "recovered"}, name)
        boolean(row.get("control_plane_available"), f"{name}.control_plane_available", True)
        integer(row.get("skipped_secret_count"), f"{name}.skipped_secret_count", 1)
        integer(row.get("other_secret_failure_count"), f"{name}.other_secret_failure_count", 0)
        if row.get("error_class") != expected_error:
            fail(f"{name}.error_class: expected {expected_error!r}")
        boolean(row.get("error_redacted"), f"{name}.error_redacted", True)
        boolean(row.get("secret_value_recorded"), f"{name}.secret_value_recorded", False)
        boolean(row.get("recovered"), f"{name}.recovered", False)
    good = cases["correct_bundle"]
    exact_keys(good, {"id", "control_plane_available", "skipped_secret_count", "other_secret_failure_count",
                      "error_class", "error_redacted", "secret_value_recorded", "recovered"},
               "sds_recovery.cases[correct_bundle]")
    boolean(good.get("control_plane_available"), "sds_recovery.cases[correct_bundle].control_plane_available", True)
    integer(good.get("skipped_secret_count"), "sds_recovery.cases[correct_bundle].skipped_secret_count", 0)
    integer(good.get("other_secret_failure_count"), "sds_recovery.cases[correct_bundle].other_secret_failure_count", 0)
    if good.get("error_class") is not None:
        fail("sds_recovery.cases[correct_bundle].error_class: null required")
    boolean(good.get("error_redacted"), "sds_recovery.cases[correct_bundle].error_redacted", True)
    boolean(good.get("secret_value_recorded"), "sds_recovery.cases[correct_bundle].secret_value_recorded", False)
    boolean(good.get("recovered"), "sds_recovery.cases[correct_bundle].recovered", True)


def check_upgrade(e: dict[str, Any]) -> None:
    upgrade = obj(e.get("upgrade"), "upgrade")
    exact_keys(upgrade, {"isolated_pre_migration_database", "published_old_binary_initialized",
                         "published_old_binary_started", "candidate_read_only_preflight",
                         "old_binary_pre_migration_rollback", "pre_migration_backup_frozen",
                         "candidate_migration", "candidate_started", "candidate_ready",
                         "old_binary_post_migration_exit_nonzero", "exact_exit_status_recorded", "old_binary_post_migration_rejected",
                         "rollback_eligibility", "separate_backup_restore"}, "upgrade")
    for key in ("isolated_pre_migration_database", "published_old_binary_initialized",
                "published_old_binary_started", "candidate_read_only_preflight",
                "old_binary_pre_migration_rollback", "pre_migration_backup_frozen",
                "candidate_migration", "candidate_started", "candidate_ready",
                "old_binary_post_migration_exit_nonzero", "old_binary_post_migration_rejected"):
        boolean(upgrade.get(key), f"upgrade.{key}", True)
    boolean(upgrade.get("exact_exit_status_recorded"), "upgrade.exact_exit_status_recorded", False)
    rollback = obj(upgrade.get("rollback_eligibility"), "upgrade.rollback_eligibility")
    exact_keys(rollback, {"documented_query_used", "forbidden_lifecycle_write_count", "eligible"}, "upgrade.rollback_eligibility")
    boolean(rollback.get("documented_query_used"), "upgrade.rollback_eligibility.documented_query_used", True)
    integer(rollback.get("forbidden_lifecycle_write_count"), "upgrade.rollback_eligibility.forbidden_lifecycle_write_count", 0)
    boolean(rollback.get("eligible"), "upgrade.rollback_eligibility.eligible", True)
    separate = obj(upgrade.get("separate_backup_restore"), "upgrade.separate_backup_restore")
    exact_keys(separate, {"separate_database", "pre_migration_backup_restored", "published_v3_1_2_started", "ready"}, "upgrade.separate_backup_restore")
    for key in separate:
        boolean(separate.get(key), f"upgrade.separate_backup_restore.{key}", True)


def check_fixture(e: dict[str, Any]) -> None:
    fixture = obj(e.get("fixture"), "fixture")
    exact_keys(fixture, {"organizations", "teams", "vms", "streams", "local_lima_xds",
                         "platform_filtering_classification"}, "fixture")
    orgs = indexed(fixture.get("organizations"), "fixture.organizations")
    if set(orgs) != {"platform", "alpha", "beta"}:
        fail("fixture.organizations: governance platform plus Alpha/Beta required")
    for org_id, row in orgs.items():
        exact_keys(row, {"id", "governance_only", "tenant_selectable"}, f"fixture.organizations[{org_id}]")
        boolean(row.get("governance_only"), f"fixture.organizations[{org_id}].governance_only", org_id == "platform")
        boolean(row.get("tenant_selectable"), f"fixture.organizations[{org_id}].tenant_selectable", org_id != "platform")
    teams = indexed(fixture.get("teams"), "fixture.teams", "team_key")
    if set(teams) != set(TEAMS):
        fail("fixture.teams: exact Alpha/Beta x Payments/Shared required")
    for team_key, row in teams.items():
        exact_keys(row, {"team_key", "tenant_selectable"}, f"fixture.teams[{team_key}]")
        boolean(row.get("tenant_selectable"), f"fixture.teams[{team_key}].tenant_selectable", True)
    vms = indexed(fixture.get("vms"), "fixture.vms")
    if set(vms) != set(TEAMS):
        fail("fixture.vms: exactly four team VMs required")
    vm_refs: set[str] = set()
    for vm_id, row in vms.items():
        name = f"fixture.vms[{vm_id}]"
        exact_keys(row, {"id", "fresh", "linux", "architecture", "host_mount_count", "vm_ref"}, name)
        for key in ("fresh", "linux"):
            boolean(row.get(key), f"{name}.{key}", True)
        if row.get("architecture") != "x86_64":
            fail(f"{name}.architecture: x86_64 required")
        integer(row.get("host_mount_count"), f"{name}.host_mount_count", 0)
        vm_refs.add(safe_ref(row.get("vm_ref"), f"{name}.vm_ref"))
    if len(vm_refs) != 4:
        fail("fixture.vms: four distinct VM references required")
    streams = indexed(fixture.get("streams"), "fixture.streams")
    if set(streams) != set(TEAMS):
        fail("fixture.streams: one stream per team required")
    markers: set[str] = set()
    for stream_id, row in streams.items():
        name = f"fixture.streams[{stream_id}]"
        exact_keys(row, {"id", "certificate_bound", "team_binding_matches", "healthy", "agent_healthy", "traffic_marker"}, name)
        for key in ("certificate_bound", "team_binding_matches", "healthy", "agent_healthy"):
            boolean(row.get(key), f"{name}.{key}", True)
        markers.add(safe_ref(row.get("traffic_marker"), f"{name}.traffic_marker"))
    if len(markers) != 4:
        fail("fixture.streams: deterministic distinct traffic markers required")
    local = obj(fixture.get("local_lima_xds"), "fixture.local_lima_xds")
    exact_keys(local, {"host_forwarding_used", "certificate_real_san_used", "san_matches_endpoint"}, "fixture.local_lima_xds")
    for key in local:
        boolean(local.get(key), f"fixture.local_lima_xds.{key}", True)


def check_rehearsals(e: dict[str, Any]) -> None:
    rehearsals = obj(e.get("rehearsals"), "rehearsals")
    exact_keys(rehearsals, {"initial_canary", "canary2", "capacity_tuning", "canary3", "writer_burst"}, "rehearsals")
    initial = obj(rehearsals.get("initial_canary"), "rehearsals.initial_canary")
    exact_keys(initial, {"traffic_attempted", "traffic_succeeded", "traffic_failed", "failure_instrumented", "acceptance_claimed"}, "rehearsals.initial_canary")
    integer(initial.get("traffic_attempted"), "rehearsals.initial_canary.traffic_attempted", 1200)
    integer(initial.get("traffic_succeeded"), "rehearsals.initial_canary.traffic_succeeded", 1198)
    integer(initial.get("traffic_failed"), "rehearsals.initial_canary.traffic_failed", 2)
    boolean(initial.get("failure_instrumented"), "rehearsals.initial_canary.failure_instrumented", False)
    boolean(initial.get("acceptance_claimed"), "rehearsals.initial_canary.acceptance_claimed", False)
    canary2 = obj(rehearsals.get("canary2"), "rehearsals.canary2")
    exact_keys(canary2, {"traffic_attempted", "traffic_succeeded", "traffic_failed", "writers_blocked",
                         "observed_pool_max", "classification"}, "rehearsals.canary2")
    integer(canary2.get("traffic_attempted"), "rehearsals.canary2.traffic_attempted", 1200)
    integer(canary2.get("traffic_succeeded"), "rehearsals.canary2.traffic_succeeded", 1200)
    integer(canary2.get("traffic_failed"), "rehearsals.canary2.traffic_failed", 0)
    boolean(canary2.get("writers_blocked"), "rehearsals.canary2.writers_blocked", True)
    integer(canary2.get("observed_pool_max"), "rehearsals.canary2.observed_pool_max", 10)
    if canary2.get("classification") != "harness_capacity_tuning":
        fail("rehearsals.canary2.classification: harness capacity tuning required")
    tuning = obj(rehearsals.get("capacity_tuning"), "rehearsals.capacity_tuning")
    exact_keys(tuning, {"variable", "value", "applied"}, "rehearsals.capacity_tuning")
    if tuning.get("variable") != "FLOWPLANE_DB_MAX_CONNECTIONS":
        fail("rehearsals.capacity_tuning.variable: exact supported variable required")
    integer(tuning.get("value"), "rehearsals.capacity_tuning.value", 24)
    boolean(tuning.get("applied"), "rehearsals.capacity_tuning.applied", True)
    canary3 = obj(rehearsals.get("canary3"), "rehearsals.canary3")
    exact_keys(canary3, {"traffic_attempted", "traffic_succeeded", "traffic_failed", "writer_token_expired", "acceptance_claimed"}, "rehearsals.canary3")
    integer(canary3.get("traffic_attempted"), "rehearsals.canary3.traffic_attempted", 1200)
    integer(canary3.get("traffic_succeeded"), "rehearsals.canary3.traffic_succeeded", 1200)
    integer(canary3.get("traffic_failed"), "rehearsals.canary3.traffic_failed", 0)
    boolean(canary3.get("writer_token_expired"), "rehearsals.canary3.writer_token_expired", True)
    boolean(canary3.get("acceptance_claimed"), "rehearsals.canary3.acceptance_claimed", False)
    burst = obj(rehearsals.get("writer_burst"), "rehearsals.writer_burst")
    exact_keys(burst, {"fresh_oidc_token", "token_ttl_seconds", "attempted", "succeeded", "failed"}, "rehearsals.writer_burst")
    boolean(burst.get("fresh_oidc_token"), "rehearsals.writer_burst.fresh_oidc_token", True)
    if integer(burst.get("token_ttl_seconds"), "rehearsals.writer_burst.token_ttl_seconds") <= 2100:
        fail("rehearsals.writer_burst.token_ttl_seconds: must exceed 2100")
    integer(burst.get("attempted"), "rehearsals.writer_burst.attempted", 12)
    integer(burst.get("succeeded"), "rehearsals.writer_burst.succeeded", 12)
    integer(burst.get("failed"), "rehearsals.writer_burst.failed", 0)


def check_final_soak(e: dict[str, Any]) -> None:
    soak = obj(e.get("final_soak"), "final_soak")
    exact_keys(soak, {"duration_seconds", "traffic", "writers", "healthy_stream_count", "healthy_agent_count",
                      "restart_count", "oom_count", "isolation_breach_count", "db_pool", "rss"}, "final_soak")
    integer(soak.get("duration_seconds"), "final_soak.duration_seconds", 1800)
    for section, attempted, succeeded in (("traffic", 7200, 7200), ("writers", 120, 120)):
        row = obj(soak.get(section), f"final_soak.{section}")
        exact_keys(row, {"attempted", "succeeded", "failed"}, f"final_soak.{section}")
        integer(row.get("attempted"), f"final_soak.{section}.attempted", attempted)
        integer(row.get("succeeded"), f"final_soak.{section}.succeeded", succeeded)
        integer(row.get("failed"), f"final_soak.{section}.failed", 0)
    integer(soak.get("healthy_stream_count"), "final_soak.healthy_stream_count", 4)
    integer(soak.get("healthy_agent_count"), "final_soak.healthy_agent_count", 4)
    for key in ("restart_count", "oom_count", "isolation_breach_count"):
        integer(soak.get(key), f"final_soak.{key}", 0)
    pool = obj(soak.get("db_pool"), "final_soak.db_pool")
    exact_keys(pool, {"configured_max", "observed_peak", "terminal_exhaustion_count"}, "final_soak.db_pool")
    integer(pool.get("configured_max"), "final_soak.db_pool.configured_max", 24)
    integer(pool.get("observed_peak"), "final_soak.db_pool.observed_peak", 15)
    integer(pool.get("terminal_exhaustion_count"), "final_soak.db_pool.terminal_exhaustion_count", 0)
    rss = obj(soak.get("rss"), "final_soak.rss")
    exact_keys(rss, {"control_plane_threshold_mib", "vm_threshold_mib", "post_warmup_growth_threshold_percent", "control_plane_samples_mib", "vm_samples"}, "final_soak.rss")
    cp_threshold = integer(rss.get("control_plane_threshold_mib"), "final_soak.rss.control_plane_threshold_mib")
    vm_threshold = integer(rss.get("vm_threshold_mib"), "final_soak.rss.vm_threshold_mib")
    cp_samples = seq(rss.get("control_plane_samples_mib"), "final_soak.rss.control_plane_samples_mib")
    if len(cp_samples) < 30:
        fail("final_soak.rss.control_plane_samples_mib: at least 30 minute samples required")
    for index, sample in enumerate(cp_samples):
        if integer(sample, f"final_soak.rss.control_plane_samples_mib[{index}]") > cp_threshold:
            fail("final_soak.rss: control-plane RSS exceeded explicit threshold")
    growth_threshold = integer(rss.get("post_warmup_growth_threshold_percent"), "final_soak.rss.post_warmup_growth_threshold_percent", 25)
    cp_baseline = integer(cp_samples[5], "final_soak.rss.control_plane_samples_mib[5]")
    if cp_baseline == 0 or (max(cp_samples[5:]) - cp_baseline) * 100 > cp_baseline * growth_threshold:
        fail("final_soak.rss: control-plane post-warmup growth exceeded threshold")
    vm_samples = indexed(rss.get("vm_samples"), "final_soak.rss.vm_samples", "team_key")
    if set(vm_samples) != set(TEAMS):
        fail("final_soak.rss.vm_samples: exact four-team sample sets required")
    for team, row in vm_samples.items():
        exact_keys(row, {"team_key", "samples_mib"}, f"final_soak.rss.vm_samples[{team}]")
        samples = seq(row.get("samples_mib"), f"final_soak.rss.vm_samples[{team}].samples_mib")
        if len(samples) < 30:
            fail(f"final_soak.rss.vm_samples[{team}]: at least 30 minute samples required")
        for index, sample in enumerate(samples):
            if integer(sample, f"final_soak.rss.vm_samples[{team}].samples_mib[{index}]") > vm_threshold:
                fail(f"final_soak.rss.vm_samples[{team}]: RSS exceeded explicit threshold")
        baseline = integer(samples[5], f"final_soak.rss.vm_samples[{team}].samples_mib[5]")
        if baseline == 0 or (max(samples[5:]) - baseline) * 100 > baseline * growth_threshold:
            fail(f"final_soak.rss.vm_samples[{team}]: post-warmup growth exceeded threshold")


def check_limitations_and_classification(e: dict[str, Any]) -> None:
    rows = indexed(e.get("limitations"), "limitations")
    if set(rows) != {"retired_source_key_unavailable", "component_skew"}:
        fail("limitations: exact retired-key and component-skew boundary inventory required")
    retired = rows["retired_source_key_unavailable"]
    exact_keys(retired, {"id", "applicable", "reason", "claim_not_made"}, "limitations[retired_source_key_unavailable]")
    expected_applicable = not field(e, "sds_recovery.source_key_coverage.retired_source_key_existed")
    boolean(retired.get("applicable"), "limitations.retired_source_key_unavailable.applicable", expected_applicable)
    expected_reason = "no_retired_source_key_existed" if expected_applicable else "not_applicable"
    if retired.get("reason") != expected_reason:
        fail(f"limitations.retired_source_key_unavailable.reason: expected {expected_reason!r}")
    boolean(retired.get("claim_not_made"), "limitations.retired_source_key_unavailable.claim_not_made", expected_applicable)
    skew = rows["component_skew"]
    exact_keys(skew, {"id", "tested_beyond_control_plane_binary_upgrade", "rls_skew_claimed", "agent_skew_claimed", "explicit_limitation"}, "limitations[component_skew]")
    for key in ("tested_beyond_control_plane_binary_upgrade", "rls_skew_claimed", "agent_skew_claimed"):
        boolean(skew.get(key), f"limitations.component_skew.{key}", False)
    boolean(skew.get("explicit_limitation"), "limitations.component_skew.explicit_limitation", True)


def check_platform_filtering(e: dict[str, Any]) -> None:
    fixture = obj(e.get("fixture"), "fixture")
    # Preserve exact fixture fields by requiring the classification member in the fixture schema.
    classification = obj(fixture.get("platform_filtering_classification"), "fixture.platform_filtering_classification")
    exact_keys(classification, {"verdict", "reviewer", "github_issue", "github_issue_closed",
                                "bead", "bead_closed", "product_bug"}, "fixture.platform_filtering_classification")
    if classification.get("verdict") != "HARNESS DEFECT" or classification.get("reviewer") != "claude-fable-5":
        fail("fixture.platform_filtering_classification: independent claude-fable-5 HARNESS DEFECT verdict required")
    if classification.get("github_issue") != "#255" or classification.get("bead") != ".14":
        fail("fixture.platform_filtering_classification: GitHub #255 and Bead .14 required")
    boolean(classification.get("github_issue_closed"), "fixture.platform_filtering_classification.github_issue_closed", True)
    boolean(classification.get("bead_closed"), "fixture.platform_filtering_classification.bead_closed", True)
    boolean(classification.get("product_bug"), "fixture.platform_filtering_classification.product_bug", False)


def check_fixture_scenario(e: dict[str, Any]) -> None:
    check_fixture(e)
    check_platform_filtering(e)


def check_incident(e: dict[str, Any]) -> None:
    incident = obj(e.get("credential_output_incident"), "credential_output_incident")
    exact_keys(incident, {"occurred", "classification", "exposure_surface", "material_type",
                          "flowplane_secret_exposed", "remediation", "post_rotation"}, "credential_output_incident")
    boolean(incident.get("occurred"), "credential_output_incident.occurred", True)
    if incident.get("classification") != "harness_incident" or incident.get("exposure_surface") != "private_tool_output":
        fail("credential_output_incident: private tool-output harness incident classification required")
    if incident.get("material_type") != "fly_postgres_ssh_key":
        fail("credential_output_incident.material_type: Fly Postgres SSH key required")
    boolean(incident.get("flowplane_secret_exposed"), "credential_output_incident.flowplane_secret_exposed", False)
    remediation = obj(incident.get("remediation"), "credential_output_incident.remediation")
    exact_keys(remediation, {"supported_renew_certs_used", "ssh_key_digest_rotated", "ssh_cert_digest_rotated"}, "credential_output_incident.remediation")
    for key in remediation:
        boolean(remediation.get(key), f"credential_output_incident.remediation.{key}", True)
    post = obj(incident.get("post_rotation"), "credential_output_incident.post_rotation")
    exact_keys(post, {"same_primary_database_machine", "database_checks_attempted", "database_checks_passed", "control_plane_ready"}, "credential_output_incident.post_rotation")
    boolean(post.get("same_primary_database_machine"), "credential_output_incident.post_rotation.same_primary_database_machine", True)
    integer(post.get("database_checks_attempted"), "credential_output_incident.post_rotation.database_checks_attempted", 3)
    integer(post.get("database_checks_passed"), "credential_output_incident.post_rotation.database_checks_passed", 3)
    boolean(post.get("control_plane_ready"), "credential_output_incident.post_rotation.control_plane_ready", True)


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(e.get("cleanup"), "cleanup")
    exact_keys(cleanup, {"evidence_frozen_before_cleanup", "inventories", "primary_services_ready",
                         "primary_database_memory_mib", "retained_history"}, "cleanup")
    boolean(cleanup.get("evidence_frozen_before_cleanup"), "cleanup.evidence_frozen_before_cleanup", True)
    rows = indexed(cleanup.get("inventories"), "cleanup.inventories", "resource_kind")
    if set(rows) != CLEANUP_KINDS:
        fail(f"cleanup.inventories: exact cleanup inventory required; got {sorted(rows)!r}")
    for kind, row in rows.items():
        name = f"cleanup.inventories[{kind}]"
        exact_keys(row, {"resource_kind", "created_count", "remaining_count", "authoritative_inventory_checked"}, name)
        created = integer(row.get("created_count"), f"{name}.created_count")
        if kind == "vm_disks" and created != 5:
            fail(f"{name}.created_count: exactly five VM disks required")
        integer(row.get("remaining_count"), f"{name}.remaining_count", 0)
        boolean(row.get("authoritative_inventory_checked"), f"{name}.authoritative_inventory_checked", True)
    services = obj(cleanup.get("primary_services_ready"), "cleanup.primary_services_ready")
    exact_keys(services, {"control_plane", "database", "rls"}, "cleanup.primary_services_ready")
    for key in services:
        boolean(services.get(key), f"cleanup.primary_services_ready.{key}", True)
    integer(cleanup.get("primary_database_memory_mib"), "cleanup.primary_database_memory_mib", 256)
    retained = obj(cleanup.get("retained_history"), "cleanup.retained_history")
    exact_keys(retained, {"explicitly_retained", "inert", "active", "addressable", "can_serve_traffic"}, "cleanup.retained_history")
    boolean(retained.get("explicitly_retained"), "cleanup.retained_history.explicitly_retained", True)
    boolean(retained.get("inert"), "cleanup.retained_history.inert", True)
    for key in ("active", "addressable", "can_serve_traffic"):
        boolean(retained.get(key), f"cleanup.retained_history.{key}", False)


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
    exact_keys(redaction, {"sanitized_projection", "secret_values_recorded", "private_key_hashes_recorded",
                           "raw_logs_recorded", "raw_identifiers_recorded", "private_paths_recorded",
                           "scan_after_final_edit", "undisposed_match_count", "pattern_classes"}, "redaction")
    boolean(redaction.get("sanitized_projection"), "redaction.sanitized_projection", True)
    for key in ("secret_values_recorded", "private_key_hashes_recorded", "raw_logs_recorded",
                "raw_identifiers_recorded", "private_paths_recorded"):
        boolean(redaction.get(key), f"redaction.{key}", False)
    boolean(redaction.get("scan_after_final_edit"), "redaction.scan_after_final_edit", True)
    integer(redaction.get("undisposed_match_count"), "redaction.undisposed_match_count", 0)
    required = {"credentials", "bearer_tokens", "private_keys", "private_key_hashes", "kek_material",
                "certificate_bodies", "database_urls", "private_paths", "raw_identifiers", "raw_logs"}
    if set(seq(redaction.get("pattern_classes"), "redaction.pattern_classes")) != required:
        fail("redaction.pattern_classes: exact strict scan classes required")
    walk(e)


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.11-RUN-ARTIFACTS": ("immutable v3.1.2 archive and pinned v3.1.3 Fly image", check_run_and_artifacts),
    "FPV2-D23.11-PRIMARY-BACKUP": ("positive private PostgreSQL backup with value-free input manifest", check_primary_backup),
    "FPV2-D23.11-PRIMARY-RESTORE": ("fresh isolated Fly restore starts ready with candidate inputs", check_primary_restore),
    "FPV2-D23.11-SDS-RECOVERY": ("synthetic SDS KEK mismatch, missing-key and recovery behavior", check_sds_recovery),
    "FPV2-D23.11-UPGRADE": ("published-old-binary migration and rollback lifecycle", check_upgrade),
    "FPV2-D23.11-FIXTURE": ("honest platform filtering and four fresh certificate-bound VMs", check_fixture_scenario),
    "FPV2-D23.11-REHEARSALS": ("canary failures honestly classified and corrected", check_rehearsals),
    "FPV2-D23.11-FINAL-SOAK": ("exact zero-error 30-minute traffic/writer soak with bounded resources", check_final_soak),
    "FPV2-D23.11-LIMITATIONS": ("honest retired-key and component-skew limitations", check_limitations_and_classification),
    "FPV2-D23.11-CREDENTIAL-INCIDENT": ("Fly SSH material output incident rotated and primary rechecked", check_incident),
    "FPV2-D23.11-CLEANUP": ("all fpq11 resources removed and primary services restored", check_cleanup),
    "FPV2-D23.11-REDACTION": ("strict value, key-hash, log, identifier and path redaction", check_redaction),
}


def fp(number: int) -> str:
    return "sha256:" + f"{number:064x}"


def fixture() -> dict[str, Any]:
    backup_inputs = []
    for input_id in sorted(BACKUP_INPUTS):
        backup_inputs.append({"id": input_id, "present": True, "value_recorded": False,
                              "private_key_hash_recorded": False,
                              "certificate_public_key_matches_private_key": True if input_id in {"api_tls", "xds_tls", "issuer_tls"} else None})
    cases = [
        {"id": "mismatched_kek", "control_plane_available": True, "skipped_secret_count": 1,
         "other_secret_failure_count": 0, "error_class": "undecryptable_secret", "error_redacted": True,
         "secret_value_recorded": False, "recovered": False},
        {"id": "missing_kek", "control_plane_available": True, "skipped_secret_count": 1,
         "other_secret_failure_count": 0, "error_class": "missing_kek", "error_redacted": True,
         "secret_value_recorded": False, "recovered": False},
        {"id": "correct_bundle", "control_plane_available": True, "skipped_secret_count": 0,
         "other_secret_failure_count": 0, "error_class": None, "error_redacted": True,
         "secret_value_recorded": False, "recovered": True},
    ]
    orgs = [{"id": org, "governance_only": org == "platform", "tenant_selectable": org != "platform"}
            for org in ("platform", "alpha", "beta")]
    teams = [{"team_key": team, "tenant_selectable": True} for team in TEAMS]
    vms = [{"id": team, "fresh": True, "linux": True, "architecture": "x86_64",
            "host_mount_count": 0, "vm_ref": f"vm_{index}"} for index, team in enumerate(TEAMS, 1)]
    streams = [{"id": team, "certificate_bound": True, "team_binding_matches": True,
                "healthy": True, "agent_healthy": True, "traffic_marker": f"marker_{index}"}
               for index, team in enumerate(TEAMS, 1)]
    vm_rss = [{"team_key": team, "samples_mib": [20] * 31} for team in TEAMS]
    inventories = [{"resource_kind": kind, "created_count": (5 if kind == "vm_disks" else 1),
                    "remaining_count": 0, "authoritative_inventory_checked": True}
                   for kind in sorted(CLEANUP_KINDS)]
    return {
        "schema": SCHEMA,
        "run": {"live_qualification": True, "rerunnable": True, "sanitized_projection": True,
                "independent_author_read_implementation": False, "direct_primary_database_access": False,
                "started_at_utc": "2026-08-23T01:00:00Z", "finished_at_utc": "2026-08-23T02:30:00Z"},
        "artifacts": {
            "prior_release": {"release": "3.1.2", "platform": "linux", "architecture": "amd64",
                              "published_archive": True, "immutable_source": True,
                              "expected_archive_digest": fp(1), "observed_archive_digest": fp(1),
                              "digest_verified_before_use": True},
            "candidate": {"release": "3.1.3", "fly_image_digest": fp(2),
                          "deployed_image_digest": fp(2), "digest_pinned": True}},
        "primary_backup": {"source": "primary_postgresql", "private": True, "byte_count": 4096,
                           "catalog_object_count": 42, "value_free_manifest": True,
                           "active_kek_count": 1, "retired_kek_count": 0, "inputs": backup_inputs},
        "primary_restore": {"fresh_isolated_fly_database": True, "separate_from_primary": True,
                            "backup_restored": True, "candidate_inputs_applied": True,
                            "candidate_started": True, "candidate_ready": True, "primary_unchanged": True},
        "sds_recovery": {"isolated_synthetic_fixture": True, "backup_restored": True,
                         "snapshot_materialized": True,
                         "source_key_coverage": {"active_source_key_existed": True,
                                                 "active_source_key_recovery_proven": True,
                                                 "retired_source_key_existed": False,
                                                 "retired_source_key_recovery_proven": False,
                                                 "limitation_id": "retired_source_key_unavailable"},
                         "cases": cases},
        "upgrade": {"isolated_pre_migration_database": True, "published_old_binary_initialized": True,
                    "published_old_binary_started": True, "candidate_read_only_preflight": True,
                    "old_binary_pre_migration_rollback": True, "pre_migration_backup_frozen": True,
                    "candidate_migration": True, "candidate_started": True, "candidate_ready": True,
                    "old_binary_post_migration_exit_nonzero": True, "exact_exit_status_recorded": False,
                    "old_binary_post_migration_rejected": True,
                    "rollback_eligibility": {"documented_query_used": True,
                                             "forbidden_lifecycle_write_count": 0, "eligible": True},
                    "separate_backup_restore": {"separate_database": True,
                                                "pre_migration_backup_restored": True,
                                                "published_v3_1_2_started": True, "ready": True}},
        "fixture": {"organizations": orgs, "teams": teams, "vms": vms, "streams": streams,
                    "local_lima_xds": {"host_forwarding_used": True, "certificate_real_san_used": True,
                                       "san_matches_endpoint": True},
                    "platform_filtering_classification": {"verdict": "HARNESS DEFECT", "reviewer": "claude-fable-5",
                                                          "github_issue": "#255", "github_issue_closed": True,
                                                          "bead": ".14", "bead_closed": True, "product_bug": False}},
        "rehearsals": {
            "initial_canary": {"traffic_attempted": 1200, "traffic_succeeded": 1198, "traffic_failed": 2,
                               "failure_instrumented": False, "acceptance_claimed": False},
            "canary2": {"traffic_attempted": 1200, "traffic_succeeded": 1200, "traffic_failed": 0,
                        "writers_blocked": True, "observed_pool_max": 10,
                        "classification": "harness_capacity_tuning"},
            "capacity_tuning": {"variable": "FLOWPLANE_DB_MAX_CONNECTIONS", "value": 24, "applied": True},
            "canary3": {"traffic_attempted": 1200, "traffic_succeeded": 1200, "traffic_failed": 0,
                        "writer_token_expired": True, "acceptance_claimed": False},
            "writer_burst": {"fresh_oidc_token": True, "token_ttl_seconds": 2400,
                             "attempted": 12, "succeeded": 12, "failed": 0}},
        "final_soak": {"duration_seconds": 1800,
                       "traffic": {"attempted": 7200, "succeeded": 7200, "failed": 0},
                       "writers": {"attempted": 120, "succeeded": 120, "failed": 0},
                       "healthy_stream_count": 4, "healthy_agent_count": 4,
                       "restart_count": 0, "oom_count": 0, "isolation_breach_count": 0,
                       "db_pool": {"configured_max": 24, "observed_peak": 15, "terminal_exhaustion_count": 0},
                       "rss": {"control_plane_threshold_mib": 512, "vm_threshold_mib": 384,
                               "post_warmup_growth_threshold_percent": 25,
                               "control_plane_samples_mib": [37] * 30 + [38], "vm_samples": vm_rss}},
        "limitations": [
            {"id": "retired_source_key_unavailable", "applicable": True,
             "reason": "no_retired_source_key_existed", "claim_not_made": True},
            {"id": "component_skew", "tested_beyond_control_plane_binary_upgrade": False,
             "rls_skew_claimed": False, "agent_skew_claimed": False, "explicit_limitation": True}],
        "credential_output_incident": {"occurred": True, "classification": "harness_incident",
                                       "exposure_surface": "private_tool_output",
                                       "material_type": "fly_postgres_ssh_key",
                                       "flowplane_secret_exposed": False,
                                       "remediation": {"supported_renew_certs_used": True,
                                                       "ssh_key_digest_rotated": True,
                                                       "ssh_cert_digest_rotated": True},
                                       "post_rotation": {"same_primary_database_machine": True,
                                                         "database_checks_attempted": 3,
                                                         "database_checks_passed": 3,
                                                         "control_plane_ready": True}},
        "cleanup": {"evidence_frozen_before_cleanup": True, "inventories": inventories,
                    "primary_services_ready": {"control_plane": True, "database": True, "rls": True},
                    "primary_database_memory_mib": 256,
                    "retained_history": {"explicitly_retained": True, "inert": True, "active": False,
                                         "addressable": False, "can_serve_traffic": False}},
        "redaction": {"sanitized_projection": True, "secret_values_recorded": False,
                      "private_key_hashes_recorded": False, "raw_logs_recorded": False,
                      "raw_identifiers_recorded": False, "private_paths_recorded": False,
                      "scan_after_final_edit": True, "undisposed_match_count": 0,
                      "pattern_classes": ["credentials", "bearer_tokens", "private_keys", "private_key_hashes",
                                          "kek_material", "certificate_bodies", "database_urls", "private_paths",
                                          "raw_identifiers", "raw_logs"]},
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
        ("prior archive digest drift", lambda x: x["artifacts"]["prior_release"].__setitem__("observed_archive_digest", fp(9)), "FPV2-D23.11-RUN-ARTIFACTS"),
        ("candidate image unpinned", lambda x: x["artifacts"]["candidate"].__setitem__("digest_pinned", False), "FPV2-D23.11-RUN-ARTIFACTS"),
        ("empty backup", lambda x: x["primary_backup"].__setitem__("byte_count", 0), "FPV2-D23.11-PRIMARY-BACKUP"),
        ("private key hash recorded", lambda x: x["primary_backup"]["inputs"][0].__setitem__("private_key_hash_recorded", True), "FPV2-D23.11-PRIMARY-BACKUP"),
        ("candidate restore not ready", lambda x: x["primary_restore"].__setitem__("candidate_ready", False), "FPV2-D23.11-PRIMARY-RESTORE"),
        ("mismatched KEK kills CP", lambda x: x["sds_recovery"]["cases"][0].__setitem__("control_plane_available", False), "FPV2-D23.11-SDS-RECOVERY"),
        ("missing KEK leaks error", lambda x: x["sds_recovery"]["cases"][1].__setitem__("error_redacted", False), "FPV2-D23.11-SDS-RECOVERY"),
        ("fabricated retired-key claim", lambda x: x["sds_recovery"]["source_key_coverage"].__setitem__("retired_source_key_recovery_proven", True), "FPV2-D23.11-SDS-RECOVERY"),
        ("old binary accepts migrated DB", lambda x: x["upgrade"].__setitem__("old_binary_post_migration_exit_nonzero", False), "FPV2-D23.11-UPGRADE"),
        ("invented exact exit status", lambda x: x["upgrade"].__setitem__("exact_exit_status_recorded", True), "FPV2-D23.11-UPGRADE"),
        ("rollback forbidden writes", lambda x: x["upgrade"]["rollback_eligibility"].__setitem__("forbidden_lifecycle_write_count", 1), "FPV2-D23.11-UPGRADE"),
        ("platform selectable", lambda x: x["fixture"]["organizations"][0].__setitem__("tenant_selectable", True), "FPV2-D23.11-FIXTURE"),
        ("product bug misclassification", lambda x: x["fixture"]["platform_filtering_classification"].__setitem__("product_bug", True), "FPV2-D23.11-FIXTURE"),
        ("host-mounted VM", lambda x: x["fixture"]["vms"][0].__setitem__("host_mount_count", 1), "FPV2-D23.11-FIXTURE"),
        ("wrong SAN", lambda x: x["fixture"]["local_lima_xds"].__setitem__("san_matches_endpoint", False), "FPV2-D23.11-FIXTURE"),
        ("canary2 called product bug", lambda x: x["rehearsals"]["canary2"].__setitem__("classification", "product_bug"), "FPV2-D23.11-REHEARSALS"),
        ("short OIDC TTL", lambda x: x["rehearsals"]["writer_burst"].__setitem__("token_ttl_seconds", 2100), "FPV2-D23.11-REHEARSALS"),
        ("soak traffic differs", lambda x: x["final_soak"]["traffic"].__setitem__("succeeded", 7199), "FPV2-D23.11-FINAL-SOAK"),
        ("soak writer error", lambda x: x["final_soak"]["writers"].__setitem__("failed", 1), "FPV2-D23.11-FINAL-SOAK"),
        ("pool peak drift", lambda x: x["final_soak"]["db_pool"].__setitem__("observed_peak", 16), "FPV2-D23.11-FINAL-SOAK"),
        ("RSS threshold breach", lambda x: x["final_soak"]["rss"]["control_plane_samples_mib"].__setitem__(0, 513), "FPV2-D23.11-FINAL-SOAK"),
        ("truncated RSS samples", lambda x: x["final_soak"]["rss"].__setitem__("control_plane_samples_mib", [37]), "FPV2-D23.11-FINAL-SOAK"),
        ("post-warmup RSS growth", lambda x: x["final_soak"]["rss"]["control_plane_samples_mib"].__setitem__(10, 60), "FPV2-D23.11-FINAL-SOAK"),
        ("invented agent skew", lambda x: x["limitations"][1].__setitem__("agent_skew_claimed", True), "FPV2-D23.11-LIMITATIONS"),
        ("Flowplane secret exposure claim", lambda x: x["credential_output_incident"].__setitem__("flowplane_secret_exposed", True), "FPV2-D23.11-CREDENTIAL-INCIDENT"),
        ("SSH cert not rotated", lambda x: x["credential_output_incident"]["remediation"].__setitem__("ssh_cert_digest_rotated", False), "FPV2-D23.11-CREDENTIAL-INCIDENT"),
        ("fifth VM disk remains", lambda x: next(r for r in x["cleanup"]["inventories"] if r["resource_kind"] == "vm_disks").__setitem__("remaining_count", 1), "FPV2-D23.11-CLEANUP"),
        ("primary DB memory not restored", lambda x: x["cleanup"].__setitem__("primary_database_memory_mib", 512), "FPV2-D23.11-CLEANUP"),
        ("prohibited private key field", lambda x: x.__setitem__("private_key", "redacted"), "FPV2-D23.11-REDACTION"),
        ("secret-shaped value", lambda x: x["cleanup"]["retained_history"].__setitem__("note", "-----BEGIN PRIVATE KEY-----"), "FPV2-D23.11-REDACTION"),
        ("unexpected evidence field", lambda x: x["run"].__setitem__("extra", True), "FPV2-D23.11-RUN-ARTIFACTS"),
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
        print(f"lifecycle soak acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"lifecycle soak acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
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
    path = Path(args.evidence or os.environ.get("FLOWPLANE_LIFECYCLE_SOAK_EVIDENCE", DEFAULT_EVIDENCE))
    try:
        evidence = load(path)
    except ContractFailure as error:
        print(f"lifecycle soak acceptance: FAIL: {error}", file=sys.stderr)
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
        print(f"lifecycle soak acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"lifecycle soak acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
