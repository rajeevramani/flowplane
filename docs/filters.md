# HTTP Filters

Flowplane exposes Envoy HTTP filters through structured JSON models, providing user-friendly configuration without requiring protobuf knowledge. The registry keeps filters ordered, ensures the router filter is appended last, and translates configs into correct Envoy protobuf type URLs.

**Available Filters (v0.0.1):**
- **local_rate_limit** - Token bucket rate limiting (global and per-route)
- **cors** - Cross-Origin Resource Sharing policies
- **jwt_authn** - JWT authentication with JWKS providers
- **custom_response** - User-friendly custom error responses
- **header_mutation** - Request/response header manipulation
- **health_check** - Health check endpoint responses
- **router** - Terminal filter for request routing (auto-appended)

All filters support both global (listener-level) and per-route configuration via `typedPerFilterConfig`.

## Registry Model
* `HttpFilterConfigEntry` carries `name`, `isOptional`, `disabled`, and a tagged `filter` payload.
* If the router filter is missing, it is appended automatically. Multiple router entries trigger a validation error.
* Custom filters can still be supplied by providing a `TypedConfig` (type URL + base64 payload), but the goal is to cover common Envoy filters natively.

## Local Rate Limit
The Local Rate Limit filter mirrors `envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit`. Field names are snake_case in JSON and map directly to Envoy’s protobuf fields.

Key fields:

| Field | Description |
| ----- | ----------- |
| `stat_prefix` | Required prefix for emitted stats. |
| `token_bucket` | Required bucket (`max_tokens`, optional `tokens_per_fill`, `fill_interval_ms`). Validation rejects zero/negative intervals. |
| `status_code` | Optional HTTP status override (clamped to 400–599). |
| `filter_enabled` / `filter_enforced` | Runtime fractional percent wrappers (numerator + denominator + runtime key). |
| `per_downstream_connection` | Switch between global and per-connection buckets. |
| `always_consume_default_token_bucket`, `max_dynamic_descriptors`, etc. | Optional toggles that map 1:1 with Envoy.

### Global vs. Route-Specific Limits

**Listener-wide limit** – add a `local_rate_limit` filter to the HTTP filter chain:

```json
{
  "name": "envoy.filters.http.local_ratelimit",
  "filter": {
    "type": "local_rate_limit",
    "stat_prefix": "listener_global",
    "token_bucket": {
      "max_tokens": 100,
      "tokens_per_fill": 100,
      "fill_interval_ms": 1000
    }
  }
}
```

All requests traversing that connection manager share the same bucket.

**Route-specific limit** – attach a scoped config through `typedPerFilterConfig`:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.local_ratelimit": {
    "stat_prefix": "per_route",
    "token_bucket": {
      "max_tokens": 20,
      "tokens_per_fill": 20,
      "fill_interval_ms": 1000
    },
    "status_code": 429
  }
}
```

Route, virtual host, and weighted cluster entries can all supply scoped configs. The control plane converts these blocks into the correct `Any` payload for Envoy.

## CORS (Cross-Origin Resource Sharing)
The CORS filter maps to `envoy.extensions.filters.http.cors.v3.CorsPolicy` and is exposed through `CorsConfig`. Policies can be applied globally via the filter chain and overridden per route with `CorsPerRouteConfig`.

Key fields:

| Field | Description |
| ----- | ----------- |
| `allow_origin` | Required list of origin matchers (`exact`, `prefix`, `suffix`, `contains`, `regex`). Multiple entries are OR’d. |
| `allow_methods` | Optional list of HTTP methods (e.g. `GET`, `POST`). `*` is permitted for wildcard methods. |
| `allow_headers` / `expose_headers` | Optional header allowlists. Entries are validated against HTTP header syntax; `*` is allowed. |
| `max_age` | Optional max-age in seconds for preflight caching. Values must be non-negative and ≤ 315 576 000 000 (10,000 years). |
| `allow_credentials` | Enables credentialed requests. Validation rejects the configuration if credentials are allowed while an `*` origin matcher is present. |
| `filter_enabled` / `shadow_enabled` | Runtime fractional percent wrappers controlling enforcement vs. shadow evaluation. |
| `allow_private_network_access` | Propagates Chrome’s Private Network Access response headers when set. |
| `forward_not_matching_preflights` | Controls whether unmatched preflight requests are forwarded upstream (defaults to Envoy’s behaviour when omitted). |

### Example Filter Entry

```json
{
  "name": "envoy.filters.http.cors",
  "filter": {
    "type": "cors",
    "policy": {
      "allow_origin": [
        { "type": "exact", "value": "https://app.example.com" },
        { "type": "suffix", "value": ".internal.example.com" }
      ],
      "allow_methods": ["GET", "POST"],
      "allow_headers": ["authorization", "content-type"],
      "max_age": 600,
      "allow_credentials": true,
      "forward_not_matching_preflights": true
    }
  }
}
```

### Per-Route Override

Attach overrides using Envoy’s `typedPerFilterConfig` on routes, virtual hosts, or weighted clusters:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.cors": {
    "policy": {
      "allow_origin": [
        { "type": "exact", "value": "https://reports.example.net" }
      ],
      "allow_methods": ["GET"],
      "allow_credentials": false
    }
  }
}
```

The registry validates origin patterns, headers, and max-age values before producing the relevant `Any` payload (`envoy.config.route.v3.CorsPolicy`).

## Custom Response

The Custom Response filter provides user-friendly configuration for custom error responses, mapped to `envoy.extensions.filters.http.custom_response.v3.CustomResponse`. This filter allows you to return custom HTTP responses for specific status codes or conditions.

**Use Cases:**
- Branded error pages (404, 500, 503)
- Rate limit exceeded messages (429)
- Maintenance mode responses
- API error standardization

### Key Fields

| Field | Description |
| ----- | ----------- |
| `status_code` | HTTP status code to return (e.g., 429, 503) |
| `body` | Response body content (string or inline JSON) |
| `body_format` | Optional format specification for dynamic content |
| `response_headers_to_add` | Array of headers to add/override in the response |

### Example Filter Entry

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "api-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.custom_response",
            "filter": {
              "type": "custom_response",
              "custom_response_matcher": {
                "matcher": {
                  "status_code_matcher": {
                    "code": 429
                  }
                },
                "custom_response": {
                  "status_code": 429,
                  "body": "{\"error\": \"rate_limit_exceeded\", \"message\": \"Too many requests. Please try again later.\"}",
                  "response_headers_to_add": [
                    {"header": {"key": "Content-Type", "value": "application/json"}},
                    {"header": {"key": "Retry-After", "value": "60"}}
                  ]
                }
              }
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }'
```

### OpenAPI Extension

When importing OpenAPI specs, use the `x-flowplane-custom-response` extension for simplified configuration:

```yaml
paths:
  /api/users:
    get:
      x-flowplane-custom-response:
        status_code: 429
        body: "Rate limit exceeded"
```

Flowplane automatically expands this into the full CustomResponse filter configuration.

### Per-Route Override

Attach custom responses at route level via `typedPerFilterConfig`:

```json
"typedPerFilterConfig": {
  "envoy.filters.http.custom_response": {
    "status_code": 503,
    "body": "Service temporarily unavailable",
    "response_headers_to_add": [
      {"header": {"key": "Content-Type", "value": "text/plain"}}
    ]
  }
}
```

## Header Mutation

The Header Mutation filter provides request and response header manipulation, mapped to Envoy's header manipulation capabilities. This filter allows adding, removing, or modifying HTTP headers.

**Use Cases:**
- Add correlation IDs to requests
- Remove sensitive headers before forwarding
- Set CORS headers dynamically
- Add custom tracking headers
- Normalize header formats

### Key Fields

| Field | Description |
| ----- | ----------- |
| `request_headers_to_add` | Headers to add/override in the request |
| `request_headers_to_remove` | Header names to remove from the request |
| `response_headers_to_add` | Headers to add/override in the response |
| `response_headers_to_remove` | Header names to remove from the response |

### Example Filter Entry

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-routes",
    "virtualHosts": [{
      "name": "api-host",
      "domains": ["api.example.com"],
      "routes": [{
        "name": "users-route",
        "match": {
          "path": {"type": "prefix", "value": "/users"}
        },
        "action": {
          "type": "forward",
          "cluster": "backend-api"
        },
        "typedPerFilterConfig": {
          "envoy.filters.http.header_mutation": {
            "request_headers_to_add": [
              {"header": {"key": "X-Request-ID", "value": "%REQ(x-request-id)%"}},
              {"header": {"key": "X-Forwarded-For", "value": "%DOWNSTREAM_REMOTE_ADDRESS%"}}
            ],
            "request_headers_to_remove": ["X-Internal-Debug"],
            "response_headers_to_add": [
              {"header": {"key": "X-Frame-Options", "value": "DENY"}},
              {"header": {"key": "X-Content-Type-Options", "value": "nosniff"}}
            ],
            "response_headers_to_remove": ["Server", "X-Powered-By"]
          }
        }
      }]
    }]
  }'
```

### Dynamic Header Values

Envoy supports command operators for dynamic header values:

| Operator | Description | Example |
|----------|-------------|---------|
| `%DOWNSTREAM_REMOTE_ADDRESS%` | Client IP address | `X-Forwarded-For` |
| `%REQ(header)%` | Request header value | `X-Original-Path: %REQ(:path)%` |
| `%RESP(header)%` | Response header value | `X-Cache-Status: %RESP(x-cache)%` |
| `%START_TIME%` | Request start time | `X-Request-Start: %START_TIME%` |

See [Envoy command operators](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#custom-request-response-headers) for complete reference.

## Health Check

The Health Check filter provides endpoint-based health check responses without forwarding to upstreams. Mapped to `envoy.extensions.filters.http.health_check.v3.HealthCheck`.

**Use Cases:**
- Kubernetes liveness/readiness probes
- Load balancer health checks
- Monitoring system endpoints
- Service mesh health validation

### Key Fields

| Field | Description |
| ----- | ----------- |
| `pass_through_mode` | Allow requests to pass through to upstream (default: false) |
| `headers` | Array of header matchers to identify health check requests |
| `cluster_min_healthy_percentages` | Minimum healthy percentage per cluster |

### Example Filter Entry

```json
{
  "name": "envoy.filters.http.health_check",
  "filter": {
    "type": "health_check",
    "pass_through_mode": false,
    "headers": [
      {
        "name": ":path",
        "exact_match": "/healthz"
      }
    ]
  }
}
```

Requests matching the header criteria receive immediate `200 OK` responses without reaching upstream services.

## Router

The Router filter is the **required terminal filter** for HTTP listeners, handling request forwarding to upstream clusters. Mapped to `envoy.extensions.filters.http.router.v3.Router`.

**Behavior:**
- Automatically appended to HTTP filter chains if not explicitly provided
- Must be the last filter in the chain
- Multiple router filters in one chain trigger validation errors
- Handles cluster selection, retries, timeouts, and load balancing

### Auto-Append

If you omit the router filter, Flowplane automatically appends it:

```json
{
  "httpFilters": [
    {
      "name": "envoy.filters.http.local_ratelimit",
      "filter": {"type": "local_rate_limit", ...}
    }
    // Router automatically appended here
  ]
}
```

### Explicit Configuration

```json
{
  "name": "envoy.filters.http.router",
  "filter": {
    "type": "router",
    "dynamic_stats": true,
    "start_child_span": false,
    "upstream_log": []
  }
}
```

**Configuration Fields:**

| Field | Description | Default |
|-------|-------------|---------|
| `dynamic_stats` | Enable per-route statistics | `true` |
| `start_child_span` | Create child spans for distributed tracing | `false` |
| `upstream_log` | Upstream access logging configuration | `[]` |
| `suppress_envoy_headers` | Suppress Envoy-specific headers | `false` |

## JWT Authentication
Structured JWT auth lives in `JwtAuthenticationConfig` and `JwtProviderConfig`, mapping to `envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication`.

### Providers
* Support remote JWKS (`uri`, `cluster`, `timeoutMs`, optional cache duration, async fetch, retry policy) and local JWKS (filename, inline string/bytes, env var).
* Validate non-empty headers, claim mappings, and metadata keys.
* Offer payload/header metadata emission, failure status metadata, and claim-to-header forwarding with optional padding.
* Enable payload normalization (space-delimited claims -> arrays) and provider-level JWT caching.

### Requirements
`JwtRequirementConfig` composes requirements using `provider_name`, provider + audiences, AND/OR lists, and `allow_missing` / `allow_missing_or_failed`. Route rules can inline a requirement or reference a named requirement from `requirement_map`.

### Per-Route Overrides
`JwtPerRouteConfig` supports `disabled: true` or `requirementName`. The registry handles decoding/encoding of `PerRouteConfig` protos so you can attach overrides through `typedPerFilterConfig` at route, virtual host, or weighted-cluster scopes.

### Example
See the [README quick start](../README.md#example-configuration) for listener + route JSON demonstrating JWT auth plus Local Rate Limit.

## Adding a New Filter
1. Create a module in `src/xds/filters/http/` with serializable structs and `to_any()/from_proto` helpers.
2. Register it in `src/xds/filters/http/mod.rs` by extending `HttpFilterKind` and, if needed, `HttpScopedConfig`.
3. Add unit tests covering successful conversion, validation failures, and Any round-trips.
4. Document the filter here with usage examples.

This pattern keeps configuration ergonomic while maintaining full fidelity with Envoy’s proto surface.
