# E2E phase P5a — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 5a: MCP api_* descriptor-following through the live gateway. MCP issues only a
# gateway_invocation descriptor; a Flowplane descriptor-aware client follows it by calling Envoy
# directly with normal gateway credentials. RBAC on the listener proves policy parity with a
# normal direct API consumer call and rejects missing credentials at the dataplane layer.
MCP_CREATE_BODY=/tmp/fp-e2e-mcp-create.json
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/route-configs \
  -o "$MCP_CREATE_BODY" -w '%{http_code}' -d '{
  "name":"e2e-mcp-routes",
  "spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{
    "name":"mcp-parity",
    "match":{"prefix":{"prefix":"/mcp-parity"}},
    "action":{"cluster":"e2e-upstream","prefix_rewrite":"/"}
  }]}]}}')
[ "$CODE" = "201" ] || fail "MCP parity route config create failed ($CODE): $(cat "$MCP_CREATE_BODY")"
MCP_ROUTE_CONFIG_ID=$(python3 -c "import sys,json;print(json.load(open(sys.argv[1], encoding='utf-8'))['id'])" "$MCP_CREATE_BODY")
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners \
  -o "$MCP_CREATE_BODY" -w '%{http_code}' -d "{
  \"name\":\"e2e-mcp\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$MCP_PARITY_PORT,
    \"public_base_url\":\"http://127.0.0.1:$MCP_PARITY_PORT\",
    \"route_config\":\"e2e-mcp-routes\",
    \"http_filters\":[{
      \"filter\":{\"type\":\"rbac\",\"action\":\"allow\",
        \"policies\":{\"api-key\":{
          \"permissions\":[{\"kind\":\"any\"}],
          \"principals\":[{\"kind\":\"header\",\"name\":\"x-api-key\",\"exact\":\"fp-mcp-secret\"}]}}}
    }]}}")
[ "$CODE" = "201" ] || fail "MCP parity listener create failed ($CODE): $(cat "$MCP_CREATE_BODY")"
MCP_LISTENER_ID=$(python3 -c "import sys,json;print(json.load(open(sys.argv[1], encoding='utf-8'))['id'])" "$MCP_CREATE_BODY")
cat >/tmp/fp-e2e-mcp-openapi.json <<'JSON'
{
  "openapi": "3.0.3",
  "info": {"title": "MCP parity", "version": "1"},
  "paths": {
    "/mcp-parity": {
      "get": {
        "operationId": "getMcpParity",
        "responses": {"200": {"description": "ok"}}
      }
    }
  }
}
JSON
CODE=$(curl -sS "${auth[@]}" -X POST http://$API/api/v1/teams/default/api-definitions \
  -o "$MCP_CREATE_BODY" -w '%{http_code}' -d "{
  \"name\":\"e2e-mcp-api\",
  \"openapi\":$(cat /tmp/fp-e2e-mcp-openapi.json),
  \"route_binding\":{\"route_config_id\":\"$MCP_ROUTE_CONFIG_ID\",\"listener_id\":\"$MCP_LISTENER_ID\"}
}")
[ "$CODE" = "201" ] || fail "MCP parity API create failed ($CODE): $(cat "$MCP_CREATE_BODY")"
MCP_API_SPEC_ID=$(python3 -c "import sys,json;print(json.load(open(sys.argv[1], encoding='utf-8'))['latest_spec']['id'])" "$MCP_CREATE_BODY")
MCP_API_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM api_definitions WHERE team_id = '$TEAM_ID' AND name = 'e2e-mcp-api'")
# Imported specs generate tools at API-create time, but the review-loop publish command is
# learned-spec-only. Mark this fixture's imported spec as current so MCP lists the generated tool.
psql "$PG_DB_URL" -v ON_ERROR_STOP=1 -c \
  "UPDATE api_definitions SET published_spec_version_id = '$MCP_API_SPEC_ID' WHERE id = '$MCP_API_ID'" >/dev/null
MCP_TOOL_ROW=$(psql "$PG_DB_URL" -AtF $'\t' -c "SELECT name, spec_version_id FROM api_tools WHERE api_definition_id = '$MCP_API_ID' LIMIT 1")
IFS=$'\t' read -r MCP_TOOL_NAME MCP_TOOL_SPEC_ID <<<"$MCP_TOOL_ROW"
[ -n "$MCP_TOOL_NAME" ] && [ -n "$MCP_TOOL_SPEC_ID" ] || fail "MCP parity API tool was not generated"
MCP_TOOL="api_$MCP_TOOL_NAME"

for i in $(seq 1 30); do
  CODE=$(curl -s -o /tmp/fp-e2e-mcp-direct-body -w '%{http_code}' \
    -H 'x-api-key: fp-mcp-secret' "http://127.0.0.1:$MCP_PARITY_PORT/mcp-parity" 2>/dev/null || true)
  [ "$CODE" = "200" ] && break
  sleep 1
done
[ "$CODE" = "200" ] || fail "MCP parity direct credentialed call did not reach Envoy (got $CODE)"
grep -Eq "hello-from-upstream|hello-from-upstream2" /tmp/fp-e2e-mcp-direct-body \
  || fail "MCP parity direct call did not reach expected upstream"
CODE=$(curl -s -o /tmp/fp-e2e-mcp-denied-body -w '%{http_code}' \
  "http://127.0.0.1:$MCP_PARITY_PORT/mcp-parity")
[ "$CODE" = "403" ] || fail "MCP parity gateway policy did not reject missing credentials (got $CODE)"

MCP_AGENT_BODY=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/agents \
  -d "{\"name\":\"e2e-mcp-agent\",\"kind\":\"gateway-tool\",\"grants\":[{\"team_id\":\"$TEAM_ID\",\"resource\":\"mcp-tools\",\"action\":\"execute\"}]}")
MCP_AGENT_TOKEN=$(python3 -c "import sys,json;print(json.load(sys.stdin)['token'])" <<<"$MCP_AGENT_BODY")
MCP_HEADERS=/tmp/fp-e2e-mcp-headers.txt
curl -fsS -D "$MCP_HEADERS" -o /tmp/fp-e2e-mcp-init.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}' \
  http://$API/api/v1/mcp
MCP_SESSION=$(awk 'tolower($1)=="mcp-session-id:" {print $2}' "$MCP_HEADERS" | tr -d '\r')
[ -n "$MCP_SESSION" ] || fail "MCP initialize did not return a session id"
curl -fsS -o /tmp/fp-e2e-mcp-list.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" -H "mcp-session-id: $MCP_SESSION" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{\"team\":\"default\"}}" \
  http://$API/api/v1/mcp
python3 - "$MCP_TOOL" /tmp/fp-e2e-mcp-list.json <<'PY' || fail "MCP tools/list did not include generated api_* tool"
import json, sys
tool = sys.argv[1]
body = json.load(open(sys.argv[2], encoding="utf-8"))
assert any(row["name"] == tool for row in body["result"]["tools"])
PY
curl -fsS -o /tmp/fp-e2e-mcp-descriptor.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" -H "mcp-session-id: $MCP_SESSION" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"$MCP_TOOL\",\"arguments\":{\"team\":\"default\",\"headers\":{\"x-api-key\":\"fp-mcp-secret\"}}}}" \
  http://$API/api/v1/mcp
python3 - "$MCP_TOOL" "$MCP_TOOL_SPEC_ID" /tmp/fp-e2e-mcp-descriptor.json <<'PY' || fail "MCP descriptor shape/auth/source was unexpected"
import json, sys
tool, spec_id, path = sys.argv[1], sys.argv[2], sys.argv[3]
body = json.load(open(path, encoding="utf-8"))
result = body["result"]
assert result["isError"] is False
desc = result["structuredContent"]
assert desc["type"] == "gateway_invocation"
assert desc["tool"] == tool
assert desc["specVersionId"] == spec_id
assert desc["auth"]["mode"] == "caller_gateway_credentials"
assert desc["url"].startswith("http://127.0.0.1:")
assert "/mcp-parity" in desc["url"]
assert desc["headers"]["x-api-key"] == "fp-mcp-secret"
PY
MCP_DESC_URL=$(python3 -c "import json;print(json.load(open('/tmp/fp-e2e-mcp-descriptor.json', encoding='utf-8'))['result']['structuredContent']['url'])")
MCP_DESC_HOST=$(python3 -c "import json;print(json.load(open('/tmp/fp-e2e-mcp-descriptor.json', encoding='utf-8'))['result']['structuredContent']['headers'].get('host',''))")
MCP_DESC_KEY=$(python3 -c "import json;print(json.load(open('/tmp/fp-e2e-mcp-descriptor.json', encoding='utf-8'))['result']['structuredContent']['headers']['x-api-key'])")
DESC_HEADERS=(-H "x-api-key: $MCP_DESC_KEY")
[ -z "$MCP_DESC_HOST" ] || DESC_HEADERS+=(-H "host: $MCP_DESC_HOST")
CODE=$(curl -s -o /tmp/fp-e2e-mcp-desc-body -w '%{http_code}' "${DESC_HEADERS[@]}" "$MCP_DESC_URL")
[ "$CODE" = "200" ] || fail "MCP descriptor-following call did not reach Envoy (got $CODE)"
cmp -s /tmp/fp-e2e-mcp-direct-body /tmp/fp-e2e-mcp-desc-body \
  || fail "MCP descriptor-following response differed from direct gateway call"
CODE=$(curl -s -o /tmp/fp-e2e-mcp-desc-denied-body -w '%{http_code}' "$MCP_DESC_URL")
[ "$CODE" = "403" ] || fail "MCP descriptor URL without gateway credentials was not rejected by Envoy (got $CODE)"
curl -fsS -o /tmp/fp-e2e-mcp-cross-team.json \
  -H "Authorization: Bearer $MCP_AGENT_TOKEN" -H "Content-Type: application/json" -H "mcp-session-id: $MCP_SESSION" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"$MCP_TOOL\",\"arguments\":{\"team\":\"e2e-blue\",\"headers\":{\"x-api-key\":\"fp-mcp-secret\"}}}}" \
  http://$API/api/v1/mcp
python3 - /tmp/fp-e2e-mcp-cross-team.json <<'PY' || fail "MCP cross-team descriptor issuance did not fail closed"
import json, sys
body = json.load(open(sys.argv[1], encoding="utf-8"))
assert body["result"]["isError"] is True
assert body["result"]["error"]["code"] == "forbidden"
PY
echo "PHASE 5a OK: MCP api_* descriptor followed directly through Envoy; RBAC policy parity and cross-team descriptor denial verified"
