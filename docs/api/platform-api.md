# Platform API

Flowplane's Platform API turns a concise intent payload into Envoy configuration. Control-plane operators can expose HTTP services through Envoy without hand-writing listeners, routes, or clusters.

## Audience & Scopes
- **Audience**: platform teams that own public APIs.
- **Required scope**: `routes:write` for both creation and append operations.
- **Audit**: every mutation is persisted via `AuditLogRepository`.

## Endpoints
| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/api/v1/api-definitions` | Create an API definition with one or more initial routes |
| `POST` | `/api/v1/api-definitions/{id}/routes` | Append a route to an existing API definition |

> Additional read/update/delete endpoints are future work. The MVP focuses on creation and incremental growth.

## Request Schema

### Create API (`POST /api/v1/api-definitions`)
```json
{
  "team": "payments",
  "domain": "payments.flowplane.dev",
  "listenerIsolation": false,
  "tls": { "mode": "terminate", "reference": "arn:...:secret:payments-cert" },
  "routes": [
    {
      "match": { "prefix": "/api/" },
      "cluster": {
        "name": "payments-backend",
        "endpoint": "payments.svc.cluster.local:8443"
      },
      "timeoutSeconds": 15,
      "rewrite": { "prefix": "/internal/" },
      "filters": { "cors": "allow-authenticated" }
    }
  ]
}
```

### Append Route (`POST /api/v1/api-definitions/{id}/routes`)
```json
{
  "route": {
    "match": { "path": "/healthz" },
    "cluster": {
      "name": "payments-health",
      "endpoint": "payments-health.svc.cluster.local:8080"
    },
    "timeoutSeconds": 5
  },
  "deploymentNote": "Expose health endpoint"
}
```

### Route Match Rules
- Provide **either** `match.prefix` or `match.path`.
- Prefix matches are case-sensitive.
- `rewrite.prefix` rewrites the upstream prefix; regex rewrites are ignored (logged) in MVP.

### Cluster Rules
- `cluster.endpoint` must be `host:port`.
- One upstream target is persisted today; additional targets/weights are roadmap items.

### Filter Overrides
`filters` accepts shortcuts that are canonicalised before storage:
- `"cors": "allow-authenticated"`
- `"authn": "disabled"` or `"authn": "my-jwt-requirement"`
- Fully-qualified filter names map to typed per-filter config when provided as structured JSON.

## Responses

### Create API
```json
{
  "id": "5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3",
  "bootstrapUri": "/bootstrap/api-definitions/5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3.yaml",
  "routes": ["0a5ea373-16f8-4a4d-9220-9c5c4779c2d5"]
}
```

### Append Route
```json
{
  "apiId": "5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3",
  "routeId": "6f2462c1-1345-4f63-9ff0-5d52cd518fa2",
  "revision": 2,
  "bootstrapUri": "/bootstrap/api-definitions/5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3.yaml"
}
```

## Error Model
All errors share the shape:
```json
{ "error": "bad_request", "message": "domain must contain alphanumeric, '.' or '-' characters" }
```

Common failure cases:
- `bad_request`: validation failure (invalid domain, missing routes, timeout out of range)
- `conflict`: domain/path collision with an existing API
- `unauthorized` / `forbidden`: scope or team mismatch

## Generated Artefacts
- **Database**: `api_definitions` and `api_routes` tables persist intent, overrides, and listener isolation flags.
- **Envoy xDS**: `resources_from_api_definitions` converts records into listeners, routes, and clusters that the ADS service serves alongside existing configuration.
- **Bootstrap**: YAML is written to `data/bootstrap/{id}.yaml` and the URI is recorded on the definition. A download endpoint is on the roadmap; for now the file can be distributed out of band.

## Operational Notes
- Listener isolation cannot be disabled once enabled for a definition.
- Filter overrides are normalised before storage so repeated requests are idempotent.
- Audit events are emitted for both create and append operations.
- The materializer triggers `refresh_platform_api_resources` so new Envoy state is visible to dataplanes immediately.

## Related Reading
- `specs/004-platform-api-abstraction/spec.md`
- `specs/004-platform-api-abstraction/tasks.md`
- `quickstart.md` (root) for end-to-end setup
- `tests/platform_api/` for lifecycle examples
