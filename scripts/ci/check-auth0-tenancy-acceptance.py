#!/usr/bin/env python3
"""Independent black-box acceptance gate for fpv2-d23.4 Auth0/tenancy evidence.

The gate consumes a sanitized JSON projection produced by a live qualification run. It
never reads raw tokens, subjects, emails, credentials, provider resources, private paths,
production source, or deployment packaging. Every scenario is independently runnable
with ``--scenario <id>``. ``--self-test`` validates the harness without live evidence.
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

DEFAULT_EVIDENCE = Path(".artifacts/qualification/fpv2-d23.4/auth0-tenancy.json")
SCHEMA = "flowplane.qualification.auth0-tenancy/v1"
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.I)
FLOWPLANE_UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[47][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.I)
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
REQUEST_REF = re.compile(r"^req-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.I)
AUDIT_REF = re.compile(r"^audit-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.I)
EMAIL = re.compile(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b")
JWT = re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
AUTH0_SUBJECT = re.compile(r"(?i)\b(?:auth0|google-oauth2|github|windowslive|waad)\|[^\s\"']+")
ABSOLUTE_PRIVATE_PATH = re.compile(r"(?i)(?:/Users/|/home/|[A-Z]:\\Users\\|file://|~/(?:\.|[^\s]))")
SECRET_VALUE = re.compile(
    r"(?i)(?:authorization\s*:\s*bearer|client[_ -]?secret|private[_ -]?key|password|"
    r"access[_ -]?token|refresh[_ -]?token|id[_ -]?token|authorization[_ -]?code)"
    r"\s*[:=]\s*[\"']?[^\s\"',}]+"
)
PROHIBITED_KEYS = {
    "sub", "email", "access_token", "refresh_token", "id_token", "authorization_code",
    "code_verifier", "code_challenge", "password", "client_secret", "private_key", "cookie",
    "raw_token", "raw_claims", "raw_path", "user_id",
}
PERSONAS = {
    "platform_operator": "platform_admin_without_tenant_membership",
    "alpha_owner": "alpha_owner_admin",
    "alpha_payments_member": "alpha_payments_scoped_member",
    "alpha_multi_team_member": "alpha_payments_and_shared_member",
    "cross_org_user": "alpha_and_beta_member",
    "beta_owner": "beta_owner_admin",
    "beta_member": "beta_scoped_member",
    "authenticated_non_member": "authenticated_without_tenant_membership",
}


class ContractFailure(AssertionError):
    """The sanitized projection did not prove an acceptance invariant."""


def fail(message: str) -> NoReturn:
    raise ContractFailure(message)


def obj(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{name}: required object is absent or malformed")
    return value


def seq(value: Any, name: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{name}: required array is absent or malformed")
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


def text(value: Any, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        fail(f"{name}: non-empty text required")
    return value


def indexed(items: Any, name: str, key: str = "id") -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for index, value in enumerate(seq(items, name)):
        item = obj(value, f"{name}[{index}]")
        item_id = text(item.get(key), f"{name}[{index}].{key}")
        if item_id in result:
            fail(f"{name}: duplicate {key} {item_id!r}")
        result[item_id] = item
    return result


def require_uuid(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not UUID.fullmatch(candidate):
        fail(f"{name}: synthetic UUIDv4 required")
    return candidate


def require_flowplane_uuid(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not FLOWPLANE_UUID.fullmatch(candidate):
        fail(f"{name}: Flowplane UUIDv4 or UUIDv7 required")
    return candidate


def require_sha(value: Any, name: str) -> str:
    candidate = text(value, name)
    if not SHA256.fullmatch(candidate):
        fail(f"{name}: sha256 fingerprint required")
    return candidate


def check_run_and_fixtures(e: dict[str, Any]) -> None:
    equal(e, "schema", SCHEMA)
    equal(e, "run.release", "3.1.3")
    require_uuid(field(e, "run.run_id"), "run.run_id")
    text(field(e, "run.started_at_utc"), "run.started_at_utc")
    text(field(e, "run.finished_at_utc"), "run.finished_at_utc")
    equal(e, "run.synthetic_only", True)
    equal(e, "run.rerunnable", True)
    equal(e, "run.prior_run_cleanup_verified", True)
    prefix = text(field(e, "run.fixture_prefix"), "run.fixture_prefix")
    if not re.fullmatch(r"fpv2-d23-4-[a-z0-9]{8,32}", prefix):
        fail("run.fixture_prefix: unique run-scoped synthetic prefix required")

    orgs = indexed(field(e, "fixtures.organizations"), "fixtures.organizations", "key")
    if set(orgs) != {"alpha", "beta"}:
        fail("fixtures.organizations: exactly synthetic alpha and beta fixtures required")
    all_ids: list[str] = []
    all_ports: list[int] = []
    all_markers: list[str] = []
    names_by_org: dict[str, set[str]] = {}
    resources_by_team: list[set[str]] = []
    for org_key, org in orgs.items():
        all_ids.append(require_flowplane_uuid(org.get("id"), f"fixtures.organizations[{org_key}].id"))
        teams = indexed(org.get("teams"), f"fixtures.organizations[{org_key}].teams", "key")
        if set(teams) != {"payments", "shared"}:
            fail(f"fixtures.organizations[{org_key}].teams: exactly payments and shared required")
        names_by_org[org_key] = set()
        for team_key, team in teams.items():
            name = text(team.get("name"), f"fixtures.{org_key}.{team_key}.name")
            names_by_org[org_key].add(name)
            all_ids.append(require_flowplane_uuid(team.get("id"), f"fixtures.{org_key}.{team_key}.id"))
            port = team.get("listener_port")
            if not isinstance(port, int) or isinstance(port, bool) or not (1024 <= port <= 65535):
                fail(f"fixtures.{org_key}.{team_key}.listener_port: unique non-privileged port required")
            all_ports.append(port)
            marker = text(team.get("correlation_marker"), f"fixtures.{org_key}.{team_key}.correlation_marker")
            if len(marker) < 12:
                fail(f"fixtures.{org_key}.{team_key}.correlation_marker: run-unique marker required")
            all_markers.append(marker)
            resources = indexed(team.get("resources"), f"fixtures.{org_key}.{team_key}.resources")
            if not resources:
                fail(f"fixtures.{org_key}.{team_key}.resources: overlapping resource fixture required")
            resources_by_team.append({text(item.get("name"), "resource.name") for item in resources.values()})
            all_ids.extend(require_flowplane_uuid(item.get("id"), "resource.id") for item in resources.values())
    if names_by_org["alpha"] != names_by_org["beta"] or len(names_by_org["alpha"]) != 2:
        fail("fixtures: team names must deliberately overlap across organizations")
    if not set.intersection(*resources_by_team):
        fail("fixtures: at least one resource name must overlap across all four teams")
    for label, values in (("IDs", all_ids), ("listener ports", all_ports), ("correlation markers", all_markers)):
        if len(values) != len(set(values)):
            fail(f"fixtures: {label} must be globally unique")

    personas = indexed(field(e, "personas"), "personas", "id")
    if set(personas) != set(PERSONAS):
        fail(f"personas: expected exactly {sorted(PERSONAS)!r}")
    fingerprints: list[str] = []
    for persona_id, standing in PERSONAS.items():
        persona = personas[persona_id]
        if persona.get("standing") != standing:
            fail(f"personas[{persona_id}].standing: expected {standing!r}")
        if persona.get("synthetic") is not True or persona.get("real_person_data_recorded") is not False:
            fail(f"personas[{persona_id}]: synthetic-only attestation required")
        fingerprints.append(require_sha(persona.get("subject_fingerprint"), f"personas[{persona_id}].subject_fingerprint"))
    if len(fingerprints) != len(set(fingerprints)):
        fail("personas: subject fingerprints must be unique")


def check_pkce_claims(e: dict[str, Any]) -> None:
    equal(e, "identity.provider", "auth0")
    equal(e, "identity.flow", "authorization_code_pkce")
    equal(e, "identity.public_native_client", True)
    equal(e, "identity.client_secret_used", False)
    equal(e, "identity.password_grant_used", False)
    equal(e, "identity.browser_profiles_isolated", True)
    equal(e, "identity.saved_credential_kind", "access_token")
    equal(e, "identity.raw_credentials_recorded", False)
    sessions = indexed(field(e, "identity.pkce_sessions"), "identity.pkce_sessions", "persona")
    if set(sessions) != set(PERSONAS):
        fail("identity.pkce_sessions: one real PKCE session per approved persona required")
    for persona_id, session in sessions.items():
        for key in ("human_login_completed", "authorization_code_received", "pkce_s256_verified", "access_token_saved"):
            if session.get(key) is not True:
                fail(f"identity.pkce_sessions[{persona_id}].{key}: real PKCE proof absent")
        claims = obj(session.get("claim_assertions"), f"identity.pkce_sessions[{persona_id}].claim_assertions")
        expected = {
            "jwt_three_segments": True, "alg": "RS256", "kid_present": True,
            "kid_resolved_exactly_one_jwks_key": True, "signature_verified": True,
            "issuer_exact_match": True, "audience_contains_flowplane_api": True,
            "subject_present": True, "expiry_numeric_and_future": True,
        }
        for key, value in expected.items():
            if claims.get(key) != value:
                fail(f"identity.pkce_sessions[{persona_id}].claim_assertions.{key}: expected {value!r}")
        require_sha(claims.get("subject_fingerprint"), f"identity.pkce_sessions[{persona_id}].subject_fingerprint")
        if claims.get("token_value_recorded") is not False or claims.get("claim_values_recorded") is not False:
            fail(f"identity.pkce_sessions[{persona_id}]: raw token/claim values must not be recorded")


def check_identity_authority(e: dict[str, Any]) -> None:
    authority = obj(field(e, "authorization_authority"), "authorization_authority")
    if authority.get("oidc_identity_inputs") != ["immutable_subject"]:
        fail("authorization_authority.oidc_identity_inputs: OIDC must supply immutable identity only")
    ignored = set(seq(authority.get("oidc_claims_not_tenant_authority"), "oidc_claims_not_tenant_authority"))
    required_ignored = {"email", "display_name", "roles", "groups", "organization"}
    if not required_ignored.issubset(ignored):
        fail(f"authorization_authority: missing ignored provider claims {sorted(required_ignored - ignored)!r}")
    if authority.get("membership_source") != "flowplane_postgresql":
        fail("authorization_authority.membership_source: Flowplane membership must be authoritative")
    if authority.get("grant_source") != "flowplane_postgresql":
        fail("authorization_authority.grant_source: Flowplane grants must be authoritative")
    if authority.get("active_org_source") != "explicit_flowplane_selector":
        fail("authorization_authority.active_org_source: explicit Flowplane selector required")
    if authority.get("auth0_organizations_used") is not False or authority.get("auth0_roles_used") is not False:
        fail("authorization_authority: Auth0 organizations/roles must not authorize tenants")
    if authority.get("direct_sql_fixture_mutation_used") is not False:
        fail("authorization_authority.direct_sql_fixture_mutation_used: supported product paths required")


def auth_cases(e: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return indexed(field(e, "authorization.cases"), "authorization.cases")


def assert_case(case: dict[str, Any], case_id: str, expected: dict[str, Any]) -> None:
    for key, value in expected.items():
        if case.get(key) != value:
            fail(f"authorization.cases[{case_id}].{key}: expected {value!r}, observed {case.get(key)!r}")
    require_flowplane_uuid(case.get("target_org_id"), f"authorization.cases[{case_id}].target_org_id")
    require_flowplane_uuid(case.get("target_team_id"), f"authorization.cases[{case_id}].target_team_id")


def check_same_team(e: dict[str, Any]) -> None:
    cases = auth_cases(e)
    required = {
        "api_alpha_same_team": ("api", "alpha_owner", "alpha", "success"),
        "cli_alpha_same_team": ("cli", "alpha_payments_member", "alpha", "success"),
        "api_beta_same_team": ("api", "beta_owner", "beta", "success"),
        "cli_beta_same_team": ("cli", "beta_member", "beta", "success"),
    }
    for case_id, (channel, persona, org, outcome) in required.items():
        if case_id not in cases:
            fail(f"authorization.cases: missing {case_id!r}")
        assert_case(cases[case_id], case_id, {
            "channel": channel, "persona": persona, "target_org": org, "relationship": "same_team",
            "authenticated": True, "authorized": True, "outcome": outcome, "resource_visible": True,
            "intended_mutation_effect_count": 1, "foreign_mutation_effect_count": 0,
        })


def check_isolation(e: dict[str, Any]) -> None:
    cases = auth_cases(e)
    required = {
        "api_alpha_sibling_denied": ("api", "alpha_payments_member", "alpha", "sibling_team"),
        "cli_alpha_sibling_denied": ("cli", "alpha_payments_member", "alpha", "sibling_team"),
        "api_beta_sibling_denied": ("api", "beta_member", "beta", "sibling_team"),
        "cli_beta_sibling_denied": ("cli", "beta_member", "beta", "sibling_team"),
        "api_alpha_to_beta_denied": ("api", "alpha_payments_member", "beta", "cross_org"),
        "cli_alpha_to_beta_denied": ("cli", "alpha_payments_member", "beta", "cross_org"),
        "api_beta_to_alpha_denied": ("api", "beta_member", "alpha", "cross_org"),
        "cli_beta_to_alpha_denied": ("cli", "beta_member", "alpha", "cross_org"),
    }
    for case_id, (channel, persona, org, relation) in required.items():
        if case_id not in cases:
            fail(f"authorization.cases: missing {case_id!r}")
        assert_case(cases[case_id], case_id, {
            "channel": channel, "persona": persona, "target_org": org, "relationship": relation,
            "authenticated": True, "authorized": False, "outcome": "hidden_or_documented_denial",
            "resource_visible": False, "intended_mutation_effect_count": 0,
            "foreign_mutation_effect_count": 0, "state_fingerprint_unchanged": True,
        })


def check_selectors(e: dict[str, Any]) -> None:
    cases = auth_cases(e)
    required = {
        "api_multi_org_without_selector": ("api", "none", False, "ambiguous_org"),
        "cli_multi_org_without_selector": ("cli", "none", False, "ambiguous_org"),
        "api_valid_selector_name": ("api", "name", True, "success"),
        "cli_valid_selector_name": ("cli", "name", True, "success"),
        "api_valid_selector_uuid": ("api", "uuid", True, "success"),
        "cli_valid_selector_uuid": ("cli", "uuid", True, "success"),
        "api_foreign_selector_denied": ("api", "foreign_uuid", False, "foreign_org_denied"),
        "cli_foreign_selector_denied": ("cli", "foreign_name", False, "foreign_org_denied"),
    }
    for case_id, (channel, selector_kind, allowed, outcome) in required.items():
        if case_id not in cases:
            fail(f"authorization.cases: missing {case_id!r}")
        case = cases[case_id]
        expected = {
            "channel": channel, "persona": "cross_org_user", "selector_kind": selector_kind,
            "authenticated": True, "authorized": allowed, "outcome": outcome,
            "resource_visible": allowed, "foreign_mutation_effect_count": 0,
        }
        if not allowed:
            expected.update({"intended_mutation_effect_count": 0, "state_fingerprint_unchanged": True})
        assert_case(case, case_id, expected)


def check_no_implicit_tenant_access(e: dict[str, Any]) -> None:
    cases = auth_cases(e)
    required = {
        "api_non_member_denied": ("api", "authenticated_non_member"),
        "cli_non_member_denied": ("cli", "authenticated_non_member"),
        "api_platform_admin_no_membership_denied": ("api", "platform_operator"),
        "cli_platform_admin_no_membership_denied": ("cli", "platform_operator"),
    }
    for case_id, (channel, persona) in required.items():
        if case_id not in cases:
            fail(f"authorization.cases: missing {case_id!r}")
        assert_case(cases[case_id], case_id, {
            "channel": channel, "persona": persona, "authenticated": True, "authorized": False,
            "outcome": "tenant_membership_required", "resource_visible": False,
            "intended_mutation_effect_count": 0, "foreign_mutation_effect_count": 0,
            "state_fingerprint_unchanged": True,
        })


def check_stale_authorization(e: dict[str, Any]) -> None:
    cases = auth_cases(e)
    required = {
        "api_stale_membership_denied": ("api", "membership_removed"),
        "cli_stale_membership_denied": ("cli", "membership_removed"),
        "api_stale_grant_denied": ("api", "grant_removed"),
        "cli_stale_grant_denied": ("cli", "grant_removed"),
    }
    for case_id, (channel, removal) in required.items():
        if case_id not in cases:
            fail(f"authorization.cases: missing {case_id!r}")
        assert_case(cases[case_id], case_id, {
            "channel": channel, "revocation_kind": removal, "authenticated": True,
            "session_token_still_cryptographically_valid": True, "revocation_committed_before_request": True,
            "authorized": False, "outcome": "stale_authorization_denied", "resource_visible": False,
            "intended_mutation_effect_count": 0, "foreign_mutation_effect_count": 0,
            "state_fingerprint_unchanged": True,
        })


def check_invalid_tokens(e: dict[str, Any]) -> None:
    cases = indexed(field(e, "invalid_token_cases"), "invalid_token_cases")
    for channel in ("api", "cli"):
        for defect in ("wrong_issuer", "wrong_audience", "wrong_signature", "expired"):
            case_id = f"{channel}_{defect}"
            if case_id not in cases:
                fail(f"invalid_token_cases: missing {case_id!r}")
            case = cases[case_id]
            expected = {
                "channel": channel, "defect": defect, "authenticated": False, "authorized": False,
                "outcome": "rejected_before_authorization", "http_status": 401,
                "mutation_effect_count": 0, "state_fingerprint_unchanged": True,
                "raw_token_recorded": False,
            }
            for key, value in expected.items():
                if case.get(key) != value:
                    fail(f"invalid_token_cases[{case_id}].{key}: expected {value!r}")
            require_request_audit(case, f"invalid_token_cases[{case_id}]")


def require_request_audit(case: dict[str, Any], name: str) -> None:
    request_ref = text(case.get("request_id_ref"), f"{name}.request_id_ref")
    if not REQUEST_REF.fullmatch(request_ref):
        fail(f"{name}.request_id_ref: sanitized request reference required")
    audit = obj(case.get("audit"), f"{name}.audit")
    if not AUDIT_REF.fullmatch(str(audit.get("event_ref", ""))):
        fail(f"{name}.audit.event_ref: sanitized audit reference required")
    if audit.get("request_id_matched") is not True or audit.get("actor_fingerprint_only") is not True:
        fail(f"{name}.audit: request linkage and fingerprint-only actor required")
    if audit.get("tenant_scoped") is not True or audit.get("decision_recorded") is not True:
        fail(f"{name}.audit: tenant-scoped authorization decision evidence required")
    if audit.get("raw_subject_recorded") is not False or audit.get("token_recorded") is not False:
        fail(f"{name}.audit: raw subject/token must be absent")


def check_api_cli_and_audit(e: dict[str, Any]) -> None:
    equal(e, "interfaces.api.real_https_requests", True)
    equal(e, "interfaces.api.pkce_access_tokens_reused_securely", True)
    equal(e, "interfaces.api.direct_database_calls", False)
    equal(e, "interfaces.cli.real_candidate_binary", True)
    equal(e, "interfaces.cli.isolated_homes", True)
    equal(e, "interfaces.cli.context_org_selector_exercised", True)
    equal(e, "interfaces.cli.raw_output_sanitized", True)
    equal(e, "interfaces.cli.direct_database_calls", False)
    all_cases = list(auth_cases(e).items()) + list(indexed(field(e, "invalid_token_cases"), "invalid_token_cases").items())
    channels = {case.get("channel") for _, case in all_cases}
    if channels != {"api", "cli"}:
        fail("interfaces: both API and CLI evidence required")
    audited = 0
    for case_id, case in all_cases:
        if case.get("authorized") is False:
            require_request_audit(case, f"case[{case_id}]")
            audited += 1
    if audited < 24:
        fail("audit: complete negative matrix with request-linked audit evidence required")


def walk(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key.lower() in PROHIBITED_KEYS:
                fail(f"redaction: prohibited raw field {path}.{key}")
            walk(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            walk(child, f"{path}[{index}]")
    elif isinstance(value, str):
        for label, pattern in (
            ("email", EMAIL), ("JWT", JWT), ("real provider subject", AUTH0_SUBJECT),
            ("private path", ABSOLUTE_PRIVATE_PATH), ("credential value", SECRET_VALUE),
        ):
            if pattern.search(value):
                fail(f"redaction: {path} contains prohibited {label}")


def check_redaction(e: dict[str, Any]) -> None:
    equal(e, "redaction.sanitized_projection", True)
    equal(e, "redaction.raw_subjects_recorded", False)
    equal(e, "redaction.tokens_recorded", False)
    equal(e, "redaction.emails_recorded", False)
    equal(e, "redaction.private_paths_recorded", False)
    equal(e, "redaction.credentials_recorded", False)
    equal(e, "redaction.scan_after_final_edit", True)
    equal(e, "redaction.undisposed_match_count", 0)
    classes = set(seq(field(e, "redaction.pattern_classes"), "redaction.pattern_classes"))
    expected = {"real_subjects", "jwt_tokens", "emails", "private_paths", "provider_credentials", "oauth_transactions"}
    if not expected.issubset(classes):
        fail(f"redaction.pattern_classes: missing {sorted(expected - classes)!r}")
    walk(e)


def check_cleanup(e: dict[str, Any]) -> None:
    cleanup = obj(field(e, "cleanup"), "cleanup")
    if cleanup.get("fixture_names_run_scoped") is not True or cleanup.get("safe_to_rerun") is not True:
        fail("cleanup: run-scoped fixture and rerun safety attestations required")
    if cleanup.get("evidence_frozen_before_cleanup") is not True or cleanup.get("completed_at_utc") is None:
        fail("cleanup: two-phase evidence freeze and completion timestamp required")
    inventories = indexed(cleanup.get("absence_inventories"), "cleanup.absence_inventories")
    expected = {
        "auth0_users", "auth0_sessions", "auth0_clients", "auth0_apis", "auth0_grants",
        "product_memberships", "product_grants", "product_resources", "cli_credentials", "browser_profiles",
    }
    if set(inventories) != expected:
        fail(f"cleanup.absence_inventories: expected exactly {sorted(expected)!r}")
    for item_id, item in inventories.items():
        if item.get("run_owned_remaining_count") != 0:
            fail(f"cleanup.absence_inventories[{item_id}]: run-owned residue remains")
        if item.get("exact_registered_objects_checked") is not True:
            fail(f"cleanup.absence_inventories[{item_id}]: exact-object absence check required")
        text(item.get("observed_at_utc"), f"cleanup.absence_inventories[{item_id}].observed_at_utc")
    if cleanup.get("default_audience_restored_or_safe_drift_verified") is not True:
        fail("cleanup.default_audience_restored_or_safe_drift_verified: shared Auth0 state disposition required")
    if cleanup.get("provider_retained_logs_reported") is not True:
        fail("cleanup.provider_retained_logs_reported: expected provider residue must be reported")


SCENARIOS: dict[str, tuple[str, Callable[[dict[str, Any]], None]]] = {
    "FPV2-D23.4-FIXTURES": ("two-org/two-team overlapping synthetic fixtures and personas", check_run_and_fixtures),
    "FPV2-D23.4-PKCE": ("real Authorization Code + PKCE and value-free claim assertions", check_pkce_claims),
    "FPV2-D23.4-AUTHORITY": ("OIDC identity separated from Flowplane membership and grants", check_identity_authority),
    "FPV2-D23.4-SAME-TEAM": ("symmetric same-team API and CLI positives", check_same_team),
    "FPV2-D23.4-ISOLATION": ("sibling-team and cross-org denial plus non-effect", check_isolation),
    "FPV2-D23.4-SELECTORS": ("ambiguity, name/UUID selectors, and foreign selector denial", check_selectors),
    "FPV2-D23.4-NO-IMPLICIT": ("non-member and platform-admin-without-membership denial", check_no_implicit_tenant_access),
    "FPV2-D23.4-STALE": ("stale membership and grant fail-closed behavior", check_stale_authorization),
    "FPV2-D23.4-BAD-TOKENS": ("wrong issuer/audience/signature/expiry rejection", check_invalid_tokens),
    "FPV2-D23.4-API-CLI-AUDIT": ("real API/CLI evidence with request-linked audit decisions", check_api_cli_and_audit),
    "FPV2-D23.4-REDACTION": ("no real subjects, tokens, emails, paths, or credentials", check_redaction),
    "FPV2-D23.4-CLEANUP": ("fixture uniqueness, cleanup, and rerun metadata", check_cleanup),
}


def evidence_path(argument: str | None) -> Path:
    if argument:
        return Path(argument)
    configured = os.environ.get("FLOWPLANE_AUTH0_TENANCY_EVIDENCE")
    return Path(configured) if configured else DEFAULT_EVIDENCE


def load_evidence(path: Path) -> dict[str, Any]:
    if not path.is_file():
        fail(
            f"live qualification evidence absent: {path.resolve()} "
            "(set FLOWPLANE_AUTH0_TENANCY_EVIDENCE or pass --evidence)"
        )
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"sanitized evidence unreadable: {path.resolve()}: {error}")
    return obj(value, "evidence root")


def uid(number: int) -> str:
    return f"00000000-0000-4000-8000-{number:012x}"


def uuidv7(number: int) -> str:
    return f"00000000-0000-7000-8000-{number:012x}"


def fingerprint(number: int) -> str:
    return f"sha256:{number:064x}"


def audit(number: int) -> dict[str, Any]:
    return {
        "request_id_ref": f"req-{uid(number)}",
        "audit": {
            "event_ref": f"audit-{uid(number)}", "request_id_matched": True,
            "actor_fingerprint_only": True, "tenant_scoped": True, "decision_recorded": True,
            "raw_subject_recorded": False, "token_recorded": False,
        },
    }


def self_test_evidence() -> dict[str, Any]:
    next_id = 1

    def new_id() -> str:
        nonlocal next_id
        result = uid(next_id)
        next_id += 1
        return result

    organizations = []
    port = 21000
    for org_key in ("alpha", "beta"):
        teams = []
        for team_key, team_name in (("payments", "Payments"), ("shared", "Shared")):
            teams.append({
                "key": team_key, "name": team_name, "id": new_id(), "listener_port": port,
                "correlation_marker": f"marker-{org_key}-{team_key}-unique",
                "resources": [{"id": new_id(), "name": "shared-authz-probe"}],
            })
            port += 1
        organizations.append({"key": org_key, "id": new_id(), "teams": teams})
    personas = [
        {"id": persona_id, "standing": standing, "synthetic": True,
         "real_person_data_recorded": False, "subject_fingerprint": fingerprint(index + 1)}
        for index, (persona_id, standing) in enumerate(PERSONAS.items())
    ]
    sessions = []
    for index, persona_id in enumerate(PERSONAS, 1):
        sessions.append({
            "persona": persona_id, "human_login_completed": True, "authorization_code_received": True,
            "pkce_s256_verified": True, "access_token_saved": True,
            "claim_assertions": {
                "jwt_three_segments": True, "alg": "RS256", "kid_present": True,
                "kid_resolved_exactly_one_jwks_key": True, "signature_verified": True,
                "issuer_exact_match": True, "audience_contains_flowplane_api": True,
                "subject_present": True, "expiry_numeric_and_future": True,
                "subject_fingerprint": fingerprint(index), "token_value_recorded": False,
                "claim_values_recorded": False,
            },
        })
    cases: list[dict[str, Any]] = []
    case_number = 100

    def add(case_id: str, **values: Any) -> None:
        nonlocal case_number
        base = {"id": case_id, "target_org_id": new_id(), "target_team_id": new_id(), **values}
        if values.get("authorized") is False:
            base.update(audit(case_number))
            case_number += 1
        cases.append(base)

    for case_id, channel, persona, org in (
        ("api_alpha_same_team", "api", "alpha_owner", "alpha"),
        ("cli_alpha_same_team", "cli", "alpha_payments_member", "alpha"),
        ("api_beta_same_team", "api", "beta_owner", "beta"),
        ("cli_beta_same_team", "cli", "beta_member", "beta"),
    ):
        add(case_id, channel=channel, persona=persona, target_org=org, relationship="same_team",
            authenticated=True, authorized=True, outcome="success", resource_visible=True,
            intended_mutation_effect_count=1, foreign_mutation_effect_count=0)
    for case_id, channel, persona, org, relation in (
        ("api_alpha_sibling_denied", "api", "alpha_payments_member", "alpha", "sibling_team"),
        ("cli_alpha_sibling_denied", "cli", "alpha_payments_member", "alpha", "sibling_team"),
        ("api_beta_sibling_denied", "api", "beta_member", "beta", "sibling_team"),
        ("cli_beta_sibling_denied", "cli", "beta_member", "beta", "sibling_team"),
        ("api_alpha_to_beta_denied", "api", "alpha_payments_member", "beta", "cross_org"),
        ("cli_alpha_to_beta_denied", "cli", "alpha_payments_member", "beta", "cross_org"),
        ("api_beta_to_alpha_denied", "api", "beta_member", "alpha", "cross_org"),
        ("cli_beta_to_alpha_denied", "cli", "beta_member", "alpha", "cross_org"),
    ):
        add(case_id, channel=channel, persona=persona, target_org=org, relationship=relation,
            authenticated=True, authorized=False, outcome="hidden_or_documented_denial", resource_visible=False,
            intended_mutation_effect_count=0, foreign_mutation_effect_count=0, state_fingerprint_unchanged=True)
    for case_id, channel, selector, allowed, outcome in (
        ("api_multi_org_without_selector", "api", "none", False, "ambiguous_org"),
        ("cli_multi_org_without_selector", "cli", "none", False, "ambiguous_org"),
        ("api_valid_selector_name", "api", "name", True, "success"),
        ("cli_valid_selector_name", "cli", "name", True, "success"),
        ("api_valid_selector_uuid", "api", "uuid", True, "success"),
        ("cli_valid_selector_uuid", "cli", "uuid", True, "success"),
        ("api_foreign_selector_denied", "api", "foreign_uuid", False, "foreign_org_denied"),
        ("cli_foreign_selector_denied", "cli", "foreign_name", False, "foreign_org_denied"),
    ):
        values: dict[str, Any] = {
            "channel": channel, "persona": "cross_org_user", "selector_kind": selector,
            "authenticated": True, "authorized": allowed, "outcome": outcome,
            "resource_visible": allowed, "foreign_mutation_effect_count": 0,
        }
        if not allowed:
            values.update(intended_mutation_effect_count=0, state_fingerprint_unchanged=True)
        add(case_id, **values)
    for case_id, channel, persona in (
        ("api_non_member_denied", "api", "authenticated_non_member"),
        ("cli_non_member_denied", "cli", "authenticated_non_member"),
        ("api_platform_admin_no_membership_denied", "api", "platform_operator"),
        ("cli_platform_admin_no_membership_denied", "cli", "platform_operator"),
    ):
        add(case_id, channel=channel, persona=persona, authenticated=True, authorized=False,
            outcome="tenant_membership_required", resource_visible=False, intended_mutation_effect_count=0,
            foreign_mutation_effect_count=0, state_fingerprint_unchanged=True)
    for case_id, channel, removal in (
        ("api_stale_membership_denied", "api", "membership_removed"),
        ("cli_stale_membership_denied", "cli", "membership_removed"),
        ("api_stale_grant_denied", "api", "grant_removed"),
        ("cli_stale_grant_denied", "cli", "grant_removed"),
    ):
        add(case_id, channel=channel, revocation_kind=removal, authenticated=True,
            session_token_still_cryptographically_valid=True, revocation_committed_before_request=True,
            authorized=False, outcome="stale_authorization_denied", resource_visible=False,
            intended_mutation_effect_count=0, foreign_mutation_effect_count=0, state_fingerprint_unchanged=True)
    invalid = []
    for channel in ("api", "cli"):
        for defect in ("wrong_issuer", "wrong_audience", "wrong_signature", "expired"):
            item = {
                "id": f"{channel}_{defect}", "channel": channel, "defect": defect,
                "authenticated": False, "authorized": False, "outcome": "rejected_before_authorization",
                "http_status": 401, "mutation_effect_count": 0, "state_fingerprint_unchanged": True,
                "raw_token_recorded": False,
            }
            item.update(audit(case_number))
            case_number += 1
            invalid.append(item)
    cleanup_ids = {
        "auth0_users", "auth0_sessions", "auth0_clients", "auth0_apis", "auth0_grants",
        "product_memberships", "product_grants", "product_resources", "cli_credentials", "browser_profiles",
    }
    return {
        "schema": SCHEMA,
        "run": {"release": "3.1.3", "run_id": new_id(), "started_at_utc": "2026-08-18T00:00:00Z",
                "finished_at_utc": "2026-08-18T00:10:00Z", "synthetic_only": True, "rerunnable": True,
                "prior_run_cleanup_verified": True, "fixture_prefix": "fpv2-d23-4-selftest01"},
        "fixtures": {"organizations": organizations}, "personas": personas,
        "identity": {"provider": "auth0", "flow": "authorization_code_pkce", "public_native_client": True,
                     "client_secret_used": False, "password_grant_used": False, "browser_profiles_isolated": True,
                     "saved_credential_kind": "access_token", "raw_credentials_recorded": False,
                     "pkce_sessions": sessions},
        "authorization_authority": {
            "oidc_identity_inputs": ["immutable_subject"],
            "oidc_claims_not_tenant_authority": ["email", "display_name", "roles", "groups", "organization"],
            "membership_source": "flowplane_postgresql", "grant_source": "flowplane_postgresql",
            "active_org_source": "explicit_flowplane_selector", "auth0_organizations_used": False,
            "auth0_roles_used": False, "direct_sql_fixture_mutation_used": False,
        },
        "authorization": {"cases": cases}, "invalid_token_cases": invalid,
        "interfaces": {"api": {"real_https_requests": True, "pkce_access_tokens_reused_securely": True,
                                 "direct_database_calls": False},
                       "cli": {"real_candidate_binary": True, "isolated_homes": True,
                               "context_org_selector_exercised": True, "raw_output_sanitized": True,
                               "direct_database_calls": False}},
        "redaction": {"sanitized_projection": True, "raw_subjects_recorded": False,
                      "tokens_recorded": False, "emails_recorded": False, "private_paths_recorded": False,
                      "credentials_recorded": False, "scan_after_final_edit": True, "undisposed_match_count": 0,
                      "pattern_classes": ["real_subjects", "jwt_tokens", "emails", "private_paths",
                                          "provider_credentials", "oauth_transactions"]},
        "cleanup": {"fixture_names_run_scoped": True, "safe_to_rerun": True,
                    "evidence_frozen_before_cleanup": True, "completed_at_utc": "2026-08-18T00:11:00Z",
                    "absence_inventories": [{"id": item_id, "run_owned_remaining_count": 0,
                                             "exact_registered_objects_checked": True,
                                             "observed_at_utc": "2026-08-18T00:11:00Z"}
                                            for item_id in sorted(cleanup_ids)],
                    "default_audience_restored_or_safe_drift_verified": True,
                    "provider_retained_logs_reported": True},
    }


def run_self_tests() -> int:
    fixture = self_test_evidence()
    failures: list[str] = []
    for scenario_id, (_, check) in SCENARIOS.items():
        try:
            check(fixture)
        except ContractFailure as error:
            failures.append(f"valid fixture rejected by {scenario_id}: {error}")
    uuidv7_fixture = copy.deepcopy(fixture)
    uuidv7_counter = 1

    def next_uuidv7() -> str:
        nonlocal uuidv7_counter
        value = uuidv7(uuidv7_counter)
        uuidv7_counter += 1
        return value

    for organization in uuidv7_fixture["fixtures"]["organizations"]:
        organization["id"] = next_uuidv7()
        for team in organization["teams"]:
            team["id"] = next_uuidv7()
            for resource in team["resources"]:
                resource["id"] = next_uuidv7()
    for case in uuidv7_fixture["authorization"]["cases"]:
        case["target_org_id"] = next_uuidv7()
        case["target_team_id"] = next_uuidv7()
    try:
        check_run_and_fixtures(uuidv7_fixture)
        for case in uuidv7_fixture["authorization"]["cases"]:
            assert_case(case, case["id"], {})
    except ContractFailure as error:
        failures.append(f"UUIDv7 Flowplane fixtures rejected: {error}")
    mutations: list[tuple[str, Callable[[dict[str, Any]], None], str]] = [
        ("missing evidence", lambda value: value.pop("fixtures"), "FPV2-D23.4-FIXTURES"),
        ("UUIDv7 synthetic run ID", lambda value: value["run"].__setitem__("run_id", uuidv7(1)), "FPV2-D23.4-FIXTURES"),
        ("duplicate team port", lambda value: value["fixtures"]["organizations"][1]["teams"][0].__setitem__("listener_port", 21000), "FPV2-D23.4-FIXTURES"),
        ("malformed Flowplane fixture ID", lambda value: value["fixtures"]["organizations"][0].__setitem__("id", "not-a-uuid"), "FPV2-D23.4-FIXTURES"),
        ("unsupported authorization target ID version", lambda value: next(case for case in value["authorization"]["cases"] if case["id"] == "api_alpha_same_team").__setitem__("target_org_id", "00000000-0000-1000-8000-000000000001"), "FPV2-D23.4-SAME-TEAM"),
        ("PKCE signature absent", lambda value: value["identity"]["pkce_sessions"][0]["claim_assertions"].__setitem__("signature_verified", False), "FPV2-D23.4-PKCE"),
        ("foreign mutation effect", lambda value: value["authorization"]["cases"][4].__setitem__("foreign_mutation_effect_count", 1), "FPV2-D23.4-ISOLATION"),
        ("stale grant allowed", lambda value: next(case for case in value["authorization"]["cases"] if case["id"] == "api_stale_grant_denied").__setitem__("authorized", True), "FPV2-D23.4-STALE"),
        ("raw email leak", lambda value: value["leak"].__setitem__("value", "person@example.invalid"), "FPV2-D23.4-REDACTION"),
    ]
    for name, mutate, scenario_id in mutations:
        candidate = copy.deepcopy(fixture)
        candidate.setdefault("leak", {})
        mutate(candidate)
        try:
            SCENARIOS[scenario_id][1](candidate)
        except ContractFailure:
            continue
        failures.append(f"negative self-test did not fail closed: {name}")
    if failures:
        for failure in failures:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        print(f"auth0 tenancy acceptance self-test: FAIL ({len(failures)} failures)", file=sys.stderr)
        return 1
    print(f"auth0 tenancy acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", help="sanitized Auth0/tenancy JSON projection")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), help="run exactly one independently rerunnable scenario")
    parser.add_argument("--list", action="store_true", help="list scenario IDs and exit")
    parser.add_argument("--self-test", action="store_true", help="run built-in positive and fail-closed contract tests")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list:
        for scenario_id, (description, _) in SCENARIOS.items():
            print(f"{scenario_id}\t{description}")
        return 0
    if args.self_test:
        return run_self_tests()
    selected = [args.scenario] if args.scenario else list(SCENARIOS)
    path = evidence_path(args.evidence)
    try:
        evidence = load_evidence(path)
    except ContractFailure as error:
        for scenario_id in selected:
            print(f"{scenario_id}: FAIL: {error}", file=sys.stderr)
        print(f"auth0 tenancy acceptance: FAIL ({len(selected)}/{len(selected)} scenarios)", file=sys.stderr)
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
        print(f"auth0 tenancy acceptance: FAIL ({failures}/{len(selected)} scenarios)", file=sys.stderr)
        return 1
    print(f"auth0 tenancy acceptance: PASS ({len(selected)} scenarios)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
