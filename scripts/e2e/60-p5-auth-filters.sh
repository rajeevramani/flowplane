# E2E phase P5 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

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
