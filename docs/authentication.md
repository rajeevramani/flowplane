# Control Plane Authentication

Flowplane protects every control plane endpoint with bearer authentication. The control plane
accepts two credential types:

- **Personal access tokens (PATs)** – hashed secrets stored in the control plane database. These are
  the recommended choice for automation and interactive API clients.
- **JWTs** – the legacy mechanism used by the original control plane implementation. Existing JWT
  clients continue to work unchanged, but new automation should migrate to PATs to take advantage of
  scoped authorization and audit logging.

## Bootstrapping

When the control plane starts with an empty `personal_access_tokens` table it automatically seeds a
bootstrap token named `bootstrap-admin`. The secret is printed once to the logs, emitted through the
CLI if you are running in foreground mode, and recorded in the audit log as
`auth.token.seeded`. Store the value immediately—Flowplane never displays the secret again.

```text
WARN flowplane::openapi::defaults: Seeded bootstrap admin personal access token; store it securely
```

You can regenerate the bootstrap token at any time by invoking `TokenService::ensure_bootstrap_token`
or by deleting every token record and restarting the server.

## Personal Access Token Lifecycle

Tokens flow through a dedicated service (`src/auth/token_service.rs`) that enforces validation,
hashing, and audit logging. Every state change records a structured event:

| Action                    | Event name               | Metadata preview                         |
|--------------------------|--------------------------|-------------------------------------------|
| Create token              | `auth.token.created`     | scopes, created_by                        |
| Update token metadata     | `auth.token.updated`     | status, expires_at, scopes                |
| Revoke token              | `auth.token.revoked`     | status                                    |
| Rotate token secret       | `auth.token.rotated`     | rotated_at timestamp                      |
| Token used for auth       | `auth.token.authenticated` | granted scopes                          |
| Bootstrap seeded          | `auth.token.seeded`      | token name                                |

All audit events are written to the `audit_log` table through `AuditLogRepository::record_auth_event`.
The same repository powers the global audit feed, so external systems can subscribe by tailing the
database or forwarding the entries to a SIEM.

### Token Status

Tokens are always returned with a status:

- `active` – default state. Token may authenticate and is counted toward the `auth_tokens_active_total`
  gauge.
- `revoked` – access disabled by an operator. Secrets remain hashed for audit purposes but
  authentication fails.
- `expired` – Flowplane’s cleanup service automatically marks tokens as expired when `expires_at`
  falls behind the current time.

### Scopes

Scopes govern which API groups a token may call. The HTTP router layers a scope check over every
protected route (see `src/api/routes.rs`). Flowplane supports three types of scopes:

1. **Admin bypass scope**: `admin:all` grants full access across all teams and resources
2. **Resource-level scopes**: `{resource}:{action}` grants access to all resources of that type
3. **Team-scoped permissions**: `team:{name}:{resource}:{action}` grants access only to that team's resources

#### Available Scopes

| Scope Pattern | Example | Grants Access To |
|---------------|---------|------------------|
| `admin:all` | `admin:all` | Full access to all resources across all teams (bypass) |
| `{resource}:read` | `routes:read` | Read all resources of this type across all teams |
| `{resource}:write` | `clusters:write` | Create/update/delete all resources of this type across all teams |
| `team:{name}:{resource}:read` | `team:platform:routes:read` | Read only the specified team's resources |
| `team:{name}:{resource}:write` | `team:platform:clusters:write` | Create/update/delete only the specified team's resources |

#### Resource Types

| Resource | Endpoints | Notes |
|----------|-----------|-------|
| `tokens` | `/api/v1/tokens*` | Token management operations |
| `clusters` | `/api/v1/clusters*` | Envoy cluster configurations |
| `routes` | `/api/v1/routes*` | Envoy route configurations |
| `listeners` | `/api/v1/listeners*` | Envoy listener configurations |
| `api-definitions` | `/api/v1/api-definitions*` | API definition (Platform API) resources |
| `reports` | `/api/v1/reports/*` | Reporting and analytics endpoints |

#### Authorization Hierarchy

When checking permissions, Flowplane evaluates scopes in this order:

1. **Admin bypass**: If token has `admin:all`, access is granted immediately
2. **Resource-level**: If token has `{resource}:{action}`, access is granted for all teams
3. **Team-scoped**: If token has `team:{name}:{resource}:{action}`, access is granted only for that team

#### Team Isolation

Resources created by team-scoped tokens are automatically tagged with the team name and filtered at the database level:

```bash
# Team-scoped token can only see its own resources
curl -H "Authorization: Bearer fp_pat_...<team:platform:routes:read>" \
  http://127.0.0.1:8080/api/v1/routes
# Returns only routes with team='platform'

# Admin token sees all resources
curl -H "Authorization: Bearer fp_pat_...<admin:all>" \
  http://127.0.0.1:8080/api/v1/routes
# Returns all routes regardless of team
```

Tokens may carry any subset of scopes. Flowplane persists them in `token_scopes` and caches them in
memory when authenticating.

## Authenticating HTTP Requests

Send PATs as a bearer credential. Flowplane generates secrets in the format
`fp_pat_<token-id>.<random>` so the control plane can look up the record by ID before verifying the
Argon2 hash.

```bash
curl -sS \
  -H "Authorization: Bearer fp_pat_8a6f9d37-9a4c-4dbe-a494-9bd924dbd1b1.nItY..." \
  http://127.0.0.1:8080/api/v1/tokens
```

If the token is missing, malformed, revoked, expired, or lacks the required scope Flowplane responds
with `401 Unauthorized` or `403 Forbidden` as appropriate. JSON error bodies include a sanitized
message and a correlation ID for traceability.

## Managing Tokens

There are two supported workflows:

1. **REST API** – create, rotate, revoke, update metadata via `/api/v1/tokens` endpoints. The
   `docs/api.md` file contains request/response examples.
2. **CLI** – run `flowplane auth <command>` from the project root or any environment where the
   Flowplane binary is available. See [`docs/token-management.md`](docs/token-management.md) for a
   full command reference.

Both paths funnel through the same service layer, guaranteeing consistent hashing, validation, audit
logging, and metrics.

### Creating Tokens with RBAC

#### Admin Token

Create a token with full admin access (bypass all team restrictions):

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $BOOTSTRAP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "admin-token",
    "description": "Full admin access for operations team",
    "scopes": ["admin:all"],
    "expiresAt": "2026-01-01T00:00:00Z"
  }'
```

#### Resource-Level Token

Create a token with read-only access to all routes across all teams:

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "global-routes-readonly",
    "description": "Read-only access to all routes for monitoring",
    "scopes": ["routes:read", "clusters:read"],
    "expiresAt": null
  }'
```

#### Team-Scoped Token

Create a token restricted to a specific team:

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "platform-team-token",
    "description": "Platform team CI/CD pipeline",
    "scopes": [
      "team:platform:routes:read",
      "team:platform:routes:write",
      "team:platform:clusters:read",
      "team:platform:clusters:write",
      "team:platform:api-definitions:read",
      "team:platform:api-definitions:write"
    ],
    "expiresAt": null
  }'
```

#### Multi-Team Token

Create a token with access to multiple teams:

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "multi-team-token",
    "description": "Access to platform and engineering teams",
    "scopes": [
      "team:platform:routes:read",
      "team:platform:routes:write",
      "team:engineering:routes:read",
      "team:engineering:clusters:read"
    ],
    "expiresAt": null
  }'
```

### RBAC Best Practices

1. **Principle of Least Privilege**: Grant only the minimum scopes required for the task
   - Use team-scoped permissions instead of resource-level when possible
   - Use `:read` scopes for monitoring and reporting tools
   - Reserve `admin:all` for operational emergencies only

2. **Team Isolation**: Organize resources by team to enforce separation of concerns
   - All resources (clusters, routes, listeners, API definitions) support team tagging
   - Team-scoped tokens automatically filter resources at the database level
   - Use the `team` query parameter when creating resources via OpenAPI import

3. **Token Rotation**: Regularly rotate token secrets, especially for CI/CD pipelines
   ```bash
   curl -X POST http://127.0.0.1:8080/api/v1/tokens/{id}/rotate \
     -H "Authorization: Bearer $ADMIN_TOKEN"
   ```

4. **Audit Logging**: Review authentication events regularly
   - All token operations are logged to the `audit_log` table
   - Events include `auth.token.created`, `auth.token.authenticated`, `auth.token.revoked`
   - Monitor for unexpected `auth.token.authenticated` events on admin tokens

5. **Expiration Policies**: Set appropriate expiration times based on token usage
   - Short-lived tokens (24-48 hours) for interactive debugging
   - Long-lived tokens (30-90 days) for CI/CD pipelines with rotation
   - No expiration for admin tokens in secure vaults only

## Observability

Every authentication event increments the following Prometheus series (see
`src/observability/metrics.rs`):

- `auth_authentications_total{status="success"}` – successful authentications
- `auth_authentications_total{status="not_found"}` – unknown token IDs
- `auth_authentications_total{status="invalid_secret"}` – hash mismatch
- `auth_tokens_created_total`, `auth_tokens_revoked_total`, `auth_tokens_rotated_total`
- `auth_tokens_active_total{state="active"}` – gauge of active tokens

Attach your metrics collector to the configured `FLOWPLANE_METRICS_PORT` and the series will appear
as soon as Flowplane starts.

## Security Checklist

- Secrets are hashed with Argon2id (1 iteration, 0.75 MiB memory) seeded with an OS-provided salt, keeping
  verification fast while retaining modern defenses.
- Audit events capture the token ID but never the secret.
- Tracing spans include token IDs and correlation IDs for every service & handler call.
- Middleware ensures scopes are checked before any business logic runs.

With these controls in place, the new authentication subsystem satisfies the requirements laid out in
`specs/001-control-plane-auth` while remaining backwards compatible with existing JWT clients.
