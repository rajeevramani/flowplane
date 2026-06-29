# Flowplane — Capabilities Artefact

**Flowplane** is an API gateway built for humans and AI agents: a multi-tenant Rust/PostgreSQL
**control plane** that governs APIs (OIDC auth, grant-based RBAC, audit) and drives a deterministic
**Envoy** data plane over xDS/SDS. PostgreSQL is the source of truth; Envoy is the only data plane;
every product mutation flows through `fp-core` services.

> **Provenance.** This artefact is grounded in a runtime verification of the **v2.1.0** published
> artefacts (`ghcr.io/rajeevramani/flowplane:2.1.0` + `:2.1.0-eval`) plus the `docs/` tree at branch
> `feature/fpv2-0ym-adoption-evaluation-spine` (commit `ac56fe1`). Each capability is tagged:
> **✅ runtime-verified** (observed executing during this UAT) · **📄 documented** (in the shipped
> reference, not separately exercised here).

---

## 1. Capability map

```
                        ┌─────────────────────────────────────────────┐
   Identity (OIDC IdP)  │              CONTROL PLANE (fp-core)         │
   ─────────────────►   │  Auth · Multi-tenancy · Governance/RBAC     │
                        │  Gateway resources · API lifecycle · AI gw  │
                        │  Rate-limit policy · Secrets · Learning     │
                        └───────────────┬───────────────┬─────────────┘
                          xDS/SDS (mTLS) │               │ REST + MCP (Bearer/OIDC)
                                         ▼               ▼
                                ┌──────────────┐   ┌──────────────┐
                                │ Envoy data   │   │ CLI / agents │
                                │ plane (xDS)  │   │ / MCP tools  │
                                └──────────────┘   └──────────────┘
                          PostgreSQL = source of truth
```

| Domain | Capabilities |
|---|---|
| **Identity & auth** | Provider-neutral OIDC (issuer/audience/JWKS/CA-bundle), PKCE + device-code CLI login, fail-closed when unconfigured |
| **Multi-tenancy & governance** | Orgs, teams, members/roles, grant-based RBAC, platform-admin governance separation, per-tenant write budgets |
| **Gateway resources** | Clusters, listeners, route-configs as first-class team-scoped resources; `expose` one-shot chain |
| **L7 traffic policy** | Closed 9-filter chain (CORS, JWT, RBAC, rate-limit, header-mutation, ext_authz, compressor, health-check) |
| **API lifecycle** | OpenAPI import → inert artefacts → publish gate → generated tools; learning/discovery from live traffic |
| **AI gateway** | LLM provider fronting, routes, token budgets, usage accounting |
| **MCP** | Generated API operations exposed as MCP tools; static + dynamic tool surface |
| **Rate limiting** | Local (per-Envoy) + global (RLS) rate limiting, per-team overrides |
| **Data plane identity** | Dataplane registration, mTLS cert issuance (SPIFFE), xDS-mTLS, agent health/telemetry |
| **Secrets** | Write-only secrets with KEK encryption + key rotation/keyring |
| **Bootstrap & ops** | One-shot fail-closed bootstrap, health/readiness, xDS status, stats, declarative `apply` |
| **Interfaces** | REST API, CLI, MCP, machine-readable CLI/OpenAPI schema export |

---

## 2. Identity & authentication

- **Provider-neutral OIDC.** Accepts tokens from any OIDC provider publishing discovery + JWKS;
  configured via `FLOWPLANE_OIDC_ISSUER` + `FLOWPLANE_OIDC_AUDIENCE` (set together), optional
  `FLOWPLANE_OIDC_JWKS_URI` and `FLOWPLANE_OIDC_CA_BUNDLE`. **📄/✅**
- **Fail-closed auth posture** (all ✅ runtime-verified against `:2.1.0`):
  - No OIDC + dev-mode off → authenticated endpoints return **503** (`authentication is not configured on this server`); public `/healthz`/`/readyz` stay 200.
  - Only one of issuer/audience set → **startup fails**: `invalid_config: …ISSUER and …AUDIENCE must be set together`.
  - Invalid/empty OIDC CA bundle → **startup fails closed**: `…contains no usable certificates`.
- **CLI login.** `flowplane auth login --pkce` (browser) or `--device-code` (headless); default
  callback `http://127.0.0.1:8976/callback`; scopes default `openid email profile`. **📄**
- **Token precedence** (✅): `--token` flag → `FLOWPLANE_TOKEN` env → context → config file → credentials file.

## 3. Multi-tenancy & governance (RBAC)

- **Orgs / teams / members.** `org create`, `org member add|list|remove`, `team create|list`,
  `team member add|list|remove`, with roles (`owner`/`member`). **✅** (created `edgeco` org + `payments` team end-to-end)
- **Grant-based RBAC.** `team grant add|list|remove` scoped by `--resource`/`--action`
  (e.g. `api-definitions:create`, `mcp-tools:read`, `clusters:create`). Grants are membership-gated —
  granting to a non-member fails closed (`add the user to this organization before granting`). **✅**
- **Governance separation.** Bootstrap creates a **platform org** that is governance-only — it cannot
  host tenant teams or dataplanes; platform-admin is not a tenant bypass. **📄**
- **Per-tenant write budget.** `FLOWPLANE_TENANT_WRITE_LIMIT_PER_MIN` (default 120). **📄**

## 4. Gateway resources & L7 traffic policy

- **First-class resources** (team-scoped): **clusters**, **listeners**, **route-configs**, created via
  CLI `create --file` / REST, validated with `deny_unknown_fields`. **✅** (created 8 documented bodies)
  - Clusters: endpoints, weighted endpoints, LB policies (round-robin, least-request, ring-hash, maglev), upstream TLS/SNI, HTTP/2, health checks, circuit breakers, outlier detection.
  - Route-configs: virtual hosts, domain/header/query matchers, prefix/exact/template/regex match, weighted clusters, prefix-rewrite, timeouts, retry policy.
  - Listeners: bind address/port (**floor 1024**, ✅ enforced), public base URL, HTTP/HTTPS, SDS-based TLS, access logs.
- **`expose` / `unexpose`.** One command creates the cluster + route-config + listener chain for an upstream. **📄**
- **Closed L7 filter chain** (9 kinds, each at most once per listener): `cors`, `local_rate_limit`,
  `header_mutation`, `health_check`, `compressor`, `jwt_auth`, `ext_authz`, `rbac`, `global_rate_limit`. **📄**

## 5. API lifecycle, MCP & AI gateway

- **OpenAPI import → publish gate.** `api create <name> --from-openapi <file>` imports a spec and
  generates tool rows that are **inert until published**; `api spec publish <name> <ver>` flips them
  live. `api status` / `mcp status` show published state + tool counts. **✅** (catalog spec: imported
  `tool_count:1` inert → published → `dynamic_enabled_tool_count:1`)
- **Learning & discovery.** `learn` capture sessions + discovery sessions infer OpenAPI from live
  traffic, feeding the same publish gate. **📄**
- **MCP server.** Generated API operations surface as MCP tools (streamable-HTTP transport, protocol
  versions `2025-11-25`/`2025-03-26`); static tool surface (35) + dynamic per-team published tools. **✅**
- **AI gateway.** `ai` providers/routes/budgets/usage front LLM providers with per-route token
  **budgets** and weighted/priority backends + model overrides. **📄**

## 6. Data plane identity (mTLS) & xDS

- **Dataplane registration.** `dataplane create <name> --team <t>` registers a dataplane (UUID). **✅**
- **mTLS cert issuance.** `dataplane cert issue <name> --team <t> --ttl-hours N` mints a leaf cert
  from the configured issuer CA and returns `certificate_pem` + `private_key_pem` + `ca_certificate_pem`
  **once** (key never stored), bound to a **SPIFFE URI**
  `spiffe://<trust-domain>/org/<org>/team/<team>/proxy/<dataplane>`. `dataplane cert register` handles
  externally-issued certs. **✅**
- **xDS is mTLS-mandatory.** The xDS listener refuses to enable without the
  `FLOWPLANE_XDS_TLS_CERT`/`_KEY`/`_CLIENT_CA` triad. **✅** (observed `xDS listener disabled: mTLS is mandatory`)
- **Agent health/telemetry.** Dataplane agent reports Envoy stats; `ops xds status` and `stats overview`
  expose live/stale dataplanes, NACK counts, config-verify state. **✅**

## 7. Secrets, rate limiting, observability

- **Secrets.** Write-only secret store, AES KEK encryption (`FLOWPLANE_SECRET_ENCRYPTION_KEY`, 32 bytes),
  key-id + retired-key **keyring** for rotation (`secret-kek-rotation`). **📄**
- **Global rate limiting.** External **Rate Limit Service** (`flowplane-rls`); CP injects a
  `rate_limit_cluster` and reconciles policy on a ≤60 s loop; Envoy→RLS hop can be mTLS. Local
  per-Envoy `local_rate_limit` filter for in-proxy limiting. **📄**
- **Observability.** `/healthz` + `/readyz` (✅ 200), `/metrics`, OTLP trace export
  (`FLOWPLANE_OTLP_ENDPOINT`), `stats overview` (✅), `ops xds status` (✅), and a published alert baseline.

## 8. Bootstrap & operations

- **One-shot fail-closed bootstrap.** A fresh non-dev CP starts **uninitialized** and **refuses to
  start without an operator-supplied bootstrap token** (`FLOWPLANE_BOOTSTRAP_TOKEN[_FILE]`); the token
  is hashed, never logged. `POST /api/v1/bootstrap/initialize` with `admin_subject` (the OIDC `sub`)
  designates the first platform admin; replay returns 409. **✅** (no-token boot refused; initialize→200; replay→409; `whoami`→`platform_admin:true`)
- **Declarative apply.** `flowplane apply <manifest.json>` applies a declarative resource set. **📄**
- **Schema export.** `flowplane schema` (machine-readable CLI contract) and `flowplane openapi`
  (exact REST contract this binary serves). **📄**

## 9. Interfaces & surface (reference)

**CLI commands (25):** `serve · db · openapi · auth · config · org · team · cluster · listener · route ·
api · mcp · ai · rate-limit · learn · secret · dataplane · expose · unexpose · stats · ops · apply ·
completion · version · schema`

**REST endpoint groups (21):** Auth · Bootstrap · Organizations · Teams (members & grants) · Agents ·
Clusters · Listeners · Route-configs · Rate limiting · API definitions (+specs) · MCP · Learning ·
Discovery · Expose · Route-generation plans · AI · Dataplanes (+proxy-certs, telemetry, envoy-config) ·
Stats · Secrets · xDS status & ops · Operational (root/public). All app endpoints under `/api/v1`;
operational endpoints at root. Bearer/OIDC auth, `X-Flowplane-Org` active-org selector, `If-Match`
optimistic concurrency, paginated envelopes, `x-request-id`/`traceparent` correlation.

## 10. Deployment shapes

| Shape | Image | Identity | Use |
|---|---|---|---|
| **Evaluation** | `:2.1.0-eval` | in-process OIDC + seeded `dev-org`/`default` + on-disk dev token; all ports `127.0.0.1` | no-clone eval via `compose.eval.yml`; **never** production |
| **Production** | `:2.1.0` | real OIDC, API TLS, xDS-mTLS, bootstrap token, KEK | `--no-default-features` (refuses dev mode); CP+DP runbook in `docs/how-to/evaluate-platform.md` |
| **AWS reference** | `:2.1.0` | OpenTofu module (`deploy/aws/`): ECS/NLB/ALB/RDS/Secrets-Manager | strict-secure smoke environment (`tofu validate` ✅) |

---

*Security invariants observed during verification:* fail-closed bootstrap, fail-closed OIDC (503 /
startup refusal / CA-bundle), xDS mTLS mandatory, generated/learned tools inert until publish,
private keys returned once and never stored, platform-admin governance-only. These are the
constitution's fail-closed / tenant-isolation / dataplane / inert-artefact invariants in practice.
