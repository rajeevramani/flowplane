# E2E phase P1c — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 1c: traffic-first discovery loop. A Flowplane-owned discovery listener captures
# unmatched traffic, learned specs are reviewed/published, and route generation apply replays
# the dry-run plan into durable gateway resources.
./target/debug/flowplane learn discover start e2e-discover \
  --upstream "127.0.0.1:$UPSTREAM_PORT" \
  --listener-port "$DISCOVERY_PORT" \
  --target-sample-count 2 --max-bytes 65536 --max-distinct-paths 5 >/tmp/fp-e2e-discover-start.txt
grep -q "e2e-discover" /tmp/fp-e2e-discover-start.txt || fail "learn discover start did not render session"
for i in $(seq 1 50); do
  curl -fsS -H "host: s9.example.test" -H "x-request-id: fp-e2e-discover-1" \
    "http://127.0.0.1:$DISCOVERY_PORT/v1/discovered/1" >/dev/null 2>&1 || true
  curl -fsS -H "host: s9.example.test" -H "x-request-id: fp-e2e-discover-2" \
    "http://127.0.0.1:$DISCOVERY_PORT/v1/discovered/2" >/dev/null 2>&1 || true
  DISC_RAW_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM discovery_raw_observations dro JOIN discovery_sessions ds ON ds.id = dro.discovery_session_id WHERE ds.name = 'e2e-discover'" 2>/dev/null || echo 0)
  [ "$DISC_RAW_COUNT" = "2" ] && break
  sleep 1
done
[ "$DISC_RAW_COUNT" = "2" ] || fail "expected two discovery raw observations, got $DISC_RAW_COUNT"
./target/debug/flowplane learn discover stop e2e-discover >/tmp/fp-e2e-discover-stop.txt
DISCOVERY_STATUS=$(psql "$PG_DB_URL" -Atc "SELECT status FROM discovery_sessions WHERE name = 'e2e-discover'")
[ "$DISCOVERY_STATUS" = "completed" ] || fail "learn discover stop did not complete session: $DISCOVERY_STATUS"
DISCOVERY_ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM clusters c JOIN discovery_sessions ds ON ds.id = c.owner_id WHERE ds.name = 'e2e-discover' AND c.owner_kind = 'discovery') + \
  (SELECT count(*) FROM route_configs rc JOIN discovery_sessions ds ON ds.id = rc.owner_id WHERE ds.name = 'e2e-discover' AND rc.owner_kind = 'discovery') + \
  (SELECT count(*) FROM listeners l JOIN discovery_sessions ds ON ds.id = l.owner_id WHERE ds.name = 'e2e-discover' AND l.owner_kind = 'discovery')")
[ "$DISCOVERY_ORPHANS" = "0" ] || fail "discovery stop left $DISCOVERY_ORPHANS owned gateway rows"
./target/debug/flowplane learn discover generate-spec e2e-discover >/tmp/fp-e2e-discover-specs.txt
DISC_SPEC_ROW=$(psql "$PG_DB_URL" -AtF $'\t' -c "SELECT sv.id, sv.api_definition_id, sv.version, ad.name \
  FROM spec_versions sv JOIN api_definitions ad ON ad.id = sv.api_definition_id \
  WHERE sv.source_kind = 'learned' \
    AND sv.spec->'x-flowplane-learning-source'->>'discovery_session_name' = 'e2e-discover' \
  ORDER BY sv.created_at DESC LIMIT 1")
IFS=$'\t' read -r DISC_SPEC_ID DISC_API_ID DISC_SPEC_VERSION DISC_API_NAME <<<"$DISC_SPEC_ROW"
[ -n "$DISC_SPEC_ID" ] && [ -n "$DISC_API_NAME" ] || fail "discovery learned spec was not persisted"
DISC_OBSERVED_HOST=$(psql "$PG_DB_URL" -Atc "SELECT spec->'x-flowplane-learning-source'->>'observed_host' FROM spec_versions WHERE id = '$DISC_SPEC_ID'")
./target/debug/flowplane api spec publish "$DISC_API_NAME" "$DISC_SPEC_VERSION" --reason "s9 e2e approved" >/tmp/fp-e2e-discover-publish.txt
DISC_PUBLISHED_ID=$(psql "$PG_DB_URL" -Atc "SELECT published_spec_version_id FROM api_definitions WHERE id = '$DISC_API_ID'")
DISC_TOOL_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM api_tools WHERE api_definition_id = '$DISC_API_ID' AND spec_version_id = '$DISC_PUBLISHED_ID'")
[ "$DISC_PUBLISHED_ID" = "$DISC_SPEC_ID" ] || fail "discovery API did not publish learned spec"
[ "$DISC_TOOL_COUNT" -ge 1 ] || fail "published discovery spec did not generate tools"
./target/debug/flowplane route generate --from-spec "$DISC_SPEC_ID" --listener-port "$GENERATED_ROUTE_PORT" >/tmp/fp-e2e-route-plan.txt
DISC_PLAN_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_generation_plans WHERE spec_version_id = '$DISC_SPEC_ID' ORDER BY created_at DESC LIMIT 1")
[ -n "$DISC_PLAN_ID" ] || fail "route generation plan was not persisted"
./target/debug/flowplane route apply "$DISC_PLAN_ID" >/tmp/fp-e2e-route-apply.txt
PLAN_MATCH=$(psql "$PG_DB_URL" -Atc "SELECT ((rgp.plan->'cluster_spec') = c.spec AND (rgp.plan->'route_config_spec') = rc.spec AND (rgp.plan->'listener_spec') = l.spec) \
  FROM route_generation_plans rgp \
  JOIN clusters c ON c.team_id = rgp.team_id AND c.name = rgp.plan->>'cluster_name' \
  JOIN route_configs rc ON rc.team_id = rgp.team_id AND rc.name = rgp.plan->>'route_config_name' \
  JOIN listeners l ON l.team_id = rgp.team_id AND l.name = rgp.plan->>'listener_name' \
  WHERE rgp.id = '$DISC_PLAN_ID' AND rgp.status = 'applied'")
[ "$PLAN_MATCH" = "t" ] || fail "applied route resources did not match dry-run plan"
for i in $(seq 1 40); do
  BODY=$(curl -fsS -H "host: $DISC_OBSERVED_HOST" "http://127.0.0.1:$GENERATED_ROUTE_PORT/v1/discovered/1" 2>/dev/null || true)
  [[ "$BODY" == hello-from-upstream-* ]] && break
  sleep 1
done
[[ "$BODY" == hello-from-upstream-* ]] || fail "generated traffic-first route never flowed"
echo "PHASE 1c OK: traffic-first discovery -> learned spec -> publish/tools -> dry-run/apply -> generated route served '$BODY'"
