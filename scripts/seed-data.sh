#!/usr/bin/env bash
# Seed data script for local development
# Creates: platform admin, org, org-admin user, team, API (via OpenAPI import), listener, tokens
set -euo pipefail

BASE_URL="${FLOWPLANE_URL:-http://localhost:8080}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Platform admin
ADMIN_EMAIL="admin@flowplane.local"
ADMIN_PASSWORD="Admin123!"
ADMIN_NAME="Platform Admin"

# Org
ORG_NAME="acme-corp"
ORG_DISPLAY="Acme Corp"

# Org admin
ORG_ADMIN_EMAIL="orgadmin@acme-corp.local"
ORG_ADMIN_PASSWORD="OrgAdmin123!"
ORG_ADMIN_NAME="Acme Org Admin"

# Team + resources
TEAM_NAME="engineering"
LISTENER_PORT=10016
OPENAPI_SPEC="$PROJECT_DIR/examples/openapi/httpbin.yaml"

# Colors
CYAN='\033[36m'
GREEN='\033[32m'
YELLOW='\033[33m'
RED='\033[31m'
RESET='\033[0m'

log()  { echo -e "${CYAN}[seed]${RESET} $*"; }
ok()   { echo -e "${GREEN}  ✓${RESET} $*"; }
skip() { echo -e "${YELLOW}  ─${RESET} $* (already exists)"; }
fail() { echo -e "${RED}  ✗${RESET} $*"; }

# --- Globals set by HTTP helpers ---
BODY=""
HTTP_STATUS=""
CSRF_TOKEN=""
COOKIE_JAR=""
ORG_ADMIN_JAR=""
ORG_ADMIN_CSRF=""

cleanup() {
    [ -n "$COOKIE_JAR" ] && rm -f "$COOKIE_JAR"
    [ -n "$ORG_ADMIN_JAR" ] && rm -f "$ORG_ADMIN_JAR"
}
trap cleanup EXIT

# --- HTTP helpers ---
# Auth uses cookies (fp_session) + CSRF token, NOT Bearer tokens.
# The login sets an httpOnly cookie; we use a cookie jar to persist it.

post_json() {
    local url="$1" data="$2"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" \
        -b "$COOKIE_JAR" -c "$COOKIE_JAR" \
        -X POST "$url" \
        -H 'Content-Type: application/json' \
        -H "X-CSRF-Token: $CSRF_TOKEN" \
        -d "$data")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

# POST with JSON using a specific cookie jar (for org-admin login)
post_json_jar() {
    local url="$1" data="$2" jar="$3" csrf="${4:-}"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" \
        -b "$jar" -c "$jar" \
        -X POST "$url" \
        -H 'Content-Type: application/json' \
        -H "X-CSRF-Token: $csrf" \
        -d "$data")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

post_file() {
    local url="$1" file="$2" content_type="$3"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" \
        -b "$COOKIE_JAR" -c "$COOKIE_JAR" \
        -X POST "$url" \
        -H "Content-Type: $content_type" \
        -H "X-CSRF-Token: $CSRF_TOKEN" \
        --data-binary "@$file")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

# POST file using a specific cookie jar
post_file_jar() {
    local url="$1" file="$2" content_type="$3" jar="$4" csrf="${5:-}"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" \
        -b "$jar" -c "$jar" \
        -X POST "$url" \
        -H "Content-Type: $content_type" \
        -H "X-CSRF-Token: $csrf" \
        --data-binary "@$file")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

# GET using a specific cookie jar
get_json_jar() {
    local url="$1" jar="$2"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" \
        -b "$jar" -c "$jar" \
        -X GET "$url")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

get_json() {
    local url="$1"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" \
        -b "$COOKIE_JAR" -c "$COOKIE_JAR" \
        -X GET "$url")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

# GET without auth (public endpoints)
get_public() {
    local url="$1"
    local tmp
    tmp=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$tmp" -X GET "$url")
    BODY=$(cat "$tmp")
    rm -f "$tmp"
}

extract() { echo "$BODY" | python3 -c "import sys,json; print(json.load(sys.stdin)$1)" 2>/dev/null; }

# --- Wait for CP to be ready ---
wait_for_cp() {
    log "Waiting for control plane at $BASE_URL ..."
    local attempts=0
    while ! curl -sf "$BASE_URL/api/v1/bootstrap/status" > /dev/null 2>&1; do
        attempts=$((attempts + 1))
        if [ $attempts -ge 30 ]; then
            fail "Control plane not reachable after 30s"
            exit 1
        fi
        sleep 1
    done
    ok "Control plane is ready"
}

# --- Step 1: Bootstrap ---
bootstrap() {
    log "Checking bootstrap status..."
    get_public "$BASE_URL/api/v1/bootstrap/status"

    local needs_init
    needs_init=$(echo "$BODY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('needsInitialization', False))" 2>/dev/null)

    if [ "$needs_init" = "True" ]; then
        log "Bootstrapping platform admin..."
        post_json "$BASE_URL/api/v1/bootstrap/initialize" \
            "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\",\"name\":\"$ADMIN_NAME\"}"
        if [ "$HTTP_STATUS" = "201" ]; then
            ok "Platform bootstrapped ($ADMIN_EMAIL)"
        else
            fail "Bootstrap failed (HTTP $HTTP_STATUS): $BODY"
            exit 1
        fi
    else
        skip "Platform already bootstrapped"
    fi
}

# --- Step 2: Login ---
login() {
    log "Logging in as platform admin..."
    post_json "$BASE_URL/api/v1/auth/login" \
        "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}"
    if [ "$HTTP_STATUS" = "200" ]; then
        CSRF_TOKEN=$(extract "['csrfToken']")
        ok "Logged in (session cookie set, csrf acquired)"
    else
        fail "Login failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi
}

# --- Step 3: Create org ---
create_org() {
    log "Creating org '$ORG_NAME'..."
    post_json "$BASE_URL/api/v1/admin/organizations" \
        "{\"name\":\"$ORG_NAME\",\"displayName\":\"$ORG_DISPLAY\",\"description\":\"Seed data org\"}"
    if [ "$HTTP_STATUS" = "201" ]; then
        ORG_ID=$(extract "['id']")
        ok "Org '$ORG_NAME' created (id: $ORG_ID)"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key"; }; then
        skip "Org '$ORG_NAME'"
        get_json "$BASE_URL/api/v1/admin/organizations"
        ORG_ID=$(echo "$BODY" | python3 -c "
import sys, json
data = json.load(sys.stdin)
items = data if isinstance(data, list) else data.get('items', [])
for o in items:
    if o['name'] == '$ORG_NAME':
        print(o['id']); break
" 2>/dev/null)
        ok "Org '$ORG_NAME' found (id: $ORG_ID)"
    else
        fail "Create org failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi
}

# --- Step 4: Create org-admin user ---
create_org_admin() {
    log "Creating org-admin user '$ORG_ADMIN_EMAIL'..."
    post_json "$BASE_URL/api/v1/users" \
        "{\"email\":\"$ORG_ADMIN_EMAIL\",\"password\":\"$ORG_ADMIN_PASSWORD\",\"name\":\"$ORG_ADMIN_NAME\",\"isAdmin\":false,\"orgId\":\"$ORG_ID\"}"

    if [ "$HTTP_STATUS" = "201" ]; then
        USER_ID=$(extract "['id']")
        ok "User '$ORG_ADMIN_EMAIL' created (id: $USER_ID)"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key"; }; then
        skip "User '$ORG_ADMIN_EMAIL'"
        get_json "$BASE_URL/api/v1/users"
        USER_ID=$(echo "$BODY" | python3 -c "
import sys, json
data = json.load(sys.stdin)
items = data if isinstance(data, list) else data.get('items', [])
for u in items:
    if u['email'] == '$ORG_ADMIN_EMAIL':
        print(u['id']); break
" 2>/dev/null)
        ok "User '$ORG_ADMIN_EMAIL' found (id: $USER_ID)"
    else
        fail "Create user failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi

    # Add org membership with admin role
    log "Assigning org-admin role..."
    post_json "$BASE_URL/api/v1/admin/organizations/$ORG_ID/members" \
        "{\"userId\":\"$USER_ID\",\"role\":\"admin\"}"
    if [ "$HTTP_STATUS" = "201" ] || [ "$HTTP_STATUS" = "200" ]; then
        ok "Org-admin role assigned"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate\|already"; }; then
        skip "Org-admin role"
    else
        fail "Assign org-admin failed (HTTP $HTTP_STATUS): $BODY"
    fi
}

# --- Step 5: Login as org-admin ---
# Team-scoped operations (dataplane, OpenAPI import) must use the org-admin session.
# Platform admin session is scoped to the 'platform' org, so resolve_team_name()
# cannot find teams belonging to other orgs like 'acme-corp'.
login_org_admin() {
    log "Logging in as org-admin..."
    ORG_ADMIN_JAR=$(mktemp)
    post_json_jar "$BASE_URL/api/v1/auth/login" \
        "{\"email\":\"$ORG_ADMIN_EMAIL\",\"password\":\"$ORG_ADMIN_PASSWORD\"}" \
        "$ORG_ADMIN_JAR"
    if [ "$HTTP_STATUS" = "200" ]; then
        ORG_ADMIN_CSRF=$(extract "['csrfToken']")
        ok "Org-admin logged in (session cookie set, csrf acquired)"
    else
        fail "Org-admin login failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi
}

# --- Step 6: Create team ---
create_team() {
    log "Creating team '$TEAM_NAME' under '$ORG_NAME'..."
    post_json_jar "$BASE_URL/api/v1/orgs/$ORG_NAME/teams" \
        "{\"name\":\"$TEAM_NAME\",\"displayName\":\"Engineering\",\"description\":\"Engineering team\"}" \
        "$ORG_ADMIN_JAR" "$ORG_ADMIN_CSRF"
    if [ "$HTTP_STATUS" = "201" ]; then
        ok "Team '$TEAM_NAME' created"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key"; }; then
        skip "Team '$TEAM_NAME'"
    else
        fail "Create team failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi
}

# --- Step 7: Create dataplane ---
create_dataplane() {
    log "Creating dataplane for team '$TEAM_NAME'..."
    post_json_jar "$BASE_URL/api/v1/teams/$TEAM_NAME/dataplanes" \
        "{\"team\":\"$TEAM_NAME\",\"name\":\"$TEAM_NAME-dataplane\",\"gatewayHost\":\"127.0.0.1\",\"description\":\"Seed dataplane\"}" \
        "$ORG_ADMIN_JAR" "$ORG_ADMIN_CSRF"
    if [ "$HTTP_STATUS" = "201" ]; then
        DATAPLANE_ID=$(extract "['id']")
        ok "Dataplane created (id: $DATAPLANE_ID)"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key"; }; then
        skip "Dataplane '$TEAM_NAME-dataplane'"
        get_json_jar "$BASE_URL/api/v1/teams/$TEAM_NAME/dataplanes" "$ORG_ADMIN_JAR"
        DATAPLANE_ID=$(echo "$BODY" | python3 -c "
import sys, json
data = json.load(sys.stdin)
items = data if isinstance(data, list) else data.get('items', [])
for d in items:
    if d['name'] == '$TEAM_NAME-dataplane':
        print(d['id']); break
" 2>/dev/null)
        ok "Dataplane found (id: $DATAPLANE_ID)"
    else
        fail "Create dataplane failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi
}

# --- Step 8: Import OpenAPI spec ---
import_openapi() {
    if [ ! -f "$OPENAPI_SPEC" ]; then
        fail "OpenAPI spec not found: $OPENAPI_SPEC"
        exit 1
    fi

    log "Importing OpenAPI spec ($OPENAPI_SPEC)..."
    local import_url="$BASE_URL/api/v1/openapi/import?team=$TEAM_NAME&listener_mode=new&new_listener_name=$TEAM_NAME-listener&new_listener_port=$LISTENER_PORT&dataplane_id=$DATAPLANE_ID"
    post_file_jar "$import_url" "$OPENAPI_SPEC" "application/yaml" "$ORG_ADMIN_JAR" "$ORG_ADMIN_CSRF"
    if [ "$HTTP_STATUS" = "201" ]; then
        local routes_created clusters_created listener_name
        routes_created=$(extract "['routesCreated']" 2>/dev/null || echo "?")
        clusters_created=$(extract "['clustersCreated']" 2>/dev/null || echo "?")
        listener_name=$(extract "['listenerName']" 2>/dev/null || echo "?")
        ok "OpenAPI imported: $routes_created routes, $clusters_created clusters, listener=$listener_name (port $LISTENER_PORT)"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key\|already exists"; }; then
        skip "OpenAPI import (resources may already exist)"
    else
        fail "OpenAPI import failed (HTTP $HTTP_STATUS): $BODY"
        exit 1
    fi
}

# --- Step 9: Generate API tokens ---
generate_tokens() {
    # Platform admin token (full access) — using admin session from cookie jar
    log "Generating platform admin API token..."
    post_json "$BASE_URL/api/v1/tokens" \
        "{\"name\":\"seed-admin-token\",\"description\":\"Seed script platform admin token\",\"scopes\":[\"admin:all\"]}"
    if [ "$HTTP_STATUS" = "201" ]; then
        ADMIN_API_TOKEN=$(extract "['token']")
        ok "Platform admin token created"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key\|already exists"; }; then
        skip "Platform admin token"
        ADMIN_API_TOKEN="<already created - check UI or rotate>"
    else
        fail "Create platform admin token failed (HTTP $HTTP_STATUS): $BODY"
        ADMIN_API_TOKEN="<failed to create>"
    fi

    # Org admin token — reuse the org-admin session already established
    log "Generating org-admin API token..."
    post_json_jar "$BASE_URL/api/v1/tokens" \
        "{\"name\":\"seed-orgadmin-token\",\"description\":\"Seed script org admin token\",\"scopes\":[\"org:$ORG_NAME:admin\"]}" \
        "$ORG_ADMIN_JAR" "$ORG_ADMIN_CSRF"
    if [ "$HTTP_STATUS" = "201" ]; then
        ORG_ADMIN_API_TOKEN=$(extract "['token']")
        ok "Org-admin token created"
    elif [ "$HTTP_STATUS" = "409" ] || { [ "$HTTP_STATUS" = "500" ] && echo "$BODY" | grep -q "duplicate key\|already exists"; }; then
        skip "Org-admin token"
        ORG_ADMIN_API_TOKEN="<already created - check UI or rotate>"
    else
        fail "Create org-admin token failed (HTTP $HTTP_STATUS): $BODY"
        ORG_ADMIN_API_TOKEN="<failed to create>"
    fi
}

# --- Main ---
main() {
    echo ""
    echo -e "${CYAN}━━━ Flowplane Seed Data ━━━${RESET}"
    echo ""

    COOKIE_JAR=$(mktemp)
    TOKEN=""
    ORG_ID=""
    USER_ID=""
    DATAPLANE_ID=""
    ADMIN_API_TOKEN=""
    ORG_ADMIN_API_TOKEN=""

    wait_for_cp
    bootstrap
    login                # platform admin session (for admin ops)
    create_org
    create_org_admin
    login_org_admin      # org-admin session (for team-scoped ops)
    create_team
    create_dataplane
    import_openapi
    generate_tokens

    echo ""
    echo -e "${GREEN}━━━ Seed complete ━━━${RESET}"
    echo ""
    echo -e "  ${GREEN}Credentials:${RESET}"
    echo -e "    Platform admin:  ${CYAN}$ADMIN_EMAIL${RESET} / ${CYAN}$ADMIN_PASSWORD${RESET}"
    echo -e "    Org admin:       ${CYAN}$ORG_ADMIN_EMAIL${RESET} / ${CYAN}$ORG_ADMIN_PASSWORD${RESET}"
    echo ""
    echo -e "  ${GREEN}API Tokens (use with Authorization: Bearer <token>):${RESET}"
    echo -e "    Platform admin:  ${CYAN}$ADMIN_API_TOKEN${RESET}"
    echo -e "    Org admin:       ${CYAN}$ORG_ADMIN_API_TOKEN${RESET}"
    echo ""
    echo -e "  ${GREEN}Resources:${RESET}"
    echo -e "    Org:             ${CYAN}$ORG_NAME${RESET}"
    echo -e "    Team:            ${CYAN}$TEAM_NAME${RESET}"
    echo -e "    Listener port:   ${CYAN}$LISTENER_PORT${RESET}"
    echo -e "    API spec:        ${CYAN}$(basename "$OPENAPI_SPEC")${RESET}"
    echo ""
    echo -e "  ${GREEN}Usage:${RESET}"
    echo -e "    curl -H 'Authorization: Bearer ${CYAN}<token>${RESET}' $BASE_URL/api/v1/clusters"
    echo ""
}

main "$@"
