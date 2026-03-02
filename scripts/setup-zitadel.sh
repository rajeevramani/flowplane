#!/usr/bin/env bash
# Setup Zitadel for local Flowplane development
#
# Creates: Zitadel project + SPA app, writes env files, restarts control-plane
# Prerequisites: curl, jq
# Usage: ./scripts/setup-zitadel.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8081}"
FLOWPLANE_URL="${FLOWPLANE_URL:-http://localhost:8080}"
PAT_FILE="${PROJECT_DIR}/zitadel/machinekey/admin-pat.txt"

# Defaults
PROJECT_NAME="Flowplane"
SPA_APP_NAME="Flowplane UI"
SPA_REDIRECT_DOCKER="http://localhost:8080/auth/callback"
SPA_REDIRECT_DEV="http://localhost:5173/auth/callback"
SPA_POST_LOGOUT_DOCKER="http://localhost:8080"
SPA_POST_LOGOUT_DEV="http://localhost:5173"

# Colors
CYAN='\033[36m'
GREEN='\033[32m'
YELLOW='\033[33m'
RED='\033[31m'
BOLD='\033[1m'
RESET='\033[0m'

log()  { echo -e "${CYAN}[zitadel]${RESET} $*"; }
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
  local tmp
  tmp=$(mktemp)
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
  rm -f "$tmp"
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
  ok "Zitadel is ready (projections caught up)"
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

# ── Create project ────────────────────────────────────────────────
create_project() {
  log "Creating project '${PROJECT_NAME}'..."
  api POST /management/v1/projects \
    "{\"name\": \"${PROJECT_NAME}\", \"projectRoleAssertion\": true}"

  if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
    PROJECT_ID=$(echo "$BODY" | jq -r '.id')
    ok "Project created (id: ${PROJECT_ID})"
  elif [ "$HTTP_CODE" = "409" ] || echo "$BODY" | grep -qi "already exists"; then
    # Find existing project
    api POST "/management/v1/projects/_search" '{"queries":[]}'
    PROJECT_ID=$(echo "$BODY" | jq -r ".result[] | select(.name==\"${PROJECT_NAME}\") | .id" 2>/dev/null | head -1)
    if [ -z "$PROJECT_ID" ] || [ "$PROJECT_ID" = "null" ]; then
      fail "Project exists but could not find its ID"
      exit 1
    fi
    skip "Project '${PROJECT_NAME}' (id: ${PROJECT_ID})"
  else
    fail "Create project failed (HTTP ${HTTP_CODE}): ${BODY}"
    exit 1
  fi
}

# ── Create SPA application ────────────────────────────────────────
create_spa_app() {
  log "Creating SPA application '${SPA_APP_NAME}'..."
  api POST "/management/v1/projects/${PROJECT_ID}/apps/oidc" "{
    \"name\": \"${SPA_APP_NAME}\",
    \"redirectUris\": [\"${SPA_REDIRECT_DOCKER}\", \"${SPA_REDIRECT_DEV}\"],
    \"postLogoutRedirectUris\": [\"${SPA_POST_LOGOUT_DOCKER}\", \"${SPA_POST_LOGOUT_DEV}\"],
    \"responseTypes\": [\"OIDC_RESPONSE_TYPE_CODE\"],
    \"grantTypes\": [\"OIDC_GRANT_TYPE_AUTHORIZATION_CODE\"],
    \"appType\": \"OIDC_APP_TYPE_USER_AGENT\",
    \"authMethodType\": \"OIDC_AUTH_METHOD_TYPE_NONE\",
    \"accessTokenType\": \"OIDC_TOKEN_TYPE_JWT\",
    \"devMode\": true
  }"

  if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
    SPA_CLIENT_ID=$(echo "$BODY" | jq -r '.clientId')
    ok "SPA app created (clientId: ${SPA_CLIENT_ID})"
  elif [ "$HTTP_CODE" = "409" ] || echo "$BODY" | grep -qi "already exists"; then
    # Find existing app
    api POST "/management/v1/projects/${PROJECT_ID}/apps/_search" '{"queries":[]}'
    SPA_CLIENT_ID=$(echo "$BODY" | jq -r ".result[] | select(.name==\"${SPA_APP_NAME}\") | .oidcConfig.clientId" 2>/dev/null | head -1)
    if [ -z "$SPA_CLIENT_ID" ] || [ "$SPA_CLIENT_ID" = "null" ]; then
      fail "SPA app exists but could not find its client ID"
      exit 1
    fi
    skip "SPA app '${SPA_APP_NAME}' (clientId: ${SPA_CLIENT_ID})"
  else
    fail "Create SPA app failed (HTTP ${HTTP_CODE}): ${BODY}"
    exit 1
  fi
}

# ── Write env files ───────────────────────────────────────────────
write_env_files() {
  log "Writing .env.zitadel..."
  cat > "${PROJECT_DIR}/.env.zitadel" <<EOF
# Generated by scripts/setup-zitadel.sh — do not commit
ZITADEL_PROJECT_ID=${PROJECT_ID}
ZITADEL_ADMIN_PAT=${ZITADEL_PAT}
FLOWPLANE_ZITADEL_PROJECT_ID=${PROJECT_ID}
FLOWPLANE_ZITADEL_ADMIN_PAT=${ZITADEL_PAT}
FLOWPLANE_ZITADEL_SPA_CLIENT_ID=${SPA_CLIENT_ID}
EOF
  chmod 600 "${PROJECT_DIR}/.env.zitadel"
  ok "Wrote .env.zitadel"

  log "Writing ui/.env..."
  cat > "${PROJECT_DIR}/ui/.env" <<EOF
# Generated by scripts/setup-zitadel.sh — do not commit
PUBLIC_API_BASE=http://localhost:8080
VITE_ZITADEL_ISSUER=http://localhost:8081
VITE_ZITADEL_CLIENT_ID=${SPA_CLIENT_ID}
VITE_APP_URL=http://localhost:5173
EOF
  ok "Wrote ui/.env"
}

# ── Restart control-plane with project ID ─────────────────────────
restart_control_plane() {
  log "Restarting control-plane to pick up ZITADEL_PROJECT_ID..."
  docker-compose -f "${PROJECT_DIR}/docker-compose.yml" up -d --force-recreate control-plane 2>/dev/null \
    || docker compose -f "${PROJECT_DIR}/docker-compose.yml" up -d --force-recreate control-plane 2>/dev/null
  ok "Control-plane restarting"

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

# ── Main ───────────────────────────────────────────────────────
main() {
  echo ""
  echo -e "${CYAN}━━━ Flowplane Zitadel Setup ━━━${RESET}"
  echo ""

  PROJECT_ID=""
  SPA_CLIENT_ID=""

  wait_for_zitadel
  read_pat

  create_project
  create_spa_app

  write_env_files
  restart_control_plane

  echo ""
  echo -e "${GREEN}━━━ Setup complete ━━━${RESET}"
  echo ""
  echo -e "  ${BOLD}Zitadel Console:${RESET}"
  echo -e "    URL:         ${CYAN}${ZITADEL_HOST}${RESET}"
  echo -e "    Admin login: ${CYAN}zitadel-admin${RESET} / ${CYAN}Password1!${RESET}"
  echo ""
  echo -e "  ${BOLD}Project:${RESET}"
  echo -e "    Project ID:  ${CYAN}${PROJECT_ID}${RESET}"
  echo ""
  echo -e "  ${BOLD}Next step:${RESET}"
  echo -e "    Run ${CYAN}make seed${RESET} to create demo data (org, users, teams)"
  echo ""
}

main "$@"
