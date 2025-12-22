# External Authorization Filter

The External Authorization (ext_authz) filter delegates authorization decisions to an external gRPC or HTTP service. This enables centralized policy enforcement, integration with existing authorization systems, and complex access control logic that would be difficult to express in Envoy's native configuration.

## Envoy Documentation

- [External Authorization Filter Reference](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/ext_authz_filter)
- [External Authorization Filter API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/ext_authz/v3/ext_authz.proto)

## How It Works in Envoy

The ext_authz filter intercepts requests and consults an external service before allowing them to proceed:

```
┌─────────┐     ┌─────────┐     ┌──────────────┐     ┌─────────────┐
│  User   │     │  Envoy  │     │ Authz Service│     │  Upstream   │
└────┬────┘     └────┬────┘     └──────┬───────┘     └──────┬──────┘
     │               │                 │                    │
     │ 1. Request    │                 │                    │
     ├──────────────►│                 │                    │
     │               │                 │                    │
     │               │ 2. Check        │                    │
     │               │   request       │                    │
     │               ├────────────────►│                    │
     │               │                 │                    │
     │               │ 3. Allow/Deny   │                    │
     │               │◄────────────────┤                    │
     │               │                 │                    │
     │               │     [If Allowed]│                    │
     │               │ 4. Forward      │                    │
     │               ├─────────────────────────────────────►│
     │               │                 │                    │
     │ 5. Response   │◄────────────────────────────────────┤
     │◄──────────────┤                 │                    │
     │               │                 │                    │
     │               │     [If Denied] │                    │
     │ 6. 403        │                 │                    │
     │◄──────────────┤                 │                    │
```

### Key Behaviors

1. **Service Types**: Supports both gRPC and HTTP authorization services
2. **Request Context**: Sends request attributes (headers, path, method) to the authz service
3. **Header Injection**: Authz service can add/modify headers on allowed requests
4. **Failure Mode**: Configure behavior when the authz service is unavailable
5. **Request Body**: Optionally buffer and send request body to authz service

### Per-Route Support

**The ext_authz filter supports per-route configuration** via `typedPerFilterConfig`. You can:
- Disable ext_authz for specific routes
- Pass custom context extensions to the authz service
- Control request body buffering per route

## Flowplane Configuration

The ext_authz filter uses the **Filter Management API**. You create a named filter, then install it on listeners.

### Filter Configuration Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `service` | object | Yes | - | Authorization service configuration (gRPC or HTTP) |
| `failure_mode_allow` | boolean | No | `false` | Allow requests if authz service fails |
| `with_request_body` | object | No | - | Request body buffering configuration |
| `clear_route_cache` | boolean | No | `false` | Clear route cache on successful authz |
| `status_on_error` | integer | No | - | HTTP status code when authz service errors |
| `stat_prefix` | string | No | - | Statistics prefix for metrics |
| `include_peer_certificate` | boolean | No | `false` | Include client certificate in authz request |

### Service Configuration

The `service` field must specify either a gRPC or HTTP authorization service.

#### gRPC Service

```json
{
  "service": {
    "type": "grpc",
    "target_uri": "authz-cluster",
    "timeout_ms": 200,
    "initial_metadata": [
      {"key": "x-custom-header", "value": "custom-value"}
    ]
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `type` | string | Yes | - | Must be `"grpc"` |
| `target_uri` | string | Yes | - | Cluster name for the gRPC authz service |
| `timeout_ms` | integer | No | `200` | Request timeout in milliseconds |
| `initial_metadata` | array | No | `[]` | Metadata headers sent with gRPC requests |

#### HTTP Service

```json
{
  "service": {
    "type": "http",
    "server_uri": {
      "uri": "http://authz.example.com/check",
      "cluster": "authz-cluster",
      "timeout_ms": 200
    },
    "path_prefix": "/authz",
    "headers_to_add": [
      {"key": "x-api-key", "value": "secret"}
    ],
    "authorization_request": {
      "allowed_headers": ["authorization", "x-request-id"],
      "headers_to_add": [{"key": "x-source", "value": "envoy"}]
    },
    "authorization_response": {
      "allowed_upstream_headers": ["x-user-id"],
      "allowed_client_headers": ["x-error-reason"]
    }
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `type` | string | Yes | - | Must be `"http"` |
| `server_uri.uri` | string | Yes | - | Full URI for the HTTP authz service |
| `server_uri.cluster` | string | Yes | - | Cluster name for the HTTP authz service |
| `server_uri.timeout_ms` | integer | No | `200` | Request timeout in milliseconds |
| `path_prefix` | string | No | `""` | Path prefix prepended to original request path |
| `headers_to_add` | array | No | `[]` | Headers to add to authorization requests |
| `authorization_request` | object | No | - | Configuration for the authz request |
| `authorization_response` | object | No | - | Configuration for handling authz response |

### Authorization Request Configuration

Controls what is sent to the authorization service:

| Field | Type | Description |
|-------|------|-------------|
| `allowed_headers` | array | Headers from the original request to include in the authz request |
| `headers_to_add` | array | Additional headers to add to the authz request |

### Authorization Response Configuration

Controls how the authorization response is processed:

| Field | Type | Description |
|-------|------|-------------|
| `allowed_upstream_headers` | array | Headers from authz response to add to the upstream request on success |
| `allowed_client_headers` | array | Headers from authz response to add to client response on denial |
| `allowed_client_headers_on_success` | array | Headers from authz response to add to client response on success |

### Request Body Configuration

Buffer and send request body to the authz service:

```json
{
  "with_request_body": {
    "max_request_bytes": 1024,
    "allow_partial_message": true,
    "pack_as_bytes": false
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_request_bytes` | integer | - | Maximum bytes to buffer from request body |
| `allow_partial_message` | boolean | `false` | Send partial body if max bytes exceeded |
| `pack_as_bytes` | boolean | `false` | Pack body as raw bytes instead of UTF-8 string |

## Complete Example: HTTP External Authorization

This example demonstrates setting up ext_authz with an HTTP authorization service.

### Step 1: Create Authorization Service Cluster

Create a cluster pointing to your authorization service:

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ext-authz-service",
    "team": "my-team",
    "endpoints": [
      {"host": "authz.example.com", "port": 8080}
    ]
  }'
```

### Step 2: Create the ext_authz Filter

Create the filter using the Filter Management API:

```bash
curl -X POST http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-ext-authz-filter",
    "filterType": "ext_authz",
    "description": "External authorization filter for API protection",
    "team": "my-team",
    "config": {
      "type": "ext_authz",
      "config": {
        "service": {
          "type": "http",
          "server_uri": {
            "uri": "http://ext-authz-service/check",
            "cluster": "ext-authz-service",
            "timeout_ms": 500
          },
          "path_prefix": "/authorize",
          "authorization_request": {
            "allowed_headers": ["authorization", "x-request-id", "x-forwarded-for"],
            "headers_to_add": []
          },
          "authorization_response": {
            "allowed_upstream_headers": ["x-user-id", "x-user-roles"],
            "allowed_client_headers": ["x-error-code"]
          }
        },
        "failure_mode_allow": false,
        "clear_route_cache": false,
        "stat_prefix": "ext_authz",
        "status_on_error": 403,
        "include_peer_certificate": false
      }
    }
  }'
```

Response includes the filter ID:

```json
{
  "id": "522325cf-8ef4-439b-9b03-cc1b2f4f3d1e",
  "name": "my-ext-authz-filter",
  "filterType": "ext_authz",
  ...
}
```

### Step 3: Create Backend Cluster

Create a cluster for your backend service:

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-service",
    "team": "my-team",
    "endpoints": [
      {"host": "backend.example.com", "port": 8080}
    ]
  }'
```

### Step 4: Create Route Configuration

Create the route configuration:

```bash
curl -X POST http://localhost:8080/api/v1/route-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-routes",
    "team": "my-team",
    "virtualHosts": [
      {
        "name": "api",
        "domains": ["*"],
        "routes": [
          {
            "name": "catch-all",
            "match": {"path": {"type": "prefix", "value": "/"}},
            "action": {"type": "forward", "cluster": "backend-service"}
          }
        ]
      }
    ]
  }'
```

### Step 5: Create Listener

Create a listener with the router filter (ext_authz will be installed separately):

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-listener",
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
            "routeConfigName": "my-routes",
            "httpFilters": [
              {
                "filter": {"type": "router"}
              }
            ]
          }
        ]
      }
    ]
  }'
```

### Step 6: Install Filter on Listener

Install the ext_authz filter on the listener:

```bash
curl -X POST http://localhost:8080/api/v1/filters/{FILTER_ID}/installations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "listenerName": "my-listener",
    "order": 1
  }'
```

The filter is now active on the listener. All requests will be authorized through the external service.

## Complete Example: gRPC External Authorization

For gRPC authorization services, the filter configuration differs slightly:

### Step 1: Create gRPC Authorization Service Cluster

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "grpc-authz-service",
    "team": "my-team",
    "http2": true,
    "endpoints": [
      {"host": "authz.example.com", "port": 50051}
    ]
  }'
```

### Step 2: Create gRPC ext_authz Filter

```bash
curl -X POST http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "grpc-ext-authz-filter",
    "filterType": "ext_authz",
    "description": "gRPC external authorization filter",
    "team": "my-team",
    "config": {
      "type": "ext_authz",
      "config": {
        "service": {
          "type": "grpc",
          "target_uri": "grpc-authz-service",
          "timeout_ms": 500,
          "initial_metadata": [
            {"key": "x-source", "value": "envoy-proxy"}
          ]
        },
        "failure_mode_allow": false,
        "stat_prefix": "grpc_ext_authz"
      }
    }
  }'
```

Then follow Steps 3-6 from the HTTP example to create the backend, routes, listener, and install the filter.

## Integration with OPA (Open Policy Agent)

OPA provides a popular HTTP-based authorization service:

```bash
# Create OPA cluster
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "opa-cluster",
    "team": "my-team",
    "endpoints": [{"host": "opa.internal", "port": 8181}]
  }'

# Create OPA ext_authz filter
curl -X POST http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "opa-authz-filter",
    "filterType": "ext_authz",
    "description": "OPA authorization filter",
    "team": "my-team",
    "config": {
      "type": "ext_authz",
      "config": {
        "service": {
          "type": "http",
          "server_uri": {
            "uri": "http://opa-cluster:8181/v1/data/httpapi/authz",
            "cluster": "opa-cluster",
            "timeout_ms": 500
          },
          "authorization_request": {
            "allowed_headers": ["authorization"]
          }
        },
        "failure_mode_allow": false,
        "stat_prefix": "opa_authz"
      }
    }
  }'
```

## Per-Route Configuration

ext_authz can be customized per-route using `typedPerFilterConfig` in the route configuration.

### Disable ext_authz for Health Check Routes

```bash
curl -X POST http://localhost:8080/api/v1/route-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-routes",
    "team": "my-team",
    "virtualHosts": [
      {
        "name": "api",
        "domains": ["api.example.com"],
        "routes": [
          {
            "name": "health",
            "match": {"path": {"type": "exact", "value": "/healthz"}},
            "action": {"type": "forward", "cluster": "backend"},
            "typedPerFilterConfig": {
              "envoy.filters.http.ext_authz": {
                "filter_type": "ext_authz",
                "disabled": true
              }
            }
          },
          {
            "name": "api",
            "match": {"path": {"type": "prefix", "value": "/"}},
            "action": {"type": "forward", "cluster": "backend"}
          }
        ]
      }
    ]
  }'
```

### Pass Context Extensions to Authz Service

Pass additional context to the authz service for specific routes:

```bash
curl -X POST http://localhost:8080/api/v1/route-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "admin-routes",
    "team": "my-team",
    "virtualHosts": [
      {
        "name": "admin",
        "domains": ["admin.example.com"],
        "routes": [
          {
            "name": "admin-api",
            "match": {"path": {"type": "prefix", "value": "/admin"}},
            "action": {"type": "forward", "cluster": "backend"},
            "typedPerFilterConfig": {
              "envoy.filters.http.ext_authz": {
                "filter_type": "ext_authz",
                "context_extensions": {
                  "required_role": "admin",
                  "audit_level": "high"
                }
              }
            }
          }
        ]
      }
    ]
  }'
```

### Per-Route Configuration Fields

| Field | Type | Description |
|-------|------|-------------|
| `disabled` | boolean | Disable ext_authz for this route |
| `context_extensions` | object | Key-value pairs passed to the authz service |
| `disable_request_body_buffering` | boolean | Disable request body buffering for this route |

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
    "description": "Updated description",
    "config": {
      "type": "ext_authz",
      "config": {
        "service": {
          "type": "http",
          "server_uri": {
            "uri": "http://ext-authz-service/check",
            "cluster": "ext-authz-service",
            "timeout_ms": 1000
          }
        },
        "failure_mode_allow": true,
        "stat_prefix": "ext_authz"
      }
    }
  }'
```

### Uninstall Filter from Listener

```bash
curl -X DELETE "http://localhost:8080/api/v1/filters/{FILTER_ID}/installations?listenerName=my-listener" \
  -H "Authorization: Bearer $TOKEN"
```

### Delete Filter

```bash
curl -X DELETE http://localhost:8080/api/v1/filters/{FILTER_ID} \
  -H "Authorization: Bearer $TOKEN"
```

## Troubleshooting

### Common Issues

1. **403 Forbidden - Authz Denied**

   The authz service returned a deny response:
   ```bash
   # Check ext_authz stats
   curl -s "http://localhost:9902/stats" | grep ext_authz
   ```

2. **503 Service Unavailable**

   The authz service is unreachable:
   ```bash
   # Check cluster health
   curl -s "http://localhost:9902/clusters" | grep authz
   ```

3. **Timeout Errors**

   Authz requests are timing out:
   - Increase `timeout_ms` in service configuration
   - Check authz service performance
   - Verify network connectivity

4. **gRPC Connection Issues**

   For gRPC services:
   - Ensure cluster is configured with `http2: true`
   - Verify the authz service is listening on the correct port
   - Check TLS configuration if using secure connections

5. **Headers Not Forwarded**

   Headers from authz response not appearing:
   - Verify header names in `allowed_upstream_headers`
   - Check that the authz service is returning the expected headers

### Debug Checklist

```bash
# 1. Check ext_authz filter is configured
curl -s "http://localhost:9902/config_dump?resource=dynamic_listeners" | \
  jq '.configs[].active_state.listener.filter_chains[].filters[].typed_config.http_filters[] | select(.name == "envoy.filters.http.ext_authz")'

# 2. Check authz cluster health
curl -s "http://localhost:9902/clusters" | grep -A10 authz

# 3. Check ext_authz stats
curl -s "http://localhost:9902/stats" | grep ext_authz

# 4. Test authz service directly
# For HTTP:
curl -X POST http://authz.internal/check -H "Content-Type: application/json" -d '{}'

# For gRPC:
grpcurl -plaintext authz.internal:50051 envoy.service.auth.v3.Authorization/Check
```

### Metrics

With `stat_prefix` configured, Envoy emits metrics:

| Metric | Description |
|--------|-------------|
| `ext_authz.{prefix}.ok` | Successful authorization requests |
| `ext_authz.{prefix}.denied` | Requests denied by authz service |
| `ext_authz.{prefix}.error` | Authz service errors |
| `ext_authz.{prefix}.timeout` | Authz request timeouts |
| `ext_authz.{prefix}.failure_mode_allowed` | Requests allowed due to failure_mode_allow |

## Security Considerations

1. **Fail-Closed Default**: Keep `failure_mode_allow: false` in production to deny requests when authz fails
2. **Timeout Configuration**: Set appropriate timeouts to prevent request delays
3. **Network Security**: Use TLS for connections to external authz services
4. **Header Sanitization**: Be careful with `allowed_upstream_headers` to prevent header injection
5. **Request Body**: Only enable `with_request_body` when necessary, as it adds latency
6. **Cluster Security**: Ensure the authz cluster is properly secured and authenticated

## gRPC vs HTTP Service Comparison

| Feature | gRPC | HTTP |
|---------|------|------|
| Protocol | HTTP/2 + protobuf | HTTP/1.1 + JSON |
| Performance | Higher throughput | More widely supported |
| Request format | CheckRequest proto | Customizable |
| Response format | CheckResponse proto | Customizable |
| Streaming | Supported | Not applicable |
| Debugging | Requires gRPC tools | Standard HTTP tools |

## See Also

- [Filters Overview](../filters.md) - All available filters
- [JWT Authentication](./jwt_authn.md) - JWT token validation
