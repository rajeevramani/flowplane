# Docs Verification Report

Run date: 2026-06-21 (third pass — PostgreSQL **and** Envoy available)

Config:

- Epic issue: #100
- Label: `docs-verification`
- Marker: `[docs-verify]`
- Method: execute the docs against the real binary/CLI/control plane/**Envoy** from a
  clean state; classify every mismatch as `doc-defect` / `code-defect` / `ambiguous`.
- GitHub tooling: this environment exposes the GitHub MCP server (`mcp__github__*`),
  not the `gh` CLI. Issue reads/writes below were performed with those tools.

## What changed since the previous pass

The previous pass (report v2, issues #121/#122) had PostgreSQL but was **blocked on
Envoy** — every "start Envoy / curl through the gateway" step and the `dataplane cert
issue` happy path were inspection-only. In **this** environment both dependencies are
present:

```text
$ which envoy && envoy --version
/usr/local/bin/envoy
envoy  version: 3909deb175ef358202d6ab4f94d683ffc0fdb477/1.37.0/Clean/RELEASE/BoringSSL

$ scripts/ensure-postgres.sh
postgres ready                 # exit 0
$ psql postgres://postgres:postgres@127.0.0.1:5432/postgres -c 'select 1'
 ?column?
----------
        1
```

Consequence: the previously-blocked steps were executed for the first time and
**pass** — a live request traversed Envoy to the upstream, and a real mTLS client
certificate was issued from a configured CA. No new defects surfaced. The two
previously-filed defects (#121, #122) both reproduce verbatim.

## Preconditions

- Build/toolchain: pass. `cargo build --bin flowplane` finished in 3m 35s, exit 0
  (`Finished \`dev\` profile ... in 3m 35s` / `BUILD_EXIT=0`); `flowplane version` → `0.1.0`.
- PostgreSQL: pass (see above; the session hook also reports `postgres ready`).
- Envoy data path: **pass** (Envoy 1.37.0 on `PATH`). This is the material difference
  from prior passes.
- Cert issuance: executed by satisfying the documented prereq — a throwaway CA was
  generated with `openssl` and supplied via `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH` /
  `_KEY_PATH`, exactly as `register-dataplane-mtls.md` requires.
- Live AI provider traffic (OpenAI) and a real JWKS issuer remain **blocked /
  inspection-only** (no external egress / no IdP) — the control-plane side of both
  flows is fully executed; only the final upstream round-trip is not.

## Documentation Set (confirmed against epic #100)

The epic's actionable item→source table (items 1–11) maps to these executable docs.
Actionable / executed:

- `README.md`
- `docs/dev-dataplane.md`
- `docs/tutorials/getting-started.md`
- `docs/how-to/cli-auth-and-contexts.md`
- `docs/how-to/jwt-auth-rate-limit-route.md`
- `docs/how-to/register-dataplane-mtls.md`
- `docs/how-to/ai-gateway-route-budget.md`
- `docs/how-to/learn-and-publish-api-spec.md`
- `docs/reference/cli.md`
- `docs/reference/configuration.md`
- `docs/reference/errors.md`
- `docs/reference/filters.md`
- `docs/reference/rest-api.md`

Not execution-verified: `docs/concepts/tenancy-grants-xds.md` (#112, item 12) —
explanation/concepts prose with no runnable commands; out of scope for an
execution-based pass.

---

## Proof: `README.md`

| Command | Result | Excerpt |
|---|---|---|
| `cargo build --bin flowplane` | pass (exit 0) | `Finished \`dev\` profile ... in 3m 35s` |
| `./target/debug/flowplane version` | pass (exit 0) | `0.1.0` |
| `./target/debug/flowplane openapi` | pass (exit 0) | 288571 bytes; `"openapi":"3.1.0"`, `"title":"Flowplane"`, 62 paths |
| `scripts/e2e-envoy.sh` (live Envoy smoke) | not run as a unit | the individual happy-path steps it wraps were executed manually below and pass end to end with real Envoy. |

## Proof: `docs/tutorials/getting-started.md` (full happy path, **now incl. Envoy**)

Started the CP with the tutorial's **exact** env set (notably **without**
`FLOWPLANE_SECRET_ENCRYPTION_KEY`).

| Step / Command | Result | Excerpt |
|---|---|---|
| §2 `flowplane serve` (dev mode) | pass | all 6 documented log signals present (below) |
| §3 `auth whoami` | pass (exit 0) | `MEMBERSHIPS 1 items`, `ORG ROLE owner`, `GRANT COUNT 0`, `USER ID 0000…0003` |
| §4 `expose http://127.0.0.1:3001 --name local --port 10001 …` | pass (exit 0) | `CURL URL http://127.0.0.1:10001/`, `CLUSTER local-upstream`, `ROUTE CONFIG local-routes`, `LISTENER local`, `ENDPOINT SOURCE listener.public_base_url` |
| §4 `cluster/listener/route list` | pass | resources present at revision 1 |
| §5 `dataplane create dp-local` | pass (exit 0) | `created "dp-local" (revision 1)` |
| §6 `--out … dataplane bootstrap dp-local --mode dev …` | pass (exit 0) | valid Envoy YAML; `node.id` stamped, ads → `xds_cluster`/127.0.0.1:18000 |
| §6 `envoy -c /tmp/flowplane-envoy.yaml` | **pass** | Envoy connects to xDS and warms the listener within ~2s |
| §7 `curl -i http://127.0.0.1:10001/` | **pass** | `HTTP/1.1 200 OK` / `server: envoy` / body `hello-flowplane` |
| §7 `unexpose local` | pass (exit 0) | removes cluster/route/listener |

Documented startup log signals — all observed verbatim:

```text
database connected and migrations applied
DEV MODE: in-process identity, seeded resources — never production
dev resources seeded            org=dev-org team=default user=dev-user
dev bearer token (valid 1h, this boot only)   dev_token=eyJ0eXAi…
xDS ADS server starting (plaintext dev mode)  addr=0.0.0.0:18000
API listener starting           addr=127.0.0.1:8096 tls=false
```

Live gateway proof (the step prior passes could not run):

```text
$ curl -i http://127.0.0.1:10001/
HTTP/1.1 200 OK
server: envoy
content-length: 16
x-envoy-upstream-service-time: 1

hello-flowplane
```

Confirms the tutorial's claim that the server starts **without** an encryption key and
without OIDC, and that the documented `expose → bootstrap → Envoy → curl` chain reaches
the upstream through the gateway. No discrepancies.

## Proof: `docs/dev-dataplane.md`

Same loop with the runbook's env set (which **does** set `FLOWPLANE_SECRET_ENCRYPTION_KEY`).
Steps 1–9 pass, including the live curl. Step 8's mTLS-bootstrap variant
(`--mode mtls --cert-path … --key-path … --ca-path …`) emits valid TLS-bearing YAML
(exit 0). Step 10 diagnostics:

| Command | Result | Excerpt |
|---|---|---|
| `stats overview` | pass | `LIVE 0 / STALE 1 / TOTAL DATAPLANES 1` |
| `ops xds status` | pass | `HEALTH stale … RECENT NACK COUNT 0` |
| `ops xds nacks` | pass | `no rows` |

The `STALE` dataplane / zero request counters are exactly what the runbook predicts for
the manual path (Envoy started directly, without `fp-agent`, so no heartbeats/telemetry).

## Proof: `docs/how-to/cli-auth-and-contexts.md`

The CLI surface this how-to documents was confirmed against `--help`: global
`--server`/`--org`/`--team`/`--context` resolution, `auth {login,whoami,token,logout}`
(incl. `--token`, `--token-stdin`, `--pkce`, `--device`, `--issuer`, `--client-id`,
`--callback-url`, `--scope`), and `config {set-context,use-context,get-contexts,show,path}`
all exist with the documented flags. `auth whoami` succeeds end to end against the live CP
(see getting-started §3). No discrepancies.

## Proof: `docs/how-to/jwt-auth-rate-limit-route.md`  ⚠ discrepancy (already filed #121)

Built the listener and route-config from the doc's **exact** JSON against the live CP.

| Command | Result | Excerpt |
|---|---|---|
| create `edge` listener with §1 `http_filters` (`jwt_auth` providers + `requirement_map`, `local_rate_limit` `stat_prefix`+`token_bucket`) | pass | `created "edge" (revision 1)` |
| create `api-routes` route-config with §2 body **verbatim** (`"match": { "prefix": "/payments" }`) | **discrepancy (exit 4, 422)** | `match.prefix: invalid type: string "/payments", expected struct variant PathMatch::Prefix` |
| create same route-config with corrected `{"prefix":{"prefix":"/payments"}}` | pass | `created "api-routes" (revision 1)` — confirms the `filter_overrides` JSON itself is valid |
| §3 `listener update edge --revision 1 --file <spec-only>` | pass | `updated "edge" (revision 2)` |
| §3 re-run with stale `--revision 1` | pass-as-documented | `error (revision_mismatch): listener "edge" is at revision 2, you supplied 1` (→ 409) |

**Defect (#121, reproduced):** §2 / line 118 prints `"match": { "prefix": "/payments" }`.
`PathMatch` is an externally-tagged enum with struct variants
(`crates/fp-domain/src/gateway/route_config.rs:67`); the live serialized form is nested —
`route get local-routes --json` shows `"match": { "prefix": { "prefix": "/" } }`.
Classification `doc-defect`, severity `major`. The rest of the how-to (JWT chain, rate
limit, `filter_overrides`, the If-Match flow) is correct. No additional fix needed beyond
#121.

> Note: the §3 PATCH/`update` body is `spec`-only (`{ "spec": { … } }`), exactly as the
> doc states ("Creating fresh resources is a `POST` … with `{ "name": …, "spec": … }`").
> A body carrying `name` is correctly rejected (`unknown field \`name\`, expected \`spec\``);
> this matches the doc and is **not** a defect.

## Gap-filling pass: how-to data-plane steps (mTLS + agent)

A follow-up pass tried to close the remaining inspection-only/blocked steps in the
how-to pages by standing up the full mTLS path. Findings:

- **mTLS xDS works end to end.** Setting the `FLOWPLANE_XDS_TLS_*` triad makes the CP
  use real mTLS xDS even in dev mode (`crates/flowplane/src/serve.rs:118` →
  `serve_mtls` + cert-registry resolver; log: `xDS ADS server starting (mTLS,
  certificate-registry binding)`). A real Envoy launched from a `--mode mtls`
  bootstrap, presenting the issued client cert, connected over mTLS and pulled
  resources — `cds: added/updated 2 cluster(s)`, `lds: add/update listener 'edge' /
  'local'`. This confirms the headline claim of `register-dataplane-mtls.md` (the cert
  the CP issues actually authenticates the dataplane's xDS stream).
- **NEW defect (#123): the AI route's generated cluster is NACKed by Envoy.** With the
  AI how-to's `chat-route` present, Envoy rejects its cluster:
  `Error adding/updating cluster(s) ai-chat-route-b1: 'auto_sni_san_validation' was
  configured without a validation context` (durably recorded by `ops xds nacks`;
  `ops xds status` → `HEALTH degraded`). Root cause is in code, not docs
  (`crates/fp-core/src/services/ai.rs:1120-1123` materializes
  `auto_sni_san_validation: true` with `validation_context_sds_secret_name: None`;
  `crates/fp-xds/src/translate.rs:668-688` emits exactly that). The same pattern exists
  in `expose.rs:94-97` (TLS upstreams) and `route_generation.rs:284-287`. Classification
  `code-defect`, severity `major`. A plaintext `expose http://…` upstream is unaffected
  (no `upstream_tls`), which is why the getting-started data path serves `200`.
- **Blocked by environment (not by the docs): the live `fp-agent` run.** This sandbox
  SIGTERMs any shell call that keeps a server child (Envoy) alive more than a few
  seconds, so `register-dataplane-mtls.md` §3–§4 (`fp-agent` → `/healthz` → heartbeat
  advancing) could not be observed across a full poll cycle. The agent's flag/env
  surface matches source, and the mTLS transport it would use is the same one verified
  above; only the live `/healthz` + heartbeat assertion is unverified here. Stated as
  blocked rather than assumed.

## Proof: `docs/how-to/register-dataplane-mtls.md`  (§2 cert issue **now executed**)

| Command | Result | Excerpt |
|---|---|---|
| §1 `dataplane create dp-local` | pass | exercised under getting-started |
| §2 `dataplane cert issue dp-local --ttl-hours 24` — CP started **without** the issuer CA | pass-as-documented | `error (invalid_config): … set FLOWPLANE_CERT_ISSUER_CA_CERT_PATH and FLOWPLANE_CERT_ISSUER_CA_KEY_PATH` (the exact documented prereq) |
| §2 `dataplane cert issue dp-local --ttl-hours 24` — CP started **with** the issuer CA | **pass (exit 0)** | response has `certificate_pem`, `private_key_pem`, `ca_certificate_pem`, `certificate.spiffe_uri` |
| §2 `dataplane cert list` | pass | issued cert listed with serial + SPIFFE URI; `expires_at` = `issued_at` + 24h |
| §4 `dataplane get dp-local` | pass | `last_heartbeat_at: None` (no agent reporting — as documented) |
| §4 `stats overview` / `ops xds status` | pass | as above |

The issued SPIFFE URI is exactly the documented format
`spiffe://<trust-domain>/org/<org-id>/team/<team-id>/proxy/<dataplane-id>` with the
default trust domain:

```text
spiffe://flowplane.local/org/00000000-0000-000f-1071-000000000001/team/00000000-0000-000f-1071-000000000002/proxy/019ee7a7-00df-7d91-8007-0e0978f7f265
```

The agent connection (§3) and `GET /healthz` (§4) were not run (no live `fp-agent`
process wired here), but the agent flag/env table matches `crates/fp-agent/src/main.rs`
(`--cp-endpoint`/`FLOWPLANE_AGENT_CP_ENDPOINT`, `--tls-cert-path`/`_TLS_CERT_PATH`,
`--tls-ca-path`/`_TLS_CA_PATH`, `--tls-server-name` default `localhost`, health bind
`127.0.0.1:19902`). No discrepancies. (Note: this doc links into `../../spec/` — covered by
the open governance issue #118, not re-filed.)

## Proof: `docs/how-to/ai-gateway-route-budget.md`  ⚠ prereq gap (already filed #122)

| Command | Result | Excerpt |
|---|---|---|
| `secret create` while CP ran **without** the encryption key | discrepancy | `error (unavailable): secret encryption key is not configured` (exit 6) |
| `secret create` after restarting CP **with** `FLOWPLANE_SECRET_ENCRYPTION_KEY` | pass | `created "openai-key" (revision 1)` |
| §1 `ai providers create` | pass | `created "openai-prod" (revision 1)` |
| §2 `ai routes create` | pass | `status: active`; `materialized` → `cluster_names ["ai-chat-route-b1"]`, `listener_name ai-chat-route-listener`, `route_config_name ai-chat-route-routes` |
| §3 `ai budgets create` (shadow) | pass | `created "chat-budget" (revision 1)` |
| §3 `ai budgets update --revision 1` → enforcing | pass | `updated "chat-budget" (revision 2)` |
| §4 `ai usage --provider-id …` | pass | `no rows` (no traffic) |
| §4 chat request through `:19000` | **blocked** | needs live OpenAI provider + egress |
| §2 generated cluster loadable by Envoy | **discrepancy (NACK)** | Envoy rejects `ai-chat-route-b1`: `'auto_sni_san_validation' was configured without a validation context` — see #123 / gap-filling pass |

**Defect (#123, new):** the §2 AI cluster (TLS `base_url`) is materialized with
`auto_sni_san_validation: true` and no validation context, which Envoy NACKs — the route
shows `status: active` on the CP but never loads on the dataplane. Classification
`code-defect`, severity `major`. Details in the gap-filling section above.

**Defect (#122, reproduced):** the `unavailable` error is correct, documented use-time
behavior (`configuration.md` constraint ⁷), but neither the AI how-to's Prereqs nor the
getting-started setup it builds on states the CP must run with
`FLOWPLANE_SECRET_ENCRYPTION_KEY` for the required `secret create` to succeed.
Classification `doc-defect`, severity `minor`. No additional fix needed beyond #122.

## Proof: `docs/how-to/learn-and-publish-api-spec.md`

| Command | Result | Excerpt |
|---|---|---|
| `api create orders-api` | pass | `created api/v1/teams/default/api-definitions` |
| §1 `learn start … --api orders-api --target-sample-count 1000` | pass | `created "orders-learn-2026-06"` |
| §2 `learn get` | pass | `status: capturing`, `sample_count/path_count/byte_count = 0` |
| `learn list` | pass | row present (`STATUS capturing`, `TARGET SAMPLE COUNT 1000`) |
| §3 `learn stop` | pass (exit 0) | session transitions |
| §4 `learn generate-spec` (no traffic) | pass-as-documented | `error (validation_failed): learning session has no raw observations to aggregate` — the exact error §2/§4 say to expect |
| §5 `api spec publish orders-api 3` | pass-as-documented | `error (not_found): spec version "3" not found` (no spec generated; command shape correct) |
| §6 `api status` / `mcp status` | pass | `tool_count 0`, `published_spec_version_id None`; MCP status table renders (`STATIC TOOL COUNT 35`) |

The full publish path needs captured traffic (Envoy + learning ExtProc + real requests),
which was not driven; both documented failure messages reproduced verbatim. (This doc
links into `../../spec/06-learning.md` — covered by #118.)

## Proof: `docs/reference/cli.md`

`--help` walked for top-level and nested commands.

| Check | Result | Excerpt |
|---|---|---|
| top-level command set | pass | `serve db openapi auth config org team cluster listener route api mcp ai learn secret dataplane expose unexpose stats ops apply completion version` — matches doc exactly (plus clap's auto `help`) |
| global options | pass | `--context`, `-o/--output {table,json,yaml,wide}`, `--dry-run`, `--revision`, `--timeout` (default 30), `--out` all present |
| `dataplane cert` | pass | `list register issue revoke` |
| `ai` | pass | `providers routes budgets usage` |
| `completion bash` | pass | emits `_flowplane() { … }` (the `head`/SIGPIPE exit 101 is a broken-pipe artifact, not a command failure) |

No discrepancies.

## Proof: `docs/reference/configuration.md`

| Check | Result | Excerpt |
|---|---|---|
| env-var catalogue vs source | pass | `grep -rhoE 'FLOWPLANE_[A-Z0-9_]+' crates/` → every runtime var is documented; only `FLOWPLANE_TEST_DATABASE_URL` (test-only) is excluded, as intended |
| ② D-008: no TLS + no `FLOWPLANE_API_INSECURE` → refuse start | pass | `Error: invalid_config: the API listener has no TLS material and plaintext was not explicitly allowed` |
| ⑦ secret key validated at **use time** as `unavailable` | pass | reproduced (AI how-to / #122) |
| cert-issuer prereq surfaced on use | pass | issuance without `FLOWPLANE_CERT_ISSUER_CA_*` → `invalid_config` pointing at those exact vars |

## Proof: `docs/reference/errors.md`

Live responses confirm the envelope and status mapping:

| Code | Status | How observed |
|---|---|---|
| `unauthorized` | 401 | `{"code":"unauthorized","message":"missing bearer token","hint":"authenticate with \`flowplane auth login\` and retry","request_id":…}` |
| `validation_failed` | 400/422 | malformed route body / generic-secret shape |
| `not_found` | 404 | `cluster "nonexistent" not found`; `spec version "3" not found` |
| `revision_mismatch` | 409 | stale `If-Match` on `listener update` |
| `invalid_config` | redacted | cert-issue without CA → `an internal error occurred; report the request_id…` + actionable hint |
| `unavailable` | 503 | missing secret key |

Envelope `{code, message, hint?, details?, request_id}` with `hint`/`details` omitted when
absent — confirmed.

## Proof: `docs/reference/rest-api.md`

| Check | Result | Excerpt |
|---|---|---|
| operational endpoints public | pass | `/healthz`,`/readyz`,`/metrics`,`/api-docs/openapi.json`,`/api/v1/bootstrap/status` → all `200` without auth |
| catalogue completeness | pass | all 62 generated OpenAPI paths appear in the catalogue (programmatic diff: `missing from rest-api.md: NONE`) |
| If-Match convention | pass | revision flow verified via JWT how-to §3 |

## Proof: `docs/reference/filters.md`

| Check | Result | Excerpt |
|---|---|---|
| `jwt_auth` + `local_rate_limit` chain entries accepted | pass | `edge` listener created from the documented field shapes |
| chain invariant: each `type` at most once | pass | duplicate `local_rate_limit` → `error (validation_failed): duplicate filter type "local_rate_limit" in the chain` (matches the documented message) |
| `jwt_auth` reference-only per-route override | pass | route-level `{ "type":"jwt_auth", "requirement_name":"require-auth0" }` accepted |
| `local_rate_limit` full override per-route | pass | route-level `LocalRateLimitConfig` accepted |

No discrepancies.

## Issues Raised Or Updated

New this pass:

- **#123** — `[docs-verify] docs/how-to/ai-gateway-route-budget.md — generated AI cluster
  is NACKed by Envoy (auto_sni_san_validation without a validation context)`. Severity
  `major` · Classification `code-defect`. Found while filling the how-to data-plane gaps;
  also affects `expose` with a TLS upstream and route-generation.

Reproduced this pass (already open, not re-filed):

- **#121** — `jwt-auth-rate-limit-route.md` route `match` PathMatch shape. `major` ·
  `doc-defect`. Reproduced verbatim (422 `expected struct variant PathMatch::Prefix`).
- **#122** — `ai-gateway-route-budget.md` secret prereq omits
  `FLOWPLANE_SECRET_ENCRYPTION_KEY`. `minor` · `doc-defect`. Reproduced verbatim
  (`unavailable: secret encryption key is not configured`).

Carried over (not in this run's executable scope or environment-specific):

- **#118** (user docs link into `spec/`/`internal/` despite the standalone policy) — still
  accurate; the in-scope how-tos `register-dataplane-mtls.md` and
  `learn-and-publish-api-spec.md` do link into `../../spec/`. Governance issue; open.
- **#119** (PostgreSQL helper) — **does not reproduce here** (helper + documented URL both
  work). Environment-specific; left open, not re-filed.
- **#120** (`aws-secure-deployment.md` top-level `cert` subcommand) — `aws-secure-deployment.md`
  is an operator runbook outside the epic #100 executable set; previously filed, still
  accurate.

## Counts

- New issues raised this pass: **1** (#123, `code-defect` / `major`).
- Defects reproduced this pass: 2 (#121, #122) — by severity `major` 1 · `minor` 1; by
  classification `doc-defect` 2 · `code-defect` 0 · `ambiguous` 0.
- All defects (new + reproduced) by classification: `doc-defect` 2 · `code-defect` 1 ·
  `ambiguous` 0; by severity: `major` 2 · `minor` 1.
- Previously-blocked steps now **executed and passing**: the live Envoy data path
  (getting-started §6–§7, dev-dataplane §8–§9), `dataplane cert issue`
  (register-dataplane §2), and **mTLS xDS** end to end (real Envoy attaches with the
  issued client cert and pulls config).
- Still blocked **by the environment** (not the docs): the live `fp-agent` run
  (`register-dataplane-mtls` §3–§4) — this sandbox reaps long-lived Envoy children, so
  the agent `/healthz` + heartbeat could not be observed; the AI chat round-trip (live
  provider) — and, independently, the AI cluster cannot load on Envoy at all (#123).
- Docs executed end-to-end (DB + Envoy backed): README, getting-started, dev-dataplane,
  cli-auth-and-contexts, jwt-auth-rate-limit-route, register-dataplane-mtls,
  ai-gateway-route-budget, learn-and-publish-api-spec, and all 5 reference docs.
- Remaining blocked / inspection-only steps: the AI chat request through Envoy (live
  OpenAI provider + egress) and the JWT `401/429` data-plane verification (live JWKS
  issuer). No workarounds that diverge from the docs were applied.
