#!/usr/bin/env bash
# Seed demo data for local Flowplane development
#
# Creates: demo org (acme-corp), demo user, machine user, DB permissions
# Prerequisites: curl, jq, running Zitadel + Flowplane (run 'make up' first)
# Usage: ./scripts/seed-demo.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Source auth helper (get_oidc_token) and env vars
# shellcheck source=lib/zitadel-auth.sh
source "${SCRIPT_DIR}/lib/zitadel-auth.sh"

# Load Zitadel env (CLIENT_ID, PROJECT_ID)
if [ -f "${PROJECT_DIR}/.env.zitadel" ]; then
  # shellcheck source=/dev/null
  source "${PROJECT_DIR}/.env.zitadel"
fi

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

# Superadmin email (must match docker-compose.yml FLOWPLANE_SUPERADMIN_EMAIL)
SUPERADMIN_EMAIL="${FLOWPLANE_SUPERADMIN_EMAIL:-admin@flowplane.local}"
ADMIN_TOKEN=""

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

# ── Wait for superadmin ────────────────────────────────────────
# The superadmin user is seeded asynchronously on first startup.
# Poll Zitadel until the superadmin user exists, then obtain a real
# OIDC access token via the Session API flow (human users cannot get PATs).
SUPERADMIN_PASSWORD="${FLOWPLANE_SUPERADMIN_INITIAL_PASSWORD:-Flowplane1!}"

wait_for_superadmin() {
  log "Waiting for superadmin seeding..."

  # Poll until the superadmin user appears in Zitadel
  local attempts=0 superadmin_id=""
  while true; do
    api POST /v2/users '{"queries":[{"emailQuery":{"emailAddress":"'"${SUPERADMIN_EMAIL}"'"}}]}'
    superadmin_id=$(echo "$BODY" | jq -r '.result[0].userId // .result[0].id // empty' 2>/dev/null | head -1)
    if [ -n "$superadmin_id" ] && [ "$superadmin_id" != "null" ]; then
      ok "Superadmin found in Zitadel (id: ${superadmin_id})"
      break
    fi
    attempts=$((attempts + 1))
    if [ $attempts -ge 30 ]; then
      fail "Superadmin user not found after 30 attempts"
      exit 1
    fi
    sleep 2
  done

  # Obtain OIDC token via Session API + authorize flow
  log "Obtaining OIDC token for superadmin..."
  ADMIN_TOKEN=$(get_oidc_token "$SUPERADMIN_EMAIL" "$SUPERADMIN_PASSWORD")
  if [ -z "$ADMIN_TOKEN" ]; then
    fail "Failed to obtain OIDC token for superadmin"
    exit 1
  fi
  ok "Superadmin OIDC token obtained"
}

# ── Invite human user via API ────────────────────────────────
invite_user() {
  local org_id="$1" email="$2" role="$3" first="$4" last="$5"

  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "{\"email\": \"${email}\", \"role\": \"${role}\", \"firstName\": \"${first}\", \"lastName\": \"${last}\"}" \
    "${FLOWPLANE_URL}/api/v1/admin/organizations/${org_id}/invite")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "$http_code" = "201" ]; then
    ok "User '${email}' invited to org (role: ${role})"
  elif [ "$http_code" = "200" ]; then
    skip "User '${email}' already a member (role: ${role})"
  else
    fail "Invite failed (HTTP ${http_code}): ${body}"
    exit 1
  fi
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

# ── Seed machine user permissions ────────────────────────────────
# TODO: Replace with POST /api/v1/orgs/{org}/agents once Phase C lands
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

  # TODO: Replace with POST /api/v1/orgs/{org}/agents once Phase C lands
}

# ── Main ───────────────────────────────────────────────────────
main() {
  echo ""
  echo -e "${CYAN}━━━ Flowplane Demo Seed ━━━${RESET}"
  echo ""

  wait_for_zitadel
  wait_for_flowplane
  read_pat

  # Machine user is created in Zitadel directly (no invite API yet)
  create_machine_user
  generate_machine_secret

  # Wait for superadmin to be seeded (async on first startup)
  wait_for_superadmin

  # Bootstrap creates the org + team in the Flowplane DB
  bootstrap_demo_org

  # Get org_id via API for the invite call
  log "Looking up org '${ORG_NAME}'..."
  local org_list_body
  org_list_body=$(curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    "${FLOWPLANE_URL}/api/v1/admin/organizations")
  ORG_ID=$(echo "$org_list_body" | jq -r ".items[] | select(.name==\"${ORG_NAME}\") | .id")
  if [ -z "$ORG_ID" ] || [ "$ORG_ID" = "null" ]; then
    fail "Could not find org '${ORG_NAME}' via API"
    exit 1
  fi
  ok "Org '${ORG_NAME}' id: ${ORG_ID}"

  # Invite human user via the API (creates Zitadel user + local DB records)
  log "Inviting human user '${HUMAN_USERNAME}'..."
  invite_user "$ORG_ID" "$HUMAN_USERNAME" "admin" "$HUMAN_FIRST" "$HUMAN_LAST"

  # Set the demo user's password in Zitadel (invite creates user without password)
  log "Setting password for demo user..."
  api POST /v2/users '{"queries":[{"emailQuery":{"emailAddress":"'"${HUMAN_USERNAME}"'"}}]}'
  local demo_user_id
  demo_user_id=$(echo "$BODY" | jq -r '.result[0].userId // .result[0].id // empty' 2>/dev/null | head -1)
  if [ -n "$demo_user_id" ] && [ "$demo_user_id" != "null" ]; then
    local pw_body
    pw_body=$(jq -n --arg p "$HUMAN_PASSWORD" '{newPassword: {password: $p, changeRequired: false}}')
    api POST "/v2/users/${demo_user_id}/password" "$pw_body"
    if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
      ok "Password set for ${HUMAN_USERNAME}"
    else
      fail "Failed to set password (HTTP ${HTTP_CODE}): ${BODY}"
    fi
  else
    fail "Could not find demo user in Zitadel to set password"
  fi

  # Machine user DB permissions still via psql_exec (until Phase C)
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
