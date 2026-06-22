# E2E phase P3 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

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
