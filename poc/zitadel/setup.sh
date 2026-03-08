#!/usr/bin/env bash
set -euo pipefail

# ──────────────────────────────────────────────────────────────
# Zitadel PoC Setup Script
#
# Sets up a Zitadel project for Flowplane PoC:
#   1. Creates a "Flowplane" project with role assertion enabled
#   2. Adds RBAC role keys (backend:*, sre:*)
#   3. Creates a machine user with JWT access token type
#   4. Generates client credentials
#   5. Grants a subset of roles to the machine user
#   6. Saves credentials to .credentials.json
#
# Prerequisites: curl, jq
# Usage: ZITADEL_PAT=<admin-pat> ./setup.sh
# ──────────────────────────────────────────────────────────────

ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8080}"
ZITADEL_PAT="${ZITADEL_PAT:?Error: ZITADEL_PAT must be set to an admin Personal Access Token}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CREDS_FILE="${SCRIPT_DIR}/.credentials.json"

echo "============================================="
echo " Flowplane Zitadel PoC Setup"
echo "============================================="
echo "Host: ${ZITADEL_HOST}"
echo ""

api() {
  local method="$1" path="$2" body="${3:-}"
  local args=(
    -s -f
    -X "${method}"
    -H "Authorization: Bearer ${ZITADEL_PAT}"
    -H "Content-Type: application/json"
  )
  if [ -n "${body}" ]; then
    args+=(-d "${body}")
  fi
  curl "${args[@]}" "${ZITADEL_HOST}${path}"
}

# ── Step 1: Create Project ──────────────────────────────────
echo "[1/6] Creating project 'Flowplane'..."
project_resp=$(api POST /management/v1/projects \
  '{"name": "Flowplane", "projectRoleAssertion": true}')
project_id=$(echo "${project_resp}" | jq -r '.id')
if [ -z "${project_id}" ] || [ "${project_id}" = "null" ]; then
  echo "FAIL: Could not create project. Response:" >&2
  echo "${project_resp}" >&2
  exit 1
fi
echo "  Project ID: ${project_id}"

# ── Step 2: Add Roles ───────────────────────────────────────
echo "[2/6] Adding role keys..."
api POST "/management/v1/projects/${project_id}/roles/_bulk" '{
  "roles": [
    {"key": "backend:clusters:read",  "displayName": "Backend Clusters Read"},
    {"key": "backend:clusters:write", "displayName": "Backend Clusters Write"},
    {"key": "backend:routes:read",    "displayName": "Backend Routes Read"},
    {"key": "backend:routes:admin",   "displayName": "Backend Routes Admin"},
    {"key": "sre:clusters:read",      "displayName": "SRE Clusters Read"},
    {"key": "sre:routes:admin",       "displayName": "SRE Routes Admin"},
    {"key": "sre:listeners:read",     "displayName": "SRE Listeners Read"},
    {"key": "sre:listeners:write",    "displayName": "SRE Listeners Write"}
  ]
}' > /dev/null
echo "  Added 8 roles"

# ── Step 3: Create Machine User ─────────────────────────────
echo "[3/6] Creating machine user 'flowplane-poc-bot'..."
user_resp=$(api POST /management/v1/users/machine '{
  "userName": "flowplane-poc-bot",
  "name": "PoC Bot",
  "accessTokenType": 1
}')
user_id=$(echo "${user_resp}" | jq -r '.userId')
if [ -z "${user_id}" ] || [ "${user_id}" = "null" ]; then
  echo "FAIL: Could not create machine user. Response:" >&2
  echo "${user_resp}" >&2
  exit 1
fi
echo "  User ID: ${user_id}"

# ── Step 4: Generate Client Secret ──────────────────────────
echo "[4/6] Generating client credentials..."
secret_resp=$(api PUT "/management/v1/users/${user_id}/secret")
client_id=$(echo "${secret_resp}" | jq -r '.clientId')
client_secret=$(echo "${secret_resp}" | jq -r '.clientSecret')
if [ -z "${client_id}" ] || [ "${client_id}" = "null" ]; then
  echo "FAIL: Could not generate secret. Response:" >&2
  echo "${secret_resp}" >&2
  exit 1
fi
echo "  Client ID: ${client_id}"

# ── Step 5: Grant Roles to User ─────────────────────────────
echo "[5/6] Granting roles to machine user..."
grant_resp=$(api POST "/management/v1/users/${user_id}/grants" "{
  \"projectId\": \"${project_id}\",
  \"roleKeys\": [\"backend:clusters:read\", \"backend:clusters:write\", \"sre:routes:admin\"]
}")
grant_id=$(echo "${grant_resp}" | jq -r '.userGrantId')
if [ -z "${grant_id}" ] || [ "${grant_id}" = "null" ]; then
  echo "FAIL: Could not create user grant. Response:" >&2
  echo "${grant_resp}" >&2
  exit 1
fi
echo "  Grant ID: ${grant_id}"
echo "  Roles: backend:clusters:read, backend:clusters:write, sre:routes:admin"

# ── Step 6: Save Credentials ────────────────────────────────
echo "[6/6] Saving credentials to .credentials.json..."
jq -n \
  --arg cid "${client_id}" \
  --arg cs  "${client_secret}" \
  --arg te  "${ZITADEL_HOST}/oauth/v2/token" \
  --arg pid "${project_id}" \
  '{client_id: $cid, client_secret: $cs, token_endpoint: $te, project_id: $pid}' \
  > "${CREDS_FILE}"
chmod 600 "${CREDS_FILE}"
echo "  Saved to ${CREDS_FILE}"

echo ""
echo "============================================="
echo " Setup complete!"
echo "============================================="
echo "Next: python3 validate_claims.py"
