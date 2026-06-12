#!/usr/bin/env bash
# Live Envoy E2E (S5.6, dev-mode path): boot CP -> configure cluster/route/listener via the
# REST API -> a real Envoy joins over ADS -> traffic flows end to end.
set -euo pipefail
cd "$(dirname "$0")/.."

API=127.0.0.1:8096
XDS_PORT=18000
GW_PORT=10001
UPSTREAM_PORT=3001
DB=flowplane_e2e

cleanup() {
  docker rm -f fp-e2e-envoy >/dev/null 2>&1 || true
  [ -n "${CP_PID:-}" ] && kill "$CP_PID" >/dev/null 2>&1 || true
  [ -n "${UP_PID:-}" ] && kill "$UP_PID" >/dev/null 2>&1 || true
  [ -n "${ENVOY_PID:-}" ] && kill "$ENVOY_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

bash scripts/ensure-postgres.sh >/dev/null
su postgres -s /bin/bash -c "dropdb --if-exists $DB && createdb $DB"

# Distinctive upstream.
mkdir -p /tmp/fp-e2e-www && echo "hello-from-upstream-$(date +%s)" > /tmp/fp-e2e-www/index.html
(cd /tmp/fp-e2e-www && python3 -m http.server $UPSTREAM_PORT >/dev/null 2>&1) &
UP_PID=$!

cargo build --bin flowplane -q
FLOWPLANE_DATABASE_URL=postgres://postgres:postgres@localhost/$DB \
FLOWPLANE_API_INSECURE=true FLOWPLANE_DEV_MODE=true \
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
  address: { socket_address: { address: 127.0.0.1, port_value: 9901 } }
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
  envoyproxy/envoy:v1.31-latest -c /etc/envoy/envoy.yaml --log-level info >/dev/null 2>&1; then
  echo "envoy started (docker); waiting for traffic to flow"
else
  command -v envoy >/dev/null || { echo "neither docker envoy nor local envoy binary available"; exit 1; }
  envoy -c /tmp/fp-e2e-bootstrap.yaml --log-level info > /tmp/fp-e2e-envoy.log 2>&1 &
  ENVOY_PID=$!
  echo "envoy started (local binary $(envoy --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)); waiting for traffic to flow"
fi

fail() {
  echo "E2E FAILED: $1"
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
FLOWPLANE_DATABASE_URL=postgres://postgres:postgres@localhost/$DB \
FLOWPLANE_API_INSECURE=true FLOWPLANE_DEV_MODE=true \
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
DUMP=$(curl -fsS http://127.0.0.1:9901/config_dump)
if echo "$DUMP" | grep -q "blue-upstream\|blue-edge\|blue-routes"; then
  fail "team e2e-blue resources leaked into team default's Envoy"
fi
if curl -fsS --max-time 2 http://127.0.0.1:$((GW_PORT+1))/ >/dev/null 2>&1; then
  fail "Envoy is serving another team's listener port"
fi
echo "PHASE 3 OK: e2e-blue resources never reached team default's dataplane"

echo "E2E PASSED: traffic, restart convergence, cross-team isolation"
exit 0
