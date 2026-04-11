# CLI: Ops & Diagnostics

Request tracing, configuration validation, xDS status, audit logs, route views, reports, and statistics.

---

## Request Tracing

### flowplane trace

Trace a request path through the gateway routing table: listener -> route config -> virtual host -> route -> cluster. Shows where routing succeeds or fails.

```
flowplane trace <PATH> [--port <PORT>] [-o json|yaml|table]
```

Exit code 0 = route found, exit code 1 = no match.

```bash
flowplane trace /api/users
flowplane trace /api/users --port 10001
flowplane trace /api/users -o json
```

---

## Configuration Health

### flowplane topology

Show the complete gateway layout — listeners, route configs, clusters, routes, and orphaned resources.

```
flowplane topology [-o json|yaml|table]
```

```bash
flowplane topology
flowplane topology -o json              # includes summary counts
```

### flowplane validate

Scan configuration for misconfigurations: orphan clusters, unbound route configs, empty virtual hosts, duplicate path matchers, and recent NACKs.

```
flowplane validate [-o json|yaml|table]
```

Exit code 0 = valid.

```bash
flowplane validate
flowplane validate -o json              # JSON with issues array and summary counts
```

---

## xDS

### flowplane xds status

Show per-dataplane xDS delivery status (ACK/NACK for CDS, RDS, LDS, EDS).

```
flowplane xds status [--dataplane <NAME>] [-o json|yaml|table]
```

```bash
flowplane xds status
flowplane xds status --dataplane dev-dataplane
flowplane xds status -o json
```

### flowplane xds nacks

Query recent NACK events — times Envoy rejected a config push.

```
flowplane xds nacks [--dataplane <NAME>] [--type <TYPE>] [--since <ISO8601>] \
  [--limit N] [--offset N] [-o json|yaml|table]
```

| Flag | Description |
|---|---|
| `--dataplane <NAME>` | Filter by dataplane |
| `--type <TYPE>` | Filter by xDS type: `CDS`, `RDS`, `LDS`, `EDS` |
| `--since <ISO8601>` | Only show NACKs after this timestamp |
| `--limit N` | Maximum results |
| `--offset N` | Pagination offset |

```bash
flowplane xds nacks
flowplane xds nacks --dataplane dev-dataplane --type CDS
flowplane xds nacks --since 2026-04-01T00:00:00Z --limit 50
```

---

## Audit

### flowplane audit list

View the audit trail of resource changes. Running `flowplane audit` with no subcommand defaults to `audit list`.

```
flowplane audit list [--resource-type <TYPE>] [--action <ACTION>] [--since <ISO8601>] \
  [--limit N] [--offset N] [-o json|yaml|table]
```

| Flag | Description |
|---|---|
| `--resource-type <TYPE>` | Filter by resource type (e.g., `cluster`, `listener`, `route`) |
| `--action <ACTION>` | Filter by action: `create`, `update`, `delete` |
| `--since <ISO8601>` | Only entries after this timestamp |
| `--limit N` | Maximum results (default: 20) |

```bash
flowplane audit list
flowplane audit list --resource-type cluster --action create
flowplane audit list --since 2026-04-01T00:00:00Z --limit 50
flowplane audit list -o json
```

---

## Route Views

Aggregated route views showing how routes map across listeners and clusters.

```bash
flowplane route-views list [-o json|yaml|table]          # Table: Route, Method, Cluster, Route Config
flowplane route-views stats [-o json|yaml|table]         # Summary statistics
```

Stats fields: `totalRoutes`, `totalRouteConfigs`, `totalVirtualHosts`, `uniqueClusters`, `uniqueDomains`, `mcpEnabledCount`.

---

## Reports

```bash
flowplane reports route-flows [-o json|yaml|table]       # Route flow data
```

---

## Stats

System and cluster statistics. Stats dashboard is auto-enabled in dev mode. In prod, enable via `PUT /api/v1/admin/apps/stats_dashboard`.

```bash
flowplane stats overview [-o json|yaml|table]            # System overview
flowplane stats clusters [-o json|yaml|table]            # All clusters
flowplane stats cluster <NAME> [-o json|yaml|table]      # Specific cluster
```
