# Local Rate Limit Filter

The Local Rate Limit filter provides in-process rate limiting using a token bucket algorithm. It limits the number of requests that can be processed per unit of time, protecting upstream services from being overwhelmed. Unlike the distributed Rate Limit filter, this operates entirely within each Envoy instance without requiring an external rate limit service.

## Envoy Documentation

- [Local Rate Limit Filter Reference](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/local_rate_limit_filter)
- [Local Rate Limit Filter API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/local_ratelimit/v3/local_rate_limit.proto)

## How It Works in Envoy

The Local Rate Limit filter uses a token bucket algorithm to control request rates:

```
┌─────────┐     ┌─────────────────────────────────────────┐     ┌─────────────┐
│  User   │     │                 Envoy                   │     │  Upstream   │
└────┬────┘     │  ┌─────────────────────────────────┐   │     └──────┬──────┘
     │          │  │         Token Bucket            │   │            │
     │          │  │  ┌───┬───┬───┬───┬───┬───┐     │   │            │
     │          │  │  │ ● │ ● │ ● │ ● │ ○ │ ○ │     │   │            │
     │          │  │  └───┴───┴───┴───┴───┴───┘     │   │            │
     │          │  │    Available    │  Empty       │   │            │
     │          │  └─────────────────────────────────┘   │            │
     │          └───────────────────────────────────────┘            │
     │               │                                               │
     │ 1. Request    │                                               │
     ├──────────────►│                                               │
     │               │                                               │
     │               │ 2. Check bucket                               │
     │               │    Token available?                           │
     │               │                                               │
     │               │ 3a. YES: Consume token                        │
     │               │     Forward request ──────────────────────────►
     │               │                                               │
     │               │ 3b. NO: Reject with 429                       │
     │◄──────────────┤     Too Many Requests                         │
     │               │                                               │
     │               │ 4. Bucket refills over time                   │
     │               │    (tokens_per_fill / fill_interval)          │
```

### Key Behaviors

1. **Token Bucket Algorithm**: Each request consumes one token. When tokens are exhausted, requests are rejected with 429 (configurable)
2. **Per-Instance Limiting**: Rate limits apply per Envoy instance, not globally across a cluster
3. **Configurable Refill**: Tokens refill at a specified rate (e.g., 10 tokens every 60 seconds)
4. **Percentage-Based Enabling**: Filter can be enabled/enforced for a percentage of requests
5. **Response Headers**: Optional headers showing rate limit status (`x-ratelimit-limit`, `x-ratelimit-remaining`)

### Per-Route Support

**The Local Rate Limit filter supports per-route configuration** via `typed_per_filter_config`. You can:
- Override rate limits for specific routes (stricter or more permissive)
- Disable rate limiting entirely for certain routes
- Apply different rate limit policies to different virtual hosts

## Flowplane Configuration

### Filter Definition

Create a reusable filter definition with base configuration:

```json
{
  "name": "local-rate-limit-filter",
  "filterType": "local_rate_limit",
  "description": "Rate limit filter - 10 req/min base",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "local_rate_limit",
      "token_bucket": {
        "max_tokens": 10,
        "tokens_per_fill": 10,
        "fill_interval_ms": 60000
      },
      "status_code": 429,
      "filter_enabled": {
        "numerator": 100,
        "denominator": "hundred"
      },
      "filter_enforced": {
        "numerator": 100,
        "denominator": "hundred"
      }
    }
  },
  "team": "my-team"
}
```

### Top-Level Configuration Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `stat_prefix` | string | Yes | - | Prefix for statistics emitted by the filter |
| `token_bucket` | object | Yes | - | Token bucket configuration |
| `status_code` | integer | No | `429` | HTTP status code when rate limited |
| `filter_enabled` | object | No | 100% | Percentage of requests to evaluate |
| `filter_enforced` | object | No | 100% | Percentage of evaluated requests to enforce |
| `response_headers_to_add` | array | No | `[]` | Headers to add on rate-limited responses |
| `request_headers_to_add_when_not_enforced` | array | No | `[]` | Headers added when limit hit but not enforced |
| `enable_x_ratelimit_headers` | string | No | `"OFF"` | Enable standard rate limit headers (`OFF`, `DRAFT_VERSION_03`) |
| `local_rate_limit_per_downstream_connection` | boolean | No | `false` | Apply limit per connection instead of globally |
| `local_cluster_rate_limit` | object | No | - | Cluster-aware rate limiting configuration |
| `descriptors` | array | No | `[]` | Rate limit descriptors for differentiated limiting |
| `stage` | integer | No | `0` | Filter processing stage |
| `always_consume_default_token_bucket` | boolean | No | `true` | Consume from default bucket even when descriptors match |
| `rate_limited_as_resource_exhausted` | boolean | No | `false` | Use gRPC RESOURCE_EXHAUSTED status |

### Token Bucket Configuration

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `max_tokens` | integer | Yes | - | Maximum tokens in the bucket |
| `tokens_per_fill` | integer | Yes | - | Tokens added per fill interval |
| `fill_interval_ms` | integer | Yes | - | Interval between refills in milliseconds |

### Percentage Configuration

Used for `filter_enabled` and `filter_enforced`:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `numerator` | integer | Yes | - | Numerator of the percentage |
| `denominator` | string | Yes | - | Denominator: `"hundred"`, `"ten_thousand"`, or `"million"` |

### Rate Limit Headers

When `enable_x_ratelimit_headers` is set to `"DRAFT_VERSION_03"`, Envoy adds:

| Header | Description |
|--------|-------------|
| `x-ratelimit-limit` | Maximum requests allowed in the window |
| `x-ratelimit-remaining` | Remaining requests in current window |
| `x-ratelimit-reset` | Seconds until the bucket refills |

## Filter Installation Workflow

### Step 1: Create the Filter Definition

```bash
curl -X POST http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "local-rate-limit-filter",
    "filterType": "local_rate_limit",
    "description": "Rate limit filter - 10 req/min base, can be overridden per-route",
    "config": {
      "type": "local_rate_limit",
      "config": {
        "stat_prefix": "local_rate_limit",
        "token_bucket": {
          "max_tokens": 10,
          "tokens_per_fill": 10,
          "fill_interval_ms": 60000
        },
        "status_code": 429,
        "filter_enabled": {
          "numerator": 100,
          "denominator": "hundred"
        },
        "filter_enforced": {
          "numerator": 100,
          "denominator": "hundred"
        }
      }
    },
    "team": "my-team"
  }'
```

### Step 2: Create Supporting Resources

Create the cluster for your backend:

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ratelimit-test-cluster",
    "team": "my-team",
    "connectTimeout": "5s",
    "loadBalancingPolicy": "round_robin",
    "endpoints": [
      {"host": "localhost", "port": 8001}
    ]
  }'
```

Create the route configuration:

```bash
curl -X POST http://localhost:8080/api/v1/route-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ratelimit-test-routes",
    "team": "my-team",
    "virtualHosts": [{
      "name": "ratelimit-test-vhost",
      "domains": ["ratelimit-test.local"],
      "routes": [
        {
          "name": "high-limit-route",
          "match": {"path": {"type": "prefix", "value": "/high/limit"}},
          "action": {
            "type": "forward",
            "cluster": "ratelimit-test-cluster",
            "timeoutSeconds": 30,
            "prefixRewrite": "/get"
          }
        },
        {
          "name": "low-limit-route",
          "match": {"path": {"type": "prefix", "value": "/low/limit"}},
          "action": {
            "type": "forward",
            "cluster": "ratelimit-test-cluster",
            "timeoutSeconds": 30,
            "prefixRewrite": "/get"
          }
        }
      ]
    }]
  }'
```

Create the listener:

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ratelimit-test-listener",
    "address": "0.0.0.0",
    "port": 10050,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "ratelimit-test-routes",
        "httpFilters": [
          {"filter": {"type": "router"}}
        ]
      }]
    }]
  }'
```

### Step 3: Install Filter on Listener

```bash
curl -X POST http://localhost:8080/api/v1/filters/{filter_id}/installations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "listenerName": "ratelimit-test-listener",
    "order": 1
  }'
```

### Step 4: Configure Filter at Route-Config Level

Apply the filter configuration to all routes in the route-config:

```bash
curl -X POST http://localhost:8080/api/v1/filters/{filter_id}/configurations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "scopeType": "route-config",
    "scopeId": "ratelimit-test-routes"
  }'
```

This makes all routes inherit the base configuration (10 tokens/min).

### Step 5: Override for Specific Routes

Apply a stricter limit to the `/low/limit` route:

```bash
curl -X POST http://localhost:8080/api/v1/filters/{filter_id}/configurations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "scopeType": "route",
    "scopeId": "ratelimit-test-routes/ratelimit-test-vhost/low-limit-route",
    "settings": {
      "behavior": "override",
      "config": {
        "stat_prefix": "low_limit_override",
        "token_bucket": {
          "max_tokens": 5,
          "tokens_per_fill": 5,
          "fill_interval_ms": 60000
        },
        "status_code": 429,
        "filter_enabled": {
          "numerator": 100,
          "denominator": "hundred"
        },
        "filter_enforced": {
          "numerator": 100,
          "denominator": "hundred"
        }
      }
    }
  }'
```

### Result

After this configuration:

| Route | Rate Limit | Source |
|-------|-----------|--------|
| `/high/limit` | 10 tokens/min | Inherited from route-config |
| `/low/limit` | 5 tokens/min | Route-level override |

## Testing Rate Limits

### Test the High Limit Route

```bash
curl -v http://localhost:10050/high/limit \
  -H "Host: ratelimit-test.local"
```

Expected: 10 requests per minute allowed (inherited from route-config).

### Test the Low Limit Route

```bash
curl -v http://localhost:10050/low/limit \
  -H "Host: ratelimit-test.local"
```

Expected: 5 requests per minute allowed (route override).

### Burst Test Script

```bash
#!/bin/bash
# Test rate limiting by sending requests in quick succession

ROUTE=$1  # "high" or "low"
HOST="ratelimit-test.local"
URL="http://localhost:10050/${ROUTE}/limit"

echo "Testing rate limit on ${ROUTE} route..."
for i in {1..15}; do
  STATUS=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: ${HOST}" "${URL}")
  echo "Request $i: HTTP $STATUS"
done
```

## Complete Examples

### Basic Rate Limiting (100 req/sec)

```json
{
  "name": "high-throughput-limiter",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "high_throughput",
      "token_bucket": {
        "max_tokens": 100,
        "tokens_per_fill": 100,
        "fill_interval_ms": 1000
      },
      "status_code": 429,
      "enable_x_ratelimit_headers": "DRAFT_VERSION_03"
    }
  },
  "team": "my-team"
}
```

### Gradual Rollout (50% Enforcement)

```json
{
  "name": "gradual-limiter",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "gradual_rollout",
      "token_bucket": {
        "max_tokens": 10,
        "tokens_per_fill": 10,
        "fill_interval_ms": 60000
      },
      "filter_enabled": {
        "numerator": 100,
        "denominator": "hundred"
      },
      "filter_enforced": {
        "numerator": 50,
        "denominator": "hundred"
      },
      "request_headers_to_add_when_not_enforced": [
        {"header": {"key": "x-rate-limit-would-block", "value": "true"}}
      ]
    }
  },
  "team": "my-team"
}
```

### Per-Connection Rate Limiting

```json
{
  "name": "per-connection-limiter",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "per_connection",
      "token_bucket": {
        "max_tokens": 50,
        "tokens_per_fill": 50,
        "fill_interval_ms": 1000
      },
      "local_rate_limit_per_downstream_connection": true
    }
  },
  "team": "my-team"
}
```

### Custom Response Headers

```json
{
  "name": "custom-response-limiter",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "custom_response",
      "token_bucket": {
        "max_tokens": 10,
        "tokens_per_fill": 10,
        "fill_interval_ms": 60000
      },
      "status_code": 429,
      "response_headers_to_add": [
        {"header": {"key": "x-rate-limited", "value": "true"}},
        {"header": {"key": "retry-after", "value": "60"}}
      ]
    }
  },
  "team": "my-team"
}
```

## Per-Route Configuration

### Disable Rate Limiting for a Route

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.local_ratelimit": {
      "filter_type": "local_rate_limit",
      "disabled": true
    }
  }
}
```

### Override with Stricter Limit

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.local_ratelimit": {
      "filter_type": "local_rate_limit",
      "stat_prefix": "api_route",
      "token_bucket": {
        "max_tokens": 5,
        "tokens_per_fill": 5,
        "fill_interval_ms": 60000
      }
    }
  }
}
```

### Override with Higher Limit (Premium Tier)

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.local_ratelimit": {
      "filter_type": "local_rate_limit",
      "stat_prefix": "premium_tier",
      "token_bucket": {
        "max_tokens": 1000,
        "tokens_per_fill": 1000,
        "fill_interval_ms": 60000
      }
    }
  }
}
```

## Troubleshooting

### Common Issues

1. **Rate Limits Not Applied**

   Check filter installation:
   ```bash
   curl -s "http://localhost:9902/config_dump?resource=dynamic_listeners" | \
     jq '.configs[].active_state.listener.filter_chains[].filters[].typed_config.http_filters[] | select(.name | contains("local_ratelimit"))'
   ```

2. **Wrong Rate Limit on Route**

   Verify per-route configuration:
   ```bash
   curl -s "http://localhost:9902/config_dump?resource=dynamic_route_configs" | \
     jq '.configs[].route_config.virtual_hosts[].routes[].typed_per_filter_config'
   ```

3. **Filter Not Enforcing**

   Check `filter_enabled` and `filter_enforced` percentages. Both must be 100% for full enforcement.

4. **Unexpected 429 Responses**

   - Check if multiple Envoy instances share load (limits are per-instance)
   - Verify `fill_interval_ms` is set correctly (milliseconds, not seconds)

5. **No Rate Limit Headers**

   Ensure `enable_x_ratelimit_headers` is set to `"DRAFT_VERSION_03"`:
   ```json
   "enable_x_ratelimit_headers": "DRAFT_VERSION_03"
   ```

### Debug Checklist

```bash
# 1. Check filter stats
curl -s "http://localhost:9902/stats" | grep local_rate_limit

# 2. Verify listener configuration
curl -s "http://localhost:9902/config_dump?resource=dynamic_listeners" | \
  jq '.configs[].active_state.listener.name'

# 3. Check route configuration
curl -s "http://localhost:9902/config_dump?resource=dynamic_route_configs" | \
  jq '.configs[].route_config.virtual_hosts[].routes[].name'

# 4. Test with verbose output
curl -v http://localhost:10050/your/route -H "Host: your-domain.local"
```

### Metrics

With local rate limiting enabled, Envoy emits metrics:

| Metric | Description |
|--------|-------------|
| `{stat_prefix}.http_local_rate_limit.enabled` | Requests evaluated by the filter |
| `{stat_prefix}.http_local_rate_limit.enforced` | Requests where limit was enforced |
| `{stat_prefix}.http_local_rate_limit.ok` | Requests allowed |
| `{stat_prefix}.http_local_rate_limit.rate_limited` | Requests rate limited |

## Local vs Distributed Rate Limiting

| Feature | Local Rate Limit | Rate Limit (Distributed) |
|---------|------------------|--------------------------|
| Scope | Per Envoy instance | Global across cluster |
| External Service | Not required | Required (rate limit service) |
| Accuracy | Approximate (per-instance) | Exact (centralized) |
| Latency | Very low | Additional network hop |
| Complexity | Simple | More complex |
| Use Case | Per-instance protection | Global quotas, API rate limiting |

**When to use Local Rate Limit:**
- Protecting individual instances from overload
- Simple rate limiting without external dependencies
- Low-latency requirements
- Development and testing

**When to use Distributed Rate Limit:**
- Global API quotas (e.g., 1000 req/day per user)
- Consistent limits across all instances
- Multi-tenant rate limiting
- Billing/usage enforcement

## Security Considerations

1. **Instance Scaling**: Total cluster capacity = instances × per-instance limit. Account for this when setting limits.
2. **Burst Protection**: Set `max_tokens` equal to `tokens_per_fill` to prevent burst allowance.
3. **Gradual Rollout**: Use `filter_enforced` < 100% to monitor impact before full enforcement.
4. **Monitoring**: Always enable stats to detect rate limiting issues.
5. **Fail Open vs Closed**: The filter fails open by default. Consider your security requirements.

## See Also

- [Filters Overview](../filters.md) - All available filters
- [Rate Limit Filter](./rate_limit.md) - Distributed rate limiting with external service
- [Header Mutation](./header_mutation.md) - Add custom headers to requests/responses
