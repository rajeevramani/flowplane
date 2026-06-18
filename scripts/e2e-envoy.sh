#!/usr/bin/env bash
# Live Envoy E2E (S5.6/S7.7e, dev-mode path): boot CP -> configure via the CLI expose shortcut
# -> generate dataplane bootstrap -> a real Envoy joins over ADS -> traffic flows end to end.
set -euo pipefail
cd "$(dirname "$0")/.."

# Port contract: defaults are stable for copy/paste, but every local bind is overrideable so
# concurrent runs and developer machines with occupied ports do not require editing this file.
API=${FLOWPLANE_E2E_API:-127.0.0.1:8096}
XDS_PORT=${FLOWPLANE_E2E_XDS_PORT:-18000}
GW_PORT=${FLOWPLANE_E2E_GW_PORT:-10001}
UPSTREAM_PORT=${FLOWPLANE_E2E_UPSTREAM_PORT:-3001}
ADMIN_PORT=${FLOWPLANE_E2E_ADMIN_PORT:-9901}
DISCOVERY_PORT=$((GW_PORT+6))
GENERATED_ROUTE_PORT=$((GW_PORT+7))
AI_PROVIDER_PORT=$((GW_PORT+8))
AI_GATEWAY_PORT=$((GW_PORT+9))
AI_MULTI_GATEWAY_PORT=$((GW_PORT+10))
AI_FAILOVER_GATEWAY_PORT=$((GW_PORT+11))
AI_FALLBACK_PROVIDER_PORT=$((GW_PORT+12))
AI_UNAVAILABLE_PROVIDER_PORT=$((GW_PORT+13))
AI_STREAM_GATEWAY_PORT=$((GW_PORT+14))
AI_STREAM_DIE_PORT=$((GW_PORT+15))
AI_STREAM_FALLBACK_PORT=$((GW_PORT+16))
AI_MALFORMED_PORT=$((GW_PORT+17))
AI_MALFORMED_GATEWAY_PORT=$((GW_PORT+18))
MCP_PARITY_PORT=$((GW_PORT+19))
DB=flowplane_e2e
PG_ADMIN_URL=${FLOWPLANE_E2E_PG_ADMIN_URL:-postgres://postgres:postgres@127.0.0.1:5432/postgres}
PG_DB_URL=${FLOWPLANE_E2E_DATABASE_URL:-postgres://postgres:postgres@127.0.0.1:5432/$DB}

cleanup() {
  docker rm -f fp-e2e-envoy >/dev/null 2>&1 || true
  [ -n "${CP_PID:-}" ] && kill "$CP_PID" >/dev/null 2>&1 || true
  [ -n "${UP_PID:-}" ] && kill "$UP_PID" >/dev/null 2>&1 || true
  [ -n "${AI_PID:-}" ] && kill "$AI_PID" >/dev/null 2>&1 || true
  [ -n "${AI_FALLBACK_PID:-}" ] && kill "$AI_FALLBACK_PID" >/dev/null 2>&1 || true
  [ -n "${AI_STREAM_DIE_PID:-}" ] && kill "$AI_STREAM_DIE_PID" >/dev/null 2>&1 || true
  [ -n "${AI_STREAM_FALLBACK_PID:-}" ] && kill "$AI_STREAM_FALLBACK_PID" >/dev/null 2>&1 || true
  [ -n "${AI_MALFORMED_PID:-}" ] && kill "$AI_MALFORMED_PID" >/dev/null 2>&1 || true
  [ -n "${ENVOY_PID:-}" ] && kill "$ENVOY_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if psql "$PG_ADMIN_URL" -tc "select 1" >/dev/null 2>&1; then
  psql "$PG_ADMIN_URL" -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" \
    -c "CREATE DATABASE $DB" >/dev/null
else
  bash scripts/ensure-postgres.sh >/dev/null
  su postgres -s /bin/bash -c "dropdb --if-exists $DB && createdb $DB"
  PG_DB_URL=postgres://postgres:postgres@localhost/$DB
fi

# Distinctive upstream.
mkdir -p /tmp/fp-e2e-www && echo "hello-from-upstream-$(date +%s)" > /tmp/fp-e2e-www/index.html
mkdir -p /tmp/fp-e2e-www/v1/discovered
cp /tmp/fp-e2e-www/index.html /tmp/fp-e2e-www/v1/discovered/1
cp /tmp/fp-e2e-www/index.html /tmp/fp-e2e-www/v1/discovered/2
(cd /tmp/fp-e2e-www && python3 -m http.server $UPSTREAM_PORT >/dev/null 2>&1) &
UP_PID=$!
cat >/tmp/fp-e2e-ai-provider.py <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
auth_log = sys.argv[2]
expected_auth = sys.argv[3]

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        self.rfile.read(length)
        auth = self.headers.get("authorization", "")
        with open(auth_log, "a", encoding="utf-8") as f:
            f.write(auth + "\n")
        if auth != expected_auth:
            body = b"missing AI credential"
            self.send_response(401)
            self.send_header("content-type", "text/plain")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        body = {
            "id": "chatcmpl-fp-e2e",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "mock-ai-ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        }
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
: >/tmp/fp-e2e-ai-auth.log
python3 /tmp/fp-e2e-ai-provider.py "$AI_PROVIDER_PORT" /tmp/fp-e2e-ai-auth.log "Bearer fp-e2e-ai-secret" >/tmp/fp-e2e-ai-provider.log 2>&1 &
AI_PID=$!
: >/tmp/fp-e2e-ai-fallback-auth.log
python3 /tmp/fp-e2e-ai-provider.py "$AI_FALLBACK_PROVIDER_PORT" /tmp/fp-e2e-ai-fallback-auth.log "Bearer fp-e2e-ai-fallback-secret" >/tmp/fp-e2e-ai-fallback-provider.log 2>&1 &
AI_FALLBACK_PID=$!

cargo build --bin flowplane -q
FLOWPLANE_DATABASE_URL=$PG_DB_URL \
FLOWPLANE_API_INSECURE=true FLOWPLANE_DEV_MODE=true \
FLOWPLANE_SECRET_ENCRYPTION_KEY=12345678901234567890123456789012 \
FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS=127.0.0.1:$UPSTREAM_PORT \
FLOWPLANE_API_ADDR=$API FLOWPLANE_XDS_ADDR=0.0.0.0:$XDS_PORT \
./target/debug/flowplane serve > /tmp/fp-e2e-cp.log 2>&1 &
CP_PID=$!
for i in $(seq 1 40); do curl -fsS http://$API/healthz >/dev/null 2>&1 && break; sleep 0.5; done
TOKEN=$(grep -o '"dev_token":"[^"]*"' /tmp/fp-e2e-cp.log | cut -d'"' -f4)
[ -n "$TOKEN" ] || { echo "no dev token"; exit 1; }

auth=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")
TEAM_ID=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams | python3 -c "import sys,json;print(json.load(sys.stdin)[0]['id'])")
echo "team: $TEAM_ID"
export FLOWPLANE_SERVER="http://$API"
export FLOWPLANE_TOKEN="$TOKEN"
export FLOWPLANE_ORG=dev-org
export FLOWPLANE_TEAM=default

./target/debug/flowplane dataplane create dp-e2e --description "e2e local Envoy" >/dev/null
./target/debug/flowplane expose "http://127.0.0.1:$UPSTREAM_PORT" \
  --name e2e --path / --port "$GW_PORT" --public-base-url "http://127.0.0.1:$GW_PORT" >/tmp/fp-e2e-expose.txt
grep -q "http://127.0.0.1:$GW_PORT/" /tmp/fp-e2e-expose.txt || {
  echo "expose output did not include curl URL"
  cat /tmp/fp-e2e-expose.txt
  exit 1
}
./target/debug/flowplane --out /tmp/fp-e2e-bootstrap.yaml \
  dataplane bootstrap dp-e2e --mode dev \
  --xds-host 127.0.0.1 --xds-port "$XDS_PORT" --admin-port "$ADMIN_PORT"
grep -q "team=$TEAM_ID/dp-" /tmp/fp-e2e-bootstrap.yaml || {
  echo "bootstrap did not include team UUID in node.id"
  cat /tmp/fp-e2e-bootstrap.yaml
  exit 1
}
echo "dataplane bootstrap and gateway resources created via CLI"

if docker run -d --name fp-e2e-envoy --network host \
  -v /tmp/fp-e2e-bootstrap.yaml:/etc/envoy/envoy.yaml:ro \
  envoyproxy/envoy:v1.37-latest -c /etc/envoy/envoy.yaml --log-level info >/dev/null 2>&1; then
  echo "envoy started (docker); waiting for traffic to flow"
else
  command -v envoy >/dev/null || { echo "neither docker envoy nor local envoy binary available"; exit 1; }
  envoy -c /tmp/fp-e2e-bootstrap.yaml --log-level info > /tmp/fp-e2e-envoy.log 2>&1 &
  ENVOY_PID=$!
  echo "envoy started (local binary $(envoy --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)); waiting for traffic to flow"
fi

fail() {
  echo "E2E FAILED: $1"
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump > /tmp/fp-e2e-dump.json 2>/dev/null || true
  echo "--- envoy logs:"; (docker logs fp-e2e-envoy 2>/dev/null || cat /tmp/fp-e2e-envoy.log) | tail -25
  echo "--- cp logs:"; tail -15 "${CP_LOG:-/tmp/fp-e2e-cp.log}"
  exit 1
}

# Record a known, tracked failure (a filed product bug) without aborting the run, so the rest of
# the certification suite still exercises and verifies every other phase. The run still ends
# non-zero if any known failure was recorded.
KNOWN_FAIL_COUNT=0
known_fail() {
  echo "KNOWN-FAIL: $1"
  KNOWN_FAIL_COUNT=$((KNOWN_FAIL_COUNT + 1))
}

# Tier-0 certification gate: provider credential values must never appear in control-plane logs,
# Envoy logs, config dumps, access logs, or persisted usage rows. Mock-provider auth logs
# legitimately receive the injected credential (that is the point), and the dataplane bootstrap
# carries one-time dev PKI by design -- both are excluded.
redaction_sweep() {
  local sentinels=(
    fp-e2e-ai-secret
    fp-e2e-ai-fallback-secret
    fp-e2e-ai-stream-primary
    fp-e2e-ai-stream-fallback
  )
  local artifacts=(
    /tmp/fp-e2e-cp.log
    /tmp/fp-e2e-envoy.log
    /tmp/fp-e2e-dump.json
    /tmp/fp-e2e-ai-dump.json
    /tmp/fp-e2e-advanced-access.log
  )
  local leaked=0
  for s in "${sentinels[@]}"; do
    for a in "${artifacts[@]}"; do
      if [ -f "$a" ] && grep -qF "$s" "$a"; then
        echo "REDACTION LEAK: credential value '$s' found in $a"
        leaked=1
      fi
    done
    if psql "$PG_DB_URL" -Atc "SELECT row_to_json(t)::text FROM (SELECT * FROM ai_usage_events) t" 2>/dev/null \
      | grep -qF "$s"; then
      echo "REDACTION LEAK: credential value '$s' found in ai_usage_events rows"
      leaked=1
    fi
  done
  # The API bearer token must not be logged either.
  if [ -n "${TOKEN:-}" ] && grep -qF "$TOKEN" /tmp/fp-e2e-cp.log 2>/dev/null; then
    echo "REDACTION LEAK: API bearer token found in control-plane log"
    leaked=1
  fi
  [ "$leaked" = "0" ] || fail "credential/secret/token values leaked (see REDACTION LEAK lines)"
  echo "REDACTION SWEEP OK: no credential/secret/token values in logs, config dumps, access logs, or usage rows"
}

# Poll the gateway port until the body matches the given prefix.
wait_port_body() {
  local port=$1 prefix=$2 tries=${3:-40}
  for i in $(seq 1 "$tries"); do
    BODY=$(curl -fsS http://127.0.0.1:$port/ 2>/dev/null || true)
    [[ "$BODY" == $prefix* ]] && return 0
    sleep 1
  done
  return 1
}

wait_body() {
  wait_port_body "$GW_PORT" "$1" "${2:-40}"
}

wait_body hello-from-upstream- || fail "initial traffic never flowed"
./target/debug/flowplane stats overview >/tmp/fp-e2e-stats.txt
grep -q "TOTAL DATAPLANES" /tmp/fp-e2e-stats.txt || fail "stats overview did not render dataplane totals"
./target/debug/flowplane ops xds status >/tmp/fp-e2e-xds-status.txt
grep -q "TOTAL DATAPLANES" /tmp/fp-e2e-xds-status.txt || fail "xds status did not render dataplane totals"
./target/debug/flowplane ops xds nacks >/tmp/fp-e2e-xds-nacks.txt
grep -q "no rows" /tmp/fp-e2e-xds-nacks.txt || fail "unexpected xDS NACKs after happy-path expose"
echo "PHASE 1 OK: '$BODY' served through Envoy via ADS-delivered config"

# ---- Phase 1a: OpenAI-compatible AI gateway path with usage settlement and enforcing budget trip.
AI_SECRET_VALUE="Bearer fp-e2e-ai-secret"
AI_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-secret").decode())')
AI_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-key\",\"description\":\"AI e2e credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_PROVIDER_BODY")
AI_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e\",\"spec\":{\"listener_port\":$AI_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}")
AI_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_ROUTE_BODY")
AI_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$TEAM_ID' AND name = 'ai-ai-e2e-routes' AND owner_kind = 'ai'")
[ -n "$AI_ROUTE_CONFIG_ID" ] || fail "AI route materialized route config not found"
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "flowplane_ai" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "flowplane_ai" /tmp/fp-e2e-ai-dump.json || fail "AI listener did not receive ExtProc filter"
AI_REQUEST='{"model":"gpt-5","messages":[{"role":"user","content":"hello"}]}'
for i in $(seq 1 50); do
  AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-warm.json -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_CODE" = "200" ] || fail "AI route never served mock provider (last code $AI_CODE)"
grep -q "mock-ai-ok" /tmp/fp-e2e-ai-warm.json || fail "AI mock response did not reach client"
grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-auth.log || fail "AI provider did not receive injected credential"
curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json
if grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-dump.json; then
  fail "AI provider credential leaked into Envoy config dump"
fi
if psql "$PG_DB_URL" -Atc "SELECT spec::text FROM route_configs WHERE id = '$AI_ROUTE_CONFIG_ID'" | grep -q "$AI_SECRET_VALUE"; then
  fail "AI provider credential leaked into materialized route config"
fi
AI_MULTI_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-multi\",\"spec\":{\"listener_port\":$AI_MULTI_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_PROVIDER_ID\",\"models\":[],\"weight\":1},{\"provider_id\":\"$AI_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}")
AI_MULTI_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_MULTI_ROUTE_BODY")
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "ai-ai-e2e-multi-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-multi-listener" /tmp/fp-e2e-ai-dump.json || fail "AI multi-backend listener did not converge"
: >/tmp/fp-e2e-ai-auth.log
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-multi.json -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_MULTI_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "200" ] || fail "AI multi-backend route did not inject credential via upstream processor (code $AI_CODE, body $(cat /tmp/fp-e2e-ai-multi.json))"
grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-auth.log || fail "AI multi-backend provider did not receive injected credential"
AI_MULTI_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_MULTI_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_MULTI_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-multi >/dev/null
AI_MULTI_ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM clusters WHERE owner_kind = 'ai' AND owner_id = '$AI_MULTI_ROUTE_ID') + \
  (SELECT count(*) FROM route_configs WHERE owner_kind = 'ai' AND owner_id = '$AI_MULTI_ROUTE_ID') + \
  (SELECT count(*) FROM listeners WHERE owner_kind = 'ai' AND owner_id = '$AI_MULTI_ROUTE_ID')")
[ "$AI_MULTI_ORPHANS" = "0" ] || fail "AI multi-backend route cleanup left $AI_MULTI_ORPHANS owned gateway rows"
AI_FALLBACK_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-fallback-secret").decode())')
AI_FALLBACK_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-fallback-key\",\"description\":\"AI e2e fallback credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_FALLBACK_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_UNAVAILABLE_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-unavailable-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_UNAVAILABLE_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_UNAVAILABLE_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_UNAVAILABLE_PROVIDER_BODY")
AI_FALLBACK_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-fallback-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_FALLBACK_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_FALLBACK_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_FALLBACK_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_FALLBACK_PROVIDER_BODY")
AI_FAILOVER_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-failover\",\"spec\":{\"listener_port\":$AI_FAILOVER_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_UNAVAILABLE_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":0},{\"provider_id\":\"$AI_FALLBACK_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":1}]}}")
AI_FAILOVER_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_FAILOVER_ROUTE_BODY")
AI_FAILOVER_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$TEAM_ID' AND name = 'ai-ai-e2e-failover-routes' AND owner_kind = 'ai'")
[ -n "$AI_FAILOVER_ROUTE_CONFIG_ID" ] || fail "AI failover route materialized route config not found"
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json && grep -q "ai-ai-e2e-failover-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json || fail "AI failover aggregate cluster did not converge"
: >/tmp/fp-e2e-ai-fallback-auth.log
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-failover.json -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_FAILOVER_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "200" ] || fail "AI priority failover did not reach fallback provider (code $AI_CODE, body $(cat /tmp/fp-e2e-ai-failover.json))"
grep -q "mock-ai-ok" /tmp/fp-e2e-ai-failover.json || fail "AI failover mock response did not reach client"
grep -q "Bearer fp-e2e-ai-fallback-secret" /tmp/fp-e2e-ai-fallback-auth.log || fail "AI failover fallback provider did not receive fallback credential"
if grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-fallback-auth.log; then
  fail "AI failover fallback provider received primary credential"
fi
for i in $(seq 1 20); do
  AI_FAILOVER_USAGE_ATTR=$(psql "$PG_DB_URL" -Atc "SELECT provider_id || ',' || COALESCE(backend_position::text, '') || ',' || total_tokens FROM ai_usage_events WHERE team_id = '$TEAM_ID' AND route_config_id = '$AI_FAILOVER_ROUTE_CONFIG_ID' ORDER BY created_at DESC LIMIT 1" 2>/dev/null || true)
  [ "$AI_FAILOVER_USAGE_ATTR" = "$AI_FALLBACK_PROVIDER_ID,1,5" ] && break
  sleep 1
done
[ "$AI_FAILOVER_USAGE_ATTR" = "$AI_FALLBACK_PROVIDER_ID,1,5" ] || fail "AI failover usage attribution unexpected: $AI_FAILOVER_USAGE_ATTR"
AI_FAILOVER_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_FAILOVER_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_FAILOVER_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-failover >/dev/null
AI_FALLBACK_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_FALLBACK_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_FALLBACK_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-fallback-provider >/dev/null
AI_UNAVAILABLE_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_UNAVAILABLE_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_UNAVAILABLE_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-unavailable-provider >/dev/null
AI_BUDGET_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/budgets \
  -d "{\"name\":\"ai-e2e-budget\",\"spec\":{\"mode\":\"enforcing\",\"limit_units\":5,\"window_seconds\":3600,\"provider_id\":\"$AI_PROVIDER_ID\",\"route_config_id\":\"$AI_ROUTE_CONFIG_ID\",\"prompt_token_weight\":1,\"completion_token_weight\":1}}")
AI_BUDGET_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_BUDGET_BODY")
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-metered.json -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "200" ] || fail "AI metered request failed before budget trip (code $AI_CODE)"
for i in $(seq 1 20); do
  AI_USED_UNITS=$(psql "$PG_DB_URL" -Atc "SELECT COALESCE(sum(used_units), 0) FROM ai_budget_counters WHERE budget_id = '$AI_BUDGET_ID'" 2>/dev/null || echo 0)
  [ "$AI_USED_UNITS" = "5" ] && break
  sleep 1
done
[ "$AI_USED_UNITS" = "5" ] || fail "AI budget counter did not settle to 5 units, got $AI_USED_UNITS"
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-blocked.txt -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "429" ] || fail "AI budget did not trip on second metered request (code $AI_CODE, body $(cat /tmp/fp-e2e-ai-blocked.txt))"
grep -q "AI budget" /tmp/fp-e2e-ai-blocked.txt || fail "AI budget 429 body did not name budget failure"
AI_USAGE_ATTR=$(psql "$PG_DB_URL" -Atc "SELECT provider_id || ',' || COALESCE(backend_position::text, '') || ',' || total_tokens FROM ai_usage_events WHERE team_id = '$TEAM_ID' AND route_config_id = '$AI_ROUTE_CONFIG_ID' ORDER BY created_at DESC LIMIT 1")
[ "$AI_USAGE_ATTR" = "$AI_PROVIDER_ID,0,5" ] || fail "AI usage attribution unexpected: $AI_USAGE_ATTR"
AI_USAGE_ROW=$(psql "$PG_DB_URL" -Atc "SELECT row_to_json(t)::text FROM (SELECT * FROM ai_usage_events WHERE route_config_id = '$AI_ROUTE_CONFIG_ID') t")
if grep -q "$AI_SECRET_VALUE" <<<"$AI_USAGE_ROW"; then
  fail "AI provider credential leaked into usage rows"
fi
./target/debug/flowplane --json ai usage --route-config-id "$AI_ROUTE_CONFIG_ID" >/tmp/fp-e2e-ai-usage.json
python3 - "$AI_PROVIDER_ID" /tmp/fp-e2e-ai-usage.json <<'PY' || fail "flowplane ai usage did not show attributed token usage"
import json, sys
rows = json.load(open(sys.argv[2], encoding="utf-8"))
provider_id = sys.argv[1]
assert any(row["provider_id"] == provider_id and row["total_tokens"] >= 5 for row in rows)
PY
AI_BUDGET_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_budgets WHERE id = '$AI_BUDGET_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_BUDGET_REV" \
  http://$API/api/v1/teams/default/ai/budgets/ai-e2e-budget >/dev/null
AI_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e >/dev/null
AI_ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM clusters WHERE owner_kind = 'ai' AND owner_id = '$AI_ROUTE_ID') + \
  (SELECT count(*) FROM route_configs WHERE owner_kind = 'ai' AND owner_id = '$AI_ROUTE_ID') + \
  (SELECT count(*) FROM listeners WHERE owner_kind = 'ai' AND owner_id = '$AI_ROUTE_ID')")
[ "$AI_ORPHANS" = "0" ] || fail "AI route cleanup left $AI_ORPHANS owned gateway rows"
AI_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-provider >/dev/null
echo "PHASE 1a OK: AI credential injection (single + multi-backend) -> priority failover credential/usage -> enforcing budget trip -> usage attribution -> cleanup"

# ---- Phase 1d: AI streaming-failover boundary. Once the higher-priority backend has started
# streaming a response (200 + first chunk sent downstream), a mid-stream backend failure must be
# terminal: Envoy cannot fail over to the lower-priority backend after the response is committed,
# and the client keeps the partial stream. We prove this with a primary mock that emits one SSE
# chunk then RSTs the connection, and assert the lower-priority fallback is never contacted.
cat >/tmp/fp-e2e-ai-stream-die.py <<'PY'
import socket
import struct
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
auth_log = sys.argv[2]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or 0)
        if length:
            self.rfile.read(length)
        with open(auth_log, "a", encoding="utf-8") as f:
            f.write(self.headers.get("authorization", "") + "\n")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(b'data: {"choices":[{"delta":{"content":"partial-stream"}}]}\n\n')
        self.wfile.flush()
        # Abruptly reset mid-stream (SO_LINGER 0 -> RST) after the first chunk is on the wire.
        self.close_connection = True
        try:
            self.connection.setsockopt(
                socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0)
            )
            self.connection.close()
        except OSError:
            pass


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
: >/tmp/fp-e2e-ai-stream-die-auth.log
: >/tmp/fp-e2e-ai-stream-fallback-auth.log
python3 /tmp/fp-e2e-ai-stream-die.py "$AI_STREAM_DIE_PORT" /tmp/fp-e2e-ai-stream-die-auth.log \
  >/tmp/fp-e2e-ai-stream-die.log 2>&1 &
AI_STREAM_DIE_PID=$!
python3 /tmp/fp-e2e-ai-provider.py "$AI_STREAM_FALLBACK_PORT" /tmp/fp-e2e-ai-stream-fallback-auth.log \
  "Bearer fp-e2e-ai-stream-fallback" >/tmp/fp-e2e-ai-stream-fallback.log 2>&1 &
AI_STREAM_FALLBACK_PID=$!

AI_STREAM_PRIMARY_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-stream-primary").decode())')
AI_STREAM_PRIMARY_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-stream-primary-key\",\"description\":\"stream primary\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_STREAM_PRIMARY_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_FALLBACK_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-stream-fallback").decode())')
AI_STREAM_FALLBACK_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-stream-fallback-key\",\"description\":\"stream fallback\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_STREAM_FALLBACK_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_PRIMARY_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-stream-primary\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_STREAM_DIE_PORT\",\"credential_secret_id\":\"$AI_STREAM_PRIMARY_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_FALLBACK_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-stream-fallback\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_STREAM_FALLBACK_PORT\",\"credential_secret_id\":\"$AI_STREAM_FALLBACK_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_ROUTE_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-stream\",\"spec\":{\"listener_port\":$AI_STREAM_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_STREAM_PRIMARY_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":0},{\"provider_id\":\"$AI_STREAM_FALLBACK_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":1}]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
for i in $(seq 1 60); do
  curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json 2>/dev/null || true
  grep -q "ai-ai-e2e-stream-listener" /tmp/fp-e2e-ai-dump.json \
    && grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-stream-listener" /tmp/fp-e2e-ai-dump.json \
  && grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json \
  || fail "AI streaming route listener/aggregate cluster did not converge"
AI_STREAM_CODE=$(curl -sN --max-time 10 -o /tmp/fp-e2e-ai-stream-body.txt -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data '{"model":"gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}' \
  "http://127.0.0.1:$AI_STREAM_GATEWAY_PORT/v1/chat/completions" 2>/dev/null || true)
if [ "$AI_STREAM_CODE" = "200" ]; then
  grep -q "partial-stream" /tmp/fp-e2e-ai-stream-body.txt \
    || fail "AI streaming client did not receive the partial stream chunk"
  grep -q "Bearer fp-e2e-ai-stream-primary" /tmp/fp-e2e-ai-stream-die-auth.log \
    || fail "AI streaming primary did not receive its injected credential"
  [ ! -s /tmp/fp-e2e-ai-stream-fallback-auth.log ] \
    || fail "AI failed over AFTER stream start: fallback was contacted ($(cat /tmp/fp-e2e-ai-stream-fallback-auth.log))"
  AI_STREAM_RESULT="PHASE 1d OK: AI streaming-failover boundary -> partial stream delivered, no failover after first byte, fallback untouched"
else
  # D9 / #67: AI gateway 500s on stream:true requests before reaching a backend. The boundary
  # logic itself is verified on the non-stream path; this asserts the boundary once #67 lands.
  known_fail "PHASE 1d: streaming request returned $AI_STREAM_CODE (#67: AI gateway 500s on stream:true before backend dispatch)"
  AI_STREAM_RESULT="PHASE 1d KNOWN-FAIL: streaming blocked by #67 (code $AI_STREAM_CODE)"
fi
AI_STREAM_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_STREAM_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_STREAM_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-stream >/dev/null
for prov in ai-e2e-stream-primary ai-e2e-stream-fallback; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE team_id = '$TEAM_ID' AND name = '$prov'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/default/ai/providers/$prov" >/dev/null
done
kill "$AI_STREAM_DIE_PID" "$AI_STREAM_FALLBACK_PID" >/dev/null 2>&1 || true
echo "$AI_STREAM_RESULT"

# ---- Phase 1e: malformed provider response. A backend that returns a 200 with a non-OpenAI,
# non-JSON body must be passed through to the client without the gateway 500ing, and must not
# settle any usage (missing/unparseable usage is fail-open per D-018) -- proves robustness to
# providers that don't speak clean OpenAI.
cat >/tmp/fp-e2e-ai-malformed.py <<'PY'
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or 0)
        if length:
            self.rfile.read(length)
        body = b"this is definitely not an openai json response"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
python3 /tmp/fp-e2e-ai-malformed.py "$AI_MALFORMED_PORT" >/tmp/fp-e2e-ai-malformed.log 2>&1 &
AI_MALFORMED_PID=$!
AI_MALFORMED_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-malformed").decode())')
AI_MALFORMED_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-malformed-key\",\"description\":\"malformed\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_MALFORMED_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_MALFORMED_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-malformed\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_MALFORMED_PORT\",\"credential_secret_id\":\"$AI_MALFORMED_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_MALFORMED_ROUTE_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-malformed\",\"spec\":{\"listener_port\":$AI_MALFORMED_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_MALFORMED_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_MALFORMED_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$TEAM_ID' AND name = 'ai-ai-e2e-malformed-routes' AND owner_kind = 'ai'")
for i in $(seq 1 60); do
  curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json 2>/dev/null || true
  grep -q "ai-ai-e2e-malformed-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-malformed-listener" /tmp/fp-e2e-ai-dump.json || fail "AI malformed route listener did not converge"
AI_MALFORMED_CODE=""
for i in $(seq 1 30); do
  AI_MALFORMED_CODE=$(curl -sS --max-time 10 -o /tmp/fp-e2e-ai-malformed-body.txt -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    "http://127.0.0.1:$AI_MALFORMED_GATEWAY_PORT/v1/chat/completions" 2>/dev/null || true)
  [ "$AI_MALFORMED_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_MALFORMED_CODE" = "200" ] \
  || fail "AI malformed-provider response was not passed through (gateway returned $AI_MALFORMED_CODE)"
grep -q "not an openai json response" /tmp/fp-e2e-ai-malformed-body.txt \
  || fail "AI malformed-provider body was not delivered to the client"
AI_MALFORMED_USAGE=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM ai_usage_events WHERE team_id = '$TEAM_ID' AND route_config_id = '$AI_MALFORMED_ROUTE_CONFIG_ID'")
[ "$AI_MALFORMED_USAGE" = "0" ] \
  || fail "AI malformed-provider response settled usage ($AI_MALFORMED_USAGE rows); unparseable usage must be fail-open (no settlement)"
AI_MALFORMED_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_MALFORMED_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_MALFORMED_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-malformed >/dev/null
AI_MALFORMED_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_MALFORMED_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_MALFORMED_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-malformed >/dev/null
kill "$AI_MALFORMED_PID" >/dev/null 2>&1 || true
echo "PHASE 1e OK: AI malformed-provider response passed through, no 500, no usage settled (fail-open)"

# ---- Phase 1b: learning capture through the real injected ALS/ExtProc path. Start a
# route-scoped session on the resources created by expose, then send traffic with stable
# request IDs so ALS metadata and ExtProc body observations merge into raw_observations.
RC_ID=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams/default/route-configs/e2e-routes \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
LISTENER_ID=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams/default/listeners/e2e \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
./target/debug/flowplane api create e2e-api \
  --route-config-id "$RC_ID" --listener-id "$LISTENER_ID" >/tmp/fp-e2e-api-create.txt
API_CREATED=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM api_definitions WHERE name = 'e2e-api'")
[ "$API_CREATED" = "1" ] || fail "api create did not persist API"
./target/debug/flowplane learn start e2e-capture \
  --api e2e-api \
  --target-sample-count 2 --max-bytes 65536 --max-distinct-paths 5 >/tmp/fp-e2e-learn-start.txt
grep -q "e2e-capture" /tmp/fp-e2e-learn-start.txt || fail "learn start did not render session"
for i in $(seq 1 40); do
  curl -fsS -H "x-request-id: fp-e2e-learn-1" -H "x-api-key: secret-one" \
    http://127.0.0.1:$GW_PORT/ >/dev/null 2>&1 || true
  curl -fsS -H "x-request-id: fp-e2e-learn-2" -H "x-api-key: secret-two" \
    http://127.0.0.1:$GW_PORT/ >/dev/null 2>&1 || true
  LEARN_COUNTS=$(psql "$PG_DB_URL" -Atc "SELECT sample_count || ',' || path_count || ',' || drop_count || ',' || status FROM capture_sessions WHERE name = 'e2e-capture'" 2>/dev/null || true)
  RAW_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.name = 'e2e-capture'" 2>/dev/null || echo 0)
  BODY_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.name = 'e2e-capture' AND ro.body_seen" 2>/dev/null || echo 0)
  if [ "$LEARN_COUNTS" = "2,1,0,completed" ] && [ "$RAW_COUNT" = "2" ] && [ "$BODY_COUNT" -ge 1 ]; then
    break
  fi
  sleep 1
done
[ "$LEARN_COUNTS" = "2,1,0,completed" ] || fail "learning counters unexpected: $LEARN_COUNTS"
[ "$RAW_COUNT" = "2" ] || fail "expected two raw observations, got $RAW_COUNT"
[ "$BODY_COUNT" -ge 1 ] || fail "expected ExtProc body capture, got $BODY_COUNT body rows"
REDACTED_KEYS=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.name = 'e2e-capture' AND ro.request_headers->>'x-api-key' = '[REDACTED]'" 2>/dev/null || echo 0)
[ "$REDACTED_KEYS" = "2" ] || fail "expected x-api-key redaction on both raw observations, got $REDACTED_KEYS"
./target/debug/flowplane learn get e2e-capture >/tmp/fp-e2e-learn-get.txt
grep -q "completed" /tmp/fp-e2e-learn-get.txt || fail "learn get did not show completed session"
./target/debug/flowplane learn generate-spec e2e-capture >/tmp/fp-e2e-learn-spec.txt
API_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM api_definitions WHERE name = 'e2e-api'")
SPEC_VERSION=$(psql "$PG_DB_URL" -Atc "SELECT version FROM spec_versions WHERE api_definition_id = '$API_ID' AND source_kind = 'learned' ORDER BY version DESC LIMIT 1")
[ -n "$SPEC_VERSION" ] || fail "learned spec version was not persisted"
./target/debug/flowplane api spec publish e2e-api "$SPEC_VERSION" --reason "e2e approved" >/tmp/fp-e2e-publish.txt
PUBLISHED_ID=$(psql "$PG_DB_URL" -Atc "SELECT published_spec_version_id FROM api_definitions WHERE id = '$API_ID'")
TOOL_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM api_tools WHERE api_definition_id = '$API_ID' AND spec_version_id = '$PUBLISHED_ID'")
[ -n "$PUBLISHED_ID" ] || fail "api did not record published spec pointer"
[ "$TOOL_COUNT" -ge 1 ] || fail "published learned spec did not generate tools"
API_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM api_definitions WHERE id = '$API_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $API_REV" \
  http://$API/api/v1/teams/default/api-definitions/e2e-api >/dev/null
ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM api_route_bindings WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM spec_versions WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM api_tools WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM spec_version_review_events WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM capture_sessions WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.api_definition_id = '$API_ID')")
[ "$ORPHANS" = "0" ] || fail "S8 API cleanup left $ORPHANS orphan rows"
echo "PHASE 1b OK: live capture -> learned spec -> publish -> generated tools -> API cleanup left zero S8 orphans"

# ---- Phase 1c: traffic-first discovery loop. A Flowplane-owned discovery listener captures
# unmatched traffic, learned specs are reviewed/published, and route generation apply replays
# the dry-run plan into durable gateway resources.
./target/debug/flowplane learn discover start e2e-discover \
  --upstream "127.0.0.1:$UPSTREAM_PORT" \
  --listener-port "$DISCOVERY_PORT" \
  --target-sample-count 2 --max-bytes 65536 --max-distinct-paths 5 >/tmp/fp-e2e-discover-start.txt
grep -q "e2e-discover" /tmp/fp-e2e-discover-start.txt || fail "learn discover start did not render session"
for i in $(seq 1 50); do
  curl -fsS -H "host: s9.example.test" -H "x-request-id: fp-e2e-discover-1" \
    "http://127.0.0.1:$DISCOVERY_PORT/v1/discovered/1" >/dev/null 2>&1 || true
  curl -fsS -H "host: s9.example.test" -H "x-request-id: fp-e2e-discover-2" \
    "http://127.0.0.1:$DISCOVERY_PORT/v1/discovered/2" >/dev/null 2>&1 || true
  DISC_RAW_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM discovery_raw_observations dro JOIN discovery_sessions ds ON ds.id = dro.discovery_session_id WHERE ds.name = 'e2e-discover'" 2>/dev/null || echo 0)
  [ "$DISC_RAW_COUNT" = "2" ] && break
  sleep 1
done
[ "$DISC_RAW_COUNT" = "2" ] || fail "expected two discovery raw observations, got $DISC_RAW_COUNT"
./target/debug/flowplane learn discover stop e2e-discover >/tmp/fp-e2e-discover-stop.txt
DISCOVERY_STATUS=$(psql "$PG_DB_URL" -Atc "SELECT status FROM discovery_sessions WHERE name = 'e2e-discover'")
[ "$DISCOVERY_STATUS" = "completed" ] || fail "learn discover stop did not complete session: $DISCOVERY_STATUS"
DISCOVERY_ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM clusters c JOIN discovery_sessions ds ON ds.id = c.owner_id WHERE ds.name = 'e2e-discover' AND c.owner_kind = 'discovery') + \
  (SELECT count(*) FROM route_configs rc JOIN discovery_sessions ds ON ds.id = rc.owner_id WHERE ds.name = 'e2e-discover' AND rc.owner_kind = 'discovery') + \
  (SELECT count(*) FROM listeners l JOIN discovery_sessions ds ON ds.id = l.owner_id WHERE ds.name = 'e2e-discover' AND l.owner_kind = 'discovery')")
[ "$DISCOVERY_ORPHANS" = "0" ] || fail "discovery stop left $DISCOVERY_ORPHANS owned gateway rows"
./target/debug/flowplane learn discover generate-spec e2e-discover >/tmp/fp-e2e-discover-specs.txt
DISC_SPEC_ROW=$(psql "$PG_DB_URL" -AtF $'\t' -c "SELECT sv.id, sv.api_definition_id, sv.version, ad.name \
  FROM spec_versions sv JOIN api_definitions ad ON ad.id = sv.api_definition_id \
  WHERE sv.source_kind = 'learned' \
    AND sv.spec->'x-flowplane-learning-source'->>'discovery_session_name' = 'e2e-discover' \
  ORDER BY sv.created_at DESC LIMIT 1")
IFS=$'\t' read -r DISC_SPEC_ID DISC_API_ID DISC_SPEC_VERSION DISC_API_NAME <<<"$DISC_SPEC_ROW"
[ -n "$DISC_SPEC_ID" ] && [ -n "$DISC_API_NAME" ] || fail "discovery learned spec was not persisted"
DISC_OBSERVED_HOST=$(psql "$PG_DB_URL" -Atc "SELECT spec->'x-flowplane-learning-source'->>'observed_host' FROM spec_versions WHERE id = '$DISC_SPEC_ID'")
./target/debug/flowplane api spec publish "$DISC_API_NAME" "$DISC_SPEC_VERSION" --reason "s9 e2e approved" >/tmp/fp-e2e-discover-publish.txt
DISC_PUBLISHED_ID=$(psql "$PG_DB_URL" -Atc "SELECT published_spec_version_id FROM api_definitions WHERE id = '$DISC_API_ID'")
DISC_TOOL_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM api_tools WHERE api_definition_id = '$DISC_API_ID' AND spec_version_id = '$DISC_PUBLISHED_ID'")
[ "$DISC_PUBLISHED_ID" = "$DISC_SPEC_ID" ] || fail "discovery API did not publish learned spec"
[ "$DISC_TOOL_COUNT" -ge 1 ] || fail "published discovery spec did not generate tools"
./target/debug/flowplane route generate --from-spec "$DISC_SPEC_ID" --listener-port "$GENERATED_ROUTE_PORT" >/tmp/fp-e2e-route-plan.txt
DISC_PLAN_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_generation_plans WHERE spec_version_id = '$DISC_SPEC_ID' ORDER BY created_at DESC LIMIT 1")
[ -n "$DISC_PLAN_ID" ] || fail "route generation plan was not persisted"
./target/debug/flowplane route apply "$DISC_PLAN_ID" >/tmp/fp-e2e-route-apply.txt
PLAN_MATCH=$(psql "$PG_DB_URL" -Atc "SELECT ((rgp.plan->'cluster_spec') = c.spec AND (rgp.plan->'route_config_spec') = rc.spec AND (rgp.plan->'listener_spec') = l.spec) \
  FROM route_generation_plans rgp \
  JOIN clusters c ON c.team_id = rgp.team_id AND c.name = rgp.plan->>'cluster_name' \
  JOIN route_configs rc ON rc.team_id = rgp.team_id AND rc.name = rgp.plan->>'route_config_name' \
  JOIN listeners l ON l.team_id = rgp.team_id AND l.name = rgp.plan->>'listener_name' \
  WHERE rgp.id = '$DISC_PLAN_ID' AND rgp.status = 'applied'")
[ "$PLAN_MATCH" = "t" ] || fail "applied route resources did not match dry-run plan"
for i in $(seq 1 40); do
  BODY=$(curl -fsS -H "host: $DISC_OBSERVED_HOST" "http://127.0.0.1:$GENERATED_ROUTE_PORT/v1/discovered/1" 2>/dev/null || true)
  [[ "$BODY" == hello-from-upstream-* ]] && break
  sleep 1
done
[[ "$BODY" == hello-from-upstream-* ]] || fail "generated traffic-first route never flowed"
echo "PHASE 1c OK: traffic-first discovery -> learned spec -> publish/tools -> dry-run/apply -> generated route served '$BODY'"

# ---- Phase 2: restart convergence. Kill the CP while Envoy keeps running; the restarted
# CP must prime its snapshot cache from the DB (not wipe the dataplane) and a post-restart
# mutation must reach the already-connected Envoy.
echo "hello-from-upstream2-$(date +%s)" > /tmp/fp-e2e-www2.html
mkdir -p /tmp/fp-e2e-www2 && cp /tmp/fp-e2e-www2.html /tmp/fp-e2e-www2/index.html
(cd /tmp/fp-e2e-www2 && python3 -m http.server $((UPSTREAM_PORT+1)) >/dev/null 2>&1) &
UP2_PID=$!
trap '[ -n "${UP2_PID:-}" ] && kill $UP2_PID >/dev/null 2>&1 || true; cleanup' EXIT

kill "$CP_PID"; wait "$CP_PID" 2>/dev/null || true
CP_LOG=/tmp/fp-e2e-cp2.log
FLOWPLANE_DATABASE_URL=$PG_DB_URL \
FLOWPLANE_API_INSECURE=true FLOWPLANE_DEV_MODE=true \
FLOWPLANE_SECRET_ENCRYPTION_KEY=12345678901234567890123456789012 \
FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS=127.0.0.1:$UPSTREAM_PORT \
FLOWPLANE_API_ADDR=$API FLOWPLANE_XDS_ADDR=0.0.0.0:$XDS_PORT \
./target/debug/flowplane serve > "$CP_LOG" 2>&1 &
CP_PID=$!
for i in $(seq 1 40); do curl -fsS http://$API/healthz >/dev/null 2>&1 && break; sleep 0.5; done
TOKEN=$(grep -o '"dev_token":"[^"]*"' "$CP_LOG" | cut -d'"' -f4)
[ -n "$TOKEN" ] || fail "no dev token after restart"
export FLOWPLANE_TOKEN="$TOKEN"
auth=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")
grep -q "snapshot cache primed" "$CP_LOG" || fail "restarted CP did not prime the snapshot cache"

# Traffic must still flow on the original config while Envoy reconnects.
wait_body hello-from-upstream- 10 || fail "traffic broke across CP restart"

# Point the cluster at upstream2 via the restarted CP; Envoy must converge.
REV=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams/default/clusters/e2e-upstream \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['revision'])")
curl -fsS "${auth[@]}" -X PATCH -H "If-Match: $REV" http://$API/api/v1/teams/default/clusters/e2e-upstream \
  -d "{\"spec\":{\"endpoints\":[{\"host\":\"127.0.0.1\",\"port\":$((UPSTREAM_PORT+1))}]}}" >/dev/null
wait_body hello-from-upstream2- || fail "post-restart mutation never reached Envoy"
echo "PHASE 2 OK: CP restarted, Envoy survived and converged to '$BODY'"

# ---- Phase 3: cross-team isolation against the live Envoy. Resources for another team
# must never reach this team's dataplane.
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams -d '{"name":"e2e-blue"}' >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/e2e-blue/clusters \
  -d "{\"name\":\"blue-upstream\",\"spec\":{\"endpoints\":[{\"host\":\"127.0.0.1\",\"port\":$UPSTREAM_PORT}]}}" >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/e2e-blue/route-configs \
  -d '{"name":"blue-routes","spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{"name":"all","match":{"prefix":{"prefix":"/"}},"action":{"cluster":"blue-upstream"}}]}]}}' >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/e2e-blue/listeners \
  -d "{\"name\":\"blue-edge\",\"spec\":{\"address\":\"0.0.0.0\",\"port\":$((GW_PORT+1)),\"route_config\":\"blue-routes\"}}" >/dev/null
sleep 3
DUMP=$(curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump)
if echo "$DUMP" | grep -q "blue-upstream\|blue-edge\|blue-routes"; then
  fail "team e2e-blue resources leaked into team default's Envoy"
fi
if curl -fsS --max-time 2 http://127.0.0.1:$((GW_PORT+1))/ >/dev/null 2>&1; then
  fail "Envoy is serving another team's listener port"
fi
echo "PHASE 3 OK: e2e-blue resources never reached team default's dataplane"

# ---- Phase 4: HTTP filters through the live Envoy (S5.8). A second listener carries
# local_rate_limit (2 tokens/min) + header_mutation; the /quiet route disables the limit.
FILTER_PORT=$((GW_PORT+2))
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs -d '{
  "name":"e2e-filter-routes",
  "spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[
    {"name":"quiet","match":{"prefix":{"prefix":"/quiet"}},
     "action":{"cluster":"e2e-upstream","prefix_rewrite":"/"},
     "filter_overrides":[{"type":"disable","filter_type":"local_rate_limit"}]},
    {"name":"all","match":{"prefix":{"prefix":"/"}},"action":{"cluster":"e2e-upstream"}}
  ]}]}}' >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners -d "{
  \"name\":\"e2e-filtered\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$FILTER_PORT,\"route_config\":\"e2e-filter-routes\",
    \"http_filters\":[
      {\"filter\":{\"type\":\"local_rate_limit\",\"stat_prefix\":\"e2e\",
        \"token_bucket\":{\"max_tokens\":2,\"fill_interval_ms\":60000},\"status_code\":429}},
      {\"filter\":{\"type\":\"header_mutation\",
        \"response_headers_to_add\":[{\"key\":\"x-fp-e2e\",\"value\":\"on\"}]}}
    ]}}" >/dev/null

for i in $(seq 1 30); do
  curl -fsS --max-time 2 http://127.0.0.1:$FILTER_PORT/ >/dev/null 2>&1 && break
  sleep 1
done
HEADERS=$(curl -fsS -D - -o /dev/null http://127.0.0.1:$FILTER_PORT/ 2>/dev/null || true)
echo "$HEADERS" | grep -qi "x-fp-e2e: on" || fail "header_mutation response header missing"
# Two tokens were consumed by the readiness+header probes; the next hit must be 429.
CODE=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$FILTER_PORT/)
[ "$CODE" = "429" ] || fail "expected 429 from local_rate_limit, got $CODE"
# /quiet disables the limiter per route: always 200 even though the bucket is empty.
for i in 1 2 3; do
  CODE=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$FILTER_PORT/quiet)
  [ "$CODE" = "200" ] || fail "per-route disable failed: /quiet returned $CODE"
done
echo "PHASE 4 OK: rate limit enforced (429), /quiet exempt per-route, header mutation applied"

# ---- Phase 5: auth-grade filters (S5.8c). A listener carries jwt_auth (allow-missing, so
# unauthenticated traffic still flows — proves the filter ACKs and loads its JWKS cluster)
# plus rbac (DENY policy on /blocked). A real Envoy must ACCEPT this config (a malformed
# proto would NACK and the listener would never serve) and enforce the rbac path rule.
AUTH_PORT=$((GW_PORT+3))
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs -d '{
  "name":"e2e-auth-routes",
  "spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[
    {"name":"blocked","match":{"prefix":{"prefix":"/blocked"}},"action":{"cluster":"e2e-upstream"}},
    {"name":"all","match":{"prefix":{"prefix":"/"}},"action":{"cluster":"e2e-upstream"}}
  ]}]}}' >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners -d "{
  \"name\":\"e2e-auth\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$AUTH_PORT,\"route_config\":\"e2e-auth-routes\",
    \"http_filters\":[
      {\"filter\":{\"type\":\"jwt_auth\",
        \"providers\":{\"local\":{\"jwks\":{\"source\":\"remote\",
          \"uri\":\"http://127.0.0.1:$UPSTREAM_PORT/jwks\",\"cluster\":\"e2e-upstream\",\"timeout_ms\":5000}}},
        \"requirement_map\":{\"opt\":{\"kind\":\"allow_missing\"}},
        \"rules\":[{\"match\":{\"prefix\":{\"prefix\":\"/\"}},\"requirement_name\":\"opt\"}]}},
      {\"filter\":{\"type\":\"rbac\",\"action\":\"deny\",
        \"policies\":{\"block-path\":{
          \"permissions\":[{\"kind\":\"url_path\",\"prefix\":\"/blocked\"}],
          \"principals\":[{\"kind\":\"any\"}]}}}}
    ]}}" >/dev/null

for i in $(seq 1 30); do
  curl -fsS --max-time 2 http://127.0.0.1:$AUTH_PORT/ >/dev/null 2>&1 && break
  sleep 1
done
# Unauthenticated request flows (jwt allow_missing) and is not rbac-denied.
CODE=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$AUTH_PORT/)
[ "$CODE" = "200" ] || fail "auth listener did not serve open path (got $CODE — config likely NACKed)"
# rbac DENY enforces on /blocked.
CODE=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$AUTH_PORT/blocked)
[ "$CODE" = "403" ] || fail "rbac deny not enforced on /blocked (got $CODE)"
echo "PHASE 5 OK: real Envoy ACKed jwt_auth + rbac; rbac DENY enforced on /blocked"

# ---- Phase 5a: MCP api_* descriptor-following through the live gateway. MCP issues only a
# gateway_invocation descriptor; a Flowplane descriptor-aware client follows it by calling Envoy
# directly with normal gateway credentials. RBAC on the listener proves policy parity with a
# normal direct API consumer call and rejects missing credentials at the dataplane layer.
MCP_CREATE_BODY=/tmp/fp-e2e-mcp-create.json
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs \
  -o "$MCP_CREATE_BODY" -w '%{http_code}' -d '{
  "name":"e2e-mcp-routes",
  "spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{
    "name":"mcp-parity",
    "match":{"prefix":{"prefix":"/mcp-parity"}},
    "action":{"cluster":"e2e-upstream","prefix_rewrite":"/"}
  }]}]}}')
[ "$CODE" = "201" ] || fail "MCP parity route config create failed ($CODE): $(cat "$MCP_CREATE_BODY")"
MCP_ROUTE_CONFIG_ID=$(python3 -c "import sys,json;print(json.load(open(sys.argv[1], encoding='utf-8'))['id'])" "$MCP_CREATE_BODY")
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners \
  -o "$MCP_CREATE_BODY" -w '%{http_code}' -d "{
  \"name\":\"e2e-mcp\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$MCP_PARITY_PORT,
    \"public_base_url\":\"http://127.0.0.1:$MCP_PARITY_PORT\",
    \"route_config\":\"e2e-mcp-routes\",
    \"http_filters\":[{
      \"filter\":{\"type\":\"rbac\",\"action\":\"allow\",
        \"policies\":{\"api-key\":{
          \"permissions\":[{\"kind\":\"any\"}],
          \"principals\":[{\"kind\":\"header\",\"name\":\"x-api-key\",\"exact\":\"fp-mcp-secret\"}]}}}
    }]}}")
[ "$CODE" = "201" ] || fail "MCP parity listener create failed ($CODE): $(cat "$MCP_CREATE_BODY")"
MCP_LISTENER_ID=$(python3 -c "import sys,json;print(json.load(open(sys.argv[1], encoding='utf-8'))['id'])" "$MCP_CREATE_BODY")
cat >/tmp/fp-e2e-mcp-openapi.json <<'JSON'
{
  "openapi": "3.0.3",
  "info": {"title": "MCP parity", "version": "1"},
  "paths": {
    "/mcp-parity": {
      "get": {
        "operationId": "getMcpParity",
        "responses": {"200": {"description": "ok"}}
      }
    }
  }
}
JSON
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/api-definitions \
  -o "$MCP_CREATE_BODY" -w '%{http_code}' -d "{
  \"name\":\"e2e-mcp-api\",
  \"openapi\":$(cat /tmp/fp-e2e-mcp-openapi.json),
  \"route_binding\":{\"route_config_id\":\"$MCP_ROUTE_CONFIG_ID\",\"listener_id\":\"$MCP_LISTENER_ID\"}
}")
[ "$CODE" = "201" ] || fail "MCP parity API create failed ($CODE): $(cat "$MCP_CREATE_BODY")"
MCP_API_SPEC_ID=$(python3 -c "import sys,json;print(json.load(open(sys.argv[1], encoding='utf-8'))['latest_spec']['id'])" "$MCP_CREATE_BODY")
MCP_API_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM api_definitions WHERE team_id = '$TEAM_ID' AND name = 'e2e-mcp-api'")
# Imported specs generate tools at API-create time, but the review-loop publish command is
# learned-spec-only. Mark this fixture's imported spec as current so MCP lists the generated tool.
psql "$PG_DB_URL" -v ON_ERROR_STOP=1 -c \
  "UPDATE api_definitions SET published_spec_version_id = '$MCP_API_SPEC_ID' WHERE id = '$MCP_API_ID'" >/dev/null
MCP_TOOL_ROW=$(psql "$PG_DB_URL" -AtF $'\t' -c "SELECT name, spec_version_id FROM api_tools WHERE api_definition_id = '$MCP_API_ID' LIMIT 1")
IFS=$'\t' read -r MCP_TOOL_NAME MCP_TOOL_SPEC_ID <<<"$MCP_TOOL_ROW"
[ -n "$MCP_TOOL_NAME" ] && [ -n "$MCP_TOOL_SPEC_ID" ] || fail "MCP parity API tool was not generated"
MCP_TOOL="api_$MCP_TOOL_NAME"

for i in $(seq 1 30); do
  CODE=$(curl -s -o /tmp/fp-e2e-mcp-direct-body -w '%{http_code}' \
    -H 'x-api-key: fp-mcp-secret' "http://127.0.0.1:$MCP_PARITY_PORT/mcp-parity" 2>/dev/null || true)
  [ "$CODE" = "200" ] && break
  sleep 1
done
[ "$CODE" = "200" ] || fail "MCP parity direct credentialed call did not reach Envoy (got $CODE)"
grep -Eq "hello-from-upstream|hello-from-upstream2" /tmp/fp-e2e-mcp-direct-body \
  || fail "MCP parity direct call did not reach expected upstream"
CODE=$(curl -s -o /tmp/fp-e2e-mcp-denied-body -w '%{http_code}' \
  "http://127.0.0.1:$MCP_PARITY_PORT/mcp-parity")
[ "$CODE" = "403" ] || fail "MCP parity gateway policy did not reject missing credentials (got $CODE)"

MCP_AGENT_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/agents \
  -d "{\"name\":\"e2e-mcp-agent\",\"kind\":\"gateway-tool\",\"grants\":[{\"team_id\":\"$TEAM_ID\",\"resource\":\"mcp-tools\",\"action\":\"execute\"}]}")
MCP_AGENT_TOKEN=$(python3 -c "import sys,json;print(json.load(sys.stdin)['token'])" <<<"$MCP_AGENT_BODY")
MCP_HEADERS=/tmp/fp-e2e-mcp-headers.txt
curl -fsS -D "$MCP_HEADERS" -o /tmp/fp-e2e-mcp-init.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}' \
  http://$API/api/v1/mcp
MCP_SESSION=$(awk 'tolower($1)=="mcp-session-id:" {print $2}' "$MCP_HEADERS" | tr -d '\r')
[ -n "$MCP_SESSION" ] || fail "MCP initialize did not return a session id"
curl -fsS -o /tmp/fp-e2e-mcp-list.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" -H "mcp-session-id: $MCP_SESSION" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{\"team\":\"default\"}}" \
  http://$API/api/v1/mcp
python3 - "$MCP_TOOL" /tmp/fp-e2e-mcp-list.json <<'PY' || fail "MCP tools/list did not include generated api_* tool"
import json, sys
tool = sys.argv[1]
body = json.load(open(sys.argv[2], encoding="utf-8"))
assert any(row["name"] == tool for row in body["result"]["tools"])
PY
curl -fsS -o /tmp/fp-e2e-mcp-descriptor.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" -H "mcp-session-id: $MCP_SESSION" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"$MCP_TOOL\",\"arguments\":{\"team\":\"default\",\"headers\":{\"x-api-key\":\"fp-mcp-secret\"}}}}" \
  http://$API/api/v1/mcp
python3 - "$MCP_TOOL" "$MCP_TOOL_SPEC_ID" /tmp/fp-e2e-mcp-descriptor.json <<'PY' || fail "MCP descriptor shape/auth/source was unexpected"
import json, sys
tool, spec_id, path = sys.argv[1], sys.argv[2], sys.argv[3]
body = json.load(open(path, encoding="utf-8"))
result = body["result"]
assert result["isError"] is False
desc = result["structuredContent"]
assert desc["type"] == "gateway_invocation"
assert desc["tool"] == tool
assert desc["specVersionId"] == spec_id
assert desc["auth"]["mode"] == "caller_gateway_credentials"
assert desc["url"].startswith("http://127.0.0.1:")
assert "/mcp-parity" in desc["url"]
assert desc["headers"]["x-api-key"] == "fp-mcp-secret"
PY
MCP_DESC_URL=$(python3 -c "import json;print(json.load(open('/tmp/fp-e2e-mcp-descriptor.json', encoding='utf-8'))['result']['structuredContent']['url'])")
MCP_DESC_HOST=$(python3 -c "import json;print(json.load(open('/tmp/fp-e2e-mcp-descriptor.json', encoding='utf-8'))['result']['structuredContent']['headers'].get('host',''))")
MCP_DESC_KEY=$(python3 -c "import json;print(json.load(open('/tmp/fp-e2e-mcp-descriptor.json', encoding='utf-8'))['result']['structuredContent']['headers']['x-api-key'])")
DESC_HEADERS=(-H "x-api-key: $MCP_DESC_KEY")
[ -z "$MCP_DESC_HOST" ] || DESC_HEADERS+=(-H "host: $MCP_DESC_HOST")
CODE=$(curl -s -o /tmp/fp-e2e-mcp-desc-body -w '%{http_code}' "${DESC_HEADERS[@]}" "$MCP_DESC_URL")
[ "$CODE" = "200" ] || fail "MCP descriptor-following call did not reach Envoy (got $CODE)"
cmp -s /tmp/fp-e2e-mcp-direct-body /tmp/fp-e2e-mcp-desc-body \
  || fail "MCP descriptor-following response differed from direct gateway call"
CODE=$(curl -s -o /tmp/fp-e2e-mcp-desc-denied-body -w '%{http_code}' "$MCP_DESC_URL")
[ "$CODE" = "403" ] || fail "MCP descriptor URL without gateway credentials was not rejected by Envoy (got $CODE)"
curl -fsS -o /tmp/fp-e2e-mcp-cross-team.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" -H "mcp-session-id: $MCP_SESSION" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"$MCP_TOOL\",\"arguments\":{\"team\":\"e2e-blue\",\"headers\":{\"x-api-key\":\"fp-mcp-secret\"}}}}" \
  http://$API/api/v1/mcp
python3 - /tmp/fp-e2e-mcp-cross-team.json <<'PY' || fail "MCP cross-team descriptor issuance did not fail closed"
import json, sys
body = json.load(open(sys.argv[1], encoding="utf-8"))
assert body["result"]["isError"] is True
assert body["result"]["error"]["code"] == "forbidden"
PY
echo "PHASE 5a OK: MCP api_* descriptor followed directly through Envoy; RBAC policy parity and cross-team descriptor denial verified"

# ---- Phase 6: SDS-backed downstream TLS rotation. The listener references a
# tls_certificate secret by name; rotating the secret must update the certificate presented by
# the already-running Envoy without restart.
SDS_PORT=$((GW_PORT+4))
command -v openssl >/dev/null || fail "openssl is required for SDS rotation phase"
mkdir -p /tmp/fp-e2e-sds
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=fp-sds-one" \
  -keyout /tmp/fp-e2e-sds/one.key -out /tmp/fp-e2e-sds/one.crt >/dev/null 2>&1
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=fp-sds-two" \
  -keyout /tmp/fp-e2e-sds/two.key -out /tmp/fp-e2e-sds/two.crt >/dev/null 2>&1
python3 - /tmp/fp-e2e-sds/one.crt /tmp/fp-e2e-sds/one.key > /tmp/fp-e2e-sds/secret-one.json <<'PY'
import json, sys
cert, key = sys.argv[1], sys.argv[2]
print(json.dumps({
    "name": "edge-sds",
    "spec": {
        "type": "tls_certificate",
        "certificate_chain": open(cert).read(),
        "private_key": open(key).read(),
    },
}))
PY
python3 - /tmp/fp-e2e-sds/two.crt /tmp/fp-e2e-sds/two.key > /tmp/fp-e2e-sds/secret-two.json <<'PY'
import json, sys
cert, key = sys.argv[1], sys.argv[2]
print(json.dumps({
    "spec": {
        "type": "tls_certificate",
        "certificate_chain": open(cert).read(),
        "private_key": open(key).read(),
    },
}))
PY
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  --data-binary @/tmp/fp-e2e-sds/secret-one.json >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners -d "{
  \"name\":\"e2e-sds\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$SDS_PORT,\"route_config\":\"e2e-auth-routes\",
    \"tls_context\":{\"tls_certificate_sds_secret_name\":\"edge-sds\"}}}" >/dev/null

for i in $(seq 1 40); do
  curl -fksS --max-time 2 https://127.0.0.1:$SDS_PORT/ >/dev/null 2>&1 && break
  sleep 1
done
SUBJECT=$(echo | openssl s_client -connect 127.0.0.1:$SDS_PORT -servername localhost 2>/dev/null \
  | openssl x509 -noout -subject 2>/dev/null || true)
echo "$SUBJECT" | grep -q "fp-sds-one" || fail "SDS listener did not present initial cert (subject: $SUBJECT)"
SECRET_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM secrets WHERE name = 'edge-sds'")
curl -fsS "${auth[@]}" -X POST -H "If-Match: $SECRET_REV" http://$API/api/v1/teams/default/secrets/edge-sds/rotate \
  --data-binary @/tmp/fp-e2e-sds/secret-two.json >/dev/null
for i in $(seq 1 40); do
  SUBJECT=$(echo | openssl s_client -connect 127.0.0.1:$SDS_PORT -servername localhost 2>/dev/null \
    | openssl x509 -noout -subject 2>/dev/null || true)
  echo "$SUBJECT" | grep -q "fp-sds-two" && break
  sleep 1
done
echo "$SUBJECT" | grep -q "fp-sds-two" || fail "SDS rotation did not update Envoy cert (subject: $SUBJECT)"
curl -fksS --max-time 2 https://127.0.0.1:$SDS_PORT/ >/dev/null 2>&1 \
  || fail "HTTPS traffic failed after SDS rotation"
echo "PHASE 6 OK: SDS TLS secret rotated live; Envoy presented the new certificate"

# ---- Phase 7: S7.8 field parity ACK/smoke. This proves the richer V2 gateway IR is accepted
# by a real Envoy and not only by unit-level proto decoding.
ADV_PORT=$((GW_PORT+5))
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/clusters \
  -d "{\"name\":\"e2e-canary\",\"spec\":{\"endpoints\":[{\"host\":\"127.0.0.1\",\"port\":$((UPSTREAM_PORT+1))}]}}" >/dev/null
ADV_CREATE_BODY=/tmp/fp-e2e-advanced-create.json
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs \
  -o "$ADV_CREATE_BODY" -w '%{http_code}' -d "{
  \"name\":\"e2e-advanced-routes\",
  \"spec\":{\"virtual_hosts\":[{
    \"name\":\"default\",
    \"domains\":[\"*\"],
    \"routes\":[{
      \"name\":\"advanced\",
      \"match\":{\"regex\":{\"pattern\":\"^/v[0-9]+/items.*$\"}},
      \"headers\":[{\"name\":\"x-api-version\",\"type\":\"exact\",\"value\":\"2\"}],
      \"query_parameters\":[{\"name\":\"preview\",\"type\":\"present\",\"value\":true}],
      \"action\":{
        \"weighted_clusters\":[
          {\"cluster\":\"e2e-upstream\",\"weight\":80},
          {\"cluster\":\"e2e-canary\",\"weight\":20}
        ],
        \"timeout_secs\":10,
        \"retry_policy\":{\"retry_on\":\"5xx,connect-failure\",\"num_retries\":2,\"per_try_timeout_secs\":3,
          \"retriable_status_codes\":[502,503]},
        \"rate_limits\":[{
          \"actions\":[{\"type\":\"request_headers\",\"header_name\":\"x-api-key\",\"descriptor_key\":\"api_key\"}]
        }]
      }
    },{
      \"name\":\"advanced-smoke\",
      \"match\":{\"prefix\":{\"prefix\":\"/advanced-smoke\"}},
      \"action\":{
        \"weighted_clusters\":[
          {\"cluster\":\"e2e-upstream\",\"weight\":80},
          {\"cluster\":\"e2e-canary\",\"weight\":20}
        ],
        \"timeout_secs\":10,
        \"retry_policy\":{\"retry_on\":\"5xx,connect-failure\",\"num_retries\":2,\"per_try_timeout_secs\":3,
          \"retriable_status_codes\":[502,503]},
        \"prefix_rewrite\":\"/\",
        \"rate_limits\":[{
          \"actions\":[{\"type\":\"generic_key\",\"descriptor_value\":\"advanced-smoke\",\"descriptor_key\":\"route\"}]
        }]
      }
    }]
  }]}}")
[ "$CODE" = "201" ] || fail "advanced route config create failed ($CODE): $(cat "$ADV_CREATE_BODY")"
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners \
  -o "$ADV_CREATE_BODY" -w '%{http_code}' -d "{
  \"name\":\"e2e-advanced\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$ADV_PORT,\"protocol\":\"http2\",
    \"route_config\":\"e2e-advanced-routes\",
    \"access_logs\":[{\"path\":\"/tmp/fp-e2e-advanced-access.log\"}],
    \"http_filters\":[{
      \"filter\":{\"type\":\"global_rate_limit\",\"domain\":\"flowplane\",\"service_cluster\":\"e2e-upstream\",
        \"timeout_ms\":50,\"failure_mode_deny\":false,\"request_type\":\"external\",
        \"enable_x_ratelimit_headers\":true}
    }]}}")
[ "$CODE" = "201" ] || fail "advanced listener create failed ($CODE): $(cat "$ADV_CREATE_BODY")"

for i in $(seq 1 30); do
  CODE=$(curl --http2-prior-knowledge -s -o /tmp/fp-e2e-advanced-body -w '%{http_code}' \
    -H 'x-api-version: 2' -H 'x-api-key: smoke' \
    "http://127.0.0.1:$ADV_PORT/advanced-smoke" 2>/dev/null || true)
  [ "$CODE" = "200" ] && break
  sleep 1
done
[ "$CODE" = "200" ] || fail "advanced parity listener did not serve matching request (got $CODE)"
grep -Eq "hello-from-upstream|hello-from-upstream2" /tmp/fp-e2e-advanced-body \
  || fail "advanced parity request did not reach an expected weighted upstream"
# Poll for a single, consistent config_dump that contains the e2e-advanced listener AND its
# global rate-limit filter. Bounded curl (--max-time) avoids a hung/partial read of the large
# dump being misread as "filter absent", and requiring both tokens in the same snapshot avoids a
# mid-rebuild window where the listener is transiently out of the snapshot (#64 polled the filter
# string only, single-shot curl, and still flaked ~1/3 runs).
ADV_FILTER_READY=0
for i in $(seq 1 60); do
  ADV_DUMP=$(curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump 2>/dev/null || true)
  if grep -q "e2e-advanced" <<<"$ADV_DUMP" \
    && grep -q "envoy.filters.http.ratelimit" <<<"$ADV_DUMP"; then
    ADV_FILTER_READY=1
    break
  fi
  sleep 1
done
[ "$ADV_FILTER_READY" = "1" ] || fail "advanced config dump missing global rate-limit filter"
echo "PHASE 7 OK: advanced route/listener/filter parity ACKed and served traffic"

redaction_sweep

if [ "$KNOWN_FAIL_COUNT" -gt 0 ]; then
  echo "E2E INCOMPLETE: $KNOWN_FAIL_COUNT known failure(s) recorded (tracked product bugs); all other phases + redaction sweep passed"
  exit 1
fi
echo "E2E PASSED: traffic, learning capture, traffic-first discovery, restart convergence, cross-team isolation, http filters, auth filters, MCP descriptor gateway parity, SDS rotation, advanced parity, AI streaming boundary, redaction sweep"
exit 0
