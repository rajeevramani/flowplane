#!/usr/bin/env bash
# Test: can a machine user (agent) call the MCP server?
#
# Steps:
#   1. Exchange client_credentials for a JWT at Zitadel
#   2. Initialize an MCP session with that JWT
#   3. Call tools/list and verify tools are returned
#
# Usage: ./scripts/test-agent-mcp.sh
set -euo pipefail

FLOWPLANE_URL="${FLOWPLANE_URL:-http://localhost:8080}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Load Zitadel env for project ID
if [ -f "${PROJECT_DIR}/.env.zitadel" ]; then
  # shellcheck source=/dev/null
  source "${PROJECT_DIR}/.env.zitadel"
fi

# Agent credentials (from provisioning)
AGENT_CLIENT_ID="${AGENT_CLIENT_ID:-acme-corp--test-agent}"
AGENT_CLIENT_SECRET="${AGENT_CLIENT_SECRET:-nHMKV5iJTEsip5c84xyNzahYxbtZiPMTjzCyPZhIY4uuxsKTxY11LsPVFqbkGlMA}"
TOKEN_ENDPOINT="${TOKEN_ENDPOINT:-http://localhost:8081/oauth/v2/token}"
ZITADEL_PROJECT_ID="${ZITADEL_PROJECT_ID:?ZITADEL_PROJECT_ID required (set in .env.zitadel)}"

# Zitadel requires this scope for the JWT to carry the project audience claim.
# Without it, aud=client_id which fails Flowplane's JWT validation.
PROJECT_AUD_SCOPE="urn:zitadel:iam:org:project:id:${ZITADEL_PROJECT_ID}:aud"

# Colors
GREEN='\033[32m'
RED='\033[31m'
CYAN='\033[36m'
BOLD='\033[1m'
RESET='\033[0m'

pass() { echo -e "${GREEN}PASS${RESET}: $1"; }
fail() { echo -e "${RED}FAIL${RESET}: $1"; exit 1; }
step() { echo -e "\n${CYAN}${BOLD}Step $1${RESET}: $2"; }

# ── Step 1: Get JWT via client_credentials ──────────────────────────
step 1 "Exchange client_credentials for JWT"

TOKEN_RESPONSE=$(curl -sf -X POST "$TOKEN_ENDPOINT" \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=client_credentials" \
  -d "client_id=${AGENT_CLIENT_ID}" \
  -d "client_secret=${AGENT_CLIENT_SECRET}" \
  -d "scope=openid ${PROJECT_AUD_SCOPE}") || fail "Token request failed (is Zitadel running?)"

AGENT_JWT=$(echo "$TOKEN_RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])" 2>/dev/null) \
  || fail "Could not extract access_token from response: $TOKEN_RESPONSE"

echo "  Got JWT: ${AGENT_JWT:0:20}...${AGENT_JWT: -10}"
pass "Obtained agent JWT"

# ── Step 2: Initialize MCP session ──────────────────────────────────
step 2 "Initialize MCP session"

INIT_RESPONSE=$(curl -sf -w "\n%{http_code}\n%{header_json}" \
  -X POST "${FLOWPLANE_URL}/api/v1/mcp" \
  -H "Authorization: Bearer ${AGENT_JWT}" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2025-03-26",
      "capabilities": {},
      "clientInfo": { "name": "test-agent", "version": "0.1.0" }
    }
  }') || fail "MCP initialize request failed (is Flowplane running?)"

# Parse response: body is everything before the last two lines (status + headers)
INIT_BODY=$(echo "$INIT_RESPONSE" | sed -n '1p')
INIT_STATUS=$(echo "$INIT_RESPONSE" | sed -n '2p')
INIT_HEADERS=$(echo "$INIT_RESPONSE" | sed -n '3,$p')

if [ "$INIT_STATUS" != "200" ]; then
  echo "  HTTP status: $INIT_STATUS"
  echo "  Body: $INIT_BODY"
  fail "Expected 200, got $INIT_STATUS"
fi

# Extract session ID from response headers
SESSION_ID=$(echo "$INIT_HEADERS" | python3 -c "
import sys, json
headers = json.load(sys.stdin)
# Header keys are lowercased in curl's header_json
sid = headers.get('mcp-session-id', [None])[0]
print(sid or '')
" 2>/dev/null)

if [ -z "$SESSION_ID" ]; then
  fail "No MCP-Session-Id header in response"
fi

echo "  HTTP $INIT_STATUS"
echo "  Session ID: $SESSION_ID"
SERVER_INFO=$(echo "$INIT_BODY" | python3 -c "
import sys, json
r = json.load(sys.stdin).get('result', {})
si = r.get('serverInfo', {})
print(si.get('name', '?') + ' ' + si.get('version', '?'))
" 2>/dev/null)
echo "  Server info: $SERVER_INFO"
pass "MCP session initialized"

# ── Step 3: Call tools/list ─────────────────────────────────────────
step 3 "Call tools/list"

TOOLS_RESPONSE=$(curl -sf \
  -X POST "${FLOWPLANE_URL}/api/v1/mcp" \
  -H "Authorization: Bearer ${AGENT_JWT}" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -H "MCP-Session-Id: ${SESSION_ID}" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
  }') || fail "tools/list request failed"

TOOL_COUNT=$(echo "$TOOLS_RESPONSE" | python3 -c "
import sys, json
r = json.load(sys.stdin)
if 'error' in r:
    print(f\"ERROR: {r['error']}\")
    sys.exit(1)
tools = r.get('result', {}).get('tools', [])
print(len(tools))
" 2>/dev/null) || fail "tools/list returned error: $TOOLS_RESPONSE"

echo "  Tools returned: $TOOL_COUNT"

if [ "$TOOL_COUNT" -eq 0 ]; then
  fail "No tools returned — agent may lack permissions"
fi

# Print first few tool names
echo "  Sample tools:"
echo "$TOOLS_RESPONSE" | python3 -c "
import sys, json
tools = json.load(sys.stdin)['result']['tools']
for t in tools[:5]:
    print(f\"    - {t['name']}\")
if len(tools) > 5:
    print(f\"    ... and {len(tools) - 5} more\")
"

pass "Agent can call MCP tools/list ($TOOL_COUNT tools)"

echo -e "\n${GREEN}${BOLD}All steps passed.${RESET} Agent → MCP flow works end-to-end."
