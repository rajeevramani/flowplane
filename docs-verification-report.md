# Docs Verification Report

Run date: 2026-06-21 (fourth pass — latest `main` @ `5adaa90`, PostgreSQL **and** Envoy available)

Config:

- Epic issue: #100 (`Document flowplane-v2 (user-facing docs epic)`)
- Label: `docs-verification`
- Marker: `[docs-verify]`
- Method: execute the docs against the real binary/CLI/control plane/**Envoy** from a
  clean state; classify every mismatch as `doc-defect` / `code-defect` / `ambiguous`.
- GitHub tooling: this environment exposes the GitHub MCP server (`mcp__github__*`),
  not the `gh` CLI. Issue reads/writes below were performed with those tools.
- Branch: developed/verified on `claude/gifted-gates-2num35` (pulled latest `main`).

## What changed since the previous pass

The previous pass (report v3) filed/repro'd #121, #122, #123 and carried #118/#119/#120.
**Every one of those is now CLOSED**, and the docs/code were reorganised since:

- `docs/dev-dataplane.md` was reclassified to **`internal/dev-dataplane.md`** (#116). The
  README now points the dev path at `internal/dev-dataplane.md`. The task's
  "`docs/dev-dataplane.md`" is read as that file.
- `docs/how-to/bootstrap-platform.md` is **new** (operator-supplied bootstrap token,
  #113/#133) and is now part of the executable how-to set.
- `FLOWPLANE_UPSTREAM_CA_BUNDLE` was added to `configuration.md` (#132 fix, commit
  `5adaa90`), and the AI-cluster TLS materialisation switched to **verify-by-default**
  (#125), which is what closes the old #123 NACK.

This pass **re-proves every prior defect is actually fixed** against a live data path,
and surfaces **2 new, previously-unreported discrepancies** (both `minor`).

## Preconditions

- Build/toolchain: pass. `cargo build --bin flowplane` finished exit 0;
  `./target/debug/flowplane version` → `0.1.0`.
- PostgreSQL: pass. `scripts/ensure-postgres.sh` → `postgres ready` (exit 0); the
  session hook also reports `postgres ready`.
- Envoy data path: **pass** — Envoy 1.37.0 on `PATH`
  (`3909deb175ef358202d6ab4f94d683ffc0fdb477/1.37.0/Clean/RELEASE/BoringSSL`). A live
  request traversed Envoy to the upstream, and a real mTLS leaf certificate was issued
  from a configured CA.
- Cert issuance: executed by satisfying the documented prereq — a throwaway CA was
  generated with `openssl` and supplied via `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH` /
  `_KEY_PATH`, exactly as `register-dataplane-mtls.md` requires.
- Still blocked **by the environment** (not by the docs): the live `fp-agent` run and
  the live AI chat round-trip. This sandbox **SIGTERMs any long-lived Envoy/server
  child after ~12s** (observed: `caught ENVOY_SIGTERM`), and there is no external egress
  / no IdP / no OpenAI. The control-plane sides of both flows are fully executed; only
  the final upstream round-trips are not.

## Documentation Set (confirmed against epic #100)

Actionable / executed:

- `README.md`
- `internal/dev-dataplane.md` (was `docs/dev-dataplane.md`; relocated by #116)
- `docs/tutorials/getting-started.md`
- `docs/how-to/cli-auth-and-contexts.md`
- `docs/how-to/jwt-auth-rate-limit-route.md`
- `docs/how-to/register-dataplane-mtls.md`
- `docs/how-to/ai-gateway-route-budget.md`
- `docs/how-to/learn-and-publish-api-spec.md`
- `docs/how-to/bootstrap-platform.md`
- `docs/reference/cli.md`
- `docs/reference/configuration.md`
- `docs/reference/errors.md`
- `docs/reference/filters.md`
- `docs/reference/rest-api.md`

- `docs/how-to/secret-kek-rotation.md` (executed end to end)

Operator runbooks (epic #100 **non-goals**; "new docs link to these") — inspection /
command-shape verification only, because they require cloud infra / a full production
deploy that is not available here: `docs/how-to/aws-secure-deployment.md`,
`docs/how-to/production-readiness.md`.

Not execution-verified: `docs/concepts/tenancy-grants-xds.md` (#112, item 12) —
explanation prose, no runnable commands.

---

## Proof: `README.md`

| Command | Result | Excerpt |
|---|---|---|
| `cargo build --bin flowplane` | pass (exit 0) | binary at `target/debug/flowplane` |
| `./target/debug/flowplane version` | pass (exit 0) | `0.1.0` |
| `./target/debug/flowplane openapi` | pass (exit 0) | 289430 bytes; `"openapi":"3.1.0"`, `"title":"Flowplane"`, 62 paths |
| `scripts/ensure-postgres.sh` | pass (exit 0) | `postgres ready` |

The README's PostgreSQL caveat (the helper "does **not** create that role"; macOS guidance)
is present and accurate — this is the **#119 fix**, and the helper works in this Linux
environment. README now points the dev path at `internal/dev-dataplane.md` (relocated by #116).

## Proof: `docs/tutorials/getting-started.md` (full happy path, **incl. live Envoy**)

Started the CP with the tutorial's **exact** §2 env set (notably **without**
`FLOWPLANE_SECRET_ENCRYPTION_KEY`).

| Step / Command | Result | Excerpt |
|---|---|---|
| §2 `flowplane serve` (dev mode) | pass | all documented startup signals present (below) |
| §3 `auth whoami` | pass (exit 0) | `ORG ROLE owner`, `MEMBERSHIPS 1 items`, `GRANT COUNT 0`, `ORG SELECTOR REQUIRED false` |
| §4 `expose http://127.0.0.1:3001 …` | pass (exit 0) | `CURL URL http://127.0.0.1:10001/`, `CLUSTER local-upstream`, `ROUTE CONFIG local-routes`, `LISTENER local`, `ENDPOINT SOURCE listener.public_base_url` |
| §4 `cluster/listener/route list` | pass | resources present at revision 1 |
| §5 `dataplane create dp-local` | pass (exit 0) | `created "dp-local" (revision 1)` |
| §6 `--out … dataplane bootstrap dp-local --mode dev …` | pass (exit 0) | valid Envoy YAML; `node.id` stamped, ads → `xds_cluster`/127.0.0.1:18000 |
| §6 `envoy -c /tmp/flowplane-envoy.yaml` | **pass** | `cds: added/updated 1 cluster(s)`, `lds: add/update listener 'local'` |
| §7 `curl -i http://127.0.0.1:10001/` | **pass** | `HTTP/1.1 200 OK` / `server: envoy` / body `hello-flowplane` |
| §7 `unexpose local` | pass (exit 0) | removes cluster/route/listener (confirmed absent afterwards) |

Documented startup log signals — all observed verbatim:

```text
database connected and migrations applied
DEV MODE: in-process identity, seeded resources — never production
dev resources seeded            org=dev-org team=default user=dev-user
dev bearer token (valid 1h, this boot only)   dev_token=eyJ0eXAi…
xDS ADS server starting (plaintext dev mode)  addr=0.0.0.0:18000
API listener starting           addr=127.0.0.1:8096 tls=false
```

Live gateway proof:

```text
$ curl -i http://127.0.0.1:10001/
HTTP/1.1 200 OK
server: envoy
content-length: 16
x-envoy-upstream-service-time: 0

hello-flowplane
```

No discrepancies.

## Proof: `internal/dev-dataplane.md`

Same loop with the runbook's env set (which **does** set `FLOWPLANE_SECRET_ENCRYPTION_KEY`).
Steps 1–9 pass, including the live curl. Step 8's mTLS-bootstrap variant emits valid
TLS-bearing YAML (exit 0). Step 10 diagnostics (Envoy attached, no `fp-agent`):

| Command | Result | Excerpt |
|---|---|---|
| `stats overview` | pass | `LIVE 0 / STALE 1 / TOTAL DATAPLANES 1` |
| `ops xds status` | pass | `HEALTH stale … RECENT NACK COUNT 0` |
| `ops xds nacks` | pass | `no rows` |

`STALE` / zero counters are exactly what the runbook predicts for the manual path (Envoy
started directly, no heartbeats). No discrepancies.

## Proof: `docs/how-to/cli-auth-and-contexts.md`  (executed end to end)

Ran against a clean `$HOME` so config/credentials were created from scratch.

| Step / Command | Result | Excerpt |
|---|---|---|
| `config path` | pass | `/…/.flowplane/config.toml` (documented default) |
| `config set-context prod --server … --org … --team …` | pass | `context saved`; becomes current when none set |
| `config get-contexts` | pass | current marked `*`: `* prod  http://127.0.0.1:8096  dev-org  default` |
| `config set-context staging …` + `config use-context prod` | pass | `context selected`; `*` moves to `prod` |
| `auth login --token <tok>` | pass | `token saved to /…/.flowplane/credentials` (file mode `0600`) |
| `auth token` | pass | prints the resolved bearer token |
| `--context prod auth whoami` (no `FLOWPLANE_TOKEN` env) | pass | identity resolved from the credentials file → confirms the documented token precedence |
| `config show` | pass | renders `current_context` + both `[[contexts]]` blocks |
| `auth logout` | pass | `logged out`; credentials file deleted (confirmed absent) |

The token+server fast path (`FLOWPLANE_TOKEN`/`FLOWPLANE_SERVER` → `whoami`) was already
exercised under getting-started §3. OIDC PKCE/device login (`--pkce`/`--device`) is **blocked
by environment** (no IdP); its flag surface matches `cli.md`. No discrepancies.

## Proof: `docs/how-to/jwt-auth-rate-limit-route.md`  (#121 **fixed**)

Built the cluster, route-config and listener from the doc's **exact** JSON.

| Command | Result | Excerpt |
|---|---|---|
| create `payments-backend` cluster (prereq) | pass | `created "payments-backend" (revision 1)` |
| create `api-routes` route-config with §2 body **verbatim** (`"match": { "prefix": { "prefix": "/payments" } }`) | **pass (exit 0)** | `created "api-routes" (revision 1)` |
| create `edge` listener with §1 `http_filters` (jwt_auth providers+requirement_map, local_rate_limit) | pass | `created "edge" (revision 1)` |
| §3 `listener update edge --revision 1 --file <spec-only>` | pass | `updated "edge" (revision 2)` |
| §3 `route update api-routes --revision 1 --file <spec-only>` | pass | `updated "api-routes" (revision 2)` |
| §3 re-run with stale `--revision 1` | pass-as-documented | `error (revision_mismatch): route config "api-routes" is at revision 2, you supplied 1` (→ 409) |

**#121 confirmed fixed:** the doc now prints the nested `PathMatch` form (line 83) and the
control plane accepts it verbatim. §4 (live `401`/`429` through Envoy with a real JWKS issuer)
is **blocked by environment** (no IdP); the filter chain itself is accepted and validated.

## Proof: `docs/how-to/register-dataplane-mtls.md`

| Command | Result | Excerpt |
|---|---|---|
| §1 `dataplane create dp-local` | pass | exercised under getting-started |
| §2 `dataplane cert issue dp-local` — CP started **without** the issuer CA | pass-as-documented | `error (invalid_config): an internal error occurred …` + hint `-> set FLOWPLANE_CERT_ISSUER_CA_CERT_PATH and FLOWPLANE_CERT_ISSUER_CA_KEY_PATH …` |
| §2 `dataplane cert issue dp-local --ttl-hours 24` — CP started **with** the issuer CA | **pass (exit 0)** | `certificate_pem`, `private_key_pem`, `ca_certificate_pem` all present |
| §2 `dataplane cert list` | pass | issued cert listed with serial + SPIFFE URI; `expires_at` = `issued_at` + 24h |
| §4 `dataplane get dp-local` | pass | `last_heartbeat_at: None` (no agent — as documented) |

Issued SPIFFE URI matches the documented format with default trust domain `flowplane.local`:

```text
spiffe://flowplane.local/org/00000000-0000-000f-1071-000000000001/team/00000000-0000-000f-1071-000000000002/proxy/019eec60-5d36-7dd0-b19e-92aaba3104c1
```

§3 (live `fp-agent` → `/healthz` → heartbeat advancing) is **blocked by environment** (the
sandbox reaps long-lived children). The agent flag/env surface matches source
(`configuration.md` / `cli.md`). The spec links are now under "Design references (optional)"
(#118 policy). No discrepancies.

## Proof: `docs/how-to/ai-gateway-route-budget.md`  (#122 **fixed**, #123 **fixed**; 1 new minor)

| Command | Result | Excerpt |
|---|---|---|
| Prereq: `secret create` while CP ran **without** the encryption key | pass-as-documented | `error (unavailable): secret encryption key is not configured` (exit 6) — exactly what the doc's Prereqs now warn about (**#122 fixed**) |
| `secret create` after restarting CP **with** `FLOWPLANE_SECRET_ENCRYPTION_KEY` | pass | `created "openai-key" (revision 1)` |
| §1 `ai providers create` (TLS `base_url`) | pass | `created "openai-prod" (revision 1)` |
| §2 `ai routes create` | pass | `status: active`; `materialized` → `cluster_names ["ai-chat-route-b1"]`, `listener_name ai-chat-route-listener`, `route_config_name ai-chat-route-routes` |
| §2 generated cluster loadable by Envoy | **pass (#123 fixed)** | real Envoy CDS `added/updated 3 cluster(s)`; admin `/clusters` lists `ai-chat-route-b1::`; `ops xds nacks` → `no rows`, `RECENT NACK COUNT 0`, **no** `auto_sni_san_validation` rejection |
| §3 `ai budgets create` (shadow) | pass | `created "chat-budget" (revision 1)` |
| §4 chat request through `:19000` | **blocked** | needs live OpenAI provider + egress |

**#123 confirmed fixed:** the TLS-upstream AI cluster now materialises with a default
validation context (verify-by-default, `FLOWPLANE_UPSTREAM_CA_BUNDLE`), and
`auto_sni_san_validation` is only set when a validation context is present
(`crates/fp-xds/src/translate.rs:710-723`). A live Envoy loads `ai-chat-route-b1` cleanly.

**NEW (minor, `doc-defect`):** the how-to says "create a secret holding the provider API
key — `flowplane secret create --file secret.json`" but **never shows the contents of
`secret.json`**, and the shape (`{"name":…,"spec":{"type":"generic_secret","secret":"<base64>"}}`)
is not documented anywhere in `docs/`. Completing the step required three trial-and-error
attempts guided only by error hints (`missing field \`type\``; then `generic secret must be
base64`). Filed below.

## Proof: `docs/how-to/learn-and-publish-api-spec.md`

| Command | Result | Excerpt |
|---|---|---|
| `api create orders-api` | pass | `created api/v1/teams/default/api-definitions` |
| §1 `learn start … --api orders-api --target-sample-count 1000` | pass | `created "orders-learn-2026-06"` |
| §2 `learn get` | pass | `STATUS capturing`, `SAMPLE/PATH/BYTE COUNT 0`, `TARGET SAMPLE COUNT 1000` |
| `learn list` | pass | row present |
| §3 `learn stop` | pass (exit 0) | session transitions |
| §4 `learn generate-spec` (no traffic) | pass-as-documented | `error (validation_failed): learning session has no raw observations to aggregate` |
| §5 `api spec publish orders-api 3` | pass-as-documented | `error (not_found): spec version "3" not found` |
| §6 `api status` / `mcp status` | pass | `TOOL COUNT 0`; MCP status renders (`STATIC TOOL COUNT 35`) |

The full publish path needs captured traffic (Envoy + learning ExtProc + real requests),
which was not driven; both documented failure messages reproduce verbatim. The doc's
endpoints/CLI (singular `spec-version`; `learn discover` plural `spec-versions`) match
`cli.md` and `rest-api.md`. No discrepancies.

## Proof: `docs/how-to/bootstrap-platform.md`  (new — all pass)

Ran a fresh **non-dev** instance against a clean database.

| Step / Command | Result | Excerpt |
|---|---|---|
| §2 fail-closed: uninitialized, non-dev, **no** token | pass-as-documented | `Error: instance is uninitialized and no bootstrap token was supplied; set FLOWPLANE_BOOTSTRAP_TOKEN or FLOWPLANE_BOOTSTRAP_TOKEN_FILE …` (refuses to start) |
| token too short (`<32` chars) | pass-as-documented | `Error: the supplied bootstrap token is too short; it must be at least 32 characters after trimming whitespace` |
| token never logged | pass | `grep` for the token value in CP logs → `0` matches |
| §3 `POST /api/v1/bootstrap/initialize` | pass | `{"org_id":"…","admin_user_id":"…"}` / `STATUS=200` |
| §4 replay initialize | pass-as-documented | `{"code":"conflict","message":"this instance is already initialized"}` / `STATUS=409` |
| `GET /api/v1/bootstrap/status` (public) | pass | `{"initialized":true}` / `STATUS=200` |

No discrepancies.

## Proof: `docs/reference/cli.md`

`--help` walked for top-level and nested commands.

| Check | Result | Excerpt |
|---|---|---|
| top-level command set | pass | `serve db openapi auth config org team cluster listener route api mcp ai learn secret dataplane expose unexpose stats ops apply completion version` — matches doc (plus clap `help`) |
| global options | pass | `--context`, `--server`, `--team`, `--org`, `-o/--output {table,json,yaml,wide}`, `--json`, `--no-color`, `--quiet`, `--verbose`, `--dry-run`, `-y/--yes`, `--revision`, `--timeout` (default 30), `--out` all present |
| `dataplane` subcommands | pass | `list get create telemetry bootstrap cert` |
| `dataplane cert` | pass | `list register issue revoke` |
| `learn` subcommands | pass | `discover start list get stop generate-spec cancel` |
| `learn discover` | pass | `start list status stop generate-spec` |
| `ai` | pass | `providers routes budgets usage` |

No discrepancies.

## Proof: `docs/reference/configuration.md`

| Check | Result | Excerpt |
|---|---|---|
| "Every `FLOWPLANE_*` variable" claim | pass | `grep -rhoE 'FLOWPLANE_[A-Z0-9_]+' crates/` ∖ docs = only `FLOWPLANE_TEST_DATABASE_URL` (test-only, intentionally excluded). `FLOWPLANE_UPSTREAM_CA_BUNDLE` present (**#132 fixed**); bootstrap-token vars present (**#113**) |
| ² D-008: no TLS + no `FLOWPLANE_API_INSECURE` | pass (source/behaviour) | startup refuses without the flag |
| ⁷ secret key validated at **use time** as `unavailable` | pass | reproduced (AI prereq, exit 6 / 503) |
| cert-issuer prereq surfaced on use | pass | issuance without `FLOWPLANE_CERT_ISSUER_CA_*` → `invalid_config` naming those exact vars |
| ¹³ bootstrap fail-closed | pass | reproduced under bootstrap-platform above |

## Proof: `docs/reference/errors.md`  (1 new minor)

Status mapping verified against source (`crates/fp-api/src/error.rs:34-45`) — all 13 codes
match the table exactly (e.g. `validation_failed`→400, `quota_exceeded`→422,
`invalid_config`/`internal`→500, `unavailable`→503). Live responses confirm the envelope:

| Code | Status | How observed |
|---|---|---|
| `unauthorized` | 401 | `{"code":"unauthorized","message":"missing bearer token","hint":"…","request_id":…}` |
| `validation_failed` | 400 | empty `stat_prefix` → `{"code":"validation_failed",…}` / `STATUS=400`; missing `If-Match` → same |
| `not_found` | 404 | `cluster "nonexistent" not found`; `spec version "3" not found` |
| `revision_mismatch` | 409 | stale `--revision` on `route update` |
| `conflict` | 409 | bootstrap replay |
| `invalid_config` | 500/redacted | cert-issue without CA → `an internal error occurred; report the request_id…` + actionable hint |
| `unavailable` | 503 | missing secret key (CLI exit 6) |

**NEW (minor, `code-defect`):** a request body that fails JSON **type** deserialization is
returned as a **bare `422`** with body `Failed to deserialize the JSON body into the target
type: …` and **no `code` field** — i.e. outside the documented envelope. This contradicts
errors.md ("**Every** Flowplane API failure is returned as a single stable JSON envelope
carrying a machine-readable `code`") and the table that maps `422` exclusively to
`quota_exceeded`. Semantically-invalid-but-well-typed bodies and missing `If-Match` correctly
return the envelope at `400`; only the deserialization path leaks the framework default. Root
cause: handlers use the stock `axum::Json<T>` extractor with no rejection wrapping
(`crates/fp-api/src/*_api.rs`). Filed below.

## Proof: `docs/reference/filters.md`

| Check | Result | Excerpt |
|---|---|---|
| chain invariant: each `type` at most once | pass | duplicate `local_rate_limit` → `error (validation_failed): duplicate filter type "local_rate_limit" in the chain` (matches documented message) |
| `jwt_auth` + `local_rate_limit` chain entries accepted | pass | `edge` listener created from the documented field shapes |
| `jwt_auth` reference-only per-route override | pass | route-level `{ "type":"jwt_auth","requirement_name":"require-auth0" }` accepted (in `api-routes`) |
| `local_rate_limit` full override per-route | pass | route-level `LocalRateLimitConfig` accepted (in `api-routes`) |

No discrepancies.

## Proof: `docs/reference/rest-api.md`

| Check | Result | Excerpt |
|---|---|---|
| operational endpoints public | pass | `/api/v1/bootstrap/status` → `200` without auth |
| catalogue completeness | pass | programmatic diff of all 62 generated OpenAPI paths vs the catalogue: `missing-from-doc: 0` |
| `If-Match` convention (`validation_failed`→400, `revision_mismatch`→409) | pass | verified live (errors.md table above) |
| Bearer / `fpat_` and `X-Flowplane-Org` selector | pass (source/whoami) | `whoami` reports `ORG SELECTOR REQUIRED false` |

(Note: the 422 deserialization leak above also affects rest-api.md's "Errors always use the
envelope" statement — same root cause, filed once.)

## Proof: `docs/how-to/secret-kek-rotation.md`  (executed end to end)

Ran the rotation procedure (steps 1–6) against the live CP, restarting it between keyrings.

| Step / Command | Result | Excerpt |
|---|---|---|
| create `kek-test` secret while CP active key id = `key-a` | pass | `created "kek-test" (revision 1)`; `secret get` → `encryption_key_id: key-a` |
| steps 2–5: restart CP with `FLOWPLANE_SECRET_ENCRYPTION_KEY`=new, `_KEY_ID=key-b`, `_KEYS={"key-a":"…"}` | pass | starts clean; old secret stays present and decryptable (key-a in keyring) |
| step 6: `secret rotate kek-test --revision 1 --file <new spec>` | pass | `created "kek-test" (revision 2)`; `secret get` → `encryption_key_id: key-b` (re-encrypted with the new active key) |
| step 7 warning (line 28) | confirmed independently | a leftover secret whose `encryption_key_id` (`default`) was **not** in the keyring logged the exact documented behaviour: `skipping undecryptable SDS secret during xDS rebuild … secret encryption key "default" is not configured` |

The KEK env-var contract (raw-32/base64 key material; `_KEY_ID` written to new/rotated rows;
`_KEYS` retired keyring) matches `configuration.md`. No discrepancies.

## Proof: operator runbooks (inspection only — epic non-goals, need cloud/full deploy)

| Doc | Check | Result |
|---|---|---|
| `aws-secure-deployment.md` | `flowplane … dataplane cert issue` (the old #120) | **#120 fixed** — line 121 uses `dataplane cert issue`, a real subcommand. Full AWS/Terraform deploy not runnable here. |
| `production-readiness.md` | command shapes (`db migrate`, `route generate --from-spec --listener-port`, `route apply`, `api spec publish`, `mcp enable --api`, `learn discover …`) | all resolve to real CLI subcommands/flags via `--help`. Full production deploy not runnable here. |

## Prior defects — re-verification (all CLOSED upstream)

| Issue | Was | This pass |
|---|---|---|
| #118 | user docs link into `spec/`/`internal/` despite standalone policy | **Resolved** — `docs/README.md` now permits optional spec links under "Further reading"/"Design references"; in-scope how-tos comply |
| #119 | README/tutorial PostgreSQL helper fails on missing `postgres` role | **Resolved** — README, getting-started §1, and `internal/dev-dataplane.md` §1 document the role caveat + macOS steps; helper works here |
| #120 | `aws-secure-deployment.md` used top-level `cert issue` | **Fixed** — now `dataplane cert issue` |
| #121 | `jwt-auth-rate-limit-route.md` wrong `PathMatch` shape | **Fixed** — nested `prefix` accepted verbatim |
| #122 | AI how-to omitted `FLOWPLANE_SECRET_ENCRYPTION_KEY` prereq | **Fixed** — prereq now stated; `unavailable` reproduces as the doc warns |
| #123 | AI cluster NACKed (`auto_sni_san_validation` w/o validation context) | **Fixed** — verify-by-default; live Envoy loads `ai-chat-route-b1`, 0 NACKs |
| #132 | `FLOWPLANE_UPSTREAM_CA_BUNDLE` missing from config catalogue | **Fixed** — row present |

## Issues Raised This Pass

New (both `minor`):

- **#138** — `[docs-verify] errors.md / rest-api.md — malformed JSON body returns a bare 422
  outside the documented error envelope` · `code-defect` · `minor`.
- **#139** — `[docs-verify] ai-gateway-route-budget.md — secret.json contents are never shown
  (step is not completable as written)` · `doc-defect` · `minor`.

## Counts

- New issues raised this pass: **2** — by classification `code-defect` 1 · `doc-defect` 1;
  by severity `minor` 2.
- Prior defects re-verified as fixed/resolved: **7** (#118, #119, #120, #121, #122, #123, #132).
- Docs executed end-to-end (DB + Envoy backed): README, getting-started (incl. live curl +
  unexpose), internal/dev-dataplane, **cli-auth-and-contexts (contexts + login/token/logout
  persistence)**, jwt-auth-rate-limit-route (§1–§3), register-dataplane-mtls (incl. live cert
  issue), ai-gateway-route-budget (incl. live Envoy load of the AI cluster),
  learn-and-publish-api-spec, bootstrap-platform, **secret-kek-rotation (full KEK rotation
  across two key ids)**, and all 5 reference docs.
- Inspection / command-shape only (need cloud infra or a full production deploy, not the
  docs' fault): `aws-secure-deployment.md`, `production-readiness.md` (both epic non-goals).
- Blocked **by the environment** (not the docs): live `fp-agent` `/healthz`+heartbeat
  (register-dataplane-mtls §3–§4; sandbox reaps long-lived children); JWT `401/429`
  data-plane verification (no IdP); AI chat round-trip (no OpenAI egress). No workarounds that
  diverge from the docs were applied.
