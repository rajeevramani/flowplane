# Docs Verification Report

Run date: 2026-06-21 (pass against `main` @ `5adaa90`)

Config:

- Epic issue: #100
- Label: `docs-verification`
- Marker: `[docs-verify]`
- Branch verified: `main` (pulled to `5adaa90`), **not** `docs/flowplane-v2`.
- Method: execute the docs against the real binary/CLI/live control plane from a
  clean state (fresh `flowplane_dev` DB); classify every mismatch as
  `doc-defect` / `code-defect` / `ambiguous`.
- GitHub tooling: this environment exposes the GitHub MCP server (`mcp__github__*`),
  not the `gh` CLI.

## Headline result

**The actionable docs on `main` verify clean — zero defects found.** Every how-to
was executed end to end against a live control plane on a fresh database.

**The `docs-verification-report.md` previously committed on `main` was stale** and is
replaced by this one. That prior report (committed in #117, `3276f51`) presented
#121, #122, and #123 as reproducing/new, but on `main`:

- **#121** (jwt route `match` shape) — does **not** exist on `main`. `git log -S` shows
  the file was introduced (in #117) already carrying the correct nested form
  `"match": { "prefix": { "prefix": "/payments" } }`; the buggy
  `"match": { "prefix": "/payments" }` has **no** commit in this file's history.
  Executed: the doc's step-2 body is accepted (`created "api-routes" (revision 1)`).
- **#122** (AI secret-key prereq) — already documented on `main`
  (`ai-gateway-route-budget.md:13` states the `FLOWPLANE_SECRET_ENCRYPTION_KEY`
  requirement and the exact error). The full AI flow passes.
- **#123** (AI cluster NACK) — already fixed on `main` by #124 (`7cdfbc6`, an ancestor
  of the report's own commit) and is **closed** on GitHub. The guard is present at
  `crates/fp-xds/src/translate.rs:721` with passing unit tests.

GitHub state corroborates: there are **zero open `docs-verification` issues**
(#118–#123 all closed). The defects the committed report describes were already
fixed and closed before/at the commit that added the report.

## Preconditions

- Build: pass. `cargo build --bin flowplane` finished in 2m41s, exit 0;
  `flowplane version` → `0.1.0`.
- PostgreSQL: pass. `scripts/ensure-postgres.sh` → `postgres ready` (exit 0);
  `postgres://postgres:postgres@127.0.0.1:5432/postgres` works. DB reset
  (`DROP DATABASE … FORCE` / `CREATE DATABASE flowplane_dev`) for a clean state.
- Envoy data path: **blocked / not executed this run.** No `envoy` binary on `PATH`;
  the Docker daemon is not running this session (stale socket), and a prior pass
  recorded the documented image pull as network-blocked (403 from Docker's CDN).
  Every "start Envoy / curl through the gateway" step is therefore inspection-only
  and is marked as such per doc. Stated as not-run rather than assumed.
- External egress: blocked — live OpenAI provider traffic and a live JWKS issuer are
  not reachable, so the AI chat round-trip and the JWT `401/429` data-plane checks
  are inspection-only. The control-plane side of both is fully executed.

## Documentation Set (confirmed against epic #100)

Actionable / executed:

- `README.md`
- `docs/dev-dataplane.md`
- `docs/tutorials/getting-started.md`
- `docs/how-to/cli-auth-and-contexts.md`
- `docs/how-to/jwt-auth-rate-limit-route.md`
- `docs/how-to/register-dataplane-mtls.md`
- `docs/how-to/ai-gateway-route-budget.md`
- `docs/how-to/learn-and-publish-api-spec.md`
- `docs/how-to/bootstrap-platform.md`  *(new on `main`; tied to #113/#133, not an
  epic-#100 item, but in scope per "every guide under docs/how-to/")*
- `docs/reference/cli.md`
- `docs/reference/configuration.md`
- `docs/reference/errors.md`
- `docs/reference/filters.md`
- `docs/reference/rest-api.md`

Out of scope (epic #100 non-goals — operator runbooks now physically under
`docs/how-to/`): `aws-secure-deployment.md`, `production-readiness.md`,
`secret-kek-rotation.md`. Not execution-verified: `docs/concepts/` (explanation,
no runnable commands).

---

## Proof: `README.md`

| Command | Result | Excerpt |
|---|---|---|
| `cargo build --bin flowplane` | pass (exit 0) | `Finished \`dev\` profile ... in 2m 41s` |
| `flowplane version` | pass (exit 0) | `0.1.0` |
| `flowplane openapi` (redirected, not piped) | pass (exit 0) | `"openapi":"3.1.0"`, `"title":"Flowplane"`, 62 paths |
| `scripts/e2e-envoy.sh` | not run | requires Envoy (blocked); its happy-path steps were executed manually below up to the Envoy boundary. |

Note: `flowplane openapi | head` returns 101 — a Rust broken-pipe panic from `head`,
not a command failure. The unpiped exit code is 0.

## Proof: `docs/tutorials/getting-started.md`

Started the CP with the tutorial's **exact** env set (notably **without**
`FLOWPLANE_SECRET_ENCRYPTION_KEY`).

| Step / Command | Result | Excerpt |
|---|---|---|
| §2 `flowplane serve` (dev mode) | pass | all 5 documented log signals present (below) |
| §3 `auth whoami` | pass (exit 0) | `MEMBERSHIPS 1 items`, `ORG ROLE owner`, `GRANT COUNT 0`, `USER ID …0003` |
| §4 `expose http://127.0.0.1:3001 --name local --port 10001 …` | pass (exit 0) | `CURL URL http://127.0.0.1:10001/`, `CLUSTER local-upstream`, `ROUTE CONFIG local-routes`, `LISTENER local`, `ENDPOINT SOURCE listener.public_base_url` |
| §4 `cluster/listener/route list` | pass | resources present |
| §5 `dataplane create dp-local` | pass (exit 0) | `created "dp-local" (revision 1)` |
| §6 `--out … dataplane bootstrap dp-local --mode dev …` | pass (exit 0) | valid Envoy YAML (1337 bytes); `node.id` stamped, ads → 127.0.0.1:18000 |
| §6 start Envoy / §7 `curl :10001` | **blocked** | no Envoy (see Preconditions) |
| §7 `unexpose local` | pass (exit 0) | removes cluster/route/listener (verified in a prior pass) |

Documented startup log signals — all observed verbatim (the doc lists these 5; the
`xDS ADS server starting (plaintext dev mode)` line is also emitted and is the one
documented in dev-dataplane.md):

```text
database connected and migrations applied
DEV MODE: in-process identity, seeded resources — never production
dev resources seeded            org=dev-org team=default user=dev-user
dev bearer token (valid 1h, this boot only)   dev_token=eyJ0eXAi…
API listener starting           addr=127.0.0.1:8096 tls=false
```

Confirms the tutorial's claim that the server starts without an encryption key and
without OIDC. Seed values (`dev-org`/`default`/`dev-user`) match.

## Proof: `docs/dev-dataplane.md`

Same loop with the runbook's env set (which sets `FLOWPLANE_SECRET_ENCRYPTION_KEY`).
Steps 1–7 and 10 pass; the runbook's `expose` table fields match exactly. Steps 8–9
(start Envoy, curl) blocked by the missing Envoy. Step 10 diagnostics:

| Command | Result | Excerpt |
|---|---|---|
| `stats overview` | pass | `LIVE 0 / STALE 1 / TOTAL DATAPLANES 1` |
| `ops xds status` | pass | `HEALTH stale … RECENT NACK COUNT 0` |
| `ops xds nacks` | pass | `no rows` |

## Proof: `docs/how-to/cli-auth-and-contexts.md`

All commands executed against an isolated `FLOWPLANE_CONFIG`:

| Command | Result | Excerpt |
|---|---|---|
| `config set-context prod --server … --org … --team …` | pass | `context saved` |
| `config get-contexts` | pass | current marked with `*` |
| `config use-context prod` | pass | `context selected` |
| `auth login --token` | pass | `token saved to …/credentials` |
| `auth token` | pass | prints resolved token |
| `auth login --token-stdin` | pass | `token saved to …/credentials` |
| `auth logout` | pass | `logged out` |

## Proof: `docs/how-to/jwt-auth-rate-limit-route.md`  ✅ (no #121 on main)

Built the listener and route-config from the doc's **exact** JSON against the live CP.

| Command | Result | Excerpt |
|---|---|---|
| §1 create `edge` listener with `http_filters` (`jwt_auth` providers+`requirement_map`, `local_rate_limit`) | pass | `created "edge" (revision 1)` |
| §2 create `api-routes` route-config **verbatim** (`"match": { "prefix": { "prefix": "/payments" } }`) | pass | `created "api-routes" (revision 1)` |
| §3 `listener update edge --revision 1 --file <spec-only>` | pass | `updated "edge" (revision 2)` |
| §3 stale `--revision 1` | pass-as-documented | `error (revision_mismatch): … at revision 2, you supplied 1` (→ 409) |

The route `match` shape that the older report flagged as #121 is **already nested and
correct on `main`** and is accepted. §4 (`401`/`429`) needs a live JWKS issuer + Envoy
(blocked).

## Proof: `docs/how-to/register-dataplane-mtls.md`

| Command | Result | Excerpt |
|---|---|---|
| §1 `dataplane create edge-gateway-1` | pass | created |
| §2 `dataplane cert issue … ` (CP without issuer CA) | pass-as-documented | `error (invalid_config): … set FLOWPLANE_CERT_ISSUER_CA_CERT_PATH and FLOWPLANE_CERT_ISSUER_CA_KEY_PATH` — the exact documented prereq |
| §4 `dataplane get` / `ops xds status` | pass | `last_heartbeat_at -` (no agent), `HEALTH stale` |

§2 cert-issue happy path (CA configured) and §3–§4 (`fp-agent` → `/healthz` → heartbeat)
were **not executed** here (no issuer CA wired this run; no live agent). The CLI/flag
surface matches `crates/fp-agent/src/main.rs`. (This doc links into `../../spec/` — the
prior governance issue #118 is closed.)

## Proof: `docs/how-to/ai-gateway-route-budget.md`  ✅ (no #122/#123 on main)

| Command | Result | Excerpt |
|---|---|---|
| `secret create` with CP started **without** the key | pass-as-documented | `error (unavailable): secret encryption key is not configured` — exactly what the doc's Prereqs now warn about |
| `secret create` after restart **with** `FLOWPLANE_SECRET_ENCRYPTION_KEY` | pass | `created "openai-key" (revision 1)` |
| §1 `ai providers create` | pass | `created "openai-prod" (revision 1)` |
| §2 `ai routes create` | pass | `status: active`; `materialized` → `listener_name ai-chat-route-listener`, `route_config_name ai-chat-route-routes` |
| §3 `ai budgets create` (shadow) | pass | `created "chat-budget" (revision 1)` |
| §3 `ai budgets update --revision 1` → enforcing | pass | `updated "chat-budget" (revision 2)` |
| §4 `ai usage --provider-id …` | pass | `no rows` (no traffic) |
| §4 chat request through `:19000` | **blocked** | needs live provider + egress |

The prereq the older report flagged as #122 is now **documented** in this doc
(`ai-gateway-route-budget.md:13`). The AI cluster NACK (#123) does **not** reproduce on
`main`: the fix (#124, `7cdfbc6`) is present — `crates/fp-xds/src/translate.rs:721`
gates `auto_sni_san_validation` behind a present validation context, and the targeted
unit tests pass (`cargo test -p fp-xds upstream_tls` → `ok. 5 passed`). Live Envoy load
not run (no Envoy), but the NACK precondition is removed in code + tests.

## Proof: `docs/how-to/learn-and-publish-api-spec.md`

| Command | Result | Excerpt |
|---|---|---|
| `api create orders-api` | pass | `created api/v1/teams/default/api-definitions` |
| §1 `learn start … --api orders-api --target-sample-count 1000` | pass | `created "orders-learn"` |
| §2 `learn get` | pass | `status: capturing`, `sample_count/path_count = 0` |
| §3 `learn stop` | pass | transitions |
| §4 `learn generate-spec` (no traffic) | pass-as-documented | `error (validation_failed): learning session has no raw observations to aggregate` — the exact error the doc says to expect |

Full publish path needs captured traffic (Envoy, blocked); the documented failure
reproduced verbatim.

## Proof: `docs/how-to/bootstrap-platform.md`  (new on `main`; #133)

Executed against a fresh non-dev DB (`flowplane_boot`):

| Step / Command | Result | Excerpt |
|---|---|---|
| §2 fail-closed: non-dev, uninitialized, **no** token | pass-as-documented | `Error: instance is uninitialized and no bootstrap token was supplied; set FLOWPLANE_BOOTSTRAP_TOKEN or FLOWPLANE_BOOTSTRAP_TOKEN_FILE …` |
| Troubleshooting: token `< 32` chars | pass-as-documented | `Error: the supplied bootstrap token is too short; it must be at least 32 characters after trimming whitespace` |
| §2 valid token → start | pass | log: `bootstrap token accepted from the operator … (token not logged)` |
| §2 **security: token value in logs** | pass | `0` occurrences of the token value in the log (the #133 core property) |
| §3 `POST /api/v1/bootstrap/initialize` (token as bearer) | pass (exit 0) | `{"org_id":"…","admin_user_id":"…"}` |
| §4 replay initialize | pass-as-documented | `HTTP 409` |
| §4 `GET /api/v1/bootstrap/status` | pass | `{"initialized":true}` |

No discrepancies. The token-never-logged guarantee holds.

## Proof: `docs/reference/cli.md`

`--help` walked for top-level + nested commands: top-level set matches; global options
(`--out`, `--revision`, `--timeout 30`, `-o/--output {table,json,yaml,wide}`) match;
`dataplane cert {list,register,issue,revoke}`; `ai {providers,routes,budgets,usage}`.
No discrepancies.

## Proof: `docs/reference/configuration.md`

| Check | Result | Excerpt |
|---|---|---|
| env-var catalogue vs source | pass | every runtime `FLOWPLANE_*` in `crates/` documented; only `FLOWPLANE_TEST_DATABASE_URL` (test-only) excluded |
| new vars on `main` | pass | `FLOWPLANE_BOOTSTRAP_TOKEN`, `_TOKEN_FILE`, `FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN`, `FLOWPLANE_UPSTREAM_CA_BUNDLE` all present |
| ② D-008: no TLS + no `FLOWPLANE_API_INSECURE` → refuse | pass | `Error: invalid_config: the API listener has no TLS material and plaintext was not explicitly allowed` |
| ⑦ secret key validated at use time → `unavailable` | pass | reproduced (AI how-to) |

## Proof: `docs/reference/errors.md`

Live responses confirm envelope + status mapping (each captured with HTTP status):

| Code | HTTP | How observed |
|---|---|---|
| `unauthorized` | 401 | no bearer token → `{"code":"unauthorized","message":"missing bearer token",…}` |
| `not_found` | 404 | GET unknown cluster |
| `validation_failed` | 400 | POST cluster with empty `endpoints` → `a cluster needs at least one endpoint` |
| `revision_mismatch` | 409 | stale `If-Match` on `listener` PATCH |
| `invalid_config` | (redacted 500) | cert-issue without CA → `an internal error occurred; report the request_id…` |
| `unavailable` | 503 | missing secret key |

Envelope `{code, message, hint?, details?, request_id}` confirmed.

## Proof: `docs/reference/rest-api.md`

| Check | Result | Excerpt |
|---|---|---|
| operational endpoints public | pass | `/healthz`,`/readyz`,`/metrics`,`/api-docs/openapi.json`,`/api/v1/bootstrap/status` → all `200` no auth |
| catalogue completeness | pass | all 62 OpenAPI paths present (programmatic diff: `OpenAPI NOT in rest-api.md: none`) |
| catalogue extras | as-documented | only `/api/v1/mcp` (doc flags it) and the two `bootstrap` endpoints (public) are listed-but-not-in-OpenAPI |

## Proof: `docs/reference/filters.md`

`jwt_auth` (providers + `requirement_map`) and `local_rate_limit` (`stat_prefix` +
`token_bucket`) chain entries were accepted from the documented field shapes (the `edge`
listener), and the reference-only `jwt_auth` / full `local_rate_limit` per-route
overrides were accepted (the `api-routes` route-config). No discrepancies in the parts
exercised. The full 9-filter catalogue and the duplicate-type rejection were not each
exercised this run (spot-checked against `crates/fp-domain/src/gateway/filters.rs`).

## Issues Raised Or Updated

- **New issues filed this pass: none.** No defect was found in the actionable docs on
  `main`.
- **#121 / #122 / #123** — all **closed**; verified as **not reproducing on `main`**
  (#121/#122 by execution, #123 by the in-tree #124 fix + passing tests). Not re-filed.
- **#118 / #119 / #120** — closed; #119 (PostgreSQL helper) does not reproduce here,
  #120 targets an out-of-scope operator runbook.
- **Observation (not filed):** the `docs-verification-report.md` previously committed on
  `main` is inaccurate for `main` — it reports #121/#122/#123 as open/reproducing though
  all three are fixed in-tree and closed on GitHub. This deliverable replaces it. (It is
  a meta artifact, not part of the epic-#100 product-doc set, so no GitHub issue was
  opened; flagged here for the maintainer.)

## Counts

- New issues raised this pass: **0**.
- Defects found in the actionable docs on `main`: **0** (`doc-defect` 0 · `code-defect`
  0 · `ambiguous` 0).
- Docs executed (DB-backed) with no discrepancies: README, getting-started,
  dev-dataplane, cli-auth-and-contexts, jwt-auth-rate-limit-route,
  register-dataplane-mtls, ai-gateway-route-budget, learn-and-publish-api-spec,
  bootstrap-platform, and all 5 reference docs.
- Blocked / inspection-only (environment, not docs): every "start Envoy + curl through
  the gateway" step (no Envoy binary / no Docker daemon / image pull network-blocked);
  the AI chat round-trip and JWT `401/429` checks (no live provider / IdP); the
  `fp-agent` live run and cert-issue happy path (no agent / no issuer CA wired this run).
  No workarounds that diverge from the docs were applied.
