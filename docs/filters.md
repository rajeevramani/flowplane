# Filters

Add rate limiting, authentication, CORS, header manipulation, and more to your gateway. Filters are Envoy HTTP filter configurations managed through Flowplane — define them once and attach them to listeners or route configs.

For initial setup, see [Getting Started](getting-started.md). For CLI flag details, see [CLI Reference](cli-reference.md).

## How Filters Work

1. **Create** a filter definition with a type-specific configuration
2. **Attach** the filter to a listener (applies to all traffic on that port) or a route config (applies to routes within that config)
3. **Verify** the attachment took effect

Filters execute in order within the listener's filter chain. The Router filter is always auto-appended by Flowplane as the final filter — never add it manually.

## Filter Types

| Filter | Type Key | Envoy Filter | Per-Route | Description |
|--------|----------|-------------|-----------|-------------|
| Local Rate Limit | `local_rate_limit` | `envoy.filters.http.local_ratelimit` | Yes | Per-instance token bucket rate limiting |
| Rate Limit | `rate_limit` | `envoy.filters.http.ratelimit` | Yes | Distributed rate limiting via external service |
| Rate Limit Quota | `rate_limit_quota` | `envoy.filters.http.rate_limit_quota` | Yes | Quota-based rate limiting (RLQS) |
| JWT Auth | `jwt_auth` | `envoy.filters.http.jwt_authn` | Yes | Validate JWTs against JWKS endpoints |
| OAuth2 | `oauth2` | `envoy.filters.http.oauth2` | No | OAuth 2.0 authorization code flow |
| Ext Auth | `ext_authz` | `envoy.filters.http.ext_authz` | Yes | Delegate auth to external service |
| RBAC | `rbac` | `envoy.filters.http.rbac` | Yes | Role-based access control policies |
| CORS | `cors` | `envoy.filters.http.cors` | Yes | Cross-origin resource sharing |
| Header Mutation | `header_mutation` | `envoy.filters.http.header_mutation` | Yes | Add, remove, or modify headers |
| Custom Response | `custom_response` | `envoy.filters.http.custom_response` | Yes | Override responses for specific status codes |
| Health Check | `health_check` | `envoy.filters.http.health_check` | No | Respond to health probes at the proxy |
| Credential Injector | `credential_injector` | `envoy.filters.http.credential_injector` | No | Inject credentials into upstream requests via SDS |
| Ext Proc | `ext_proc` | `envoy.filters.http.ext_proc` | No | External request/response processing via gRPC |
| Compressor | `compressor` | `envoy.filters.http.compressor` | Yes | Compress responses (gzip, brotli) |

**Per-Route** indicates whether the filter supports per-route overrides via `typedPerFilterConfig` (see [Per-Route Overrides](#per-route-overrides)).

## Config Structure

All filter JSON uses a nested `config.type` + `config.config` structure:

```json
{
  "name": "my-filter",
  "filterType": "<type_key>",
  "config": {
    "type": "<type_key>",
    "config": {
      ... type-specific fields ...
    }
  }
}
```

The inner `type` must match the `filterType` field. This structure applies to CLI JSON files (`-f filter.json`). For MCP, the argument name is `configuration` instead of `config` — see the MCP column in the examples below.

## Workflow: Create, Attach, Verify

<table>
<tr><th>CLI</th><th>MCP</th></tr>
<tr>
<td>

```bash
# 1. Create
flowplane filter create -f filter.json

# 2. Attach to a listener
flowplane filter attach <name> \
  --listener <listener> --order <n>

# 3. Verify
flowplane filter get <name>
# Check listenerInstallations in output
```

</td>
<td>

```json
// 1. Create
{"method": "tools/call", "params": {
  "name": "cp_create_filter",
  "arguments": {
    "name": "my-filter",
    "filterType": "local_rate_limit",
    "configuration": {
      "type": "local_rate_limit",
      "config": { ... }
    },
    "team": "default"
  }
}}

// 2. Attach
{"method": "tools/call", "params": {
  "name": "cp_attach_filter",
  "arguments": {
    "filter": "my-filter",
    "listener": "my-listener",
    "order": 10,
    "team": "default"
  }
}}

// 3. Verify
{"method": "tools/call", "params": {
  "name": "cp_get_filter",
  "arguments": {
    "name": "my-filter",
    "team": "default"
  }
}}
```

</td>
</tr>
</table>

> ⚠️ **MCP in dev mode:** Always include `"team": "default"` in every tool call.

> ⚠️ **MCP argument name:** The MCP tool `cp_create_filter` uses `configuration` (not `config`) as the argument key. The CLI JSON file uses `config`.

## Attachment Levels

### Listener-level

Applies to **all traffic** on that listener's port.

```bash
flowplane filter attach my-filter --listener my-listener --order 10
```

### Route-config-level

Applies to routes within a specific route configuration. Available via MCP only:

```json
{"method": "tools/call", "params": {
  "name": "cp_attach_filter",
  "arguments": {
    "filter": "my-filter",
    "route_config": "my-route-config",
    "team": "default"
  }
}}
```

Use listener-level for policies that should apply to everything on a port (rate limiting, CORS). Use route-config-level to scope a filter to specific routes.

## Execution Order

The `--order` value controls where the filter sits in the chain. Lower numbers execute first.

| Order | Filter | Reason |
|-------|--------|--------|
| 1 | CORS | Handle preflight before auth |
| 2 | JWT Auth / Ext Auth | Validate identity early |
| 3 | RBAC | Enforce access policies |
| 5 | Rate Limit | Don't waste tokens on unauthenticated requests |
| 10 | Header Mutation | Modify headers for upstream |
| — | Router | Auto-appended by Flowplane |

> ⚠️ Order values must be unique per listener. Use gaps (1, 5, 10, 20) so you can insert filters later without reordering.

## Per-Route Overrides

Filters marked **Per-Route: Yes** can be customized or disabled on individual routes or virtual hosts using `typedPerFilterConfig`. The key is the **Envoy filter name** (not the Flowplane type key).

### Override config for a route

Add `typedPerFilterConfig` to a route in your route config JSON:

```json
{
  "name": "api-route",
  "match": { "path": { "type": "prefix", "value": "/api" } },
  "action": { "type": "forward", "cluster": "my-backend" },
  "typedPerFilterConfig": {
    "envoy.filters.http.local_ratelimit": {
      "stat_prefix": "api_rl",
      "token_bucket": {
        "max_tokens": 50,
        "tokens_per_fill": 50,
        "fill_interval_ms": 1000
      }
    }
  }
}
```

This replaces the listener-level rate limit for requests matching `/api`.

### Disable a filter for a route

To skip a filter entirely on a specific route:

```json
{
  "name": "health-route",
  "match": { "path": { "type": "exact", "value": "/healthz" } },
  "action": { "type": "forward", "cluster": "my-backend" },
  "typedPerFilterConfig": {
    "envoy.filters.http.jwt_authn": {
      "disabled": true
    }
  }
}
```

> ⚠️ **`disabled: true` vs `allow_missing`**: `disabled: true` skips the filter entirely — it never executes. `"requirement_name": "allow_missing"` still runs the JWT filter but accepts requests without tokens. Use `disabled` for fully public endpoints; use `allow_missing` for endpoints that optionally accept tokens.

### Exempt a sub-path

To protect a broad prefix but exempt a specific sub-path, add a more-specific route **before** the broader one. Envoy uses first-match routing — order in the `routes` array matters.

```json
{
  "name": "my-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [
      {
        "name": "health-exempt",
        "match": { "path": { "type": "exact", "value": "/healthz" } },
        "action": { "type": "forward", "cluster": "my-backend" },
        "typedPerFilterConfig": {
          "envoy.filters.http.jwt_authn": { "disabled": true }
        }
      },
      {
        "name": "api-protected",
        "match": { "path": { "type": "prefix", "value": "/" } },
        "action": { "type": "forward", "cluster": "my-backend" }
      }
    ]
  }]
}
```

> ⚠️ `flowplane route update` performs a **full replacement** of the `virtualHosts` array. Always fetch the existing config with `flowplane route get` first, then include all routes plus your changes.

### REST API for per-route overrides

Override or disable a filter on a route via the REST API:

```
POST /api/v1/filters/{filterId}/configurations
```

**Override:**
```json
{
  "scopeType": "route",
  "scopeId": "route-config-name/vhost-name/route-name",
  "settings": {
    "behavior": "override",
    "config": { ... filter-specific config ... }
  }
}
```

**Disable:**
```json
{
  "scopeType": "route",
  "scopeId": "route-config-name/vhost-name/route-name",
  "settings": {
    "behavior": "disable"
  }
}
```

The `scopeId` format for routes is `{route-config-name}/{vhost-name}/{route-name}`. For route-config-level scope, use `"scopeType": "route-config"` and `"scopeId": "my-route-config"`.

---

## Example 1: Local Rate Limiting

Limit httpbin to 3 requests per minute.

<table>
<tr><th>CLI</th><th>MCP</th></tr>
<tr>
<td>

**filter.json:**

```json
{
  "name": "httpbin-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "httpbin_rl",
      "token_bucket": {
        "max_tokens": 3,
        "tokens_per_fill": 3,
        "fill_interval_ms": 60000
      }
    }
  }
}
```

```bash
flowplane filter create -f filter.json
flowplane filter attach httpbin-rate-limit \
  --listener httpbin-listener --order 1
```

</td>
<td>

```json
// Create
{"method": "tools/call", "params": {
  "name": "cp_create_filter",
  "arguments": {
    "name": "httpbin-rate-limit",
    "filterType": "local_rate_limit",
    "configuration": {
      "type": "local_rate_limit",
      "config": {
        "stat_prefix": "httpbin_rl",
        "token_bucket": {
          "max_tokens": 3,
          "tokens_per_fill": 3,
          "fill_interval_ms": 60000
        }
      }
    },
    "team": "default"
  }
}}

// Attach
{"method": "tools/call", "params": {
  "name": "cp_attach_filter",
  "arguments": {
    "filter": "httpbin-rate-limit",
    "listener": "httpbin-listener",
    "order": 1,
    "team": "default"
  }
}}
```

</td>
</tr>
</table>

**Test:**

```bash
for i in 1 2 3 4 5; do
  echo "Request $i: $(curl -s -o /dev/null -w '%{http_code}' http://localhost:10001/get)"
done
```

```
Request 1: 200
Request 2: 200
Request 3: 200
Request 4: 429
Request 5: 429
```

The token bucket allows 3 requests per 60-second window. Requests 4 and 5 get `429 Too Many Requests`.

**Token bucket fields:**

| Field | Description |
|---|---|
| `max_tokens` | Maximum tokens in the bucket |
| `tokens_per_fill` | Tokens added per fill interval |
| `fill_interval_ms` | Refill interval in milliseconds |
| `stat_prefix` | Required. Namespace for rate limit metrics |

**Clean up:**

```bash
flowplane filter detach httpbin-rate-limit --listener httpbin-listener
flowplane filter delete httpbin-rate-limit --yes
```

---

## Example 2: JWT Authentication

Protect API routes with JWT validation. Requests without a valid token are rejected with 401.

<table>
<tr><th>CLI</th><th>MCP</th></tr>
<tr>
<td>

**jwt-filter.json:**

```json
{
  "name": "api-jwt-auth",
  "filterType": "jwt_auth",
  "config": {
    "type": "jwt_auth",
    "config": {
      "providers": {
        "my-provider": {
          "issuer": "https://auth.example.com",
          "audiences": ["my-api"],
          "jwks": {
            "type": "remote",
            "http_uri": {
              "uri": "https://auth.example.com/.well-known/jwks.json",
              "cluster": "auth-jwks-cluster",
              "timeout_ms": 5000
            },
            "cache_duration_seconds": 300
          },
          "fromHeaders": [
            { "name": "Authorization", "value_prefix": "Bearer " }
          ],
          "forward": true
        }
      },
      "bypass_cors_preflight": true
    }
  }
}
```

```bash
flowplane filter create -f jwt-filter.json
flowplane filter attach api-jwt-auth \
  --listener httpbin-listener --order 2
```

</td>
<td>

```json
// Create
{"method": "tools/call", "params": {
  "name": "cp_create_filter",
  "arguments": {
    "name": "api-jwt-auth",
    "filterType": "jwt_auth",
    "configuration": {
      "type": "jwt_auth",
      "config": {
        "providers": {
          "my-provider": {
            "issuer": "https://auth.example.com",
            "audiences": ["my-api"],
            "jwks": {
              "type": "remote",
              "http_uri": {
                "uri": "https://auth.example.com/.well-known/jwks.json",
                "cluster": "auth-jwks-cluster",
                "timeout_ms": 5000
              },
              "cache_duration_seconds": 300
            },
            "fromHeaders": [
              {"name": "Authorization", "value_prefix": "Bearer "}
            ],
            "forward": true
          }
        },
        "bypass_cors_preflight": true
      }
    },
    "team": "default"
  }
}}

// Attach
{"method": "tools/call", "params": {
  "name": "cp_attach_filter",
  "arguments": {
    "filter": "api-jwt-auth",
    "listener": "httpbin-listener",
    "order": 2,
    "team": "default"
  }
}}
```

</td>
</tr>
</table>

> ⚠️ **JWT without `rules` does not enforce authentication.** A JWT filter without rules lets all traffic through unauthenticated. Add rules to specify which paths require tokens:
>
> ```json
> "rules": [{
>   "match": { "path": { "Prefix": "/api" } },
>   "requires": { "type": "provider_name", "provider_name": "my-provider" }
> }]
> ```

> ⚠️ **Remote JWKS requires a reachable cluster.** The `cluster` field in `http_uri` must reference an existing Envoy cluster that can reach the JWKS endpoint. In a dev environment without a real auth provider, the filter will create successfully but JWKS fetch will fail at runtime.

**JWT with SDS (secrets):** For JWKS delivered via SDS instead of HTTP fetch, replace the `jwks` block:

```json
"jwks": {
  "type": "sds",
  "name": "my-jwks-secret"
}
```

Create the secret first using the REST API or MCP `cp_create_secret` tool. See the secrets skill for details.

---

## Example 3: CORS

Allow cross-origin requests from a specific frontend domain.

<table>
<tr><th>CLI</th><th>MCP</th></tr>
<tr>
<td>

**cors-filter.json:**

```json
{
  "name": "api-cors",
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          { "type": "exact", "value": "https://app.example.com" }
        ],
        "allow_methods": [
          "GET", "POST", "PUT", "DELETE", "OPTIONS"
        ],
        "allow_headers": [
          "Authorization", "Content-Type"
        ],
        "expose_headers": ["X-Request-Id"],
        "max_age": 86400,
        "allow_credentials": true
      }
    }
  }
}
```

```bash
flowplane filter create -f cors-filter.json
flowplane filter attach api-cors \
  --listener httpbin-listener --order 1
```

</td>
<td>

```json
// Create
{"method": "tools/call", "params": {
  "name": "cp_create_filter",
  "arguments": {
    "name": "api-cors",
    "filterType": "cors",
    "configuration": {
      "type": "cors",
      "config": {
        "policy": {
          "allow_origin": [
            {"type": "exact", "value": "https://app.example.com"}
          ],
          "allow_methods": [
            "GET", "POST", "PUT", "DELETE", "OPTIONS"
          ],
          "allow_headers": [
            "Authorization", "Content-Type"
          ],
          "expose_headers": ["X-Request-Id"],
          "max_age": 86400,
          "allow_credentials": true
        }
      }
    },
    "team": "default"
  }
}}

// Attach (before auth filters)
{"method": "tools/call", "params": {
  "name": "cp_attach_filter",
  "arguments": {
    "filter": "api-cors",
    "listener": "httpbin-listener",
    "order": 1,
    "team": "default"
  }
}}
```

</td>
</tr>
</table>

**Test preflight:**

```bash
curl -s -D - -o /dev/null \
  -H "Origin: https://app.example.com" \
  -H "Access-Control-Request-Method: POST" \
  -X OPTIONS http://localhost:10001/get
```

Expected response headers:

```
access-control-allow-origin: https://app.example.com
access-control-allow-methods: GET, POST, PUT, DELETE, PATCH, OPTIONS
access-control-max-age: 3600
access-control-allow-credentials: true
```

> ⚠️ Envoy may add methods (e.g., `PATCH`) and cap `max_age` at 3600 regardless of the configured value.

**Test with disallowed origin:**

```bash
curl -s -D - -o /dev/null \
  -H "Origin: https://evil.example.com" \
  http://localhost:10001/get
```

> ⚠️ Some upstream services (like httpbin) add their own CORS headers, which pass through Envoy unmodified. The Envoy CORS filter controls its own headers but does not strip upstream CORS headers for non-matching origins.

**Origin matcher types:**

| Type | Example | Matches |
|---|---|---|
| `exact` | `https://app.example.com` | Exact string match |
| `prefix` | `https://app.` | Starts with prefix |
| `suffix` | `.example.com` | Ends with suffix |
| `contains` | `example` | Contains substring |
| `regex` | `https://.*\\.example\\.com` | RE2 regex match |

**Clean up:**

```bash
flowplane filter detach api-cors --listener httpbin-listener
flowplane filter delete api-cors --yes
```

---

## Additional Filter Configs

<details>
<summary>Header Mutation</summary>

Add, remove, or modify request and response headers.

```json
{
  "name": "security-headers",
  "filterType": "header_mutation",
  "config": {
    "type": "header_mutation",
    "config": {
      "request_headers_to_add": [
        { "key": "X-Request-Id", "value": "generated", "append": false }
      ],
      "request_headers_to_remove": [],
      "response_headers_to_add": [
        { "key": "X-Content-Type-Options", "value": "nosniff", "append": false },
        { "key": "X-Frame-Options", "value": "DENY", "append": false }
      ],
      "response_headers_to_remove": ["X-Powered-By"]
    }
  }
}
```

</details>

<details>
<summary>Custom Response</summary>

Return custom responses for specific status codes.

```json
{
  "name": "custom-errors",
  "filterType": "custom_response",
  "config": {
    "type": "custom_response",
    "config": {
      "matchers": [{
        "status_code": { "type": "range", "min": 500, "max": 599 },
        "response": {
          "status_code": 500,
          "body": "{\"error\": \"Internal server error\"}",
          "headers": { "content-type": "application/json" }
        }
      }]
    }
  }
}
```

</details>

<details>
<summary>Compressor (gzip)</summary>

Compress responses for supported content types.

```json
{
  "name": "gzip-compress",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 100,
          "content_type": ["application/json", "text/html", "text/plain"],
          "disable_on_etag_header": false,
          "remove_accept_encoding_header": false
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "best_speed",
        "compression_strategy": "default_strategy",
        "memory_level": 5,
        "window_bits": 12,
        "chunk_size": 4096
      }
    }
  }
}
```

</details>

<details>
<summary>Ext Authz (HTTP)</summary>

Delegate authorization to an external HTTP service.

```json
{
  "name": "ext-auth",
  "filterType": "ext_authz",
  "config": {
    "type": "ext_authz",
    "config": {
      "service": {
        "type": "http",
        "server_uri": {
          "uri": "http://authz-service-cluster",
          "cluster": "authz-service-cluster",
          "timeout_ms": 500
        },
        "path_prefix": "/auth",
        "authorization_request": {
          "allowed_headers": ["authorization", "x-request-id"],
          "headers_to_add": []
        }
      },
      "failure_mode_allow": false,
      "clear_route_cache": false,
      "stat_prefix": "ext_authz",
      "status_on_error": 403
    }
  }
}
```

</details>

<details>
<summary>OAuth2</summary>

Full OAuth2 authorization code flow. Requires a secret for the client credential — create it first via the REST API or MCP `cp_create_secret`.

```json
{
  "name": "oauth2-flow",
  "filterType": "oauth2",
  "config": {
    "type": "oauth2",
    "config": {
      "token_endpoint": {
        "uri": "https://auth.example.com/oauth/token",
        "cluster": "oauth2-auth-cluster",
        "timeout_ms": 5000
      },
      "authorization_endpoint": "https://auth.example.com/authorize",
      "credentials": {
        "client_id": "my-app",
        "token_secret": { "type": "sds", "name": "oauth2-client-secret" }
      },
      "redirect_uri": "https://app.example.com/callback",
      "redirect_path": "/callback",
      "signout_path": "/logout",
      "auth_scopes": ["openid", "profile", "email"],
      "auth_type": "url_encoded_body",
      "forward_bearer_token": true,
      "use_refresh_token": true,
      "default_expires_in_seconds": 3600,
      "stat_prefix": "oauth2_filter"
    }
  }
}
```

The `token_secret` references a Flowplane secret by name. Create the secret first:

```bash
flowplane secret create --name oauth2-client-secret --type generic_secret \
  --config '{"type":"generic_secret","secret":"<base64-encoded-client-secret>"}'
```

</details>

---

## Secrets Integration

Filters that need credentials can reference Flowplane secrets by name. The secret must exist before creating the filter.

| Filter | Secret Field | Secret Type |
|---|---|---|
| OAuth2 | `credentials.token_secret.name` | `generic_secret` |
| JWT Auth | `jwks` (SDS variant) | `generic_secret` |
| Ext Authz | `auth_secret.name` / `tls_secret.name` | `generic_secret` / `tls_certificate` |
| Credential Injector | `secret_ref.name` | `generic_secret` |

Secrets are encrypted at rest (AES-256-GCM) and delivered to Envoy via SDS. Rotation is automatic — update the secret and Envoy receives the new value without restart.

---

## Gotchas

> ⚠️ **Router is auto-appended.** Never create or attach a Router filter. Flowplane adds it as the last filter in every chain.

> ⚠️ **JWT without rules = no enforcement.** A `jwt_auth` filter without `rules` lets all traffic through unauthenticated. Always include at least one rule.

> ⚠️ **Config nesting.** Filter JSON uses `"config": {"type": "...", "config": {...}}` — not a flat structure. MCP uses `"configuration"` as the argument key instead of `"config"`.

> ⚠️ **`filter_enabled` defaults.** Envoy's `filter_enabled` defaults to 0% for some filter types. Flowplane sets sensible defaults, but if passing raw config, verify `filter_enabled` is set.

> ⚠️ **Order must be unique.** Two filters cannot share the same order value on one listener. The second attachment will fail.

> ⚠️ **Auth filters need reachable clusters.** JWT remote JWKS, ext_authz, and OAuth2 all reference upstream clusters. Those clusters must exist and be deliverable via CDS. A CDS NACK from any bad cluster blocks all cluster updates — including auth-related ones.

> ⚠️ **Route config update is full replacement.** When modifying routes to add `typedPerFilterConfig`, fetch existing config first with `flowplane route get`. The update replaces the entire `virtualHosts` array.

---

See also: [Getting Started](getting-started.md) | [CLI Reference](cli-reference.md)
