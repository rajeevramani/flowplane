# E2E phase P4 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

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
