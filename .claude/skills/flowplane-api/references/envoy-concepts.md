# Envoy Concepts for Flowplane

Envoy semantics that affect how Flowplane configurations behave at runtime. This is not a mirror of envoyproxy.io — it covers the concepts agents need to produce correct Flowplane configs on the first attempt.

## 1. xDS Resource Dependencies

Flowplane pushes four resource types to Envoy via xDS. They have a strict dependency order:

```
CDS (Clusters) → EDS (Endpoints) → LDS (Listeners) → RDS (Routes)
```

**Why this matters:** If a route references a cluster that hasn't been loaded yet, traffic to that route blackholes until the cluster arrives. Flowplane handles the push ordering, but the agent must **create clusters before route configs that reference them**.

**NACK behavior:**
- A NACK means Envoy rejected at least one resource in the update
- Envoy keeps the **previous valid config** — it does not apply partial updates
- A NACK on CDS blocks ALL cluster changes in that batch, not just the bad one
- The `error_detail` field contains the rejection reason (e.g., missing required field)

**Cascading NACKs:** A bad cluster config (e.g., missing health check thresholds) can NACK the entire CDS update. This blocks unrelated clusters from loading — including JWKS clusters needed by JWT filters, auth service clusters needed by ext_authz, etc. A single bad resource can break seemingly unrelated features.

## 2. Route Matching

### Evaluation order: first match wins

Routes within a virtual host are evaluated **top-to-bottom in array order**. The first matching route handles the request. There is no longest-prefix-match or most-specific-match — order in the `routes` array is the only thing that matters.

**Implication:** When adding a route exemption (e.g., disable auth for a sub-path), the more-specific route MUST come before the broader one in the array.

### Virtual host domain matching

Domains are matched against the request's `Host` or `:authority` header in this priority order:
1. Exact match (`api.example.com`)
2. Suffix wildcard (`*.example.com`)
3. Prefix wildcard (`api.*`)
4. Universal wildcard (`*`)

### Path match types

| Type | Envoy field | Behavior |
|------|-------------|----------|
| Exact | `path` | Must match the entire path exactly |
| Prefix | `prefix` | Matches if the request path starts with this value |
| Regex | `safe_regex` | RE2 regex against the full path |
| Template | `path_match_policy` (URI template) | RESTful patterns with `{param}` captures |
| Path-separated prefix | `path_separated_prefix` | Like prefix but respects `/` boundaries |

### Prefix rewrite semantics

`prefix_rewrite` performs a **literal string replacement** of the matched prefix:
```
result_path = prefix_rewrite + request_path[len(matched_prefix):]
```

If the match prefix is `/api/v1` and the rewrite is `/internal`, a request to `/api/v1/users` becomes `/internal/users`. The rewrite value replaces only the matched portion.

## 3. Per-Route Filter Overrides

### How `typed_per_filter_config` works

Filter behavior can be overridden at four levels (highest to lowest priority):

1. **Route** — applies to a specific path match
2. **Weighted cluster** — applies per-backend in weighted routing
3. **Virtual host** — applies to all routes in that domain group
4. **Route configuration** — applies to all virtual hosts

The most specific non-empty config wins. Values are NOT inherited between levels.

### Three override modes

The key in `typedPerFilterConfig` is the Envoy filter name (e.g., `envoy.filters.http.jwt_authn`). The value depends on what you want:

| Mode | Config value | Effect |
|------|-------------|--------|
| **Disable** | `{ "disabled": true }` | Filter is completely skipped for this route |
| **Override** | Filter-specific config object | Replaces the listener-level config for this route |
| **Named requirement** | `{ "requirement_name": "<name>" }` | Selects a pre-defined requirement from the filter's config (JWT-specific) |

### `disabled: true` vs filter-specific overrides

`disabled: true` uses Envoy's generic `FilterConfig` wrapper — it works for ANY filter with per-route support. It completely skips the filter.

Filter-specific overrides (like `requirement_name` for JWT) still run the filter but change its behavior. For example:
- `"requirement_name": "allow_missing"` — JWT filter runs, accepts requests without tokens, but still validates tokens that are present
- `"requirement_name": "allow_missing_or_failed"` — JWT filter runs, accepts both missing and invalid tokens
- `"disabled": true"` — JWT filter does not run at all

## 4. Filter Chain Execution

### Order matters

Filters execute in the order they appear in the listener's HTTP filter chain. The `router` filter is always last (Flowplane appends it automatically).

Typical ordering:
```
CORS → JWT Auth → Ext Auth → RBAC → Rate Limit → Header Mutation → Router
```

Auth filters should come before rate limiters — otherwise unauthenticated requests consume rate limit tokens before being rejected.

### Per-route filter support

| Filter | Per-Route | Override type |
|--------|-----------|---------------|
| JWT Auth | Yes | `disabled`, `requirement_name` |
| Local Rate Limit | Yes | Full config replacement, `disabled` |
| CORS | Yes | Full config replacement, `disabled` |
| RBAC | Yes | Full config replacement, `disabled` |
| Ext Auth | Yes | `disabled`, context extensions |
| Header Mutation | Yes | Full config replacement, `disabled` |
| Compressor | Yes | `disabled`, config replacement |
| Ext Proc | Yes | `disabled`, config replacement |
| Rate Limit | Yes | Full config replacement, `disabled` |
| OAuth2 | **No** | Configured globally only |
| Health Check | **No** | Configured globally only |
| Credential Injector | **No** | Configured globally only |

## 5. Cluster Configuration

### Service discovery types (auto-detected by Flowplane)

| Endpoints | Discovery type | Behavior |
|-----------|---------------|----------|
| All IP addresses | STATIC | Endpoints are fixed at config time |
| Single hostname | LOGICAL_DNS | DNS resolved, single connection reused |
| Multiple hostnames | STRICT_DNS | DNS resolved per host, all results used |

### Health check required fields

If you configure health checks, ALL four threshold fields are **required by Envoy** — omitting any causes a NACK:

| Field | Required | What it means |
|-------|----------|--------------|
| `timeout` | Yes | Time to wait for a check response before marking it failed |
| `interval` | Yes | Time between health check attempts |
| `unhealthy_threshold` | Yes | Consecutive failures before marking host unhealthy |
| `healthy_threshold` | Yes | Consecutive successes before marking host healthy again |

**Flowplane defaults:** timeout=5s, interval=10s, but `healthy_threshold` and `unhealthy_threshold` have **no defaults**. If health checks are configured without thresholds, Envoy NACKs the entire CDS update.

### Circuit breakers

| Field | Default | Meaning |
|-------|---------|---------|
| `max_connections` | 1024 | Max concurrent connections to cluster |
| `max_pending_requests` | 1024 | Max queued requests when all connections busy |
| `max_requests` | 1024 | Max concurrent requests (HTTP/2 multiplexing) |
| `max_retries` | 3 | Max concurrent retries |

### Connection timeout

Default connect timeout is 5 seconds. This is the TCP connection establishment timeout, not the request timeout. Request timeouts are set on routes.

### TLS auto-detection

Flowplane auto-enables TLS when the endpoint port is 443 (unless explicitly overridden). SNI is set from the first hostname endpoint.

## 6. Listener Configuration

### Address binding

Listeners bind to `address:port`. Only one listener can bind to a given port. Flowplane validates port availability via `cp_query_port`.

### Filter chain matching

When a listener has multiple filter chains, Envoy selects the chain based on:
1. Destination port
2. Server names (SNI for TLS)
3. Transport protocol (TLS vs raw TCP)
4. Application protocols (ALPN)

If no chain matches, the `default_filter_chain` handles the connection. If there's no default, the connection is closed.

### HTTP Connection Manager

The HCM is the network filter that handles HTTP traffic. It references route configs either:
- **Inline** — route config embedded in the listener (static)
- **RDS** — route config fetched dynamically by name (Flowplane's default)

Flowplane automatically sets:
- `generate_request_id: true`
- `always_set_request_id_in_response: true`
- Codec type: AUTO (handles HTTP/1.1 and HTTP/2)

## 7. JWT Authentication Specifics

### Provider configuration

Each JWT provider needs:
- `issuer` — must match the `iss` claim in the token
- `audiences` — list of accepted `aud` values
- `remote_jwks` or `local_jwks` — key source for signature validation
- `forward` — whether to pass the JWT to the upstream (usually true)

### Remote JWKS

Remote JWKS requires:
- A **cluster** that can reach the JWKS endpoint (e.g., Auth0's `/.well-known/jwks.json`)
- The cluster must be loaded via CDS before the JWT filter tries to fetch keys
- If the JWKS cluster is NACKed (e.g., by a bad unrelated cluster in the same CDS batch), the JWT filter cannot fetch keys and ALL JWT-protected requests fail with 401

### Rules and requirements

- **Rules** define which paths require JWT: `[{ "match": {"path": {"Prefix": "/api"}}, "requires": {"type": "provider_name", "provider_name": "my-provider"} }]`
- `match` wraps a `path` object — not a flat `{"prefix": ...}`. `PathMatch` variants are **capitalized**: `Prefix`, `Exact`, `Regex`, `Template`
- `requires` uses tagged enum form: `{"type": "provider_name", "provider_name": "..."}`
- A filter with **no rules** passes all traffic through unauthenticated
- A rule with `requires` but no `match` causes Envoy to NACK the entire listener update (LDS rejection) — active listener stays on old config
- Rules use **first-match** semantics (same as routes)

### Requirement map

The `requirement_map` defines named requirements that per-route config can reference:
```json
"requirement_map": {
  "allow_missing": { "allow_missing": {} },
  "strict": { "provider_name": "my-provider" }
}
```

Per-route `requirement_name` must match a key in this map.

## 8. Rate Limiting Specifics

### Local rate limit

The local rate limit filter uses an in-process token bucket. Key fields:

- `stat_prefix` — **required**, namespace for metrics
- `token_bucket` — rate limit parameters
- `filter_enabled` — fraction of requests that CHECK the limit (defaults to 0% — filter is off by default!)
- `filter_enforced` — fraction of checked requests that ENFORCE the limit

**Common mistake:** Configuring a token bucket but forgetting `filter_enabled` — the filter appears configured but never actually rate limits because both `filter_enabled` and `filter_enforced` default to 0%.

Flowplane handles this by setting appropriate defaults, but agents should be aware.

### Per-route rate limiting

A route-level `typedPerFilterConfig` replaces the entire rate limit config for that route. To disable: use `{ "disabled": true }`.

## 9. Common NACK Causes

Things that cause Envoy to reject a config update:

| Cause | Resource type | Error pattern |
|-------|--------------|---------------|
| Missing health check thresholds | CDS | `unhealthy_threshold` / `healthy_threshold` required |
| Duplicate resource names in batch | Any | "duplicate resource name" |
| Invalid regex syntax | RDS | RE2 parse error |
| Referencing non-existent requirement_name | RDS | requirement not found in map |
| Missing required fields in proto | Any | field validation error |
| Invalid enum values | Any | unknown enum value |
| Conflicting filter chain matchers | LDS | overlapping match criteria |

Use `ops_nack_history` and `ops_xds_delivery_status` to diagnose NACK issues.

## 10. Eventual Consistency

Envoy's xDS model is eventually consistent. During config updates:
- New routes may reference clusters that haven't loaded yet (traffic blackholes)
- Old routes may still reference clusters that are being removed (works until removed)
- Filter changes take effect on new connections, not existing ones

The safe update order is: **add new clusters first, then update routes, then remove old clusters last** (make-before-break).
