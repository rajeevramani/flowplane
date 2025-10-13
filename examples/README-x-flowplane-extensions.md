# x-flowplane OpenAPI Extensions Guide

This guide explains how to use `x-flowplane` vendor extensions to configure HTTP filters when importing OpenAPI specifications via the Platform API.

## Overview

Flowplane supports two types of filter configuration through OpenAPI extensions:

1. **Global Filters** (`x-flowplane-filters`) - Applied to ALL routes on the API's listener
2. **Route-Level Overrides** (`x-flowplane-route-overrides`) - Override or customize filters for specific routes

## Filter Alias Reference

This table shows how OpenAPI filter aliases map to Envoy filter names and where they can be used.

**Key Concept:** Filter **type names** (used in global filters) are different from filter **aliases** (used in route overrides). The aliases are intentionally shorter for cleaner route configuration.

| Filter | Envoy Filter Name | Global Filter Type (`x-flowplane-filters`) | Route Override Alias (`x-flowplane-route-overrides`) |
|--------|-------------------|-------------------------------------------|-----------------------------------------------------|
| **CORS** | `envoy.filters.http.cors` | ✅ `type: cors` | ✅ `cors:` |
| **JWT Authentication** | `envoy.filters.http.jwt_authn` | ✅ `type: jwt_authn` | ✅ `authn:` or `jwt_authn:` |
| **Header Mutation** | `envoy.filters.http.header_mutation` | ✅ `type: header_mutation` | ✅ `header_mutation:` |
| **Custom Response** | `envoy.filters.http.custom_response` | ✅ `type: custom_response` | ✅ `custom_response:` |
| **Local Rate Limit** | `envoy.filters.http.local_ratelimit` | ✅ `type: local_rate_limit` | ✅ `rate_limit:` (NOT `local_rate_limit`) |
| **Distributed Rate Limit** | `envoy.filters.http.ratelimit` | ❌ No | ✅ `ratelimit:` |
| **Rate Limit Quota** | `envoy.filters.http.rate_limit_quota` | ❌ No | ✅ `rate_limit_quota:` |
| **Credential Injector** | `envoy.filters.http.credential_injector` | ❌ No | ❌ No (Native API only) |
| **External Processor** | `envoy.filters.http.ext_proc` | ❌ No | ❌ No (Native API only) |

### Important Naming Distinctions

The **Local Rate Limit** filter has different names in different contexts:

| Context | Use This Name | Why |
|---------|---------------|-----|
| **Filter Name** | `local_rate_limit` | The actual Envoy filter is called "local_ratelimit" (with underscore) |
| **Global Filter Type** | `type: local_rate_limit` | Matches the filter name for consistency |
| **Route Override Alias** | `rate_limit:` | Shortened alias for cleaner route configuration |

**Common Mistake:**
```yaml
# ❌ WRONG - using filter type name as route override
x-flowplane-route-overrides:
  local_rate_limit:  # This will fail!
    stat_prefix: my_rl

# ✅ CORRECT - using the route override alias
x-flowplane-route-overrides:
  rate_limit:  # Shortened alias
    stat_prefix: my_rl
```

### Alias Usage Examples

**Global Filter (x-flowplane-filters):**
```yaml
x-flowplane-filters:
  - filter:
      type: local_rate_limit  # Use 'type' field with filter type name
      stat_prefix: global_rl
      token_bucket:
        max_tokens: 100
```

**Route Override (x-flowplane-route-overrides):**
```yaml
x-flowplane-route-overrides:
  rate_limit:  # Use alias name (NOT 'local_rate_limit')
    stat_prefix: route_rl
    token_bucket:
      max_tokens: 10
```

## Importing OpenAPI with Filters

To import an OpenAPI spec with x-flowplane extensions, use the Platform API endpoint with listener isolation:

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?listenerIsolation=true" \
  -H "Content-Type: application/yaml" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  --data-binary @openapi-with-x-flowplane-filters.yaml
```

**Important**: You MUST use `listenerIsolation=true` for x-flowplane filters to take effect. This creates a dedicated listener with your custom filters instead of using the shared gateway listener.

## Global Filters: `x-flowplane-filters`

Global filters are defined at the root level of your OpenAPI spec and apply to **all routes** on the API's isolated listener.

### Format

```yaml
x-flowplane-filters:
  - filter:
      type: <filter_type>
      <filter_config_fields>
```

### Supported Filter Types

#### 1. CORS Filter

```yaml
x-flowplane-filters:
  - filter:
      type: cors
      policy:
        allow_origin:
          - type: exact
            value: "https://app.example.com"
          - type: prefix
            value: "https://*.example.com"
        allow_methods:
          - GET
          - POST
          - PUT
          - DELETE
        allow_headers:
          - content-type
          - authorization
        expose_headers:
          - x-request-id
        max_age: 3600
        allow_credentials: true
```

**Fields:**
- `allow_origin`: Array of allowed origins (exact match or prefix)
- `allow_methods`: HTTP methods to allow
- `allow_headers`: Request headers to allow
- `expose_headers`: Response headers to expose to the browser
- `max_age`: Preflight cache duration in seconds
- `allow_credentials`: Whether to allow credentials (cookies, auth headers)

#### 2. Header Mutation Filter

```yaml
x-flowplane-filters:
  - filter:
      type: header_mutation
      request_headers_to_add:
        - key: x-api-version
          value: "v1"
          append: false
      request_headers_to_remove:
        - x-internal-header
      response_headers_to_add:
        - key: x-powered-by
          value: "Flowplane"
          append: false
      response_headers_to_remove:
        - server
```

**Fields:**
- `request_headers_to_add`: Headers to add to incoming requests
- `request_headers_to_remove`: Headers to remove from requests
- `response_headers_to_add`: Headers to add to outgoing responses
- `response_headers_to_remove`: Headers to remove from responses
- `append`: Whether to append (true) or replace (false) existing header

#### 3. Local Rate Limit Filter

```yaml
x-flowplane-filters:
  - filter:
      type: local_rate_limit
      stat_prefix: api_rate_limit
      token_bucket:
        max_tokens: 1000
        tokens_per_fill: 1000
        fill_interval_ms: 60000  # 1 minute
      status_code: 429
```

**Fields:**
- `stat_prefix`: Prefix for statistics (required)
- `token_bucket`: Token bucket configuration (required)
  - `max_tokens`: Maximum tokens in bucket
  - `tokens_per_fill`: Tokens added per refill
  - `fill_interval_ms`: Refill interval in milliseconds
- `status_code`: HTTP status to return when rate limited (default: 429)

## Route-Level Overrides: `x-flowplane-route-overrides`

Route-level overrides are defined on individual operation objects (GET, POST, etc.) and allow you to:
- Disable specific filters
- Override filter configurations
- Add route-specific filter behavior

### Format

```yaml
paths:
  /api/endpoint:
    get:
      x-flowplane-route-overrides:
        <filter_alias>: <override_value>
```

### Override Types

#### 1. Disable a Filter

```yaml
x-flowplane-route-overrides:
  authn: disabled
```

This completely disables authentication for this specific route.

#### 2. Use a Named Configuration

```yaml
x-flowplane-route-overrides:
  authn: jwt-validation
  cors: admin-only
```

This references pre-configured filter configurations by name.

#### 3. Inline Filter Configuration

```yaml
x-flowplane-route-overrides:
  cors:
    policy:
      allow_origin:
        - type: exact
          value: "https://admin.example.com"
      allow_methods:
        - GET
      allow_credentials: true
```

This provides a complete inline configuration that overrides the global filter.

#### 4. Override Rate Limit

```yaml
x-flowplane-route-overrides:
  rate_limit:  # Use 'rate_limit' alias (NOT 'local_rate_limit')
    stat_prefix: endpoint_specific_rl
    token_bucket:
      max_tokens: 100
      tokens_per_fill: 100
      fill_interval_ms: 60000  # More restrictive: 100/min
```

#### 5. Override Header Mutation

```yaml
x-flowplane-route-overrides:
  header_mutation:
    request_headers_to_add:
      - key: x-route-specific
        value: "special-endpoint"
        append: false
```

## Common Use Cases

### Use Case 1: Public Health Check with Rate Limit Override

```yaml
paths:
  /health:
    get:
      x-flowplane-route-overrides:
        authn: disabled  # No auth required
        rate_limit:  # More permissive rate limit (use 'rate_limit' alias)
          stat_prefix: health_rl
          token_bucket:
            max_tokens: 10000
            tokens_per_fill: 10000
            fill_interval_ms: 60000
```

### Use Case 2: Admin Endpoints with Restricted CORS

```yaml
# Global CORS allows multiple origins
x-flowplane-filters:
  - filter:
      type: cors
      policy:
        allow_origin:
          - type: exact
            value: "https://app.example.com"

paths:
  /admin/users:
    get:
      # Admin endpoints only allow admin panel origin
      x-flowplane-route-overrides:
        cors:
          policy:
            allow_origin:
              - type: exact
                value: "https://admin.example.com"
            allow_credentials: true
```

### Use Case 3: Webhook with Custom Headers

```yaml
paths:
  /webhooks/github:
    post:
      x-flowplane-route-overrides:
        authn: webhook-signature
        header_mutation:
          request_headers_to_add:
            - key: x-webhook-source
              value: "github"
              append: false
```

### Use Case 4: Strict Rate Limiting for Write Operations

```yaml
paths:
  /users:
    get:
      # Read operations: use global rate limit
      responses:
        '200':
          description: List users

    post:
      # Write operations: much stricter
      x-flowplane-route-overrides:
        rate_limit:  # Use 'rate_limit' alias for route overrides
          stat_prefix: users_create_rl
          token_bucket:
            max_tokens: 10
            tokens_per_fill: 10
            fill_interval_ms: 60000  # Only 10 creates per minute
```

## Filter Execution Order

When both global and route-level filters are defined:

1. **Global filters** are applied to the listener's HTTP connection manager
2. **Route-level overrides** are applied via Envoy's `typed_per_filter_config`
3. Route-level overrides take precedence over global filters

## Validation

The Platform API validates filter configurations during import:

- **Required fields**: Ensures all required fields are present
- **Type checking**: Validates field types (strings, numbers, booleans)
- **Enum validation**: Checks that enum values are valid
- **Structure validation**: Ensures nested objects match expected schema

If validation fails, the import returns a `400 Bad Request` with details about the error.

## Complete Example

See [openapi-with-x-flowplane-filters.yaml](./openapi-with-x-flowplane-filters.yaml) for a complete working example that demonstrates:

- Global CORS, header mutation, and rate limiting
- Public endpoints with disabled authentication
- Protected endpoints with JWT validation
- Admin endpoints with custom CORS
- Webhook endpoints with signature validation
- Per-route rate limit overrides

## Testing Your Configuration

After importing your OpenAPI spec with filters, you can verify the configuration:

1. **Check the API definition**:
   ```bash
   curl "http://localhost:8080/api/v1/api-definitions/{id}" \
     -H "Authorization: Bearer YOUR_TOKEN"
   ```

2. **Download the bootstrap config**:
   ```bash
   curl "http://localhost:8080/api/v1/api-definitions/{id}/bootstrap" \
     -H "Authorization: Bearer YOUR_TOKEN" > bootstrap.json
   ```

3. **Verify filters are present**:
   ```bash
   jq '.static_resources.listeners[] | select(.name | contains("platform")) | .filter_chains[].filters[] | select(.name == "envoy.filters.network.http_connection_manager") | .typed_config.http_filters' bootstrap.json
   ```

## Troubleshooting

### Filters not appearing in config

- Ensure you used `listenerIsolation=true` in the import URL
- Check that the filter syntax is correct (YAML indentation, field names)
- Verify the OpenAPI spec is valid (use `openapi: 3.0.0` format)

### Rate limiting not working

- Ensure `stat_prefix` is provided (required field)
- Check `fill_interval_ms` is in milliseconds, not seconds
- Verify `token_bucket` is properly configured

### CORS not working

- Ensure `policy` wrapper is present around CORS config
- Check that `allow_origin` is an array of objects with `type` and `value`
- Verify origin exactly matches (including protocol and port)

### Error: "Unsupported filter override 'local_rate_limit'"

This error occurs when you use the **filter type name** instead of the **route override alias** for local rate limiting.

**Problem:**
```yaml
# ❌ WRONG - 'local_rate_limit' is the filter type name, not the route override alias
x-flowplane-route-overrides:
  local_rate_limit:
    stat_prefix: my_rl
    token_bucket:
      max_tokens: 100
```

**Solution:**
```yaml
# ✅ CORRECT - use the 'rate_limit' alias for route overrides
x-flowplane-route-overrides:
  rate_limit:  # Shortened alias
    stat_prefix: my_rl
    token_bucket:
      max_tokens: 100
```

**Why this happens:**
- The actual Envoy filter is named `local_rate_limit` (with "local_")
- For **global filters**, you use `type: local_rate_limit` (matches the filter name)
- For **route overrides**, you use the alias `rate_limit` (shortened for cleaner syntax)

See the [Filter Alias Reference](#filter-alias-reference) section for the complete mapping.

## References

- [Platform API Documentation](../docs/platform-api.md)
- [HTTP Filter Reference](../docs/filters.md)
- [OpenAPI 3.0 Specification](https://swagger.io/specification/)
