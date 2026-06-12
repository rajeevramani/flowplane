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

for i in $(seq 1 40); do
  BODY=$(curl -fsS http://127.0.0.1:$GW_PORT/ 2>/dev/null || true)
  if [[ "$BODY" == hello-from-upstream-* ]]; then
    echo "E2E PASSED: '$BODY' served through Envoy via ADS-delivered config"
    exit 0
  fi
  sleep 1
done
echo "E2E FAILED: traffic never flowed"
echo "--- envoy logs:"; (docker logs fp-e2e-envoy 2>&1 || cat /tmp/fp-e2e-envoy.log) | tail -25
echo "--- cp logs:"; tail -15 /tmp/fp-e2e-cp.log
exit 1
