# Changelog

All notable changes to Flowplane are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Flowplane v2 is a ground-up Rust/PostgreSQL control plane. It is a new product, not an
upgrade of an earlier Flowplane line — there is no in-place migration path from any prior
version. `1.0.0` is its first stable release and the point at which the public REST API,
CLI surface, and configuration contract become subject to semantic versioning.

## [2.1.0] - 2026-06-27

Minor release. **CLI Tier-2 help quality: `flowplane --help` is now self-sufficient.** Every
command and subcommand renders a one-line summary, every flag and positional renders help text,
and every spine workflow command's `--help` carries a copy-pasteable, parse-tested example.
Purely **additive** — no command, flag, output-shape, or exit-code change (semver MINOR); existing
scripts and config files are unaffected.

### Added

- **One-line `about` on every command and subcommand** (CLI-R-05). `flowplane <group> --help`
  (e.g. `flowplane cluster --help`) now lists each subcommand with a summary instead of a blank
  line.
- **Help text on every flag and positional** (CLI-R-07). No argument — including the global
  options and bare positionals like `<NAME>` — renders a blank meaning in `--help`.
- **Runnable examples on spine workflow commands** (CLI-R-06). The 44 workflow leaves (resource
  `create`/`update`, `route generate`, `api create`, `expose`/`unexpose`/`apply`, the
  capture/discovery starters, `dataplane bootstrap`/`cert register`/`issue`/`revoke`,
  `secret create`/`rotate`, …) each
  show at least one copy-pasteable `flowplane …` example in their `--help`.
- **Conformance lints** `chk:nonempty-about`, `chk:nonempty-arg-help`, and `chk:help-examples-parse`.
  The first two walk the live `flowplane schema -o json` tree (black-box, reusing the Tier-1 S7
  coverage harness); the third walks the in-process clap tree, freezes the spine/exempt
  classification with a union-equals-live guard, and asserts every extracted example parses. Missing
  help or a non-parsing example now fails CI — help coverage is provable and regression-proof, like
  the Tier-1 machine contract.

## [2.0.0] - 2026-06-27

Major release. CLI Tier-1 conformance: the `flowplane` CLI is brought to a documented,
machine-consumable standard (output model, error/exit-code contract, config precedence,
optimistic concurrency, introspection, and destructive-action safety). These are **breaking**
changes to the CLI surface and exit-code behavior (semver MAJOR); the REST API and MCP surface
are unchanged, and the configuration file gains only an optional `timeout` field (existing
config files remain valid).

### Added

- **`flowplane schema` subcommand.** Prints the machine-readable CLI catalog (every command,
  its flags, value types, and defaults) as the typed envelope `{schemaVersion, kind:"cliSchema",
  data}`, with no network call — the canonical CLI contract and MCP-derivation seam (FP-DEC-0003).
- **`--fields a,b,c` projection.** Projects reader output to only the named keys inside the
  envelope `data` (per item for lists); `schemaVersion`/`kind` always survive, absent keys are omitted.
- **`--token` flag** (highest-priority token source) and **`FLOWPLANE_TIMEOUT`** environment
  variable (HTTP timeout; env tier of the precedence rule).
- **Destructive-action confirmation.** `delete` and `unexpose` prompt `[y/N]` on an interactive
  terminal; `--yes`/`-y` skips. On a non-interactive terminal without `--yes` they fail fast with
  exit code `2` and never block on stdin.

### Changed

- **Output format default is now context-aware.** Reader output is `table` on an interactive
  terminal and `json` when stdout is piped/redirected (so `… | jq` works without `-o json`); an
  explicit `-o/--output` always wins. `--json` is exactly `-o json`.
- **`-o json` success payloads are wrapped in a typed envelope** `{schemaVersion, kind, data}`
  (e.g. `kind: "cluster"`/`"clusterList"`/`"mutationResult"`), frozen by a snapshot suite so the
  shape cannot silently drift.
- **Structured error contract.** Errors render a single structured envelope on stderr (empty
  stdout) with `code`/`message`/`retryable`/`request_id` — JSON under `-o json`, YAML under
  `-o yaml`, and a compact prose form otherwise; transport and 429/5xx are `retryable:true`,
  4xx are `false`.
- **Exit codes are now a defined 0–7 range** mapped from the failure class (e.g. usage `2`,
  auth `3`, conflict `4`, connection-refused `7`) instead of a generic non-zero.
- **Uniform configuration precedence** `flag > env > context > file > default` for *every* value
  including the token; the token is redacted in `config show`.
- **`update`/`delete` are optimistically concurrent.** With no `--revision`, the CLI does
  read-modify-write (reads the current revision, sends it as `If-Match`); a stale `--revision`
  fails with `409` naming both the attempted and the server's current revision.
- **`--no-color`/`NO_COLOR` and `--yes` are now honored** (previously declared but inert).

## [1.1.0] - 2026-06-26

Minor release. Two features that close gaps the gateway advertised but did not yet ship.
Additive only — no REST/CLI/config/MCP removals (semver MINOR).

### Added

- **First-party global rate-limit service.** A new `flowplane-rls` crate implements an
  Envoy RateLimitService (RLS) gRPC server with fixed-window counters, so global rate
  limiting works out of the box without operators supplying an external limiter (before,
  Flowplane shipped only the Envoy RLS *client*). Includes: REST + CLI CRUD for
  rate-limit domains, policies, and per-descriptor overrides
  (`/api/v1/teams/{team}/rate-limit-domains/...`); a control-plane `rls_sync` worker that
  pushes policies to the RLS server and an `admin/rls/force-repush` endpoint; built-in
  `rate_limit_cluster` CDS injection with mTLS; and the `FLOWPLANE_RLS_GRPC_URL`
  configuration variable.
- **Publish imported OpenAPI specs (#187).** `api spec publish` now accepts *imported*
  spec versions, not just learned ones, so the tools generated by
  `api create --from-openapi` can be served over MCP through the existing publish gate.
  Import stays inert by default (no auto-publish); the CLI now prints a hint after import
  pointing at the publish step.

### Documentation

- New how-to: **import and publish an OpenAPI spec** — the full
  import → publish → serve-over-MCP flow, including the listener-binding precondition for
  `tools/call`.

## [1.0.1] - 2026-06-24

Patch release. Fixes defects found during the v1.0.0 separated CP/DP end-to-end
(GHCR-extracted binaries, real Envoy 1.37, mTLS xDS). No REST, CLI, or MCP surface
change; one additive, optional, backward-compatible configuration env var.

### Added

- `FLOWPLANE_OIDC_CA_BUNDLE` — an operator-supplied PEM bundle the control plane
  trusts **in addition to** its bundled webpki roots when fetching OIDC discovery +
  JWKS. Needed when the IdP is reachable only through a TLS-intercepting egress proxy
  (the outbound fetch otherwise fails `invalid peer certificate: UnknownIssuer`).
  Optional and default-off — unset behavior is unchanged. A set-but-unreadable,
  non-PEM, or zero-certificate bundle **fails server startup closed** (`invalid_config`)
  rather than silently weakening trust. (#171)

### Fixed

- `fp-agent` heartbeat: `/stats?format=json` from Envoy 1.37 returns histogram
  elements without the `{name, value}` shape, which broke strict deserialization so
  `last_heartbeat_at` was never populated. The parser now tolerates non-`{name,value}`
  elements. (#170)

### Documentation

- New how-to: **create a tenant org and a team**. The prod/separated onboarding docs
  jumped straight to `team create`, but a freshly bootstrapped control plane has only
  the governance-only platform org, so the step failed closed with
  `org_selector_required` (D-014). The onboarding path now documents the
  tenant-org → first-owner → team sequence and why `--org platform` is rejected. (#172)
- `docs/reference/errors.md`: the `org_selector_required` row now also names the
  platform-org-selector and no-tenant-org-yet causes.

## [1.0.0] - 2026-06-22

First stable release. PostgreSQL is the source of truth, Envoy is the only dataplane,
xDS/SDS is the config channel, and all product mutations flow through `fp-core` services.

### Added

#### Control plane & runtime
- Single `flowplane` binary running the control plane (`serve`), backed by PostgreSQL with
  schema migrations.
- Operational endpoints: `/healthz`, `/readyz` (database + xDS-consumer health), `/metrics`,
  and `/api-docs/openapi.json`.
- Structured logging and Prometheus metrics for the API, xDS, and database layers.

#### Identity, authentication & tenancy
- OIDC authentication for production (generic, any compliant IdP) and a self-contained dev
  mode with an in-process issuer and a boot-logged dev token.
- One-shot platform bootstrap that seeds the platform admin from an OIDC subject.
- Multi-tenant governance: organizations, teams, users, memberships, and grants, with
  per-team authorization scoping enforced across the API and xDS.

#### Gateway resources & REST API
- CRUD for clusters, listeners, route-configs, and dataplane bindings over a versioned REST
  API, with an OpenAPI 3.1 document generated from the code.
- Optimistic concurrency via `If-Match`/revision on mutations; a consistent error envelope
  (`code`, `message`, `hint?`, `details?`, `request_id`) with redaction of internal detail.

#### xDS, Envoy & filters
- ADS delivery of CDS/EDS/RDS/LDS to real Envoy dataplanes with make-before-break
  convergence and snapshot priming from the database across control-plane restarts.
- NACK quarantine: an offending resource is isolated and last-good config keeps serving.
- HTTP filter support proven against a live Envoy: local rate limit (with per-route
  disable), header mutation, JWT authentication, RBAC, ext_authz, and global rate-limit
  (RLS) filter wiring.

#### Secrets, SDS & dataplane lifecycle
- Write-only secret management (values never returned over the API), encrypted at rest.
- SDS-backed downstream TLS with live certificate rotation — no Envoy restart required.
- Dataplane CRUD, Envoy bootstrap generation, and certificate issue/register/revoke for
  mTLS xDS.

#### CLI & operator workflows
- `flowplane` CLI: auth (`login`, device-code, `whoami`, token), contexts
  (`config set-context`/`use-context`/`get-contexts`), resource management, `expose`/
  `unexpose` shortcuts, declarative `apply`, and `stats`/`ops xds` diagnostics.

#### Learning & discovery
- Config-first learning: live traffic capture through an injected ALS/ExtProc path, bounded
  and redacted observations, deterministic learned-spec generation, review/publish gates,
  and tool/route generation.
- Traffic-first discovery: a discovery listener that captures traffic, generates a spec and
  routes, and tears down its owned resources cleanly — with SSRF guards on upstreams.

#### AI gateway
- OpenAI-compatible AI gateway: provider/route/budget/usage resources, secret-backed
  credential injection, usage settlement and attribution, per-team budgets with enforcing
  trips, and priority-based backend failover.
- Streaming (`stream:true`) support with a correct stream-start failover boundary and SSE
  forwarding.

#### Deployment
- Secure AWS reference deployment (OpenTofu/Terraform under `deploy/aws/`): ECS/Fargate
  control plane in private subnets, RDS PostgreSQL, ALB HTTPS for the API path, NLB TCP
  passthrough for mTLS xDS, OIDC and TLS/mTLS material wired via AWS Secrets Manager, with
  an operator runbook for bootstrap, login, dataplane onboarding, and teardown.

#### Testing & certification
- Workspace test suite plus a live Envoy end-to-end suite (`scripts/e2e-envoy.sh`, split
  into `scripts/e2e/`) covering the full gateway, filter, restart-convergence, isolation,
  SDS-rotation, learning, discovery, and AI paths, with a Tier-0 credential-redaction sweep.
- Certified at Tier 0 for S1–S10 with the live suite green across 5 consecutive runs and
  zero open critical/major defects.

[1.0.0]: https://github.com/rajeevramani/flowplane/releases/tag/v1.0.0
