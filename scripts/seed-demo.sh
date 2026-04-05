#!/usr/bin/env bash
# Seed demo data for local Flowplane development
#
# Creates: demo org (acme-corp), demo user, machine user (via agent API)
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

# Demo human user (org admin)
HUMAN_USERNAME="demo@acme-corp.com"
HUMAN_FIRST="Demo"
HUMAN_LAST="User"
HUMAN_PASSWORD="${FLOWPLANE_DEMO_PASSWORD:-Flowplane1!}"  # pragma: allowlist secret

# Superadmin email (must match docker-compose.yml FLOWPLANE_SUPERADMIN_EMAIL)
SUPERADMIN_EMAIL="${FLOWPLANE_SUPERADMIN_EMAIL:-admin@flowplane.local}"
ADMIN_TOKEN=""

# Machine user / agent
AGENT_NAME="flowplane-agent"
AGENT_CLIENT_ID=""
AGENT_CLIENT_SECRET=""
AGENT_ID=""

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

  # Reset superadmin password via admin PAT (idempotent — handles case where
  # Playwright tests or a previous run changed the password)
  log "Resetting superadmin password..."
  local pw_body
  pw_body=$(jq -n --arg p "$SUPERADMIN_PASSWORD" '{"newPassword":{"password":$p,"changeRequired":false}}')
  local pw_raw pw_code
  pw_raw=$(curl -s -w '\n%{http_code}' -X POST \
    -H "Authorization: Bearer ${ZITADEL_PAT}" \
    -H "Content-Type: application/json" \
    -d "$pw_body" \
    "${ZITADEL_HOST}/v2/users/${superadmin_id}/password")
  pw_code=$(echo "$pw_raw" | tail -1)
  if [ "$pw_code" = "200" ] || [ "$pw_code" = "201" ]; then
    ok "Superadmin password reset"
  else
    log "Password reset returned HTTP ${pw_code} (non-fatal)"
  fi

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
    -d "{\"email\": \"${email}\", \"role\": \"${role}\", \"firstName\": \"${first}\", \"lastName\": \"${last}\", \"initialPassword\": \"${HUMAN_PASSWORD}\"}" \
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

# ── Create demo org via admin API ────────────────────────────────
# Uses the platform admin token to create the org. Returns org ID in ORG_ID.
# Idempotent: if the org already exists (409), looks it up by name.
create_demo_org() {
  log "Creating org '${ORG_NAME}' via admin API..."
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg name "$ORG_NAME" \
      --arg display "$ORG_DISPLAY" \
      '{name: $name, displayName: $display}')" \
    "${FLOWPLANE_URL}/api/v1/admin/organizations")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "$http_code" = "201" ]; then
    ORG_ID=$(echo "$body" | jq -r '.id')
    ok "Org '${ORG_NAME}' created (id: ${ORG_ID})"
  elif [ "$http_code" = "409" ]; then
    # Org exists — look it up
    local list_body
    list_body=$(curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" \
      "${FLOWPLANE_URL}/api/v1/admin/organizations")
    ORG_ID=$(echo "$list_body" | jq -r ".items[] | select(.name==\"${ORG_NAME}\") | .id")
    if [ -z "$ORG_ID" ] || [ "$ORG_ID" = "null" ]; then
      fail "Org '${ORG_NAME}' exists (409) but could not resolve its ID"
      exit 1
    fi
    skip "Org '${ORG_NAME}' (id: ${ORG_ID})"
  else
    fail "Org creation failed (HTTP ${http_code}): ${body}"
    exit 1
  fi
}

# ── Create agent via Flowplane API ───────────────────────────────
# Requires org admin Bearer token in ADMIN_TOKEN.
create_agent() {
  local org_name="$1" agent_name="$2" description="$3" teams_json="$4"

  log "Provisioning agent '${agent_name}' in org '${org_name}'..."
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg name "$agent_name" \
      --arg desc "$description" \
      --argjson teams "$teams_json" \
      '{name: $name, description: $desc, teams: $teams}')" \
    "${FLOWPLANE_URL}/api/v1/orgs/${org_name}/agents")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "$http_code" = "201" ]; then
    ok "Agent '${agent_name}' created in org '${org_name}'"
    AGENT_CLIENT_ID=$(echo "$body" | jq -r '.clientId // empty')
    AGENT_CLIENT_SECRET=$(echo "$body" | jq -r '.clientSecret // empty')
    AGENT_ID=$(echo "$body" | jq -r '.agentId // empty')
  elif [ "$http_code" = "200" ]; then
    # Idempotent: agent exists — extract agentId so grants can still be created.
    # Credentials are only returned on first creation (201), not on re-provision (200).
    AGENT_ID=$(echo "$body" | jq -r '.agentId // empty')
    skip "Agent '${agent_name}' already exists in '${org_name}'"
  else
    fail "Agent creation failed (HTTP ${http_code}): ${body}"
    exit 1
  fi
}

# ── Resolve team name to UUID ─────────────────────────────────────
get_team_id() {
  local org_name="$1" team_name="$2"
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    "${FLOWPLANE_URL}/api/v1/orgs/${org_name}/teams")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "$http_code" = "200" ]; then
    echo "$body" | jq -r ".teams[] | select(.name==\"${team_name}\") | .id // empty" 2>/dev/null | head -1
  else
    fail "Failed to list teams in org '${org_name}' (HTTP ${http_code}): ${body}"
    exit 1
  fi
}

# ── Create grants for agent ───────────────────────────────────────
# Creates resource grants for each resource:action pair.
# Idempotent: handles 409 (duplicate grant) gracefully.
create_agent_grants() {
  local org_name="$1" agent_id="$2" team_id="$3"

  local grants=(
    "clusters:read"
    "clusters:create"
    "clusters:update"
    "routes:read"
    "routes:create"
    "routes:update"
    "listeners:read"
    "listeners:create"
    "listeners:update"
    "filters:read"
    "filters:create"
    "filters:update"
    "learning-sessions:read"
    "learning-sessions:create"
    "learning-sessions:execute"
    "secrets:read"
    "secrets:create"
    "secrets:update"
    "secrets:delete"
  )

  log "Creating grants for agent '${agent_id}' in org '${org_name}'..."
  local created=0 skipped=0
  for grant in "${grants[@]}"; do
    local resource_type action
    resource_type="${grant%%:*}"
    action="${grant##*:}"

    local raw http_code body
    raw=$(curl -s -w '\n%{http_code}' \
      -X POST \
      -H "Authorization: Bearer ${ADMIN_TOKEN}" \
      -H "Content-Type: application/json" \
      -d "$(jq -n \
        --arg team "$team_id" \
        --arg resourceType "$resource_type" \
        --arg action "$action" \
        '{"grantType": "resource", "team": $team, "resourceType": $resourceType, "action": $action}')" \
      "${FLOWPLANE_URL}/api/v1/orgs/${org_name}/principals/${agent_id}/grants")
    http_code=$(echo "$raw" | tail -1)
    body=$(echo "$raw" | sed '$d')

    if [ "$http_code" = "201" ]; then
      created=$((created + 1))
    elif [ "$http_code" = "409" ]; then
      skipped=$((skipped + 1))
    else
      fail "Grant creation failed for ${grant} (HTTP ${http_code}): ${body}"
      exit 1
    fi
  done
  ok "Grants: ${created} created, ${skipped} already existed"
}

# ── Create dataplane ────────────────────────────────────────────
# Creates a dataplane for a team so that expose/listeners work.
# Idempotent: handles 409 (already exists) gracefully.
create_dataplane() {
  local team="$1" name="$2" description="$3"

  log "Creating dataplane '${name}' for team '${team}'..."
  local raw http_code body
  raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg name "$name" \
      --arg desc "$description" \
      '{name: $name, description: $desc}')" \
    "${FLOWPLANE_URL}/api/v1/teams/${team}/dataplanes")
  http_code=$(echo "$raw" | tail -1)
  body=$(echo "$raw" | sed '$d')

  if [ "$http_code" = "201" ]; then
    ok "Dataplane '${name}' created for team '${team}'"
  elif [ "$http_code" = "409" ]; then
    skip "Dataplane '${name}' in team '${team}'"
  else
    fail "Dataplane creation failed (HTTP ${http_code}): ${body}"
    exit 1
  fi
}

# ── Main ───────────────────────────────────────────────────────
main() {
  echo ""
  echo -e "${CYAN}━━━ Flowplane Demo Seed ━━━${RESET}"
  echo ""

  # Check if server is running in dev mode (no Zitadel, auto-seeded)
  local auth_mode
  auth_mode=$(curl -sf --max-time 5 "${FLOWPLANE_URL}/api/v1/auth/mode" 2>/dev/null | jq -r '.auth_mode // empty' 2>/dev/null)
  if [ "$auth_mode" = "dev" ]; then
    echo -e "${GREEN}Dev mode detected — data is auto-seeded on startup.${RESET}"
    echo -e "  User:  ${CYAN}dev@flowplane.local${RESET}"
    echo -e "  Org:   ${CYAN}dev-org${RESET}"
    echo -e "  Team:  ${CYAN}default${RESET}"
    echo -e "  Token: ${CYAN}~/.flowplane/credentials${RESET}"
    echo ""
    echo -e "  ${YELLOW}No manual seeding needed. Run 'make seed-info' for details.${RESET}"
    echo ""
    exit 0
  fi

  wait_for_zitadel
  wait_for_flowplane
  read_pat

  # Wait for superadmin to be seeded (async on first startup)
  wait_for_superadmin

  # Platform admin creates the org
  create_demo_org

  # Platform admin invites the org admin user (with initial password)
  log "Inviting human user '${HUMAN_USERNAME}'..."
  invite_user "$ORG_ID" "$HUMAN_USERNAME" "admin" "$HUMAN_FIRST" "$HUMAN_LAST"

  # Authenticate as org admin to provision agent
  log "Obtaining OIDC token for org admin '${HUMAN_USERNAME}'..."
  local org_admin_token
  org_admin_token=$(get_oidc_token "$HUMAN_USERNAME" "$HUMAN_PASSWORD")
  if [ -z "$org_admin_token" ]; then
    fail "Failed to obtain OIDC token for org admin"
    exit 1
  fi
  ok "Org admin token obtained"

  # Switch to org admin token for remaining operations
  ADMIN_TOKEN="$org_admin_token"

  # Org admin creates the team
  log "Creating team '${TEAM_NAME}' in org '${ORG_NAME}'..."
  local team_raw team_code team_body
  team_raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg name "$TEAM_NAME" \
      --arg display "${TEAM_NAME}" \
      '{name: $name, displayName: $display}')" \
    "${FLOWPLANE_URL}/api/v1/orgs/${ORG_NAME}/teams")
  team_code=$(echo "$team_raw" | tail -1)
  team_body=$(echo "$team_raw" | sed '$d')
  if [ "$team_code" = "201" ]; then
    ok "Team '${TEAM_NAME}' created in org '${ORG_NAME}'"
  elif [ "$team_code" = "409" ]; then
    skip "Team '${TEAM_NAME}' in org '${ORG_NAME}'"
  else
    fail "Team creation failed (HTTP ${team_code}): ${team_body}"
    exit 1
  fi

  # Create default dataplane for the engineering team (required for expose)
  create_dataplane "${TEAM_NAME}" "default-dataplane" "Default dataplane for ${TEAM_NAME}"

  # Provision agent via the Flowplane API (org admin required)
  create_agent \
    "${ORG_NAME}" \
    "${AGENT_NAME}" \
    "Flowplane agent service account" \
    '["'"${TEAM_NAME}"'"]'

  # Always attempt to create grants — idempotent (409 = already exists).
  # This ensures grants are created even on re-runs where the agent already existed.
  if [ -z "${AGENT_ID}" ]; then
    fail "Agent ID not available — cannot create grants"
    exit 1
  fi
  log "Resolving team '${TEAM_NAME}' for agent grants..."
  local team_id
  team_id=$(get_team_id "${ORG_NAME}" "${TEAM_NAME}")
  if [ -z "$team_id" ] || [ "$team_id" = "null" ]; then
    fail "Could not resolve team '${TEAM_NAME}' to a UUID"
    exit 1
  fi
  ok "Team '${TEAM_NAME}' id: ${team_id}"
  create_agent_grants "${ORG_NAME}" "${AGENT_ID}" "${team_id}"

  echo ""
  echo -e "${GREEN}━━━ Seed complete ━━━${RESET}"
  echo ""
  echo -e "  ${BOLD}Flowplane UI:${RESET}"
  echo -e "    URL:       ${CYAN}${FLOWPLANE_URL}${RESET}"
  echo -e "    Login:     ${CYAN}${HUMAN_USERNAME}${RESET} / ${CYAN}${HUMAN_PASSWORD}${RESET}"
  echo -e "    Org:       ${CYAN}${ORG_NAME}${RESET}"
  echo -e "    Team:      ${CYAN}${TEAM_NAME}${RESET}"
  echo ""
  echo -e "  ${BOLD}Agent (${AGENT_NAME}):${RESET}"
  if [ -n "${AGENT_CLIENT_ID}" ]; then
    echo -e "    Client ID:     ${CYAN}${AGENT_CLIENT_ID}${RESET}"
    echo -e "    Client Secret: ${CYAN}${AGENT_CLIENT_SECRET}${RESET}"
    echo -e "    Token URL:     ${CYAN}${ZITADEL_HOST}/oauth/v2/token${RESET}"
  else
    echo -e "    ${YELLOW}Credentials were returned at first creation only.${RESET}"
  fi
  echo -e "    Grants:        clusters/routes/listeners/filters/learning-sessions/secrets (read,create,update)"
  echo ""
  echo -e "  ${BOLD}Superadmin:${RESET}"
  echo -e "    Login:     ${CYAN}admin@flowplane.local${RESET} (seeded automatically on startup)"
  echo ""
}

main "$@"
