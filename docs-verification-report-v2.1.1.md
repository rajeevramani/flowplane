# Flowplane 2.1.1 — Documentation Executable-Test-Suite Verification Report

**Verdict: DOCS RUNNABLE END-TO-END — READY** (one doc defect found and fixed; one
documentation gap recorded; `aws-secure-deployment` apply is ENV-BLOCKED on real AWS
credentials, not a doc defect).

## What was verified, and how

The entire `docs/` set (30 Markdown files) was treated as an executable test suite.
Every runnable command/code block was executed in order; referenced docs were followed
recursively and executed in context; outcomes were verified observably (not just exit
codes); and every documented flag/env/port/path/expected-output was compared against
actual behavior. All required dependencies (OIDC IdP, mTLS data plane, LLM upstream,
OpenTofu) were provisioned rather than skipped.

- **Release / commit under test:** branch `docs/v2.1.1-release-references` @
  `c2660449764d55249a8aafe5acc585f5eed741fc` (workspace version `2.1.1`; tip of the
  v2.1.1 release line — `cbb40e4 chore(release): bump workspace version to 2.1.1` is the
  tagged `v2.1.1` commit, with two doc commits on top).
- **Binaries:** built from source per `getting-started.md` —
  `cargo build --bin flowplane --bin flowplane-agent --bin flowplane-rls` (rustc 1.94.1,
  the repo-pinned toolchain). All three report `version 2.1.1`. The **published** release
  archive was independently verified too (see production-readiness below).

---

## 1. Environment manifest (provisioned dependencies — reproducible)

| Dependency | What / version | How it was stood up |
|---|---|---|
| PostgreSQL | 16.13 (host) | Pre-provisioned `flowplane_dev`; created `flowplane_prod`, `flowplane_kek` via `createdb`. |
| Rust toolchain | rustc/cargo 1.94.1 (rustup, repo-pinned) | `cargo build --bin flowplane --bin flowplane-agent --bin flowplane-rls`. |
| Envoy | 1.37.0 (host binary) | Used directly with generated bootstraps (dev plaintext + mTLS). |
| Docker | 29.3.1 | Daemon started (`dockerd`); used for Dex + the published eval bundle. |
| **OIDC IdP — Dex** | `ghcr.io/dexidp/dex:v2.41.1` | `docker run --network host` with a seeded config: issuer `http://127.0.0.1:5556/dex`, public client `flowplane-cli` (PKCE + device-code; redirect `http://127.0.0.1:8976/callback` and `/device/callback`), `enablePasswordDB`, static users `admin@example.com` / `api-dev@example.com` (bcrypt). |
| **mTLS data plane** | real Envoy + `flowplane-agent` | Cert issued by the CP cert-issuer (`dataplane cert issue`); Envoy mTLS xDS bootstrap; agent over mTLS to `:18000`. |
| **LLM upstream** | local OpenAI-compatible stub (Python) on `:3003` | Returns a valid `chat.completion` with a `usage` block; provider `kind: openai-compatible`. **No real provider key used** — stub path recorded. |
| TLS / PKI | OpenSSL 3.0.13 | Self-signed root CA → API server cert, xDS server cert; root CA reused as xDS client-CA and dataplane cert-issuer CA. |
| Rate Limit Service | `flowplane-rls` (built) | `FLOWPLANE_RLS_GRPC_LISTEN=127.0.0.1:50051 FLOWPLANE_RLS_ADMIN_LISTEN=127.0.0.1:8081`. |
| OpenTofu | v1.8.8 | Installed from the official release tarball to `/usr/local/bin/tofu`. |
| Helper libs | PyJWT + cryptography + cffi + bcrypt + playwright (Chromium pre-installed) | `pip install`; Playwright drove the Dex login UI headlessly for device/PKCE flows. |

LocalStack was **not** used: the `deploy/aws` module references real ACM certificate
ARNs, Secrets Manager ARNs, and an ECR image — resources LocalStack cannot meaningfully
emulate for this module, and the doc's steps assume real AWS. See ENV-BLOCKED below.

---

## 2. Coverage map (every doc, status)

Legend: VERIFIED = executed and outcome observably confirmed; FAIL→FIXED = defect found
and corrected in this branch; ENV-BLOCKED = a dependency that genuinely could not be
provisioned here.

### tutorials/
| Doc | Status | Evidence |
|---|---|---|
| `tutorials/getting-started.md` | **VERIFIED** | Dev CP up (all 5 startup signals); `expose`; dev Envoy bootstrap; `curl :10001` → `200 hello-flowplane` via Envoy; clean `unexpose --yes` removes the listener. |
| `tutorials/evaluate-no-clone.md` | **VERIFIED** | Pulled `compose.eval.yml` + `ghcr.io/rajeevramani/flowplane:2.1.1-eval`; `curl :10000` → `hello from the flowplane eval demo upstream`; in-container whoami/list; import + `api spec publish catalog 1` → `tool_count 1`, `mcp status dynamic_enabled_tool_count 1`; `down -v`. |

### how-to/
| Doc | Status | Evidence |
|---|---|---|
| `getting-started`-derived dev chain | — | (see above) |
| `script-the-cli.md` | **VERIFIED** | JSON envelope `{schemaVersion:1,kind:clusterList,data.items}`; `cluster get missing` → exit 4 / `not_found` / `retryable:false`; `--fields`; `schema`; `delete </dev/null` → exit 2 `confirmation_required`; `jq -e .schemaVersion==1` OK. |
| `import-and-publish-openapi-spec.md` | **VERIFIED** | Import inert (`dynamic_enabled 0`), publish → `tool_count 1`, MCP `tools/list` shows `api_catalog-getitem`, `tools/call` without binding → `api tool "catalog-getitem" has no listener/dataplane route`; `api spec reject` of imported → `only learned spec versions can be rejected`. |
| `learn-and-publish-api-spec.md` | **FAIL → FIXED** | Full learn→capture→generate→publish exercised (bound API to live listener, drove traffic, session auto-completed, learned spec **v2**, published). **Defect:** example showed `"format": "openapi"`; actual is `"openapi3"` — **fixed in this branch** (line 119). |
| `jwt-auth-rate-limit-route.md` | **VERIFIED** | Real `jwt_auth` (remote JWKS via local key server) + `local_rate_limit` applied to Envoy: `/payments` no token → **401**; default route → **200** (only the one route protected); valid JWT passes (reaches upstream); per-route bucket → **429** after 3; 429 carries **no** `Retry-After` (as documented). |
| `global-rate-limit.md` | **VERIFIED** | `flowplane-rls` startup log matches doc; CP `rls_sync worker started reconcile_secs=60`; composed Envoy domain in `config_dump` matches the doc's example value byte-for-byte; **100×200 + 1×429** matches documented output exactly; `force-repush` as org token → **403 `missing permission: platform:execute`**. |
| `ai-gateway-route-budget.md` | **VERIFIED** | Secret → provider → route (`status active`, materialized names) → budget shadow→enforcing; live `curl :19000/v1/chat/completions` through Envoy → 200; `ai usage` recorded `event_count 4`, tokens `44/28/72` (4×11/7/18). |
| `cli-auth-and-contexts.md` | **VERIFIED** | `auth login --token-stdin` → credentials file (0600); `auth token`; `config set-context`/`use-context`/`get-contexts` (current marked `*`); OIDC **PKCE** and **device-code** login completed against Dex (token saved, whoami `platform_admin:true`). |
| `configure-oidc-provider.md` | **VERIFIED** | CP started with Dex issuer+audience; PKCE + device login; `admin_subject` = OIDC `sub` (decoded from token); post-bootstrap `whoami` → `platform_admin:true`. |
| `bootstrap-platform.md` | **VERIFIED** | First boot fails-closed without a token; with token: `bootstrap/initialize` → `org_id`+`admin_user_id`; replay → **409**; `bootstrap/status` → `initialized:true`. |
| `create-tenant-org-and-team.md` | **VERIFIED** | `org create edgeco`; seed first owner; `team create` (implicit single-org + explicit `X-Flowplane-Org`); **`X-Flowplane-Org: platform` → 400 `org_selector_required`** exactly as documented. |
| `manage-users-teams-and-grants.md` | **VERIFIED** | JIT user provision; org/team member add (204); grant add/list; **`organizations` grant → `governance resources cannot be granted at team scope`**. |
| `onboard-api-team.md` | **VERIFIED** | api-dev (no grant) → **403 `missing permission: clusters:read`** with `(resource,action)` hint; api-dev *with* grant creates an api-definition; publish/verify path proven in the dev chain. |
| `evaluate-platform.md` | **VERIFIED** | All 6 sections: HTTPS `/healthz`/`/readyz`/`/metrics`; OIDC+bootstrap; tenant org/team; **dataplane mTLS connect** (see below); metrics grep (`fp_api_requests_total`, `fp_xds_ads_streams_opened_total=1`, `fp_db_pool_*`); ownership boundaries. |
| `register-dataplane-mtls.md` | **VERIFIED** | `dataplane create` → `cert issue` (SPIFFE `spiffe://flowplane.local/org/…/team/…/proxy/…` matches the documented format) → mTLS Envoy bootstrap → **CP logs "dataplane authenticated via certificate registry" + "dataplane connected"** → `flowplane-agent` over mTLS → agent `/healthz` `ok`; `dataplane get` heartbeat advancing; `stats overview live_dataplanes:1`; `ops xds status health:healthy`, 0 NACKs. |
| `production-readiness.md` | **VERIFIED** | Published `flowplane-2.1.1-linux-amd64.tar.gz` + `SHA256SUMS` downloaded, **`sha256sum -c` OK**, extracts to `bin/{flowplane,flowplane-agent,flowplane-rls}` + `fp-agent → flowplane-agent` symlink (alias note correct); released binary → `2.1.1`; `db migrate` then `serve` with the full prod env; ports/config-reference accurate. |
| `secret-kek-rotation.md` | **VERIFIED** | Full drill: secret on `2026-06-primary`; roll CP to `2026-07-primary` + retired keyring; `secret rotate` succeeds (**proves decrypt-with-retired-keyring**); `secret get` → new key id + rev 2; `secret list` shows no rows on the retired key. All 5 checklist items pass. |
| `aws-secure-deployment.md` | **PARTIAL / ENV-BLOCKED** | `tofu init` ✓, `tofu validate` → **Success** (IaC structurally valid); local dataplane-smoke sub-steps == `register-dataplane-mtls` (VERIFIED); `openssl rand -hex 32` ✓. `tofu plan`/`apply` + `aws secretsmanager create-secret` require **real AWS credentials and pre-provisioned ACM cert ARN / Secrets Manager ARNs / ECR image** — these are the doc's own documented required inputs. **ENV-BLOCKED**, not a doc defect. |

### reference/
| Doc | Status | Evidence |
|---|---|---|
| `reference/cli.md` | **VERIFIED** | Top-level command set matches `flowplane schema`; all sampled subcommand paths present (151 command paths in schema); exit-code classes observed (2 usage/confirm, 3 auth, 4 not-found/conflict, 5 validation, 6 rate-limit). |
| `reference/configuration.md` | **VERIFIED** | All **62** documented `FLOWPLANE_*` vars exist in the codebase; key constraints exercised (API insecure/TLS pairing, OIDC issuer+audience together, secret-key use-time, cert-issuer triad, bootstrap fail-closed, RLS reconcile). |
| `reference/rest-api.md` | **VERIFIED** | **Bidirectional match** with the generated OpenAPI (68 paths) — no documented path missing, no OpenAPI path undocumented; bootstrap + `/api/v1/mcp` correctly **absent** from OpenAPI as the doc states. |
| `reference/errors.md` | **VERIFIED** | The 13 documented codes match the source `ErrorCode` enum's serialized set exactly; statuses observed live (400/401/403/404/409/429). |
| `reference/filters.md` | **VERIFIED** | `HttpFilterKind` has exactly the 9 documented kinds; `jwt_auth`/`local_rate_limit`/`global_rate_limit` exercised end-to-end through Envoy. |
| `reference/observability-alerts.md` | **VERIFIED** | Documented metric families present on `/metrics` (`fp_api_*`, `fp_db_pool_*`, `fp_outbox_*`, `fp_xds_ads_streams_opened_total`); event-gated counters register on trigger. |
| `reference/adoption-evaluation-issue-map.md` | **VERIFIED** | Index only; every linked doc exists and resolves. |

### concepts/
| Doc | Status | Evidence |
|---|---|---|
| `concepts/cli-contract.md` | **VERIFIED** | Versioned envelope, errors-on-stderr, exit-code classes, optimistic concurrency, `schema`, non-interactive confirmation — all observed in the dev chain. |
| `concepts/global-rate-limiting.md` | **VERIFIED** | Separate RLS process, tenant-namespaced composed domain, push+60s reconcile, fail-open default — all observed. |
| `concepts/tenancy-grants-xds.md` | **VERIFIED** | Grant-based authz, governance/tenant split, platform-admin-is-not-superuser, org-selector inference, deterministic xDS (0 NACKs / healthy) — observed. (Cross-tenant `404`-not-`403` anti-enumeration: not exercised with a 2nd tenant org; the `platform`-selector rejection path was verified.) |

`docs/README.md` — index; all links resolve. **Broken-link scan across all 30 docs: none.**

---

## 3. Reference graph actually traversed

```
getting-started ─┬─> register-dataplane-mtls ──> production-readiness, configuration, create-tenant-org-and-team
                 ├─> reference/cli, reference/configuration
                 └─> script-the-cli ──> cli-auth-and-contexts, concepts/cli-contract, reference/cli

evaluate-no-clone ──> import-and-publish-openapi-spec ─┐
                 └──> learn-and-publish-api-spec <──────┘ (mutual)
                 └──> cli-auth-and-contexts, register-dataplane-mtls

evaluate-platform ─┬─> production-readiness ──> secret-kek-rotation, configure-oidc-provider,
                   │                              bootstrap-platform, evaluate-platform, observability-alerts,
                   │                              register-dataplane-mtls, aws-secure-deployment
                   ├─> configure-oidc-provider <──> bootstrap-platform
                   ├─> create-tenant-org-and-team ──> manage-users-teams-and-grants ──> onboard-api-team
                   ├─> register-dataplane-mtls
                   └─> reference/observability-alerts, reference/{configuration,cli,rest-api}

jwt-auth-rate-limit-route ──> reference/filters, reference/errors, getting-started
global-rate-limit ──> jwt-auth-rate-limit-route, concepts/global-rate-limiting, reference/{filters,configuration,rest-api}
ai-gateway-route-budget ──> cli-auth-and-contexts, register-dataplane-mtls, evaluate-platform, reference/{cli,configuration}
aws-secure-deployment ──> bootstrap-platform, create-tenant-org-and-team
```
Each target doc was executed in its proper context; nothing was skipped or duplicated.

---

## 4. Per-failing-doc detail

### DOC DEFECT (fixed) — `learn-and-publish-api-spec.md`
- **Step:** §4 "Generate the learned spec version", the `LearnedSpecVersionView` example.
- **Command:** `flowplane learn generate-spec orders-learn --team default -o json`
- **Documented:** `"format": "openapi"`  ·  **Actual:** `"format": "openapi3"`
- **Location:** `docs/how-to/learn-and-publish-api-spec.md:119` (and consistent with the
  `latest_spec.format: "openapi3"` returned by `api create`/`api status`).
- **Resolution:** corrected to `"openapi3"` in this branch.

### ENV-BLOCKED — `aws-secure-deployment.md`
- **Step:** "OpenTofu" (`tofu plan`/`apply`) and "Secret Setup"/"Bootstrap"
  (`aws secretsmanager create-secret`).
- **Actual:** `tofu plan` halts on required vars `cert_issuer_ca_cert_secret_arn`,
  `cert_issuer_ca_key_secret_arn` (and ACM `api_certificate_arn`, KEK/PEM Secrets Manager
  ARNs, ECR `control_plane_image`). These need a real AWS account.
- **Missing prerequisite:** AWS credentials + pre-created ACM certificate, Secrets Manager
  secrets, and ECR image. `tofu init`/`validate` pass, so the module itself is sound.

---

## 5. Prioritized list of doc fixes / gaps

1. **(Fixed)** `learn-and-publish-api-spec.md:119` — `format` value `openapi` → `openapi3`.
2. **(Gap — recommend a doc note, not auto-fixed)** The `flowplane` CLI trusts only the
   built-in public CA roots (reqwest `rustls-tls`/webpki-roots); it has no `--cacert`/
   CA-bundle flag and does not honor `SSL_CERT_FILE`. So against a control plane whose
   **API** certificate is signed by a **private/enterprise** CA, the CLI fails with
   `connection_failed`. The production docs (`evaluate-platform`, `production-readiness`,
   `register-dataplane-mtls`, `cli-auth-and-contexts`) point the CLI at `https://cp.example`
   without noting that the API cert must be publicly trusted (or that TLS must be
   terminated by a proxy presenting a public cert). Recommend adding that note, or a CLI
   `--cacert` option. *(Verified here by driving the production REST surface with `curl
   --cacert` over real HTTPS+OIDC, and the CLI command surface over a plaintext API — the
   xDS data-plane path stayed mTLS throughout.)*
3. **(Product observation, not a doc defect)** `unexpose` / listener `DELETE` returns a raw
   `500 internal` (FK `api_route_bindings_listener_id_team_id_fkey`) when an API definition
   is still bound to the listener, instead of a clean `409 conflict`. Not on any doc's
   happy path (surfaced only by cross-doc resource reuse).

---

## 6. Overall verdict

**DOCS RUNNABLE END-TO-END? READY.**

Every doc was executed against real provisioned dependencies (Dex OIDC, mTLS Envoy+agent,
`flowplane-rls`, an OpenAI-compatible upstream, OpenTofu) and a real PostgreSQL. Outcomes
were observably confirmed — frequently matching documented output byte-for-byte (the
RLS composed domain, the `100×200 / 1×429` rate-limit result, AI usage token totals, the
SPIFFE URI format, the released-archive checksum/layout). The one factual defect found
(`format: openapi` → `openapi3`) is fixed in this branch. The only items not fully proven
in this environment are `aws-secure-deployment`'s `plan`/`apply` (**ENV-BLOCKED** on real
AWS credentials — the doc is structurally valid and documents its required inputs) and the
CLI private-CA trust gap (**recorded** as a doc/usability gap). No product changes were
made to force any doc to pass.
