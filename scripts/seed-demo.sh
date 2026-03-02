#!/usr/bin/env bash
# Seed demo data for local Flowplane development
#
# Creates: demo org (acme-corp), demo user, machine user, DB permissions
# Prerequisites: curl, jq, running Zitadel + Flowplane (run 'make up' first)
# Usage: ./scripts/seed-demo.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8081}"
FLOWPLANE_URL="${FLOWPLANE_URL:-http://localhost:8080}"
PAT_FILE="${PROJECT_DIR}/zitadel/machinekey/admin-pat.txt"

# Demo org
ORG_NAME="acme-corp"
ORG_DISPLAY="Acme Corp"
TEAM_NAME="engineering"

# Demo human user
HUMAN_USERNAME="demo@acme-corp.com"
HUMAN_FIRST="Demo"
HUMAN_LAST="User"
HUMAN_PASSWORD="Flowplane1!"

# Machine user
MACHINE_USERNAME="flowplane-agent"
MACHINE_NAME="Flowplane Agent"

# Colors
CYAN='\033[36m'
GREEN='\033[32m'
YELLOW='\033[33m'
RED='\033[31m'
BOLD='\033[1m'
RESET='\033[0m'

log()  { echo -e "${CYAN}[seed]${RESET} $*"; }
ok()   { echo -e "${GREEN}  ✓${RESET} $*"; }
skip() { echo -e "${YELLOW}  ─${RESET} $* (already exists)"; }
fail() { echo -e "${RED}  ✗${RESET} $*"; }

# ── Prerequisite check ──────────────────────────────────────────
for cmd in curl jq; do
  if ! command -v "$cmd" &>/dev/null; then
    fail "Required command not found: $cmd"
    exit 1
  fi
done

# ── API helper ──────────────────────────────────────────────────
# Calls Zitadel Management API with admin PAT.
# Returns response body. Sets HTTP_CODE global.
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

# ── Container runtime ────────────────────────────────────────────
if command -v docker &>/dev/null; then
  CONTAINER_RT=docker
elif command -v podman &>/dev/null; then
  CONTAINER_RT=podman
else
  fail "Neither docker nor podman found"
  exit 1
fi

# ── psql helper ─────────────────────────────────────────────────
# Runs a SQL command against the flowplane-postgres container.
psql_exec() {
  $CONTAINER_RT exec flowplane-postgres psql -U flowplane -d flowplane -tAc "$1"
}

# ── Wait for Zitadel ─────────────────────────────────────────────
wait_for_zitadel() {
  log "Waiting for Zitadel at ${ZITADEL_HOST}..."
  local attempts=0
  while ! curl -sf "${ZITADEL_HOST}/debug/ready" > /dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ $attempts -ge 90 ]; then
      fail "Zitadel not ready after 90s"
      exit 1
    fi
    sleep 1
  done
  ok "Zitadel is ready"
}

# ── Wait for Flowplane ───────────────────────────────────────────
wait_for_flowplane() {
  log "Waiting for Flowplane at ${FLOWPLANE_URL}..."
  local attempts=0
  while ! curl -sf "${FLOWPLANE_URL}/swagger-ui/" > /dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ $attempts -ge 90 ]; then
      fail "Flowplane not reachable after 90s"
      exit 1
    fi
    sleep 1
  done
  ok "Flowplane is ready"
}

# ── Read PAT ───────────────────────────────────────────────────
read_pat() {
  log "Waiting for admin PAT at ${PAT_FILE}..."
  local attempts=0
  while [ ! -s "$PAT_FILE" ]; do
    attempts=$((attempts + 1))
    if [ $attempts -ge 60 ]; then
      fail "PAT file not written after 60s: $PAT_FILE"
      echo "  Check Zitadel logs: docker logs flowplane-zitadel" >&2
      exit 1
    fi
    sleep 1
  done
  ZITADEL_PAT=$(cat "$PAT_FILE" | tr -d '[:space:]')
  ok "PAT loaded (${#ZITADEL_PAT} chars)"

  # Validate PAT works before proceeding
  log "Validating PAT against Zitadel API..."
  local valid_attempts=0
  while true; do
    local code
    code=$(curl -s -o /dev/null -w '%{http_code}' \
      -H "Authorization: Bearer ${ZITADEL_PAT}" \
      "${ZITADEL_HOST}/management/v1/projects/_search" \
      -X POST -H "Content-Type: application/json" -d '{"queries":[]}')
    if [ "$code" = "200" ]; then
      ok "PAT validated successfully"
      return
    fi
    valid_attempts=$((valid_attempts + 1))
    if [ $valid_attempts -ge 30 ]; then
      fail "PAT validation failed after 30 attempts (last HTTP ${code})"
      exit 1
    fi
    sleep 2
  done
}

# ── Create human user ────────────────────────────────────────────
HUMAN_USER_ID=""

create_human_user() {
  log "Creating human user '${HUMAN_USERNAME}'..."
  api POST /v2/users/human "{
    \"username\": \"${HUMAN_USERNAME}\",
    \"profile\": {
      \"givenName\": \"${HUMAN_FIRST}\",
      \"familyName\": \"${HUMAN_LAST}\"
    },
    \"email\": {
      \"email\": \"${HUMAN_USERNAME}\",
      \"isVerified\": true
    },
    \"password\": {
      \"password\": \"${HUMAN_PASSWORD}\",
      \"changeRequired\": false
    }
  }"

  if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
    HUMAN_USER_ID=$(echo "$BODY" | jq -r '.userId')
    ok "Human user created (id: ${HUMAN_USER_ID})"
  elif [ "$HTTP_CODE" = "409" ] || echo "$BODY" | grep -qi "already exists"; then
    # Look up existing user
    api POST /v2/users '{"queries":[{"userNameQuery":{"userName":"'"${HUMAN_USERNAME}"'"}}]}'
    HUMAN_USER_ID=$(echo "$BODY" | jq -r '.result[0].userId // .result[0].id' 2>/dev/null | head -1)
    if [ -z "$HUMAN_USER_ID" ] || [ "$HUMAN_USER_ID" = "null" ]; then
      fail "Human user exists but could not find its ID"
      exit 1
    fi
    skip "Human user '${HUMAN_USERNAME}' (id: ${HUMAN_USER_ID})"
  else
    fail "Create human user failed (HTTP ${HTTP_CODE}): ${BODY}"
    exit 1
  fi
}

# ── Create machine user ──────────────────────────────────────────
MACHINE_USER_ID=""

create_machine_user() {
  log "Creating machine user '${MACHINE_USERNAME}'..."
  api POST /management/v1/users/machine "{
    \"userName\": \"${MACHINE_USERNAME}\",
    \"name\": \"${MACHINE_NAME}\",
    \"description\": \"Flowplane agent service account\",
    \"accessTokenType\": \"ACCESS_TOKEN_TYPE_JWT\"
  }"

  if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
    MACHINE_USER_ID=$(echo "$BODY" | jq -r '.userId')
    ok "Machine user created (id: ${MACHINE_USER_ID})"
  elif [ "$HTTP_CODE" = "409" ] || echo "$BODY" | grep -qi "already exists"; then
    api POST /v2/users '{"queries":[{"userNameQuery":{"userName":"'"${MACHINE_USERNAME}"'"}}]}'
    MACHINE_USER_ID=$(echo "$BODY" | jq -r '.result[0].userId // .result[0].id' 2>/dev/null | head -1)
    if [ -z "$MACHINE_USER_ID" ] || [ "$MACHINE_USER_ID" = "null" ]; then
      fail "Machine user exists but could not find its ID"
      exit 1
    fi
    skip "Machine user '${MACHINE_USERNAME}' (id: ${MACHINE_USER_ID})"
  else
    fail "Create machine user failed (HTTP ${HTTP_CODE}): ${BODY}"
    exit 1
  fi
}

# ── Generate machine secret ──────────────────────────────────────
MACHINE_CLIENT_ID=""
MACHINE_CLIENT_SECRET=""

generate_machine_secret() {
  log "Generating client credentials for machine user..."
  api PUT "/management/v1/users/${MACHINE_USER_ID}/secret" '{}'

  if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
    MACHINE_CLIENT_ID=$(echo "$BODY" | jq -r '.clientId')
    MACHINE_CLIENT_SECRET=$(echo "$BODY" | jq -r '.clientSecret')
    ok "Client credentials generated (clientId: ${MACHINE_CLIENT_ID})"
  else
    fail "Generate machine secret failed (HTTP ${HTTP_CODE}): ${BODY}"
    exit 1
  fi
}

# ── Bootstrap demo org ───────────────────────────────────────────
bootstrap_demo_org() {
  log "Bootstrapping org '${ORG_NAME}' with team '${TEAM_NAME}'..."
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Content-Type: application/json" \
    -d "{\"orgName\": \"${ORG_NAME}\", \"displayName\": \"${ORG_DISPLAY}\", \"teamName\": \"${TEAM_NAME}\"}" \
    "${FLOWPLANE_URL}/api/v1/bootstrap/initialize")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "$http_code" = "201" ]; then
    ok "Org '${ORG_NAME}' and team '${TEAM_NAME}' created"
  elif [ "$http_code" = "409" ]; then
    skip "Org '${ORG_NAME}'"
  else
    fail "Bootstrap failed (HTTP ${http_code}): ${body}"
    exit 1
  fi
}

# ── Seed human user permissions ──────────────────────────────────
seed_human_permissions() {
  log "Seeding DB permissions for human user '${HUMAN_USERNAME}'..."

  # The Zitadel userId IS the sub claim
  local sub="${HUMAN_USER_ID}"

  # Upsert user row
  psql_exec "INSERT INTO users (id, email, password_hash, name, status, is_admin, zitadel_sub)
    VALUES (gen_random_uuid()::TEXT, '${HUMAN_USERNAME}', '', '${HUMAN_FIRST} ${HUMAN_LAST}', 'active', false, '${sub}')
    ON CONFLICT (zitadel_sub) DO NOTHING;"
  ok "User row upserted"

  # Get org_id
  local org_id
  org_id=$(psql_exec "SELECT id FROM organizations WHERE name = '${ORG_NAME}';")
  if [ -z "$org_id" ]; then
    fail "Organization '${ORG_NAME}' not found in DB"
    exit 1
  fi

  # Get user_id
  local user_id
  user_id=$(psql_exec "SELECT id FROM users WHERE zitadel_sub = '${sub}';")

  # Get team_id (FK requires UUID, not name)
  local team_id
  team_id=$(psql_exec "SELECT id FROM teams WHERE name = '${TEAM_NAME}' AND org_id = '${org_id}';")
  if [ -z "$team_id" ]; then
    fail "Team '${TEAM_NAME}' not found in org '${ORG_NAME}'"
    exit 1
  fi

  # Org membership (admin)
  psql_exec "INSERT INTO organization_memberships (id, user_id, org_id, role)
    VALUES (gen_random_uuid()::TEXT, '${user_id}', '${org_id}', 'admin')
    ON CONFLICT (user_id, org_id) DO NOTHING;"
  ok "Org membership created (admin)"

  # Team membership with full scopes
  local scopes='["clusters:read","clusters:write","routes:read","routes:write","listeners:read","listeners:write","filters:read","filters:write","learning:read","learning:write","secrets:read","secrets:write"]'
  psql_exec "INSERT INTO user_team_memberships (id, user_id, team, scopes)
    VALUES (gen_random_uuid()::TEXT, '${user_id}', '${team_id}', '${scopes}')
    ON CONFLICT (user_id, team) DO NOTHING;"
  ok "Team membership created (${TEAM_NAME})"

  # TODO: Replace with POST /api/v1/admin/organizations/{org}/invite once Phase B lands
}

# ── Seed machine user permissions ────────────────────────────────
seed_machine_permissions() {
  log "Seeding DB permissions for machine user '${MACHINE_USERNAME}'..."

  # Machine user sub = the Zitadel userId
  local sub="${MACHINE_USER_ID}"

  # Upsert user row
  psql_exec "INSERT INTO users (id, email, password_hash, name, status, is_admin, zitadel_sub)
    VALUES (gen_random_uuid()::TEXT, 'flowplane-agent@machine.local', '', '${MACHINE_NAME}', 'active', false, '${sub}')
    ON CONFLICT (zitadel_sub) DO NOTHING;"
  ok "User row upserted"

  # Get org_id
  local org_id
  org_id=$(psql_exec "SELECT id FROM organizations WHERE name = '${ORG_NAME}';")
  if [ -z "$org_id" ]; then
    fail "Organization '${ORG_NAME}' not found in DB"
    exit 1
  fi

  # Get user_id
  local user_id
  user_id=$(psql_exec "SELECT id FROM users WHERE zitadel_sub = '${sub}';")

  # Get team_id (FK requires UUID, not name)
  local team_id
  team_id=$(psql_exec "SELECT id FROM teams WHERE name = '${TEAM_NAME}' AND org_id = '${org_id}';")
  if [ -z "$team_id" ]; then
    fail "Team '${TEAM_NAME}' not found in org '${ORG_NAME}'"
    exit 1
  fi

  # Org membership (admin)
  psql_exec "INSERT INTO organization_memberships (id, user_id, org_id, role)
    VALUES (gen_random_uuid()::TEXT, '${user_id}', '${org_id}', 'admin')
    ON CONFLICT (user_id, org_id) DO NOTHING;"
  ok "Org membership created (admin)"

  # Team membership with full scopes
  local scopes='["clusters:read","clusters:write","routes:read","routes:write","listeners:read","listeners:write","filters:read","filters:write","learning:read","learning:write","secrets:read","secrets:write"]'
  psql_exec "INSERT INTO user_team_memberships (id, user_id, team, scopes)
    VALUES (gen_random_uuid()::TEXT, '${user_id}', '${team_id}', '${scopes}')
    ON CONFLICT (user_id, team) DO NOTHING;"
  ok "Team membership created (${TEAM_NAME})"

  # TODO: Replace with POST /api/v1/admin/organizations/{org}/invite once Phase B lands
}

# ── Main ───────────────────────────────────────────────────────
main() {
  echo ""
  echo -e "${CYAN}━━━ Flowplane Demo Seed ━━━${RESET}"
  echo ""

  wait_for_zitadel
  wait_for_flowplane
  read_pat

  create_human_user
  create_machine_user
  generate_machine_secret
  bootstrap_demo_org
  seed_human_permissions
  seed_machine_permissions

  echo ""
  echo -e "${GREEN}━━━ Seed complete ━━━${RESET}"
  echo ""
  echo -e "  ${BOLD}Flowplane UI:${RESET}"
  echo -e "    URL:       ${CYAN}${FLOWPLANE_URL}${RESET}"
  echo -e "    Login:     ${CYAN}${HUMAN_USERNAME}${RESET} / ${CYAN}${HUMAN_PASSWORD}${RESET}"
  echo -e "    Org:       ${CYAN}${ORG_NAME}${RESET}"
  echo -e "    Team:      ${CYAN}${TEAM_NAME}${RESET}"
  echo ""
  echo -e "  ${BOLD}Machine User:${RESET}"
  echo -e "    Client ID:     ${CYAN}${MACHINE_CLIENT_ID}${RESET}"
  echo -e "    Client Secret: ${CYAN}${MACHINE_CLIENT_SECRET}${RESET}"
  echo -e "    Token URL:     ${CYAN}${ZITADEL_HOST}/oauth/v2/token${RESET}"
  echo ""
  echo -e "  ${BOLD}Superadmin:${RESET}"
  echo -e "    Login:     ${CYAN}admin@flowplane.local${RESET} (seeded automatically on startup)"
  echo ""
}

main "$@"
