# Trace an AI request through the gateway

> Audience: api-teams, operators · Status: stable

Every request that reaches an AI listener leaves a **trace row**: a per-hop
timeline of what the gateway did (route match, auth, budget check, credential
injection, upstream call, usage accounting), with an outcome and timing for each
hop. This guide shows how to retrieve that timeline to answer "where did my AI
request die, and why" — without reading Envoy logs — and how to control how long
trace rows are kept.

Prerequisites: a working AI provider + route (see
[Create an AI provider, route traffic to it, and attach a token budget](./ai-gateway-route-budget.md))
and a token with the `ai-usage` read grant for the team. Command surface details
are in the [CLI reference](../reference/cli.md); endpoint shapes are in the
[REST API reference](../reference/rest-api.md).

## 1. Send a request and capture its request id

Send a chat completion through your AI listener and keep the response headers:

```bash
curl -sS -D /tmp/ai-headers.txt \
  -H 'content-type: application/json' \
  -H 'x-flowplane-ai-model: gpt-4o-mini' \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}' \
  http://127.0.0.1:19000/v1/chat/completions

grep -i '^x-request-id:' /tmp/ai-headers.txt
```

Expected result: the completion body, and a header like

```text
x-request-id: 019f2f5a-9ae3-76d1-bd61-dcc24cfe6e6c
```

That id is the key to the trace row.

> **AI listeners always generate their own `x-request-id`.** A client-supplied
> `x-request-id` header is ignored on AI listeners (unlike the control-plane
> REST API): the server generates a fresh id, returns it in the response, and
> only that server id keys a trace. If you send your own id, it will not find
> a trace — use the id from the *response*.

## 2. Retrieve the hop timeline

```bash
flowplane ai trace --request-id 019f2f5a-9ae3-76d1-bd61-dcc24cfe6e6c --json
```

Expected output (a successful request; timestamps trimmed):

```json
{
  "data": {
    "traces": [
      {
        "request_id": "019f2f5a-9ae3-76d1-bd61-dcc24cfe6e6c",
        "status_code": 200,
        "failure_hop": null,
        "model": "gpt-4o-mini",
        "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
        "hops": [
          { "hop": "route_match", "origin": "listener", "outcome": "matched",
            "detail": { "model": "gpt-4o-mini", "listener_id": "…", "route_config_id": "…" } },
          { "hop": "auth", "origin": "listener", "outcome": "not_configured", "detail": {} },
          { "hop": "budget", "origin": "upstream", "outcome": "allowed",
            "detail": { "mode": "enforcing", "verdict": "allowed" } },
          { "hop": "credential_injection", "origin": "upstream", "outcome": "injected",
            "detail": { "provider_id": "…", "auth_header": "authorization" } },
          { "hop": "upstream", "origin": "upstream", "outcome": "ok",
            "detail": { "status": 200, "latency_ms": 8, "provider_id": "…" } },
          { "hop": "usage", "origin": "upstream", "outcome": "settled",
            "detail": { "prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5 } }
        ],
        "created_at": "2026-07-04T22:58:14.883109Z",
        "expires_at": "2026-08-03T22:58:14.883109Z"
      }
    ]
  },
  "kind": "aiTrace",
  "schemaVersion": 1
}
```

Each hop carries `started_at`/`ended_at` (elided above), so slow requests show
*where* the time went — `upstream.detail.latency_ms` is the provider round trip.

Trace rows never contain prompt or completion text, request/response bodies, or
credential values; the schema stores outcomes, ids, timings, and token counts
only.

## 3. Correlate with your own distributed trace

If your client sends a W3C `traceparent` header, the AI listener forwards it to
the provider unchanged and stores its `trace_id` on the row. Look traces up by
that id to join the gateway's view to your APM's view of the same request:

```bash
flowplane ai trace --trace-id 4bf92f3577b34da6a3ce929d0e0e4736 --json
```

Requests without a `traceparent` store `trace_id: null` — they are only
findable by `--request-id`.

## 4. Triage a failed request

On failure, `failure_hop` names the first hop that failed and its outcome tells
you the class:

| `failure_hop` | hop outcome | client saw | meaning |
|---|---|---|---|
| `route_match` | `no_eligible_backend` | 400 `no_eligible_ai_backend` | the request's model matches no backend on the route — check the route's `backends[].models` and the `x-flowplane-ai-model` header |
| `budget` | verdict `rejected` | 429 `flowplane_ai_budget_exceeded` | an enforcing budget is exhausted — raise the budget or wait for the window to reset |
| `credential_injection` | `secret_missing` or `decrypt_failed` | "AI provider credential unavailable" | the provider's credential secret is missing, expired, or undecryptable — rotate the secret referenced by the provider |
| `upstream` | provider status (e.g. `500`) | provider error passthrough | the provider answered with an error — `detail.status` has the code |
| `upstream` | `no_upstream_connection` | 503 | Envoy could not connect to the provider — check `base_url` and network reachability |
| `upstream` | `client_disconnect` | — | the client dropped mid-stream; the partial row is still persisted |

A **shadow** budget that would have rejected shows up on *successful* requests:
the `budget` hop records verdict `would_reject` while the request still returns
2xx — use it to preview an enforcement change before flipping the budget mode.

## 5. When there is no trace

A lookup that finds nothing returns an explicit miss, not an empty list:

```json
{
  "data": {
    "miss": {
      "message": "no trace row found",
      "hint": "no trace row exists for this id; requests that never complete HTTP processing at the AI listener are never traced: TCP/TLS-level failures, client disconnect before request headers, and pre-ExtProc declared-filter denials"
    },
    "traces": []
  },
  "kind": "aiTrace",
  "schemaVersion": 1
}
```

Two common causes: you queried the *client-supplied* request id instead of the
server-returned one (step 1), or the request failed before HTTP processing
completed at the listener (the classes the hint names).

## Control how long traces are kept

Trace rows expire. With no team policy, rows live **30 days**:

```bash
flowplane ai retention get --json
```

```json
{
  "data": { "is_default": true, "trace_ttl_days": 30 },
  "kind": "aiRetention",
  "schemaVersion": 1
}
```

Set a team TTL (1–365 days):

```bash
flowplane ai retention set --days 7 --json
```

```json
{
  "data": { "is_default": false, "revision": 1, "trace_ttl_days": 7,
            "updated_at": "2026-07-05T11:06:28.118358Z" },
  "kind": "aiRetention",
  "schemaVersion": 1
}
```

The TTL is stamped at capture time: changing it affects only traces recorded
*after* the change; existing rows keep the expiry they were written with. A
control-plane background sweep deletes expired rows hourly. Setting the policy
requires the `ai-usage` update grant.

## Export the timeline as OTLP spans

When the control plane runs with `FLOWPLANE_OTLP_ENDPOINT` set, every hop is
also emitted as a tracing span (nested under a per-request span) to your OTLP
collector. Export is best-effort — the trace row above remains the primary
record and is unaffected by collector availability. See the
[configuration reference](../reference/configuration.md).

## Further reading

- [CLI reference — `ai trace`, `ai retention`](../reference/cli.md)
- [REST API reference — request correlation and the `ai/trace`, `ai/retention` endpoints](../reference/rest-api.md)
- [Observability alert pack — `fp_ai_trace_dropped_total`](../reference/observability-alerts.md)
