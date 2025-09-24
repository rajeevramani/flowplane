#!/usr/bin/env bash
set -euo pipefail

API_BASE=${API_BASE:-http://127.0.0.1:8080}
LISTENER_HOST=${LISTENER_HOST:-http://127.0.0.1:10000}

post_or_skip() {
  local endpoint="$1"
  local payload="$2"
  local code
  code=$(curl -sS -o /dev/null -w '%{http_code}\n' -X POST "${API_BASE}${endpoint}" -H 'Content-Type: application/json' -d "${payload}")
  if [[ "${code}" != "200" && "${code}" != "201" ]]; then
    echo "[smoke] ${endpoint} returned ${code} (continuing)"
  fi
}

echo "[smoke] Creating smoke test cluster..."
post_or_skip "/api/v1/clusters" '{
  "name": "smoke-cluster",
  "serviceName": "smoke-service",
  "endpoints": [{"host": "httpbin.org", "port": 443}],
  "connectTimeoutSeconds": 5,
  "useTls": true
}'

echo "[smoke] Publishing route configuration..."
post_or_skip "/api/v1/routes" '{
  "name": "smoke-routes",
  "virtualHosts": [
    {
      "name": "default",
      "domains": ["*"],
      "routes": [
        {
          "name": "status",
          "match": {"type": "prefix", "value": "/"},
          "action": {"type": "forward", "cluster": "smoke-cluster"}
        }
      ]
    }
  ]
}'

echo "[smoke] Registering listener bound to smoke-routes..."
post_or_skip "/api/v1/listeners" '{
  "name": "smoke-listener",
  "address": "0.0.0.0",
  "port": 10000,
  "protocol": "HTTP",
  "filterChains": [
    {
      "name": "default",
      "filters": [
        {
          "name": "envoy.filters.network.http_connection_manager",
          "type": "httpConnectionManager",
          "routeConfigName": "smoke-routes"
        }
      ]
    }
  ]
}'

echo "[smoke] Waiting for LDS refresh..."
sleep 1

echo "[smoke] Hitting listener via Envoy..."
response=$(curl -sS -w '\n' "${LISTENER_HOST}/status/200")
echo "${response}" | head -n 5

echo "[smoke] Completed"
