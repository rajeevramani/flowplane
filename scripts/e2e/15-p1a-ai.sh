# E2E phase P1a — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 1a: OpenAI-compatible AI gateway path with usage settlement and enforcing budget trip.
AI_SECRET_VALUE="Bearer fp-e2e-ai-secret"
AI_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-secret").decode())')
AI_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-key\",\"description\":\"AI e2e credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_PROVIDER_BODY")
AI_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e\",\"spec\":{\"listener_port\":$AI_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}")
AI_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_ROUTE_BODY")
AI_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$TEAM_ID' AND name = 'ai-ai-e2e-routes' AND owner_kind = 'ai'")
[ -n "$AI_ROUTE_CONFIG_ID" ] || fail "AI route materialized route config not found"
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "flowplane_ai" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "flowplane_ai" /tmp/fp-e2e-ai-dump.json || fail "AI listener did not receive ExtProc filter"
AI_REQUEST='{"model":"gpt-5","messages":[{"role":"user","content":"hello"}]}'
for i in $(seq 1 50); do
  AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-warm.json -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_CODE" = "200" ] || fail "AI route never served mock provider (last code $AI_CODE)"
grep -q "mock-ai-ok" /tmp/fp-e2e-ai-warm.json || fail "AI mock response did not reach client"
grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-auth.log || fail "AI provider did not receive injected credential"
# fpv2-ti2: the mock must see Host == provider authority (host:port from base_url),
# not the client-sent gateway Host — on the same logged line as the credential.
AI_WARM_LINE="$AI_SECRET_VALUE"$'\t'"127.0.0.1:$AI_PROVIDER_PORT"
grep -qF "$AI_WARM_LINE" /tmp/fp-e2e-ai-auth.log \
  || fail "AI provider did not receive rewritten :authority (fpv2-ti2); last line: $(tail -1 /tmp/fp-e2e-ai-auth.log)"
curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json
if grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-dump.json; then
  fail "AI provider credential leaked into Envoy config dump"
fi
if psql "$PG_DB_URL" -Atc "SELECT spec::text FROM route_configs WHERE id = '$AI_ROUTE_CONFIG_ID'" | grep -q "$AI_SECRET_VALUE"; then
  fail "AI provider credential leaked into materialized route config"
fi
# fpv2-ti2: the weighted route uses TWO DISTINCT providers with distinguishable
# authorities (different mock ports) so a wrong-backend Host cannot pass. The alt
# provider fronts the fallback mock, which expects the fallback credential.
AI_FALLBACK_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-fallback-secret").decode())')
AI_FALLBACK_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-fallback-key\",\"description\":\"AI e2e fallback credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_FALLBACK_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_ALT_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-alt-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_FALLBACK_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_FALLBACK_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_ALT_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_ALT_PROVIDER_BODY")
AI_MULTI_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-multi\",\"spec\":{\"listener_port\":$AI_MULTI_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_PROVIDER_ID\",\"models\":[],\"weight\":1},{\"provider_id\":\"$AI_ALT_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}")
AI_MULTI_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_MULTI_ROUTE_BODY")
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "ai-ai-e2e-multi-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-multi-listener" /tmp/fp-e2e-ai-dump.json || fail "AI multi-backend listener did not converge"
: >/tmp/fp-e2e-ai-auth.log
: >/tmp/fp-e2e-ai-fallback-auth.log
# 20 requests over a 1:1 weighted pair — both providers get traffic (P(one side empty)
# ~= 2e-6) and every logged line must pair the provider's own credential with the
# provider's own authority.
for i in $(seq 1 20); do
  AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-multi.json -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_MULTI_GATEWAY_PORT/v1/chat/completions)
  [ "$AI_CODE" = "200" ] || fail "AI multi-backend request $i failed (code $AI_CODE, body $(cat /tmp/fp-e2e-ai-multi.json))"
done
[ -s /tmp/fp-e2e-ai-auth.log ] || fail "AI weighted route never reached the primary provider in 20 requests"
[ -s /tmp/fp-e2e-ai-fallback-auth.log ] || fail "AI weighted route never reached the alt provider in 20 requests"
awk -F'\t' -v cred="$AI_SECRET_VALUE" -v host="127.0.0.1:$AI_PROVIDER_PORT" \
  '$1 != cred || $2 != host { print "bad line: " $0; exit 1 }' /tmp/fp-e2e-ai-auth.log \
  || fail "primary provider saw a mismatched credential/Host pair (fpv2-ti2)"
awk -F'\t' -v cred="Bearer fp-e2e-ai-fallback-secret" -v host="127.0.0.1:$AI_FALLBACK_PROVIDER_PORT" \
  '$1 != cred || $2 != host { print "bad line: " $0; exit 1 }' /tmp/fp-e2e-ai-fallback-auth.log \
  || fail "alt provider saw a mismatched credential/Host pair (fpv2-ti2)"
AI_MULTI_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_MULTI_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_MULTI_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-multi >/dev/null
AI_ALT_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_ALT_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_ALT_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-alt-provider >/dev/null
AI_MULTI_ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM clusters WHERE owner_kind = 'ai' AND owner_id = '$AI_MULTI_ROUTE_ID') + \
  (SELECT count(*) FROM route_configs WHERE owner_kind = 'ai' AND owner_id = '$AI_MULTI_ROUTE_ID') + \
  (SELECT count(*) FROM listeners WHERE owner_kind = 'ai' AND owner_id = '$AI_MULTI_ROUTE_ID')")
[ "$AI_MULTI_ORPHANS" = "0" ] || fail "AI multi-backend route cleanup left $AI_MULTI_ORPHANS owned gateway rows"
# (fallback secret already created for the weighted-pair section above)
AI_UNAVAILABLE_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-unavailable-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_UNAVAILABLE_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_UNAVAILABLE_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_UNAVAILABLE_PROVIDER_BODY")
AI_FALLBACK_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-fallback-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_FALLBACK_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_FALLBACK_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_FALLBACK_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_FALLBACK_PROVIDER_BODY")
AI_FAILOVER_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-failover\",\"spec\":{\"listener_port\":$AI_FAILOVER_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_UNAVAILABLE_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":0},{\"provider_id\":\"$AI_FALLBACK_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":1}]}}")
AI_FAILOVER_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_FAILOVER_ROUTE_BODY")
AI_FAILOVER_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$TEAM_ID' AND name = 'ai-ai-e2e-failover-routes' AND owner_kind = 'ai'")
[ -n "$AI_FAILOVER_ROUTE_CONFIG_ID" ] || fail "AI failover route materialized route config not found"
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json && grep -q "ai-ai-e2e-failover-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json || fail "AI failover aggregate cluster did not converge"
: >/tmp/fp-e2e-ai-fallback-auth.log
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-failover.json -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_FAILOVER_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "200" ] || fail "AI priority failover did not reach fallback provider (code $AI_CODE, body $(cat /tmp/fp-e2e-ai-failover.json))"
grep -q "mock-ai-ok" /tmp/fp-e2e-ai-failover.json || fail "AI failover mock response did not reach client"
grep -q "Bearer fp-e2e-ai-fallback-secret" /tmp/fp-e2e-ai-fallback-auth.log || fail "AI failover fallback provider did not receive fallback credential"
if grep -q "$AI_SECRET_VALUE" /tmp/fp-e2e-ai-fallback-auth.log; then
  fail "AI failover fallback provider received primary credential"
fi
# fpv2-ti2: the retry attempt's Host must be the FALLBACK provider's authority — proves
# the upstream ExtProc rewrites :authority per attempt, not once per request.
AI_FAILOVER_LINE="Bearer fp-e2e-ai-fallback-secret"$'\t'"127.0.0.1:$AI_FALLBACK_PROVIDER_PORT"
grep -qF "$AI_FAILOVER_LINE" /tmp/fp-e2e-ai-fallback-auth.log \
  || fail "AI failover retry did not rewrite :authority to the fallback provider (fpv2-ti2); last line: $(tail -1 /tmp/fp-e2e-ai-fallback-auth.log)"
for i in $(seq 1 20); do
  AI_FAILOVER_USAGE_ATTR=$(psql "$PG_DB_URL" -Atc "SELECT provider_id || ',' || COALESCE(backend_position::text, '') || ',' || total_tokens FROM ai_usage_events WHERE team_id = '$TEAM_ID' AND route_config_id = '$AI_FAILOVER_ROUTE_CONFIG_ID' ORDER BY created_at DESC LIMIT 1" 2>/dev/null || true)
  [ "$AI_FAILOVER_USAGE_ATTR" = "$AI_FALLBACK_PROVIDER_ID,1,5" ] && break
  sleep 1
done
[ "$AI_FAILOVER_USAGE_ATTR" = "$AI_FALLBACK_PROVIDER_ID,1,5" ] || fail "AI failover usage attribution unexpected: $AI_FAILOVER_USAGE_ATTR"
AI_FAILOVER_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_FAILOVER_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_FAILOVER_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-failover >/dev/null
AI_FALLBACK_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_FALLBACK_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_FALLBACK_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-fallback-provider >/dev/null
AI_UNAVAILABLE_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_UNAVAILABLE_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_UNAVAILABLE_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-unavailable-provider >/dev/null
AI_BUDGET_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/budgets \
  -d "{\"name\":\"ai-e2e-budget\",\"spec\":{\"mode\":\"enforcing\",\"limit_units\":5,\"window_seconds\":3600,\"provider_id\":\"$AI_PROVIDER_ID\",\"route_config_id\":\"$AI_ROUTE_CONFIG_ID\",\"prompt_token_weight\":1,\"completion_token_weight\":1}}")
AI_BUDGET_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_BUDGET_BODY")
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-metered.json -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "200" ] || fail "AI metered request failed before budget trip (code $AI_CODE)"
for i in $(seq 1 20); do
  AI_USED_UNITS=$(psql "$PG_DB_URL" -Atc "SELECT COALESCE(sum(used_units), 0) FROM ai_budget_counters WHERE budget_id = '$AI_BUDGET_ID'" 2>/dev/null || echo 0)
  [ "$AI_USED_UNITS" = "5" ] && break
  sleep 1
done
[ "$AI_USED_UNITS" = "5" ] || fail "AI budget counter did not settle to 5 units, got $AI_USED_UNITS"
AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-blocked.txt -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
  http://127.0.0.1:$AI_GATEWAY_PORT/v1/chat/completions)
[ "$AI_CODE" = "429" ] || fail "AI budget did not trip on second metered request (code $AI_CODE, body $(cat /tmp/fp-e2e-ai-blocked.txt))"
grep -q "AI budget" /tmp/fp-e2e-ai-blocked.txt || fail "AI budget 429 body did not name budget failure"
AI_USAGE_ATTR=$(psql "$PG_DB_URL" -Atc "SELECT provider_id || ',' || COALESCE(backend_position::text, '') || ',' || total_tokens FROM ai_usage_events WHERE team_id = '$TEAM_ID' AND route_config_id = '$AI_ROUTE_CONFIG_ID' ORDER BY created_at DESC LIMIT 1")
[ "$AI_USAGE_ATTR" = "$AI_PROVIDER_ID,0,5" ] || fail "AI usage attribution unexpected: $AI_USAGE_ATTR"
AI_USAGE_ROW=$(psql "$PG_DB_URL" -Atc "SELECT row_to_json(t)::text FROM (SELECT * FROM ai_usage_events WHERE route_config_id = '$AI_ROUTE_CONFIG_ID') t")
if grep -q "$AI_SECRET_VALUE" <<<"$AI_USAGE_ROW"; then
  fail "AI provider credential leaked into usage rows"
fi
./target/debug/flowplane --json ai usage --route-config-id "$AI_ROUTE_CONFIG_ID" >/tmp/fp-e2e-ai-usage.json
python3 - "$AI_PROVIDER_ID" /tmp/fp-e2e-ai-usage.json <<'PY' || fail "flowplane ai usage did not show attributed token usage"
import json, sys
doc = json.load(open(sys.argv[2], encoding="utf-8"))
rows = doc["data"] if isinstance(doc, dict) else doc  # CLI --json envelope {data,kind,schemaVersion}
provider_id = sys.argv[1]
assert any(row["provider_id"] == provider_id and row["total_tokens"] >= 5 for row in rows)
PY
AI_BUDGET_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_budgets WHERE id = '$AI_BUDGET_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_BUDGET_REV" \
  http://$API/api/v1/teams/default/ai/budgets/ai-e2e-budget >/dev/null
AI_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e >/dev/null
AI_ORPHANS=$(psql "$PG_DB_URL" -Atc "SELECT \
  (SELECT count(*) FROM clusters WHERE owner_kind = 'ai' AND owner_id = '$AI_ROUTE_ID') + \
  (SELECT count(*) FROM route_configs WHERE owner_kind = 'ai' AND owner_id = '$AI_ROUTE_ID') + \
  (SELECT count(*) FROM listeners WHERE owner_kind = 'ai' AND owner_id = '$AI_ROUTE_ID')")
[ "$AI_ORPHANS" = "0" ] || fail "AI route cleanup left $AI_ORPHANS owned gateway rows"
AI_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-provider >/dev/null
echo "PHASE 1a OK: AI credential injection (single + multi-backend) -> priority failover credential/usage -> enforcing budget trip -> usage attribution -> cleanup"

# ---- fpv2-7oe smoke: provider update re-materializes the cluster through xDS, no route touch.
AI_REMAT_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-fallback-secret").decode())')
AI_REMAT_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-remat-key\",\"description\":\"remat smoke credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_REMAT_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_REMAT_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-remat-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}")
AI_REMAT_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_REMAT_PROVIDER_BODY")
AI_REMAT_ROUTE_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-remat\",\"spec\":{\"listener_port\":$AI_REMAT_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_REMAT_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}")
AI_REMAT_ROUTE_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_REMAT_ROUTE_BODY")
for i in $(seq 1 50); do
  AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-remat-warm.json -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_REMAT_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_CODE" = "200" ] || fail "remat smoke: route never served the original provider (last code $AI_CODE)"

# Update ONLY the provider (base_url -> fallback mock, credential -> fallback secret).
# The route is never touched again.
AI_REMAT_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_REMAT_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X PATCH -H "If-Match: $AI_REMAT_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-remat-provider \
  -d "{\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_FALLBACK_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_REMAT_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" >/dev/null

# The materialized cluster row must already carry the new endpoint (same request),
# the route must stay active with a bumped version, and Envoy must converge via xDS.
AI_REMAT_CLUSTER_HOSTPORT=$(psql "$PG_DB_URL" -Atc \
  "SELECT spec->'endpoints'->0->>'port' FROM clusters WHERE owner_kind = 'ai' AND name = 'ai-ai-remat-b1'")
[ "$AI_REMAT_CLUSTER_HOSTPORT" = "$AI_FALLBACK_PROVIDER_PORT" ] \
  || fail "remat smoke: cluster row not re-materialized (endpoint port $AI_REMAT_CLUSTER_HOSTPORT, wanted $AI_FALLBACK_PROVIDER_PORT)"
AI_REMAT_ROUTE_STATE=$(psql "$PG_DB_URL" -Atc "SELECT status || ':' || version FROM ai_routes WHERE id = '$AI_REMAT_ROUTE_ID'")
[ "$AI_REMAT_ROUTE_STATE" = "active:2" ] \
  || fail "remat smoke: route state $AI_REMAT_ROUTE_STATE (wanted active:2 — status untouched, version bumped)"
: >/tmp/fp-e2e-ai-fallback-auth.log
for i in $(seq 1 50); do
  AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-remat.json -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_REMAT_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_CODE" = "200" ] && grep -q "Bearer fp-e2e-ai-fallback-secret" /tmp/fp-e2e-ai-fallback-auth.log && break
  sleep 1
done
[ "$AI_CODE" = "200" ] || fail "remat smoke: route did not converge on the updated provider (last code $AI_CODE)"
grep -q "Bearer fp-e2e-ai-fallback-secret" /tmp/fp-e2e-ai-fallback-auth.log \
  || fail "remat smoke: traffic did not reach the NEW provider endpoint after the update"

# Cleanup (route then provider; secret stays, harness DB is disposable).
AI_REMAT_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_REMAT_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_REMAT_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-remat >/dev/null
AI_REMAT_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_REMAT_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_REMAT_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-remat-provider >/dev/null
echo "PHASE 1a remat smoke OK: provider base_url update re-materialized the cluster over xDS without touching the route"

# ---- fpv2-crv: provider auth_scheme credential injection (design AC 1 + AC 3, live).
# HAPPY: a secret storing a BARE key + provider "auth_scheme": "Bearer" (auth_header left to
# its documented default "authorization") must reach the provider as EXACTLY
# "Bearer <bare key>" — scheme + single space + key, no double scheme, no bare key.
# CONFLICT: a secret whose decoded value ALREADY starts with the scheme fails closed at the
# gateway (credential-unavailable 500) BEFORE any upstream contact, and the persisted trace
# row records failure_hop=credential_injection with outcome scheme_conflict, no upstream hop,
# and no credential material.
AI_SCHEME_HEADER="Bearer sk-e2e-scheme-key"
# Dedicated stub (same harness mock script): expects the SCHEMED value, so the request only
# turns 200 if the gateway composed scheme + bare key correctly.
: >/tmp/fp-e2e-ai-scheme-auth.log
python3 /tmp/fp-e2e-ai-provider.py "$AI_SCHEME_PROVIDER_PORT" /tmp/fp-e2e-ai-scheme-auth.log \
  "$AI_SCHEME_HEADER" >/tmp/fp-e2e-ai-scheme-provider.log 2>&1 &
AI_SCHEME_STUB_PID=$!
AI_SCHEME_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"sk-e2e-scheme-key").decode())')
AI_SCHEME_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-scheme-key\",\"description\":\"AI e2e bare-key credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_SCHEME_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_SCHEME_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-scheme-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_SCHEME_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SCHEME_SECRET_ID\",\"auth_scheme\":\"Bearer\",\"models\":[\"gpt-5\"]}}")
AI_SCHEME_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_SCHEME_PROVIDER_BODY")
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-scheme\",\"spec\":{\"listener_port\":$AI_SCHEME_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_SCHEME_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}" \
  >/dev/null
for i in $(seq 1 50); do
  AI_CODE=$(curl -sS -o /tmp/fp-e2e-ai-scheme.json -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_SCHEME_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_CODE" = "200" ] || fail "auth_scheme happy request never served the scheme stub (last code $AI_CODE)"
grep -q "mock-ai-ok" /tmp/fp-e2e-ai-scheme.json || fail "auth_scheme mock response did not reach client"
# The stub logs credential<TAB>Host per request: at least one line must carry EXACTLY the
# composed value, and EVERY line must — no "Bearer Bearer", no bare key, no extra whitespace.
AI_SCHEME_LINE="$AI_SCHEME_HEADER"$'\t'"127.0.0.1:$AI_SCHEME_PROVIDER_PORT"
grep -qF "$AI_SCHEME_LINE" /tmp/fp-e2e-ai-scheme-auth.log \
  || fail "scheme stub did not receive exactly '$AI_SCHEME_HEADER'; last line: $(tail -1 /tmp/fp-e2e-ai-scheme-auth.log)"
awk -F'\t' -v cred="$AI_SCHEME_HEADER" \
  '$1 != cred { print "bad line: " $0; exit 1 }' /tmp/fp-e2e-ai-scheme-auth.log \
  || fail "scheme stub saw an auth header other than exactly '$AI_SCHEME_HEADER'"
curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json
if grep -q "sk-e2e-scheme-key" /tmp/fp-e2e-ai-dump.json; then
  fail "auth_scheme bare key leaked into Envoy config dump"
fi

# Baseline BEFORE any conflict resources exist: the conflict request must add ZERO lines
# to the stub log (fails closed before upstream contact).
AI_SCHEME_STUB_LINES_BEFORE=$(wc -l < /tmp/fp-e2e-ai-scheme-auth.log | tr -d ' ')
AI_SCHEME_CONFLICT_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer sk-e2e-conflict-key").decode())')
AI_SCHEME_CONFLICT_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-scheme-conflict-key\",\"description\":\"AI e2e already-schemed credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_SCHEME_CONFLICT_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_SCHEME_CONFLICT_PROVIDER_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-scheme-conflict-provider\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_SCHEME_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_SCHEME_CONFLICT_SECRET_ID\",\"auth_scheme\":\"Bearer\",\"models\":[\"gpt-5\"]}}")
AI_SCHEME_CONFLICT_PROVIDER_ID=$(python3 -c "import sys,json;print(json.load(sys.stdin)['id'])" <<<"$AI_SCHEME_CONFLICT_PROVIDER_BODY")
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-scheme-conflict\",\"spec\":{\"listener_port\":$AI_SCHEME_CONFLICT_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_SCHEME_CONFLICT_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}" \
  >/dev/null
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json || true
  grep -q "ai-ai-e2e-scheme-conflict-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-scheme-conflict-listener" /tmp/fp-e2e-ai-dump.json \
  || fail "auth_scheme conflict listener did not converge"
AI_SCHEME_CONFLICT_CODE=""
for i in $(seq 1 50); do
  AI_SCHEME_CONFLICT_CODE=$(curl -sS -o /tmp/fp-e2e-ai-scheme-conflict.body -D /tmp/fp-e2e-ai-scheme-conflict.hdrs -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    http://127.0.0.1:$AI_SCHEME_CONFLICT_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_SCHEME_CONFLICT_CODE" = "500" ] && grep -qi "credential unavailable" /tmp/fp-e2e-ai-scheme-conflict.body && break
  sleep 1
done
[ "$AI_SCHEME_CONFLICT_CODE" = "500" ] \
  || fail "scheme-conflict request expected 500, got $AI_SCHEME_CONFLICT_CODE (body $(cat /tmp/fp-e2e-ai-scheme-conflict.body))"
grep -qi "AI provider credential unavailable" /tmp/fp-e2e-ai-scheme-conflict.body \
  || fail "scheme-conflict 500 body did not carry the credential-unavailable envelope: $(cat /tmp/fp-e2e-ai-scheme-conflict.body)"
AI_SCHEME_CONFLICT_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-ai-scheme-conflict.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
[ -n "$AI_SCHEME_CONFLICT_REQ" ] || fail "scheme-conflict response carried no x-request-id"
# Trace row via the trace REST API (CLI --json wrapper), keyed by the server request id.
AI_SCHEME_TRACE_OK=0
for i in $(seq 1 30); do
  ./target/debug/flowplane --json ai trace --request-id "$AI_SCHEME_CONFLICT_REQ" \
    >/tmp/fp-e2e-ai-scheme-trace.json 2>/dev/null || true
  grep -q '"scheme_conflict"' /tmp/fp-e2e-ai-scheme-trace.json && { AI_SCHEME_TRACE_OK=1; break; }
  sleep 1
done
[ "$AI_SCHEME_TRACE_OK" = "1" ] \
  || fail "no trace row with outcome scheme_conflict for request $AI_SCHEME_CONFLICT_REQ (last: $(cat /tmp/fp-e2e-ai-scheme-trace.json 2>/dev/null))"
python3 - /tmp/fp-e2e-ai-scheme-trace.json <<'PY' || fail "scheme-conflict trace row failed assertions"
import json, sys
doc = json.load(open(sys.argv[1], encoding="utf-8"))
traces = doc["data"]["traces"]
assert len(traces) == 1, f"expected exactly one trace row, got {len(traces)}"
row = traces[0]
assert row["failure_hop"] == "credential_injection", row["failure_hop"]
assert row["status_code"] == 500, row["status_code"]
by_name = {h["hop"]: h for h in row["hops"]}
cred = by_name["credential_injection"]
assert cred["outcome"] == "scheme_conflict", cred
assert "upstream" not in by_name, sorted(by_name)
row_text = json.dumps(row)
assert "sk-e2e-conflict-key" not in row_text, "credential material must never appear in the trace row"
PY
# Fail-closed means fail BEFORE upstream: the stub must not have been contacted at all.
AI_SCHEME_STUB_LINES_AFTER=$(wc -l < /tmp/fp-e2e-ai-scheme-auth.log | tr -d ' ')
[ "$AI_SCHEME_STUB_LINES_AFTER" = "$AI_SCHEME_STUB_LINES_BEFORE" ] \
  || fail "scheme-conflict request reached the upstream stub ($AI_SCHEME_STUB_LINES_BEFORE -> $AI_SCHEME_STUB_LINES_AFTER log lines)"
if grep -q "sk-e2e-conflict-key" /tmp/fp-e2e-ai-scheme-auth.log; then
  fail "conflict credential material reached the upstream stub"
fi

# Cleanup (routes then providers; secrets stay, harness DB is disposable).
kill "$AI_SCHEME_STUB_PID" >/dev/null 2>&1 || true
for route in "ai-e2e-scheme" "ai-e2e-scheme-conflict"; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE team_id = '$TEAM_ID' AND name = '$route'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/default/ai/routes/$route" >/dev/null
done
for prov in "ai-e2e-scheme-provider" "ai-e2e-scheme-conflict-provider"; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE team_id = '$TEAM_ID' AND name = '$prov'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/default/ai/providers/$prov" >/dev/null
done
echo "PHASE 1a auth_scheme OK: bare key + auth_scheme Bearer injected as exactly scheme+SP+key -> already-schemed secret failed closed (500 credential-unavailable, scheme_conflict trace row, zero upstream contact)"
