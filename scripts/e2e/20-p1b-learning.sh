# E2E phase P1b — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 1b: learning capture through the real injected ALS/ExtProc path. Start a
# route-scoped session on the resources created by expose, then send traffic with stable
# request IDs so ALS metadata and ExtProc body observations merge into raw_observations.
RC_ID=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams/default/route-configs/e2e-routes \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
LISTENER_ID=$(curl -fsS "${auth[@]}" http://$API/api/v1/teams/default/listeners/e2e \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
./target/debug/flowplane api create e2e-api \
  --route-config-id "$RC_ID" --listener-id "$LISTENER_ID" >/tmp/fp-e2e-api-create.txt
API_CREATED=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM api_definitions WHERE name = 'e2e-api'")
[ "$API_CREATED" = "1" ] || fail "api create did not persist API"
./target/debug/flowplane learn start e2e-capture \
  --api e2e-api \
  --target-sample-count 2 --max-bytes 65536 --max-distinct-paths 5 >/tmp/fp-e2e-learn-start.txt
grep -q "e2e-capture" /tmp/fp-e2e-learn-start.txt || fail "learn start did not render session"
for i in $(seq 1 40); do
  curl -fsS -H "x-request-id: fp-e2e-learn-1" -H "x-api-key: secret-one" \
    http://127.0.0.1:$GW_PORT/ >/dev/null 2>&1 || true
  curl -fsS -H "x-request-id: fp-e2e-learn-2" -H "x-api-key: secret-two" \
    http://127.0.0.1:$GW_PORT/ >/dev/null 2>&1 || true
  LEARN_COUNTS=$(psql "$PG_DB_URL" -Atc "SELECT sample_count || ',' || path_count || ',' || drop_count || ',' || status FROM capture_sessions WHERE name = 'e2e-capture'" 2>/dev/null || true)
  RAW_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.name = 'e2e-capture'" 2>/dev/null || echo 0)
  BODY_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.name = 'e2e-capture' AND ro.body_seen" 2>/dev/null || echo 0)
  if [ "$LEARN_COUNTS" = "2,1,0,completed" ] && [ "$RAW_COUNT" = "2" ] && [ "$BODY_COUNT" -ge 1 ]; then
    break
  fi
  sleep 1
done
[ "$LEARN_COUNTS" = "2,1,0,completed" ] || fail "learning counters unexpected: $LEARN_COUNTS"
[ "$RAW_COUNT" = "2" ] || fail "expected two raw observations, got $RAW_COUNT"
[ "$BODY_COUNT" -ge 1 ] || fail "expected ExtProc body capture, got $BODY_COUNT body rows"
REDACTED_KEYS=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.name = 'e2e-capture' AND ro.request_headers->>'x-api-key' = '[REDACTED]'" 2>/dev/null || echo 0)
[ "$REDACTED_KEYS" = "2" ] || fail "expected x-api-key redaction on both raw observations, got $REDACTED_KEYS"
./target/debug/flowplane learn get e2e-capture >/tmp/fp-e2e-learn-get.txt
grep -q "completed" /tmp/fp-e2e-learn-get.txt || fail "learn get did not show completed session"
./target/debug/flowplane learn generate-spec e2e-capture >/tmp/fp-e2e-learn-spec.txt
API_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM api_definitions WHERE name = 'e2e-api'")
SPEC_VERSION=$(psql "$PG_DB_URL" -Atc "SELECT version FROM spec_versions WHERE api_definition_id = '$API_ID' AND source_kind = 'learned' ORDER BY version DESC LIMIT 1")
[ -n "$SPEC_VERSION" ] || fail "learned spec version was not persisted"
./target/debug/flowplane api spec publish e2e-api "$SPEC_VERSION" --reason "e2e approved" >/tmp/fp-e2e-publish.txt
PUBLISHED_ID=$(psql "$PG_DB_URL" -Atc "SELECT published_spec_version_id FROM api_definitions WHERE id = '$API_ID'")
TOOL_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM api_tools WHERE api_definition_id = '$API_ID' AND spec_version_id = '$PUBLISHED_ID'")
[ -n "$PUBLISHED_ID" ] || fail "api did not record published spec pointer"
[ "$TOOL_COUNT" -ge 1 ] || fail "published learned spec did not generate tools"
API_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM api_definitions WHERE id = '$API_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $API_REV" \
  http://$API/api/v1/teams/default/api-definitions/e2e-api >/dev/null
ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM api_route_bindings WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM spec_versions WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM api_tools WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM spec_version_review_events WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM capture_sessions WHERE api_definition_id = '$API_ID') + \
  (SELECT count(*) FROM raw_observations ro JOIN capture_sessions cs ON cs.id = ro.capture_session_id WHERE cs.api_definition_id = '$API_ID')")
[ "$ORPHANS" = "0" ] || fail "S8 API cleanup left $ORPHANS orphan rows"
echo "PHASE 1b OK: live capture -> learned spec -> publish -> generated tools -> API cleanup left zero S8 orphans"
