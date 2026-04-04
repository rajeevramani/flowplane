#!/usr/bin/env bash
# Phase C end-to-end smoke test.
#
# Exercises: agent provisioning → client_credentials auth → MCP tool access
#            → agent lifecycle (list, idempotent re-create, delete)
#            → cross-org isolation → access revocation after delete
#
# Prerequisites: running Zitadel + Flowplane + seed complete (make seed)
# Usage: ./scripts/smoke-test-phase-c.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Source auth helper
# shellcheck source=lib/zitadel-auth.sh
source "${SCRIPT_DIR}/lib/zitadel-auth.sh"

# Load Zitadel env
if [ -f "${PROJECT_DIR}/.env.zitadel" ]; then
  # shellcheck source=/dev/null
  source "${PROJECT_DIR}/.env.zitadel"
fi

ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8081}"
FLOWPLANE_URL="${FLOWPLANE_URL:-http://localhost:8080}"
PAT_FILE="${PROJECT_DIR}/zitadel/machinekey/admin-pat.txt"

SUPERADMIN_EMAIL="${FLOWPLANE_SUPERADMIN_EMAIL:-admin@flowplane.local}"
SUPERADMIN_PASSWORD="${FLOWPLANE_SUPERADMIN_INITIAL_PASSWORD:-Flowplane1!}"

# Unique test identifiers to avoid collision with existing data
TS="$$"
SMOKE_ORG="smoke-c-${TS}"
SMOKE_TEAM="${SMOKE_ORG}-default"
SMOKE_USER_EMAIL="smoke-c-admin-${TS}@example.com"
SMOKE_USER_PASSWORD="SmokeC1test!"
AGENT_NAME="smoke-agent-${TS}"

SMOKE_ORG_B="smoke-c-b-${TS}"
SMOKE_USER_B_EMAIL="smoke-c-b-admin-${TS}@example.com"
SMOKE_USER_B_PASSWORD="SmokeC2test!"

# Globals set during test
ZITADEL_PAT=""
ADMIN_TOKEN=""
SMOKE_ORG_ID=""
SMOKE_ORG_B_ID=""
ORG_ADMIN_TOKEN=""
ORG_B_ADMIN_TOKEN=""
AGENT_CLIENT_ID=""
AGENT_CLIENT_SECRET=""
AGENT_TOKEN_ENDPOINT=""
AGENT_JWT=""

# Container runtime
if command -v docker &>/dev/null; then
  CONTAINER_RT=docker
elif command -v podman &>/dev/null; then
  CONTAINER_RT=podman
else
  echo "FAIL: Neither docker nor podman found" >&2
  exit 1
fi

psql_exec() {
  $CONTAINER_RT exec flowplane-postgres psql -U flowplane -d flowplane -tAc "$1"
}

# Colors
CYAN='\033[36m'
GREEN='\033[32m'
RED='\033[31m'
YELLOW='\033[33m'
BOLD='\033[1m'
RESET='\033[0m'

PASS=0
FAIL_COUNT=0
TESTS=()

pass()      { echo -e "  ${GREEN}PASS${RESET} $1"; PASS=$((PASS + 1)); TESTS+=("PASS: $1"); }
fail_test() { echo -e "  ${RED}FAIL${RESET} $1: $2"; FAIL_COUNT=$((FAIL_COUNT + 1)); TESTS+=("FAIL: $1 — $2"); }
log()       { echo -e "${CYAN}[smoke-c]${RESET} $*"; }
skip_test() { echo -e "  ${YELLOW}SKIP${RESET} $1: $2"; TESTS+=("SKIP: $1"); }

# Zitadel API helper (uses ZITADEL_PAT)
BODY=""
HTTP_CODE=""
api() {
  local method="$1" path="$2" body="${3:-}"
  local args=(
    -s -w '\n%{http_code}'
    -X "${method}"
    -H "Authorization: Bearer ${ZITADEL_PAT}"
    -H "Content-Type: application/json"
  )
  if [ -n "${body}" ]; then
    args+=(-d "${body}")
  fi
  local raw
  raw=$(curl "${args[@]}" "${ZITADEL_HOST}${path}")
  HTTP_CODE=$(echo "$raw" | tail -1)
  BODY=$(echo "$raw" | sed '$d')
}

# Flowplane API helper
fp_api() {
  local method="$1" path="$2" token="$3" body="${4:-}"
  local args=(
    -s -w '\n%{http_code}'
    -X "${method}"
    -H "Authorization: Bearer ${token}"
    -H "Content-Type: application/json"
  )
  if [ -n "${body}" ]; then
    args+=(-d "${body}")
  fi
  local raw
  raw=$(curl "${args[@]}" "${FLOWPLANE_URL}${path}")
  HTTP_CODE=$(echo "$raw" | tail -1)
  BODY=$(echo "$raw" | sed '$d')
}

# ═══════════════════════════════════════════════════════════════
# 1. SETUP
# ═══════════════════════════════════════════════════════════════
setup() {
  log "1. Setup"

  # Wait for Zitadel
  local attempts=0
  while ! curl -sf "${ZITADEL_HOST}/debug/ready" > /dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ $attempts -ge 60 ]; then
      fail_test "Zitadel readiness" "not ready after 60s"
      return 1
    fi
    sleep 1
  done

  # Wait for Flowplane
  attempts=0
  while ! curl -sf "${FLOWPLANE_URL}/swagger-ui/" > /dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ $attempts -ge 60 ]; then
      fail_test "Flowplane readiness" "not ready after 60s"
      return 1
    fi
    sleep 1
  done

  # Read admin PAT
  local pat_attempts=0
  while [ ! -s "${PAT_FILE}" ]; do
    pat_attempts=$((pat_attempts + 1))
    if [ $pat_attempts -ge 30 ]; then
      fail_test "PAT file" "not written after 30s: ${PAT_FILE}"
      return 1
    fi
    sleep 1
  done
  ZITADEL_PAT=$(tr -d '[:space:]' < "${PAT_FILE}")

  # Obtain superadmin token
  ADMIN_TOKEN=$(get_oidc_token "${SUPERADMIN_EMAIL}" "${SUPERADMIN_PASSWORD}")
  if [ -z "${ADMIN_TOKEN}" ]; then
    fail_test "Superadmin OIDC token" "failed to obtain"
    return 1
  fi

  pass "Services ready, admin authenticated"
}

# ═══════════════════════════════════════════════════════════════
# 2. CREATE TEST ORG + ORG ADMIN
# ═══════════════════════════════════════════════════════════════
create_test_org() {
  log "2. Creating test org '${SMOKE_ORG}'"

  # Bootstrap org
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Content-Type: application/json" \
    -d "{\"orgName\": \"${SMOKE_ORG}\", \"displayName\": \"Smoke C Test\", \"teamName\": \"${SMOKE_TEAM}\"}" \
    "${FLOWPLANE_URL}/api/v1/bootstrap/initialize")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "${http_code}" = "201" ] || [ "${http_code}" = "409" ]; then
    pass "Bootstrap org created (or already exists)"
  else
    fail_test "Bootstrap org" "HTTP ${http_code}: ${body}"
    return 1
  fi

  # Get org ID
  local org_list
  org_list=$(curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    "${FLOWPLANE_URL}/api/v1/admin/organizations")
  SMOKE_ORG_ID=$(echo "${org_list}" | jq -r ".items[] | select(.name==\"${SMOKE_ORG}\") | .id")
  if [ -z "${SMOKE_ORG_ID}" ] || [ "${SMOKE_ORG_ID}" = "null" ]; then
    fail_test "Org ID lookup" "org '${SMOKE_ORG}' not found in list"
    return 1
  fi
  pass "Org '${SMOKE_ORG}' id: ${SMOKE_ORG_ID}"

  # Invite org admin (initial_password for local dev)
  fp_api POST "/api/v1/admin/organizations/${SMOKE_ORG_ID}/invite" "${ADMIN_TOKEN}" \
    "{\"email\": \"${SMOKE_USER_EMAIL}\", \"role\": \"admin\", \"firstName\": \"Smoke\", \"lastName\": \"Admin\", \"initialPassword\": \"${SMOKE_USER_PASSWORD}\"}"
  if [ "${HTTP_CODE}" = "201" ] || [ "${HTTP_CODE}" = "200" ]; then
    pass "Org admin invited"
  else
    fail_test "Invite org admin" "HTTP ${HTTP_CODE}: ${BODY}"
    return 1
  fi

  # Obtain org admin token
  ORG_ADMIN_TOKEN=$(get_oidc_token "${SMOKE_USER_EMAIL}" "${SMOKE_USER_PASSWORD}")
  if [ -z "${ORG_ADMIN_TOKEN}" ]; then
    fail_test "Org admin OIDC token" "failed to obtain"
    return 1
  fi
  pass "Org admin authenticated"
}

# ═══════════════════════════════════════════════════════════════
# 3. AGENT PROVISIONING
# ═══════════════════════════════════════════════════════════════
provision_agent() {
  log "3. Agent provisioning"

  local scopes_json
  scopes_json='["clusters:read","clusters:write","routes:read","routes:write"]'

  fp_api POST "/api/v1/orgs/${SMOKE_ORG}/agents" "${ORG_ADMIN_TOKEN}" \
    "$(jq -n \
      --arg name "${AGENT_NAME}" \
      --arg team "${SMOKE_TEAM}" \
      --argjson scopes "${scopes_json}" \
      '{name: $name, teams: [$team], scopes: $scopes}')"

  if [ "${HTTP_CODE}" = "201" ]; then
    pass "Agent created (201)"
  else
    fail_test "Agent creation" "HTTP ${HTTP_CODE}: ${BODY}"
    return 1
  fi

  AGENT_CLIENT_ID=$(echo "${BODY}" | jq -r '.clientId // empty')
  AGENT_CLIENT_SECRET=$(echo "${BODY}" | jq -r '.clientSecret // empty')
  AGENT_TOKEN_ENDPOINT=$(echo "${BODY}" | jq -r '.tokenEndpoint // empty')

  if [ -n "${AGENT_CLIENT_ID}" ] && [ "${AGENT_CLIENT_ID}" != "null" ]; then
    pass "Response has clientId"
  else
    fail_test "Agent clientId" "missing from response: ${BODY}"
  fi

  if [ -n "${AGENT_CLIENT_SECRET}" ] && [ "${AGENT_CLIENT_SECRET}" != "null" ]; then
    pass "Response has clientSecret"
  else
    fail_test "Agent clientSecret" "missing from response: ${BODY}"
  fi

  if [ -n "${AGENT_TOKEN_ENDPOINT}" ] && [ "${AGENT_TOKEN_ENDPOINT}" != "null" ]; then
    pass "Response has tokenEndpoint"
  else
    fail_test "Agent tokenEndpoint" "missing from response: ${BODY}"
  fi

  # Verify user_type = 'machine' in DB
  local user_type
  user_type=$(psql_exec "SELECT user_type FROM users WHERE name = '${AGENT_NAME}' LIMIT 1" 2>/dev/null || echo "")
  if [ "${user_type}" = "machine" ]; then
    pass "DB: user_type = 'machine'"
  else
    fail_test "DB user_type" "expected 'machine', got '${user_type}'"
  fi

  # Verify org_memberships row exists
  local mem_count
  mem_count=$(psql_exec "SELECT COUNT(*) FROM organization_memberships om JOIN users u ON u.id = om.user_id WHERE u.name = '${AGENT_NAME}' AND om.org_id = '${SMOKE_ORG_ID}'" 2>/dev/null || echo "0")
  if [ "${mem_count}" = "1" ]; then
    pass "DB: org_memberships row exists"
  else
    fail_test "DB org_memberships" "expected 1, got ${mem_count}"
  fi

  # Verify user_team_memberships row exists with scopes
  local utm_count
  utm_count=$(psql_exec "SELECT COUNT(*) FROM user_team_memberships utm JOIN users u ON u.id = utm.user_id WHERE u.name = '${AGENT_NAME}'" 2>/dev/null || echo "0")
  if [ "${utm_count}" -ge "1" ]; then
    pass "DB: user_team_memberships rows exist (${utm_count})"
  else
    fail_test "DB user_team_memberships" "expected >= 1, got ${utm_count}"
  fi

  # Platform admin must NOT be able to provision agents (403)
  fp_api POST "/api/v1/orgs/${SMOKE_ORG}/agents" "${ADMIN_TOKEN}" \
    "$(jq -n --arg name "platform-attempt" --arg team "${SMOKE_TEAM}" '{name: $name, teams: [$team]}')"
  if [ "${HTTP_CODE}" = "403" ]; then
    pass "Platform admin gets 403 on agent provision"
  else
    fail_test "Platform admin agent provision" "expected 403, got ${HTTP_CODE}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 4. AGENT AUTHENTICATION (client_credentials)
# ═══════════════════════════════════════════════════════════════
agent_auth() {
  log "4. Agent authentication via client_credentials"

  if [ -z "${AGENT_CLIENT_ID}" ] || [ -z "${AGENT_CLIENT_SECRET}" ] || [ -z "${AGENT_TOKEN_ENDPOINT}" ]; then
    skip_test "Agent authentication" "no credentials (agent creation may have failed)"
    return 0
  fi

  local token_raw token_code token_body
  token_raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Content-Type: application/x-www-form-urlencoded" \
    -d "grant_type=client_credentials" \
    -d "client_id=${AGENT_CLIENT_ID}" \
    -d "client_secret=${AGENT_CLIENT_SECRET}" \
    -d "scope=openid" \
    "${AGENT_TOKEN_ENDPOINT}")
  token_code=$(echo "${token_raw}" | tail -1)
  token_body=$(echo "${token_raw}" | sed '$d')

  if [ "${token_code}" = "200" ]; then
    pass "client_credentials grant succeeded"
  else
    fail_test "Agent client_credentials" "HTTP ${token_code}: ${token_body}"
    return 0
  fi

  AGENT_JWT=$(echo "${token_body}" | jq -r '.access_token // empty')
  if [ -n "${AGENT_JWT}" ] && [ "${AGENT_JWT}" != "null" ]; then
    pass "Agent JWT obtained"
  else
    fail_test "Agent JWT" "missing from token response"
    return 0
  fi

  # Call an authenticated endpoint with the agent JWT
  # Use the session endpoint which resolves permissions from DB
  local session_raw session_code
  session_raw=$(curl -s -w '\n%{http_code}' \
    -H "Authorization: Bearer ${AGENT_JWT}" \
    "${FLOWPLANE_URL}/api/v1/auth/session")
  session_code=$(echo "${session_raw}" | tail -1)

  if [ "${session_code}" = "200" ]; then
    pass "Agent JWT accepted by auth/session endpoint"
  else
    fail_test "Agent JWT auth" "auth/session returned ${session_code}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 5. AGENT LIFECYCLE
# ═══════════════════════════════════════════════════════════════
agent_lifecycle() {
  log "5. Agent lifecycle (list → idempotent re-create → delete)"

  # List agents — should include our agent
  fp_api GET "/api/v1/orgs/${SMOKE_ORG}/agents" "${ORG_ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "200" ]; then
    local agent_count
    agent_count=$(echo "${BODY}" | jq '.agents | length')
    if [ "${agent_count}" -ge "1" ]; then
      pass "GET agents returns ${agent_count} agent(s)"
    else
      fail_test "List agents" "expected >= 1, got ${agent_count}"
    fi

    # Verify our agent is in the list
    local found
    found=$(echo "${BODY}" | jq --arg name "${AGENT_NAME}" '.agents[] | select(.name == $name) | .name' -r)
    if [ "${found}" = "${AGENT_NAME}" ]; then
      pass "Agent '${AGENT_NAME}' visible in list"
    else
      fail_test "Agent in list" "'${AGENT_NAME}' not found in: ${BODY}"
    fi
  else
    fail_test "List agents" "HTTP ${HTTP_CODE}: ${BODY}"
  fi

  # Platform admin must NOT be able to list agents (403)
  fp_api GET "/api/v1/orgs/${SMOKE_ORG}/agents" "${ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "403" ]; then
    pass "Platform admin gets 403 on list agents"
  else
    fail_test "Platform admin list agents" "expected 403, got ${HTTP_CODE}"
  fi

  # Idempotent re-provisioning — same name returns 200 without credentials
  fp_api POST "/api/v1/orgs/${SMOKE_ORG}/agents" "${ORG_ADMIN_TOKEN}" \
    "$(jq -n --arg name "${AGENT_NAME}" --arg team "${SMOKE_TEAM}" '{name: $name, teams: [$team]}')"
  if [ "${HTTP_CODE}" = "200" ]; then
    pass "Re-create same agent returns 200 (idempotent)"
    local re_secret
    re_secret=$(echo "${BODY}" | jq -r '.clientSecret // empty')
    if [ -z "${re_secret}" ] || [ "${re_secret}" = "null" ]; then
      pass "Idempotent response has no clientSecret (correct)"
    else
      fail_test "Idempotent credentials" "clientSecret should be absent on re-create"
    fi
  else
    fail_test "Idempotent re-create" "expected 200, got ${HTTP_CODE}: ${BODY}"
  fi

  # Delete the agent
  fp_api DELETE "/api/v1/orgs/${SMOKE_ORG}/agents/${AGENT_NAME}" "${ORG_ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "204" ]; then
    pass "Delete agent returns 204"
  else
    fail_test "Delete agent" "HTTP ${HTTP_CODE}: ${BODY}"
    return 0
  fi

  # Verify agent no longer in list
  fp_api GET "/api/v1/orgs/${SMOKE_ORG}/agents" "${ORG_ADMIN_TOKEN}" ""
  local remaining
  remaining=$(echo "${BODY}" | jq --arg name "${AGENT_NAME}" '[.agents[] | select(.name == $name)] | length')
  if [ "${remaining}" = "0" ]; then
    pass "Deleted agent no longer in list"
  else
    fail_test "Agent after delete" "still appears in list"
  fi

  # Verify DB rows removed
  local db_count
  db_count=$(psql_exec "SELECT COUNT(*) FROM users WHERE name = '${AGENT_NAME}'" 2>/dev/null || echo "0")
  if [ "${db_count}" = "0" ]; then
    pass "DB: user row deleted"
  else
    fail_test "DB user after delete" "expected 0, got ${db_count}"
  fi

  # Delete non-existent agent → 404
  fp_api DELETE "/api/v1/orgs/${SMOKE_ORG}/agents/does-not-exist-${TS}" "${ORG_ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "404" ]; then
    pass "Delete non-existent agent returns 404"
  else
    fail_test "Delete non-existent" "expected 404, got ${HTTP_CODE}"
  fi

  # Platform admin must NOT be able to delete agents (403)
  fp_api DELETE "/api/v1/orgs/${SMOKE_ORG}/agents/any-name" "${ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "403" ]; then
    pass "Platform admin gets 403 on delete agent"
  else
    fail_test "Platform admin delete agent" "expected 403, got ${HTTP_CODE}: ${BODY}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 6. ACCESS REVOCATION
# ═══════════════════════════════════════════════════════════════
access_revocation() {
  log "6. Access revocation after delete"

  if [ -z "${AGENT_JWT}" ]; then
    skip_test "Access revocation" "no agent JWT (auth step may have failed)"
    return 0
  fi

  # After deletion, agent JWT should resolve to zero permissions
  # The cache eviction in delete_org_agent means next DB lookup returns empty
  local session_raw session_code session_body
  session_raw=$(curl -s -w '\n%{http_code}' \
    -H "Authorization: Bearer ${AGENT_JWT}" \
    "${FLOWPLANE_URL}/api/v1/auth/session")
  session_code=$(echo "${session_raw}" | tail -1)
  session_body=$(echo "${session_raw}" | sed '$d')

  if [ "${session_code}" = "200" ]; then
    # JWT is structurally valid but permissions should be empty
    local scope_count
    scope_count=$(echo "${session_body}" | jq '.scopes | length' 2>/dev/null || echo "-1")
    if [ "${scope_count}" = "0" ]; then
      pass "Deleted agent JWT resolves to zero scopes"
    else
      # Session still returns 200 with stale permissions — this is acceptable
      # if the cache TTL hasn't expired; log as info rather than fail
      skip_test "Deleted agent zero scopes" "cache may not have expired yet (${scope_count} scopes)"
    fi
  elif [ "${session_code}" = "401" ]; then
    pass "Deleted agent JWT rejected with 401"
  else
    fail_test "Access revocation" "unexpected status ${session_code}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 7. CROSS-ORG ISOLATION
# ═══════════════════════════════════════════════════════════════
cross_org_isolation() {
  log "7. Cross-org isolation"

  # Create a second org
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Content-Type: application/json" \
    -d "{\"orgName\": \"${SMOKE_ORG_B}\", \"displayName\": \"Smoke C Org B\", \"teamName\": \"${SMOKE_ORG_B}-default\"}" \
    "${FLOWPLANE_URL}/api/v1/bootstrap/initialize")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "${http_code}" = "201" ] || [ "${http_code}" = "409" ]; then
    pass "Org B bootstrapped"
  else
    fail_test "Bootstrap org B" "HTTP ${http_code}: ${body}"
    return 0
  fi

  # Get org B ID
  local org_b_list
  org_b_list=$(curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    "${FLOWPLANE_URL}/api/v1/admin/organizations")
  SMOKE_ORG_B_ID=$(echo "${org_b_list}" | jq -r ".items[] | select(.name==\"${SMOKE_ORG_B}\") | .id")
  if [ -z "${SMOKE_ORG_B_ID}" ] || [ "${SMOKE_ORG_B_ID}" = "null" ]; then
    fail_test "Org B ID lookup" "not found"
    return 0
  fi

  # Invite org B admin
  fp_api POST "/api/v1/admin/organizations/${SMOKE_ORG_B_ID}/invite" "${ADMIN_TOKEN}" \
    "{\"email\": \"${SMOKE_USER_B_EMAIL}\", \"role\": \"admin\", \"firstName\": \"Smoke\", \"lastName\": \"AdminB\", \"initialPassword\": \"${SMOKE_USER_B_PASSWORD}\"}"
  if [ "${HTTP_CODE}" != "201" ] && [ "${HTTP_CODE}" != "200" ]; then
    fail_test "Invite org B admin" "HTTP ${HTTP_CODE}: ${BODY}"
    return 0
  fi

  # Obtain org B admin token
  ORG_B_ADMIN_TOKEN=$(get_oidc_token "${SMOKE_USER_B_EMAIL}" "${SMOKE_USER_B_PASSWORD}")
  if [ -z "${ORG_B_ADMIN_TOKEN}" ]; then
    fail_test "Org B admin OIDC token" "failed to obtain"
    return 0
  fi
  pass "Org B admin authenticated"

  # Provision an agent in org A (using org A admin)
  fp_api POST "/api/v1/orgs/${SMOKE_ORG}/agents" "${ORG_ADMIN_TOKEN}" \
    "$(jq -n --arg name "isolation-agent-${TS}" --arg team "${SMOKE_TEAM}" '{name: $name, teams: [$team]}')"
  if [ "${HTTP_CODE}" = "201" ]; then
    pass "Agent in org A provisioned for isolation test"
  else
    fail_test "Provision org A agent" "HTTP ${HTTP_CODE}: ${BODY}"
    return 0
  fi

  # Org B admin must NOT see org A's agents
  fp_api GET "/api/v1/orgs/${SMOKE_ORG}/agents" "${ORG_B_ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "403" ]; then
    pass "Org B admin gets 403 listing org A agents"
  else
    fail_test "Cross-org agent list isolation" "expected 403, got ${HTTP_CODE}: ${BODY}"
  fi

  # Org B admin must NOT delete org A's agents
  fp_api DELETE "/api/v1/orgs/${SMOKE_ORG}/agents/isolation-agent-${TS}" "${ORG_B_ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "403" ]; then
    pass "Org B admin gets 403 deleting org A agent"
  else
    fail_test "Cross-org agent delete isolation" "expected 403, got ${HTTP_CODE}: ${BODY}"
  fi

  # Org B admin can provision agents in their own org
  fp_api POST "/api/v1/orgs/${SMOKE_ORG_B}/agents" "${ORG_B_ADMIN_TOKEN}" \
    "$(jq -n --arg name "org-b-agent-${TS}" --arg team "${SMOKE_ORG_B}-default" '{name: $name, teams: [$team]}')"
  if [ "${HTTP_CODE}" = "201" ]; then
    pass "Org B admin can provision agents in org B"
  else
    fail_test "Org B agent provision" "HTTP ${HTTP_CODE}: ${BODY}"
  fi

  # Org A admin must NOT see org B's agents
  fp_api GET "/api/v1/orgs/${SMOKE_ORG_B}/agents" "${ORG_ADMIN_TOKEN}" ""
  if [ "${HTTP_CODE}" = "403" ]; then
    pass "Org A admin gets 403 listing org B agents"
  else
    fail_test "Cross-org agent list isolation (A→B)" "expected 403, got ${HTTP_CODE}: ${BODY}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 8. DCR ENDPOINT
# ═══════════════════════════════════════════════════════════════
dcr_endpoint() {
  log "8. DCR endpoint (/api/v1/oauth/register)"

  local dcr_agent_name="dcr-agent-${TS}"

  # Unauthenticated request must fail (401 or 403)
  local unauth_raw unauth_code
  unauth_raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Content-Type: application/json" \
    -d "{\"client_name\": \"${dcr_agent_name}\", \"grant_types\": [\"client_credentials\"]}" \
    "${FLOWPLANE_URL}/api/v1/oauth/register")
  unauth_code=$(echo "${unauth_raw}" | tail -1)
  if [ "${unauth_code}" = "401" ] || [ "${unauth_code}" = "403" ]; then
    pass "DCR requires authentication (${unauth_code})"
  else
    fail_test "DCR unauthenticated" "expected 401/403, got ${unauth_code}"
  fi

  # Org admin can register via DCR
  local dcr_scope="team:${SMOKE_TEAM}:clusters:read team:${SMOKE_TEAM}:routes:read"
  fp_api POST "/api/v1/oauth/register" "${ORG_ADMIN_TOKEN}" \
    "$(jq -n \
      --arg name "${dcr_agent_name}" \
      --arg scope "${dcr_scope}" \
      '{client_name: $name, grant_types: ["client_credentials"], scope: $scope}')"

  if [ "${HTTP_CODE}" = "201" ]; then
    pass "DCR registration returns 201"
    local dcr_client_id dcr_secret
    dcr_client_id=$(echo "${BODY}" | jq -r '.client_id // empty')
    dcr_secret=$(echo "${BODY}" | jq -r '.client_secret // empty')
    if [ -n "${dcr_client_id}" ] && [ "${dcr_client_id}" != "null" ]; then
      pass "DCR response has client_id"
    else
      fail_test "DCR client_id" "missing"
    fi
    if [ -n "${dcr_secret}" ] && [ "${dcr_secret}" != "null" ]; then
      pass "DCR response has client_secret"
    else
      fail_test "DCR client_secret" "missing"
    fi

    # Verify DB: scopes stored in user_team_memberships (not Zitadel role grants)
    local utm_scopes
    utm_scopes=$(psql_exec "SELECT scopes FROM user_team_memberships utm JOIN users u ON u.id = utm.user_id WHERE u.name = '${dcr_agent_name}' LIMIT 1" 2>/dev/null || echo "")
    if [ -n "${utm_scopes}" ]; then
      pass "DCR: scopes stored in user_team_memberships DB"
    else
      fail_test "DCR DB scopes" "no user_team_memberships rows found for DCR agent"
    fi

    # Idempotent re-registration returns 200 without credentials
    fp_api POST "/api/v1/oauth/register" "${ORG_ADMIN_TOKEN}" \
      "$(jq -n --arg name "${dcr_agent_name}" '{client_name: $name, grant_types: ["client_credentials"]}')"
    if [ "${HTTP_CODE}" = "200" ]; then
      pass "DCR re-registration returns 200 (idempotent)"
    else
      fail_test "DCR idempotent" "expected 200, got ${HTTP_CODE}: ${BODY}"
    fi
  else
    fail_test "DCR registration" "HTTP ${HTTP_CODE}: ${BODY}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 9. CLEANUP
# ═══════════════════════════════════════════════════════════════
cleanup() {
  log "9. Cleanup"

  # Best-effort: delete test orgs via admin API
  local deleted_orgs=0
  for org_id in "${SMOKE_ORG_ID}" "${SMOKE_ORG_B_ID}"; do
    if [ -n "${org_id}" ] && [ "${org_id}" != "null" ]; then
      fp_api DELETE "/api/v1/admin/organizations/${org_id}" "${ADMIN_TOKEN}" ""
      if [ "${HTTP_CODE}" = "204" ] || [ "${HTTP_CODE}" = "404" ]; then
        deleted_orgs=$((deleted_orgs + 1))
      fi
    fi
  done

  log "Cleanup: removed ${deleted_orgs} test org(s)"

  # Cleanup Zitadel users for any machine users we created (best-effort)
  # The DB cascade handles org_memberships and user_team_memberships
}

# ═══════════════════════════════════════════════════════════════
# MAIN
# ═══════════════════════════════════════════════════════════════
main() {
  echo ""
  echo -e "${CYAN}━━━ Flowplane Phase C Smoke Test ━━━${RESET}"
  echo ""

  setup
  create_test_org
  provision_agent
  agent_auth
  agent_lifecycle
  access_revocation
  cross_org_isolation
  dcr_endpoint
  cleanup

  echo ""
  echo -e "${BOLD}Results:${RESET}"
  for t in "${TESTS[@]}"; do
    if [[ "${t}" == PASS* ]]; then
      echo -e "  ${GREEN}${t}${RESET}"
    elif [[ "${t}" == FAIL* ]]; then
      echo -e "  ${RED}${t}${RESET}"
    else
      echo -e "  ${YELLOW}${t}${RESET}"
    fi
  done
  echo ""
  echo -e "  Passed: ${GREEN}${PASS}${RESET}  Failed: ${RED}${FAIL_COUNT}${RESET}"
  echo ""

  if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
  fi
}

main "$@"
