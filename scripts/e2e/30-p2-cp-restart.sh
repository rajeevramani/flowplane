# E2E phase P2 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 2: restart convergence. Kill the CP while Envoy keeps running; the restarted
# CP must prime its snapshot cache from the DB (not wipe the dataplane) and a post-restart
# mutation must reach the already-connected Envoy.
echo "hello-from-upstream2-$(date +%s)" > /tmp/fp-e2e-www2.html
mkdir -p /tmp/fp-e2e-www2 && cp /tmp/fp-e2e-www2.html /tmp/fp-e2e-www2/index.html
(cd /tmp/fp-e2e-www2 && python3 -m http.server $((UPSTREAM_PORT+1)) >/dev/null 2>&1) &
UP2_PID=$!
# UP2_PID is reaped by the central cleanup() in lib.sh; no per-phase trap re-install needed.

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
