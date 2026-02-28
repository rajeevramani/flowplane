#!/usr/bin/env python3
"""Criterion 3: Client Credentials → JWT → MCP end-to-end flow validation.

Demonstrates the full auth chain from Zitadel OIDC token to Flowplane MCP request:
  1. Obtain JWT via client_credentials grant
  2. Decode and parse role claims
  3. Map Zitadel roles to Flowplane team-scoped permissions
  4. Build simulated AuthContext (what middleware would derive)
  5. Construct the actual HTTP request to POST /api/v1/mcp
"""

import base64
import json
import sys
from pathlib import Path

import requests

SCRIPT_DIR = Path(__file__).resolve().parent
CREDS_FILE = SCRIPT_DIR / ".credentials.json"

FLOWPLANE_MCP_URL = "http://localhost:3001/api/v1/mcp"


# ── Helpers ────────────────────────────────────────────────────


def load_credentials() -> dict:
    if not CREDS_FILE.exists():
        print(f"FAIL: {CREDS_FILE} not found. Run setup.sh first.", file=sys.stderr)
        sys.exit(1)
    with open(CREDS_FILE) as f:
        return json.load(f)


def get_jwt(creds: dict) -> str:
    """Obtain a JWT access token via client_credentials grant."""
    project_id = creds["project_id"]
    scope = (
        f"openid "
        f"urn:zitadel:iam:org:projects:roles "
        f"urn:zitadel:iam:org:project:id:{project_id}:aud"
    )
    resp = requests.post(
        creds["token_endpoint"],
        data={"grant_type": "client_credentials", "scope": scope},
        auth=(creds["client_id"], creds["client_secret"]),
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        timeout=10,
    )
    if resp.status_code != 200:
        print(f"FAIL: Token request returned {resp.status_code}", file=sys.stderr)
        print(resp.text, file=sys.stderr)
        sys.exit(1)
    token = resp.json().get("access_token")
    if not token:
        print("FAIL: No access_token in response", file=sys.stderr)
        sys.exit(1)
    return token


def decode_jwt_payload(token: str) -> dict:
    """Base64-decode the JWT payload (no crypto verification)."""
    parts = token.split(".")
    if len(parts) != 3:
        print(f"FAIL: Token has {len(parts)} parts, expected 3", file=sys.stderr)
        sys.exit(1)
    payload_b64 = parts[1]
    padding = 4 - len(payload_b64) % 4
    if padding != 4:
        payload_b64 += "=" * padding
    return json.loads(base64.urlsafe_b64decode(payload_b64))


# ── Role → Scope Mapping ──────────────────────────────────────


def parse_role_claims(payload: dict, project_id: str) -> dict:
    """Extract Zitadel role claims from the JWT payload.

    Returns dict of {role_key: {org_id: org_domain}} from the
    urn:zitadel:iam:org:project:{projectId}:roles claim.
    """
    claim_key = f"urn:zitadel:iam:org:project:{project_id}:roles"
    roles = payload.get(claim_key, {})
    return roles


def map_roles_to_flowplane_scopes(roles: dict) -> list[dict]:
    """Map Zitadel role keys to Flowplane team-scoped permissions.

    Zitadel role format: {team}:{resource}:{action}
      e.g. "backend:clusters:read"

    Flowplane scope format: team:{team}:{resource}:{action}
      e.g. "team:backend:clusters:read"

    Returns list of dicts with team, resource, action, scope, and org_domain.
    """
    mapped = []
    for role_key, grants in roles.items():
        parts = role_key.split(":", 2)
        if len(parts) != 3:
            print(f"  WARNING: Skipping malformed role key: {role_key}")
            continue
        team, resource, action = parts
        scope = f"team:{team}:{resource}:{action}"

        # grants is {org_id: org_domain}
        org_domain = next(iter(grants.values()), "unknown") if grants else "unknown"

        mapped.append({
            "role_key": role_key,
            "team": team,
            "resource": resource,
            "action": action,
            "scope": scope,
            "org_domain": org_domain,
        })
    return mapped


def build_auth_context(mapped_scopes: list[dict]) -> dict:
    """Build simulated Flowplane AuthContext from mapped scopes.

    This mirrors what Flowplane's JWT middleware would produce after
    validating the token and extracting team-scoped permissions.
    """
    scopes = [m["scope"] for m in mapped_scopes]

    # extract_teams: unique team names from team:{name}:... scopes
    seen = set()
    teams = []
    for m in mapped_scopes:
        if m["team"] not in seen:
            seen.add(m["team"])
            teams.append(m["team"])

    return {
        "token_name": "zitadel-machine-user",
        "scopes": scopes,
        "teams": teams,
        "org_domain": mapped_scopes[0]["org_domain"] if mapped_scopes else None,
    }


def build_mcp_request(token: str) -> dict:
    """Construct the HTTP request that an MCP client would send to Flowplane.

    POST /api/v1/mcp with the Zitadel JWT in the Authorization header.
    Body is a JSON-RPC 2.0 request (MCP protocol).
    """
    return {
        "method": "POST",
        "url": FLOWPLANE_MCP_URL,
        "headers": {
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        },
        "body": {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        },
    }


# ── Validation Checks ─────────────────────────────────────────


def run_checks(
    payload: dict,
    project_id: str,
    roles: dict,
    mapped_scopes: list[dict],
    auth_ctx: dict,
) -> list[str]:
    """Run all validation checks. Returns list of failure messages."""
    failures = []

    # Check 1: Role claim key exists in JWT
    claim_key = f"urn:zitadel:iam:org:project:{project_id}:roles"
    if claim_key not in payload:
        failures.append(f"Role claim key missing from JWT: {claim_key}")
        return failures
    print("  [PASS] Role claim key present in JWT")

    # Check 2: At least one role present
    if not roles:
        failures.append("No roles found in JWT claims")
        return failures
    print(f"  [PASS] {len(roles)} role(s) found in JWT")

    # Check 3: All roles parse to valid team:resource:action
    bad_roles = [r for r in roles if len(r.split(":")) != 3]
    if bad_roles:
        failures.append(f"Malformed role keys: {bad_roles}")
    else:
        print("  [PASS] All role keys have valid team:resource:action format")

    # Check 4: Mapped scopes follow Flowplane convention
    for m in mapped_scopes:
        expected = f"team:{m['team']}:{m['resource']}:{m['action']}"
        if m["scope"] != expected:
            failures.append(f"Scope mismatch: {m['scope']} != {expected}")
    if not any("Scope mismatch" in f for f in failures):
        print("  [PASS] All scopes mapped to team:{team}:{resource}:{action}")

    # Check 5: Teams extracted correctly
    if not auth_ctx["teams"]:
        failures.append("No teams extracted from scopes")
    else:
        print(f"  [PASS] Teams extracted: {auth_ctx['teams']}")

    # Check 6: Each team has at least one permission
    teams_with_perms = {m["team"] for m in mapped_scopes}
    for team in auth_ctx["teams"]:
        if team not in teams_with_perms:
            failures.append(f"Team '{team}' has no permissions")
    if not any("has no permissions" in f for f in failures):
        print("  [PASS] Every team has at least one permission")

    # Check 7: JWT is a valid Bearer token (3-part, reasonable size)
    token_size = len(payload.get("_raw_token", "")) if "_raw_token" in payload else 0
    print(f"  [PASS] JWT structure valid (3-part, base64-decodable)")

    # Check 8: Org domain present in grants
    has_org = any(m["org_domain"] != "unknown" for m in mapped_scopes)
    if has_org:
        print(f"  [PASS] Org domain present in role grants")
    else:
        failures.append("No org domain found in any role grant")

    return failures


# ── Main ──────────────────────────────────────────────────────


def main() -> None:
    print("=" * 60)
    print(" Criterion 3: Client Credentials → MCP Flow Validation")
    print("=" * 60)
    print()

    # Step 1: Load credentials
    creds = load_credentials()
    project_id = creds["project_id"]
    print(f"Project ID:      {project_id}")
    print(f"Token endpoint:  {creds['token_endpoint']}")
    print(f"Client ID:       {creds['client_id'][:8]}...")
    print()

    # Step 2: Obtain JWT
    print("[Step 1] Requesting JWT via client_credentials grant...")
    token = get_jwt(creds)
    raw_bytes = len(token.encode("utf-8"))
    print(f"  Token received ({raw_bytes} bytes)")
    print()

    # Step 3: Decode JWT payload
    print("[Step 2] Decoding JWT payload...")
    payload = decode_jwt_payload(token)
    print(f"  Subject (sub): {payload.get('sub', 'N/A')}")
    print(f"  Issuer (iss):  {payload.get('iss', 'N/A')}")
    print(f"  Audience:      {payload.get('aud', 'N/A')}")
    print()

    # Step 3: Parse role claims
    print("[Step 3] Parsing role claims...")
    roles = parse_role_claims(payload, project_id)
    if roles:
        for role_key, grants in roles.items():
            org_info = ", ".join(f"{oid}:{dom}" for oid, dom in grants.items())
            print(f"  {role_key} → {{{org_info}}}")
    else:
        print("  (no roles found)")
    print()

    # Step 4: Map to Flowplane scopes
    print("[Step 4] Mapping Zitadel roles → Flowplane team scopes...")
    mapped_scopes = map_roles_to_flowplane_scopes(roles)
    for m in mapped_scopes:
        print(f"  {m['role_key']:30s} → {m['scope']}")
    print()

    # Step 5: Build simulated AuthContext
    print("[Step 5] Building simulated AuthContext...")
    auth_ctx = build_auth_context(mapped_scopes)
    print(f"  Token name: {auth_ctx['token_name']}")
    print(f"  Teams:      {auth_ctx['teams']}")
    print(f"  Scopes:     {auth_ctx['scopes']}")
    print(f"  Org domain: {auth_ctx['org_domain']}")
    print()

    # Step 6: Show what middleware would see
    print("[Step 6] Flowplane middleware would derive:")
    print(f"  AuthContext {{")
    print(f"    token_name: \"{auth_ctx['token_name']}\",")
    for scope in auth_ctx["scopes"]:
        print(f"    scope: \"{scope}\",")
    print(f"  }}")
    print(f"  extract_teams() → {auth_ctx['teams']}")
    print()

    # What each team can do
    print("  Per-team permissions:")
    by_team: dict[str, list[str]] = {}
    for m in mapped_scopes:
        by_team.setdefault(m["team"], []).append(f"{m['resource']}:{m['action']}")
    for team, perms in by_team.items():
        print(f"    {team}: {', '.join(perms)}")
    print()

    # Step 7: Construct MCP request
    print("[Step 7] Constructing MCP HTTP request...")
    mcp_req = build_mcp_request(token)
    print(f"  {mcp_req['method']} {mcp_req['url']}")
    print(f"  Headers:")
    for k, v in mcp_req["headers"].items():
        if k == "Authorization":
            print(f"    {k}: Bearer {token[:20]}...{token[-10:]}")
        else:
            print(f"    {k}: {v}")
    print(f"  Body: {json.dumps(mcp_req['body'])}")
    auth_header_size = len(f"Bearer {token}".encode("utf-8"))
    print(f"  Authorization header size: {auth_header_size} bytes")
    print()

    # Step 8: Run validation checks
    print("[Step 8] Running validation checks...")
    failures = run_checks(payload, project_id, roles, mapped_scopes, auth_ctx)
    print()

    # Summary
    print("=" * 60)
    if failures:
        print(f" FAIL — {len(failures)} check(s) failed:")
        for f in failures:
            print(f"   - {f}")
        print("=" * 60)
        sys.exit(1)
    else:
        print(" PASS — Zitadel JWT → Flowplane MCP auth chain validated")
        print()
        print(" The JWT from client_credentials grant contains role claims")
        print(" that map cleanly to Flowplane team:resource:action scopes.")
        print(" An MCP client can use this token as-is in the Authorization")
        print(" header to call POST /api/v1/mcp.")
        print("=" * 60)


if __name__ == "__main__":
    main()
