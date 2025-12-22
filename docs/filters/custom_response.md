# Custom Response Filter

The Custom Response filter intercepts HTTP responses based on status codes and replaces them with custom responses. This is useful for providing user-friendly error pages, standardizing error formats across services, or adding additional context to error responses.

## Envoy Documentation

- [Custom Response Filter Reference](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/custom_response_filter)
- [Custom Response Filter API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/custom_response/v3/custom_response.proto)

## How It Works

The custom_response filter inspects upstream responses and matches them against configured rules:

```
┌─────────┐     ┌─────────┐     ┌─────────────┐
│  User   │     │  Envoy  │     │  Upstream   │
└────┬────┘     └────┬────┘     └──────┬──────┘
     │               │                 │
     │ 1. Request    │                 │
     ├──────────────►│                 │
     │               │ 2. Forward      │
     │               ├────────────────►│
     │               │                 │
     │               │ 3. Response     │
     │               │   (e.g., 503)   │
     │               │◄────────────────┤
     │               │                 │
     │               │ 4. Match status │
     │               │    code rules   │
     │               │                 │
     │ 5. Custom     │                 │
     │   Response    │                 │
     │   (friendly   │                 │
     │    JSON)      │                 │
     │◄──────────────┤                 │
```

### Key Features

1. **Status Code Matching**: Match exact codes, ranges, or lists of status codes
2. **Custom Bodies**: Return custom response bodies (JSON, HTML, plain text)
3. **Header Injection**: Add custom headers to the response
4. **Status Code Override**: Optionally change the response status code
5. **Per-Route Overrides**: Different response rules for different routes

## Flowplane Configuration

The custom_response filter uses the **Filter Management API**. You create a named filter, then install it on listeners.

### Filter Configuration Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `matchers` | array | Yes | List of matcher rules defining which responses to customize |

### Matcher Rule Structure

Each matcher rule has two parts:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `status_code` | object | Yes | Status code matching criteria |
| `response` | object | Yes | Custom response to return when matched |

### Status Code Matchers

#### Exact Match

Match a specific status code:

```json
{
  "status_code": {
    "type": "exact",
    "code": 503
  }
}
```

#### Range Match

Match a range of status codes (inclusive):

```json
{
  "status_code": {
    "type": "range",
    "min": 500,
    "max": 599
  }
}
```

#### List Match

Match any of the specified status codes:

```json
{
  "status_code": {
    "type": "list",
    "codes": [400, 401, 403, 429]
  }
}
```

### Response Policy

Define the custom response to return:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `status_code` | integer | No | Original | Override the response status code |
| `body` | string | No | - | Custom response body |
| `headers` | object | No | `{}` | Headers to add to the response |

```json
{
  "response": {
    "status_code": 503,
    "body": "{\"error\": \"Service temporarily unavailable\", \"retry_after\": 30}",
    "headers": {
      "content-type": "application/json",
      "retry-after": "30"
    }
  }
}
```

## Complete Example: Error Response Standardization

This example demonstrates setting up custom_response to provide friendly JSON error messages for server errors.

### Step 1: Create the Custom Response Filter

Create the filter using the Filter Management API:

```bash
curl -X POST http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "friendly-errors-filter",
    "filterType": "custom_response",
    "description": "Custom response filter - friendly JSON errors for 5xx",
    "team": "my-team",
    "config": {
      "type": "custom_response",
      "config": {
        "matchers": [
          {
            "status_code": {"type": "range", "min": 500, "max": 599},
            "response": {
              "status_code": 500,
              "body": "{\"error\": \"Internal server error\", \"message\": \"Please try again later\"}",
              "headers": {"content-type": "application/json"}
            }
          }
        ]
      }
    }
  }'
```

Response includes the filter ID:

```json
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "name": "friendly-errors-filter",
  "filterType": "custom_response",
  ...
}
```

### Step 2: Create Backend Cluster

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-cluster",
    "team": "my-team",
    "endpoints": [
      {"host": "backend.example.com", "port": 8080}
    ]
  }'
```

### Step 3: Create Route Configuration

```bash
curl -X POST http://localhost:8080/api/v1/route-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-routes",
    "team": "my-team",
    "virtualHosts": [
      {
        "name": "api-vhost",
        "domains": ["api.example.com"],
        "routes": [
          {
            "name": "api-route",
            "match": {"path": {"type": "prefix", "value": "/"}},
            "action": {"type": "forward", "cluster": "backend-cluster"}
          }
        ]
      }
    ]
  }'
```

### Step 4: Create Listener

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-listener",
    "address": "0.0.0.0",
    "port": 10080,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [
      {
        "name": "default",
        "filters": [
          {
            "name": "envoy.filters.network.http_connection_manager",
            "type": "httpConnectionManager",
            "routeConfigName": "api-routes",
            "httpFilters": [
              {"filter": {"type": "router"}}
            ]
          }
        ]
      }
    ]
  }'
```

### Step 5: Install Filter on Listener

```bash
curl -X POST http://localhost:8080/api/v1/filters/{FILTER_ID}/installations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "listenerName": "api-listener",
    "order": 1
  }'
```

### Step 6: Configure at Route-Config Level

Apply the filter configuration to all routes in the route-config:

```bash
curl -X POST http://localhost:8080/api/v1/filters/{FILTER_ID}/configurations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "scopeType": "route-config",
    "scopeId": "api-routes"
  }'
```

Now all 5xx responses will be transformed into the friendly JSON format.

## Per-Route Overrides

You can configure different custom responses for specific routes, overriding the inherited configuration.

### Route-Level Override Example

Configure a specific route to handle rate limiting responses differently:

```bash
curl -X POST http://localhost:8080/api/v1/filters/{FILTER_ID}/configurations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "scopeType": "route",
    "scopeId": "api-routes/api-vhost/rate-limited-route",
    "settings": {
      "behavior": "override",
      "config": {
        "matchers": [
          {
            "status_code": {"type": "exact", "code": 429},
            "response": {
              "status_code": 429,
              "body": "{\"error\": \"Rate limit exceeded\", \"retry_after\": 60}",
              "headers": {
                "content-type": "application/json",
                "retry-after": "60"
              }
            }
          }
        ]
      }
    }
  }'
```

### Scope ID Format

The `scopeId` for route-level configurations uses the format:
```
{route-config-name}/{virtual-host-name}/{route-name}
```

## Common Use Cases

### Standardize All Error Responses

Convert all error responses to a consistent JSON format:

```json
{
  "matchers": [
    {
      "status_code": {"type": "range", "min": 400, "max": 499},
      "response": {
        "body": "{\"error\": \"Client error\", \"code\": 400}",
        "headers": {"content-type": "application/json"}
      }
    },
    {
      "status_code": {"type": "range", "min": 500, "max": 599},
      "response": {
        "status_code": 500,
        "body": "{\"error\": \"Server error\", \"code\": 500}",
        "headers": {"content-type": "application/json"}
      }
    }
  ]
}
```

### Rate Limiting Response

Provide helpful information when rate limited:

```json
{
  "matchers": [
    {
      "status_code": {"type": "exact", "code": 429},
      "response": {
        "status_code": 429,
        "body": "{\"error\": \"Too many requests\", \"message\": \"Please wait before retrying\", \"retry_after\": 60}",
        "headers": {
          "content-type": "application/json",
          "retry-after": "60"
        }
      }
    }
  ]
}
```

### Service Unavailable with Retry Header

```json
{
  "matchers": [
    {
      "status_code": {"type": "exact", "code": 503},
      "response": {
        "status_code": 503,
        "body": "{\"error\": \"Service temporarily unavailable\", \"message\": \"Maintenance in progress\"}",
        "headers": {
          "content-type": "application/json",
          "retry-after": "300"
        }
      }
    }
  ]
}
```

### Authentication Errors

Standardize authentication-related error responses:

```json
{
  "matchers": [
    {
      "status_code": {"type": "list", "codes": [401, 403]},
      "response": {
        "body": "{\"error\": \"Authentication required\", \"message\": \"Please provide valid credentials\"}",
        "headers": {
          "content-type": "application/json",
          "www-authenticate": "Bearer"
        }
      }
    }
  ]
}
```

## Managing Filters

### List All Filters

```bash
curl http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN"
```

### Get Filter Details

```bash
curl http://localhost:8080/api/v1/filters/{FILTER_ID} \
  -H "Authorization: Bearer $TOKEN"
```

### Update Filter Configuration

```bash
curl -X PUT http://localhost:8080/api/v1/filters/{FILTER_ID} \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "description": "Updated custom response filter",
    "config": {
      "type": "custom_response",
      "config": {
        "matchers": [
          {
            "status_code": {"type": "range", "min": 500, "max": 599},
            "response": {
              "status_code": 503,
              "body": "{\"error\": \"Service unavailable\", \"support\": \"support@example.com\"}",
              "headers": {"content-type": "application/json"}
            }
          }
        ]
      }
    }
  }'
```

### List Filter Configurations

```bash
curl http://localhost:8080/api/v1/filters/{FILTER_ID}/configurations \
  -H "Authorization: Bearer $TOKEN"
```

### Delete Configuration

```bash
curl -X DELETE "http://localhost:8080/api/v1/filters/{FILTER_ID}/configurations/{CONFIG_ID}" \
  -H "Authorization: Bearer $TOKEN"
```

### Uninstall Filter from Listener

```bash
curl -X DELETE "http://localhost:8080/api/v1/filters/{FILTER_ID}/installations?listenerName=api-listener" \
  -H "Authorization: Bearer $TOKEN"
```

### Delete Filter

```bash
curl -X DELETE http://localhost:8080/api/v1/filters/{FILTER_ID} \
  -H "Authorization: Bearer $TOKEN"
```

## Configuration Hierarchy

The custom_response filter supports hierarchical configuration:

```
Listener (filter installation)
    └── Route-Config (inherited by all routes)
            └── Route (override for specific route)
```

1. **Listener Level**: Filter is installed on the listener
2. **Route-Config Level**: Base configuration applied to all routes
3. **Route Level**: Override configuration for specific routes

Route-level configurations take precedence over route-config level configurations.

## Troubleshooting

### Custom Response Not Applied

1. **Check filter installation**:
   ```bash
   curl http://localhost:8080/api/v1/filters/{FILTER_ID}/installations \
     -H "Authorization: Bearer $TOKEN"
   ```

2. **Check filter configuration scope**:
   ```bash
   curl http://localhost:8080/api/v1/filters/{FILTER_ID}/configurations \
     -H "Authorization: Bearer $TOKEN"
   ```

3. **Verify Envoy config**:
   ```bash
   curl -s "http://localhost:9902/config_dump?resource=dynamic_listeners" | \
     jq '.configs[].active_state.listener.filter_chains[].filters[].typed_config.http_filters[] | select(.name == "envoy.filters.http.custom_response")'
   ```

### Status Code Not Matching

- Ensure the status code is within valid range (100-599)
- For range matchers, verify `min` <= `max`
- For list matchers, ensure the list is not empty

### Headers Not Added

- Header names and values cannot be empty
- Check that the `headers` object is properly formatted

## Validation Rules

| Field | Validation |
|-------|------------|
| `status_code` (in matcher) | Must be 100-599 |
| `status_code` (in response) | Must be 100-599 |
| Range `min`/`max` | min <= max, both 100-599 |
| List `codes` | Non-empty, all codes 100-599 |
| `body` | Cannot be empty string (use `null` to omit) |
| Header names | Cannot be empty |
| Header values | Cannot be empty |

## See Also

- [Filters Overview](../filters.md) - All available filters
- [External Authorization](./ext_authz.md) - External authorization filter
- [JWT Authentication](./jwt_authn.md) - JWT token validation
