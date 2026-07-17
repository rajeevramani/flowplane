# Trace an AI request through the gateway

> Audience: api-teams, operators · Status: stable

Every request that reaches an AI listener leaves a **trace row**: one merged
record of the ExtProc stream contributions for the request (route match, auth,
budget check, credential injection, upstream call, usage accounting), with an
outcome and timing window for each hop. This guide shows how to retrieve that
row to answer "where did my AI request die, and why" — without reading Envoy
logs — and how to control how long trace rows are kept.

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

## 2. Retrieve the hop record

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
            "detail": {
              "mode": "enforcing",
              "verdict": "allowed",
              "shadow": [
                {
                  "budget": "preview-team-monthly",
                  "verdict": "would_reject",
                  "used_units": 100000,
                  "limit_units": 100000
                }
              ]
            } },
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

The `hops` array is a merged row view of one or more ExtProc stream
contributions, not the execution-order contract for the request. Do not infer a
single sequential waterfall from array position. `origin` is the stream-context
label attached to the hop, such as `listener` or `upstream`; it is not a
physical-side guarantee and it does not by itself prove the order in which gates
executed.

Each hop carries `started_at`/`ended_at` (elided above). These timestamps are
per-hop windows from the stream that pushed the hop, so windows may overlap or
appear back-anchored. In the successful sample above, the surviving `budget`,
`credential_injection`, `upstream`, and `usage` hops all have
`origin: "upstream"` because they are upstream-labeled stream contributions
after merge. The `upstream` hop can start before or overlap the same-origin
`budget` and `credential_injection` hops because it is anchored to that stream's
request-header window, while the budget and credential windows start when those
checks run later in the stream.

That timestamp shape does not mean the checks were skipped or bypassed.
Enforcing budget rejection and credential failure short-circuit before upstream
forwarding; failed traces for those cases do not produce an upstream hop.
`upstream.detail.latency_ms` remains the upstream-hop latency signal, but it is
measured from the upstream-labeled stream window rather than promised as pure
provider-only time.

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
| `credential_injection` | `scheme_conflict` | "AI provider credential unavailable" | the provider sets `auth_scheme` but the decoded secret already starts with that scheme (`Bearer Bearer <key>` misconfiguration) — store the bare key in the secret, or drop the provider's `auth_scheme` |
| `upstream` | provider status (e.g. `500`) | provider error passthrough | the provider answered with an error — `detail.status` has the code |
| `upstream` | `no_upstream_connection` | 503 | Envoy could not connect to the provider — check `base_url` and network reachability |
| `upstream` | `client_disconnect` | — | the client dropped mid-stream; the partial row is still persisted |

A **shadow** budget preview that would have rejected shows up on *successful*
requests under `budget.detail.shadow[]`. The budget hop's top-level
`detail.mode` and `detail.verdict` describe the enforcing gate for the request,
so a successful request can keep top-level `detail.verdict: "allowed"` while a
shadow entry reports `detail.shadow[].verdict: "would_reject"`. Use shadow
entries to preview an enforcement change before flipping the budget mode.

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
