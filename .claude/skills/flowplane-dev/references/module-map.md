# Module Map Reference

## src/ Directory Overview

### `src/api/` — REST API Layer
HTTP handlers, middleware, and route definitions for the REST API.

| File/Dir | Purpose |
|---|---|
| `routes.rs` | Axum router setup, middleware installation (dev vs prod) |
| `handlers/` | REST endpoint handlers (one file per resource type) |
| `handlers/organizations.rs` | Org, team, user, agent CRUD |
| `handlers/bootstrap.rs` | First-time setup endpoints |
| `handlers/dataplane.rs` | Dataplane operations |
| `error.rs` | `ApiError` type, error response formatting |
| `rate_limit.rs` | Request rate limiting |

### `src/auth/` — Authentication & Authorization
All auth logic — middleware, JWT validation, scope checking, team resolution.

| File | Purpose |
|---|---|
| `middleware.rs` | `authenticate()` (prod) and `dev_authenticate()` (dev) |
| `authorization.rs` | `check_resource_access()` — single enforcement point |
| `models.rs` | `AuthContext`, `AgentContext`, `AuthError` |
| `zitadel.rs` | JWT validation, JWKS cache, Zitadel integration |
| `dev_token.rs` | Dev token generation and credential file I/O |
| `team.rs` | Team resolution (hardcoded dev vs DB-backed prod) |
| `cache.rs` | Permission cache (prod mode) |
| `permissions.rs` | `load_permissions()` from DB |
| `scope_registry.rs` | Scope definitions |

### `src/cli/` — CLI Layer
All `flowplane` CLI subcommands defined with clap.

| File | Purpose |
|---|---|
| `mod.rs` | Entry point, all clap command definitions (~922 lines) |
| `auth.rs` | `login`, `token`, `whoami`, `logout` |
| `compose.rs` | `init`, `up`, `down` (Docker lifecycle) |
| `compose_runner.rs` | `ComposeRunner` trait for testability |
| `filter.rs` | Filter CRUD + attach/detach |
| `expose.rs` | Quick service exposure |
| `import.rs` | OpenAPI import |
| `learn.rs` | Learning sessions |
| `status.rs` | Health checks, doctor |
| `config.rs` | CLI config management |

### `src/config/` — Configuration
Application config from environment variables.

| File | Purpose |
|---|---|
| `mod.rs` | `AuthMode` enum (`Dev`/`Prod`), `Config` struct |
| `settings.rs` | `AppConfig` with all `FLOWPLANE_*` env vars |
| `tls.rs` | TLS configuration for API and xDS servers |

### `src/domain/` — Domain Types
Core business entities — one file per entity type.

| File | Purpose |
|---|---|
| `cluster.rs` | Cluster, ClusterEndpoint |
| `listener.rs` | Listener |
| `route_config.rs` | RouteConfig |
| `route.rs` | Route, RouteMatch, RouteAction |
| `filter.rs` | Filter, FilterType, FilterAttachment |
| `dataplane.rs` | Dataplane |
| `virtual_host.rs` | VirtualHost |
| `learning.rs` | LearningSession |
| `user.rs` | User, UserType |
| `team.rs` | Team, TeamMembership |
| `organization.rs` | Organization |
| `agent.rs` | Agent types |

### `src/mcp/` — MCP Server
MCP protocol implementation and tool definitions.

| File/Dir | Purpose |
|---|---|
| `handler.rs` | MCP message handler, tool dispatch |
| `tool_registry.rs` | Tool registration, authorization map (`TOOL_AUTHORIZATIONS`) |
| `tools/` | 14 tool modules (see below) |
| `prompts/` | MCP prompt definitions |
| `session.rs` | MCP session management |

**Tool modules** (14 files, ~64 total tools):

| Module | Category | Tools |
|---|---|---|
| `clusters.rs` | control_plane | `cp_create_cluster`, `cp_list_clusters`, `cp_get_cluster`, `cp_update_cluster`, `cp_delete_cluster`, `cp_get_cluster_health`, `cp_query_service` |
| `listeners.rs` | control_plane | `cp_create_listener`, `cp_list_listeners`, `cp_get_listener`, `cp_update_listener`, `cp_delete_listener`, `cp_get_listener_status`, `cp_query_port` |
| `routes.rs` | control_plane | `cp_create_route_config`, `cp_list_route_configs`, `cp_get_route_config`, `cp_update_route_config`, `cp_delete_route_config`, `cp_list_routes`, `cp_get_route` |
| `virtual_hosts.rs` | control_plane | `cp_list_virtual_hosts`, `cp_get_virtual_host` |
| `filters.rs` | control_plane | `cp_create_filter`, `cp_list_filters`, `cp_get_filter`, `cp_delete_filter`, `cp_attach_filter`, `cp_detach_filter`, `cp_list_filter_attachments` |
| `filter_types.rs` | control_plane | `cp_list_filter_types`, `cp_get_filter_type` |
| `dataplanes.rs` | control_plane | `cp_list_dataplanes`, `cp_get_dataplane` |
| `learning.rs` | control_plane | `cp_create_learning_session`, `cp_get_learning_session`, `cp_list_learning_sessions`, `cp_delete_learning_session`, `cp_activate_learning_session` |
| `schemas.rs` | control_plane | `cp_list_aggregated_schemas`, `cp_export_schema_openapi` |
| `openapi.rs` | control_plane | OpenAPI import tools |
| `ops_agent.rs` | control_plane | `ops_trace_request`, `ops_topology`, `ops_config_validate`, `ops_audit_query`, `ops_xds_delivery_status`, `ops_nack_history` |
| `devops_agent.rs` | control_plane | `devops_get_deployment_status` |
| `dev_agent.rs` | control_plane | Dev-specific tools |

**Two tool categories:**
- **control_plane** (`cp_*`, `ops_*`, `devops_*`) — Manage gateway configuration. Requires `team:{name}:cp:read` or `team:{name}:cp:write` scopes.
- **gateway_api** (`gw_*`) — Proxy requests to upstream services through the gateway. Requires `team:{name}:api:read` or `team:{name}:api:execute` scopes.

### `src/internal_api/` — Unified Request Layer
Bridges REST, MCP, and CLI into a single operation interface. All three surfaces route through here for consistency.

| File | Purpose |
|---|---|
| `clusters.rs` | `ClusterOperations` — list, get, create, update, delete, health |
| `listeners.rs` | `ListenerOperations` — CRUD + query port + status |
| `routes.rs` | `RouteOperations` — CRUD for routes and route configs |
| `filters.rs` | `FilterOperations` — CRUD + attach/detach |
| `virtual_hosts.rs` | `VirtualHostOperations` — CRUD |
| `dataplanes.rs` | `DataplaneOperations` — CRUD |
| `auth.rs` | Auth context resolution |
| `error.rs` | Unified error handling |

Each operation validates team membership, checks permissions, and delegates to services.

### `src/services/` — Business Logic
Service implementations called by the internal API layer. One service per domain entity.

### `src/storage/` — Data Layer
SQLx-based PostgreSQL repositories and migrations.

| File/Dir | Purpose |
|---|---|
| `pool.rs` | Database pool type (`DbPool = PgPool`) |
| `migrations.rs` | Filesystem-based migration runner |
| `migrations/*.sql` | SQL migration files |
| `repos/` | Repository implementations (one per entity) |
| `test_helpers.rs` | Testcontainers PostgreSQL setup |

### `src/xds/` — Envoy Control Plane
xDS protocol implementation — translates domain entities to Envoy protobuf resources.

| File/Dir | Purpose |
|---|---|
| `server.rs` | ADS gRPC server |
| `resources.rs` | Entity → protobuf conversion (clusters, routes, listeners) |
| `filters/http/` | 17 HTTP filter implementations |
| `filters/http/mod.rs` | `HttpFilterKind` enum (listener config) + `HttpScopedConfig` enum (per-route config) |
| `filters/injection/` | Filter chain injection: `listener.rs`, `route.rs`, `merger.rs` |
| `snapshot.rs` | xDS snapshot management |
| `delivery.rs` | ACK/NACK tracking |

**Filter dual representation:**
- `src/domain/filter.rs` — `FilterType` enum + `filter_registry()` metadata
- `src/domain/filter_schema.rs` — `FilterSchemaDefinition` loaded from YAML
- `src/services/filter_validation.rs` — `FilterConfigValidator` using JSON schemas
- `filter-schemas/built-in/` — YAML schema files for each filter type
- `src/xds/filters/http/mod.rs` — `HttpFilterKind` (listener) + `HttpScopedConfig` (per-route) enums with `to_any()`/`from_any()` methods

### Other Directories

| Directory | Purpose |
|---|---|
| `src/errors/` | Error types (`Error`, `Result`) |
| `src/internal_api/` | Internal API endpoints |
| `src/observability/` | OpenTelemetry tracing, Prometheus metrics |
| `src/openapi/` | OpenAPI spec generation |
| `src/schema/` | Schema inference from traffic |
| `src/secrets/` | Vault integration for secret management |
| `src/utils/` | Shared utilities |
| `src/validation/` | Input validation |

## Architecture Flow

```
CLI (src/cli/)
  → REST API (src/api/) ──┐
                           ├──→ Internal API (src/internal_api/)
MCP Client                 │      → Services (src/services/)
  → MCP Handler ───────────┘        → Storage (src/storage/)
    (src/mcp/handler.rs)             → xDS Push (src/xds/)
      → Tool modules
        (src/mcp/tools/)
```

All three surfaces converge on the Internal API layer, which delegates to services and storage. xDS pushes happen automatically when entities change.

**Key pattern:** The `internal_api/` layer is the single point where team validation, permission checks, and business logic execution happen — regardless of whether the request came from REST, MCP, or CLI.
