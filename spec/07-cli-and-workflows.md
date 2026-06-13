# 07 — CLI Surface and UI Workflow Inventory

Behavioral specification extracted from Flowplane v1 (`/tmp/flowplane-v1`, docs verified against v0.2.10).
Part A documents the v1 CLI faithfully. Part B inventories every v1 UI workflow and recommends a v2 fate
(CLI / MCP / cut). **All fates in §5 are recommendations**, to be ratified into DECISIONS.md by the orchestrator.

Sources: `docs/reference/cli.md` (auto-generated from clap definitions in `src/cli/mod.rs`), `src/cli/*.rs`
(46 modules), `ui/src/routes/**` (62 pages), `ui/src/lib/api/client.ts` (~150 API methods).

---

## 1. v1 CLI Overview

### Binary and entry point

- Single binary: `flowplane` (`src/main.rs` → `flowplane::cli::run_cli()`).
- The same binary contains the control-plane server as a **hidden** subcommand `flowplane serve [--dev]`
  (used by the Docker entrypoint; `#[command(hide = true)]`).
- Running `flowplane` with no subcommand prints help and exits 0.
- `after_long_help` on the root command prints a "GETTING STARTED" block (init → expose → curl → list/status/down).

### Global flags (clap `global = true`, inherited by every subcommand)

| Flag | Meaning | Default |
|---|---|---|
| `--token <TOKEN>` | Bearer token (lowest precedence — see below) | — |
| `--token-file <FILE>` | Path to token file (plain text or OIDC JSON) | — |
| `--base-url <URL>` | API endpoint | `http://localhost:8080` |
| `--timeout <SECS>` | HTTP request timeout | `30` |
| `--team <NAME>` | Team context for resource commands | — |

Root-only (not global): `--database-url` (used only by `database` subcommands and `serve`),
`-v/--verbose` (enables verbose logging; **not listed in the auto-generated docs' global options** — drift).

### Config and state directory: `~/.flowplane/`

| File | Purpose |
|---|---|
| `config.toml` | CLI config: `token` (deprecated), `base_url`, `timeout`, `team`, `org`, `oidc_issuer`, `oidc_client_id`, `callback_url`. Written with mode 0600. |
| `credentials` | Token store. Plain-text token (dev: written by the control-plane container during `init`) or OIDC JSON (`access_token`, refresh data) written by `auth login`. |
| `docker-compose-init.yml` | Embedded compose file written by `flowplane init` (legacy name `docker-compose-dev.yml` still honored with a warning). |
| `encryption.key`, `certs/` | Dev encryption key and mTLS PKI — deliberately survive `down --purge-state`. |
| `dp/<NAME>/` | Per-dataplane local stack state for `dataplane up/down`. |

### Configuration precedence (resolved per-value in `src/cli/config.rs`)

- **Token** (changed in fp-xsli.3.13 — env now beats flags): `FLOWPLANE_TOKEN` env → `--token-file` →
  `~/.flowplane/credentials` → `config.toml` `token` (deprecated, warns) → `--token` flag (lowest!).
  Empty/whitespace env values fall through. Error message lists all four sources.
- **Base URL**: `FLOWPLANE_BASE_URL` env → `--base-url` flag → `config.toml` → default `http://localhost:8080`.
- **Timeout**: `--timeout` flag → `config.toml` → 30s. (Note: no env var — inconsistent with token/base-url.)
- **Team**: `--team` flag → `config.toml` → `FLOWPLANE_TEAM` env → error. (Flag beats env here — the
  *opposite* order of token resolution.)
- **Org**: `--org` flag → `config.toml` → `FLOWPLANE_ORG` env → error. (Same inverted order as team.)

Other env vars: `FLOWPLANE_API_HOST_PORT`, `FLOWPLANE_POSTGRES_HOST_PORT`, `FLOWPLANE_GATEWAY_PORT_RANGE`,
`FLOWPLANE_CP_IMAGE`, `FLOWPLANE_AGENT_IMAGE` (init fallbacks); `FLOWPLANE_BOOTSTRAP_TOKEN`
(`bootstrap initialize` — deliberately bypasses the normal token path); `FLOWPLANE_OIDC_ISSUER`,
`FLOWPLANE_OIDC_CLIENT_ID` (login fallbacks); `FLOWPLANE_XDS_ADVERTISE_ADDRESS` (server side, referenced
by `dataplane config`); `RUST_LOG`-style filtering via tracing.

### Auth / login flow (`src/cli/auth.rs`)

1. `auth login` probes `GET {base_url}/api/v1/auth/mode` (unauthenticated).
2. **Dev mode**: prints "no OIDC login needed — use the dev token from `flowplane init`", shows a
   truncated current token if `~/.flowplane/credentials` exists. No login is performed.
3. **Prod mode**: OIDC issuer/client-id resolved from `--issuer`/`--client-id` flags → `/auth/mode`
   response → env vars. The cached `config.toml` values are deliberately NOT consulted (stale-issuer bug).
   Then either:
   - **PKCE browser flow** (default): spins up a loopback HTTP callback server, opens the browser
     (`prompt=login` forces re-auth), waits up to 5 minutes, exchanges the code.
   - **Device code flow** (`--device-code`, present in code but missing from generated docs): for headless
     environments.
4. Credentials saved as OIDC JSON to `~/.flowplane/credentials`; issuer/client-id cached into `config.toml`.
- `auth token` prints the current access token; `auth whoami` shows the authenticated user; `auth logout`
  clears stored credentials.

### HTTP client and error presentation (`src/cli/client.rs`)

- `reqwest` with bearer auth on every request; optimistic concurrency via `If-Match: <version>` header on
  rate-limit update/delete.
- **Error presentation is raw**: any non-2xx becomes
  `anyhow!("HTTP request failed with status {status}: {body}")` — the unparsed JSON error body is dumped
  to the user. No mapping of 401→"run flowplane auth login", 403→scopes, 409→version conflict, etc.
- Deserialization failures echo the entire response body into the error.
- Errors bubble up through `anyhow` → wrapped as `crate::Error::config(...)` in `run_cli` → printed by
  Rust's default `main`-error path. **Exit code is effectively 0 (success) or 1 (any error)** — no
  differentiated exit codes. Explicit `std::process::exit(1)` only in: `serve` task failure,
  `database status` (pending migrations), `database validate` (schema invalid).

### Output conventions (`src/cli/output.rs`)

- `-o/--output` accepts `json`, `yaml`, `table` (case-insensitive). Defaults: `list` commands → `table`,
  `get/create/update` → `json`, `scaffold` → `yaml`, `config show` → `yaml`, `dataplane config` → `yaml`.
- JSON/YAML are generic serde dumps; **table format is hand-rolled per command** (fixed-width columns,
  `-`-rule separators, truncation with `...`). No shared table engine.
- Several commands (`expose`, `unexpose`, `list`, `status`, `doctor`, `logs`, `init`, `down`,
  `database *`, `mcp enable/disable`, `filter attach/detach`, all `delete` subcommands) print **plain
  prose only** with no `-o` flag at all — they cannot emit JSON.
- Human-facing progress goes to stderr in the compose machinery (`eprintln!`); resource output to stdout.
  Database commands use emoji status markers (✅/⚠️/❌).

### Shell completion

- **None.** No `clap_complete` dependency, no `completion` subcommand.

### The init/down compose machinery (`src/cli/compose.rs`, `compose_runner.rs`, `dev_certs.rs`)

- `flowplane init`: detects container runtime (DOCKER_HOST → /var/run/docker.sock → OrbStack → Rancher
  Desktop → podman socket → PATH fallback), generates/refreshes the dev mTLS PKI under
  `~/.flowplane/certs/` (CA + control-plane + agent + Envoy, SPIFFE SANs), generates/persists a dev
  encryption key, writes the embedded `docker-compose-init.yml` (PostgreSQL + control plane + Envoy +
  flowplane-agent), runs `docker compose up -d --force-recreate`, then polls the CP health endpoint with
  a cold/warm SLA (image-cache-aware) and verifies the CP wrote `~/.flowplane/credentials` (the dev token
  is minted server-side, not by the CLI). Port/image flags all have env fallbacks.
- `flowplane down [--volumes] [--purge-state]`: compose down; `--volumes` deletes DB data; `--purge-state`
  wipes credentials/bootstrap/compose file but intentionally keeps `encryption.key` and `certs/`.
  (**Doc drift**: the auto-generated reference shows `flowplane down` with no options.)
- `flowplane init-certs`: standalone PKI mint for systemd/K8s pre-boot, with `--if-missing-or-expired`
  + `--refresh-buffer` idempotency (humantime parsing with bespoke, operator-friendly error strings).
  (**Doc drift**: `--if-missing-or-expired` itself is missing from the generated flag list.)
- `flowplane logs [-f/--follow]`: thin wrapper over `docker compose logs`; refuses (politely, exit 0)
  when `base_url` is not loopback. (**Doc drift**: `--follow` not in generated docs.)
- `flowplane dataplane up/down <NAME>`: boots a local Envoy + flowplane-agent compose stack for an
  already-registered dataplane, fetching the Envoy bootstrap from the CP (`/envoy-config`), pinning Envoy
  minor version, managing per-DP state under `~/.flowplane/dp/<NAME>/` and mTLS client material.

---

## 2. Full Command Catalog

36 top-level commands (35 documented + hidden `serve`), **~140 leaf subcommands**. All resource commands
are team-scoped via `/api/v1/teams/{team}/...` unless noted. "std CRUD flags" = `-f/--file` (YAML/JSON
spec) on create/update, `-o/--output` per conventions above.

### Stack lifecycle (local, no API token needed)

| Command | Flags | Behavior / API |
|---|---|---|
| `init` | `--api-port`, `--xds-port` (18000), `--postgres-port`, `--admin-port`, `--agent-health-port`, `--gateway-port-range`, `--cp-image`, `--agent-image` (all with env fallbacks) | Compose machinery (§1). Defaults are examples only; resolved ports are written into generated artifacts. |
| `down` | `--volumes`, `--purge-state` | Compose down. Plain output. |
| `init-certs` | `--out`, `--trust-domain` (flowplane.local), `--lifetime-hours` (24), `--if-missing-or-expired`, `--refresh-buffer` (1h) | Local PKI generation. Plain output. |
| `logs` | `-f/--follow` | `docker compose logs`. Loopback-only. |
| `database migrate\|status\|list\|validate` | `migrate --dry-run` | Direct DB connection (`--database-url`/`DATABASE_URL`), not the REST API. status/validate exit 1 on failure. |

### Auth and config

| Command | Flags | Behavior / API |
|---|---|---|
| `auth login` | `--device-code`, `--callback-url`, `--issuer`, `--client-id` | GET `/api/v1/auth/mode`, then OIDC PKCE/device flow (§1). |
| `auth token` / `whoami` / `logout` | — | Print token / show identity / clear `~/.flowplane/credentials`. |
| `config init` | `--force` (undocumented in generated ref) | Create `~/.flowplane/config.toml`. |
| `config show` | `-o` (default yaml) | Token redacted. |
| `config set <KEY> <VALUE>` | keys validated: `token`, `base_url`, `timeout`, `team`, `org`, `oidc_issuer`, `oidc_client_id`, `callback_url` | Writes config.toml. |
| `config path` | — | Prints path. |

### Gateway resources (noun → verbs; team-scoped)

| Noun | Verbs | API base | Notes |
|---|---|---|---|
| `cluster` | create, list (`--service`, `--limit`, `--offset`), get, update, delete, scaffold | `/api/v1/teams/{team}/clusters` | scaffold emits a local template, no API call. |
| `listener` | create, list (`--protocol`), get, update, delete, scaffold | `/api/v1/teams/{team}/listeners` | |
| `route` | create, list (`--cluster`), get, update, delete, scaffold | `/api/v1/teams/{team}/route-configs` | The noun is `route` but the resource is a **route-config** (whole config, not an individual route). |
| `vhost` | list (`--route-config`), get `<ROUTE_CONFIG> <VHOST_NAME>` | `/route-configs/{rc}/virtual-hosts` | Read-only. |
| `filter` | create, list, get, update, delete, attach (`--listener`, `--order`), detach (`--listener`), types, type `<NAME>`, scaffold `<TYPE>` | `/filters`, attach: `/listeners/{l}/filters[/{id}]`, types: `/api/v1/filter-types` (unscoped) | Listener-level attach only — no route/vhost-scoped configure (UI has it; see §4). |
| `dataplane` | list, get, create, update, delete, config (`--cert-path`, `--key-path`, `--ca-path`, `--xds-host`, `--xds-port`), scaffold, up, down | `/dataplanes`, `/dataplanes/{n}/envoy-config` | `config` returns Envoy bootstrap YAML. up/down are local compose. |
| `secret` | create (`--name`, `--type`, `--config` JSON string, `--description`, `--expires-at`), list (`--type`), get, delete, rotate (`--config`) | `/secrets`, `/secrets/{id}/rotate` | **create takes inline flags, not `-f`** — inconsistent with every other create. No `update`. |
| `rate-limit domain` | create, list, get, update (`--version` → If-Match), delete (`--version`), scaffold | `/rate-limit-domains` | Only resource with optimistic concurrency in the CLI. ID-addressed, not name-addressed. |
| `rate-limit policy` | create, list (`--domain`), get, update (`--version`), delete (`--version`), scaffold | `/rate-limit-policies` | |
| `team` | create (`--org`), list (`--org`), get, update, delete | `/api/v1/orgs/{org}/teams[/{name}]`; list w/o org: `/api/v1/admin/teams` | |
| `apply` | `-f <file-or-dir>` | GET-then-PUT-or-POST per manifest against `/api/v1/teams/{team}/{kind-path}` | kubectl-style; kinds: Cluster, Listener, RouteConfig, Filter, Secret, Dataplane. |

### Expose / import / learn / schema

| Command | Flags | API |
|---|---|---|
| `expose <UPSTREAM>` | `--name`, `--path` (repeatable), `--port` | POST `/api/v1/teams/{team}/expose`. Plain output + curl hint. |
| `unexpose <NAME>` | — | DELETE `/expose/{name}`. |
| `list` | — | GET `/listeners?limit=1000`, rendered as exposed services. Plain table, no `-o`. |
| `import openapi <FILE>` | `--name`, `--port` (10000) | POST `/openapi/import?{q}` (checks a dataplane exists first). |
| `import list/get/delete` | std | `/openapi/imports[/{id}]` (get/delete are unscoped `/api/v1/openapi/imports/{id}`). |
| `learn start` | `--name`, `--route-pattern`, `--cluster-name`, `--http-methods`, `--target-sample-count`, `--max-duration-seconds`, `--triggered-by`, `--deployment-version` | POST `/learning-sessions`. |
| `learn stop/cancel/activate <NAME_OR_ID>` | std | POST `/learning-sessions/{id}/stop`, DELETE-ish cancel, `/activate`. |
| `learn list/get/health` | `--status`, paging | `/learning-sessions`, `/ops/learning/{id}/health`. |
| `learn export` | `--session`, `--min-confidence`, `--title`, `--version`, `--description`, `-o <FILE>` | `/aggregated-schemas/export`. **`-o` here means output FILE, not format** — collides with the global convention. |
| `schema list/get/compare/export` | list: `--session`, `--min-confidence`, `--path`, `--method`; compare: `--with` | `/aggregated-schemas...`. `schema export -o` is also a file path. |

### Diagnostics and ops (team-scoped `/ops/` endpoints)

| Command | Flags | API |
|---|---|---|
| `status [NAME]` | — | listeners/clusters/filters `?limit=1` + stats overview, or listener lookup. Plain output, no `-o`. |
| `doctor` | — | Health probes against base URL + team resources. Plain checklist output. |
| `trace` | `--request-id`, `--trace-id`, `--path`, `--limit` | GET `/api/v1/teams/{team}/ops/trace?...`; persisted audit/outbox correlation, not a live Envoy admin trace. |
| `topology` / `validate` | `-o` | `/ops/topology`, `/ops/validate`. |
| `xds status` | `--team` | `/api/v1/teams/{team}/xds/status`. |
| `xds nacks` | `--team` | `/api/v1/teams/{team}/xds/nacks`. |
| `audit [list]` | `--resource-type`, `--action`, `--since`, `--limit` (20) | `/ops/audit`. Bare `flowplane audit` defaults to `list`. |
| `stats overview/clusters/cluster <NAME>` | `-o` | `/stats/overview`, `/stats/clusters[/{n}]`. |
| `route-views list/stats` | `-o` | `/api/v1/route-views[/stats]` (unscoped). |
| `reports route-flows` | `-o` | `/api/v1/reports/route-flows` (unscoped). |

### MCP / WASM

| Command | Flags | API |
|---|---|---|
| `mcp tools` | `-o` | GET `/mcp/tools`. |
| `mcp enable/disable <ROUTE_ID>` | — | POST `/routes/{id}/mcp/enable|disable`. Addressed by **route ID**, while everything else is name-addressed. |
| `wasm list/get/create/update/delete/download` | std; download `-o <FILE>` | `/custom-filters[...]`, `/{id}/download` (binary). |

### Platform administration

| Command | Flags | API |
|---|---|---|
| `admin resources` / `scopes` | `-o` | `/api/v1/admin/resources/summary`, `/admin/scopes`. |
| `admin audit` | `--org`, `--resource-type`, `--action`, `--start-date`, `--end-date`, `--limit` (50), `--offset` | `/admin/audit/feed`. Different flag names from team `audit` (`--since` vs `--start-date`). |
| `admin reload-filter-schemas` | — | POST `/admin/filter-schemas/reload`. |
| `admin rate-limit reap-tombstones` | `--older-than` (30d) | POST `/admin/rate-limit/reap-tombstones`. |
| `admin rls force-repush` | `--target` (all/limits/domains/identities) | POST `/admin/rls/force-repush`. |
| `org list/get/create/delete/members` | std | `/api/v1/admin/organizations[...]`. **No `org update`.** |
| `agent list/create/delete` | `--org` | `/api/v1/orgs/{org}/agents`. No get/update; no grant management. |
| `mtls status` | `-o` | `/api/v1/mtls/status`. |
| `cert list/get/create/revoke` | create: `-f`, `--write-dir`; revoke: `--reason` | `/proxy-certificates`, `/{id}/revoke`. |
| `bootstrap initialize` | `--org`, `--display`, `--team` (default) | POST `/api/v1/bootstrap/initialize`, authed via `FLOWPLANE_BOOTSTRAP_TOKEN` env (bypasses normal token path). |

---

## 3. CLI Consistency Critique (feeds spec/08)

- **Noun-verb grammar is mostly followed but leaks**: `route-views`, `reports`, `stats`, `mcp`, `xds`
  are pseudo-nouns wrapping one or two reads; `list`, `status`, `doctor`, `trace`, `topology`,
  `validate`, `expose`, `unexpose` are bare top-level verbs. 36 top-level commands is far past the
  gh/kubectl sweet spot; diagnostics alone account for 8 of them.
- **`route` doesn't mean route**: `flowplane route` CRUDs route-*configs*; individual routes inside
  vhosts are only editable in the UI. `vhost` is read-only. `mcp enable` takes a route **ID** while
  every other command takes names. Three different addressing schemes (name, UUID, numeric schema ID).
- **`-o` is overloaded**: output *format* everywhere except `learn export`, `schema export`, and
  `wasm download`, where `-o` is an output *file path*.
- **No uniform `--json`**: ~20 commands are plain-prose-only (`expose`, `list`, `status`, `doctor`,
  all deletes, `mcp enable/disable`, `filter attach/detach`, `database *`) — they cannot be scripted
  with structured output. Defaults flip between json/yaml/table by verb.
- **Create input is inconsistent**: every resource uses `-f` spec files except `secret create`
  (inline `--config '<JSON string>'`) and `expose`/`learn start` (flag-soup). `apply` exists but covers
  only 6 kinds (no rate-limit, org, team, agent, cert).
- **Precedence rules contradict each other**: token = env > file > flag (flag is *lowest*);
  team/org = flag > config > env; base-url = env > flag > config; timeout has no env var at all.
  Each value has a different mental model.
- **Error presentation is raw HTTP**: `HTTP request failed with status 409 Conflict: {"error":...}` —
  no exit-code taxonomy (everything is 1), no remediation hints (401 doesn't say "run flowplane auth
  login"), JSON bodies dumped verbatim.
- **Doc/code drift in the "auto-generated" reference**: `down --volumes/--purge-state`,
  `logs --follow`, `auth login --device-code`, `config init --force`, `init-certs
  --if-missing-or-expired`, and global `--verbose` are all in the code but absent from
  `docs/reference/cli.md` despite the CI change-guard.
- **No shell completion**, no `--help`-discoverable command grouping, no aliases.
- **Mixed transport layers**: `database *` talks to PostgreSQL directly; everything else uses the REST
  API; `init/down/logs/dataplane up` shell out to docker compose. A single binary doing CLI client +
  server (`serve`) + migration runner.
- **Pagination is manual** (`--limit`/`--offset`) with inconsistent caps (`list` hardcodes
  `limit=1000`; `apply` reconciles with `limit=1000`), and some lists lack paging entirely
  (`cert list`, `wasm list`, `org list`).
- **Optimistic concurrency only for rate-limit** (`--version`/If-Match); all other updates are
  last-writer-wins, while the UI uses version-aware updates for the same resources.

---

## 4. UI Page Inventory

62 pages under `ui/src/routes/`. API calls are `apiClient.*` methods (`ui/src/lib/api/client.ts`) plus
helpers in `lib/api/routes.ts` / `route-views.ts`; `getSessionInfo` (GET `/api/v1/auth/session`) is used
by nearly every page for identity/team context and is omitted below unless it is the point of the page.
CLI coverage: **yes** (v1 CLI does the task), **partial**, **no**.

| Page route | User task | Key API calls | CLI coverage |
|---|---|---|---|
| `/` (root) | Redirect to login/bootstrap/dashboard | getBootstrapStatus, getSessionInfo | n/a |
| `/login` | Session login (redirect to `/api/v1/auth/login`, cookie session) | login | yes (auth login, token-based) |
| `/bootstrap` | First-run platform setup wizard | getBootstrapStatus, bootstrapInitialize | partial (`bootstrap initialize`, token-gated, no status check) |
| `/dashboard` | Team overview; platform KPI dashboard for admins | listClusters/Listeners/RouteConfigs/Imports; PlatformDashboard: getAdminResourceSummary, getAuditVolume, adminAuditFeed, getXdsHealth, getOrgsActivity, getProvisioningRecent, getMcpConnectionsSummary | partial (`status`, `admin resources`, `admin audit`; no xds-health/org-activity/provisioning/mcp-summary rollups) |
| `/clusters` | List/delete clusters, see import linkage | listClusters, deleteCluster, listImports | yes |
| `/clusters/create`, `/clusters/[name]/edit` | Form-based cluster create/edit (visual endpoint/LB editor) | createCluster, getCluster, updateCluster | yes (file-based, no interactive form) |
| `/listeners` | List/delete listeners; dataplane + import context | listListeners, deleteListener, listDataplanes, listRouteConfigs, listImports | yes |
| `/listeners/create`, `/listeners/[name]/edit` | Listener create/edit incl. filter install/uninstall on listener | createListener, getListener, updateListener, listListenerFilters, installFilter, uninstallFilter | yes (create/update + filter attach/detach) |
| `/route-configs` | List route configs; per-vhost/route drill-down; MCP enable from list | listRouteConfigs, deleteRouteConfig, listVirtualHosts, listRoutesInVirtualHost, listRouteConfigFilters, getMcpStatus, enableMcp, listMcpTools, listRouteViews | partial (no per-route drill-down listing; `route-views list` approximates) |
| `/route-configs/create` | Route config create form | createRouteConfig, listClusters | yes |
| `/route-configs/[id]/edit` | Edit route config; per-vhost/per-route **filter configuration**; **bulk MCP enable/disable** | getRouteConfig, updateRouteConfig, listRouteHierarchyFilters, listVirtualHostFilters, configureFilter, removeFilterConfiguration, bulkEnableMcp, bulkDisableMcp, getMcpStatus | **no** (filter configure at route/vhost scope and bulk MCP have no CLI) |
| `/routes/[id]/edit` | Edit a **single route** inside a vhost; per-route filters + MCP toggle | getSingleRouteForEdit, updateSingleRoute, deleteSingleRoute, getRouteFilters, configureFilter, enableMcpForRoute/disableMcpForRoute | **no** (CLI edits whole route-configs only; `mcp enable` covers the toggle) |
| `/filters` | List filters w/ install status; delete | listFilters, getFilterStatus, deleteFilter | partial (no status rollup) |
| `/filters/create` | Create filter from type-driven schema form | listFilterTypes, createFilter | yes (`filter types` + `filter create -f`) |
| `/filters/[id]/edit` | Edit filter config against its JSON schema | getFilter, getFilterType, updateFilter, deleteFilter, getFilterStatus | yes |
| `/filters/[id]/install` | Install/uninstall filter on listeners with ordering | getFilter, listListeners, listFilterInstallations, installFilter, uninstallFilter | yes (`filter attach/detach --order`) |
| `/filters/[id]/configure` | Scope filter config to route-config/vhost/route | getFilterType, listRouteConfigs, listVirtualHosts, listRoutesInVirtualHost, configureFilter, listFilterConfigurations, removeFilterConfiguration | **no** |
| `/custom-filters`, `/[id]`, `/[id]/edit`, `/upload` | WASM filter list/detail/edit/upload/download | listCustomWasmFilters, getCustomWasmFilter, createCustomWasmFilter, updateCustomWasmFilter, deleteCustomWasmFilter, downloadCustomWasmFilterBinary | yes (`wasm *`) |
| `/secrets`, `/secrets/create`, `/secrets/[id]/edit` | Secret CRUD incl. **update**, rotate, and **external secret references** | listSecrets, createSecret, createSecretReference, getSecret, updateSecret, rotateSecret, deleteSecret | partial (no `secret update`, no reference creation) |
| `/dataplanes`, `/create`, `/[id]/edit` | Dataplane CRUD; fetch Envoy bootstrap; generate proxy cert from edit page | listDataplanes, createDataplane, getDataplane, updateDataplane, deleteDataplane, getDataplaneEnvoyConfig, getMtlsStatus, generateProxyCertificate | yes (`dataplane *`, `cert create`, `mtls status`) |
| `/certificates` | List/revoke proxy certificates | listProxyCertificates, revokeProxyCertificate | yes (`cert list/revoke`) |
| `/expose` | One-click expose/unexpose with resource preview | expose, unexpose, listClusters/Listeners/RouteConfigs | yes |
| `/imports`, `/imports/import`, `/imports/[id]` | OpenAPI import wizard, list, detail incl. created resources | importOpenApiSpec, listImports, getImport, deleteImport, listOrgTeams | yes (`import *`) |
| `/learning`, `/create`, `/[id]` | Learning session list/create/detail, stop/cancel | listLearningSessions, createLearningSession, getLearningSession, stopLearningSession, cancelLearningSession | yes (`learn *`) |
| `/learning/schemas`, `/[id]` | Discovered schema browse/detail, version compare, OpenAPI export | listAggregatedSchemas, getAggregatedSchema, compareSchemaVersions, exportSchemaAsOpenApi | yes (`schema *`) |
| `/mcp-tools` | MCP tool catalog; edit tool metadata; apply learned schemas to tools | listMcpTools, updateMcpTool, checkLearnedSchema, applyLearnedSchema | partial (`mcp tools` list only — no update/apply-learned-schema) |
| `/mcp-connections` | Live MCP connection observability (sessions, clients) | listMcpConnections | **no** |
| `/stats` | Live cluster/system stats with polling | isStatsEnabled, getStatsOverview, getClusterStats (via stores, 30s poll) | partial (`stats *` snapshot, no watch) |
| `/audit-log` | Team/org audit browsing with volume charts | getTeamAuditFull, getTeamAuditVolume, getOrgAuditLogs, getOrgAuditVolume | partial (`audit list`; no volume aggregates/org view) |
| `/organizations/[org]/settings` | View current org settings | getCurrentOrg | partial (`org get`) |
| `/organizations/[org]/teams`, `/create`, `/[team]` | Org-scoped team CRUD | listOrgTeams, createOrgTeam, getOrgTeam, updateOrgTeam, deleteOrgTeam | yes (`team *`) |
| `/organizations/[org]/teams/[team]/members` | **Team membership management** | listTeamMembers, addTeamMember, removeTeamMember, listOrgMembers, listPrincipalGrants | **no** |
| `/organizations/[org]/agents`, `/create`, `/[agent]` | Agent CRUD + **principal grant management** | listOrgAgents, createOrgAgent, deleteOrgAgent, createPrincipalGrant, listPrincipalGrants, deletePrincipalGrant | partial (`agent list/create/delete`; grants have no CLI) |
| `/profile/password` | Deep-link to IdP account console | (none — builds issuer URL) | n/a |
| `/admin/organizations`, `/create`, `/[id]` | Platform org CRUD + **org member/role management** | listOrganizations, createOrganizationDetailed, getOrganization, updateOrganization, deleteOrganization, listOrgMembers, updateOrgMemberRole, removeOrgMember | partial (`org *` lacks update + member add/role/remove) |
| `/admin/teams`, `/create`, `/[id]` | Admin team CRUD by ID across orgs | adminListTeams, adminCreateTeam, adminGetTeam, adminUpdateTeam, adminDeleteTeam, listOrganizations | yes (`team *`) |
| `/admin/audit-log` | Platform-wide audit feed + volume KPI | adminAuditFeed, getAuditVolume | partial (`admin audit`; no volume) |
| `/admin/scopes` | Browse registered authz scopes | listAllScopes | yes (`admin scopes`) |
| `/admin/apps` | **Enable/disable platform apps (feature toggles)** | listApps, setAppStatus | **no** |
| `/admin/system` | Reload filter schemas | reloadFilterSchemas | yes (`admin reload-filter-schemas`) |
| `/admin/tenants/[org]` | Per-org governance drill-in (resources, filters, activity) | getAdminResourceSummary, getFiltersByOrg, getOrgsActivity | **no** (only the resource summary exists as `admin resources`) |

**Totals**: 62 pages → ~38 distinct workflows. CLI coverage: ~21 yes, ~10 partial, **7 with no CLI
equivalent at all** (route/vhost-scoped filter configuration, single-route editing + bulk MCP, MCP tool
update/apply-learned-schema, MCP connections observability, team/org membership + principal grants,
admin apps toggles, per-org governance drill-in).

---

## 5. Workflow Fates (RECOMMENDATIONS — to be ratified in DECISIONS.md)

| Workflow (v1 UI) | Recommended v2 fate | Rationale |
|---|---|---|
| Login / session | CLI equivalent: `flowplane auth login` (PKCE + device-code) | Already exists; cookie-session login is UI-only and dies with the UI. |
| First-run bootstrap wizard | CLI equivalent: `flowplane bootstrap` (add a `status` check) | Token-gated CLI path already exists; wizard adds nothing. |
| Resource CRUD (cluster/listener/route-config/filter/secret/dataplane) | CLI equivalent: `flowplane <noun> create/get/list/update/delete` + `apply -f` | Direct carry-over; v2 should make `apply` cover all kinds. |
| Form-based visual editors (cluster/listener/route builders) | CLI equivalent: `scaffold` templates + `--edit`-in-$EDITOR; MCP equivalent for conversational building | The forms are UX sugar over the same payloads. |
| Per-vhost / per-route filter configuration | CLI equivalent (new): `flowplane filter configure <name> --route-config X [--vhost Y] [--route Z]` | API exists (`configureFilter` scopes); v1 CLI gap that must close or the feature is unreachable in v2. |
| Single-route edit/delete inside a vhost | CLI equivalent (new): `flowplane route edit <route-id>` or structured `route-config patch` | API exists; otherwise users must round-trip whole route-configs. |
| MCP enable/disable on routes (incl. bulk) | CLI equivalent: extend `flowplane mcp enable/disable --all/--route ...`; MCP equivalent (self-referential, natural fit) | Single-route toggle exists; bulk + refresh-schema do not. |
| MCP tool metadata update / apply learned schema | MCP equivalent (manage tools via MCP itself) + `flowplane mcp tool update` | Core to the MCP story; CLI fallback needed for CI. |
| MCP connections observability | CLI equivalent (new): `flowplane mcp connections` | Pure read; trivial to expose. |
| Learning sessions + schema browse/export | CLI equivalent: `flowplane learn *` / `schema *` (exists) | Carry over. |
| OpenAPI import wizard | CLI equivalent: `flowplane import openapi` (exists) | Carry over. |
| Expose / unexpose | CLI equivalent: `flowplane expose/unexpose` (exists) | Carry over; flagship dev workflow. |
| Stats dashboard (30s polling, KPI tiles) | **Candidate cut** (keep `flowplane stats` snapshot + `--watch`; push charts to Prometheus/Grafana) | Visualization-only; control plane shouldn't own dashboards without a UI. |
| Team/org audit browsing + volume charts | CLI equivalent: `flowplane audit list` (+ org flag); **cut volume charts** (derive from `--json` output) | Data access matters; chart rendering doesn't. |
| Platform admin dashboard (xDS health, org activity, provisioning feed, MCP summary KPIs) | **Candidate cut** for KPI widgets; CLI equivalent (new) `flowplane admin health` for the xDS rollup | Aggregation endpoints can stay, but dashboard rendering is UI-bound; xDS health is genuinely useful operationally. |
| Per-org governance drill-in (`/admin/tenants/[org]`) | **Candidate cut**; partial CLI via `flowplane admin resources --org` | Niche multi-tenant reporting; revisit if v2 targets platform teams. |
| Org CRUD + member/role management | CLI equivalent (new): `flowplane org update`, `flowplane org member add/remove/set-role/invite` | Without a UI this is the only way to administer orgs; must exist. |
| Team membership management | CLI equivalent (new): `flowplane team member add/remove/list` | Same reasoning. |
| Agent CRUD + principal grants | CLI equivalent: `flowplane agent *` + new `flowplane agent grant add/list/remove` | Machine identities are CLI/CI-native anyway. |
| Proxy certs + mTLS status | CLI equivalent: `flowplane cert *`, `mtls status` (exists) | Carry over. |
| WASM filter upload/download | CLI equivalent: `flowplane wasm *` (exists) | Carry over. |
| Secret update + external secret references | CLI equivalent (new): `flowplane secret update`, `secret create --from-ref` | API exists; CLI gap. |
| Admin apps enable/disable (feature toggles) | CLI equivalent (new): `flowplane admin app list/enable/disable` — or **candidate cut** if v2 drops the app-toggle concept | Tiny surface; decision hinges on whether "apps" survive into v2. |
| Change password page | **Candidate cut** (print IdP account URL in `auth whoami`) | The page is literally a link to the IdP console. |

---

## 6. Gaps and Smells (feeds spec/08)

1. **The UI is API-richer than the CLI.** ~150 ApiClient methods vs ~140 CLI leaf commands, but the
   overlap is incomplete in both directions: 7 UI workflow families have zero CLI path (membership,
   grants, scoped filter config, single-route edit, MCP tool mgmt, MCP connections, app toggles). In a
   UI-less v2 every one of these is a hard blocker unless explicitly cut.
2. **Version-aware updates exist in the API and UI but not the CLI** (except rate-limit). v2 needs a
   uniform concurrency story (ETag/If-Match everywhere or nowhere).
3. **Addressing chaos**: names (clusters/listeners/route-configs), UUIDs (filters via name→id lookup
   inside the CLI, imports, secrets, certs, wasm), numeric ids (schemas), route IDs (mcp). v2 should
   pick one user-facing handle per resource.
4. **No structured output for mutations** (`delete`, `attach`, `expose`, `mcp enable` print prose) →
   unscriptable; no `--quiet`/`-o json` on exactly the commands CI pipelines call.
5. **Exit code monoculture** (0/1) and raw HTTP error dumps; no machine-readable error envelope
   pass-through.
6. **`apply` is half-declarative**: 6 kinds, no delete/prune, no dry-run, no diff, list-reconcile with
   hardcoded `limit=1000`.
7. **Doc generation drift** despite a CI change-guard — the generator misses flags (`--follow`,
   `--device-code`, `--force`, `--if-missing-or-expired`, `down` flags, global `--verbose`), meaning
   the guard checks staleness of the file, not completeness of the generator.
8. **Local-stack coupling**: `init/down/logs/dataplane up` embed docker-compose and PKI generation in
   the API client binary; v2 should decide whether dev-stack orchestration is in-binary or a separate
   tool/plugin.
9. **Auth precedence inversion** (env beats explicit `--token` flag) was a deliberate v1 fix but
   violates least-surprise vs every mainstream CLI (flag > env > file); team/org resolve in the
   opposite order — v2 must define one precedence rule globally.
10. **Per-command hand-rolled tables** and three output defaults; no pager, no column selection, no
    `-o jsonpath/template`.
11. **Session endpoints the CLI never uses** (`/api/v1/auth/session`, cookie login) exist purely for
    the UI; v2 can drop session auth entirely if the UI is gone (MCP + PAT/OIDC only).
12. **No shell completion, no plugin mechanism, no telemetry/update-check** — green-field opportunities
    for the gh/kubectl-style redesign.
