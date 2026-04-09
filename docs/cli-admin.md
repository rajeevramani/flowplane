# CLI: Admin & Platform

Platform administration, organizations, agents, MCP tools, WASM filters, mTLS, and certificates.

---

## Admin

Platform administration commands. `admin resources` and `admin reload-filter-schemas` require `admin:all` scope.

```bash
flowplane admin scopes [-o json|table]                   # List all 45 permission scopes
flowplane admin resources [-o json|table]                # Platform resource summary
flowplane admin reload-filter-schemas                    # Reload filter schemas from disk
```

---

## Organizations

Organization management. Requires `admin:all` scope (returns 403 in dev mode).

```bash
flowplane org list [--limit N] [--offset N] [-o json|yaml|table]
flowplane org get <NAME> [-o json|yaml|table]
flowplane org create -f org.json [-o json|yaml]
flowplane org delete <NAME> [--yes]
flowplane org members <NAME> [-o json|yaml|table]
```

---

## Agents

Org-scoped machine identities for programmatic access. All commands require `--org`.

```bash
flowplane agent list --org <ORG> [--limit N] [--offset N] [-o json|yaml|table]
flowplane agent create --org <ORG> -f agent.json [-o json|yaml]
flowplane agent delete --org <ORG> <NAME> [--yes]
```

---

## MCP Tools

List registered MCP tools and enable/disable MCP exposure on routes.

```bash
flowplane mcp tools [-o json|table]                      # List all MCP tools
flowplane mcp enable <ROUTE_ID>                          # Enable MCP on a route
flowplane mcp disable <ROUTE_ID>                         # Disable MCP on a route
```

`mcp tools` supports `-o json|table` (no yaml). Table columns: Name, Description, Enabled, Route.

---

## WASM Filters

Custom WASM filter management.

```bash
flowplane wasm list [--limit N] [--offset N] [-o json|table]
flowplane wasm get <ID> [-o json|yaml]
flowplane wasm create -f filter.json [-o json]
flowplane wasm update <ID> -f filter.json [-o json]
flowplane wasm delete <ID> [--yes]
flowplane wasm download <ID> -o <FILE>                   # Download WASM binary
```

---

## mTLS & Certificates

### mTLS Status

```bash
flowplane mtls status [-o json|yaml|table]               # mTLS gateway status
```

### Proxy Certificates

```bash
flowplane cert list [-o json|yaml|table]
flowplane cert get <ID> [-o json|yaml|table]
flowplane cert create -f cert-request.json [-o json|yaml]
flowplane cert revoke <ID> [--yes]
```
