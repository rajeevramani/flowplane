#!/usr/bin/env bash
# Live Envoy E2E (S5.6, dev-mode path): boot CP -> configure cluster/route/listener via the
# REST API -> a real Envoy joins over ADS -> traffic flows end to end.
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

curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/clusters \
  -d "{\"name\":\"e2e-upstream\",\"spec\":{\"endpoints\":[{\"host\":\"127.0.0.1\",\"port\":$UPSTREAM_PORT}]}}" >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs \
  -d '{"name":"e2e-routes","spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{"name":"all","match":{"prefix":{"prefix":"/"}},"action":{"cluster":"e2e-upstream"}}]}]}}' >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners \
  -d "{\"name\":\"e2e-edge\",\"spec\":{\"address\":\"0.0.0.0\",\"port\":$GW_PORT,\"route_config\":\"e2e-routes\"}}" >/dev/null
echo "resources created via REST"

cat > /tmp/fp-e2e-bootstrap.yaml <<EOF
node:
  id: "team=$TEAM_ID/dp-e2e"
  cluster: e2e
admin:
  address: { socket_address: { address: 127.0.0.1, port_value: $ADMIN_PORT } }
dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services: [{ envoy_grpc: { cluster_name: xds_cluster } }]
  cds_config: { ads: {}, resource_api_version: V3 }
  lds_config: { ads: {}, resource_api_version: V3 }
static_resources:
  clusters:
    - name: xds_cluster
      connect_timeout: 1s
      type: STRICT_DNS
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config: { http2_protocol_options: {} }
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address: { socket_address: { address: 127.0.0.1, port_value: $XDS_PORT } }
EOF

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
echo "PHASE 1 OK: '$BODY' served through Envoy via ADS-delivered config"

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
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets/edge-sds/rotate \
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

echo "E2E PASSED: traffic, restart convergence, cross-team isolation, http filters, auth filters, SDS rotation, advanced parity"
exit 0
