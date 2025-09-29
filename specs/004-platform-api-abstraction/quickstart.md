# Platform API Abstraction Quickstart

This guide shows how to create and extend an Envoy-facing API using Flowplane's Platform API abstraction. The control plane turns a concise intent payload into Envoy clusters, routes, listeners, and a bootstrap artefact that you can hand to a dataplane.

## Prerequisites
- Flowplane control plane running with the Platform API enabled
- Personal Access Token (PAT) that grants `routes:write`
- Team membership configured in Flowplane
- `curl` (or any HTTP client) for issuing requests

## Available Endpoints
| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/api/v1/api-definitions` | Create a new API definition with one or more routes |
| `POST` | `/api/v1/api-definitions/{id}/routes` | Append an additional route to an existing API definition |

The responses include a `bootstrapUri` that indicates where the bootstrap can be downloaded once a dedicated endpoint is implemented. During MVP the bootstrap YAML is written to `data/bootstrap/{id}.yaml` on the control plane host.

---

## 1. Create an API Definition
Issue a request that declares your team, domain, and desired routing behaviour:

```bash
curl -X POST http://localhost:8080/api/v1/api-definitions \
  -H "Authorization: Bearer $FLOWPLANE_PAT" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "payments",
    "domain": "payments.flowplane.dev",
    "listenerIsolation": false,
    "routes": [
      {
        "match": { "prefix": "/api/v1/" },
        "cluster": {
          "name": "payments-backend",
          "endpoint": "payments.svc.cluster.local:8443"
        },
        "timeoutSeconds": 15,
        "filters": { "cors": "allow-authenticated" }
      }
    ]
  }'
```

**Example response:**
```json
{
  "id": "5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3",
  "bootstrapUri": "/bootstrap/api-definitions/5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3.yaml",
  "routes": [
    "0a5ea373-16f8-4a4d-9220-9c5c4779c2d5"
  ]
}
```

After a successful request the control plane:
- Stores the API definition and route records in SQLite (`api_definitions` / `api_routes`)
- Generates Envoy clusters, routes, and listener snapshots for xDS
- Writes `data/bootstrap/<definition-id>.yaml`
- Returns the `bootstrapUri` so you can reference it later

### Inspect the Bootstrap Artefact
```bash
API_ID="5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3"
ls data/bootstrap/${API_ID}.yaml
```

The bootstrap references the generated cluster and route resources—no manual YAML assembly required.

---

## 2. Append an Additional Route
Extend the existing API by sending the route payload only:

```bash
curl -X POST \
  http://localhost:8080/api/v1/api-definitions/${API_ID}/routes \
  -H "Authorization: Bearer $FLOWPLANE_PAT" \
  -H "Content-Type: application/json" \
  -d '{
    "route": {
      "match": { "prefix": "/admin/" },
      "cluster": {
        "name": "payments-admin",
        "endpoint": "payments-admin.svc.cluster.local:8080"
      },
      "timeoutSeconds": 10,
      "filters": { "cors": { "allowMethods": ["GET"], "allowCredentials": false } }
    },
    "deploymentNote": "Expose admin tooling"
  }'
```

**Example response:**
```json
{
  "apiId": "5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3",
  "routeId": "6f2462c1-1345-4f63-9ff0-5d52cd518fa2",
  "revision": 2,
  "bootstrapUri": "/bootstrap/api-definitions/5b9b6a6d-8b81-4d62-92f4-7e9355d8f5c3.yaml"
}
```

Notes:
- You do **not** need to resend existing routes. The control plane looks up the prior definition, persists the new route, regenerates the bootstrap, and bumps the stored revision.
- `filters` accepts shorthand templates such as `"cors": "allow-authenticated"` or structured overrides. Payloads are canonicalised before persistence.

---

## 3. Validation & Collision Handling

### Required Fields
- `team`, `domain`, and at least one route are required on creation.
- Each route needs either a `prefix` or an exact `path` match, plus a cluster `name` and `endpoint` (`host:port`).
- `timeoutSeconds` must be positive and ≤ 3600.

**Example validation failure:**
```json
{
  "error": "bad_request",
  "message": "domain must contain alphanumeric, '.' or '-' characters"
}
```

### Domain and Path Collisions
If another API already claims the same domain + matcher, the request is rejected:
```json
{
  "error": "conflict",
  "message": "Route matcher 'prefix /api/' already exists"
}
```

### Listener Isolation Rules
Once `listenerIsolation` is enabled for an API it cannot be disabled on future updates; the materializer enforces this business rule.

---

## 4. Verifying the Generated State

### Database Tables
Use `sqlite3` (or `sqlx` from tests) to confirm persisted rows:
```bash
sqlite3 flowplane.db 'SELECT domain, listener_isolation FROM api_definitions;'
sqlite3 flowplane.db 'SELECT match_type, match_value FROM api_routes;'
```

### xDS Snapshots
Platform API routes are inserted into the ADS caches. You can inspect the in-memory state via the existing diagnostics endpoints or by tailing logs tagged `xds::state` when running with `RUST_LOG=debug`.

### Bootstrap Regeneration
Each create/append call updates `api_definitions.bootstrap_uri` and increments `bootstrap_revision`. Use this to drive cache invalidation or download workflows once the bootstrap endpoint is available.

---

## 5. Next Steps
- Launch an Envoy dataplane with the generated bootstrap.
- Add more routes as your API surface grows.
- Explore filter templates (`cors`, `authn`) by sending different `filters` payloads.
- Track audit entries in the `audit_log` table for create/append events.

For deeper architectural context see `specs/004-platform-api-abstraction/spec.md` and the inline Rust modules under `src/platform_api/`.
