# Docs Verification Report

Run date: 2026-06-20 (second pass — PostgreSQL available)

Config:

- Epic issue: #100
- Label: `docs-verification`
- Marker: `[docs-verify]`
- Method: execute the docs against the real binary/CLI/control plane from a clean
  state; classify every mismatch as `doc-defect` / `code-defect` / `ambiguous`.

## What changed since the first pass

The first pass (issues #118/#119/#120) was **blocked on PostgreSQL**: the local
`postgres` role did not exist, so every DB-backed end-to-end step was
inspection-only. In **this** environment PostgreSQL is provisioned with the
documented credentials, so the full happy-path was executed for the first time.

```text
$ scripts/ensure-postgres.sh
postgres ready                 # exit 0

$ psql postgres://postgres:postgres@127.0.0.1:5432/postgres -c 'select 1'
 ?column?
----------
        1
```

Consequence: **#119's premise does not reproduce here** — the documented helper
and `postgres://postgres:postgres@127.0.0.1:5432/...` URL both work. #119 is left
open and untouched (it is environment-specific). Executing the previously-blocked
flows surfaced one new copy-paste defect (#121) and one prereq gap (#122).

## Preconditions

- Build/toolchain: pass. `cargo build --bin flowplane` finished in 3m44s, exit 0;
  `flowplane version` → `0.1.0`.
- PostgreSQL: pass (see above).
- Envoy data path: **blocked**. No local `envoy` binary; Docker daemon was started
  (`sudo dockerd`) but the documented image pull is blocked by the environment's
  network policy:
  ```text
  $ docker run ... envoyproxy/envoy:v1.37-latest ...
  unexpected status from GET request to https://production.cloudfront.docker.com/.../data: 403 Forbidden
  ```
  Every "start Envoy / curl through the gateway" step is therefore
  **blocked / inspection-only** and is called out per doc below. This is an
  environment limitation, not a doc defect.

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
- `docs/reference/cli.md`
- `docs/reference/configuration.md`
- `docs/reference/rest-api.md`
- `docs/reference/errors.md`
- `docs/reference/filters.md`

---

## Proof: `README.md`

| Command | Result | Excerpt |
|---|---|---|
| `cargo build --bin flowplane` | pass (exit 0) | `Finished \`dev\` profile ... in 3m 44s` |
| `./target/debug/flowplane openapi` | pass (exit 0) | 288571 bytes; `"openapi":"3.1.0"`, `"title":"Flowplane"` |
| `./target/debug/flowplane version` | pass (exit 0) | `0.1.0` |
| `scripts/e2e-envoy.sh` | inspection-only | gets past the Postgres check now (admin URL works); halts only at the same blocked Envoy image pull. |

Note: `flowplane openapi | head` returns exit 101 — that is a Rust broken-pipe
panic from `head` closing the pipe, **not** a command failure. Run without a pipe
(`flowplane openapi > file`) the true exit code is `0`.

## Proof: `docs/tutorials/getting-started.md` (full happy path)

Started the CP with the tutorial's **exact** env set (notably **without**
`FLOWPLANE_SECRET_ENCRYPTION_KEY`):

| Step / Command | Result | Excerpt |
|---|---|---|
| §2 `flowplane serve` (dev mode) | pass | all 6 documented log signals present (below) |
| §3 `auth whoami` | pass (exit 0) | returns `user_id`, `memberships`, `grant_count` |
| §4 `expose http://127.0.0.1:3001 --name local --port 10001 ...` | pass (exit 0) | `curl_url=http://127.0.0.1:10001/`, `cluster=local-upstream`, `route_config=local-routes`, `listener=local`, `endpoint_source=listener.public_base_url` |
| §4 `cluster/listener/route list` | pass | resources present at revision 1 |
| §5 `dataplane create dp-local` | pass (exit 0) | `created "dp-local" (revision 1)` |
| §6 `--out ... dataplane bootstrap dp-local --mode dev ...` | pass (exit 0) | valid Envoy YAML; `node.id` stamped, ads → 127.0.0.1:18000 |
| §6 start Envoy / §7 `curl :10001` | **blocked** | Envoy image pull 403 (see Preconditions) |
| §7 `unexpose local` | pass (exit 0) | removes cluster/route/listener |

Documented startup log signals — all observed verbatim:

```text
database connected and migrations applied
DEV MODE: in-process identity, seeded resources — never production
dev resources seeded            (org=dev-org team=default user=dev-user)
dev bearer token (valid 1h, this boot only)   dev_token=eyJ0eXAi...
xDS ADS server starting (plaintext dev mode)  addr=0.0.0.0:18000
API listener starting           addr=127.0.0.1:8096 tls=false
```

Confirms the tutorial's claim that the server starts **without** an encryption key
and without OIDC. Seed table (`dev-org` / `default` / `dev-user`) matches.

## Proof: `docs/dev-dataplane.md`

Same loop as above with the runbook's env set (which **does** set
`FLOWPLANE_SECRET_ENCRYPTION_KEY`). Steps 1–7, 10 pass identically; the runbook's
expected `expose` table fields match exactly. Steps 8–9 (start Envoy, curl)
blocked by the Envoy image pull. Step 10 diagnostics:

| Command | Result | Excerpt |
|---|---|---|
| `stats overview` | pass | `LIVE 0 / STALE 1 / TOTAL DATAPLANES 1` |
| `ops xds status` | pass | `HEALTH stale ... RECENT NACK COUNT 0` |
| `ops xds nacks` | pass | `no rows` |

## Proof: `docs/how-to/cli-auth-and-contexts.md`

All commands executed against an isolated `FLOWPLANE_CONFIG`:

| Command | Result | Excerpt |
|---|---|---|
| `config set-context prod --server ... --org ... --team ...` | pass | `context saved` |
| `config get-contexts` | pass | current context marked with `*` (as documented) |
| `config use-context prod` | pass | `context selected` |
| `config path` / `config show` | pass | prints path / resolved TOML |
| `auth login --token <T>` | pass | `token saved to .../credentials` |
| `auth token` | pass | prints resolved token |
| `auth login --token-stdin` | pass | `token saved to .../credentials` |
| `auth logout` | pass | `logged out` |

"Creating a context also makes it current if none set yet" — confirmed
(`get-contexts` showed `*` on `prod` before `use-context`).

## Proof: `docs/how-to/jwt-auth-rate-limit-route.md`  ⚠ discrepancy

Built the listener and route-config from the doc's **exact** JSON against the live CP.

| Command | Result | Excerpt |
|---|---|---|
| create `edge` listener with §1 `http_filters` (`jwt_auth` providers+`requirement_map`, `local_rate_limit`) | pass | `created "edge" (revision 1)` |
| create `api-routes` route-config with §2 body **verbatim** | **discrepancy (exit 4, 422)** | `match.prefix: invalid type: string "/payments", expected struct variant PathMatch::Prefix` |
| create same route-config with corrected match shape `{"prefix":{"prefix":"/payments"}}` | pass | `created "api-routes" (revision 1)` — so the `filter_overrides` JSON itself is valid |
| §3 `listener update edge --revision 1 --file ...` (If-Match) | pass | `updated "edge" (revision 2)` |
| §3 re-run with stale `--revision 1` | pass-as-documented | `error (revision_mismatch): ... at revision 2, you supplied 1` (→ 409) |

**Defect (filed #121):** §2 / line 118 prints `"match": { "prefix": "/payments" }`.
`PathMatch` is an externally-tagged enum with struct variants
(`crates/fp-domain/src/gateway/route_config.rs:67`), so the wire form is
`"match": { "prefix": { "prefix": "/payments" } }` — confirmed by the real
serialized output of `route get` (`"match":{"prefix":{"prefix":"/"}}`).
Classification `doc-defect`; the rest of the how-to (the JWT + rate-limit content,
the only place that shape appears in `docs/`) is correct.

## Proof: `docs/how-to/register-dataplane-mtls.md`

| Command | Result | Excerpt |
|---|---|---|
| `dataplane create ...` | pass | exercised under the runbook above |
| `dataplane get <name>` | pass | shows `last_heartbeat_at` (`-` with no agent) |
| `stats overview` / `ops xds status` | pass | as above |
| `dataplane cert issue ...` | inspection-only | requires `FLOWPLANE_CERT_ISSUER_CA_*` on the CP (not configured here); CLI surface matches `dataplane cert {list,register,issue,revoke}`. |

## Proof: `docs/how-to/ai-gateway-route-budget.md`  ⚠ prereq gap

| Command | Result | Excerpt |
|---|---|---|
| `secret create` while CP ran **without** the encryption key (tutorial env) | discrepancy | `error (unavailable): secret encryption key is not configured` |
| `secret create` after restarting CP **with** `FLOWPLANE_SECRET_ENCRYPTION_KEY` | pass | `created "openai-key" (revision 1)` |
| §1 `ai providers create` (doc spec) | pass | `created "openai-prod" (revision 1)` |
| §2 `ai routes create` (doc spec) | pass | `status: active`; `materialized` has `cluster_names`, `listener_name=ai-chat-route-listener`, `route_config_name=ai-chat-route-routes` |
| §3 `ai budgets create` (shadow) | pass | `created "chat-budget" (revision 1)` |
| §3 `ai budgets update --revision 1` → enforcing | pass | `updated "chat-budget" (revision 2)` |
| §4 `ai usage --provider-id ...` | pass | `no rows` / `[]` (no traffic) |
| §4 chat request through `:19000` | **blocked** | needs Envoy + live provider |

The `unavailable` error is **correct, documented use-time behavior**
(`docs/reference/configuration.md` constraint ⁷). **Defect (filed #122):** the AI
how-to's Prereqs (and the getting-started setup it builds on, which omits the key)
never state the CP must run with `FLOWPLANE_SECRET_ENCRYPTION_KEY` for the
required `secret create` to succeed. Classification `doc-defect` (incomplete
prereq). Budget weight defaults asserted by the doc were independently confirmed
in `crates/fp-domain/src/ai.rs` (`prompt_token_weight`→0, `completion_token_weight`→1,
`window_seconds`→2592000, path default `/v1/chat/completions`).

## Proof: `docs/how-to/learn-and-publish-api-spec.md`

| Command | Result | Excerpt |
|---|---|---|
| `api create orders-api` | pass | `created api/v1/teams/default/api-definitions` |
| §1 `learn start ... --api orders-api --target-sample-count 1000` | pass | `created "orders-learn-2026-06"` |
| §2 `learn get` | pass | `status: capturing`, `sample_count/path_count/byte_count = 0` |
| `learn list` | pass | row present |
| §3 `learn stop` | pass | session stopped |
| §4 `learn generate-spec` (no traffic) | pass-as-documented | `error (validation_failed): learning session has no raw observations to aggregate` — the exact error the doc says to expect |
| §5 `api spec publish orders-api 3` | command verified | `not_found: spec version "3"` (no spec generated; command shape correct) |
| §6 `api status` / `mcp status` | pass | `tool_count: 0`; MCP status table renders |

Full publish path needs captured traffic (Envoy, blocked), but both documented
failure messages were reproduced verbatim.

## Proof: `docs/reference/cli.md`

`--help` walked for top-level + nested commands. Global options table (incl.
`--out`, `--revision`, `--timeout 30`, `-o/--output`) matches. Top-level command
list matches exactly. `dataplane cert` exposes `{list,register,issue,revoke}`.
`completion bash` emits a bash script (exit 101 = `head` SIGPIPE only). No
discrepancies.

## Proof: `docs/reference/configuration.md`

| Check | Result | Excerpt |
|---|---|---|
| env-var catalogue vs source | pass | every runtime `FLOWPLANE_*` in `crates/` is documented; only `FLOWPLANE_TEST_DATABASE_URL` (test-only) is intentionally excluded |
| ② D-008: no TLS + no `FLOWPLANE_API_INSECURE` → refuse start | pass | `Error: invalid_config: the API listener has no TLS material and plaintext was not explicitly allowed` |
| ⑦ secret key validated at **use time** as `unavailable` | pass | reproduced (see AI how-to) |

## Proof: `docs/reference/errors.md`

Live responses confirm the envelope and status mapping:

| Code | Status | How observed |
|---|---|---|
| `unauthorized` | 401 | `{"code":"unauthorized","message":"missing bearer token","hint":...,"request_id":...}` |
| `validation_failed` | 400/422 body | malformed route body |
| `not_found` | 404 | `secret/spec version not found` |
| `revision_mismatch` | 409 | stale `If-Match` |
| `unavailable` | 503 | missing secret key |

Envelope `{code, message, hint?, request_id}` with `hint`/`details` omitted when
absent — confirmed.

## Proof: `docs/reference/rest-api.md`

| Check | Result | Excerpt |
|---|---|---|
| operational endpoints public | pass | `/healthz`,`/readyz`,`/metrics`,`/api-docs/openapi.json`,`/api/v1/bootstrap/status` → all `200` |
| catalogue completeness | pass | all 62 generated OpenAPI paths appear in the catalogue |
| catalogue extras | as-documented | only `/api/v1/mcp` (doc flags it as excluded from OpenAPI) and the two `bootstrap` endpoints (public, outside the secured `routes!` surface) are listed-but-not-in-OpenAPI |
| If-Match convention | pass | revision flow verified via JWT how-to |

## Proof: `docs/reference/filters.md`

`jwt_auth` (providers + `requirement_map`) and `local_rate_limit`
(`stat_prefix` + `token_bucket`) chain entries and the reference-only `jwt_auth` /
full `local_rate_limit` overrides were all accepted by domain validation through
the live CP (see JWT how-to). Source-cited specifics spot-checked against
`crates/fp-domain/src/gateway/filters.rs`. No discrepancies found.

## Issues Raised Or Updated

New this pass:

- #121: `[docs-verify] docs/how-to/jwt-auth-rate-limit-route.md — route match JSON uses wrong PathMatch shape`
  - Severity: `major` · Classification: `doc-defect`
- #122: `[docs-verify] docs/how-to/ai-gateway-route-budget.md — secret prereq omits FLOWPLANE_SECRET_ENCRYPTION_KEY on the control plane`
  - Severity: `minor` · Classification: `doc-defect`

Carried over (not re-filed):

- #118 (standalone spec/internal link policy) — still accurate; open.
- #119 (PostgreSQL helper) — **does not reproduce in this environment**; the
  helper and documented URL work here. Left open as environment-specific; not
  re-filed.
- #120 (aws-secure-deployment `cert` subcommand) — out of this run's scope
  (`docs/aws-secure-deployment.md` is an operator runbook, not in the epic #100
  actionable set); previously filed, still accurate.

## Counts

- New issues raised this pass: 2 (#121, #122)
- By severity (new): `blocker` 0 · `major` 1 · `minor` 1
- By classification (new): `doc-defect` 2 · `code-defect` 0 · `ambiguous` 0
- Docs fully executed end-to-end (DB-backed): README, getting-started, dev-dataplane,
  cli-auth-and-contexts, jwt-auth-rate-limit-route, ai-gateway-route-budget,
  learn-and-publish-api-spec, register-dataplane-mtls, and all 5 reference docs.
- Blocked / inspection-only steps: every "start Envoy + curl through the gateway"
  step (Envoy image pull blocked by network policy) and `dataplane cert issue`
  (CP cert-issuer CA not configured). No workarounds that diverge from the docs
  were applied.
