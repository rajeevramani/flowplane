# MCP Server

Flowplane exposes its control plane as an MCP (Model Context Protocol) server. AI agents and tools like Claude Code connect over Streamable HTTP to manage Envoy gateway configuration — clusters, routes, listeners, filters, and more.

## Connection

**Endpoint:** `POST /api/v1/mcp` (Streamable HTTP)

The same path handles `GET` (SSE session) and `DELETE` (session teardown).

**Authentication:** Bearer token via the `Authorization` header.

```bash
flowplane auth token   # prints the bearer token
```

### Claude Code

Add to `.mcp.json` in your project root, or to your Claude Code MCP settings:

```json
{
  "mcpServers": {
    "flowplane": {
      "type": "streamable-http",
      "url": "http://localhost:8080/api/v1/mcp",
      "headers": {
        "Authorization": "Bearer <TOKEN>"
      }
    }
  }
}
```

Replace `<TOKEN>` with the output of `flowplane auth token`.

### Generic MCP Client

Any client that supports Streamable HTTP transport can connect. Set the URL to `http://localhost:8080/api/v1/mcp` and pass the bearer token in the `Authorization` header.

## Tools

Flowplane registers 69 tools. They fall into two categories:

- **Control plane tools** (`cp_*`, `ops_*`, `devops_*`, `dev_*`) — manage gateway configuration and diagnose issues.
- **Gateway API tools** (`api_*`) — proxy calls through the Envoy gateway to upstream services. These are generated dynamically from learned or imported API schemas.

### Control Plane Tools

| Category | Tools | Description |
|---|---|---|
| **Clusters** | `cp_list_clusters`, `cp_get_cluster`, `cp_create_cluster`, `cp_update_cluster`, `cp_delete_cluster`, `cp_get_cluster_health`, `cp_query_service` | Upstream service endpoints |
| **Listeners** | `cp_list_listeners`, `cp_get_listener`, `cp_create_listener`, `cp_update_listener`, `cp_delete_listener`, `cp_query_port`, `cp_get_listener_status` | Envoy listener ports and protocols |
| **Route Configs** | `cp_list_route_configs`, `cp_get_route_config`, `cp_create_route_config`, `cp_update_route_config`, `cp_delete_route_config` | Top-level routing configuration |
| **Virtual Hosts** | `cp_list_virtual_hosts`, `cp_get_virtual_host`, `cp_create_virtual_host`, `cp_update_virtual_host`, `cp_delete_virtual_host` | Domain-based request matching |
| **Routes** | `cp_list_routes`, `cp_get_route`, `cp_create_route`, `cp_update_route`, `cp_delete_route`, `cp_query_path` | Path-to-cluster routing rules |
| **Filters** | `cp_list_filters`, `cp_get_filter`, `cp_create_filter`, `cp_update_filter`, `cp_delete_filter`, `cp_attach_filter`, `cp_detach_filter`, `cp_list_filter_attachments`, `cp_list_filter_types`, `cp_get_filter_type` | HTTP filters and filter chains |
| **Dataplanes** | `cp_list_dataplanes`, `cp_get_dataplane`, `cp_create_dataplane`, `cp_update_dataplane`, `cp_delete_dataplane` | Envoy instance management |
| **Learning** | `cp_list_learning_sessions`, `cp_get_learning_session`, `cp_create_learning_session`, `cp_activate_learning_session`, `cp_stop_learning`, `cp_delete_learning_session` | API traffic learning and schema generation. Sessions support `name` for human-readable references and `autoAggregate` for continuous collection with periodic snapshots |
| **Schemas** | `cp_list_aggregated_schemas`, `cp_get_aggregated_schema`, `cp_export_schema_openapi` | Aggregated API schemas and OpenAPI 3.1 export with domain model `$ref` deduplication |
| **OpenAPI Import** | `cp_list_openapi_imports`, `cp_get_openapi_import` | Import routes from OpenAPI specs |
| **Secrets** | `cp_list_secrets`, `cp_get_secret`, `cp_create_secret`, `cp_delete_secret` | Secrets for filter configs |

### Ops and Agent Tools

| Tool | Description |
|---|---|
| `ops_trace_request` | Trace a request path through the gateway |
| `ops_topology` | View cluster and route topology |
| `ops_config_validate` | Validate current configuration |
| `ops_xds_delivery_status` | Check xDS config delivery to Envoy |
| `ops_nack_history` | View rejected xDS config history |
| `ops_audit_query` | Query the audit log |
| `ops_learning_session_health` | Health check for learning sessions |
| `devops_get_deployment_status` | Check deployment status |
| `dev_preflight_check` | Pre-creation validation |

### Risk Levels

Every tool has a risk level that indicates its impact:

| Level | Meaning | Examples |
|---|---|---|
| **SAFE** | Read-only, no side effects | `cp_list_clusters`, `ops_topology` |
| **LOW** | Easily reversible, additive | `cp_create_cluster`, `cp_create_route` |
| **MEDIUM** | Affects live traffic | `cp_update_cluster`, `cp_attach_filter` |
| **HIGH** | Potential outage | `cp_delete_listener`, `cp_detach_filter` |
| **CRITICAL** | Organization-wide impact | Reserved for future use |

## Examples

### 1. Expose httpbin Through the Gateway

Route traffic from an Envoy listener to a running httpbin instance. Each step is an MCP `tools/call` request.

**Step 1: Create a cluster**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_cluster",
    "arguments": {
      "name": "httpbin",
      "serviceName": "httpbin-service",
      "endpoints": [{"host": "httpbin", "port": 80}],
      "team": "default"
    }
  }
}
```

**Step 2: Create routing (route config + virtual host + route)**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_route_config",
    "arguments": {
      "name": "httpbin-routes",
      "virtualHosts": [{
        "name": "httpbin-vhost",
        "domains": ["*"],
        "routes": [{
          "name": "catch-all",
          "match": {"path": {"type": "prefix", "value": "/"}},
          "action": {"type": "forward", "cluster": "httpbin"}
        }]
      }],
      "team": "default"
    }
  }
}
```

**Step 3: Create a listener** (get `dataplaneId` from `cp_list_dataplanes` first)

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_listener",
    "arguments": {
      "name": "httpbin-listener",
      "address": "0.0.0.0",
      "port": 10001,
      "routeConfigName": "httpbin-routes",
      "dataplaneId": "<from cp_list_dataplanes>",
      "team": "default"
    }
  }
}
```

Verify:

```bash
curl http://localhost:10001/get
```

### 2. Add Rate Limiting

Attach a local rate limit filter to an existing listener.

**Step 1: Create the filter**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_filter",
    "arguments": {
      "name": "rate-limit-10rpm",
      "filterType": "local_rate_limit",
      "configuration": {
        "type": "local_rate_limit",
        "config": {
          "stat_prefix": "httpbin_rl",
          "token_bucket": {
            "max_tokens": 10,
            "tokens_per_fill": 10,
            "fill_interval_ms": 60000
          }
        }
      },
      "team": "default"
    }
  }
}
```

**Step 2: Attach to the listener**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_attach_filter",
    "arguments": {
      "filter": "rate-limit-10rpm",
      "listener": "httpbin-listener",
      "order": 1,
      "team": "default"
    }
  }
}
```

Verify:

```bash
for i in $(seq 1 15); do
  curl -s -o /dev/null -w "%{http_code}\n" http://localhost:10001/get
done
```

### 3. Import an OpenAPI Spec

Import via the CLI (the `cp_list_openapi_imports` and `cp_get_openapi_import` tools can verify the result):

```bash
flowplane import openapi ./petstore.yaml --name petstore --port 10002
```

Then list what was created:

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_list_openapi_imports",
    "arguments": {"team": "default"}
  }
}
```

## Authorization

Tools are authorized through Flowplane's role-based access control. Each tool maps to a `(resource, action)` pair checked by `check_resource_access()`.

Resource scopes: `clusters`, `listeners`, `routes`, `filters`, `secrets`, `proxy-certificates`, `learning-sessions`, `aggregated-schemas`, `dataplanes`, `audit`, `api`.

Actions: `read`, `create`, `update`, `delete`, `execute`.

Gateway API tools (`api_*`) all require `api:execute`.
