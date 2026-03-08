#!/usr/bin/env python3
"""Criterion 2: JWT Token Size Scaling Test.

Creates N teams with M roles each, assigns them to a machine user,
and measures the JWT token size at each step. Reports the ceiling
(max teams before hitting 8KB).
"""

import base64
import json
import sys
import time
from pathlib import Path

import requests

SCRIPT_DIR = Path(__file__).resolve().parent
CREDS_FILE = SCRIPT_DIR / ".credentials.json"

ZITADEL_HOST = "http://localhost:8080"

# Realistic set: 8 roles per team
RESOURCES_ACTIONS = [
    ("clusters", "read"),
    ("clusters", "write"),
    ("routes", "read"),
    ("routes", "admin"),
    ("listeners", "read"),
    ("listeners", "write"),
    ("filters", "read"),
    ("schemas", "read"),
]

# Full set: all 32 permutations (8 resources × 4 actions)
FULL_RESOURCES = ["clusters", "routes", "listeners", "filters",
                  "schemas", "secrets", "tools", "dataplanes"]
FULL_ACTIONS = ["read", "write", "admin", "delete"]

# HTTP header limits for reference
LIMITS = {
    "nginx default": 8 * 1024,
    "AWS API Gateway": 10240,
    "AWS ALB": 16 * 1024,
    "Cloudflare": 16 * 1024,
}


def load_creds():
    with open(CREDS_FILE) as f:
        return json.load(f)


def load_pat():
    pat_file = SCRIPT_DIR / "machinekey" / "admin-pat.txt"
    return pat_file.read_text().strip()


def api(pat, method, path, body=None):
    headers = {
        "Authorization": f"Bearer {pat}",
        "Content-Type": "application/json",
    }
    resp = requests.request(method, f"{ZITADEL_HOST}{path}",
                            headers=headers, json=body, timeout=15)
    if resp.status_code >= 400:
        print(f"  API error {resp.status_code}: {resp.text[:200]}", file=sys.stderr)
        return None
    return resp.json()


def get_jwt(creds, project_id):
    scope = f"openid urn:zitadel:iam:org:projects:roles urn:zitadel:iam:org:project:id:{project_id}:aud"
    resp = requests.post(
        creds["token_endpoint"],
        data={"grant_type": "client_credentials", "scope": scope},
        auth=(creds["client_id"], creds["client_secret"]),
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        timeout=10,
    )
    if resp.status_code != 200:
        print(f"Token error: {resp.status_code} {resp.text[:200]}", file=sys.stderr)
        return None
    return resp.json().get("access_token")


def decode_jwt(token):
    payload_b64 = token.split(".")[1]
    padding = 4 - len(payload_b64) % 4
    if padding != 4:
        payload_b64 += "=" * padding
    return json.loads(base64.urlsafe_b64decode(payload_b64))


def measure_jwt(token):
    raw = len(token.encode("utf-8"))
    # Authorization: Bearer <token> header size
    header_size = len(f"Bearer {token}".encode("utf-8"))
    return raw, header_size


def get_existing_roles(pat, project_id):
    """Get all existing role keys for the project."""
    existing = set()
    result = api(pat, "POST",
                 f"/management/v1/projects/{project_id}/roles/_search",
                 {"query": {"limit": 1000}})
    if result and result.get("result"):
        for r in result["result"]:
            existing.add(r["key"])
    return existing


def create_roles_for_teams(pat, project_id, team_names, roles_per_team,
                           existing_roles=None):
    """Create role keys for the given teams (skip existing)."""
    if existing_roles is None:
        existing_roles = get_existing_roles(pat, project_id)

    new_roles = []
    for team in team_names:
        for resource, action in roles_per_team:
            key = f"{team}:{resource}:{action}"
            if key not in existing_roles:
                new_roles.append({
                    "key": key,
                    "displayName": f"{team} {resource} {action}",
                })

    if not new_roles:
        return True

    # Bulk create in batches of 100
    for i in range(0, len(new_roles), 100):
        batch = new_roles[i:i + 100]
        result = api(pat, "POST",
                     f"/management/v1/projects/{project_id}/roles/_bulk",
                     {"roles": batch})
        if result is None:
            return False
        # Track newly created roles
        for r in batch:
            existing_roles.add(r["key"])
    return True


def grant_roles_to_user(pat, user_id, project_id, team_names, roles_per_team):
    """Grant all team roles to the machine user."""
    role_keys = []
    for team in team_names:
        for resource, action in roles_per_team:
            role_keys.append(f"{team}:{resource}:{action}")

    # Update existing grant or create new one
    # First, find existing grants
    search = api(pat, "POST", "/management/v1/users/grants/_search",
                 {"queries": [{"userIdQuery": {"userId": user_id}}]})
    if search and search.get("result"):
        # Update existing grant
        grant_id = search["result"][0]["id"]
        result = api(pat, "PUT",
                     f"/management/v1/users/{user_id}/grants/{grant_id}",
                     {"roleKeys": role_keys})
    else:
        # Create new grant
        result = api(pat, "POST",
                     f"/management/v1/users/{user_id}/grants",
                     {"projectId": project_id, "roleKeys": role_keys})
    return result is not None


def run_scaling_test(mode="realistic"):
    print("=" * 60)
    print(f" Criterion 2: JWT Token Size Scaling Test ({mode})")
    print("=" * 60)
    print()

    creds = load_creds()
    pat = load_pat()
    project_id = creds["project_id"]

    # Get user ID from existing grant
    search = api(pat, "POST", "/management/v1/users/grants/_search",
                 {"queries": [{"projectIdQuery": {"projectId": project_id}}]})
    if not search or not search.get("result"):
        print("FAIL: No user grants found for project", file=sys.stderr)
        sys.exit(1)
    user_id = search["result"][0]["userId"]

    if mode == "realistic":
        roles_per_team = RESOURCES_ACTIONS  # 8 per team
    else:
        roles_per_team = [(r, a) for r in FULL_RESOURCES for a in FULL_ACTIONS]  # 32 per team

    roles_count = len(roles_per_team)
    print(f"Roles per team: {roles_count}")
    print(f"User ID: {user_id}")
    print(f"Project ID: {project_id}")
    print()

    results = []
    team_counts = [1, 3, 5, 8, 10, 15, 20]
    max_teams = max(team_counts)
    all_team_names = [f"team-{i:02d}" for i in range(1, max_teams + 1)]

    # Create all roles upfront to avoid duplicate errors
    print(f"Creating all role keys for {max_teams} teams upfront...", end=" ", flush=True)
    existing_roles = get_existing_roles(pat, project_id)
    if not create_roles_for_teams(pat, project_id, all_team_names, roles_per_team,
                                  existing_roles):
        print("FAIL")
        sys.exit(1)
    print(f"OK ({len(existing_roles)} total roles)")
    print()

    for n_teams in team_counts:
        team_names = all_team_names[:n_teams]
        total_roles = n_teams * roles_count

        print(f"--- {n_teams} teams × {roles_count} roles = {total_roles} grants ---")

        # Grant roles
        print(f"  Granting roles to user...", end=" ", flush=True)
        if not grant_roles_to_user(pat, user_id, project_id, team_names, roles_per_team):
            print("FAIL")
            continue
        print("OK")

        # Brief pause for Zitadel to process
        time.sleep(1)

        # Get JWT and measure
        print(f"  Issuing JWT...", end=" ", flush=True)
        token = get_jwt(creds, project_id)
        if not token:
            print("FAIL")
            continue
        raw_bytes, header_bytes = measure_jwt(token)
        payload = decode_jwt(token)
        role_claim_key = f"urn:zitadel:iam:org:project:{project_id}:roles"
        actual_roles = len(payload.get(role_claim_key, {}))
        print("OK")

        # Check against limits
        status_parts = []
        for name, limit in sorted(LIMITS.items(), key=lambda x: x[1]):
            if header_bytes > limit:
                status_parts.append(f"EXCEEDS {name} ({limit}B)")
            else:
                status_parts.append(f"OK for {name}")

        result = {
            "teams": n_teams,
            "roles_per_team": roles_count,
            "total_grants": total_roles,
            "actual_roles_in_jwt": actual_roles,
            "jwt_raw_bytes": raw_bytes,
            "auth_header_bytes": header_bytes,
        }
        results.append(result)

        print(f"  JWT: {raw_bytes} bytes (raw), {header_bytes} bytes (Authorization header)")
        print(f"  Roles in JWT: {actual_roles}/{total_roles}")
        for s in status_parts:
            print(f"  → {s}")
        print()

    # Summary table
    print()
    print("=" * 60)
    print(" SUMMARY")
    print("=" * 60)
    print()
    print(f"{'Teams':>5} {'Roles':>6} {'Grants':>7} {'JWT (B)':>8} {'Header (B)':>10} {'nginx 8K':>9} {'ALB 16K':>8}")
    print("-" * 60)
    for r in results:
        nginx_ok = "OK" if r["auth_header_bytes"] <= 8192 else "FAIL"
        alb_ok = "OK" if r["auth_header_bytes"] <= 16384 else "FAIL"
        print(f"{r['teams']:>5} {r['roles_per_team']:>6} {r['total_grants']:>7} "
              f"{r['jwt_raw_bytes']:>8} {r['auth_header_bytes']:>10} "
              f"{nginx_ok:>9} {alb_ok:>8}")

    # Find ceiling
    print()
    for name, limit in sorted(LIMITS.items(), key=lambda x: x[1]):
        ceiling = 0
        for r in results:
            if r["auth_header_bytes"] <= limit:
                ceiling = r["teams"]
        print(f"Max teams under {name} ({limit}B): {ceiling}")

    # Save results
    output_file = SCRIPT_DIR / f"scale-results-{mode}.json"
    with open(output_file, "w") as f:
        json.dump({"mode": mode, "roles_per_team": roles_count, "results": results}, f, indent=2)
    print(f"\nResults saved to {output_file}")

    return results


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "realistic"
    if mode not in ("realistic", "full"):
        print(f"Usage: {sys.argv[0]} [realistic|full]", file=sys.stderr)
        sys.exit(1)
    run_scaling_test(mode)
