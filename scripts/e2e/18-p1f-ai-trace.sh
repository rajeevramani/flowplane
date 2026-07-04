# E2E phase P1f — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], helpers).
REQUIRES=""

# ---- Phase 1f: AI e2e trace rows (feature ai-gateway-e2e-trace, slice s2).
# Covers design AC 1 (success trace with the full hop timeline), AC 14 (server-owned
# request identity — client-supplied x-request-id never keys a row), AC 9 inbound-only
# (traceparent reaches the stub unchanged and trace_id lands on the row / stays NULL when
# absent), AC 6 (no prompt/secret strings in the stored row), and the 30-day default TTL.
# Trace retrieval REST/CLI is slice s4; this phase asserts the persisted rows directly,
# the same way earlier phases assert ai_usage_events.
#
# Parallel-safety shape (constitution invariant 18, plan s2 e2e layer): this phase creates
# its own uniquely named team with its own dataplane and Envoy (xDS is team-scoped, so the
# harness Envoy bound to team default cannot serve another team's listener), uses uniquely
# suffixed resource names, and no fixed ports — the stub upstream binds port 0 and reports
# the kernel-chosen port back; the gateway listener and Envoy admin ports must be concrete
# numbers in config, so they come from a bind-0-then-release per-run allocation.

AI_TRACE_SFX=$(python3 -c 'import secrets; print(secrets.token_hex(4))')
AI_TRACE_TEAM="e2e-trace-$AI_TRACE_SFX"
AI_TRACE_ROUTE_NAME="ai-e2e-trace-$AI_TRACE_SFX"
AI_TRACE_SECRET_VALUE="Bearer fp-e2e-trace-secret"
AI_TRACE_SECRET_B64=$(python3 -c 'import base64; print(base64.b64encode(b"Bearer fp-e2e-trace-secret").decode())')

# Gateway listener + trace-Envoy admin ports: unique per-run kernel allocations (both
# sockets held open together so the two ports cannot collide with each other).
AI_TRACE_PORTS=$(python3 - <<'PY'
import socket
socks = [socket.socket() for _ in range(2)]
for s in socks:
    s.bind(("127.0.0.1", 0))
print(" ".join(str(s.getsockname()[1]) for s in socks))
for s in socks:
    s.close()
PY
)
AI_TRACE_GATEWAY_PORT=${AI_TRACE_PORTS%% *}
AI_TRACE_ADMIN_PORT=${AI_TRACE_PORTS##* }

# Header-logging stub provider: binds port 0, writes the chosen port to a file, and records
# traceparent/x-request-id/authorization per request.
cat >/tmp/fp-e2e-ai-trace-provider.py <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]
log_path = sys.argv[2]

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        self.rfile.read(length)
        with open(log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({
                "traceparent": self.headers.get("traceparent"),
                "x_request_id": self.headers.get("x-request-id"),
                "authorization": self.headers.get("authorization"),
            }) + "\n")
        body = {
            "id": "chatcmpl-fp-e2e-trace",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "mock-ai-trace-ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        }
        data = json.dumps(body).encode()
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
: >/tmp/fp-e2e-ai-trace-upstream.jsonl
rm -f /tmp/fp-e2e-ai-trace-provider.port
python3 /tmp/fp-e2e-ai-trace-provider.py /tmp/fp-e2e-ai-trace-provider.port /tmp/fp-e2e-ai-trace-upstream.jsonl >/tmp/fp-e2e-ai-trace-provider.log 2>&1 &
AI_TRACE_STUB_PID=$!
AI_TRACE_PROVIDER_PORT=""
for i in $(seq 1 20); do
  AI_TRACE_PROVIDER_PORT=$(cat /tmp/fp-e2e-ai-trace-provider.port 2>/dev/null || true)
  [ -n "$AI_TRACE_PROVIDER_PORT" ] && break
  sleep 0.5
done
[ -n "$AI_TRACE_PROVIDER_PORT" ] || fail "AI trace stub provider did not report its bound port"

# Unique team + its own dataplane and Envoy (xDS is team-scoped; see phase header).
AI_TRACE_TEAM_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams -d "{\"name\":\"$AI_TRACE_TEAM\"}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
[ -n "$AI_TRACE_TEAM_ID" ] || fail "AI trace team $AI_TRACE_TEAM was not created"

AI_TRACE_SECRET_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_TRACE_TEAM/secrets \
  -d "{\"name\":\"ai-e2e-trace-key-$AI_TRACE_SFX\",\"description\":\"AI e2e trace credential\",\"spec\":{\"type\":\"generic_secret\",\"secret\":\"$AI_TRACE_SECRET_B64\"}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_TRACE_PROVIDER_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_TRACE_TEAM/ai/providers \
  -d "{\"name\":\"ai-e2e-trace-provider-$AI_TRACE_SFX\",\"spec\":{\"kind\":\"openai-compatible\",\"base_url\":\"http://127.0.0.1:$AI_TRACE_PROVIDER_PORT\",\"credential_secret_id\":\"$AI_TRACE_SECRET_ID\",\"auth_header\":\"authorization\",\"models\":[\"gpt-5\"]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_TRACE_ROUTE_ID=$(curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/$AI_TRACE_TEAM/ai/routes \
  -d "{\"name\":\"$AI_TRACE_ROUTE_NAME\",\"spec\":{\"listener_port\":$AI_TRACE_GATEWAY_PORT,\"backends\":[{\"provider_id\":\"$AI_TRACE_PROVIDER_ID\",\"models\":[],\"weight\":1}]}}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
AI_TRACE_ROUTE_CONFIG_ID=$(psql "$PG_DB_URL" -Atc "SELECT id FROM route_configs WHERE team_id = '$AI_TRACE_TEAM_ID' AND name = 'ai-$AI_TRACE_ROUTE_NAME-routes' AND owner_kind = 'ai'")
[ -n "$AI_TRACE_ROUTE_CONFIG_ID" ] || fail "AI trace route materialized route config not found"

FLOWPLANE_TEAM=$AI_TRACE_TEAM ./target/debug/flowplane dataplane create "dp-trace-$AI_TRACE_SFX" \
  --description "AI trace e2e Envoy" >/dev/null
FLOWPLANE_TEAM=$AI_TRACE_TEAM ./target/debug/flowplane --out /tmp/fp-e2e-ai-trace-bootstrap.yaml \
  dataplane bootstrap "dp-trace-$AI_TRACE_SFX" --mode dev \
  --xds-host 127.0.0.1 --xds-port "$XDS_PORT" --admin-port "$AI_TRACE_ADMIN_PORT"
if [ "$ENVOY_MODE" = docker ]; then
  docker run -d --name fp-e2e-envoy-trace --network host \
    -v /tmp/fp-e2e-ai-trace-bootstrap.yaml:/etc/envoy/envoy.yaml:ro \
    envoyproxy/envoy:v${ENVOY_VERSION} -c /etc/envoy/envoy.yaml --log-level info >/dev/null
else
  # --base-id 1: a second Envoy process on the same host must not share the default
  # shared-memory region with the harness Envoy.
  envoy -c /tmp/fp-e2e-ai-trace-bootstrap.yaml --base-id 1 --log-level info \
    > /tmp/fp-e2e-ai-trace-envoy.log 2>&1 &
  AI_TRACE_ENVOY_PID=$!
fi
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:$AI_TRACE_ADMIN_PORT/config_dump >/tmp/fp-e2e-ai-trace-dump.json 2>/dev/null || true
  grep -q "ai-$AI_TRACE_ROUTE_NAME-listener" /tmp/fp-e2e-ai-trace-dump.json && break
  sleep 1
done
grep -q "ai-$AI_TRACE_ROUTE_NAME-listener" /tmp/fp-e2e-ai-trace-dump.json || fail "AI trace listener did not converge on the trace-team Envoy"

AI_TRACE_PROMPT="fp-e2e-trace-prompt-$AI_TRACE_SFX"
AI_TRACE_REQUEST="{\"model\":\"gpt-5\",\"messages\":[{\"role\":\"user\",\"content\":\"$AI_TRACE_PROMPT\"}]}"
AI_TRACE_TRACEPARENT="00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"

# ---- AC 1 + AC 9 (inbound traceparent) + AC 6 + TTL: successful chat completion.
AI_TRACE_CODE=""
for i in $(seq 1 50); do
  AI_TRACE_CODE=$(curl -sS -o /tmp/fp-e2e-ai-trace-1.json -D /tmp/fp-e2e-ai-trace-1.hdrs -w '%{http_code}' \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
    -H "traceparent: $AI_TRACE_TRACEPARENT" --data "$AI_TRACE_REQUEST" \
    http://127.0.0.1:$AI_TRACE_GATEWAY_PORT/v1/chat/completions 2>/dev/null || true)
  [ "$AI_TRACE_CODE" = "200" ] && break
  sleep 1
done
[ "$AI_TRACE_CODE" = "200" ] || fail "AI trace route never served stub provider (last code $AI_TRACE_CODE)"
grep -q "mock-ai-trace-ok" /tmp/fp-e2e-ai-trace-1.json || fail "AI trace stub response did not reach client"
AI_TRACE_REQ_ID=$(grep -i '^x-request-id:' /tmp/fp-e2e-ai-trace-1.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
[ -n "$AI_TRACE_REQ_ID" ] || fail "AI trace response did not carry x-request-id"

# Both ExtProc streams persist at stream end — poll until the merged row is complete.
AI_TRACE_ROW=""
for i in $(seq 1 20); do
  AI_TRACE_ROW=$(psql "$PG_DB_URL" -Atc "SELECT row_to_json(t)::text FROM (SELECT * FROM ai_trace_events WHERE team_id = '$AI_TRACE_TEAM_ID' AND request_id = '$AI_TRACE_REQ_ID') t" 2>/dev/null || true)
  grep -q '"usage"' <<<"$AI_TRACE_ROW" && grep -q '"route_match"' <<<"$AI_TRACE_ROW" && break
  sleep 1
done
[ -n "$AI_TRACE_ROW" ] || fail "no ai_trace_events row for request $AI_TRACE_REQ_ID"
printf '%s' "$AI_TRACE_ROW" > /tmp/fp-e2e-ai-trace-row.json
python3 - "$AI_TRACE_PROVIDER_ID" /tmp/fp-e2e-ai-trace-row.json <<'PY' || fail "AI trace success row failed hop-timeline assertions"
import json, sys
row = json.load(open(sys.argv[2], encoding="utf-8"))
hops = row["hops"]
by_name = {h["hop"]: h for h in hops}
expected = ["route_match", "auth", "budget", "credential_injection", "upstream", "usage"]
assert sorted(by_name) == sorted(expected), f"hops {sorted(by_name)}"
assert len(hops) == len(expected), "duplicate hop entries"
assert by_name["auth"]["outcome"] == "not_configured"
assert by_name["budget"]["outcome"] == "allowed"
assert by_name["credential_injection"]["outcome"] == "injected"
assert by_name["upstream"]["detail"]["status"] == 200
assert by_name["usage"]["detail"]["total_tokens"] > 0
for h in hops:
    assert h["started_at"] <= h["ended_at"], f"hop {h['hop']} window inverted"
assert row["failure_hop"] is None
assert row["status_code"] == 200
assert row["trace_id"] == "4bf92f3577b34da6a3ce929d0e0e4736"
assert row["listener_id"] is not None
assert row["provider_id"] == sys.argv[1]
PY
# AC 9: the stub upstream received the inbound traceparent unchanged.
grep -q "$AI_TRACE_TRACEPARENT" /tmp/fp-e2e-ai-trace-upstream.jsonl || fail "inbound traceparent did not reach the stub upstream unchanged"
# AC 6: neither the prompt string nor the secret value is stored anywhere in the row.
if grep -q "$AI_TRACE_PROMPT" <<<"$AI_TRACE_ROW"; then
  fail "prompt string leaked into ai_trace_events row"
fi
if grep -q "$AI_TRACE_SECRET_VALUE" <<<"$AI_TRACE_ROW"; then
  fail "AI credential value leaked into ai_trace_events row"
fi
# Default TTL: with no ai_retention_policies row, expires_at = created_at + 30 days exactly.
AI_TRACE_TTL_OK=$(psql "$PG_DB_URL" -Atc "SELECT expires_at = created_at + interval '30 days' FROM ai_trace_events WHERE team_id = '$AI_TRACE_TEAM_ID' AND request_id = '$AI_TRACE_REQ_ID'")
[ "$AI_TRACE_TTL_OK" = "t" ] || fail "ai_trace_events expires_at is not created_at + 30 days (got $AI_TRACE_TTL_OK)"

# ---- AC 14: server-owned request identity under a reused client-supplied x-request-id.
AI_TRACE_CLIENT_ID="11111111-2222-4333-8444-555555555555"
AI_TRACE_ID_A=""
AI_TRACE_ID_B=""
for attempt in A B; do
  curl -sS -o /tmp/fp-e2e-ai-trace-dup.json -D /tmp/fp-e2e-ai-trace-dup.hdrs \
    -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" \
    -H "x-request-id: $AI_TRACE_CLIENT_ID" --data "$AI_TRACE_REQUEST" \
    http://127.0.0.1:$AI_TRACE_GATEWAY_PORT/v1/chat/completions >/dev/null
  SERVED_ID=$(grep -i '^x-request-id:' /tmp/fp-e2e-ai-trace-dup.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
  [ -n "$SERVED_ID" ] || fail "duplicate-id request $attempt returned no x-request-id"
  [ "$SERVED_ID" != "$AI_TRACE_CLIENT_ID" ] || fail "server honored the client-supplied x-request-id on AI listener (attempt $attempt)"
  if [ "$attempt" = "A" ]; then AI_TRACE_ID_A=$SERVED_ID; else AI_TRACE_ID_B=$SERVED_ID; fi
done
[ "$AI_TRACE_ID_A" != "$AI_TRACE_ID_B" ] || fail "two requests received the same server-generated x-request-id"
# Each served id must key exactly one row whose merged hop timeline carries both the
# listener-side hops (route_match/auth) and the upstream-side hops (upstream/usage) —
# the full six-hop set, same shape as the AC 1 row (plan AC 14).
for served in "$AI_TRACE_ID_A" "$AI_TRACE_ID_B"; do
  AI_TRACE_DUP_ROW=""
  for i in $(seq 1 20); do
    AI_TRACE_DUP_ROW=$(psql "$PG_DB_URL" -Atc "SELECT row_to_json(t)::text FROM (SELECT * FROM ai_trace_events WHERE team_id = '$AI_TRACE_TEAM_ID' AND request_id = '$served') t" 2>/dev/null || true)
    grep -q '"usage"' <<<"$AI_TRACE_DUP_ROW" && grep -q '"route_match"' <<<"$AI_TRACE_DUP_ROW" && break
    sleep 1
  done
  [ -n "$AI_TRACE_DUP_ROW" ] || fail "no ai_trace_events row for duplicate-client-id request $served"
  AI_TRACE_DUP_COUNT=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM ai_trace_events WHERE team_id = '$AI_TRACE_TEAM_ID' AND request_id = '$served'")
  [ "$AI_TRACE_DUP_COUNT" = "1" ] || fail "expected exactly one trace row for $served, got $AI_TRACE_DUP_COUNT"
  printf '%s' "$AI_TRACE_DUP_ROW" > /tmp/fp-e2e-ai-trace-dup-row.json
  python3 - /tmp/fp-e2e-ai-trace-dup-row.json <<'PY' || fail "duplicate-client-id row $served is missing listener-side or upstream-side hops"
import json, sys
row = json.load(open(sys.argv[1], encoding="utf-8"))
names = sorted(h["hop"] for h in row["hops"])
expected = sorted(["route_match", "auth", "budget", "credential_injection", "upstream", "usage"])
assert names == expected, f"hops {names}"
for h in row["hops"]:
    assert h["started_at"] <= h["ended_at"], f"hop {h['hop']} window inverted"
assert row["failure_hop"] is None
assert row["listener_id"] is not None
assert row["provider_id"] is not None
PY
done
AI_TRACE_CLIENT_ROWS=$(psql "$PG_DB_URL" -Atc "SELECT count(*) FROM ai_trace_events WHERE team_id = '$AI_TRACE_TEAM_ID' AND request_id = '$AI_TRACE_CLIENT_ID'")
[ "$AI_TRACE_CLIENT_ROWS" = "0" ] || fail "client-supplied x-request-id keyed a trace row ($AI_TRACE_CLIENT_ROWS rows)"

# ---- AC 9 (negative): a request without traceparent leaves trace_id NULL.
curl -sS -o /dev/null -D /tmp/fp-e2e-ai-trace-nt.hdrs \
  -H "content-type: application/json" -H "x-flowplane-ai-model: gpt-5" --data "$AI_TRACE_REQUEST" \
  http://127.0.0.1:$AI_TRACE_GATEWAY_PORT/v1/chat/completions
AI_TRACE_NT_ID=$(grep -i '^x-request-id:' /tmp/fp-e2e-ai-trace-nt.hdrs | head -1 | awk '{print $2}' | tr -d '\r')
AI_TRACE_NT_NULL=""
for i in $(seq 1 20); do
  AI_TRACE_NT_NULL=$(psql "$PG_DB_URL" -Atc "SELECT trace_id IS NULL FROM ai_trace_events WHERE team_id = '$AI_TRACE_TEAM_ID' AND request_id = '$AI_TRACE_NT_ID'")
  [ -n "$AI_TRACE_NT_NULL" ] && break
  sleep 1
done
[ "$AI_TRACE_NT_NULL" = "t" ] || fail "absent traceparent did not leave trace_id NULL (got '$AI_TRACE_NT_NULL')"

# ---- Cleanup: this phase's Envoy, stub, and team resources.
kill "$AI_TRACE_STUB_PID" >/dev/null 2>&1 || true
if [ "$ENVOY_MODE" = docker ]; then
  docker rm -f fp-e2e-envoy-trace >/dev/null 2>&1 || true
else
  kill "${AI_TRACE_ENVOY_PID:-}" >/dev/null 2>&1 || true
fi
AI_TRACE_ROUTE_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_routes WHERE id = '$AI_TRACE_ROUTE_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_TRACE_ROUTE_REV" \
  http://$API/api/v1/teams/$AI_TRACE_TEAM/ai/routes/$AI_TRACE_ROUTE_NAME >/dev/null
AI_TRACE_PROVIDER_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM ai_providers WHERE id = '$AI_TRACE_PROVIDER_ID'")
curl -fsS "${auth[@]}" -X DELETE -H "If-Match: $AI_TRACE_PROVIDER_REV" \
  http://$API/api/v1/teams/$AI_TRACE_TEAM/ai/providers/ai-e2e-trace-provider-$AI_TRACE_SFX >/dev/null
echo "PHASE 1f OK: AI trace rows -> success hop timeline (AC1) -> server-owned request identity + per-row hop shape (AC14) -> traceparent propagation + trace_id (AC9) -> sensitive-string exclusion (AC6) -> 30-day default TTL"
