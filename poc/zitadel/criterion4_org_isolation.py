#!/usr/bin/env python3
"""Criterion 4: Multi-Organization Tenant Isolation.

Creates two Zitadel organizations (Acme Corp, Beta Inc), grants each a
subset of project roles, creates machine users within each org, and
validates that JWTs are fully isolated — no cross-org role leakage.
"""

import base64
import json
import sys
import time
from pathlib import Path

import requests

SCRIPT_DIR = Path(__file__).resolve().parent
CREDS_FILE = SCRIPT_DIR / ".credentials.json"
PAT_FILE = SCRIPT_DIR / "machinekey" / "admin-pat.txt"

ZITADEL_HOST = "http://localhost:8080"

# Roles pre-created by scale_test.py
ACME_ROLES = [
    "team-01:clusters:read",
    "team-01:clusters:write",
    "team-01:routes:read",
    "team-01:routes:admin",
    "team-01:listeners:read",
    "team-01:listeners:write",
    "team-01:filters:read",
    "team-01:schemas:read",
]

BETA_ROLES = [
    "team-02:clusters:read",
    "team-02:clusters:write",
    "team-02:routes:read",
    "team-02:routes:admin",
    "team-02:listeners:read",
    "team-02:listeners:write",
    "team-02:filters:read",
    "team-02:schemas:read",
]


def load_creds():
    if not CREDS_FILE.exists():
        print(f"FAIL: {CREDS_FILE} not found. Run setup.sh first.", file=sys.stderr)
        sys.exit(1)
    with open(CREDS_FILE) as f:
        return json.load(f)


def load_pat():
    if not PAT_FILE.exists():
        print(f"FAIL: {PAT_FILE} not found. Run setup.sh first.", file=sys.stderr)
        sys.exit(1)
    return PAT_FILE.read_text().strip()


def api(pat, method, path, body=None, org_id=None):
    """Call Zitadel API. Optional org_id scopes the call to that org."""
    headers = {
        "Authorization": f"Bearer {pat}",
        "Content-Type": "application/json",
    }
    if org_id:
        headers["x-zitadel-orgid"] = org_id
    resp = requests.request(
        method, f"{ZITADEL_HOST}{path}",
        headers=headers, json=body, timeout=15,
    )
    if resp.status_code >= 400:
        print(f"  API error {resp.status_code} {method} {path}: "
              f"{resp.text[:300]}", file=sys.stderr)
        return None
    return resp.json()


def decode_jwt(token):
    parts = token.split(".")
    if len(parts) != 3:
        print(f"FAIL: Token has {len(parts)} parts, expected 3", file=sys.stderr)
        sys.exit(1)
    payload_b64 = parts[1]
    padding = 4 - len(payload_b64) % 4
    if padding != 4:
        payload_b64 += "=" * padding
    return json.loads(base64.urlsafe_b64decode(payload_b64))


def create_org(pat, name):
    """Create a new Zitadel org via the v2 API."""
    result = api(pat, "POST", "/v2/organizations", {"name": name})
    if not result:
        print(f"FAIL: Could not create org '{name}'", file=sys.stderr)
        sys.exit(1)
    org_id = result.get("organizationId")
    if not org_id:
        print(f"FAIL: No 'organizationId' in response: {result}", file=sys.stderr)
        sys.exit(1)
    return org_id


def grant_project_to_org(pat, project_id, org_id, role_keys):
    """Grant the project to an org with specific role keys."""
    result = api(pat, "POST",
                 f"/management/v1/projects/{project_id}/grants",
                 {"grantedOrgId": org_id, "roleKeys": role_keys})
    if not result:
        print(f"FAIL: Could not grant project to org {org_id}", file=sys.stderr)
        sys.exit(1)
    grant_id = result.get("grantId")
    if not grant_id:
        print(f"FAIL: No 'grantId' in response: {result}", file=sys.stderr)
        sys.exit(1)
    return grant_id


def create_machine_user(pat, org_id, username, display_name):
    """Create a machine user within an org and return (user_id, client_id, client_secret)."""
    # Create user
    result = api(pat, "POST", "/management/v1/users/machine",
                 {"userName": username, "name": display_name,
                  "accessTokenType": 1},
                 org_id=org_id)
    if not result:
        print(f"FAIL: Could not create machine user '{username}' in org {org_id}",
              file=sys.stderr)
        sys.exit(1)
    user_id = result.get("userId")
    if not user_id:
        print(f"FAIL: No 'userId' in response: {result}", file=sys.stderr)
        sys.exit(1)

    # Generate client secret
    secret_result = api(pat, "PUT",
                        f"/management/v1/users/{user_id}/secret",
                        {}, org_id=org_id)
    if not secret_result:
        print(f"FAIL: Could not generate secret for user {user_id}",
              file=sys.stderr)
        sys.exit(1)
    client_id = secret_result.get("clientId")
    client_secret = secret_result.get("clientSecret")
    if not client_id or not client_secret:
        print(f"FAIL: Missing clientId/clientSecret: {secret_result}",
              file=sys.stderr)
        sys.exit(1)

    return user_id, client_id, client_secret


def grant_roles_to_user(pat, org_id, user_id, project_id, grant_id, role_keys):
    """Grant project roles to a user within their org."""
    result = api(pat, "POST",
                 f"/management/v1/users/{user_id}/grants",
                 {"projectId": project_id, "projectGrantId": grant_id,
                  "roleKeys": role_keys},
                 org_id=org_id)
    if not result:
        print(f"FAIL: Could not grant roles to user {user_id} in org {org_id}",
              file=sys.stderr)
        sys.exit(1)
    return result


def get_jwt(client_id, client_secret, project_id):
    """Issue a JWT via client_credentials grant."""
    scope = (f"openid urn:zitadel:iam:org:projects:roles "
             f"urn:zitadel:iam:org:project:id:{project_id}:aud")
    resp = requests.post(
        f"{ZITADEL_HOST}/oauth/v2/token",
        data={"grant_type": "client_credentials", "scope": scope},
        auth=(client_id, client_secret),
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        timeout=10,
    )
    if resp.status_code != 200:
        print(f"FAIL: Token request returned {resp.status_code}: "
              f"{resp.text[:300]}", file=sys.stderr)
        return None
    return resp.json().get("access_token")


def validate_jwt(label, token, project_id, expected_org_id,
                 forbidden_org_id, expected_roles, forbidden_roles):
    """Validate a JWT and return list of (check_name, passed, detail)."""
    checks = []
    payload = decode_jwt(token)
    role_claim_key = f"urn:zitadel:iam:org:project:{project_id}:roles"

    # Check 1: Correct org ID in token
    # Zitadel puts org ID in urn:zitadel:iam:org:id claim or sub claim metadata
    # The key structure in role claims has org IDs as values
    token_roles = payload.get(role_claim_key, {})
    role_keys = set(token_roles.keys())

    # Check org ID presence — role values contain {orgId: domainName}
    org_ids_in_token = set()
    for role_key, role_value in token_roles.items():
        if isinstance(role_value, dict):
            org_ids_in_token.update(role_value.keys())

    has_own_org = expected_org_id in org_ids_in_token
    checks.append((
        f"{label}: JWT contains own org ID ({expected_org_id})",
        has_own_org,
        f"Org IDs in token: {org_ids_in_token}" if not has_own_org else "present"
    ))

    has_foreign_org = forbidden_org_id in org_ids_in_token
    checks.append((
        f"{label}: JWT does NOT contain other org ID ({forbidden_org_id})",
        not has_foreign_org,
        f"LEAKED: {forbidden_org_id} found in token" if has_foreign_org else "absent"
    ))

    # Check expected roles present
    missing = set(expected_roles) - role_keys
    checks.append((
        f"{label}: JWT contains all expected roles ({len(expected_roles)})",
        len(missing) == 0,
        f"Missing: {missing}" if missing else f"all {len(expected_roles)} present"
    ))

    # Check forbidden roles absent
    leaked = set(forbidden_roles) & role_keys
    checks.append((
        f"{label}: JWT contains ZERO forbidden roles",
        len(leaked) == 0,
        f"LEAKED roles: {leaked}" if leaked else "none found"
    ))

    return checks, payload


def main():
    print("=" * 60)
    print(" Criterion 4: Multi-Org Tenant Isolation Validation")
    print("=" * 60)
    print()

    creds = load_creds()
    pat = load_pat()
    project_id = creds["project_id"]
    print(f"Project ID: {project_id}")
    print()

    # ── Step 1: Create two organizations ──────────────────────
    print("Step 1: Creating organizations...")
    ts = int(time.time())
    acme_org_id = create_org(pat, f"Acme Corp {ts}")
    print(f"  Acme Corp org ID: {acme_org_id}")
    beta_org_id = create_org(pat, f"Beta Inc {ts}")
    print(f"  Beta Inc org ID:  {beta_org_id}")
    print()

    # ── Step 2: Grant project to each org ─────────────────────
    print("Step 2: Granting project to each org...")
    acme_grant_id = grant_project_to_org(pat, project_id, acme_org_id, ACME_ROLES)
    print(f"  Acme project grant ID: {acme_grant_id}")
    beta_grant_id = grant_project_to_org(pat, project_id, beta_org_id, BETA_ROLES)
    print(f"  Beta project grant ID: {beta_grant_id}")
    print()

    # ── Step 3: Create machine users in each org ──────────────
    print("Step 3: Creating machine users...")
    acme_user_id, acme_client_id, acme_client_secret = create_machine_user(
        pat, acme_org_id, f"acme-bot-{ts}", "Acme Bot")
    print(f"  Acme user: {acme_user_id} (client: {acme_client_id})")

    beta_user_id, beta_client_id, beta_client_secret = create_machine_user(
        pat, beta_org_id, f"beta-bot-{ts}", "Beta Bot")
    print(f"  Beta user: {beta_user_id} (client: {beta_client_id})")
    print()

    # ── Step 4: Grant roles to each user ──────────────────────
    print("Step 4: Granting roles to users...")
    grant_roles_to_user(pat, acme_org_id, acme_user_id, project_id,
                        acme_grant_id, ACME_ROLES)
    print(f"  Acme user granted {len(ACME_ROLES)} team-01 roles")
    grant_roles_to_user(pat, beta_org_id, beta_user_id, project_id,
                        beta_grant_id, BETA_ROLES)
    print(f"  Beta user granted {len(BETA_ROLES)} team-02 roles")
    print()

    # Brief pause for Zitadel to process grants
    print("Waiting for grants to propagate...", end=" ", flush=True)
    time.sleep(2)
    print("OK")
    print()

    # ── Step 5: Issue JWTs ────────────────────────────────────
    print("Step 5: Issuing JWTs...")
    acme_token = get_jwt(acme_client_id, acme_client_secret, project_id)
    if not acme_token:
        print("FAIL: Could not get Acme JWT", file=sys.stderr)
        sys.exit(1)
    print(f"  Acme JWT: {len(acme_token)} bytes")

    beta_token = get_jwt(beta_client_id, beta_client_secret, project_id)
    if not beta_token:
        print("FAIL: Could not get Beta JWT", file=sys.stderr)
        sys.exit(1)
    print(f"  Beta JWT: {len(beta_token)} bytes")
    print()

    # ── Step 6: Validate isolation ────────────────────────────
    print("Step 6: Validating tenant isolation...")
    print()

    all_checks = []

    acme_checks, acme_payload = validate_jwt(
        "Acme", acme_token, project_id,
        expected_org_id=acme_org_id,
        forbidden_org_id=beta_org_id,
        expected_roles=ACME_ROLES,
        forbidden_roles=BETA_ROLES,
    )
    all_checks.extend(acme_checks)

    beta_checks, beta_payload = validate_jwt(
        "Beta", beta_token, project_id,
        expected_org_id=beta_org_id,
        forbidden_org_id=acme_org_id,
        expected_roles=BETA_ROLES,
        forbidden_roles=ACME_ROLES,
    )
    all_checks.extend(beta_checks)

    # Print results
    failures = 0
    for name, passed, detail in all_checks:
        status = "PASS" if passed else "FAIL"
        if not passed:
            failures += 1
        print(f"  [{status}] {name}")
        print(f"         {detail}")

    print()

    # ── Decoded payloads for inspection ───────────────────────
    print("-" * 60)
    print("Acme JWT payload (decoded):")
    print(json.dumps(acme_payload, indent=2))
    print()
    print("Beta JWT payload (decoded):")
    print(json.dumps(beta_payload, indent=2))
    print("-" * 60)
    print()

    # ── Summary ───────────────────────────────────────────────
    total = len(all_checks)
    print("=" * 60)
    if failures:
        print(f" FAIL: {failures}/{total} checks failed")
    else:
        print(f" PASS: all {total} isolation checks passed")
    print("=" * 60)

    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
