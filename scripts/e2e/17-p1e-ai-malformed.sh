# E2E phase P1e — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 1e: malformed provider response. A backend that returns a 200 with a non-OpenAI,
# non-JSON body must be passed through to the client without the gateway 500ing, and must not
# settle any usage (missing/unparseable usage is fail-open per D-018) -- proves robustness to
# providers that don't speak clean OpenAI.
cat >/tmp/fp-e2e-ai-malformed.py <<'PY'
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or 0)
        if length:
            self.rfile.read(length)
        body = b"this is definitely not an openai json response"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
python3 /tmp/fp-e2e-ai-malformed.py "$AI_MALFORMED_PORT" >/tmp/fp-e2e-ai-malformed.log 2>&1 &
AI_MALFORMED_PID=$!
AI_MALFORMED_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-malformed").decode())')
AI_MALFORMED_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-malformed-key\",\"description\":\"malformed\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_MALFORMED_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_MALFORMED_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-malformed\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_MALFORMED_PORT\",\"credential_secret_id\":\"$AI_MALFORMED_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_MALFORMED_ROUTE_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-malformed\",\"spec\":{\"listener_port\":$AI_MALFORMED_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_MALFORMED_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_MALFORMED_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$TEAM_ID' AND name = 'ai-ai-e2e-malformed-routes' AND owner_kind = 'ai'")
for i in $(seq 1 60); do
  curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json 2>/dev/null || true
  grep -q "ai-ai-e2e-malformed-listener" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-malformed-listener" /tmp/fp-e2e-ai-dump.json || fail "AI malformed route listener did not converge"
AI_MALFORMED_CODE=""
for i in $(seq 1 30); do
  AI_MALFORMED_CODE=$(curl -sS --max-time 10 -o /tmp/fp-e2e-ai-malformed-body.txt -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_REQUEST" \
    "http://127.0.0.1:$AI_MALFORMED_GATEWAY_PORT/v1/chat/completions" 2>/dev/null || true)
  [ "$AI_MALFORMED_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_MALFORMED_CODE" = "200" ] \
  || fail "AI malformed-provider response was not passed through (gateway returned $AI_MALFORMED_CODE)"
grep -q "not an openai json response" /tmp/fp-e2e-ai-malformed-body.txt \
  || fail "AI malformed-provider body was not delivered to the client"
AI_MALFORMED_USAGE=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM ai_usage_events WHERE team_id = '$TEAM_ID' AND route_config_id = '$AI_MALFORMED_ROUTE_CONFIG_ID'")
[ "$AI_MALFORMED_USAGE" = "0" ] \
  || fail "AI malformed-provider response settled usage ($AI_MALFORMED_USAGE rows); unparseable usage must be fail-open (no settlement)"
AI_MALFORMED_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_MALFORMED_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_MALFORMED_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-malformed >/dev/null
AI_MALFORMED_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_MALFORMED_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_MALFORMED_PROVIDER_REV" \
  http://$API/api/v1/teams/default/ai/providers/ai-e2e-malformed >/dev/null
kill "$AI_MALFORMED_PID" >/dev/null 2>&1 || true
echo "PHASE 1e OK: AI malformed-provider response passed through, no 500, no usage settled (fail-open)"
