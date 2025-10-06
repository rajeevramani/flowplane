# x-flowplane Extensions Quick Reference

## Import Command

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=TEAM&listenerIsolation=true" \
  -H "Content-Type: application/yaml" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  --data-binary @your-spec.yaml
```

⚠️ **Important**: Must use `listenerIsolation=true` for filters to work!

---

## Global Filters

Applied to ALL routes on the API's isolated listener.

```yaml
x-flowplane-filters:
  - filter:
      type: <filter_type>
      <config>
```

### Supported Global Filter Types

| Type | Purpose | Key Fields |
|------|---------|------------|
| `cors` | CORS policy | `policy.allow_origin`, `allow_methods`, `allow_headers` |
| `header_mutation` | Add/remove headers | `request_headers_to_add`, `response_headers_to_add` |
| `local_rate_limit` | Rate limiting | `stat_prefix`, `token_bucket` |

---

## Route-Level Overrides

Applied to specific routes/operations.

```yaml
paths:
  /api/endpoint:
    get:
      x-flowplane-route-overrides:
        <alias>: <config>
```

### Supported Route Override Aliases

| Alias | Purpose | Disable | Custom Config |
|-------|---------|---------|---------------|
| `cors` | CORS override | `cors: disabled` | `cors: { policy: {...} }` |
| `authn` | JWT auth | `authn: disabled` | `authn: jwt-config-name` |
| `rate_limit` | Rate limit | ❌ | `rate_limit: { stat_prefix: ..., token_bucket: {...} }` |

⚠️ **Note**: Use `rate_limit` for route overrides, NOT `local_rate_limit`!

---

## Common Patterns

### Pattern 1: Public Health Check

```yaml
/health:
  get:
    x-flowplane-route-overrides:
      authn: disabled
      cors: disabled
      rate_limit:
        stat_prefix: health_rl
        token_bucket:
          max_tokens: 1000
          tokens_per_fill: 1000
          fill_interval_ms: 60000
```

### Pattern 2: Strict Write Operation

```yaml
/users:
  post:
    x-flowplane-route-overrides:
      rate_limit:
        stat_prefix: users_create_rl
        token_bucket:
          max_tokens: 10
          tokens_per_fill: 10
          fill_interval_ms: 60000
```

### Pattern 3: Admin-Only with Custom CORS

```yaml
/admin/settings:
  get:
    x-flowplane-route-overrides:
      authn: admin-only
      cors:
        policy:
          allow_origin:
            - type: exact
              value: "https://admin.example.com"
          allow_credentials: true
```

---

## CORS Configuration

### Global CORS (Allow All)

```yaml
x-flowplane-filters:
  - filter:
      type: cors
      policy:
        allow_origin:
          - type: exact
            value: "*"
        allow_methods:
          - GET
          - POST
          - PUT
          - DELETE
        allow_headers:
          - content-type
          - authorization
```

### Route CORS (Specific Origin)

```yaml
x-flowplane-route-overrides:
  cors:
    policy:
      allow_origin:
        - type: exact
          value: "https://app.example.com"
      allow_methods:
        - GET
      allow_credentials: true
```

---

## Header Mutation

⚠️ **Only available as global filter**

```yaml
x-flowplane-filters:
  - filter:
      type: header_mutation
      request_headers_to_add:
        - key: x-api-version
          value: "v1"
          append: false
      response_headers_to_add:
        - key: x-powered-by
          value: "flowplane"
          append: false
```

---

## Rate Limiting

### Global Rate Limit

```yaml
x-flowplane-filters:
  - filter:
      type: local_rate_limit
      stat_prefix: global_rl
      token_bucket:
        max_tokens: 100
        tokens_per_fill: 100
        fill_interval_ms: 60000  # 100 per minute
```

### Route-Specific Rate Limit

```yaml
x-flowplane-route-overrides:
  rate_limit:  # ⚠️ NOT local_rate_limit!
    stat_prefix: route_rl
    token_bucket:
      max_tokens: 10
      tokens_per_fill: 10
      fill_interval_ms: 60000  # 10 per minute
```

**Common Intervals:**
- 1 second = `1000`
- 1 minute = `60000`
- 5 minutes = `300000`
- 1 hour = `3600000`

---

## Common Errors

### ❌ "Unsupported filter override 'local_rate_limit'"

```yaml
# Wrong
x-flowplane-route-overrides:
  local_rate_limit: {...}

# Correct
x-flowplane-route-overrides:
  rate_limit: {...}
```

### ❌ "Unsupported filter override 'header_mutation'"

```yaml
# Wrong - route-level not supported
x-flowplane-route-overrides:
  header_mutation: {...}

# Correct - use global
x-flowplane-filters:
  - filter:
      type: header_mutation
      ...
```

### ❌ "filters override must be a JSON object"

```yaml
# Wrong - array format
x-flowplane-route-overrides:
  - filter:
      type: cors

# Correct - object format
x-flowplane-route-overrides:
  cors:
    policy: {...}
```

### ❌ Filters not appearing in config

- Did you use `listenerIsolation=true`?
- Check camelCase: `listenerIsolation` not `listener_isolation`
- Verify YAML syntax is correct

---

## Testing

```bash
# Get the API ID from import response
API_ID="..."

# Download bootstrap config
curl "http://localhost:8080/api/v1/api-definitions/${API_ID}/bootstrap" \
  | jq > bootstrap.json

# Find assigned port
PORT=$(jq -r '.static_resources.listeners[] |
  select(.name | contains("platform")) |
  .address.socket_address.port_value' bootstrap.json)

# Test endpoint
curl "http://localhost:${PORT}/get"
```

---

## Files

- **httpbin-demo.yaml** - Complete working example
- **SUPPORTED-OVERRIDES.md** - Detailed override documentation
- **HTTPBIN-TESTING.md** - Step-by-step testing guide
- **README-x-flowplane-extensions.md** - Full extension reference
