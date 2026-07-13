# E2E phase P1d — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 1d: AI streaming-failover boundary. Once the higher-priority backend has started
# streaming a response (200 + first chunk sent downstream), a mid-stream backend failure must be
# terminal: Envoy cannot fail over to the lower-priority backend after the response is committed,
# and the client keeps the partial stream. We prove this with a primary mock that emits one SSE
# chunk then RSTs the connection, and assert the lower-priority fallback is never contacted.
cat >/tmp/fp-e2e-ai-stream-die.py <<'PY'
import socket
import struct
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
auth_log = sys.argv[2]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or 0)
        if length:
            self.rfile.read(length)
        with open(auth_log, "a", encoding="utf-8") as f:
            f.write(self.headers.get("authorization", "") + "\n")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(b'data: {"choices":[{"delta":{"content":"partial-stream"}}]}\n\n')
        self.wfile.flush()
        # Let Envoy drain the flushed chunk before the forced reset. Without this small delay,
        # the RST can race ahead of the chunk and make the harness flaky even though the product
        # behavior being tested is unchanged: no failover after a response has started.
        time.sleep(0.25)
        # Abruptly reset mid-stream (SO_LINGER 0 -> RST) after the first chunk is on the wire.
        self.close_connection = True
        try:
            self.connection.setsockopt(
                socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0)
            )
            self.connection.close()
        except OSError:
            pass


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
: >/tmp/fp-e2e-ai-stream-die-auth.log
: >/tmp/fp-e2e-ai-stream-fallback-auth.log
python3 /tmp/fp-e2e-ai-stream-die.py "$AI_STREAM_DIE_PORT" /tmp/fp-e2e-ai-stream-die-auth.log \
  >/tmp/fp-e2e-ai-stream-die.log 2>&1 &
AI_STREAM_DIE_PID=$!
python3 /tmp/fp-e2e-ai-provider.py "$AI_STREAM_FALLBACK_PORT" /tmp/fp-e2e-ai-stream-fallback-auth.log \
  "Bearer fp-e2e-ai-stream-fallback" >/tmp/fp-e2e-ai-stream-fallback.log 2>&1 &
AI_STREAM_FALLBACK_PID=$!

AI_STREAM_PRIMARY_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-stream-primary").decode())')
AI_STREAM_PRIMARY_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-stream-primary-key\",\"description\":\"stream primary\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_STREAM_PRIMARY_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_FALLBACK_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-ai-stream-fallback").decode())')
AI_STREAM_FALLBACK_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  -d "{\"name\":\"ai-e2e-stream-fallback-key\",\"description\":\"stream fallback\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_STREAM_FALLBACK_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_PRIMARY_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-stream-primary\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_STREAM_DIE_PORT\",\"credential_secret_id\":\"$AI_STREAM_PRIMARY_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_FALLBACK_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/providers \
  -d "{\"name\":\"ai-e2e-stream-fallback\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_STREAM_FALLBACK_PORT\",\"credential_secret_id\":\"$AI_STREAM_FALLBACK_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_STREAM_ROUTE_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/ai/routes \
  -d "{\"name\":\"ai-e2e-stream\",\"spec\":{\"listener_port\":$AI_STREAM_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_STREAM_PRIMARY_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":0},{\"provider_id\":\"$AI_STREAM_FALLBACK_PROVIDER_ID\",\"models\":[],\"weight\":1,\"priority\":1}]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
for i in $(seq 1 60); do
  curl -fsS --max-time 5 http://127.0.0.1:$ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-dump.json 2>/dev/null || true
  grep -q "ai-ai-e2e-stream-listener" /tmp/fp-e2e-ai-dump.json \
    && grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json && break
  sleep 1
done
grep -q "ai-ai-e2e-stream-listener" /tmp/fp-e2e-ai-dump.json \
  && grep -q "envoy.clusters.aggregate" /tmp/fp-e2e-ai-dump.json \
  || fail "AI streaming route listener/aggregate cluster did not converge"
AI_STREAM_CODE=$(curl -sN --max-time 10 -o /tmp/fp-e2e-ai-stream-body.txt -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data '{"model":"gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}' \
  "http://127.0.0.1:$AI_STREAM_GATEWAY_PORT/v1/chat/completions" 2>/dev/null || true)
[ "$AI_STREAM_CODE" = "200" ] || fail "AI streaming request expected 200, got $AI_STREAM_CODE"
grep -q "partial-stream" /tmp/fp-e2e-ai-stream-body.txt \
  || fail "AI streaming client did not receive the partial stream chunk"
grep -q "Bearer fp-e2e-ai-stream-primary" /tmp/fp-e2e-ai-stream-die-auth.log \
  || fail "AI streaming primary did not receive its injected credential"
[ ! -s /tmp/fp-e2e-ai-stream-fallback-auth.log ] \
  || fail "AI failed over AFTER stream start: fallback was contacted ($(cat /tmp/fp-e2e-ai-stream-fallback-auth.log))"
AI_STREAM_RESULT="PHASE 1d OK: AI streaming-failover boundary -> partial stream delivered, no failover after first byte, fallback untouched"
AI_STREAM_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_STREAM_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_STREAM_ROUTE_REV" \
  http://$API/api/v1/teams/default/ai/routes/ai-e2e-stream >/dev/null
for prov in ai-e2e-stream-primary ai-e2e-stream-fallback; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE team_id = '$TEAM_ID' AND name = '$prov'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/default/ai/providers/$prov" >/dev/null
done
kill "$AI_STREAM_DIE_PID" "$AI_STREAM_FALLBACK_PID" >/dev/null 2>&1 || true
echo "$AI_STREAM_RESULT"
