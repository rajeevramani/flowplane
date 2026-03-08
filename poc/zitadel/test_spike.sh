#!/usr/bin/env bash
set -euo pipefail

# ──────────────────────────────────────────────────────────────
# Zitadel Auth Spike Verification
#
# Tests that Flowplane endpoints under /api/v1/zitadel/ accept
# Zitadel JWTs and return valid responses.
#
# Prerequisites:
#   - Zitadel running on localhost:8080
#   - Flowplane running on localhost:3001 with FLOWPLANE_ZITADEL_* vars
#   - .credentials.json from setup.sh
#
# Usage: ./test_spike.sh
# ──────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CREDS_FILE="${SCRIPT_DIR}/.credentials.json"
FLOWPLANE_HOST="${FLOWPLANE_HOST:-http://localhost:3001}"
ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8080}"

if [ ! -f "${CREDS_FILE}" ]; then
  echo "ERROR: ${CREDS_FILE} not found. Run setup.sh first." >&2
  exit 1
fi

CLIENT_ID=$(jq -r .client_id "${CREDS_FILE}")
CLIENT_SECRET=$(jq -r .client_secret "${CREDS_FILE}")
PROJECT_ID=$(jq -r .project_id "${CREDS_FILE}")

echo "============================================="
echo " Zitadel Auth Spike — Verification"
echo "============================================="
echo "Zitadel:   ${ZITADEL_HOST}"
echo "Flowplane: ${FLOWPLANE_HOST}"
echo "Project:   ${PROJECT_ID}"
echo ""

# ── Step 0: Health checks ─────────────────────────────────────
echo "[0/5] Health checks..."
curl -sf "${ZITADEL_HOST}/debug/healthz" > /dev/null || { echo "FAIL: Zitadel not reachable"; exit 1; }
curl -sf "${FLOWPLANE_HOST}/health" > /dev/null || { echo "FAIL: Flowplane not reachable"; exit 1; }
echo "  Both services healthy"

# ── Step 1: Get JWT ───────────────────────────────────────────
echo "[1/5] Getting Zitadel JWT..."
TOKEN_RESP=$(curl -s -X POST "${ZITADEL_HOST}/oauth/v2/token" \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=client_credentials&scope=openid urn:zitadel:iam:org:projects:roles urn:zitadel:iam:org:project:id:${PROJECT_ID}:aud" \
  -u "${CLIENT_ID}:${CLIENT_SECRET}")

TOKEN=$(echo "${TOKEN_RESP}" | jq -r '.access_token')
if [ -z "${TOKEN}" ] || [ "${TOKEN}" = "null" ]; then
  echo "FAIL: Could not get JWT. Response:" >&2
  echo "${TOKEN_RESP}" >&2
  exit 1
fi
echo "  JWT obtained (${#TOKEN} chars)"

PASS=0
FAIL=0

check() {
  local name="$1" expected_status="$2" actual_status="$3" body="$4"
  if [ "${actual_status}" = "${expected_status}" ]; then
    echo "  PASS: ${name} (${actual_status})"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: ${name} — expected ${expected_status}, got ${actual_status}"
    echo "        body: $(echo "${body}" | head -c 200)"
    FAIL=$((FAIL + 1))
  fi
}

# ── Step 2: Test MCP tools/list ───────────────────────────────
echo "[2/5] Testing MCP tools/list..."
MCP_RESP=$(curl -s -w "\n%{http_code}" -X POST "${FLOWPLANE_HOST}/api/v1/zitadel/mcp" \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}')
MCP_STATUS=$(echo "${MCP_RESP}" | tail -1)
MCP_BODY=$(echo "${MCP_RESP}" | sed '$d')
check "POST /api/v1/zitadel/mcp (tools/list)" "200" "${MCP_STATUS}" "${MCP_BODY}"

# ── Step 3: Test REST clusters ────────────────────────────────
echo "[3/5] Testing REST clusters..."
CL_RESP=$(curl -s -w "\n%{http_code}" "${FLOWPLANE_HOST}/api/v1/zitadel/teams/team-01/clusters" \
  -H "Authorization: Bearer ${TOKEN}")
CL_STATUS=$(echo "${CL_RESP}" | tail -1)
CL_BODY=$(echo "${CL_RESP}" | sed '$d')
check "GET /api/v1/zitadel/teams/team-01/clusters" "200" "${CL_STATUS}" "${CL_BODY}"

# ── Step 4: Test REST route-configs ───────────────────────────
echo "[4/5] Testing REST route-configs..."
RC_RESP=$(curl -s -w "\n%{http_code}" "${FLOWPLANE_HOST}/api/v1/zitadel/teams/team-01/route-configs" \
  -H "Authorization: Bearer ${TOKEN}")
RC_STATUS=$(echo "${RC_RESP}" | tail -1)
RC_BODY=$(echo "${RC_RESP}" | sed '$d')
check "GET /api/v1/zitadel/teams/team-01/route-configs" "200" "${RC_STATUS}" "${RC_BODY}"

# ── Step 5: Test REST listeners ───────────────────────────────
echo "[5/5] Testing REST listeners..."
LN_RESP=$(curl -s -w "\n%{http_code}" "${FLOWPLANE_HOST}/api/v1/zitadel/teams/team-01/listeners" \
  -H "Authorization: Bearer ${TOKEN}")
LN_STATUS=$(echo "${LN_RESP}" | tail -1)
LN_BODY=$(echo "${LN_RESP}" | sed '$d')
check "GET /api/v1/zitadel/teams/team-01/listeners" "200" "${LN_STATUS}" "${LN_BODY}"

# ── Step 6: Negative test — no token ─────────────────────────
echo "[+] Negative test: no auth..."
NO_AUTH_RESP=$(curl -s -w "\n%{http_code}" "${FLOWPLANE_HOST}/api/v1/zitadel/teams/team-01/clusters")
NO_AUTH_STATUS=$(echo "${NO_AUTH_RESP}" | tail -1)
NO_AUTH_BODY=$(echo "${NO_AUTH_RESP}" | sed '$d')
check "GET without token → 401" "401" "${NO_AUTH_STATUS}" "${NO_AUTH_BODY}"

echo ""
echo "============================================="
echo " Results: ${PASS} passed, ${FAIL} failed"
echo "============================================="

if [ "${FAIL}" -gt 0 ]; then
  exit 1
fi
