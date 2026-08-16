#!/usr/bin/env python3
"""Black-box acceptance gate for fpv2-d23.3's production-shaped topology.

The gate consumes one sanitized JSON evidence projection produced by a live qualification
run. It never reads deployment files, provider credentials, certificates, private keys, or
raw logs. Every scenario can be rerun independently with ``--scenario <id>``.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys
from typing import Any, Callable, NoReturn

DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.3/secure-topology.json")
HEX_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-57][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.I)
SECRET_SHAPES = (
    re.compile(r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----"),
    re.compile(r"-----BEGIN CERTIFICATE-----"),
    re.compile(r"(?i)authorization\s*:\s*bearer\s+\S+"),
    re.compile(r"(?i)\b(?:token|password|private_key|client_secret|api_key)\b\s*[:=]\s*[\"']?[^\s\"']+"),
)


class ContractFailure(AssertionError):
    """An externally observable acceptance invariant was not proven."""


def fail(message: str) -> NoReturn:
    raise ContractFailure(message)


def obj(value: Any, field: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{field}: required object is absent")
    return value


def seq(value: Any, field: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{field}: required array is absent")
    return value


def field(root: dict[str, Any], dotted: str) -> Any:
    value: Any = root
    for part in dotted.split("."):
        value = obj(value, dotted).get(part)
        if value is None:
            fail(f"{dotted}: required evidence is absent")
    return value


def equal(root: dict[str, Any], dotted: str, expected: Any) -> None:
    actual = field(root, dotted)
    if actual != expected:
        fail(f"{dotted}: expected {expected!r}, observed {actual!r}")


def nonempty_text(root: dict[str, Any], dotted: str) -> str:
    value = field(root, dotted)
    if not isinstance(value, str) or not value.strip():
        fail(f"{dotted}: expected non-empty text")
    return value


def named_origin(value: Any, field_name: str, *, tailnet: bool, outside_fly: bool) -> dict[str, Any]:
    origin = obj(value, field_name)
    for name in ("name", "provider", "region", "tool", "tool_version", "observed_at_utc"):
        if not isinstance(origin.get(name), str) or not origin[name].strip():
            fail(f"{field_name}.{name}: named origin attestation is required")
    if origin.get("tailnet_enrolled") is not tailnet:
        fail(f"{field_name}.tailnet_enrolled: expected {tailnet!r}")
    if outside_fly:
        if origin.get("fly_organization_related") is not False:
            fail(f"{field_name}.fly_organization_related: public origin must be unrelated to Fly target")
        if origin.get("provider_private_network_reachable") is not False:
            fail(f"{field_name}.provider_private_network_reachable: public origin is not independent")
        for route in ("vpn", "proxy", "exit_node", "tunnel", "relay"):
            if origin.get(route) is not False:
                fail(f"{field_name}.{route}: public origin must attest no private route")
    return origin


def by_id(items: Any, dotted: str) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for index, value in enumerate(seq(items, dotted)):
        item = obj(value, f"{dotted}[{index}]")
        item_id = item.get("id")
        if not isinstance(item_id, str) or not item_id:
            fail(f"{dotted}[{index}].id: required")
        assert isinstance(item_id, str)
        if item_id in result:
            fail(f"{dotted}: duplicate id {item_id!r}")
        result[item_id] = item
    return result


def check_deployment(e: dict[str, Any]) -> None:
    equal(e, "run.release", "3.1.3")
    digest = nonempty_text(e, "run.candidate_image_digest")
    if not HEX_DIGEST.fullmatch(digest):
        fail("run.candidate_image_digest: immutable sha256 digest required")
    equal(e, "deployment.flowplane.hardened", True)
    equal(e, "deployment.flowplane.dev_mode", False)
    equal(e, "deployment.flowplane.one_shot_bootstrap_consumed", True)
    equal(e, "deployment.flowplane.secret_encryption_enabled", True)
    equal(e, "deployment.flowplane.oidc_issuer_audience_configured", True)
    equal(e, "deployment.api.fly_ingress.port", 443)
    equal(e, "deployment.api.fly_ingress.mode", "raw_tcp_passthrough")
    equal(e, "deployment.api.flowplane_listener.port", 8080)
    equal(e, "deployment.api.tls_terminated_by", "flowplane")
    equal(e, "deployment.xds.port", 18000)
    equal(e, "deployment.xds.public_service_declared", False)
    equal(e, "deployment.xds.tailscale_only", True)
    equal(e, "deployment.xds.tls_terminated_by", "flowplane")
    equal(e, "deployment.postgresql.persistent", True)
    equal(e, "deployment.postgresql.publicly_addressable", False)
    equal(e, "deployment.postgresql.migrations_applied", True)
    equal(e, "deployment.postgresql.ready", True)
    equal(e, "deployment.rls.separate_process", True)
    equal(e, "deployment.rls.admin.private", True)
    equal(e, "deployment.rls.admin.https", True)
    equal(e, "deployment.rls.admin.bearer_auth", True)
    equal(e, "deployment.rls.grpc.private", True)
    equal(e, "deployment.rls.grpc.mtls", True)
    equal(e, "deployment.secret_inputs.file_or_platform_references_only", True)
    equal(e, "deployment.secret_inputs.values_recorded", False)


def check_api_registration(e: dict[str, Any]) -> None:
    registration = obj(field(e, "dataplane.registration"), "dataplane.registration")
    if registration.get("interface") not in {"rest", "flowplane_cli"}:
        fail("dataplane.registration.interface: supported REST or flowplane CLI required")
    if registration.get("method") != "POST":
        fail("dataplane.registration.method: expected supported POST operation")
    path = registration.get("path")
    if not isinstance(path, str) or not re.fullmatch(r"/api/v1/teams/[^/]+/dataplanes", path):
        fail("dataplane.registration.path: expected supported team dataplane API path")
    if registration.get("status") not in {200, 201}:
        fail("dataplane.registration.status: expected successful API registration")
    if not UUID.fullmatch(str(registration.get("dataplane_id", ""))):
        fail("dataplane.registration.dataplane_id: response UUID required")
    if registration.get("direct_sql_used") is not False:
        fail("dataplane.registration.direct_sql_used: registration must use supported product API")
    if registration.get("private_key_returned") is not False:
        fail("dataplane.registration.private_key_returned: registration response must not disclose a key")


def check_xds_mtls(e: dict[str, Any]) -> None:
    probes = by_id(field(e, "xds.probes"), "xds.probes")
    expected = {
        "valid": (True, "accepted"),
        "wrong_ca": (False, "tls_rejected"),
        "wrong_server_name": (False, "tls_rejected"),
        "missing_client_certificate": (False, "tls_rejected"),
    }
    for probe_id, (connected, outcome) in expected.items():
        if probe_id not in probes:
            fail(f"xds.probes: missing {probe_id!r} case")
        probe = probes[probe_id]
        named_origin(probe.get("origin"), f"xds.probes[{probe_id}].origin", tailnet=True, outside_fly=False)
        if probe.get("network_path") != "tailscale":
            fail(f"xds.probes[{probe_id}].network_path: expected 'tailscale'")
        if probe.get("port") != 18000:
            fail(f"xds.probes[{probe_id}].port: expected 18000")
        if probe.get("connected") is not connected or probe.get("outcome") != outcome:
            fail(f"xds.probes[{probe_id}]: expected connected={connected!r}, outcome={outcome!r}")
    if probes["valid"].get("registered_client_identity") is not True:
        fail("xds.probes[valid].registered_client_identity: registered identity was not proven")
    if probes["valid"].get("server_name_verified") is not True:
        fail("xds.probes[valid].server_name_verified: SAN verification was not proven")
    if probes["valid"].get("client_certificate_unexpired") is not True:
        fail("xds.probes[valid].client_certificate_unexpired: validity was not proven")


def check_diagnostics_mtls(e: dict[str, Any]) -> None:
    probes = by_id(field(e, "diagnostics.mtls_probes"), "diagnostics.mtls_probes")
    expected = {
        "valid": (True, "accepted_and_committed"),
        "wrong_ca": (False, "tls_rejected"),
        "wrong_server_name": (False, "tls_rejected"),
        "missing_client_certificate": (False, "tls_rejected"),
    }
    for probe_id, (connected, outcome) in expected.items():
        if probe_id not in probes:
            fail(f"diagnostics.mtls_probes: missing {probe_id!r} case")
        probe = probes[probe_id]
        named_origin(probe.get("origin"), f"diagnostics.mtls_probes[{probe_id}].origin", tailnet=True, outside_fly=False)
        if probe.get("connected") is not connected or probe.get("outcome") != outcome:
            fail(f"diagnostics.mtls_probes[{probe_id}]: expected connected={connected!r}, outcome={outcome!r}")


def check_tagged_traffic(e: dict[str, Any]) -> None:
    traffic = obj(field(e, "traffic"), "traffic")
    named_origin(traffic.get("origin"), "traffic.origin", tailnet=True, outside_fly=False)
    tag = traffic.get("correlation_tag")
    if not isinstance(tag, str) or len(tag) < 8:
        fail("traffic.correlation_tag: unique tagged request value required")
    if traffic.get("via_component") != "vm_envoy":
        fail("traffic.via_component: request must traverse VM Envoy")
    if traffic.get("status") != 200:
        fail("traffic.status: tagged request did not succeed")
    if traffic.get("upstream_observed_tag") != tag:
        fail("traffic.upstream_observed_tag: controlled upstream did not observe the request tag")
    if not isinstance(traffic.get("controlled_upstream_marker"), str) or not traffic["controlled_upstream_marker"]:
        fail("traffic.controlled_upstream_marker: deterministic upstream marker required")
    if traffic.get("response_marker") != traffic.get("controlled_upstream_marker"):
        fail("traffic.response_marker: response did not come from controlled upstream")


def check_cp_absent_from_request_path(e: dict[str, Any]) -> None:
    proof = obj(field(e, "traffic.control_plane_absence"), "traffic.control_plane_absence")
    if proof.get("observation_window_includes_tagged_request") is not True:
        fail("traffic.control_plane_absence: observation window did not include tagged request")
    if proof.get("cp_ingress_requests_with_tag") != 0:
        fail("traffic.control_plane_absence.cp_ingress_requests_with_tag: CP appeared in request path")
    if proof.get("upstream_peer_component") != "vm_envoy":
        fail("traffic.control_plane_absence.upstream_peer_component: upstream peer was not VM Envoy")
    if proof.get("request_path") != ["probe_client", "vm_envoy", "controlled_upstream"]:
        fail("traffic.control_plane_absence.request_path: expected client -> VM Envoy -> controlled upstream")


def check_diagnostics_agreement(e: dict[str, Any]) -> None:
    agreement = obj(field(e, "diagnostics.agreement"), "diagnostics.agreement")
    dataplane_id = field(e, "dataplane.registration.dataplane_id")
    if agreement.get("dataplane_id") != dataplane_id:
        fail("diagnostics.agreement.dataplane_id: observables do not identify the registered dataplane")
    if agreement.get("agent_health_status") != 200:
        fail("diagnostics.agreement.agent_health_status: expected ready agent")
    if agreement.get("heartbeat_advanced") is not True:
        fail("diagnostics.agreement.heartbeat_advanced: persisted heartbeat did not advance")
    if agreement.get("stats_live_dataplanes_contains_id") is not True:
        fail("diagnostics.agreement.stats_live_dataplanes_contains_id: team stats disagree")
    if agreement.get("xds_status") != "connected":
        fail("diagnostics.agreement.xds_status: xDS status disagrees")
    if agreement.get("diagnostics_commit_acknowledged") is not True:
        fail("diagnostics.agreement.diagnostics_commit_acknowledged: committed report ack absent")
    if agreement.get("cp_dialed_envoy_admin") is not False:
        fail("diagnostics.agreement.cp_dialed_envoy_admin: CP must not access Envoy admin")
    ids = agreement.get("observable_dataplane_ids")
    if ids != [dataplane_id, dataplane_id, dataplane_id, dataplane_id]:
        fail("diagnostics.agreement.observable_dataplane_ids: agent/heartbeat/stats/xDS IDs do not agree")


def check_public_exposure(e: dict[str, Any]) -> None:
    scan = obj(field(e, "public_scan"), "public_scan")
    named_origin(scan.get("origin"), "public_scan.origin", tailnet=False, outside_fly=True)
    probes = by_id(scan.get("probes"), "public_scan.probes")
    expected = {
        "api_https_443": "reachable_tls_valid",
        "xds_18000": "unreachable",
        "postgresql": "unreachable",
        "rls_admin": "unreachable",
        "envoy_admin_9901": "unreachable",
        "agent_health_19902": "unreachable",
    }
    for probe_id, outcome in expected.items():
        if probe_id not in probes:
            fail(f"public_scan.probes: missing {probe_id!r}")
        probe = probes[probe_id]
        if probe.get("outcome") != outcome:
            fail(f"public_scan.probes[{probe_id}].outcome: expected {outcome!r}")
        for name in ("dns_result", "address_family", "protocol", "timeout_seconds", "exit_status"):
            if name not in probe:
                fail(f"public_scan.probes[{probe_id}].{name}: required public scan evidence absent")
    api = probes["api_https_443"]
    if api.get("tls_terminated_by") != "flowplane" or api.get("leaf_san_match") is not True:
        fail("public_scan.probes[api_https_443]: Flowplane API TLS/SAN proof absent")
    if api.get("fly_ingress_mode") != "raw_tcp_passthrough" or api.get("flowplane_target_port") != 8080:
        fail("public_scan.probes[api_https_443]: expected Fly raw TCP 443 -> Flowplane 8080")


def check_internal_probes(e: dict[str, Any]) -> None:
    internal = obj(field(e, "internal_probes"), "internal_probes")
    named_origin(internal.get("origin"), "internal_probes.origin", tailnet=True, outside_fly=False)
    probes = by_id(internal.get("probes"), "internal_probes.probes")
    expected = {
        "xds_18000_valid_mtls": "accepted",
        "rls_grpc_valid_mtls": "accepted",
        "rls_admin_from_cp_network": "https_authenticated",
        "postgresql_from_cp_network": "authenticated",
        "envoy_admin_vm_loopback": "reachable_loopback_only",
        "agent_health_vm_loopback": "reachable_loopback_only",
    }
    for probe_id, outcome in expected.items():
        if probe_id not in probes:
            fail(f"internal_probes.probes: missing {probe_id!r}")
        if probes[probe_id].get("outcome") != outcome:
            fail(f"internal_probes.probes[{probe_id}].outcome: expected {outcome!r}")
        if not isinstance(probes[probe_id].get("observed_at_utc"), str):
            fail(f"internal_probes.probes[{probe_id}].observed_at_utc: timestamp required")


def check_teardown(e: dict[str, Any]) -> None:
    teardown = obj(field(e, "teardown"), "teardown")
    if teardown.get("rehearsal_completed") is not True:
        fail("teardown.rehearsal_completed: teardown rehearsal evidence absent")
    if teardown.get("evidence_frozen_before_destruction") is not True:
        fail("teardown.evidence_frozen_before_destruction: two-phase teardown was not followed")
    expected_classes = {
        "fly_apps", "fly_machines", "fly_ips", "fly_volumes", "postgresql_resources",
        "tailscale_nodes", "tailscale_keys", "vm_instances", "local_credentials", "product_fixtures",
    }
    inventories = by_id(teardown.get("absence_inventories"), "teardown.absence_inventories")
    missing = sorted(expected_classes - inventories.keys())
    if missing:
        fail(f"teardown.absence_inventories: missing resource classes {missing!r}")
    for resource_id in sorted(expected_classes):
        item = inventories[resource_id]
        if item.get("unexpected_remaining_count") != 0:
            fail(f"teardown.absence_inventories[{resource_id}]: unexpected residue remains")
        if not isinstance(item.get("authoritative_source"), str) or not item["authoritative_source"]:
            fail(f"teardown.absence_inventories[{resource_id}].authoritative_source: required")
        if not isinstance(item.get("observed_at_utc"), str) or not item["observed_at_utc"]:
            fail(f"teardown.absence_inventories[{resource_id}].observed_at_utc: required")
    if teardown.get("provider_retained_residue_reported") is not True:
        fail("teardown.provider_retained_residue_reported: retained audit/billing residue disposition required")


def check_secret_redaction(e: dict[str, Any]) -> None:
    redaction = obj(field(e, "redaction"), "redaction")
    if redaction.get("sanitized_projection") is not True:
        fail("redaction.sanitized_projection: evidence must be explicitly sanitized")
    if redaction.get("secret_values_read_by_test") is not False:
        fail("redaction.secret_values_read_by_test: acceptance test must not read secrets")
    if not isinstance(redaction.get("files_scanned"), int) or redaction["files_scanned"] < 1:
        fail("redaction.files_scanned: final publication-tree scan evidence absent")
    expected_patterns = {"private_keys", "certificate_bodies", "bearer_tokens", "provider_credentials", "synthetic_canaries"}
    patterns = set(redaction.get("pattern_classes", []))
    if not expected_patterns.issubset(patterns):
        fail(f"redaction.pattern_classes: missing {sorted(expected_patterns - patterns)!r}")
    if redaction.get("match_count") != 0 or redaction.get("undisposed_match_count") != 0:
        fail("redaction: publication evidence contains undisposed secret-shaped matches")
    if redaction.get("scan_after_final_edit") is not True:
        fail("redaction.scan_after_final_edit: final-tree scan is stale or absent")
    serialized = json.dumps(e, sort_keys=True)
    for pattern in SECRET_SHAPES:
        if pattern.search(serialized):
            fail(f"sanitized evidence contains prohibited secret material matching {pattern.pattern!r}")


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.3-DEPLOY": ("hardened Flowplane, PostgreSQL, RLS, API/xDS topology", check_deployment),
    "FPV2-D23.3-REGISTER": ("supported API dataplane registration", check_api_registration),
    "FPV2-D23.3-XDS-MTLS": ("valid and negative xDS mTLS matrix", check_xds_mtls),
    "FPV2-D23.3-DIAG-MTLS": ("valid and negative diagnostics mTLS matrix", check_diagnostics_mtls),
    "FPV2-D23.3-TRAFFIC": ("tagged VM Envoy traffic to controlled upstream", check_tagged_traffic),
    "FPV2-D23.3-NO-CP-DATA": ("control plane absent from request path", check_cp_absent_from_request_path),
    "FPV2-D23.3-AGREEMENT": ("heartbeat, stats, xDS, and agent diagnostics agreement", check_diagnostics_agreement),
    "FPV2-D23.3-PUBLIC-SCAN": ("named independent public-origin exposure scan", check_public_exposure),
    "FPV2-D23.3-INTERNAL": ("named internal-origin probes", check_internal_probes),
    "FPV2-D23.3-TEARDOWN": ("two-phase teardown completeness", check_teardown),
    "FPV2-D23.3-REDACTION": ("secret-free sanitized evidence", check_secret_redaction),
}


def evidence_path(argument: str | None) -> Path:
    if argument:
        return Path(argument)
    configured = os.environ.get("FLOWPLANE_SECURE_TOPOLOGY_EVIDENCE")
    return Path(configured) if configured else DEFAULT_EVIDENCE


def load_evidence(path: Path) -> dict[str, Any]:
    if not path.is_file():
        fail(
            f"live qualification evidence absent: {path.resolve()} "
            "(set FLOWPLANE_SECURE_TOPOLOGY_EVIDENCE or pass --evidence)"
        )
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"sanitized evidence unreadable: {path.resolve()}: {error}")
    return obj(value, "evidence root")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", help="sanitized secure-topology JSON projection")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), help="run exactly one independently rerunnable scenario")
    parser.add_argument("--list", action="store_true", help="list scenario IDs and exit")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list:
        for scenario_id, (description, _) in SCENARIOS.items():
            print(f"{scenario_id}\t{description}")
        return 0

    selected = [args.scenario] if args.scenario else list(SCENARIOS)
    path = evidence_path(args.evidence)
    try:
        evidence = load_evidence(path)
    except ContractFailure as error:
        for scenario_id in selected:
            print(f"{scenario_id}: FAIL: {error}", file=sys.stderr)
        print(f"secure topology acceptance: FAIL ({len(selected)}/{len(selected)} scenarios)", file=sys.stderr)
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
        print(f"secure topology acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"secure topology acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
