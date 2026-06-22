# E2E phase P7 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES="P2"

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
# Poll for a single, consistent config_dump that contains the e2e-advanced listener AND its
# global rate-limit filter. Bounded curl (--max-time) avoids a hung/partial read of the large
# dump being misread as "filter absent", and requiring both tokens in the same snapshot avoids a
# mid-rebuild window where the listener is transiently out of the snapshot (#64 polled the filter
# string only, single-shot curl, and still flaked ~1/3 runs).
ADV_FILTER_READY=0
for i in $(seq 1 60); do
  ADV_DUMP=$(curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump 2>/dev/null || true)
  if grep -q "e2e-advanced" <<<"$ADV_DUMP" \
    && grep -q "envoy.filters.http.ratelimit" <<<"$ADV_DUMP"; then
    ADV_FILTER_READY=1
    break
  fi
  sleep 1
done
[ "$ADV_FILTER_READY" = "1" ] || fail "advanced config dump missing global rate-limit filter"
echo "PHASE 7 OK: advanced route/listener/filter parity ACKed and served traffic"
