# E2E phase P8 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 8 (fpv2-4ht / S8): first-party global rate-limit enforcement, end to end.
# Spins the real flowplane-rls (separate process), restarts the CP wired to it
# (FLOWPLANE_RLS_GRPC_URL injects the built-in rate_limit_cluster into CDS; FLOWPLANE_RLS_ADMIN_URL
# starts the 60s reconcile push), creates a team policy + a built-in-path global_rate_limit
# listener, and proves against a real Envoy:
#   #1 enforcement   — the (N+1)-th request in the window is 429; requests under the limit pass.
#   #1/#3 binding    — the emitted Envoy filter `domain` is the CP-composed {orgUUID}|{teamUUID}|<domain>
#                      namespace (NOT the raw user domain), so the 429 only fires because the S7
#                      composition and the S5 push agree on the namespace.
#   #4 fail mode     — with the RLS unreachable, failure_mode_deny=false fails OPEN (200) and a
#                      sibling failure_mode_deny=true listener fails CLOSED (5xx).
#   #5 lifecycle     — a deleted policy stops being enforced within the reconcile window (this run
#                      shortens it via FLOWPLANE_RLS_RECONCILE_SECS; force-repush is the
#                      platform-admin fast path, unavailable to the dev-mode token).
#   #6 no infra      — only PostgreSQL + the flowplane/flowplane-rls/Envoy processes; no Redis.

RLS_GRPC_PORT=$((GW_PORT+21))
RLS_ADMIN_PORT=$((GW_PORT+22))
S8_OPEN_PORT=$((GW_PORT+23))   # listener with failure_mode_deny=false (fails open)
S8_CLOSED_PORT=$((GW_PORT+24)) # listener with failure_mode_deny=true  (fails closed)

# 1. Start the real RLS (plaintext h2c gRPC + HTTP admin), in-memory counters (no Redis).
cargo build --bin flowplane-rls -q
FLOWPLANE_RLS_GRPC_LISTEN=127.0.0.1:$RLS_GRPC_PORT \
FLOWPLANE_RLS_ADMIN_LISTEN=127.0.0.1:$RLS_ADMIN_PORT \
./target/debug/flowplane-rls > /tmp/fp-e2e-rls.log 2>&1 &
RLS_PID=$!  # reaped by the central cleanup() in lib.sh
for i in $(seq 1 40); do
  curl -fsS "http://127.0.0.1:$RLS_ADMIN_PORT/healthz" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$RLS_ADMIN_PORT/healthz" >/dev/null 2>&1 || fail "flowplane-rls admin never became healthy"

# 2. Restart the CP wired to the RLS (built-in cluster injection + reconcile push).
kill "$CP_PID"; wait "$CP_PID" 2>/dev/null || true
CP_LOG=/tmp/fp-e2e-cp8.log
FLOWPLANE_DATABASE_URL=$PG_DB_URL \
FLOWPLANE_API_INSECURE=true FLOWPLANE_DEV_MODE=true \
FLOWPLANE_SECRET_ENCRYPTION_KEY=12345678901234567890123456789012 \
FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS=127.0.0.1:$UPSTREAM_PORT \
FLOWPLANE_API_ADDR=$API FLOWPLANE_XDS_ADDR=0.0.0.0:$XDS_PORT \
FLOWPLANE_RLS_GRPC_URL=127.0.0.1:$RLS_GRPC_PORT \
FLOWPLANE_RLS_ADMIN_URL=http://127.0.0.1:$RLS_ADMIN_PORT \
FLOWPLANE_RLS_RECONCILE_SECS=2 \
./target/debug/flowplane serve > "$CP_LOG" 2>&1 &
CP_PID=$!
for i in $(seq 1 40); do curl -fsS http://$API/healthz >/dev/null 2>&1 && break; sleep 0.5; done
TOKEN=$(grep -o '"dev_token":"[^"]*"' "$CP_LOG" | cut -d'"' -f4)
[ -n "$TOKEN" ] || fail "no dev token after RLS-wired CP restart"
export FLOWPLANE_TOKEN="$TOKEN"
auth=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")
grep -q "rls_sync worker started" "$CP_LOG" || fail "RLS-wired CP did not start the rls_sync worker"

# 3. Team rate-limit policy: domain "s8", descriptor api_key=smoke, 2 requests/minute.
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/rate-limit-domains \
  -o /tmp/fp-e2e-s8.json -w '%{http_code}' -d '{"name":"s8"}')
[ "$CODE" = "201" ] || fail "rate-limit domain create failed ($CODE): $(cat /tmp/fp-e2e-s8.json)"
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/rate-limit-domains/s8/policies \
  -o /tmp/fp-e2e-s8.json -w '%{http_code}' \
  -d '{"name":"s8-policy","spec":{"descriptors":{"api_key":"smoke"},"requests_per_unit":2,"unit":"minute"}}')
[ "$CODE" = "201" ] || fail "rate-limit policy create failed ($CODE): $(cat /tmp/fp-e2e-s8.json)"

# 4. Route config emitting the api_key descriptor from the x-api-key header, to the real upstream.
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs \
  -o /tmp/fp-e2e-s8.json -w '%{http_code}' -d '{
  "name":"e2e-s8-routes",
  "spec":{"virtual_hosts":[{
    "name":"default","domains":["*"],
    "routes":[{
      "name":"s8","match":{"prefix":{"prefix":"/"}},
      "action":{
        "cluster":"e2e-upstream","timeout_secs":10,"prefix_rewrite":"/",
        "rate_limits":[{"actions":[{"type":"request_headers","header_name":"x-api-key","descriptor_key":"api_key"}]}]
      }
    }]
  }]}}')
[ "$CODE" = "201" ] || fail "s8 route config create failed ($CODE): $(cat /tmp/fp-e2e-s8.json)"

# 5. Two listeners on the BUILT-IN path (service_cluster=rate_limit_cluster) — one fail-open, one
#    fail-closed. The CP injects rate_limit_cluster and composes each filter domain to the team
#    namespace; the user only ever names the policy domain "s8".
for spec in "open:$S8_OPEN_PORT:false" "closed:$S8_CLOSED_PORT:true"; do
  kind=${spec%%:*}; rest=${spec#*:}; port=${rest%%:*}; deny=${rest#*:}
  CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners \
    -o /tmp/fp-e2e-s8.json -w '%{http_code}' -d "{
    \"name\":\"e2e-s8-$kind\",
    \"spec\":{\"address\":\"0.0.0.0\",\"port\":$port,\"protocol\":\"http2\",
      \"route_config\":\"e2e-s8-routes\",
      \"http_filters\":[{
        \"filter\":{\"type\":\"global_rate_limit\",\"domain\":\"s8\",\"service_cluster\":\"rate_limit_cluster\",
          \"timeout_ms\":200,\"failure_mode_deny\":$deny,\"request_type\":\"external\"}
      }]}}")
  [ "$CODE" = "201" ] || fail "s8 $kind listener create failed ($CODE): $(cat /tmp/fp-e2e-s8.json)"
done

# 6. Wait for one consistent config_dump carrying BOTH listeners, the ratelimit filter, and the
#    injected built-in cluster. Then assert the emitted filter `domain` is the CP-composed
#    namespace {uuid}|{uuid}|s8 — NOT the bare user domain "s8" (proves S7 composition end to end).
S8_READY=0
for i in $(seq 1 60); do
  DUMP=$(curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump 2>/dev/null || true)
  if grep -q "e2e-s8-open" <<<"$DUMP" && grep -q "e2e-s8-closed" <<<"$DUMP" \
    && grep -q "envoy.filters.http.ratelimit" <<<"$DUMP" \
    && grep -q "rate_limit_cluster" <<<"$DUMP"; then
    S8_READY=1
    break
  fi
  sleep 1
done
[ "$S8_READY" = "1" ] || fail "s8 config dump missing listeners / ratelimit filter / built-in cluster"
COMPOSED=$(grep -oE '[0-9a-fA-F-]{36}\|[0-9a-fA-F-]{36}\|s8' <<<"$DUMP" | head -1)
[ -n "$COMPOSED" ] || fail "s8 filter domain was not CP-composed to {org}|{team}|s8 in config_dump"
grep -q "\"domain\":\"s8\"" <<<"$DUMP" && fail "s8 filter leaked the raw user domain (composition did not run)"
echo "s8 composed Envoy domain: $COMPOSED"

# 7. The CP pushes the policy on its reconcile loop (set to 2s for this run via
#    FLOWPLANE_RLS_RECONCILE_SECS; the production default stays 60s). force-repush is the
#    platform-admin fast path, which the dev-mode token (org admin, no admin:all) cannot call —
#    the short reconcile is the no-platform-admin way to make the test converge.
# Helper: send a request through a listener port with a chosen descriptor value, echo status.
s8_hit() { curl --http2-prior-knowledge -s -o /dev/null -w '%{http_code}' -H "x-api-key: $2" \
  "http://127.0.0.1:$1/" 2>/dev/null || true; }

# Wait for the fail-open listener to actually serve, using a descriptor value with NO policy
# ("warmup") so these readiness probes never consume the enforced "smoke" budget.
for i in $(seq 1 30); do [ "$(s8_hit $S8_OPEN_PORT warmup)" = "200" ] && break; sleep 1; done
[ "$(s8_hit $S8_OPEN_PORT warmup)" = "200" ] || fail "s8 listener never served a request"
# Give the reconcile loop a window to push the policy to the RLS before measuring enforcement.
sleep 5

# 8. Enforcement (#1): 2 req/min on descriptor api_key=smoke — first two pass, the third is 429.
declare -a CODES=()
for n in 1 2 3 4; do CODES+=("$(s8_hit $S8_OPEN_PORT smoke)"); done
echo "s8 enforcement codes: ${CODES[*]}"
[ "${CODES[0]}" = "200" ] && [ "${CODES[1]}" = "200" ] || fail "first two requests under the limit must pass (got ${CODES[*]})"
[ "${CODES[2]}" = "429" ] || fail "the 3rd request in the window must be 429 (got ${CODES[*]})"
# A descriptor with no policy still passes (proves isolation: enforcement is per-namespace+descriptor).
[ "$(s8_hit $S8_OPEN_PORT warmup)" = "200" ] || fail "an unmatched descriptor must not be rate-limited"

# 9. Lifecycle (#5): delete the policy -> enforcement stops within the reconcile window.
REV=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams/default/rate-limit-domains/s8/policies/s8-policy \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['revision'])")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
  http://$API/api/v1/teams/default/rate-limit-domains/s8/policies/s8-policy >/dev/null
DELETED_OK=0
for i in $(seq 1 20); do [ "$(s8_hit $S8_OPEN_PORT smoke)" = "200" ] && { DELETED_OK=1; break; }; sleep 1; done
[ "$DELETED_OK" = "1" ] || fail "deleted policy still enforced after the reconcile window (#5)"
echo "s8 deleted policy un-enforced after reconcile (<= window)"

# 10. Fail mode (#4): kill the RLS. The filter calls the RLS for every descriptor-bearing request,
#     so with it unreachable failure_mode_deny=false must fail OPEN (200) and failure_mode_deny=true
#     must fail CLOSED (5xx) — the two listeners diverge purely on the configured failure mode.
kill "$RLS_PID"; wait "$RLS_PID" 2>/dev/null || true; RLS_PID=""
OPEN_OK=0; CLOSED_OK=0
for i in $(seq 1 30); do
  oc=$(s8_hit $S8_OPEN_PORT smoke); cc=$(s8_hit $S8_CLOSED_PORT smoke)
  [ "$oc" = "200" ] && OPEN_OK=1
  case "$cc" in 5*) CLOSED_OK=1 ;; esac
  [ "$OPEN_OK" = "1" ] && [ "$CLOSED_OK" = "1" ] && break
  sleep 1
done
[ "$OPEN_OK" = "1" ] || fail "RLS down: failure_mode_deny=false must fail open (200, got $oc)"
[ "$CLOSED_OK" = "1" ] || fail "RLS down: failure_mode_deny=true must fail closed (5xx, got $cc)"
echo "s8 fail-open=200 / fail-closed=5xx with RLS down"

# 11. No mandatory infra (#6): every assertion above passed with only PostgreSQL + the
#     flowplane/flowplane-rls/Envoy processes. Prove the path is Redis-free by construction — the
#     CP and the RLS were launched with no Redis configuration of any kind (the RLS counter store is
#     the in-memory fixed-window impl, S4). A global `pgrep redis-server` would false-fail on a host
#     that merely happens to run an unrelated Redis, so assert on our own launched env instead.
env | grep -iq 'redis' && fail "a REDIS-* variable is set — the MVP path must not require Redis"
grep -iq 'redis' /tmp/fp-e2e-cp8.log /tmp/fp-e2e-rls.log 2>/dev/null \
  && fail "the CP or RLS logged a Redis reference — the MVP path must not require Redis"

echo "PHASE 8 OK: global rate-limit enforced (429), tenant-namespaced domain, fail open/closed, lifecycle, no Redis"
