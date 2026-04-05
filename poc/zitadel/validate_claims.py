#!/usr/bin/env python3
"""Validate JWT role claims from a Zitadel machine user token.

Reads credentials from .credentials.json, obtains a JWT via client_credentials
grant, decodes the payload, and checks that the expected roles are present
(and that unassigned roles are absent).
"""

import base64
import json
import sys
from pathlib import Path

import requests

SCRIPT_DIR = Path(__file__).resolve().parent
CREDS_FILE = SCRIPT_DIR / ".credentials.json"

EXPECTED_ROLES = {
    "backend:clusters:read",
    "backend:clusters:write",
    "sre:routes:admin",
}

MUST_BE_ABSENT = {
    "sre:listeners:write",
    "sre:listeners:read",
    "sre:clusters:read",
    "backend:routes:read",
    "backend:routes:admin",
}


def load_credentials() -> dict:
    if not CREDS_FILE.exists():
        print(f"FAIL: {CREDS_FILE} not found. Run setup.sh first.", file=sys.stderr)
        sys.exit(1)
    with open(CREDS_FILE) as f:
        return json.load(f)


def get_jwt(creds: dict) -> str:
    # urn:zitadel:iam:org:projects:roles (plural) requests role claims
    # urn:zitadel:iam:org:project:id:{id}:aud adds the project to the JWT audience
    project_id = creds["project_id"]
    scope = f"openid urn:zitadel:iam:org:projects:roles urn:zitadel:iam:org:project:id:{project_id}:aud"
    resp = requests.post(
        creds["token_endpoint"],
        data={
            "grant_type": "client_credentials",
            "scope": scope,
        },
        auth=(creds["client_id"], creds["client_secret"]),
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        timeout=10,
    )
    if resp.status_code != 200:
        print(f"FAIL: Token request returned {resp.status_code}", file=sys.stderr)
        print(resp.text, file=sys.stderr)
        sys.exit(1)
    token_data = resp.json()
    access_token = token_data.get("access_token")
    if not access_token:
        print("FAIL: No access_token in response", file=sys.stderr)
        print(json.dumps(token_data, indent=2), file=sys.stderr)
        sys.exit(1)
    return access_token


def decode_jwt_payload(token: str) -> dict:
    parts = token.split(".")
    if len(parts) != 3:
        print(f"FAIL: Token has {len(parts)} parts, expected 3", file=sys.stderr)
        sys.exit(1)
    payload_b64 = parts[1]
    # Add padding if needed
    padding = 4 - len(payload_b64) % 4
    if padding != 4:
        payload_b64 += "=" * padding
    payload_bytes = base64.urlsafe_b64decode(payload_b64)
    return json.loads(payload_bytes)


def validate_claims(payload: dict, project_id: str) -> list[str]:
    """Returns a list of failure messages (empty = all passed)."""
    failures = []

    role_claim_key = f"urn:zitadel:iam:org:project:{project_id}:roles"

    if role_claim_key not in payload:
        failures.append(f"Role claim key missing: {role_claim_key}")
        return failures

    roles_obj = payload[role_claim_key]
    present_roles = set(roles_obj.keys())

    # Check expected roles are present
    for role in sorted(EXPECTED_ROLES):
        if role in present_roles:
            print(f"  [PASS] {role} — present")
        else:
            msg = f"{role} — expected but MISSING"
            print(f"  [FAIL] {msg}")
            failures.append(msg)

    # Check that unassigned roles are absent
    for role in sorted(MUST_BE_ABSENT):
        if role not in present_roles:
            print(f"  [PASS] {role} — correctly absent")
        else:
            msg = f"{role} — should be ABSENT but found in token"
            print(f"  [FAIL] {msg}")
            failures.append(msg)

    return failures


def main() -> None:
    print("=============================================")
    print(" Zitadel JWT Role Claims Validator")
    print("=============================================")
    print()

    # Load credentials
    creds = load_credentials()
    project_id = creds["project_id"]
    print(f"Project ID: {project_id}")
    print(f"Token endpoint: {creds['token_endpoint']}")
    print()

    # Get JWT
    print("Requesting JWT via client_credentials grant...")
    token = get_jwt(creds)
    print("Token received.")
    print()

    # Size report
    raw_bytes = len(token.encode("utf-8"))
    b64_bytes = len(base64.b64encode(token.encode("utf-8")))
    print(f"JWT size: {raw_bytes} bytes (raw), {b64_bytes} bytes (base64-encoded)")
    print()

    # Decode
    print("Decoded JWT payload:")
    print("-" * 45)
    payload = decode_jwt_payload(token)
    print(json.dumps(payload, indent=2))
    print("-" * 45)
    print()

    # Validate
    role_claim_key = f"urn:zitadel:iam:org:project:{project_id}:roles"
    print(f"Role claim key: {role_claim_key}")
    print()
    print("Checking role claims:")
    failures = validate_claims(payload, project_id)
    print()

    # Summary
    print("=============================================")
    if failures:
        print(f" FAIL — {len(failures)} check(s) failed:")
        for f in failures:
            print(f"   - {f}")
        print("=============================================")
        sys.exit(1)
    else:
        total = len(EXPECTED_ROLES) + len(MUST_BE_ABSENT)
        print(f" PASS — all {total} checks passed")
        print("=============================================")


if __name__ == "__main__":
    main()
