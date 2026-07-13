# E2E phase P1g — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], helpers).
REQUIRES=""

# ---- Phase 1g: AI trace failure-path rows (feature ai-gateway-e2e-trace, slice s3).
# One live smoke per failure class, each asserting its persisted ai_trace_events row:
#   - no-eligible-backend direct 400  -> failure_hop='route_match', outcome no_eligible_backend (AC 11)
#   - provider 500                    -> failure_hop='upstream', hop status 500 + provider ids (AC 4)
#   - mid-SSE client disconnect       -> partial row, upstream hop outcome client_disconnect (Risk 6)
#   - upstream connect failure (503)  -> failure_hop='upstream', outcome no_upstream_connection (Risk 6)
#   - exhausted shadow budget         -> 2xx + budget hop shadow verdict would_reject (AC 3)
#   - exhausted enforcing budget      -> 429 + failure_hop='budget', verdict rejected, no later hops (AC 2)
#   - credential/secret failure       -> 500 credential-unavailable + failure_hop='credential_injection',
#     outcome secret_missing, provoked fully in-band by rotating the shared provider secret to a
#     near-future expires_at (the runtime credential read skips expired rows). Runs LAST: it kills
#     credentials for both providers. The decrypt_failed outcome variant of the same class stays at
#     the capture layer (fp-xds ai_credential_failure_stream_persists_secret_missing_and_decrypt_failed_rows):
#     REST validates generic secrets are base64 at create/rotate, so undecodable credential material
#     cannot be authored in-band — only out-of-band ciphertext corruption reaches that branch.
#
# Parallel-safety shape (constitution invariant 18): own uniquely named team with its own
# dataplane and Envoy, uniquely suffixed resource names, no fixed ports — the stub binds
# port 0; gateway/admin/dead-upstream ports come from a bind-0-then-release allocation.

AI_FAIL_SFX=$(python3 -c 'import secrets; print(secrets.token_hex(4))')
AI_FAIL_TEAM="e2e-trfail-$AI_FAIL_SFX"
AI_FAIL_ROUTE_NAME="ai-e2e-trfail-$AI_FAIL_SFX"
AI_FAIL_DEAD_ROUTE_NAME="ai-e2e-trfail-dead-$AI_FAIL_SFX"
AI_FAIL_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-trfail-secret").decode())')

# Gateway A (live stub), gateway B (dead upstream), Envoy admin, dead upstream port:
# unique per-run kernel allocations, all four sockets held open together.
AI_FAIL_PORTS=$(python3 - <<'PY'
import socket
socks = [socket.socket() for _ in range(4)]
for s in socks:
    s.bind(("127.0.0.1", 0))
print(" ".join(str(s.getsockname()[1]) for s in socks))
for s in socks:
    s.close()
PY
)
read -r AI_FAIL_GATEWAY_PORT AI_FAIL_DEAD_GATEWAY_PORT AI_FAIL_ADMIN_PORT AI_FAIL_DEAD_UPSTREAM_PORT <<<"$AI_FAIL_PORTS"

# Stub provider: 200+usage by default, 500 when the prompt carries the fail marker, and a
# never-ending SSE stream for stream:true requests (the client will disconnect mid-stream).
cat >/tmp/fp-e2e-ai-trfail-provider.py <<'PY'
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def read_body(self):
        # stream:true requests arrive chunked: the listener ExtProc's request-body
        # rewrite removes content-length, and BaseHTTPRequestHandler does not decode
        # chunked transfer-encoding natively.
        if "chunked" in (self.headers.get("transfer-encoding") or "").lower():
            data = b""
            while True:
                size_line = self.rfile.readline().split(b";")[0].strip()
                size = int(size_line or b"0", 16)
                if size == 0:
                    while self.rfile.readline() not in (b"\r\n", b"\n", b""):
                        pass
                    return data
                data += self.rfile.read(size)
                self.rfile.readline()
        length = int(self.headers.get("content-length", "0") or 0)
        return self.rfile.read(length) if length else b""

    def do_POST(self):
        raw = self.read_body()
        try:
            body = json.loads(raw)
        except Exception:
            body = {}
        prompt = json.dumps(body.get("messages", []))
        if "fp-fail-500" in prompt:
            data = json.dumps({"error": {"message": "mock provider failure"}}).encode()
            self.send_response(500)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        if body.get("stream") and "fp-sse-finite" in prompt:
            # Finite stream (design AC 3): N content chunks + an OpenAI include_usage
            # final chunk + [DONE], then a clean close. The gateway injected
            # include_usage (the client never asked), so the usage event must be
            # stripped downstream while Flowplane settles usage/budgets exactly once.
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            for i in range(3):
                chunk = "data: " + json.dumps(
                    {"id": "chatcmpl-trfail-finite", "choices": [{"index": 0, "delta": {"content": f"ftok{i}"}}]}
                ) + "\n\n"
                self.wfile.write(chunk.encode())
            usage = "data: " + json.dumps(
                {"id": "chatcmpl-trfail-finite", "choices": [],
                 "usage": {"prompt_tokens": 7, "completion_tokens": 11, "total_tokens": 18}}
            ) + "\n\n"
            self.wfile.write(usage.encode())
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return
        if body.get("stream"):
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            try:
                for i in range(60):
                    chunk = "data: " + json.dumps(
                        {"id": "chatcmpl-trfail-sse", "choices": [{"index": 0, "delta": {"content": f"tok{i}"}}]}
                    ) + "\n\n"
                    self.wfile.write(chunk.encode())
                    self.wfile.flush()
                    time.sleep(1)
            except OSError:
                pass
            return
        data = json.dumps({
            "id": "chatcmpl-trfail-ok",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "mock-trfail-ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        }).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
rm -f /tmp/fp-e2e-ai-trfail-provider.port
python3 /tmp/fp-e2e-ai-trfail-provider.py /tmp/fp-e2e-ai-trfail-provider.port \
  >/tmp/fp-e2e-ai-trfail-provider.log 2>&1 &
AI_FAIL_STUB_PID=$!
AI_FAIL_PROVIDER_PORT=""
for i in $(seq 1 20); do
  AI_FAIL_PROVIDER_PORT=$(cat /tmp/fp-e2e-ai-trfail-provider.port 2>/dev/null || true)
  [ -n "$AI_FAIL_PROVIDER_PORT" ] && break
  sleep 0.5
done
[ -n "$AI_FAIL_PROVIDER_PORT" ] || fail "AI trace-failure stub provider did not report its bound port"

# Unique team + its own dataplane and Envoy (xDS is team-scoped).
AI_FAIL_TEAM_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams -d "{\"name\":\"$AI_FAIL_TEAM\"}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
[ -n "$AI_FAIL_TEAM_ID" ] || fail "AI trace-failure team $AI_FAIL_TEAM was not created"

AI_FAIL_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/secrets \
  -d "{\"name\":\"ai-e2e-trfail-key-$AI_FAIL_SFX\",\"description\":\"AI trace-failure credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_FAIL_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_FAIL_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/providers \
  -d "{\"name\":\"ai-e2e-trfail-live-$AI_FAIL_SFX\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_FAIL_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_FAIL_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_FAIL_DEAD_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/providers \
  -d "{\"name\":\"ai-e2e-trfail-dead-$AI_FAIL_SFX\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_FAIL_DEAD_UPSTREAM_PORT\",\"credential_secret_id\":\"$AI_FAIL_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
# Route A: single backend constrained to gpt-5 so an unknown model has NO eligible backend.
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/routes \
  -d "{\"name\":\"$AI_FAIL_ROUTE_NAME\",\"spec\":{\"listener_port\":$AI_FAIL_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_FAIL_PROVIDER_ID\",\"models\":[\"gpt-5\"],\"weight\":1}]}}" \
  >/dev/null
# Route B: backend whose upstream port has no listener -> Envoy local 503 connect failure.
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/routes \
  -d "{\"name\":\"$AI_FAIL_DEAD_ROUTE_NAME\",\"spec\":{\"listener_port\":$AI_FAIL_DEAD_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_FAIL_DEAD_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}" \
  >/dev/null
AI_FAIL_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$AI_FAIL_TEAM_ID' AND name = 'ai-$AI_FAIL_ROUTE_NAME-routes' AND owner_kind = 'ai'")
[ -n "$AI_FAIL_ROUTE_CONFIG_ID" ] || fail "AI trace-failure route materialized route config not found"

FLOWPLANE_TEAM=$AI_FAIL_TEAM ./target/debug/flowplane dataplane create "dp-trfail-$AI_FAIL_SFX" \
  --description "AI trace-failure e2e Envoy" >/dev/null
FLOWPLANE_TEAM=$AI_FAIL_TEAM ./target/debug/flowplane --out /tmp/fp-e2e-ai-trfail-bootstrap.yaml \
  dataplane bootstrap "dp-trfail-$AI_FAIL_SFX" --mode dev \
  --xds-host 127.0.0.1 --xds-port "$XDS_PORT" --admin-port "$AI_FAIL_ADMIN_PORT"
if [ "$ENVOY_MODE" = docker ]; then
  docker run -d --name fp-e2e-envoy-trfail --network host \
    -v /tmp/fp-e2e-ai-trfail-bootstrap.yaml:/etc/envoy/envoy.yaml:ro \
    envoyproxy/envoy:v${ENVOY_VERSION} -c /etc/envoy/envoy.yaml --log-level info >/dev/null
else
  # --base-id 2: must not share the shared-memory region with the harness Envoy (0) or the
  # phase-1f Envoy (1).
  envoy -c /tmp/fp-e2e-ai-trfail-bootstrap.yaml --base-id 2 --log-level info \
    > /tmp/fp-e2e-ai-trfail-envoy.log 2>&1 &
  AI_FAIL_ENVOY_PID=$!
fi
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$AI_FAIL_ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-trfail-dump.json 2>/dev/null || true
  grep -q "ai-$AI_FAIL_ROUTE_NAME-listener" /tmp/fp-e2e-ai-trfail-dump.json \
    && grep -q "ai-$AI_FAIL_DEAD_ROUTE_NAME-listener" /tmp/fp-e2e-ai-trfail-dump.json && break
  sleep 1
done
grep -q "ai-$AI_FAIL_ROUTE_NAME-listener" /tmp/fp-e2e-ai-trfail-dump.json \
  && grep -q "ai-$AI_FAIL_DEAD_ROUTE_NAME-listener" /tmp/fp-e2e-ai-trfail-dump.json \
  || fail "AI trace-failure listeners did not converge on the phase Envoy"

# Poll helper: wait for the trace row of one request id and dump it as JSON to $2.
ai_fail_row() { # $1 = request id, $2 = out file, $3 = grep marker that must appear in the row
  local row="" i
  for i in $(seq 1 30); do
    row=$(psql "$PG_DB_URL" -Atc "SELECT row_to_json(t)::text FROM (SELECT * FROM ai_trace_events WHERE team_id = '$AI_FAIL_TEAM_ID' AND request_id = '$1') t" 2>/dev/null || true)
    [ -n "$row" ] && grep -q "$3" <<<"$row" && break
    sleep 1
  done
  [ -n "$row" ] || return 1
  grep -q "$3" <<<"$row" || return 1
  printf '%s' "$row" > "$2"
}

AI_FAIL_OK_REQUEST='{"model":"gpt-5","messages":[{"role":"user","content":"trace failure smoke"}]}'

# Warm the route: first request may race listener/cluster warm-up on a fresh Envoy.
AI_FAIL_WARM=""
for i in $(seq 1 50); do
  AI_FAIL_WARM=$(curl -sS -o /dev/null -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
    --data "$AI_FAIL_OK_REQUEST" \
    http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_FAIL_WARM" = "200" ] && break
  sleep 1
done
[ "$AI_FAIL_WARM" = "200" ] || fail "AI trace-failure route never served the live stub (last code $AI_FAIL_WARM)"

# ---- AC 11: model matching no backend -> direct 400 no_eligible_ai_backend + row.
AI_FAIL_400_CODE=$(curl -sS -o /tmp/fp-e2e-trfail-400.json -D /tmp/fp-e2e-trfail-400.hdrs -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: no-such-model" \
  --data '{"model":"no-such-model","messages":[{"role":"user","content":"nope"}]}' \
  http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions)
[ "$AI_FAIL_400_CODE" = "400" ] || fail "no-eligible-backend request expected 400, got $AI_FAIL_400_CODE"
grep -qi "no_eligible_ai_backend" /tmp/fp-e2e-trfail-400.json || fail "400 body did not carry no_eligible_ai_backend"
AI_FAIL_400_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-400.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
[ -n "$AI_FAIL_400_REQ" ] || fail "no-eligible-backend response carried no x-request-id"
ai_fail_row "$AI_FAIL_400_REQ" /tmp/fp-e2e-trfail-400-row.json '"route_match"' \
  || fail "no ai_trace_events row for the no-eligible-backend request $AI_FAIL_400_REQ"
python3 - /tmp/fp-e2e-trfail-400-row.json <<'PY' || fail "no-eligible-backend row failed assertions"
import json, sys
row = json.load(open(sys.argv[1], encoding="utf-8"))
assert row["failure_hop"] == "route_match", row["failure_hop"]
by_name = {h["hop"]: h for h in row["hops"]}
assert by_name["route_match"]["outcome"] in ("no_eligible_backend", "no_upstream_response"), by_name["route_match"]
assert by_name["route_match"]["failed"] is True
assert "upstream" not in by_name and "credential_injection" not in by_name, sorted(by_name)
PY

# ---- AC 4: stub upstream returns 500 -> failure_hop='upstream' with status + provider ids.
AI_FAIL_500_CODE=$(curl -sS -o /dev/null -D /tmp/fp-e2e-trfail-500.hdrs -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data '{"model":"gpt-5","messages":[{"role":"user","content":"fp-fail-500"}]}' \
  http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions)
[ "$AI_FAIL_500_CODE" = "500" ] || fail "provider-failure request expected 500, got $AI_FAIL_500_CODE"
AI_FAIL_500_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-500.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
ai_fail_row "$AI_FAIL_500_REQ" /tmp/fp-e2e-trfail-500-row.json '"upstream"' \
  || fail "no ai_trace_events row for the provider-500 request $AI_FAIL_500_REQ"
python3 - "$AI_FAIL_PROVIDER_ID" /tmp/fp-e2e-trfail-500-row.json <<'PY' || fail "provider-500 row failed assertions"
import json, sys
row = json.load(open(sys.argv[2], encoding="utf-8"))
assert row["failure_hop"] == "upstream", row["failure_hop"]
assert row["provider_id"] == sys.argv[1], row["provider_id"]
up = next(h for h in row["hops"] if h["hop"] == "upstream")
assert up["detail"]["status"] == 500, up["detail"]
assert up["detail"]["provider_id"] == sys.argv[1], up["detail"]
assert up["detail"]["backend_position"] == 0, up["detail"]
assert up["failed"] is True
PY

# ---- Risk 6 (disconnect half): client disconnect mid-SSE still persists a partial row.
curl -sN --max-time 3 -D /tmp/fp-e2e-trfail-sse.hdrs -o /tmp/fp-e2e-trfail-sse.body \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data '{"model":"gpt-5","stream":true,"messages":[{"role":"user","content":"keep streaming"}]}' \
  http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions || true
grep -q "tok0" /tmp/fp-e2e-trfail-sse.body || fail "mid-SSE test never received a first stream chunk"
AI_FAIL_SSE_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-sse.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
[ -n "$AI_FAIL_SSE_REQ" ] || fail "mid-SSE response carried no x-request-id"
ai_fail_row "$AI_FAIL_SSE_REQ" /tmp/fp-e2e-trfail-sse-row.json '"client_disconnect"' \
  || fail "no partial ai_trace_events row with client_disconnect for mid-SSE request $AI_FAIL_SSE_REQ"
python3 - /tmp/fp-e2e-trfail-sse-row.json <<'PY' || fail "mid-SSE disconnect row failed assertions"
import json, sys
row = json.load(open(sys.argv[1], encoding="utf-8"))
up = next(h for h in row["hops"] if h["hop"] == "upstream")
assert up["outcome"] == "client_disconnect", up
names = {h["hop"] for h in row["hops"]}
assert "usage" not in names, "a torn-down stream must not fabricate a usage hop"
PY

# ---- Risk 6 (connect half): dead upstream -> local 503 -> outcome no_upstream_connection.
AI_FAIL_503_CODE=$(curl -sS -o /dev/null -D /tmp/fp-e2e-trfail-503.hdrs -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data "$AI_FAIL_OK_REQUEST" \
  http://127.0.0.1:$AI_FAIL_DEAD_GATEWAY_PORT/v1/chat/completions)
[ "$AI_FAIL_503_CODE" = "503" ] || fail "dead-upstream request expected 503, got $AI_FAIL_503_CODE"
AI_FAIL_503_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-503.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
ai_fail_row "$AI_FAIL_503_REQ" /tmp/fp-e2e-trfail-503-row.json '"no_upstream_connection"' \
  || fail "no ai_trace_events row with no_upstream_connection for request $AI_FAIL_503_REQ"
python3 - /tmp/fp-e2e-trfail-503-row.json <<'PY' || fail "connect-failure row failed assertions"
import json, sys
row = json.load(open(sys.argv[1], encoding="utf-8"))
assert row["failure_hop"] == "upstream", row["failure_hop"]
up = next(h for h in row["hops"] if h["hop"] == "upstream")
assert up["outcome"] == "no_upstream_connection", up
assert up["failed"] is True
assert "usage" not in {h["hop"] for h in row["hops"]}
PY

# ---- AC 3: exhausted SHADOW budget -> request still 2xx, budget hop records would_reject.
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/budgets \
  -d "{\"name\":\"trfail-shadow-$AI_FAIL_SFX\",\"spec\":{\"mode\":\"shadow\",\"limit_units\":1,\"window_seconds\":3600,\"provider_id\":\"$AI_FAIL_PROVIDER_ID\",\"prompt_token_weight\":1,\"completion_token_weight\":1}}" \
  >/dev/null
# Settle usage into the shadow counter (5 units > limit 1), then trace the next request.
curl -fsS -o /dev/null \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data "$AI_FAIL_OK_REQUEST" http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions
for i in $(seq 1 20); do
  AI_FAIL_SHADOW_USED=$(psql "$PG_DB_URL" -Atc "SELECT c.used_units FROM ai_budget_counters c JOIN ai_budgets b ON b.id = c.budget_id WHERE b.team_id = '$AI_FAIL_TEAM_ID' AND b.name = 'trfail-shadow-$AI_FAIL_SFX'")
  [ -n "$AI_FAIL_SHADOW_USED" ] && [ "$AI_FAIL_SHADOW_USED" -ge 1 ] && break
  sleep 1
done
[ -n "${AI_FAIL_SHADOW_USED:-}" ] && [ "$AI_FAIL_SHADOW_USED" -ge 1 ] || fail "shadow budget counter never settled"
AI_FAIL_SHADOW_CODE=$(curl -sS -o /dev/null -D /tmp/fp-e2e-trfail-shadow.hdrs -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data "$AI_FAIL_OK_REQUEST" http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions)
[ "$AI_FAIL_SHADOW_CODE" = "200" ] || fail "shadow-exhausted request must still succeed, got $AI_FAIL_SHADOW_CODE"
AI_FAIL_SHADOW_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-shadow.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
ai_fail_row "$AI_FAIL_SHADOW_REQ" /tmp/fp-e2e-trfail-shadow-row.json '"would_reject"' \
  || fail "no ai_trace_events row with a would_reject shadow verdict for request $AI_FAIL_SHADOW_REQ"
python3 - "trfail-shadow-$AI_FAIL_SFX" /tmp/fp-e2e-trfail-shadow-row.json <<'PY' || fail "shadow-verdict row failed assertions"
import json, sys
row = json.load(open(sys.argv[2], encoding="utf-8"))
assert row["failure_hop"] is None, "shadow budgets never fail the request"
assert row["status_code"] == 200, row["status_code"]
budget = next(h for h in row["hops"] if h["hop"] == "budget")
assert budget["outcome"] == "allowed", budget
shadow = {entry["budget"]: entry for entry in budget["detail"]["shadow"]}
assert shadow[sys.argv[1]]["verdict"] == "would_reject", shadow
PY

# ---- Design AC 3 (fpv2-o6w.3): finite SSE stream — the synthetic usage event is stripped
# from a client that never asked for usage, every content chunk arrives in order, and
# Flowplane settles usage + budgets EXACTLY once (exact-count deltas, not row-existence).
# Runs before the enforcing budget exists (it would 429 this request) and settles into the
# shadow counter (weights 1/1 -> units = prompt 7 + completion 11 = 18).
# Usage insert + budget settlement share one transaction, so a stable usage-event count
# implies a stable counter; quiesce the async settlements of the earlier legs first.
AI_FAIL_FINITE_COUNT_SQL="SELECT count(*) FROM ai_usage_events WHERE team_id = '$AI_FAIL_TEAM_ID' AND provider_id = '$AI_FAIL_PROVIDER_ID' AND route_config_id = '$AI_FAIL_ROUTE_CONFIG_ID'"
AI_FAIL_FINITE_UNITS_SQL="SELECT COALESCE(SUM(c.used_units), 0) FROM ai_budget_counters c JOIN ai_budgets b ON b.id = c.budget_id WHERE b.team_id = '$AI_FAIL_TEAM_ID' AND b.name = 'trfail-shadow-$AI_FAIL_SFX'"
AI_FAIL_FINITE_USAGE_BASE=-1
AI_FAIL_FINITE_STABLE=0
for i in $(seq 1 20); do
  AI_FAIL_FINITE_NOW=$(psql "$PG_DB_URL" -Atc "$AI_FAIL_FINITE_COUNT_SQL")
  if [ "$AI_FAIL_FINITE_NOW" = "$AI_FAIL_FINITE_USAGE_BASE" ]; then
    AI_FAIL_FINITE_STABLE=1
    break
  fi
  AI_FAIL_FINITE_USAGE_BASE=$AI_FAIL_FINITE_NOW
  sleep 1
done
[ "$AI_FAIL_FINITE_STABLE" = "1" ] || fail "finite-stream leg: usage-event count never stabilized before baseline"
AI_FAIL_FINITE_UNITS_BASE=$(psql "$PG_DB_URL" -Atc "$AI_FAIL_FINITE_UNITS_SQL")
AI_FAIL_FINITE_CODE=$(curl -sN --max-time 10 -o /tmp/fp-e2e-trfail-finite.body -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data '{"model":"gpt-5","stream":true,"messages":[{"role":"user","content":"fp-sse-finite"}]}' \
  http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions)
[ "$AI_FAIL_FINITE_CODE" = "200" ] || fail "finite-stream request expected 200, got $AI_FAIL_FINITE_CODE"
# AC 3 byte-fidelity: the client body must be BYTE-IDENTICAL to the provider stream with
# only the usage event removed — rebuilt here with the same json.dumps calls as the stub.
python3 - /tmp/fp-e2e-trfail-finite.body <<'PY' || fail "finite-stream client body failed assertions"
import json, sys
body = open(sys.argv[1], "rb").read()
expected = b""
for i in range(3):
    expected += ("data: " + json.dumps(
        {"id": "chatcmpl-trfail-finite", "choices": [{"index": 0, "delta": {"content": f"ftok{i}"}}]}
    ) + "\n\n").encode()
expected += b"data: [DONE]\n\n"
assert b'"usage"' not in body, "synthetic usage event leaked to a client that never asked for usage"
assert body == expected, (
    "client body not byte-identical to the stripped provider stream:\n"
    f"got      {body!r}\nexpected {expected!r}"
)
PY
AI_FAIL_FINITE_EXPECT_COUNT=$((AI_FAIL_FINITE_USAGE_BASE + 1))
AI_FAIL_FINITE_EXPECT_UNITS=$((AI_FAIL_FINITE_UNITS_BASE + 18))
for i in $(seq 1 20); do
  AI_FAIL_FINITE_USAGE_NOW=$(psql "$PG_DB_URL" -Atc "$AI_FAIL_FINITE_COUNT_SQL")
  [ "$AI_FAIL_FINITE_USAGE_NOW" -ge "$AI_FAIL_FINITE_EXPECT_COUNT" ] && break
  sleep 1
done
[ "$AI_FAIL_FINITE_USAGE_NOW" = "$AI_FAIL_FINITE_EXPECT_COUNT" ] \
  || fail "finite-stream usage rows: expected exactly $AI_FAIL_FINITE_EXPECT_COUNT, got $AI_FAIL_FINITE_USAGE_NOW"
AI_FAIL_FINITE_TOKENS=$(psql "$PG_DB_URL" -Atc "SELECT prompt_tokens || ':' || completion_tokens || ':' || total_tokens FROM ai_usage_events WHERE team_id = '$AI_FAIL_TEAM_ID' AND provider_id = '$AI_FAIL_PROVIDER_ID' AND route_config_id = '$AI_FAIL_ROUTE_CONFIG_ID' ORDER BY created_at DESC LIMIT 1")
[ "$AI_FAIL_FINITE_TOKENS" = "7:11:18" ] \
  || fail "finite-stream usage row tokens expected 7:11:18, got $AI_FAIL_FINITE_TOKENS"
AI_FAIL_FINITE_UNITS_NOW=$(psql "$PG_DB_URL" -Atc "$AI_FAIL_FINITE_UNITS_SQL")
[ "$AI_FAIL_FINITE_UNITS_NOW" = "$AI_FAIL_FINITE_EXPECT_UNITS" ] \
  || fail "finite-stream budget units: expected exactly $AI_FAIL_FINITE_EXPECT_UNITS, got $AI_FAIL_FINITE_UNITS_NOW"
# Duplicate-settlement window: a second (double) write would land promptly after the first.
sleep 2
AI_FAIL_FINITE_USAGE_NOW=$(psql "$PG_DB_URL" -Atc "$AI_FAIL_FINITE_COUNT_SQL")
AI_FAIL_FINITE_UNITS_NOW=$(psql "$PG_DB_URL" -Atc "$AI_FAIL_FINITE_UNITS_SQL")
[ "$AI_FAIL_FINITE_USAGE_NOW" = "$AI_FAIL_FINITE_EXPECT_COUNT" ] \
  || fail "finite-stream usage settled MORE than once: $AI_FAIL_FINITE_USAGE_NOW rows (expected $AI_FAIL_FINITE_EXPECT_COUNT)"
[ "$AI_FAIL_FINITE_UNITS_NOW" = "$AI_FAIL_FINITE_EXPECT_UNITS" ] \
  || fail "finite-stream budget settled MORE than once: $AI_FAIL_FINITE_UNITS_NOW units (expected $AI_FAIL_FINITE_EXPECT_UNITS)"

# ---- AC 2: exhausted ENFORCING budget -> 429 flowplane_ai_budget_exceeded + budget row.
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/budgets \
  -d "{\"name\":\"trfail-hard-$AI_FAIL_SFX\",\"spec\":{\"mode\":\"enforcing\",\"limit_units\":1,\"window_seconds\":3600,\"provider_id\":\"$AI_FAIL_PROVIDER_ID\",\"prompt_token_weight\":1,\"completion_token_weight\":1}}" \
  >/dev/null
# Fresh counter starts at 0 < 1, so one more success settles it past the limit.
curl -fsS -o /dev/null \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data "$AI_FAIL_OK_REQUEST" http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions
for i in $(seq 1 20); do
  AI_FAIL_HARD_USED=$(psql "$PG_DB_URL" -Atc "SELECT c.used_units FROM ai_budget_counters c JOIN ai_budgets b ON b.id = c.budget_id WHERE b.team_id = '$AI_FAIL_TEAM_ID' AND b.name = 'trfail-hard-$AI_FAIL_SFX'")
  [ -n "$AI_FAIL_HARD_USED" ] && [ "$AI_FAIL_HARD_USED" -ge 1 ] && break
  sleep 1
done
[ -n "${AI_FAIL_HARD_USED:-}" ] && [ "$AI_FAIL_HARD_USED" -ge 1 ] || fail "enforcing budget counter never settled"
AI_FAIL_429_CODE=$(curl -sS -o /tmp/fp-e2e-trfail-429.body -D /tmp/fp-e2e-trfail-429.hdrs -w '%{http_code}' \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
  --data "$AI_FAIL_OK_REQUEST" http://127.0.0.1:$AI_FAIL_GATEWAY_PORT/v1/chat/completions)
[ "$AI_FAIL_429_CODE" = "429" ] || fail "budget-exhausted request expected 429, got $AI_FAIL_429_CODE"
grep -q "exceeded" /tmp/fp-e2e-trfail-429.body || fail "429 body did not carry the budget-exceeded message"
AI_FAIL_429_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-429.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
[ -n "$AI_FAIL_429_REQ" ] || fail "429 response carried no x-request-id"
ai_fail_row "$AI_FAIL_429_REQ" /tmp/fp-e2e-trfail-429-row.json '"rejected"' \
  || fail "no ai_trace_events row for the budget-rejected request $AI_FAIL_429_REQ"
python3 - "trfail-hard-$AI_FAIL_SFX" /tmp/fp-e2e-trfail-429-row.json <<'PY' || fail "budget-reject row failed assertions"
import json, sys
row = json.load(open(sys.argv[2], encoding="utf-8"))
assert row["failure_hop"] == "budget", row["failure_hop"]
by_name = {h["hop"]: h for h in row["hops"]}
budget = by_name["budget"]
assert budget["outcome"] == "rejected"
assert budget["detail"]["verdict"] == "rejected", budget["detail"]
assert budget["detail"]["budget"] == sys.argv[1], budget["detail"]
assert budget["failed"] is True
assert "credential_injection" not in by_name and "upstream" not in by_name, sorted(by_name)
PY

# ---- Credential/secret failure: expire the shared secret, then hit route B (dead provider:
# credential injection precedes upstream connect, and no budget guards that provider, so the
# request dies at the credential hop). The runtime reads the secret row per request and treats
# an expired row as absent -> outcome secret_missing + the credential-unavailable 500.
AI_FAIL_SECRET_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM secrets WHERE team_id = '$AI_FAIL_TEAM_ID' AND name = 'ai-e2e-trfail-key-$AI_FAIL_SFX'")
[ -n "$AI_FAIL_SECRET_REV" ] || fail "AI trace-failure secret revision not found for rotation"
AI_FAIL_EXPIRY=$(python3 -c 'from datetime import datetime, timezone, timedelta; print((datetime.now(timezone.utc) + timedelta(seconds=5)).isoformat())')
curl -fsS "${auth[@]}" -X POST -H "If-Match: $AI_FAIL_SECRET_REV" \
  "http://$API/api/v1/teams/$AI_FAIL_TEAM/secrets/ai-e2e-trfail-key-$AI_FAIL_SFX/rotate" \
  -d "{\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_FAIL_SECRET_B64\"},\"expires_at\":\"$AI_FAIL_EXPIRY\"}" \
  >/dev/null
# Until expires_at passes the dead route still 503s at the upstream; poll for the flip to the
# credential-unavailable 500 and keep that request's id for the row assertion.
AI_FAIL_CRED_CODE=""
for i in $(seq 1 30); do
  AI_FAIL_CRED_CODE=$(curl -sS -o /tmp/fp-e2e-trfail-cred.body -D /tmp/fp-e2e-trfail-cred.hdrs -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
    --data "$AI_FAIL_OK_REQUEST" http://127.0.0.1:$AI_FAIL_DEAD_GATEWAY_PORT/v1/chat/completions)
  [ "$AI_FAIL_CRED_CODE" = "500" ] && grep -qi "credential unavailable" /tmp/fp-e2e-trfail-cred.body && break
  sleep 1
done
[ "$AI_FAIL_CRED_CODE" = "500" ] || fail "expired-secret request expected 500, got $AI_FAIL_CRED_CODE"
grep -qi "AI provider credential unavailable" /tmp/fp-e2e-trfail-cred.body \
  || fail "500 body did not carry the credential-unavailable message"
AI_FAIL_CRED_REQ=$(grep -i '^x-request-id:' /tmp/fp-e2e-trfail-cred.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
[ -n "$AI_FAIL_CRED_REQ" ] || fail "credential-failure response carried no x-request-id"
ai_fail_row "$AI_FAIL_CRED_REQ" /tmp/fp-e2e-trfail-cred-row.json '"secret_missing"' \
  || fail "no ai_trace_events row with secret_missing for the credential-failure request $AI_FAIL_CRED_REQ"
python3 - "$AI_FAIL_DEAD_PROVIDER_ID" "$AI_FAIL_SECRET_B64" /tmp/fp-e2e-trfail-cred-row.json <<'PY' || fail "credential-failure row failed assertions"
import json, sys
row = json.load(open(sys.argv[3], encoding="utf-8"))
assert row["failure_hop"] == "credential_injection", row["failure_hop"]
# Column ownership (fp-storage ai_trace upsert): row-level provider_id is the upstream
# stream's column, and a credential failure answers before any upstream stream opens —
# so it stays NULL; the selected provider is asserted on the credential hop detail below.
assert row["provider_id"] is None, row["provider_id"]
by_name = {h["hop"]: h for h in row["hops"]}
cred = by_name["credential_injection"]
assert cred["outcome"] == "secret_missing", cred
assert cred["failed"] is True
assert cred["detail"]["auth_header"] == "authorization", cred["detail"]
assert cred["detail"]["provider_id"] == sys.argv[1], cred["detail"]
assert "upstream" not in by_name and "usage" not in by_name, sorted(by_name)
row_text = json.dumps(row)
assert "fp-e2e-trfail-secret" not in row_text, "credential material must never appear in the trace row"
assert sys.argv[2] not in row_text, "encoded credential material must never appear in the trace row"
PY

# ---- Cleanup: this phase's Envoy, stub, and team resources.
kill "$AI_FAIL_STUB_PID" >/dev/null 2>&1 || true
if [ "$ENVOY_MODE" = docker ]; then
  docker rm -f fp-e2e-envoy-trfail >/dev/null 2>&1 || true
else
  kill "${AI_FAIL_ENVOY_PID:-}" >/dev/null 2>&1 || true
fi
for budget in "trfail-shadow-$AI_FAIL_SFX" "trfail-hard-$AI_FAIL_SFX"; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_budgets WHERE team_id = '$AI_FAIL_TEAM_ID' AND name = '$budget'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/budgets/$budget" >/dev/null
done
for route in "$AI_FAIL_ROUTE_NAME" "$AI_FAIL_DEAD_ROUTE_NAME"; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE team_id = '$AI_FAIL_TEAM_ID' AND name = '$route'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/routes/$route" >/dev/null
done
for prov in "ai-e2e-trfail-live-$AI_FAIL_SFX" "ai-e2e-trfail-dead-$AI_FAIL_SFX"; do
  REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE team_id = '$AI_FAIL_TEAM_ID' AND name = '$prov'")
  curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $REV" \
    "http://$API/api/v1/teams/$AI_FAIL_TEAM/ai/providers/$prov" >/dev/null
done
echo "PHASE 1g OK: AI trace failure rows -> no-eligible 400 (AC11) -> provider 500 (AC4) -> mid-SSE client_disconnect + connect-failure 503 (Risk 6) -> shadow would_reject (AC3) -> finite-stream usage strip + exactly-once settle (fpv2-o6w AC3) -> enforcing 429 budget row (AC2) -> expired-secret credential_injection secret_missing row"
