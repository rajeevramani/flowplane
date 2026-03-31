# MCP Server

Flowplane exposes its control plane as an MCP (Model Context Protocol) server. AI agents and tools like Claude Code connect over Streamable HTTP to manage Envoy gateway configuration — clusters, routes, listeners, filters, and more.

## Connection

**Endpoint:** `POST /api/v1/mcp` (Streamable HTTP)

The same path handles `GET` (SSE session) and `DELETE` (session teardown).

**Authentication:** Bearer token via the `Authorization` header.

```bash
# After `make seed` has bootstrapped the database:
flowplane auth login
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

Flowplane registers 64+ tools. They fall into two categories:

- **Control plane tools** (`cp_*`, `ops_*`, `devops_*`, `dev_*`) — manage gateway configuration and diagnose issues.
- **Gateway API tools** (`api_*`) — proxy calls through the Envoy gateway to upstream services. These are generated dynamically from learned or imported API schemas.

### Control Plane Tools

| Category | Tools | Description |
|---|---|---|
| **Clusters** | `cp_list_clusters`, `cp_get_cluster`, `cp_create_cluster`, `cp_update_cluster`, `cp_delete_cluster`, `cp_get_cluster_health` | Upstream service endpoints |
| **Listeners** | `cp_list_listeners`, `cp_get_listener`, `cp_create_listener`, `cp_update_listener`, `cp_delete_listener`, `cp_query_port`, `cp_get_listener_status` | Envoy listener ports and protocols |
| **Route Configs** | `cp_list_route_configs`, `cp_get_route_config`, `cp_create_route_config`, `cp_update_route_config`, `cp_delete_route_config` | Top-level routing configuration |
| **Virtual Hosts** | `cp_list_virtual_hosts`, `cp_get_virtual_host`, `cp_create_virtual_host`, `cp_update_virtual_host`, `cp_delete_virtual_host` | Domain-based request matching |
| **Routes** | `cp_list_routes`, `cp_get_route`, `cp_create_route`, `cp_update_route`, `cp_delete_route`, `cp_query_path` | Path-to-cluster routing rules |
| **Filters** | `cp_list_filters`, `cp_get_filter`, `cp_create_filter`, `cp_update_filter`, `cp_delete_filter`, `cp_attach_filter`, `cp_detach_filter`, `cp_list_filter_attachments`, `cp_list_filter_types`, `cp_get_filter_type` | HTTP filters and filter chains |
| **Dataplanes** | `cp_list_dataplanes`, `cp_get_dataplane`, `cp_create_dataplane`, `cp_register_dataplane`, `cp_update_dataplane`, `cp_deregister_dataplane`, `cp_delete_dataplane` | Envoy instance management |
| **Learning** | `cp_list_learning_sessions`, `cp_get_learning_session`, `cp_create_learning_session`, `cp_start_learning`, `cp_stop_learning`, `cp_activate_learning_session`, `cp_delete_learning_session` | API traffic learning and schema generation |
| **Schemas** | `cp_list_schemas`, `cp_list_aggregated_schemas`, `cp_get_schema`, `cp_get_aggregated_schema`, `cp_export_schema`, `cp_export_schema_openapi` | Aggregated API schemas and OpenAPI export |
| **OpenAPI Import** | `cp_import_openapi`, `cp_list_openapi_imports`, `cp_get_openapi_import` | Import routes from OpenAPI specs |
| **Secrets** | `cp_list_secrets`, `cp_get_secret`, `cp_create_secret`, `cp_update_secret`, `cp_delete_secret` | Secrets for filter configs |
| **Certificates** | `cp_list_certificates`, `cp_get_certificate`, `cp_create_certificate`, `cp_delete_certificate` | TLS certificate management |
| **WASM Filters** | `cp_list_wasm_filters`, `cp_get_wasm_filter`, `cp_upload_wasm_filter`, `cp_update_wasm_filter`, `cp_delete_wasm_filter` | Custom WASM filter management |
| **Reports** | `cp_list_reports`, `cp_get_report` | Configuration reports |
| **Query** | `cp_query_service` | Service summary lookup |

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

Route traffic from an Envoy listener to a running httpbin instance.

```
# Step 1: Create a cluster pointing to httpbin
cp_create_cluster
  name: "httpbin"
  address: "host.docker.internal"
  port: 8000

# Step 2: Create routing — route config → virtual host → route
cp_create_route_config
  name: "httpbin-routes"

cp_create_virtual_host
  name: "httpbin-vhost"
  routeConfigName: "httpbin-routes"
  domains: ["*"]

cp_create_route
  name: "httpbin-catch-all"
  virtualHostName: "httpbin-vhost"
  routeConfigName: "httpbin-routes"
  route_pattern: "/"
  clusterName: "httpbin"

# Step 3: Create a listener on port 10000 with the route config
cp_create_listener
  name: "httpbin-listener"
  port: 10000
  routeConfigName: "httpbin-routes"
```

Verify:

```bash
curl http://localhost:10000/get
```

### 2. Add Rate Limiting

Attach a local rate limit filter to an existing listener.

```
# Step 1: Look up the filter type to get the config schema
cp_list_filter_types
cp_get_filter_type
  name: "local_rate_limit"

# Step 2: Create a rate limit filter (10 req/min)
cp_create_filter
  name: "rate-limit-10rpm"
  filterType: "local_rate_limit"
  config: {
    "max_tokens": 10,
    "tokens_per_fill": 10,
    "fill_interval_sec": 60
  }

# Step 3: Attach to the listener
cp_attach_filter
  filterName: "rate-limit-10rpm"
  listenerName: "httpbin-listener"
```

Verify:

```bash
# Send requests until you get HTTP 429
for i in $(seq 1 15); do
  curl -s -o /dev/null -w "%{http_code}\n" http://localhost:10000/get
done
```

### 3. Import an OpenAPI Spec

Create routes and clusters automatically from an OpenAPI specification.

```
# Import from a URL
cp_import_openapi
  name: "petstore"
  spec_url: "https://petstore3.swagger.io/api/v3/openapi.json"
  clusterName: "petstore-api"
  routeConfigName: "petstore-routes"

# Verify what was created
cp_list_routes
  routeConfigName: "petstore-routes"

cp_list_openapi_imports
```

The import parses the spec and creates a route for each operation, mapped to the target cluster.

## Authorization

Tools are authorized through Flowplane's role-based access control. Each tool maps to a `(resource, action)` pair checked by `check_resource_access()`.

Resource scopes: `clusters`, `listeners`, `routes`, `filters`, `secrets`, `proxy-certificates`, `custom-wasm-filters`, `learning-sessions`, `aggregated-schemas`, `reports`, `dataplanes`, `audit`, `api`.

Actions: `read`, `create`, `update`, `delete`, `execute`.

Gateway API tools (`api_*`) all require `api:execute`.
