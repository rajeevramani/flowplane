#!/usr/bin/env bash
# E2E smoke test for frictionless dev-mode onboarding.
#
# Exercises the complete dev-mode user journey:
#   init -> health check -> auth -> expose lifecycle -> CLI -> teardown
#
# Prerequisites: Docker/Podman available, ports 8080+5432 free
# Usage: ./scripts/e2e-dev-smoke.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors
CYAN='\033[36m'
GREEN='\033[32m'
RED='\033[31m'
YELLOW='\033[33m'
RESET='\033[0m'

PASS=0
FAIL=0
STEP=0

# Back up existing ~/.flowplane if present, restore on exit
FLOWPLANE_DIR="$HOME/.flowplane"
BACKUP_DIR=""
if [ -d "$FLOWPLANE_DIR" ]; then
  BACKUP_DIR="${TMPDIR:-/tmp}/flowplane-backup-$$"
  mv "$FLOWPLANE_DIR" "$BACKUP_DIR"
  echo -e "${YELLOW}Backed up existing ~/.flowplane to $BACKUP_DIR${RESET}"
fi

# Container runtime detection
if command -v docker &>/dev/null; then
  CONTAINER_RT=docker
elif command -v podman &>/dev/null; then
  CONTAINER_RT=podman
else
  echo -e "${RED}FAIL: Neither docker nor podman found${RESET}" >&2
  exit 1
fi

FLOWPLANE_URL="${FLOWPLANE_URL:-http://localhost:8080}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

step() {
  STEP=$((STEP + 1))
  echo -e "\n${CYAN}[$STEP] $1${RESET}"
}

pass() {
  PASS=$((PASS + 1))
  echo -e "  ${GREEN}PASS${RESET}: $1"
}

fail() {
  FAIL=$((FAIL + 1))
  echo -e "  ${RED}FAIL${RESET}: $1"
  echo -e "  ${YELLOW}Expected${RESET}: $2"
  echo -e "  ${YELLOW}Got${RESET}:      $3"
}

# Run a curl and capture status + body + content-type
# Usage: do_curl <method> <path> [data]
# Sets: HTTP_STATUS, HTTP_BODY, HTTP_CT
do_curl() {
  local method="$1" path="$2" data="${3:-}"
  local url="${FLOWPLANE_URL}${path}"
  local args=(-s -w '\n%{http_code}\n%{content_type}' -X "$method")

  if [ -n "$TOKEN" ]; then
    args+=(-H "Authorization: Bearer $TOKEN")
  fi
  args+=(-H "Content-Type: application/json")

  if [ -n "$data" ]; then
    args+=(-d "$data")
  fi

  local tmpfile header_file
  tmpfile=$(mktemp)
  header_file=$(mktemp)

  HTTP_STATUS=$(curl "${args[@]}" -o "$tmpfile" -D "$header_file" -w '%{http_code}' "$url" 2>/dev/null) || true
  HTTP_BODY=$(cat "$tmpfile" 2>/dev/null || echo "")
  HTTP_CT=$(grep -i '^content-type:' "$header_file" 2>/dev/null | head -1 | cut -d: -f2- | tr -d '[:space:]' || echo "")
  rm -f "$tmpfile" "$header_file"
}

assert_status() {
  local expected="$1" label="$2"
  if [ "$HTTP_STATUS" = "$expected" ]; then
    pass "$label (HTTP $HTTP_STATUS)"
  else
    fail "$label" "HTTP $expected" "HTTP $HTTP_STATUS — body: ${HTTP_BODY:0:200}"
  fi
}

assert_json_field() {
  local field="$1" label="$2"
  if echo "$HTTP_BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$field' in d" 2>/dev/null; then
    pass "$label"
  else
    fail "$label" "JSON with field '$field'" "${HTTP_BODY:0:200}"
  fi
}

assert_not_html() {
  local label="$1"
  if echo "$HTTP_BODY" | grep -q '<!doctype\|<!DOCTYPE\|<html'; then
    fail "$label" "JSON response" "HTML response (Content-Type: $HTTP_CT)"
  else
    pass "$label"
  fi
}

# Cleanup on exit (always runs)
cleanup() {
  echo -e "\n${CYAN}[TEARDOWN] Cleaning up...${RESET}"

  # Stop containers
  if [ -f "$FLOWPLANE_DIR/docker-compose-dev.yml" ]; then
    $CONTAINER_RT compose -f "$FLOWPLANE_DIR/docker-compose-dev.yml" -p flowplane down --volumes 2>/dev/null || true
  fi

  # Remove test ~/.flowplane
  rm -rf "$FLOWPLANE_DIR"

  # Restore backup if we had one
  if [ -n "$BACKUP_DIR" ] && [ -d "$BACKUP_DIR" ]; then
    mv "$BACKUP_DIR" "$FLOWPLANE_DIR"
    echo -e "  Restored original ~/.flowplane"
  fi

  echo ""
  echo -e "=========================================="
  echo -e "  ${GREEN}PASSED${RESET}: $PASS"
  echo -e "  ${RED}FAILED${RESET}: $FAIL"
  echo -e "=========================================="

  if [ "$FAIL" -gt 0 ]; then
    # Dump container logs before exit
    echo -e "\n${YELLOW}--- control-plane logs (last 50 lines) ---${RESET}"
    $CONTAINER_RT logs flowplane-control-plane --tail 50 2>/dev/null || echo "(no logs)"
    exit 1
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Phase 1: Init
# ---------------------------------------------------------------------------

step "Init dev environment"
cd "$PROJECT_DIR"

# Set FLOWPLANE_SOURCE_DIR so init can find the repo
export FLOWPLANE_SOURCE_DIR="$PROJECT_DIR"

cargo run -q -- init 2>&1 | tail -5
INIT_EXIT=$?

if [ "$INIT_EXIT" -eq 0 ]; then
  pass "cargo run -- init exited 0"
else
  fail "cargo run -- init" "exit 0" "exit $INIT_EXIT"
fi

# Check credentials file
if [ -f "$FLOWPLANE_DIR/credentials" ] && [ -s "$FLOWPLANE_DIR/credentials" ]; then
  pass "credentials file exists and is non-empty"
else
  fail "credentials file" "exists and non-empty" "missing or empty"
fi

# Check config.toml
if [ -f "$FLOWPLANE_DIR/config.toml" ]; then
  if grep -q 'base_url' "$FLOWPLANE_DIR/config.toml" && \
     grep -q 'team' "$FLOWPLANE_DIR/config.toml"; then
    pass "config.toml has base_url and team"
  else
    fail "config.toml contents" "base_url and team" "$(cat "$FLOWPLANE_DIR/config.toml")"
  fi
else
  fail "config.toml" "exists" "missing"
fi

# Check containers
if $CONTAINER_RT ps --format '{{.Names}}' 2>/dev/null | grep -q 'flowplane-control-plane'; then
  pass "control-plane container is running"
else
  fail "control-plane container" "running" "not found"
fi

if $CONTAINER_RT ps --format '{{.Names}}' 2>/dev/null | grep -q 'flowplane.*pg\|pg.*flowplane'; then
  pass "postgres container is running"
else
  # Show what containers are actually running for debugging
  RUNNING=$($CONTAINER_RT ps --format '{{.Names}}' 2>/dev/null | tr '\n' ', ')
  fail "postgres container" "running (flowplane-pg)" "not found in: $RUNNING"
fi

# Load token
TOKEN=$(cat "$FLOWPLANE_DIR/credentials")

# ---------------------------------------------------------------------------
# Phase 2: Health checks
# ---------------------------------------------------------------------------

step "Health checks"

do_curl GET /health
assert_status 200 "GET /health"

TOKEN="" do_curl GET /api/v1/auth/mode
# auth/mode is public, no token needed
TOKEN=""
do_curl GET /api/v1/auth/mode
TOKEN=$(cat "$FLOWPLANE_DIR/credentials")

assert_status 200 "GET /api/v1/auth/mode"

if echo "$HTTP_BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('auth_mode')=='dev'" 2>/dev/null; then
  pass "auth_mode is 'dev'"
else
  fail "auth_mode" "dev" "$HTTP_BODY"
fi

# ---------------------------------------------------------------------------
# Phase 3: Auth verification
# ---------------------------------------------------------------------------

step "Auth verification"

do_curl GET /api/v1/teams/default/clusters
assert_status 200 "valid token -> 200"

# Wrong token
SAVED_TOKEN="$TOKEN"
TOKEN="wrong-token-12345"
do_curl GET /api/v1/teams/default/clusters
assert_status 401 "wrong token -> 401"
assert_not_html "wrong token response is not HTML"

# No token
TOKEN=""
do_curl GET /api/v1/teams/default/clusters
assert_status 401 "no token -> 401"
assert_not_html "no token response is not HTML"

# Misspelled path (Bug 1 from user report)
TOKEN="$SAVED_TOKEN"
do_curl GET /api/v1/teams/default/liteners
assert_status 404 "misspelled path -> 404"
assert_not_html "misspelled path response is not HTML"

# ---------------------------------------------------------------------------
# Phase 4: Expose lifecycle
# ---------------------------------------------------------------------------

step "Expose lifecycle"

# Create
do_curl POST /api/v1/teams/default/expose '{"name":"test-svc","upstream":"http://localhost:3000"}'
assert_status 201 "POST expose -> 201"
assert_json_field name "response has 'name'"
assert_json_field port "response has 'port'"
assert_json_field cluster "response has 'cluster'"
assert_json_field route_config "response has 'route_config'"
assert_json_field listener "response has 'listener'"

# Extract port for later
EXPOSE_PORT=$(echo "$HTTP_BODY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('port',''))" 2>/dev/null || echo "")

# Verify sub-resources
do_curl GET /api/v1/teams/default/clusters/test-svc
assert_status 200 "cluster exists after expose"

do_curl GET /api/v1/teams/default/listeners/test-svc-listener
assert_status 200 "listener exists after expose"

do_curl GET /api/v1/teams/default/route-configs/test-svc-routes
assert_status 200 "route-config exists after expose"

# Idempotent re-expose
do_curl POST /api/v1/teams/default/expose '{"name":"test-svc","upstream":"http://localhost:3000"}'
assert_status 200 "idempotent re-expose -> 200"

if [ -n "$EXPOSE_PORT" ]; then
  REEXPOSE_PORT=$(echo "$HTTP_BODY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('port',''))" 2>/dev/null || echo "")
  if [ "$EXPOSE_PORT" = "$REEXPOSE_PORT" ]; then
    pass "idempotent expose returns same port ($EXPOSE_PORT)"
  else
    fail "idempotent port stability" "$EXPOSE_PORT" "$REEXPOSE_PORT"
  fi
fi

# Unexpose
do_curl DELETE /api/v1/teams/default/expose/test-svc
assert_status 204 "DELETE unexpose -> 204"

# Verify cleanup
do_curl GET /api/v1/teams/default/clusters/test-svc
assert_status 404 "cluster gone after unexpose"

do_curl GET /api/v1/teams/default/listeners/test-svc-listener
assert_status 404 "listener gone after unexpose"

# ---------------------------------------------------------------------------
# Phase 5: CLI commands
# ---------------------------------------------------------------------------

step "CLI commands"

if cargo run -q -- cluster list 2>/dev/null; then
  pass "cluster list exits 0"
else
  fail "cluster list" "exit 0" "non-zero exit"
fi

if cargo run -q -- listener list 2>/dev/null; then
  pass "listener list exits 0"
else
  fail "listener list" "exit 0" "non-zero exit"
fi

if cargo run -q -- route list 2>/dev/null; then
  pass "route list exits 0"
else
  fail "route list" "exit 0" "non-zero exit"
fi

CLI_TOKEN=$(cargo run -q -- auth token 2>/dev/null || echo "")
if [ -n "$CLI_TOKEN" ]; then
  CREDS_TOKEN=$(cat "$FLOWPLANE_DIR/credentials")
  if [ "$CLI_TOKEN" = "$CREDS_TOKEN" ]; then
    pass "auth token matches credentials file"
  else
    fail "auth token match" "$CREDS_TOKEN" "$CLI_TOKEN"
  fi
else
  fail "auth token" "non-empty output" "empty"
fi

# ---------------------------------------------------------------------------
# Phase 6: Teardown + idempotent restart
# ---------------------------------------------------------------------------

step "Teardown and idempotent restart"

cargo run -q -- down 2>/dev/null
DOWN_EXIT=$?
if [ "$DOWN_EXIT" -eq 0 ]; then
  pass "cargo run -- down exits 0"
else
  fail "cargo run -- down" "exit 0" "exit $DOWN_EXIT"
fi

# Verify containers stopped
if ! $CONTAINER_RT ps --format '{{.Names}}' 2>/dev/null | grep -q 'flowplane-control-plane'; then
  pass "control-plane container stopped"
else
  fail "container stop" "stopped" "still running"
fi

# Idempotent restart
step "Idempotent restart"

cargo run -q -- init 2>&1 | tail -3
REINIT_EXIT=$?
if [ "$REINIT_EXIT" -eq 0 ]; then
  pass "second init exits 0"
else
  fail "second init" "exit 0" "exit $REINIT_EXIT"
fi

# Reload token (may have changed)
TOKEN=$(cat "$FLOWPLANE_DIR/credentials")

# Wait briefly for health
sleep 2
do_curl GET /health
assert_status 200 "health after restart"

# Final down with volumes (cleanup does this via trap, but test the flag)
cargo run -q -- down --volumes 2>/dev/null
if [ $? -eq 0 ]; then
  pass "down --volumes exits 0"
else
  fail "down --volumes" "exit 0" "non-zero exit"
fi

echo -e "\n${GREEN}E2E dev smoke test complete.${RESET}"
