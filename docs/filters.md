# HTTP Filters

Flowplane exposes Envoy HTTP filters through structured JSON models, providing user-friendly configuration without requiring protobuf knowledge. The registry keeps filters ordered, ensures the router filter is appended last, and translates configs into correct Envoy protobuf type URLs.

All filters support both global (listener-level) and per-route configuration via `typedPerFilterConfig` unless otherwise noted.

## Available Filters

| Filter | Envoy Filter Name | Description | Per-Route Support | Documentation |
|--------|-------------------|-------------|-------------------|---------------|
| **OAuth2** | `envoy.filters.http.oauth2` | OAuth2 authentication with authorization code flow | No | [oauth2.md](filters/oauth2.md) |
| **JWT Authentication** | `envoy.filters.http.jwt_authn` | JWT token validation with JWKS providers | Yes | [jwt_authn.md](filters/jwt_authn.md) |
| **External Authorization** | `envoy.filters.http.ext_authz` | External authorization service integration | Yes | [ext_authz.md](filters/ext_authz.md) |
| **RBAC** | `envoy.filters.http.rbac` | Role-based access control policies | Yes | Coming soon |
| **Local Rate Limit** | `envoy.filters.http.local_ratelimit` | In-process token bucket rate limiting | Yes | [local_rate_limit.md](filters/local_rate_limit.md) |
| **Rate Limit** | `envoy.filters.http.ratelimit` | Distributed rate limiting with external service | Yes | Coming soon |
| **Rate Limit Quota** | `envoy.filters.http.rate_limit_quota` | Advanced quota management with RLQS | Yes | Coming soon |
| **CORS** | `envoy.filters.http.cors` | Cross-Origin Resource Sharing policies | Yes | Coming soon |
| **Header Mutation** | `envoy.filters.http.header_mutation` | Request/response header manipulation | Yes | Coming soon |
| **Custom Response** | `envoy.filters.http.custom_response` | Custom error responses for status codes | Yes | Coming soon |
| **Health Check** | `envoy.filters.http.health_check` | Health check endpoint responses | No | Coming soon |
| **Credential Injector** | `envoy.filters.http.credential_injector` | OAuth2/API key credential injection | No | Coming soon |
| **External Processor** | `envoy.filters.http.ext_proc` | External request/response processing | No | Coming soon |
| **Compressor** | `envoy.filters.http.compressor` | Response compression (gzip, brotli) | Yes | [compressor.md](filters/compressor.md) |
| **MCP** | `envoy.filters.http.ext_proc` | AI/LLM gateway traffic inspection for JSON-RPC 2.0 and SSE | No | Coming soon |
| **Router** | `envoy.filters.http.router` | Terminal filter for request routing (auto-appended) | N/A | Coming soon |

## Filter Categories

### Authentication & Authorization
- **OAuth2** - Full OAuth2 authorization code flow with PKCE support
- **JWT Authentication** - JWT validation with remote/local JWKS
- **External Authorization** - Delegate auth decisions to external service
- **RBAC** - Policy-based access control

### Traffic Control
- **Local Rate Limit** - Per-instance rate limiting
- **Rate Limit** - Distributed rate limiting across instances
- **Rate Limit Quota** - Dynamic quota management

### Request/Response Manipulation
- **Header Mutation** - Add, remove, or modify headers
- **Custom Response** - Return custom responses for specific status codes
- **Compressor** - Compress response bodies

### Security
- **CORS** - Cross-origin request handling
- **Credential Injector** - Inject credentials for upstream calls

### Observability & Health
- **Health Check** - Respond to health probes
- **External Processor** - Custom processing logic

## Registry Model

* `HttpFilterConfigEntry` carries `name`, `isOptional`, `disabled`, and a tagged `filter` payload.
* If the router filter is missing, it is appended automatically. Multiple router entries trigger a validation error.
* Custom filters can still be supplied by providing a `TypedConfig` (type URL + base64 payload), but the goal is to cover common Envoy filters natively.

## Adding Filters to a Listener

Filters are added to the `httpFilters` array within a listener's filter chain:

```json
{
  "name": "my-listener",
  "address": "0.0.0.0",
  "port": 8080,
  "protocol": "HTTP",
  "filterChains": [{
    "name": "default",
    "filters": [{
      "name": "envoy.filters.network.http_connection_manager",
      "type": "httpConnectionManager",
      "routeConfigName": "my-routes",
      "httpFilters": [
        {
          "name": "envoy.filters.http.oauth2",
          "filter": {
            "type": "oauth2",
            ...
          }
        },
        {
          "name": "envoy.filters.http.router",
          "filter": {"type": "router"}
        }
      ]
    }]
  }]
}
```

## Per-Route Configuration

For filters that support per-route configuration, use `typedPerFilterConfig` on routes, virtual hosts, or weighted clusters:

```json
{
  "routes": [{
    "name": "api-route",
    "match": {"path": {"type": "prefix", "value": "/api"}},
    "action": {"type": "forward", "cluster": "backend"},
    "typedPerFilterConfig": {
      "envoy.filters.http.local_ratelimit": {
        "filter_type": "local_rate_limit",
        "stat_prefix": "api_route",
        "token_bucket": {
          "max_tokens": 100,
          "tokens_per_fill": 100,
          "fill_interval_ms": 1000
        }
      }
    }
  }]
}
```

## Adding a New Filter

1. Create a module in `src/xds/filters/http/` with serializable structs and `to_any()/from_proto` helpers.
2. Register it in `src/xds/filters/http/mod.rs` by extending `HttpFilterKind` and, if needed, `HttpScopedConfig`.
3. Add unit tests covering successful conversion, validation failures, and Any round-trips.
4. Document the filter in `docs/filters/` with usage examples.

This pattern keeps configuration ergonomic while maintaining full fidelity with Envoy's proto surface.
