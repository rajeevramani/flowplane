# Feature Specification: Platform API Abstraction

**Feature Branch**: `004-platform-api-abstraction`
**Created**: 2025-09-28
**Status**: In Revision

## 1. Objective
- Provide platform teams with a high-level REST API to expose services through Envoy without requiring them to understand listeners, routes, or clusters.
- Keep Envoy resource generation opaque to users (stored in Flowplane) while still delivering a downloadable bootstrap tailored to each deployment.
- Preserve a path for advanced users to opt into manual Envoy control while keeping generated state authoritative and auditable.
- Enable incremental delivery so teams can ship value quickly and grow toward the full abstraction.

## 2. Problem Statement
Today, teams must understand Envoy primitives (clusters, route configs, listeners, filter chains) to add or change an API. This creates friction, invites configuration drift, and makes RBAC enforcement difficult. We want Flowplane to own those low-level details while still letting power users go deeper when needed.

## 3. Guiding Principles
- **Opinionated defaults**: start with shared listeners, sane ports, and standard filters; only ask for extra input when required.
- **API-first workflow**: expose a concise payload-driven workflow (CLI, automation, or UI) that gathers intent and emits generated state consistently.
- **Progressive control**: teams begin with a guided workflow and can layer additional routes, upstreams, or overrides later.
- **Explicit ownership**: every generated listener/route/cluster is tagged to a team so RBAC and audit trails remain clear.
- **Transparent results**: Flowplane retains full Envoy configs for audit/debug, while users receive the bootstrap artefact and visibility into what changed.

## 4. Personas
- **Platform API Owner**: wants a fast, safe way to publish endpoints for their service without learning Envoy internals.
- **Advanced Operator**: understands Envoy and occasionally needs to override generated behaviour for bespoke scenarios.
- **Security/RBAC Admin**: cares that teams cannot mutate each other's traffic surfaces and that activity is recorded.

## 5. User Journeys
### 5.1 Primary Happy Path (MVP)
1. Team issues `POST /v1/api-definitions` with host, path matchers, upstream target, and optional overrides (via CLI, automation, or UI wrapper).
2. Flowplane validates RBAC, host/path collisions, and upstream metadata; it resolves listener placement using opinionated defaults (shared listener unless isolation requested).
3. Control plane generates Envoy clusters, routes, and listener patches, stores them in Flowplane's configuration database, and tags the artefacts with team ownership.
4. Flowplane emits audit entries, returns identifiers plus a downloadable Envoy bootstrap tailored to the API definition, and reconciles dataplanes.
5. User downloads or references the bootstrap to launch a dataplane; no direct Envoy YAML is exposed to the user.

### 5.2 Additional Journeys (Post-MVP)
- Manage multiple upstream targets with rollout weights and health-check policies.
- Configure advanced route-level overrides (auth, rate limiting) on existing APIs.
- Allocate or recycle dedicated listeners with bespoke filter chains for a team.
- Import an OpenAPI document to create multiple routes at once.
- Apply manual adjustments to generated config while keeping reconciliation guardrails.

### 5.3 Exception Paths
- Collision detected (host + path already owned by another team): block creation and provide guidance.
- RBAC failure (user lacks rights on team/listener): reject with actionable error.
- Invalid upstream definition (missing endpoint, unsupported protocol): validation error.

## 6. Scope Breakdown
### 6.1 MVP (v0.0.1)
- REST endpoint (`POST /v1/api-definitions`) for creating a single-route API backed by one upstream target.
- Listener placement behavior:
  - `listenerIsolation=false` (default): API VirtualHost is merged into the default gateway RouteConfiguration and served by the default listener.
  - `listenerIsolation=true`: API is attached only to a dedicated listener provided in the payload (`listener.bindAddress`, `listener.port`, optional `listener.name`, `listener.protocol`). No merge into the default gateway.
- Automatic generation and storage of Envoy listener/route/cluster resources; users only receive the bootstrap artefact.
- Basic per-route filter overrides limited to CORS templates.
- Incremental route addition endpoint (`POST /v1/api-definitions/{id}/routes`) that appends new matches without requiring full replacement.
- Collision detection that blocks conflicting host/path combinations.
- Audit logging for create/update/delete events.
- Documentation describing how the generated Envoy resources map to the user inputs and how to request manual overrides.
- Review the existing `/api/v1/gateways/openapi` import flow and reuse its parsing/validation/default generation pieces where possible before extending the new guided experience.

### 6.2 Near-Term Enhancements (v0.0.2+)
- Multiple upstream targets with traffic weights and progressive rollout guidance.
- Optional dedicated listener provisioning with port allocation rules and capacity checks.
- Route-level filter library (rate limiting, auth) with precedence rules vs listener filters.
- Health-check, timeout, and TLS configuration hooks on upstreams.
- OpenAPI import that expands to multiple routes and shared upstreams.
- Background reconciliation to detect and report manual Envoy edits.

### 6.3 Future Direction (v0.1+)
- Traffic shadowing, canary orchestration, and automated rollback strategies.
- Full listener lifecycle management (lifecycle policies, automatic clean-up).
- Observability integrations (metrics, tracing, alerting hooks) tied to API definitions.
- Team membership synchronisation with external identity providers.

## 7. Non-Goals (for MVP)
- Weighted traffic splitting or shadow clusters.
- User-managed Envoy bootstrap generation outside of documented flows.
- Automatic creation of OpenAPI contracts.
- Custom filter development or WASM modules.

## 8. Functional Requirements
### 8.1 MVP Requirements
- **MVP-FR1**: Provide a REST endpoint (`POST /v1/api-definitions`) that accepts host, path matchers, optional rewrite, and upstream target metadata.
- **MVP-FR2**: Automatically generate and persist Envoy listener, route, and cluster resources for each new API definition.
- **MVP-FR3**: Return identifiers plus a downloadable Envoy bootstrap tailored to the created API without exposing raw Envoy YAML to the caller.
- **MVP-FR4**: Attach APIs to a team-owned shared listener and record isolation intent for future upgrades.
- **MVP-FR5**: Enforce RBAC so users can only create or modify APIs owned by their team.
- **MVP-FR6**: Detect host/path collisions and block creation with actionable errors.
- **MVP-FR7**: Provide an incremental route addition endpoint (`POST /v1/api-definitions/{id}/routes`) that appends new matches without requiring full replacement.
- **MVP-FR8**: Allow optional per-route CORS override templates that inherit listener defaults.
- **MVP-FR9**: Persist API definitions, including ownership metadata, upstream linkage, and override settings.
- **MVP-FR10**: Emit audit logs for create, update, and delete operations.
- **MVP-FR11**: Provide documentation that explains generated Envoy artefacts, available payload fields, and manual override boundaries.
- **MVP-FR12**: Evaluate and leverage the existing `/api/v1/gateways/openapi` import pipeline for parsing, validation, and default resource generation before introducing new abstraction layers.

### 8.2 Post-MVP Requirements
- **PM-FR1**: Support multiple upstream targets with configurable rollout weights and health policies.
- **PM-FR2**: Provision dedicated listeners on demand with validation against port/host capacity.
- **PM-FR3**: Support additional filter types (authN/Z, rate limiting) at listener and route scope with deterministic precedence.
- **PM-FR4**: Allow advanced users to apply manual overrides without breaking reconciliation, with clear diff reporting.
- **PM-FR5**: Integrate observability hooks (metrics, logs, traces) per API definition.
- **PM-FR6**: Synchronise team membership and permissions with external identity sources.
- **PM-FR7**: Offer OpenAPI-driven bulk API creation and update workflows.

## 9. Data & System Considerations
- Introduce an `api_definitions` table capturing team ownership, host/path metadata, upstream reference(s), isolation intent, bootstrap locations, and override settings.
- Store generated Envoy listeners/routes/clusters in configuration tables keyed by API definition; expose only metadata and bootstrap URLs externally.
- Track route-level records (`api_routes`) so incremental additions can be appended without re-posting existing paths.
- Capture relationships for future enhancements (`api_upstreams`, `api_route_overrides`, etc.) but defer physical tables until the capabilities ship.
- Ensure audit log stream records API lifecycle events with actor, timestamp, request payload, and changeset summary.

## 10. Documentation & Enablement
- Create an MVP playbook describing the REST payloads, validation rules, and how they map to generated Envoy resources and bootstrap downloads.
- Provide an "Advanced Operations" appendix that documents manual Envoy steps for exceptional cases while stating support boundaries.
- Deliver quick-start examples (importing OpenAPI, creating APIs manually) and troubleshooting guidance for collision and RBAC errors.
- Publish example request/response bodies for API creation and incremental route addition in developer docs and CLI help.

## 11. Success Metrics
- 80% of new APIs created via the guided flow within 60 days of launch.
- Time-to-first-API reduced by 50% compared to manual Envoy configuration.
- Zero unauthorized cross-team modifications detected in audit logs post-launch.

## 12. Risks & Mitigations
- **Risk**: Teams bypass the guided flow and apply manual Envoy edits. **Mitigation**: Document support boundaries, add reconciliation alerting post-MVP.
- **Risk**: Shared listener limits make isolation toggle feel broken. **Mitigation**: Clearly message that isolation is roadmap work; capture demand analytics.
- **Risk**: Collision blocks legitimate shared APIs. **Mitigation**: Provide escalation path for intentional sharing with explicit approval workflow.

## 13. Open Questions
1. What catalog or service registry do we use to present upstream targets in the UI/API? [NEEDS DECISION]
2. How do we expose the isolation toggle in MVP while setting expectations that fulfillment is deferred? [NEEDS COPY REVIEW]
3. Do we allow direct OpenAPI import in MVP, or require manual creation first? [NEEDS ALIGNMENT]
4. How are manual Envoy overrides submitted and tracked so reconciliation can warn instead of overwrite? [DESIGN PENDING]

## Appendix A: Manual Envoy Workflow (Reference Only)
For teams that must operate outside the abstraction, the supported manual sequence is:
1. Define or update the upstream cluster configuration.
2. Create or modify the route configuration that references the cluster.
3. Attach or create the listener that will receive inbound traffic (choose port/address).
4. Apply filters at the desired scope (listener or route) using approved templates.
5. Generate and validate the Envoy bootstrap/configuration artefacts.
6. Use the API to add or update additional routes within the managed route configuration.

Documented manual steps require notifying the platform team so reconciliation tooling can record the divergence.

## 10. API Endpoints

### 10.1 Create API
`POST /v1/api-definitions`

Creates a new API definition. See examples below for isolated vs non‑isolated listeners.

### 10.2 Get API Definition
`GET /v1/api-definitions/{id}`

Returns a summary of the API definition including listener placement and bootstrap metadata.

### 10.3 List API Definitions
`GET /v1/api-definitions`

Returns a list of API definition summaries. Optional filters (team, domain) and pagination (limit, offset) may be supported.

## Appendix B: API Payload Examples (Reference)
### B.1 Create API Request (non-isolated / default listener)
```json
POST /api/v1/api-definitions
{
  "team": "payments",
  "domain": "payments.flowplane.dev",
  "routes": [
    {
      "match": { "prefix": "/v1/" },
      "cluster": {
        "name": "payments-backend",
        "endpoint": "payments.svc.cluster.local:8443"
      },
      "timeoutSeconds": 3,
      "filters": { "cors": "allow-authenticated" }
    },
    {
      "match": { "path": "/healthz" },
      "cluster": {
        "name": "payments-admin",
        "endpoint": "payments-admin.svc.cluster.local:8080"
      },
      "timeoutSeconds": 1
    }
  ],
  "listenerIsolation": false,
  "tls": {
    "mode": "mutual",
    "cert": "arn:aws:secretsmanager:us-east-1:123456789012:secret:payments-cert",
    "key": "arn:aws:secretsmanager:us-east-1:123456789012:secret:payments-key"
  }
}
```

### B.1b Create API Request (isolated / dedicated listener)
```json
POST /api/v1/api-definitions
{
  "team": "payments",
  "domain": "payments.flowplane.dev",
  "listenerIsolation": true,
  "listener": {
    "name": "payments-shared-listener",
    "bindAddress": "0.0.0.0",
    "port": 10010,
    "protocol": "HTTP"
  },
  "routes": [
    {
      "match": { "prefix": "/v1/" },
      "cluster": { "name": "payments-backend", "endpoint": "httpbin.org:443" },
      "timeoutSeconds": 3
    }
  ]
}
```

### B.2 Create API Response (abbreviated)
```json
201 Created
{
  "id": "api_12345",
  "bootstrapUri": "/bootstrap/api-definitions/api_12345.yaml",
  "routes": ["37f4695e-7b12-4c8f-8c85-1c4fd6a2c11f"]
}
```

### B.3 Append Route Request
```json
POST /api/v1/api-definitions/api_12345/routes
{
  "route": {
    "match": { "prefix": "/v2/" },
    "cluster": {
      "name": "payments-backend",
      "endpoint": "payments.svc.cluster.local:8443"
    },
    "timeoutSeconds": 5,
    "rewrite": { "prefix": "/internal/v2/" },
    "filters": {
      "cors": "allow-authenticated",
      "authn": "oidc-default"
    }
  },
  "deploymentNote": "enable /v2 rollout"
}
```

### B.4 Append Route Response (abbreviated)
```json
202 Accepted
{
  "apiId": "api_12345",
  "routeId": "route_67890",
  "revision": 4,
  "bootstrapUri": "/bootstrap/api-definitions/api_12345.yaml"
}
```

Responses omit raw Envoy YAML; consumers rely on the returned bootstrap URI or metadata for deployments and auditing.

### Notes on Delivery & TLS
- Delta ADS remains enabled for all types; the control plane also pushes SOTW responses proactively on cache changes so Envoy updates without restart.
- Upstream TLS is inferred for hostname endpoints on port 443 (UpstreamTlsContext with SNI set to the hostname). Plaintext is used otherwise.
- Optional per-route TLS overrides (e.g., enable TLS on non‑443, custom SNI) are planned; absence of overrides keeps inference behavior.
