# Filters

Filters add processing logic to traffic flowing through Envoy. Flowplane manages the full lifecycle: create a filter, attach it to a listener, verify it works, then detach and delete when done.

## Workflow

```
create  -->  attach  -->  verify  -->  detach  -->  delete
```

1. **Create** a filter with its configuration
2. **Attach** it to a listener (traffic starts flowing through it immediately)
3. **Verify** the behavior with a test request
4. **Detach** from the listener when done
5. **Delete** the filter resource

A filter must be detached from all listeners before it can be deleted. Attempting to delete an attached filter returns `409 Conflict`:

```
$ flowplane filter delete my-filter -y
Error: Cannot delete filter 'my-filter': Resource conflict: Filter is attached to
1 listener(s). Detach before deleting.
```

## Filter types

All filter types from `src/domain/filter.rs`. The `filterType` value in JSON uses snake_case.

| Type | Description | Implemented | Per-route behavior |
|------|-------------|:-----------:|-------------------|
| `header_mutation` | Add, modify, or remove HTTP headers | Yes | Full config override |
| `jwt_auth` | JSON Web Token authentication | Yes | Reference only |
| `cors` | Cross-Origin Resource Sharing policy | Yes | Full config override |
| `compressor` | Response compression (gzip) | Yes | Disable only |
| `local_rate_limit` | Local (in-memory) rate limiting | Yes | Full config override |
| `rate_limit` | External/distributed rate limiting (requires gRPC service) | No | Full config override |
| `ext_authz` | External authorization service | Yes | Full config override |
| `rbac` | Role-based access control | Yes | Full config override |
| `oauth2` | OAuth2 authentication | Yes | Not supported |
| `custom_response` | Modify responses based on status codes | Yes | Full config override |
| `mcp` | Model Context Protocol for AI/LLM gateway traffic | Yes | Disable only |

> `rate_limit` (external/distributed) is defined but **not implemented**. Use `local_rate_limit` for in-memory rate limiting.

## Config structure

Every filter uses a nested config format. The outer object has three required fields:

```json
{
  "name": "my-filter",
  "filterType": "<type>",
  "config": {
    "type": "<type>",
    "config": {
      ...filter-specific settings...
    }
  }
}
```

- **`filterType`** (top-level): the filter type as a string (e.g., `"header_mutation"`)
- **`config.type`**: must match `filterType`
- **`config.config`**: the filter-specific configuration object

Top-level fields use **camelCase** (`filterType`, not `filter_type`). Inner config fields use the naming convention of each filter type (usually snake_case).

### Example: header_mutation

Save this as `header-filter.json`:

```json
{
  "name": "add-headers",
  "filterType": "header_mutation",
  "config": {
    "type": "header_mutation",
    "config": {
      "request_headers_to_add": [
        {"key": "X-Gateway", "value": "flowplane", "append": false}
      ]
    }
  }
}
```

```
$ flowplane filter create -f header-filter.json
{
  "id": "a32152e4-...",
  "name": "add-headers",
  "filterType": "header_mutation",
  "version": 1,
  "source": "native_api",
  "team": "default",
  ...
  "allowedAttachmentPoints": ["route", "listener"]
}
```

## Attachment

Filters attach to **listeners** via `filter attach`. The `--order` flag controls execution order when multiple filters are attached (lower numbers execute first).

```
$ flowplane filter attach add-headers --listener demo-listener
Filter 'add-headers' attached to listener 'demo-listener'
```

Verify the filter is working:

```
$ curl -s http://localhost:10001/headers | python3 -m json.tool
{
    "headers": {
        "Accept": "*/*",
        "Host": "localhost:10001",
        "User-Agent": "curl/8.7.1",
        "X-Envoy-Expected-Rq-Timeout-Ms": "15000",
        "X-Gateway": "flowplane"
    }
}
```

## Per-route overrides

Filters attached at the listener level apply to all traffic. To customize behavior per route, use `typedPerFilterConfig` in the route definition. The level of customization depends on the filter type:

| Per-route behavior | What you can do | Filter types |
|-------------------|-----------------|--------------|
| **Full config** | Override the entire filter config per route | header_mutation, cors, local_rate_limit, ext_authz, rbac, custom_response |
| **Reference only** | Reference a named config from the listener-level filter | jwt_auth |
| **Disable only** | Disable the filter for specific routes | compressor, mcp |
| **Not supported** | No per-route customization | oauth2 |

Per-route overrides are set in the route's `typedPerFilterConfig` field when creating or updating a route config via the API or MCP tools:

```json
{
  "routes": [{
    "name": "api-route",
    "match": {"path": {"type": "prefix", "value": "/api"}},
    "action": {"type": "forward", "cluster": "backend"},
    "typedPerFilterConfig": {
      "envoy.filters.http.local_ratelimit": {
        "stat_prefix": "api_rate_limit",
        "token_bucket": {
          "max_tokens": 100,
          "tokens_per_fill": 100,
          "fill_interval_ms": 1000
        },
        "filter_enabled": {"numerator": 100, "denominator": "hundred"},
        "filter_enforced": {"numerator": 100, "denominator": "hundred"}
      }
    }
  }]
}
```

The key in `typedPerFilterConfig` is the Envoy HTTP filter name (e.g., `envoy.filters.http.local_ratelimit`), not the Flowplane filter type name.

## Detach and delete

To remove a filter, always detach first:

```
$ flowplane filter detach add-headers --listener demo-listener
Filter 'add-headers' detached from listener 'demo-listener'

$ flowplane filter delete add-headers -y
Filter 'add-headers' deleted successfully
```

If you try to delete without detaching:

```
$ flowplane filter delete add-headers -y
Error: Cannot delete filter 'add-headers': Resource conflict: Filter is attached to
1 listener(s). Detach before deleting.
```

---

## Traffic management filters

### local_rate_limit

Local (in-memory) rate limiting. Tokens are tracked per Envoy instance — not shared across instances. Use this for single-instance deployments or as a per-node safety net.

> The external `rate_limit` type (distributed rate limiting via a gRPC service) is defined but **not implemented**. The API rejects it with `400 Bad Request: Filter type 'rate_limit' is not yet fully supported`.

**Required fields:** `stat_prefix`, `token_bucket`

Save as `rate-limit.json`:

```json
{
  "name": "demo-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "demo_rate_limit",
      "token_bucket": {
        "max_tokens": 3,
        "tokens_per_fill": 3,
        "fill_interval_ms": 60000
      },
      "filter_enabled": {"numerator": 100, "denominator": "hundred"},
      "filter_enforced": {"numerator": 100, "denominator": "hundred"}
    }
  }
}
```

```
$ flowplane filter create -f rate-limit.json
{
  "id": "e4ba4636-...",
  "name": "demo-rate-limit",
  "filterType": "local_rate_limit",
  "version": 1,
  "source": "native_api",
  "team": "default",
  ...
  "allowedAttachmentPoints": ["route", "listener"]
}

$ flowplane filter attach demo-rate-limit --listener demo-listener
Filter 'demo-rate-limit' attached to listener 'demo-listener'
```

Verify — 3 tokens per 60 seconds means the 4th request gets rejected:

```
$ for i in 1 2 3 4 5; do curl -s -o /dev/null -w "Request $i: %{http_code}\n" http://localhost:10001/get; done
Request 1: 200
Request 2: 200
Request 3: 200
Request 4: 429
Request 5: 429
```

The 429 response body is `local_rate_limited` (plain text, not JSON).

**Config fields:**

| Field | Type | Required | Description |
|-------|------|:--------:|-------------|
| `stat_prefix` | string | Yes | Prefix for rate limit stats |
| `token_bucket.max_tokens` | u32 | Yes | Maximum tokens in the bucket |
| `token_bucket.tokens_per_fill` | u32 | No | Tokens added per refill (defaults to `max_tokens`) |
| `token_bucket.fill_interval_ms` | u64 | Yes | Refill interval in milliseconds (must be > 0) |
| `filter_enabled` | object | No | Fraction of requests where filter runs (defaults to 100%) |
| `filter_enforced` | object | No | Fraction of enabled requests where limit is enforced (defaults to 100%) |
| `status_code` | u16 | No | HTTP status when rate limited (default 429, range 400-599) |
| `per_downstream_connection` | bool | No | Track tokens per connection instead of globally |
| `rate_limited_as_resource_exhausted` | bool | No | Return gRPC RESOURCE_EXHAUSTED instead of UNAVAILABLE |

**`filter_enabled` and `filter_enforced`** both default to 100% if omitted. The `denominator` field accepts `hundred`, `ten_thousand`, or `million` (snake_case).

---

### cors

Cross-Origin Resource Sharing policy. Controls which origins, methods, and headers are allowed for cross-origin requests.

**Required fields:** `policy.allow_origin` (at least one matcher)

Save as `cors.json`:

```json
{
  "name": "demo-cors",
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          {"type": "exact", "value": "https://example.com"}
        ],
        "allow_methods": ["GET", "POST", "OPTIONS"],
        "allow_headers": ["Content-Type", "Authorization"],
        "expose_headers": ["X-Request-Id"],
        "max_age": 3600,
        "allow_credentials": true,
        "filter_enabled": {"numerator": 100, "denominator": "hundred"}
      }
    }
  }
}
```

```
$ flowplane filter create -f cors.json
{
  "id": "8cdb6c06-...",
  "name": "demo-cors",
  "filterType": "cors",
  "version": 1,
  "source": "native_api",
  "team": "default",
  ...
  "allowedAttachmentPoints": ["route", "listener"]
}

$ flowplane filter attach demo-cors --listener demo-listener
Filter 'demo-cors' attached to listener 'demo-listener'
```

Verify with a CORS preflight request from a matching origin:

```
$ curl -s -D - -o /dev/null \
    -X OPTIONS \
    -H "Origin: https://example.com" \
    -H "Access-Control-Request-Method: POST" \
    -H "Access-Control-Request-Headers: Content-Type" \
    http://localhost:10001/get
HTTP/1.1 200 OK
access-control-allow-origin: https://example.com
access-control-allow-credentials: true
access-control-allow-methods: GET, POST, PUT, DELETE, PATCH, OPTIONS
access-control-max-age: 3600
access-control-allow-headers: Content-Type
...
```

On a regular GET with the matching origin:

```
$ curl -s -D - -o /dev/null -H "Origin: https://example.com" http://localhost:10001/get
HTTP/1.1 200 OK
access-control-allow-origin: https://example.com
access-control-allow-credentials: true
...
```

**Origin matcher types:**

| Type | Description | Example |
|------|-------------|---------|
| `exact` | Exact string match | `{"type": "exact", "value": "https://example.com"}` |
| `prefix` | Prefix match | `{"type": "prefix", "value": "https://"}` |
| `suffix` | Suffix match | `{"type": "suffix", "value": ".example.com"}` |
| `contains` | Substring match | `{"type": "contains", "value": "example"}` |
| `regex` | RE2 regex match | `{"type": "regex", "value": "https://.*\\.example\\.com"}` |

**Policy fields:**

| Field | Type | Required | Description |
|-------|------|:--------:|-------------|
| `allow_origin` | array | Yes | Origin matchers (at least one required) |
| `allow_methods` | array | No | Allowed HTTP methods (e.g., `["GET", "POST"]`) |
| `allow_headers` | array | No | Allowed request headers |
| `expose_headers` | array | No | Response headers exposed to clients |
| `max_age` | u64 | No | Preflight cache duration in seconds |
| `allow_credentials` | bool | No | Allow credentials (cannot be `true` with wildcard `*` origin) |
| `filter_enabled` | object | No | Fraction of requests where CORS policy is enforced |
| `shadow_enabled` | object | No | Fraction of requests where policy is evaluated but not enforced |
| `allow_private_network_access` | bool | No | Allow requests from private networks |
| `forward_not_matching_preflights` | bool | No | Forward unmatched preflights to upstream |

**Validation rules:**
- `allow_origin` must not be empty
- `allow_credentials: true` cannot be combined with a wildcard (`*`) exact origin
- Method names must be valid HTTP methods (or `*`)
- Header names must be valid HTTP header names (or `*`)

---

### compressor

Response compression using gzip. Compresses responses that match the configured content types and exceed the minimum content length.

> **Known issue:** Attaching the compressor filter causes an Envoy NACK due to an empty `RuntimeKey` in the generated xDS config. The filter creates successfully in Flowplane but Envoy rejects the listener update. This will be fixed in a future release.

Save as `compressor.json`:

```json
{
  "name": "demo-gzip",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 256,
          "content_type": ["application/json", "text/html"]
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "default_compression"
      }
    }
  }
}
```

```
$ flowplane filter create -f compressor.json
{
  "id": "02af0e00-...",
  "name": "demo-gzip",
  "filterType": "compressor",
  "version": 1,
  "source": "native_api",
  "team": "default",
  ...
  "allowedAttachmentPoints": ["route", "listener"]
}
```

**Compressor library — gzip fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `memory_level` | u32 | — | Memory level 1-9 (higher = faster, more memory) |
| `window_bits` | u32 | — | Window bits 9-15 (higher = better compression) |
| `compression_level` | string | `best_speed` | `best_speed`, `best_compression`, or `default_compression` |
| `compression_strategy` | string | `default_strategy` | `default_strategy`, `filtered`, `huffman_only`, `rle`, or `fixed` |
| `chunk_size` | u32 | — | Internal compression chunk buffer size in bytes |

**Common config fields:**

| Field | Type | Description |
|-------|------|-------------|
| `min_content_length` | u32 | Minimum response size in bytes to trigger compression |
| `content_type` | array | Content types to compress (e.g., `["application/json"]`) |
| `disable_on_etag_header` | bool | Skip compression when response has ETag |
| `remove_accept_encoding_header` | bool | Remove Accept-Encoding after compression decision |

**Per-route:** The compressor supports `disable_only` per-route behavior — you can disable compression for specific routes but cannot override the full config.

---

## JWT Authentication (`jwt_auth`)

Validates JSON Web Tokens on incoming requests. Supports local JWKS (inline keys) and remote JWKS (fetched from a URL). Rejects requests with missing or invalid tokens.

**Create:**

```bash
flowplane filter create --file jwt-auth-filter.json
```

```json
{
  "name": "my-jwt-auth",
  "filterType": "jwt_auth",
  "config": {
    "type": "jwt_auth",
    "config": {
      "providers": {
        "my-provider": {
          "issuer": "https://accounts.example.com",
          "audiences": ["my-api"],
          "forward": true,
          "from_headers": [
            {"name": "Authorization", "value_prefix": "Bearer "}
          ],
          "jwks": {
            "type": "local",
            "inline_string": "{\"keys\":[{\"kty\":\"RSA\",\"alg\":\"RS256\",\"use\":\"sig\",\"kid\":\"key-1\",\"n\":\"...\",\"e\":\"AQAB\"}]}"
          }
        }
      },
      "rules": [
        {
          "match": {"path": {"Prefix": "/"}},
          "requires": {
            "type": "provider_name",
            "provider_name": "my-provider"
          }
        }
      ]
    }
  }
}
```

The `inline_string` value must be a valid JWKS JSON string with at least one valid public key. Envoy validates the keys at config load time — an empty keyset or invalid key data causes a NACK.

**Attach and verify:**

```
$ flowplane filter attach my-jwt-auth --listener demo-listener
Filter 'my-jwt-auth' attached to listener 'demo-listener'

$ curl -s http://localhost:10001/get
Jwt is missing

$ curl -s -w "\n%{http_code}\n" http://localhost:10001/get
Jwt is missing
401

$ curl -s -w "\n%{http_code}\n" -H "Authorization: Bearer not-a-real-jwt" http://localhost:10001/get
Jwt is not in the form of Header.Payload.Signature with two dots and 3 sections
401
```

Requests without a valid JWT get `401` with a plain-text error body.

**Provider fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `issuer` | string | — | Expected `iss` claim value |
| `audiences` | array | — | Accepted `aud` claim values |
| `forward` | bool | `false` | Forward JWT to upstream after verification |
| `from_headers` | array | `[{name: "Authorization", value_prefix: "Bearer "}]` | Headers to extract JWT from |
| `from_params` | array | — | Query parameters to extract JWT from |
| `from_cookies` | array | — | Cookies to extract JWT from |
| `forward_payload_header` | string | — | Header to forward decoded JWT payload in (base64url) |
| `payload_in_metadata` | string | — | Metadata key for JWT payload (for use by other filters) |
| `claim_to_headers` | array | — | Map JWT claims to request headers (`header_name`, `claim_name`) |
| `clock_skew_seconds` | u32 | `60` | Tolerance for `exp`/`nbf` validation |
| `require_expiration` | bool | `false` | Reject tokens without `exp` claim |
| `max_lifetime_seconds` | u64 | — | Maximum allowed token lifetime |

**JWKS source options:**

| Type | Fields | Description |
|------|--------|-------------|
| `local` | `inline_string` | Inline JWKS JSON string |
| `remote` | `http_uri.uri`, `http_uri.cluster`, `http_uri.timeout_ms` | Fetch JWKS from a URL via an Envoy cluster |

Remote JWKS example:

```json
"jwks": {
  "type": "remote",
  "http_uri": {
    "uri": "https://accounts.example.com/.well-known/jwks.json",
    "cluster": "jwks-cluster",
    "timeout_ms": 1000
  },
  "cache_duration_seconds": 600
}
```

When using remote JWKS, the `cluster` must be a valid Envoy cluster name created via `flowplane cluster create`.

**Requirement types:**

| Type | Fields | Description |
|------|--------|-------------|
| `provider_name` | `provider_name` | Require JWT from a named provider |
| `provider_with_audiences` | `provider_name`, `audiences` | Require JWT with specific audiences |
| `requires_any` | `requirements` | Logical OR of nested requirements |
| `requires_all` | `requirements` | Logical AND of nested requirements |
| `allow_missing` | — | Allow missing JWT but reject invalid ones |
| `allow_missing_or_failed` | — | Allow both missing and invalid JWTs |

**Per-route:** The `jwt_auth` filter supports `reference_only` per-route behavior — you can reference a named requirement from the top-level `requirement_map` or disable JWT checks for specific routes:

```json
{"disabled": true}
```

```json
{"requirement_name": "my-named-requirement"}
```

---

## External Authorization (`ext_authz`)

Delegates authorization decisions to an external gRPC or HTTP service. If the authz service denies the request (or is unreachable with `failure_mode_allow: false`), the request is rejected.

**Create (gRPC mode):**

```bash
flowplane filter create --file ext-authz-grpc.json
```

```json
{
  "name": "my-ext-authz",
  "filterType": "ext_authz",
  "config": {
    "type": "ext_authz",
    "config": {
      "service": {
        "type": "grpc",
        "target_uri": "ext-authz-cluster",
        "timeout_ms": 500
      },
      "failure_mode_allow": false,
      "stat_prefix": "ext_authz"
    }
  }
}
```

The `target_uri` in gRPC mode is an Envoy cluster name (not a URL). Create the cluster first with `flowplane cluster create`.

```
$ flowplane filter create --file ext-authz-grpc.json
{
  "id": "...",
  "name": "my-ext-authz",
  "filterType": "ext_authz",
  ...
  "config": {
    "config": {
      "service": {
        "type": "grpc",
        "target_uri": "ext-authz-cluster",
        "timeout_ms": 500,
        "initial_metadata": []
      },
      "failure_mode_allow": false,
      "stat_prefix": "ext_authz",
      ...
    },
    "type": "ext_authz"
  },
  "allowedAttachmentPoints": ["route", "listener"]
}
```

**Attach and verify:**

```
$ flowplane filter attach my-ext-authz --listener demo-listener
Filter 'my-ext-authz' attached to listener 'demo-listener'

$ curl -s -w "\n%{http_code}\n" http://localhost:10001/get

403
```

With `failure_mode_allow: false`, requests return `403` when the authz service is unreachable. Set to `true` to allow requests through when the authz service is down.

**Create (HTTP mode):**

```json
{
  "name": "my-ext-authz-http",
  "filterType": "ext_authz",
  "config": {
    "type": "ext_authz",
    "config": {
      "service": {
        "type": "http",
        "server_uri": {
          "uri": "http://authz-service:8080/check",
          "cluster": "authz-http-cluster",
          "timeout_ms": 300
        },
        "path_prefix": "/authz",
        "authorization_request": {
          "allowed_headers": ["authorization", "x-request-id"]
        }
      },
      "failure_mode_allow": true,
      "clear_route_cache": true
    }
  }
}
```

**Config fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `service` | object | — | Tagged union: `{"type": "grpc", ...}` or `{"type": "http", ...}` |
| `failure_mode_allow` | bool | `false` | Allow requests when authz service is unavailable |
| `with_request_body` | object | — | Buffer request body: `max_request_bytes`, `allow_partial_message`, `pack_as_bytes` |
| `clear_route_cache` | bool | `false` | Clear route cache after successful authorization |
| `status_on_error` | u32 | — | HTTP status code on authz error (default: 403) |
| `stat_prefix` | string | — | Statistics prefix for metrics |
| `include_peer_certificate` | bool | `false` | Include client certificate in authz request |

**gRPC service fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `target_uri` | string | — | Envoy cluster name for the gRPC authz service |
| `timeout_ms` | u64 | `200` | gRPC call timeout in milliseconds |
| `initial_metadata` | array | — | Metadata headers: `[{"key": "...", "value": "..."}]` |

**HTTP service fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server_uri.uri` | string | — | URL of the HTTP authz service |
| `server_uri.cluster` | string | — | Envoy cluster name for the HTTP authz service |
| `server_uri.timeout_ms` | u64 | `200` | HTTP call timeout in milliseconds |
| `path_prefix` | string | — | Path prefix for authorization requests |
| `authorization_request.allowed_headers` | array | — | Original request headers to include in authz request |
| `authorization_response.allowed_upstream_headers` | array | — | Authz response headers to add to upstream request |
| `authorization_response.allowed_client_headers` | array | — | Authz response headers to add to client response on denial |

**Per-route:** The `ext_authz` filter supports `full_config_override` per-route behavior — you can disable authz or pass context extensions:

```json
{"disabled": true}
```

```json
{
  "context_extensions": {"route-type": "public"},
  "disable_request_body_buffering": true
}
```

---

## Role-Based Access Control (`rbac`)

Enforces access control based on policies that combine permissions (what actions) with principals (who). Supports allow, deny, and log (shadow) modes.

**Create (allow GET only):**

```bash
flowplane filter create --file rbac-filter.json
```

```json
{
  "name": "my-rbac",
  "filterType": "rbac",
  "config": {
    "type": "rbac",
    "config": {
      "rules": {
        "action": "allow",
        "policies": {
          "allow-get-only": {
            "permissions": [
              {"type": "header", "name": ":method", "exact_match": "GET"}
            ],
            "principals": [
              {"type": "any", "any": true}
            ]
          }
        }
      }
    }
  }
}
```

**Attach and verify:**

```
$ flowplane filter attach my-rbac --listener demo-listener
Filter 'my-rbac' attached to listener 'demo-listener'

$ curl -s -w "\n%{http_code}\n" http://localhost:10001/get
{
  "args": {},
  "headers": { ... },
  ...
}
200

$ curl -s -w "\n%{http_code}\n" -X POST http://localhost:10001/post
RBAC: access denied
403
```

GET requests are allowed; all other methods are denied with `403`.

**Create (deny by source IP):**

```json
{
  "name": "deny-internal",
  "filterType": "rbac",
  "config": {
    "type": "rbac",
    "config": {
      "rules": {
        "action": "deny",
        "policies": {
          "deny-internal-net": {
            "permissions": [
              {"type": "any", "any": true}
            ],
            "principals": [
              {"type": "source_ip", "address_prefix": "10.0.0.0", "prefix_len": 8}
            ]
          }
        }
      }
    }
  }
}
```

**Actions:**

| Action | Description |
|--------|-------------|
| `allow` | Allow requests matching any policy; deny all others |
| `deny` | Deny requests matching any policy; allow all others |
| `log` | Shadow mode — log matches without enforcing (for testing policies) |

**Permission types (tagged union, `"type"` field selects variant):**

| Type | Fields | Description |
|------|--------|-------------|
| `any` | `any: true` | Match all requests |
| `header` | `name`, `exact_match`/`prefix_match`/`suffix_match`/`present_match` | Match by request header |
| `url_path` | `path`, `ignore_case` | Match by URL path |
| `destination_port` | `port` | Match by destination port |
| `and_rules` | `rules: [...]` | Logical AND of nested permissions |
| `or_rules` | `rules: [...]` | Logical OR of nested permissions |
| `not_rule` | `rule: {...}` | Logical NOT of a permission |

**Principal types (tagged union, `"type"` field selects variant):**

| Type | Fields | Description |
|------|--------|-------------|
| `any` | `any: true` | Match any requester |
| `authenticated` | `principal_name` | Match authenticated principals |
| `source_ip` | `address_prefix`, `prefix_len` | Match by source IP CIDR |
| `direct_remote_ip` | `address_prefix`, `prefix_len` | Match by direct remote IP CIDR |
| `header` | `name`, `exact_match`/`prefix_match` | Match by request header value |
| `and_ids` | `ids: [...]` | Logical AND of nested principals |
| `or_ids` | `ids: [...]` | Logical OR of nested principals |
| `not_id` | `id: {...}` | Logical NOT of a principal |

**Shadow rules:** Use `shadow_rules` instead of `rules` to log policy matches without enforcement. Useful for testing new policies before enforcing them:

```json
{
  "rules": {
    "action": "allow",
    "policies": { "allow-all": { "permissions": [{"type": "any", "any": true}], "principals": [{"type": "any", "any": true}] } }
  },
  "shadow_rules": {
    "action": "deny",
    "policies": { "proposed-deny": { "permissions": [...], "principals": [...] } }
  },
  "track_per_rule_stats": true
}
```

**Per-route:** The `rbac` filter supports `full_config_override` per-route behavior — you can disable RBAC or provide override rules:

```json
{"disabled": true}
```

```json
{
  "rbac": {
    "action": "allow",
    "policies": {
      "route-specific-policy": {
        "permissions": [{"type": "any", "any": true}],
        "principals": [{"type": "header", "name": "x-admin", "exact_match": "true"}]
      }
    }
  }
}
```

---

## credential_injector

> **Not a Flowplane FilterType.** The `credential_injector` exists as an XDS module (`src/xds/filters/http/credential_injector.rs`) but is not registered in the `FilterType` enum in `src/domain/filter.rs`. Attempting to create one returns:
>
> ```
> Error: Unknown filter type 'credential_injector'. Available built-in types:
> header_mutation, jwt_auth, local_rate_limit, custom_response, mcp, cors,
> compressor, ext_authz, rbac, oauth2
> ```
>
> If you need to inject credentials (e.g., API keys) into upstream requests, use the `header_mutation` filter to add the required headers.

---

## Utility and observability filters

### header_mutation

Add, modify, or remove HTTP headers on requests and responses. This filter does not require listener-level config — it works with an empty config — and supports full per-route override.

**Config fields:**

| Field | Type | Description |
|---|---|---|
| `request_headers_to_add` | array | Headers to add/overwrite on requests. Each entry: `{key, value, append}` |
| `request_headers_to_remove` | array | Header names to strip from requests |
| `response_headers_to_add` | array | Headers to add/overwrite on responses. Each entry: `{key, value, append}` |
| `response_headers_to_remove` | array | Header names to strip from responses |

Each header entry has:
- `key` — header name (required, cannot be empty)
- `value` — header value
- `append` — `false` (default) overwrites existing header; `true` appends a second value

**Create:**

Save to `header-mutation.json`:

```json
{
  "name": "add-custom-headers",
  "filterType": "header_mutation",
  "config": {
    "type": "header_mutation",
    "config": {
      "request_headers_to_add": [
        {"key": "x-custom-header", "value": "hello-from-flowplane", "append": false}
      ],
      "response_headers_to_add": [
        {"key": "x-powered-by", "value": "flowplane", "append": false}
      ],
      "response_headers_to_remove": ["server"]
    }
  }
}
```

```
$ flowplane filter create --file header-mutation.json
{
  "id": "b4a9ffe5-...",
  "name": "add-custom-headers",
  "filterType": "header_mutation",
  ...
}
```

**Attach and verify:**

```
$ flowplane filter attach --listener test-filters-listener add-custom-headers
Filter 'add-custom-headers' attached to listener 'test-filters-listener'
```

Request headers — httpbin echoes them back:

```
$ curl -s http://localhost:10001/get
{
  "headers": {
    "X-Custom-Header": "hello-from-flowplane",
    ...
  }
}
```

Response headers:

```
$ curl -sI http://localhost:10001/get
HTTP/1.1 200 OK
x-powered-by: flowplane
...
```

> **Note:** `response_headers_to_remove: ["server"]` may not remove the `server: envoy` header because Envoy adds it after filter processing. Use the `envoy.reloadable_features.enable_connect_udp_support` bootstrap flag or Envoy's `server_header_transformation` setting to control the server header.

**Per-route:** Supports `full_config` override — you can set different headers per route via `typedPerFilterConfig` with key `envoy.filters.http.header_mutation`.

---

### custom_response

Replace error responses with custom content based on status code matching. Useful for returning JSON error bodies instead of default HTML, or for branding error pages.

**Config fields:**

| Field | Type | Description |
|---|---|---|
| `matchers` | array | List of status code matcher rules (preferred) |
| `custom_response_matcher` | object | Legacy base64 protobuf matcher (not recommended) |

Each matcher rule has:
- `status_code` — one of:
  - `{"type": "exact", "code": 503}` — match a single status code
  - `{"type": "range", "min": 500, "max": 599}` — match a range
  - `{"type": "list", "codes": [502, 503, 504]}` — match specific codes
- `response` — the replacement response:
  - `status_code` (optional) — override status code
  - `body` (optional) — response body string
  - `headers` — map of headers to add (e.g., `{"content-type": "application/json"}`)

**Create:**

Save to `custom-response.json`:

```json
{
  "name": "json-error-pages",
  "filterType": "custom_response",
  "config": {
    "type": "custom_response",
    "config": {
      "matchers": [
        {
          "status_code": {"type": "exact", "code": 503},
          "response": {
            "status_code": 503,
            "body": "{\"error\": \"Service temporarily unavailable\", \"status_code\": 503}",
            "headers": {"content-type": "application/json"}
          }
        }
      ]
    }
  }
}
```

```
$ flowplane filter create --file custom-response.json
{
  "id": "9edc7221-...",
  "name": "json-error-pages",
  "filterType": "custom_response",
  ...
}
```

**Attach and verify:**

```
$ flowplane filter attach --listener test-filters-listener json-error-pages
Filter 'json-error-pages' attached to listener 'test-filters-listener'
```

Normal traffic is unaffected:

```
$ curl -s http://localhost:10001/get | head -3
{
  "args": {},
  "headers": {
```

A 503 response gets replaced with the custom JSON body:

```
$ curl -s http://localhost:10001/status/503
{"error": "Service temporarily unavailable", "status_code": 503}

$ curl -sI http://localhost:10001/status/503
HTTP/1.1 503 Service Unavailable
content-type: application/json
content-length: 64
```

**Per-route:** Supports `full_config` override — you can set different custom responses per route.

---

### mcp

Model Context Protocol filter for AI/LLM gateway traffic. Inspects HTTP traffic for MCP protocol compliance (JSON-RPC 2.0 over POST, SSE streaming).

**Config fields:**

| Field | Type | Description |
|---|---|---|
| `traffic_mode` | string | `pass_through` (default) — proxy all traffic normally. `reject_no_mcp` — reject non-MCP requests |

**Create (pass-through mode):**

Save to `mcp-filter.json`:

```json
{
  "name": "mcp-gateway",
  "filterType": "mcp",
  "config": {
    "type": "mcp",
    "config": {
      "traffic_mode": "pass_through"
    }
  }
}
```

```
$ flowplane filter create --file mcp-filter.json
{
  "id": "a87b04bf-...",
  "name": "mcp-gateway",
  "filterType": "mcp",
  ...
}
```

**Create (reject mode):**

```json
{
  "name": "mcp-strict",
  "filterType": "mcp",
  "config": {
    "type": "mcp",
    "config": {
      "traffic_mode": "reject_no_mcp"
    }
  }
}
```

In `reject_no_mcp` mode, only valid MCP requests are allowed:
- POST with JSON-RPC 2.0 messages
- GET with `Accept: text/event-stream` (SSE)

> **Note:** The `reject_no_mcp` mode requires the custom `envoy.filters.http.mcp` extension to be compiled into Envoy. The standard dev-mode Envoy image may not include this extension — in that case both modes behave as pass-through.

**Attach and verify:**

```
$ flowplane filter attach --listener test-filters-listener mcp-gateway
Filter 'mcp-gateway' attached to listener 'test-filters-listener'

$ curl -s http://localhost:10001/get | head -3
{
  "args": {},
  "headers": {
```

**Per-route:** Supports `disable_only` — can disable MCP filtering for specific routes, but cannot override the config.

---

### oauth2

OAuth2 authentication filter. Redirects unauthenticated users to an OAuth2 provider and manages token cookies. This is a **listener-only** filter — Envoy does not support per-route configuration for OAuth2.

**Config fields:**

| Field | Type | Required | Description |
|---|---|---|---|
| `token_endpoint.uri` | string | yes | Token endpoint URL |
| `token_endpoint.cluster` | string | yes | Envoy cluster for token endpoint |
| `token_endpoint.timeout_ms` | number | no | Timeout in ms (default: 5000) |
| `authorization_endpoint` | string | yes | Authorization endpoint URL |
| `credentials.client_id` | string | yes | OAuth2 client ID |
| `credentials.token_secret` | object | no | SDS secret config `{name}` for client secret |
| `credentials.cookie_domain` | string | no | Domain for OAuth cookies |
| `redirect_uri` | string | yes | Callback URL (must match provider config) |
| `redirect_path` | string | no | Callback path (default: `/oauth2/callback`) |
| `signout_path` | string | no | Sign-out path (clears cookies) |
| `auth_scopes` | array | no | Scopes to request (default: `["openid", "profile", "email"]`) |
| `auth_type` | string | no | `url_encoded_body` (default) or `basic_auth` |
| `forward_bearer_token` | bool | no | Forward token to upstream (default: true) |
| `preserve_authorization_header` | bool | no | Keep existing auth header (default: false) |
| `use_refresh_token` | bool | no | Auto-renew tokens (default: false) |
| `pass_through_matcher` | array | no | Paths to bypass OAuth2 (see below) |

**Pass-through matchers** — the only way to make specific routes public (since OAuth2 has no per-route config):

| Field | Description |
|---|---|
| `path_exact` | Exact path match (e.g., `/healthz`) |
| `path_prefix` | Path prefix match (e.g., `/api/public/`) |
| `path_regex` | Regex path match (e.g., `^/static/.*`) |
| `header_name` + `header_value` | Custom header match |

**Create:**

Save to `oauth2-filter.json`:

```json
{
  "name": "oauth2-auth",
  "filterType": "oauth2",
  "config": {
    "type": "oauth2",
    "config": {
      "token_endpoint": {
        "uri": "https://auth.example.com/oauth/token",
        "cluster": "auth-cluster",
        "timeout_ms": 5000
      },
      "authorization_endpoint": "https://auth.example.com/oauth/authorize",
      "credentials": {
        "client_id": "my-client-id",
        "token_secret": {"name": "oauth2-token-secret"}
      },
      "redirect_uri": "https://app.example.com/oauth2/callback",
      "redirect_path": "/oauth2/callback",
      "auth_scopes": ["openid", "profile", "email"],
      "forward_bearer_token": true,
      "pass_through_matcher": [
        {"path_exact": "/healthz"},
        {"path_prefix": "/api/public/"}
      ]
    }
  }
}
```

```
$ flowplane filter create --file oauth2-filter.json
{
  "id": "5721d9bd-...",
  "name": "oauth2-auth",
  "filterType": "oauth2",
  "allowedAttachmentPoints": ["listener"],
  ...
}
```

Note `allowedAttachmentPoints: ["listener"]` — OAuth2 cannot be attached to routes.

**Attach:**

```
$ flowplane filter attach --listener my-listener oauth2-auth
Filter 'oauth2-auth' attached to listener 'my-listener'
```

**Prerequisites for OAuth2 to work:**

1. **Auth cluster** — the `token_endpoint.cluster` must reference an existing Flowplane cluster pointing to your OAuth provider
2. **Client secret** — create an SDS secret named `oauth2-token-secret` with the client secret value
3. **HMAC secret** — the filter automatically references an SDS secret named `hmac-secret` for cookie signing

```
flowplane secret create --name oauth2-token-secret --type generic_secret \
  --config '{"type": "generic_secret", "secret": "your-client-secret-here"}'

flowplane secret create --name hmac-secret --type generic_secret \
  --config '{"type": "generic_secret", "secret": "random-32-byte-hmac-key-here"}'
```

> **Per-route:** Not supported. Envoy rejects `typedPerFilterConfig` for `envoy.filters.http.oauth2` with: "The filter envoy.filters.http.oauth2 doesn't support virtual host or route specific configurations". Use `pass_through_matcher` to bypass OAuth2 for specific paths.
