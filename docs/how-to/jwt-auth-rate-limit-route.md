> Audience: platform-engineers · Status: stable

# Add JWT authentication and a local rate limit to an existing route

This protects one route with JWT validation and caps its request rate. It assumes you already have a working cluster, listener, and route-config bound together. If you do not, start with the [getting-started tutorial](../tutorials/getting-started.md).

**Prerequisites:** a listener (e.g. `edge`) whose `route_config` points at an existing route-config (e.g. `api-routes`) with the route you want to protect, and the listener/route revisions to hand (the `revision` field on a `GET`).

## Where each filter lives

Both filters **must be present in the listener filter chain** to be active — a per-route/vhost `filter_overrides` entry only emits per-filter *config* that Envoy applies to that scope; it cannot add a filter Envoy isn't already running. Within that rule, the two filters differ in how the config is split:

- **`jwt_auth`** — the providers and `requirement_map` (and any `rules`) live **only** in the **listener filter chain** (`HttpFilterEntry`). The per-route/vhost override is **reference-only**: it names a requirement already defined in the chain's `requirement_map` (`{ "type": "jwt_auth", "requirement_name": "<name>" }`). You cannot redefine providers per-route.
- **`local_rate_limit`** — the chain entry is **required** (it makes the filter run); the **per-route/vhost override carries a full `LocalRateLimitConfig`** that replaces the chain entry's config for that scope. The chain entry is the base/default limit.

Each `type` may appear at most once in the listener chain.

## 1. Add the filters to the listener chain

Add both a `jwt_auth` entry and a `local_rate_limit` entry to the listener's `http_filters`. Both are required for the per-route overrides in step 2 to take effect: the chain entry is what makes Envoy run the filter, and the override only supplies per-scope config. The `local_rate_limit` chain entry below acts as the base/default limit; the route override in step 2 replaces it for one route. The entry shape is `HttpFilterEntry`: `{ "filter": <HttpFilterSpec>, "disabled": <bool> }`, where the filter is tagged by `type`.

```jsonc
{
  "spec": {
    "address": "0.0.0.0",
    "port": 8080,
    "route_config": "api-routes",
    "http_filters": [
      {
        "filter": {
          "type": "jwt_auth",
          "providers": {
            "auth0": {
              "issuer": "https://issuer.example",
              "audiences": ["api"],
              "jwks": {
                "source": "remote",
                "uri": "https://issuer.example/.well-known/jwks.json",
                "cluster": "jwks-cluster",
                "timeout_ms": 5000
              }
            }
          },
          "requirement_map": {
            "require-auth0": { "kind": "provider", "provider_name": "auth0" }
          }
        }
      },
      {
        "filter": {
          "type": "local_rate_limit",
          "stat_prefix": "edge_default",
          "token_bucket": { "max_tokens": 100, "fill_interval_ms": 1000 }
        }
      }
    ]
  }
}
```

Notes on the fields used above (full field list in [`../reference/filters.md`](../reference/filters.md)):

- `jwt_auth.providers` is a map of provider name → provider; at least one is required. `jwks.source` is `remote` (needs `uri` + a same-team `cluster`) or `inline` (`{ "source": "inline", "jwks": "<JWKS JSON>" }`).
- `requirement_map` names requirements (`kind`: `provider`, `any_of`, `allow_missing`, `allow_missing_or_failed`) that `rules` and per-route overrides reference by name. Defining a requirement here enforces **nothing** on its own — it is just a lookup table.
- **To protect only one route, leave `rules` empty** and reference the requirement from that route's `filter_overrides` (step 2). With empty `rules` and no per-route reference, Envoy enforces no JWT requirement; the per-route `jwt_auth` override is what attaches `require-auth0` to the `payments` route, leaving every other route unauthenticated.
- Do **not** use chain-level `rules` for this task: `rules` are listener-wide path matches, so they would force auth on every path they match, not just your one route.
- `local_rate_limit` requires `stat_prefix` and `token_bucket` (`max_tokens` ≥ 1, `fill_interval_ms` > 0; `tokens_per_fill` defaults to `max_tokens`). `status_code` is optional (400–599; Envoy defaults to 429).

## 2. Attach the per-route override

Edit the route-config so the target route demands the JWT requirement and gets its own rate limit. Both go in the route's `filter_overrides` (a `RouteRule` field; `VirtualHost` has the same field, and a route-level override wins over the vhost's).

```jsonc
{
  "spec": {
    "virtual_hosts": [
      {
        "name": "default",
        "domains": ["*"],
        "routes": [
          {
            "name": "payments",
            "match": { "prefix": { "prefix": "/payments" } },
            "action": { "cluster": "payments-backend" },
            "filter_overrides": [
              { "type": "jwt_auth", "requirement_name": "require-auth0" },
              {
                "type": "local_rate_limit",
                "stat_prefix": "payments_route",
                "token_bucket": { "max_tokens": 10, "fill_interval_ms": 1000 }
              }
            ]
          }
        ]
      }
    ]
  }
}
```

- `requirement_name` must match a key in the chain filter's `requirement_map` (here `require-auth0`); 1–128 characters.
- At most one override per filter `type` in a scope.
- To turn a chain filter **off** for a scope instead, use `{ "type": "disable", "filter_type": "jwt_auth" }`.

## 3. Apply the updates

Updates are optimistic-concurrency-checked: send the current revision as a plain integer in the `If-Match` header. Read it from a `GET` (the `revision` field).

### REST

```bash
# Listener
curl -X PATCH https://control-plane.example/api/v1/teams/<team>/listeners/edge \
  -H "Authorization: Bearer $TOKEN" \
  -H "If-Match: <listener-revision>" \
  -H "Content-Type: application/json" \
  --data @listener.json

# Route-config
curl -X PATCH https://control-plane.example/api/v1/teams/<team>/route-configs/api-routes \
  -H "Authorization: Bearer $TOKEN" \
  -H "If-Match: <route-config-revision>" \
  -H "Content-Type: application/json" \
  --data @route-config.json
```

The PATCH body is `{ "spec": { … } }` exactly as in the JSON above. (Creating fresh resources is a `POST` to the collection with `{ "name": "...", "spec": { … } }`.)

### CLI

The CLI sends `If-Match` from the global `--revision` flag, and the file is the same `{ "spec": { … } }` body.

```bash
flowplane listener update edge --team <team> --revision <listener-revision> --file listener.json
flowplane route update api-routes --team <team> --revision <route-config-revision> --file route-config.json
```

## 4. Verify

```bash
# No token → 401
curl -i https://api.example.com/payments
# Valid token, but over 10 req/s → 429
for i in $(seq 1 20); do
  curl -s -o /dev/null -w "%{http_code}\n" \
    -H "Authorization: Bearer $JWT" https://api.example.com/payments
done
```

Expect `401` without a valid token and `429` once the bucket is empty. The `429` here is Envoy's data-plane `local_rate_limit` response; it does **not** carry a `Retry-After` header (the translator does not set one). For HTTP status-code meanings see [`../reference/errors.md`](../reference/errors.md) — note its `rate_limited` (429 with `Retry-After`) row describes the control-plane API write throttle, not this data-plane rate limit.

## See also

- [`../reference/filters.md`](../reference/filters.md) — full `JwtAuthConfig`, `LocalRateLimitConfig`, and `FilterOverride` field reference.
