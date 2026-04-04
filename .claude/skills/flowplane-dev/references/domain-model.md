# Domain Model Reference

## Entity Chain

The core request flow through the gateway:

```
Listener (address:port + filter chain)
  → RouteConfig (routing rules, bound to listener via listener_route_configs)
    → VirtualHost (domain grouping, matched by Host header)
      → Route (path match → action)
        → Cluster (backend service + LB policy)
          → Endpoints (server addresses)
```

## Entities

### Cluster
Backend service definition with endpoints and load balancing.

**Key fields:**
- `name` — unique identifier
- `endpoints` — list of `host:port` addresses
- `lb_policy` — load balancing algorithm (`ROUND_ROBIN`, `LEAST_REQUEST`, `RANDOM`, `RING_HASH`, `MAGLEV`)
- `team` — owning team (isolation boundary)

**Envoy mapping:** CDS (Cluster Discovery Service) + EDS (Endpoint Discovery Service)

**Source:** `src/domain/cluster.rs`

### Listener
Network entry point — address, port, and filter chain.

**Key fields:**
- `name` — unique identifier
- `address` — bind address (usually `0.0.0.0`)
- `port` — listen port (10000-10020 range for Envoy)
- `filter_chain` — ordered list of HTTP filters
- `route_configs` — bound route configurations

**Envoy mapping:** LDS (Listener Discovery Service)

**Source:** `src/domain/listener.rs`

### RouteConfig
Collection of routing rules (virtual hosts and routes).

**Key fields:**
- `name` — unique identifier
- `virtual_hosts` — list of VirtualHost definitions

**Envoy mapping:** RDS (Route Discovery Service) — maps to `RouteConfiguration`

**Source:** `src/domain/route_config.rs`

### VirtualHost
Domain grouping within a route config.

**Key fields:**
- `name` — identifier
- `domains` — list of domain patterns matched against the `Host` header (e.g., `["*"]`, `["api.example.com"]`)
- `routes` — ordered list of routes (first match wins)
- `typedPerFilterConfig` — per-virtual-host filter overrides

**Envoy mapping:** `VirtualHost` within a `RouteConfiguration`

### Route
Single path match with an action.

**Key fields:**
- `name` — identifier
- `match.path` — path matcher with `type` and `value`:
  - `exact` — exact string match
  - `prefix` — prefix match
  - `regex` — regular expression
  - `template` — URI template with captures (e.g., `/users/{user_id}`)
- `action` — what to do with matched requests:
  - `forward` — route to a cluster (`action.cluster`)
  - `weighted` — split traffic across clusters (`action.clusters` with weights)
  - `redirect` — HTTP redirect
- `typedPerFilterConfig` — per-route filter overrides

**Source:** `src/domain/route.rs`

### Endpoints
Actual server addresses within a cluster.

**Key fields:**
- `address` — hostname or IP
- `port` — port number

### Filter
Standalone policy resource that attaches to listeners or route configs.

**Key fields:**
- `name` — unique identifier
- `filter_type` — one of 11 types (10 implemented + 1 not yet implemented — see filter-types.md)
- `configuration` — type-specific JSON config (`typed_config`)
- `enabled` — whether the filter is active
- `listenerInstallations` — where attached at listener level
- `routeConfigInstallations` — where attached at route config level

**Source:** `src/domain/filter.rs`

### Dataplane
Registered Envoy proxy instance.

**Key fields:**
- `name` — identifier
- `team` — owning team
- `description` — human-readable description

**Envoy mapping:** Envoy node connected via ADS (Aggregated Discovery Service) over gRPC

## xDS Resource Mapping

| Flowplane Entity | xDS Resource Type | Type URL |
|---|---|---|
| Cluster | CDS | `type.googleapis.com/envoy.config.cluster.v3.Cluster` |
| Endpoints | EDS | `type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment` |
| Listener | LDS | `type.googleapis.com/envoy.config.listener.v3.Listener` |
| RouteConfig | RDS | `type.googleapis.com/envoy.config.route.v3.RouteConfiguration` |
| Secrets (TLS) | SDS | `type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret` |

Flowplane uses ADS (Aggregated Discovery Service) — all resource types are multiplexed over a single gRPC stream. This ensures ordering guarantees: CDS/EDS are delivered before RDS/LDS that reference them.

## Filter Attachment Model

Filters attach at two levels:

### Listener-level (global)
- Filter applies to ALL requests through the listener
- Attached via `cp_attach_filter` with `listener` parameter
- Each attachment has an `order` value (unique per listener, controls execution sequence)
- Lower order = earlier in the chain

### Route-level (per-route override)
- Override or disable a listener-level filter for specific routes/virtual hosts
- Set via `typedPerFilterConfig` on Route or VirtualHost
- Key is the Envoy filter name (e.g., `envoy.filters.http.jwt_authn`)
- Value is filter-specific config or `{"disabled": true}` to skip entirely
- Not all filter types support per-route overrides

### Router filter
Flowplane auto-appends `envoy.filters.http.router` as the last filter in every listener's chain. Never add it manually.

## Database Tables

Key tables backing the domain model (in `src/storage/migrations/`):

| Table | Entity |
|---|---|
| `clusters` | Cluster definitions |
| `cluster_endpoints` | Endpoints within clusters |
| `listeners` | Listener definitions |
| `listener_route_configs` | Listener ↔ RouteConfig binding |
| `route_configs` | Route configuration definitions |
| `virtual_hosts` | Virtual hosts within route configs |
| `routes` | Individual routes |
| `filters` | Filter definitions |
| `filter_attachments` | Filter ↔ Listener/RouteConfig binding |
| `dataplanes` | Registered Envoy instances |
| `xds_delivery_status` | Per-dataplane ACK/NACK tracking |

All tables include `team` column for tenant isolation.
