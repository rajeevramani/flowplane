# Shared library for the live Envoy E2E suite. Sourced by scripts/e2e/run.sh.
# Holds the port/env contract, ENVOY_VERSION pin, cleanup trap target, the assertion/wait
# helpers, and setup_harness() (boot Postgres DB + mock upstreams + AI providers + control
# plane + dataplane + a real Envoy). No `set -euo pipefail` here — the runner owns that.

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

# Envoy image pin: single override point. Default preserves the suite's historical tag exactly.
ENVOY_VERSION=${ENVOY_VERSION:-1.37-latest}

# Teardown target for the runner's `trap cleanup EXIT`. UP2_PID (started by phase P2) is reaped
# here centrally so phases never re-install the trap.
cleanup() {
  docker rm -f fp-e2e-envoy >/dev/null 2>&1 || true
  docker rm -f fp-e2e-envoy-trace >/dev/null 2>&1 || true
  [ -n "${CP_PID:-}" ] && kill "$CP_PID" >/dev/null 2>&1 || true
  [ -n "${UP_PID:-}" ] && kill "$UP_PID" >/dev/null 2>&1 || true
  [ -n "${UP2_PID:-}" ] && kill "$UP2_PID" >/dev/null 2>&1 || true
  [ -n "${AI_PID:-}" ] && kill "$AI_PID" >/dev/null 2>&1 || true
  [ -n "${AI_FALLBACK_PID:-}" ] && kill "$AI_FALLBACK_PID" >/dev/null 2>&1 || true
  [ -n "${AI_STREAM_DIE_PID:-}" ] && kill "$AI_STREAM_DIE_PID" >/dev/null 2>&1 || true
  [ -n "${AI_STREAM_FALLBACK_PID:-}" ] && kill "$AI_STREAM_FALLBACK_PID" >/dev/null 2>&1 || true
  [ -n "${AI_MALFORMED_PID:-}" ] && kill "$AI_MALFORMED_PID" >/dev/null 2>&1 || true
  [ -n "${AI_TRACE_STUB_PID:-}" ] && kill "$AI_TRACE_STUB_PID" >/dev/null 2>&1 || true
  [ -n "${AI_TRACE_ENVOY_PID:-}" ] && kill "$AI_TRACE_ENVOY_PID" >/dev/null 2>&1 || true
  [ -n "${RLS_PID:-}" ] && kill "$RLS_PID" >/dev/null 2>&1 || true
  [ -n "${ENVOY_PID:-}" ] && kill "$ENVOY_PID" >/dev/null 2>&1 || true
}

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

# Boot the full harness: fresh DB, mock upstream + AI providers, control plane, dataplane,
# and a real Envoy joined over ADS. Sets the shared globals (TOKEN, auth, TEAM_ID, ENVOY_MODE,
# *_PID) that every phase consumes. Run once by the runner before any phase.
setup_harness() {
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

# FLOWPLANE_E2E_ENVOY=local forces the local binary (docker/podman `--network host` does not reach
# the host loopback on macOS, so the container path silently can't dial the CP/upstreams there).
if [ "${FLOWPLANE_E2E_ENVOY:-}" != "local" ] && docker run -d --name fp-e2e-envoy --network host \
  -v /tmp/fp-e2e-bootstrap.yaml:/etc/envoy/envoy.yaml:ro \
  envoyproxy/envoy:v${ENVOY_VERSION} -c /etc/envoy/envoy.yaml --log-level info >/dev/null 2>&1; then
  ENVOY_MODE=docker
  echo "envoy started (docker); waiting for traffic to flow"
else
  command -v envoy >/dev/null || { echo "neither docker envoy nor local envoy binary available"; exit 1; }
  ENVOY_MODE=local
  envoy -c /tmp/fp-e2e-bootstrap.yaml --log-level info > /tmp/fp-e2e-envoy.log 2>&1 &
  ENVOY_PID=$!
  echo "envoy started (local binary $(envoy --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)); waiting for traffic to flow"
fi
}
