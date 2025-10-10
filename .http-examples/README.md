# Flowplane API HTTP Tests

Interactive HTTP tests for the Flowplane Control Plane API using VSCode REST Client extension.

## Prerequisites

1. **Install VSCode REST Client Extension**
   - Open VSCode
   - Install "REST Client" extension by Huachao Mao
   - Search for `humao.rest-client` in Extensions

2. **Start Flowplane Server**
   ```bash
   cargo run
   ```

3. **Get Bootstrap Token**
   - Check server logs on first startup for the bootstrap token
   - OR create a token using the CLI:
   ```bash
   cargo run --bin flowplane-cli auth create-token \
     --name "test" \
     --scope admin:all \
     --expires-in 90d
   ```

4. **Update Token in Test Files**
   - Replace `fp_pat_your-token-id-here.your-secret-here` with your actual token
   - Update in `_variables.http` OR in individual test files

## File Organization

| File | Description |
|------|-------------|
| `_variables.http` | Common variables (base_url, tokens, resource names) |
| `auth.http` | Token management endpoints (create, list, get, update, rotate, revoke) |
| `clusters.http` | Cluster management endpoints (create, list, get, update, delete) |
| `routes.http` | Route configuration endpoints (create, list, get, update, delete) |
| `listeners.http` | Listener configuration endpoints (create, list, get, update, delete) |
| `api-definitions.http` | OpenAPI import and API definition management |
| `reporting.http` | Reporting endpoints (route flows) |

## Usage

### Quick Start

1. Open any `.http` file in VSCode
2. Click "Send Request" above any `###` section
3. View response in the right pane

### Variables

Each file has inline variables at the top:
```http
@base_url = http://localhost:8080
@token = fp_pat_your-token-id-here.your-secret-here
```

Update these or use the shared `_variables.http` file.

### Typical Workflow

1. **Create Token** (`auth.http`)
   - Create a new API token with specific scopes

2. **Create Cluster** (`clusters.http`)
   - Define upstream service endpoints

3. **Create Routes** (`routes.http`)
   - Configure routing rules

4. **Create Listener** (`listeners.http`)
   - Set up listener with routes

5. **Test via Envoy**
   ```bash
   curl http://localhost:10000/api -H "Host: httpbin.org"
   ```

### OpenAPI Import Workflow

1. **Import OpenAPI Spec** (`api-definitions.http`)
   - Use examples from `../../../examples/` directory
   - Creates clusters, routes, and listeners automatically

2. **Get Bootstrap Config** (`api-definitions.http`)
   - Retrieve Envoy bootstrap configuration
   - Find assigned listener port

3. **Test the API**
   ```bash
   curl http://localhost:{PORT}/get -H "Host: httpbin.org"
   ```

## Examples Directory

The `examples/` directory contains sample OpenAPI specs:

- `httpbin-simple.yaml` - Simple working example
- `httpbin-demo.yaml` - Full-featured demo
- `openapi-with-x-flowplane-filters.yaml` - Custom filters example
- `openapi-custom-response-example.yaml` - Custom responses
- `method-extraction-demo.yaml` - Method extraction patterns

## Tips

- **Replace Placeholders**: Update `{id}` placeholders with actual IDs from responses
- **Sequential Testing**: Create resources in order (cluster → routes → listener)
- **Team Isolation**: Use `?team=demo` query parameter for team-scoped resources
- **Rate Limiting**: OpenAPI imports may include rate limiting filters
- **Host Header**: Always include `Host` header when testing through Envoy

## Endpoints Summary

### Authentication (`auth.http`)
- `POST /api/v1/tokens` - Create token
- `GET /api/v1/tokens` - List tokens
- `GET /api/v1/tokens/{id}` - Get token
- `PATCH /api/v1/tokens/{id}` - Update token
- `POST /api/v1/tokens/{id}/rotate` - Rotate token
- `DELETE /api/v1/tokens/{id}` - Revoke token

### Clusters (`clusters.http`)
- `POST /api/v1/clusters` - Create cluster
- `GET /api/v1/clusters` - List clusters
- `GET /api/v1/clusters/{name}` - Get cluster
- `PUT /api/v1/clusters/{name}` - Update cluster
- `DELETE /api/v1/clusters/{name}` - Delete cluster

### Routes (`routes.http`)
- `POST /api/v1/routes` - Create route
- `GET /api/v1/routes` - List routes
- `GET /api/v1/routes/{name}` - Get route
- `PUT /api/v1/routes/{name}` - Update route
- `DELETE /api/v1/routes/{name}` - Delete route

### Listeners (`listeners.http`)
- `POST /api/v1/listeners` - Create listener
- `GET /api/v1/listeners` - List listeners
- `GET /api/v1/listeners/{name}` - Get listener
- `PUT /api/v1/listeners/{name}` - Update listener
- `DELETE /api/v1/listeners/{name}` - Delete listener

### API Definitions (`api-definitions.http`)
- `POST /api/v1/api-definitions/from-openapi` - Import OpenAPI spec
- `GET /api/v1/api-definitions` - List API definitions
- `GET /api/v1/api-definitions/{id}` - Get API definition
- `PATCH /api/v1/api-definitions/{id}` - Update API definition
- `GET /api/v1/api-definitions/{id}/bootstrap` - Get Envoy bootstrap config
- `POST /api/v1/api-definitions/{id}/routes` - Append routes

### Reporting (`reporting.http`)
- `GET /api/v1/reports/route-flows` - List route flows (shows end-to-end routing)

## Troubleshooting

### 401 Unauthorized
- Check your token is valid
- Ensure token has correct scopes for the endpoint
- Bootstrap token has `admin:all` scope

### 403 Forbidden
- Token doesn't have required scope
- Check RBAC scope requirements for the endpoint

### 404 Not Found
- Resource name/ID doesn't exist
- Check exact spelling and case

### 503 Service Unavailable
- Database connection issue
- Check server logs

## Related Tools

Consider using **Bruno** for a GUI-based alternative:
- https://www.usebruno.com/
- Stores collections in git-friendly format
- Supports both REST and gRPC
- See Task #27 for Bruno setup instructions
