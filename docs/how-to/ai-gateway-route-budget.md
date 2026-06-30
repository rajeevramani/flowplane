# Create an AI provider, route traffic to it, and attach a token budget

> Audience: api-teams · Status: stable

This guide stands up a working AI gateway path: register an upstream LLM provider, publish an AI route that points a listener at it, and cap token spend with a budget — first in `shadow` (observe-only), then `enforcing`. It uses the `flowplane` CLI, whose AI resources (`ai providers`, `ai routes`, `ai budgets`) all take a JSON spec file and POST it to the team's `ai/*` endpoints.

For the full command surface and flags, see [`../reference/cli.md`](../reference/cli.md).

## Prereqs

The CLI is authenticated and scoped to your team (see [`cli-auth-and-contexts.md`](./cli-auth-and-contexts.md)).

The control plane must be running with `FLOWPLANE_SECRET_ENCRYPTION_KEY` set (a 32-byte raw or base64 key). Secrets are encrypted at rest, so `flowplane secret create` fails with `error (unavailable): secret encryption key is not configured` if the server was started without it — the dev getting-started setup omits this key, so set it before this step (see [`../reference/configuration.md`](../reference/configuration.md), constraint ⁷).

With that key set, create a secret holding the provider API key — `flowplane secret create --file secret.json` — and note its UUID for `credential_secret_id` below.

Request verification in step 4 needs a connected dataplane that can receive the materialized listener over xDS. The `flowplane ai routes create` command creates the cluster, route config, and listener in the control plane; it does not start Envoy. For a production-shaped setup, use a dataplane registered over mTLS by the platform team (see [Register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md) and [Evaluate a production-shaped platform setup](evaluate-platform.md)).

`secret.json` (the `secret` value is **standard base64** of the raw provider API key — produce it with `printf %s "$API_KEY" | base64`):

```json
{
  "name": "openai-key",
  "spec": {
    "type": "generic_secret",
    "secret": "<base64 of your provider API key>"
  }
}
```

## 1. Create a provider

The provider body is `{ "name", "spec" }`, where `spec` is the `AiProviderSpec`: a `kind` (`openai` or `openai-compatible`), an **origin-only** `base_url` (scheme + host, no path — use `path_prefix` for upstream paths), the `credential_secret_id` of your secret, the `models` the provider serves, and the `auth_header` the key is sent in (defaults to `authorization`).

`provider.json`:

```json
{
  "name": "openai-prod",
  "spec": {
    "kind": "openai",
    "base_url": "https://api.openai.com",
    "credential_secret_id": "00000000-0000-0000-0000-0000000000aa",
    "models": ["gpt-4o-mini", "gpt-4o"],
    "auth_header": "authorization"
  }
}
```

```bash
flowplane ai providers create --file provider.json
```

This POSTs to `/api/v1/teams/{team}/ai/providers` and returns the provider view, including its `id` — note it for the route backend.

## 2. Create a route to the provider

The route body is `{ "name", "spec" }`, where `spec` is the `AiRouteSpec`: a `listener_port`, a `path` (defaults to `/v1/chat/completions`, currently the only supported path), and one or more `backends`. Each `AiRouteBackend` references a `provider_id`, optionally restricts which `models` it serves (empty = catch-all), and carries a `weight` (1–1000, default 1) and `priority` (default 0). Use `model_override` to rewrite the client's `model` to a specific upstream model.

`route.json`:

```json
{
  "name": "chat-route",
  "spec": {
    "listener_port": 19000,
    "path": "/v1/chat/completions",
    "backends": [
      {
        "provider_id": "<provider-id-from-step-1>",
        "models": ["gpt-4o-mini", "gpt-4o"],
        "weight": 1,
        "priority": 0
      }
    ]
  }
}
```

```bash
flowplane ai routes create --file route.json
```

This POSTs to `/api/v1/teams/{team}/ai/routes`. The returned view's `materialized` field shows the cluster/route-config/listener names Flowplane generated, and `status` should be `active`.

## 3. Attach a budget (shadow first)

The budget body is `{ "name", "spec" }`, where `spec` is the `AiBudgetSpec`. Start in `shadow` mode so the budget records usage without blocking traffic. `limit_units` is the cap, `window_seconds` is the rolling window (defaults to ~30 days), and the per-token weights convert token counts into budget units. Note the defaults differ: `prompt_token_weight` defaults to **0** (prompt tokens are not counted unless you set it), while `completion_token_weight` defaults to **1**. The example below sets `prompt_token_weight` to `1` explicitly so prompt tokens also count toward the budget. Scope the budget to this provider with `provider_id` (or to a route config with `route_config_id`).

`budget-shadow.json`:

```json
{
  "name": "chat-budget",
  "spec": {
    "mode": "shadow",
    "limit_units": 1000000,
    "window_seconds": 86400,
    "provider_id": "<provider-id-from-step-1>",
    "prompt_token_weight": 1,
    "completion_token_weight": 1
  }
}
```

```bash
flowplane ai budgets create --file budget-shadow.json
```

### Flip to enforcing

Once the shadow run looks right, switch the same budget to `enforcing` so requests are rejected when the limit is exceeded. Updates are a PATCH carrying only the `spec` (no `name`), and require the current revision via `If-Match` — the CLI sends it from the global `--revision` flag. Read the current revision first (e.g. with `flowplane ai budgets get chat-budget`, whose `revision` field you pass below).

`budget-enforce.json`:

```json
{
  "spec": {
    "mode": "enforcing",
    "limit_units": 1000000,
    "window_seconds": 86400,
    "provider_id": "<provider-id-from-step-1>",
    "prompt_token_weight": 1,
    "completion_token_weight": 1
  }
}
```

```bash
flowplane ai budgets update chat-budget --file budget-enforce.json --revision <n>
```

## 4. Verify

Before sending traffic, verify that a dataplane is connected and Envoy accepted the generated xDS resources:

```bash
flowplane stats overview --team <team>
flowplane ops xds status --team <team>
flowplane ops xds nacks --team <team>
```

Run the `curl` from a host that can reach the dataplane listener. In the local
example below, that listener is reachable at `127.0.0.1:19000`; in a platform
evaluation, use the listener address the platform team provides. If you are
using the published eval bundle, only its default demo listener is published to
the host by default; call the AI listener from inside the dataplane/compose
network, or publish the listener port in your local dataplane runtime before
using `127.0.0.1:19000`.

The request body must include a `model`; Flowplane routes it to an eligible
backend and forwards to the provider through Envoy:

```bash
curl http://127.0.0.1:19000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}'
```

Then confirm usage was recorded:

```bash
flowplane ai usage --provider-id <provider-id-from-step-1>
```

This GETs `/api/v1/teams/{team}/ai/usage` and returns `AiUsageSummary` rows with `prompt_tokens`, `completion_tokens`, `total_tokens`, and `event_count`. A non-zero `event_count` confirms the request routed to the provider and was accounted against the budget. You can also filter by `--route-config-id`.
