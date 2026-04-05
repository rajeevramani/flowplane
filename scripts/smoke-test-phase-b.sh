#!/usr/bin/env bash
# Phase B end-to-end smoke test.
#
# Exercises: superadmin login → org creation → invite → JIT provisioning
#            → permission resolution → cross-org isolation → idempotency
#
# Prerequisites: running Zitadel + Flowplane + seed complete (make seed)
# Usage: ./scripts/smoke-test-phase-b.sh
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

SMOKE_ORG="smoke-org-$$"
SMOKE_TEAM="smoke-team"
SMOKE_USER_EMAIL="smoketest-$$@example.com"
SMOKE_USER_PASSWORD="SmokeTest1!"

# Globals set during test
ZITADEL_PAT=""
ADMIN_TOKEN=""
SMOKE_ORG_ID=""
INVITED_USER_TOKEN=""
SMOKE_USER_ZITADEL_ID=""

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

pass() { echo -e "  ${GREEN}PASS${RESET} $1"; PASS=$((PASS + 1)); TESTS+=("PASS: $1"); }
fail_test() { echo -e "  ${RED}FAIL${RESET} $1: $2"; FAIL_COUNT=$((FAIL_COUNT + 1)); TESTS+=("FAIL: $1 — $2"); }
log()  { echo -e "${CYAN}[smoke]${RESET} $*"; }

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
  if [ ! -s "$PAT_FILE" ]; then
    fail_test "Admin PAT" "file not found: $PAT_FILE"
    return 1
  fi
  ZITADEL_PAT=$(cat "$PAT_FILE" | tr -d '[:space:]')
  pass "Services ready, PAT loaded"
}

# ═══════════════════════════════════════════════════════════════
# 2. SUPERADMIN LOGIN
# ═══════════════════════════════════════════════════════════════
test_superadmin_login() {
  log "2. Superadmin login"

  ADMIN_TOKEN=$(get_oidc_token "$SUPERADMIN_EMAIL" "$SUPERADMIN_PASSWORD" 2>&1) || true
  if [ -z "$ADMIN_TOKEN" ] || echo "$ADMIN_TOKEN" | grep -q "^ERROR:"; then
    fail_test "Superadmin OIDC token" "${ADMIN_TOKEN:-empty}"
    return 1
  fi
  pass "Superadmin OIDC token obtained"

  # Verify admin scope: GET /api/v1/admin/organizations should succeed
  fp_api GET /api/v1/admin/organizations "$ADMIN_TOKEN"
  if [ "$HTTP_CODE" = "200" ]; then
    pass "Superadmin can list organizations (admin:all)"
  else
    fail_test "Superadmin list orgs" "HTTP ${HTTP_CODE}: ${BODY}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 3. CREATE ORG
# ═══════════════════════════════════════════════════════════════
test_create_org() {
  log "3. Create org"

  fp_api POST /api/v1/bootstrap/initialize "$ADMIN_TOKEN" \
    "{\"orgName\": \"${SMOKE_ORG}\", \"displayName\": \"Smoke Org\", \"teamName\": \"${SMOKE_TEAM}\"}"

  if [ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "409" ]; then
    pass "Bootstrap org ${SMOKE_ORG} (HTTP ${HTTP_CODE})"
  else
    fail_test "Bootstrap org" "HTTP ${HTTP_CODE}: ${BODY}"
    return 1
  fi
}

# ═══════════════════════════════════════════════════════════════
# 4. LOOK UP ORG
# ═══════════════════════════════════════════════════════════════
test_lookup_org() {
  log "4. Look up org"

  fp_api GET /api/v1/admin/organizations "$ADMIN_TOKEN"
  SMOKE_ORG_ID=$(echo "$BODY" | jq -r ".items[] | select(.name==\"${SMOKE_ORG}\") | .id")

  if [ -n "$SMOKE_ORG_ID" ] && [ "$SMOKE_ORG_ID" != "null" ]; then
    pass "Org '${SMOKE_ORG}' found (id: ${SMOKE_ORG_ID})"
  else
    fail_test "Lookup org" "not found in org list"
    return 1
  fi
}

# ═══════════════════════════════════════════════════════════════
# 5. INVITE USER
# ═══════════════════════════════════════════════════════════════
test_invite_user() {
  log "5. Invite user"

  fp_api POST "/api/v1/admin/organizations/${SMOKE_ORG_ID}/invite" "$ADMIN_TOKEN" \
    "{\"email\": \"${SMOKE_USER_EMAIL}\", \"role\": \"admin\", \"firstName\": \"Smoke\", \"lastName\": \"Test\"}"

  if [ "$HTTP_CODE" = "201" ]; then
    local user_created
    user_created=$(echo "$BODY" | jq -r '.userCreated // .user_created')
    pass "User invited (HTTP 201, userCreated=${user_created})"
  elif [ "$HTTP_CODE" = "200" ]; then
    pass "User already a member (HTTP 200, idempotent)"
  else
    fail_test "Invite user" "HTTP ${HTTP_CODE}: ${BODY}"
    return 1
  fi
}

# ═══════════════════════════════════════════════════════════════
# 6. VERIFY ZITADEL USER
# ═══════════════════════════════════════════════════════════════
test_verify_zitadel_user() {
  log "6. Verify Zitadel user"

  api POST /v2/users "{\"queries\":[{\"emailQuery\":{\"emailAddress\":\"${SMOKE_USER_EMAIL}\"}}]}"
  SMOKE_USER_ZITADEL_ID=$(echo "$BODY" | jq -r '.result[0].userId // .result[0].id // empty' 2>/dev/null | head -1)

  if [ -n "$SMOKE_USER_ZITADEL_ID" ] && [ "$SMOKE_USER_ZITADEL_ID" != "null" ]; then
    pass "User exists in Zitadel (id: ${SMOKE_USER_ZITADEL_ID})"
  else
    fail_test "Zitadel user" "not found"
    return 1
  fi

  # Set initial password so we can log in
  api POST "/v2/users/${SMOKE_USER_ZITADEL_ID}/password" \
    "{\"newPassword\":{\"password\":\"${SMOKE_USER_PASSWORD}\",\"changeRequired\":false}}"

  if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
    pass "Password set for smoke test user"
  else
    fail_test "Set password" "HTTP ${HTTP_CODE}: ${BODY}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 7. VERIFY DB STATE
# ═══════════════════════════════════════════════════════════════
test_verify_db() {
  log "7. Verify DB state"

  # Check user exists in local DB
  local user_count
  user_count=$(psql_exec "SELECT COUNT(*) FROM users WHERE email = '${SMOKE_USER_EMAIL}';")
  if [ "$user_count" -ge 1 ]; then
    pass "User exists in Flowplane DB"
  else
    fail_test "DB user" "not found"
  fi

  # Check org membership
  local org_role
  org_role=$(psql_exec "SELECT om.role FROM organization_memberships om
    JOIN users u ON u.id = om.user_id
    WHERE u.email = '${SMOKE_USER_EMAIL}' AND om.org_id = '${SMOKE_ORG_ID}';")
  if [ "$org_role" = "admin" ]; then
    pass "Org membership role = admin"
  else
    fail_test "Org membership" "expected admin, got '${org_role}'"
  fi

  # Check team membership scopes contain wildcard
  local scopes
  scopes=$(psql_exec "SELECT utm.scopes FROM user_team_memberships utm
    JOIN users u ON u.id = utm.user_id
    JOIN teams t ON t.id = utm.team
    WHERE u.email = '${SMOKE_USER_EMAIL}' AND t.name = '${SMOKE_TEAM}';")
  if echo "$scopes" | grep -q "write"; then
    pass "Team membership has write scopes"
  else
    fail_test "Team scopes" "scopes: ${scopes}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 8. INVITED USER LOGIN
# ═══════════════════════════════════════════════════════════════
test_invited_user_login() {
  log "8. Invited user login"

  INVITED_USER_TOKEN=$(get_oidc_token "$SMOKE_USER_EMAIL" "$SMOKE_USER_PASSWORD" 2>&1) || true
  if [ -z "$INVITED_USER_TOKEN" ] || echo "$INVITED_USER_TOKEN" | grep -q "^ERROR:"; then
    fail_test "Invited user OIDC token" "${INVITED_USER_TOKEN:-empty}"
    return 1
  fi
  pass "Invited user OIDC token obtained"
}

# ═══════════════════════════════════════════════════════════════
# 9. PERMISSION RESOLUTION
# ═══════════════════════════════════════════════════════════════
test_permission_resolution() {
  log "9. Permission resolution"

  # Org admin should be able to list members of their own org
  fp_api GET "/api/v1/admin/organizations/${SMOKE_ORG_ID}/members" "$INVITED_USER_TOKEN"
  if [ "$HTTP_CODE" = "200" ]; then
    pass "Org admin can list own org members"
  else
    fail_test "List org members" "HTTP ${HTTP_CODE}: ${BODY}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 10. CROSS-ORG ISOLATION
# ═══════════════════════════════════════════════════════════════
test_cross_org_isolation() {
  log "10. Cross-org isolation"

  # Org admin should NOT have admin:all scope
  fp_api GET /api/v1/admin/organizations "$INVITED_USER_TOKEN"
  if [ "$HTTP_CODE" = "403" ]; then
    pass "Org admin cannot list all organizations (403)"
  else
    fail_test "Cross-org isolation" "expected 403, got HTTP ${HTTP_CODE}"
  fi
}

# ═══════════════════════════════════════════════════════════════
# 11. IDEMPOTENCY
# ═══════════════════════════════════════════════════════════════
test_idempotency() {
  log "11. Idempotency"

  # Re-invite same user, same role -> 200
  fp_api POST "/api/v1/admin/organizations/${SMOKE_ORG_ID}/invite" "$ADMIN_TOKEN" \
    "{\"email\": \"${SMOKE_USER_EMAIL}\", \"role\": \"admin\", \"firstName\": \"Smoke\", \"lastName\": \"Test\"}"
  if [ "$HTTP_CODE" = "200" ]; then
    pass "Re-invite same role returns 200"
  else
    fail_test "Idempotent re-invite" "HTTP ${HTTP_CODE}: ${BODY}"
  fi

  # Re-invite different role -> 200 (role changed)
  fp_api POST "/api/v1/admin/organizations/${SMOKE_ORG_ID}/invite" "$ADMIN_TOKEN" \
    "{\"email\": \"${SMOKE_USER_EMAIL}\", \"role\": \"member\", \"firstName\": \"Smoke\", \"lastName\": \"Test\"}"
  if [ "$HTTP_CODE" = "200" ]; then
    pass "Re-invite different role returns 200 (role updated)"
  else
    fail_test "Role change invite" "HTTP ${HTTP_CODE}: ${BODY}"
  fi

  # Restore admin role for subsequent tests
  fp_api POST "/api/v1/admin/organizations/${SMOKE_ORG_ID}/invite" "$ADMIN_TOKEN" \
    "{\"email\": \"${SMOKE_USER_EMAIL}\", \"role\": \"admin\", \"firstName\": \"Smoke\", \"lastName\": \"Test\"}"
}

# ═══════════════════════════════════════════════════════════════
# 12. CLEANUP
# ═══════════════════════════════════════════════════════════════
cleanup() {
  log "12. Cleanup"

  # Remove smoke org (will fail if teams exist; clean teams first)
  local team_id
  team_id=$(psql_exec "SELECT id FROM teams WHERE name = '${SMOKE_TEAM}' AND org_id = '${SMOKE_ORG_ID}';" 2>/dev/null || true)

  if [ -n "$team_id" ]; then
    # Remove team memberships
    psql_exec "DELETE FROM user_team_memberships WHERE team = '${team_id}';" 2>/dev/null || true
    # Remove team
    psql_exec "DELETE FROM teams WHERE id = '${team_id}';" 2>/dev/null || true
  fi

  # Remove org memberships
  psql_exec "DELETE FROM organization_memberships WHERE org_id = '${SMOKE_ORG_ID}';" 2>/dev/null || true

  # Remove org
  psql_exec "DELETE FROM organizations WHERE id = '${SMOKE_ORG_ID}';" 2>/dev/null || true

  # Remove local user
  psql_exec "DELETE FROM users WHERE email = '${SMOKE_USER_EMAIL}';" 2>/dev/null || true

  # Remove Zitadel user
  if [ -n "$SMOKE_USER_ZITADEL_ID" ] && [ "$SMOKE_USER_ZITADEL_ID" != "null" ]; then
    api DELETE "/v2/users/${SMOKE_USER_ZITADEL_ID}" 2>/dev/null || true
  fi

  pass "Cleanup complete"
}

# ═══════════════════════════════════════════════════════════════
# MAIN
# ═══════════════════════════════════════════════════════════════
main() {
  echo ""
  echo -e "${BOLD}${CYAN}━━━ Phase B Smoke Test ━━━${RESET}"
  echo ""

  setup || { echo -e "\n${RED}Setup failed, aborting${RESET}"; exit 1; }

  test_superadmin_login   || true
  test_create_org         || true
  test_lookup_org         || true
  test_invite_user        || true
  test_verify_zitadel_user || true
  test_verify_db          || true
  test_invited_user_login || true
  test_permission_resolution || true
  test_cross_org_isolation   || true
  test_idempotency        || true
  cleanup                 || true

  echo ""
  echo -e "${BOLD}━━━ Results ━━━${RESET}"
  echo ""
  for t in "${TESTS[@]}"; do
    if echo "$t" | grep -q "^PASS:"; then
      echo -e "  ${GREEN}${t}${RESET}"
    else
      echo -e "  ${RED}${t}${RESET}"
    fi
  done
  echo ""
  echo -e "  ${BOLD}Total: $((PASS + FAIL_COUNT))  |  Pass: ${PASS}  |  Fail: ${FAIL_COUNT}${RESET}"
  echo ""

  if [ "$FAIL_COUNT" -gt 0 ]; then
    exit 1
  fi
}

main "$@"
