# Supported x-flowplane Route Overrides

This document lists the currently supported route-level filter overrides in the `x-flowplane-route-overrides` extension.

## Overview

Route-level overrides allow you to customize or disable specific filters for individual routes. These are configured using the `x-flowplane-route-overrides` extension on operation objects (GET, POST, etc.).

## Supported Override Aliases

### 1. `cors` - CORS Filter Override

Control CORS behavior for specific routes.

#### Disable CORS

```yaml
x-flowplane-route-overrides:
  cors: disabled
```

#### Custom CORS Policy

```yaml
x-flowplane-route-overrides:
  cors:
    policy:
      allow_origin:
        - type: exact
          value: "https://admin.example.com"
      allow_methods:
        - GET
        - POST
      allow_credentials: true
```

**Use Cases:**
- Disable CORS for public health check endpoints
- Restrict CORS to specific origins for admin endpoints
- Override global CORS for webhook endpoints

---

### 2. `authn` - JWT Authentication Override

Control JWT authentication for specific routes.

#### Disable Authentication

```yaml
x-flowplane-route-overrides:
  authn: disabled
```

#### Use Named JWT Configuration

```yaml
x-flowplane-route-overrides:
  authn: jwt-admin-only
```

**Use Cases:**
- Disable auth for public endpoints (health, metrics)
- Use different JWT providers for different route groups
- Require elevated permissions for admin routes

---

### 3. `rate_limit` - Rate Limiting Override

⚠️ **Important**: For route-level overrides, use `rate_limit` (NOT `local_rate_limit`)

#### Custom Rate Limit

```yaml
x-flowplane-route-overrides:
  rate_limit:
    stat_prefix: custom_rl
    token_bucket:
      max_tokens: 10
      tokens_per_fill: 10
      fill_interval_ms: 60000  # 10 requests per minute
```

**Required Fields:**
- `stat_prefix`: Statistics prefix (string, required)
- `token_bucket`: Token bucket configuration (required)
  - `max_tokens`: Maximum tokens (integer, required)
  - `tokens_per_fill`: Tokens added per refill (integer, optional, defaults to max_tokens)
  - `fill_interval_ms`: Refill interval in milliseconds (integer, required)

**Optional Fields:**
- `status_code`: HTTP status code to return when rate limited (default: 429)

**Use Cases:**
- Stricter limits for write operations (POST, PUT, DELETE)
- Very permissive limits for health checks
- Restrictive limits for expensive operations (delay, large queries)

---

### 4. `header_mutation` - Header Mutation Override

Customize headers for specific routes.

#### Custom Header Mutation

```yaml
x-flowplane-route-overrides:
  header_mutation:
    request_headers_to_add:
      - key: x-route-id
        value: admin-route
        append: false
    request_headers_to_remove:
      - x-debug
    response_headers_to_add:
      - key: x-cache-status
        value: hit
        append: false
    response_headers_to_remove:
      - server
```

**Required Fields:**
- At least one of: `request_headers_to_add`, `request_headers_to_remove`, `response_headers_to_add`, or `response_headers_to_remove`

**Use Cases:**
- Add route-specific tracking headers
- Remove sensitive headers for public endpoints
- Add custom cache headers per route

---

### 5. `ratelimit` - Distributed Rate Limit Override

⚠️ **Note**: Use `ratelimit` for distributed rate limiting (not `rate_limit` which is for local rate limiting)

#### Custom Distributed Rate Limit

```yaml
x-flowplane-route-overrides:
  ratelimit:
    stage: 0
    disable_key: disabled
```

**Use Cases:**
- Override distributed rate limit behavior per route
- Disable distributed rate limiting for specific endpoints
- Use different rate limit stages for different routes

---

### 6. `rate_limit_quota` - Rate Limit Quota Override

Control quota-based rate limiting for specific routes.

#### Custom Rate Limit Quota

```yaml
x-flowplane-route-overrides:
  rate_limit_quota:
    domain: premium-users
```

**Required Fields:**
- `domain`: Quota domain (string, required)

**Use Cases:**
- Different quota buckets for premium vs free tiers
- Route-specific quota allocation
- Per-endpoint quota limits

---

## NOT Currently Supported

The following are **NOT** supported as route-level overrides:

### ❌ `local_rate_limit` (at route level)
Use `rate_limit` instead for local rate limiting route-level overrides.

---

## Complete Examples

### Example 1: Public Health Check

```yaml
paths:
  /health:
    get:
      x-flowplane-route-overrides:
        # No authentication required
        authn: disabled
        # No CORS restrictions
        cors: disabled
        # Very permissive rate limit
        rate_limit:
          stat_prefix: health_rl
          token_bucket:
            max_tokens: 1000
            tokens_per_fill: 1000
            fill_interval_ms: 60000
```

### Example 2: Strict Write Endpoint

```yaml
paths:
  /users:
    post:
      x-flowplane-route-overrides:
        # Require JWT authentication
        authn: jwt-required
        # Strict rate limit for creates
        rate_limit:
          stat_prefix: users_create_rl
          token_bucket:
            max_tokens: 10
            tokens_per_fill: 10
            fill_interval_ms: 60000  # Only 10 creates per minute
```

### Example 3: Admin-Only Endpoint

```yaml
paths:
  /admin/settings:
    get:
      x-flowplane-route-overrides:
        # Admin JWT required
        authn: admin-only
        # Only allow requests from admin panel
        cors:
          policy:
            allow_origin:
              - type: exact
                value: "https://admin.example.com"
            allow_credentials: true
        # Moderate rate limit
        rate_limit:
          stat_prefix: admin_settings_rl
          token_bucket:
            max_tokens: 50
            tokens_per_fill: 50
            fill_interval_ms: 60000
```

### Example 4: Webhook with Signature Auth

```yaml
paths:
  /webhooks/github:
    post:
      x-flowplane-route-overrides:
        # Use webhook signature validation instead of JWT
        authn: webhook-signature
        # No CORS for webhooks
        cors: disabled
        # Moderate rate limit
        rate_limit:
          stat_prefix: webhook_rl
          token_bucket:
            max_tokens: 100
            tokens_per_fill: 100
            fill_interval_ms: 60000
```

---

## Error Messages

### "Unsupported filter override 'local_rate_limit'"

**Problem**: Used `local_rate_limit` instead of `rate_limit` in route overrides.

**Solution**: Change to `rate_limit`:
```yaml
# ❌ Wrong
x-flowplane-route-overrides:
  local_rate_limit: {...}

# ✅ Correct
x-flowplane-route-overrides:
  rate_limit: {...}
```

### "Unsupported filter override 'header_mutation'"

**Problem**: Header mutation is not supported at route level.

**Solution**: Use global `x-flowplane-filters` instead:
```yaml
# ❌ Wrong (route-level)
paths:
  /api:
    get:
      x-flowplane-route-overrides:
        header_mutation: {...}

# ✅ Correct (global)
x-flowplane-filters:
  - filter:
      type: header_mutation
      request_headers_to_add: [...]
```

### "filters override must be a JSON object keyed by filter name"

**Problem**: Route overrides must be an object, not an array.

**Solution**:
```yaml
# ❌ Wrong (array format)
x-flowplane-route-overrides:
  - filter:
      type: cors
      ...

# ✅ Correct (object format)
x-flowplane-route-overrides:
  cors:
    policy: {...}
```

---

## Future Enhancements

The following filter overrides may be added in future releases:

- `header_mutation` - Route-level header manipulation
- `retry` - Route-specific retry policies
- `timeout` - Per-route timeout configuration
- `compression` - Route-level compression settings
- `lua` - Custom Lua filters per route

---

## See Also

- [httpbin-demo.yaml](./httpbin-demo.yaml) - Complete working example
- [README-x-flowplane-extensions.md](./README-x-flowplane-extensions.md) - Full extension guide
- [HTTPBIN-TESTING.md](./HTTPBIN-TESTING.md) - Testing guide
