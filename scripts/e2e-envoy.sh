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
DB=flowplane_e2e
PG_ADMIN_URL=${FLOWPLANE_E2E_PG_ADMIN_URL:-postgres://postgres:postgres@127.0.0.1:5432/postgres}
PG_DB_URL=${FLOWPLANE_E2E_DATABASE_URL:-postgres://postgres:postgres@127.0.0.1:5432/$DB}

cleanup() {
  docker rm -f fp-e2e-envoy >/dev/null 2>&1 || true
  [ -n "${CP_PID:-}" ] && kill "$CP_PID" >/dev/null 2>&1 || true
  [ -n "${UP_PID:-}" ] && kill "$UP_PID" >/dev/null 2>&1 || true
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
(cd /tmp/fp-e2e-www && python3 -m http.server $UPSTREAM_PORT >/dev/null 2>&1) &
UP_PID=$!

cargo build --bin flowplane -q
FLOWPLANE_DATABASE_URL=$PG_DB_URL \
FLOWPLANE_API_INSECURE=true FLOWPLANE_DEV_MODE=true \
FLOWPLANE_SECRET_ENCRYPTION_KEY=12345678901234567890123456789012 \
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
  --name e2e --path / --port "$GW_PORT" >/tmp/fp-e2e-expose.txt
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

# Poll the gateway port until the body matches the given prefix.
wait_body() {
  local prefix=$1 tries=${2:-40}
  for i in $(seq 1 "$tries"); do
    BODY=$(curl -fsS http://127.0.0.1:$GW_PORT/ 2>/dev/null || true)
    [[ "$BODY" == $prefix* ]] && return 0
    sleep 1
  done
  return 1
}

wait_body hello-from-upstream- || fail "initial traffic never flowed"
./target/debug/flowplane stats overview >/tmp/fp-e2e-stats.txt
grep -q "TOTAL DATAPLANES" /tmp/fp-e2e-stats.txt || fail "stats overview did not render dataplane totals"
./target/debug/flowplane ops xds status >/tmp/fp-e2e-xds-status.txt
grep -q "TOTAL DATAPLANES" /tmp/fp-e2e-xds-status.txt || fail "xds status did not render dataplane totals"
./target/debug/flowplane ops xds nacks >/tmp/fp-e2e-xds-nacks.txt
grep -q "no rows" /tmp/fp-e2e-xds-nacks.txt || fail "unexpected xDS NACKs after happy-path expose"
echo "PHASE 1 OK: '$BODY' served through Envoy via ADS-delivered config"

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
curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump \
  | grep -q "envoy.filters.http.ratelimit" \
  || fail "advanced config dump missing global rate-limit filter"
echo "PHASE 7 OK: advanced route/listener/filter parity ACKed and served traffic"

echo "E2E PASSED: traffic, learning capture, restart convergence, cross-team isolation, http filters, auth filters, SDS rotation, advanced parity"
exit 0
