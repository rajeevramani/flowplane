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

*Individual filter type examples continue below. See [CLI Reference](cli-reference.md) for command syntax.*
