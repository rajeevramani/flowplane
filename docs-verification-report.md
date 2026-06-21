# Docs Verification Report — re-run on latest `main`

Run date: 2026-06-21 (re-run against `origin/main` @ `32ace95`, fresh branch `claude/docs-verify-rerun`)

Scope this run: **all how-to docs** under `docs/how-to/`, re-executed against the
real binary + PostgreSQL + Envoy 1.37.0. Method unchanged: execute the docs, classify
each mismatch as `doc-defect` / `code-defect` / `ambiguous`. GitHub via MCP (`gh` not
available).

## Headline: every previously-filed defect is fixed on `main`

The six docs-verification issues from the prior passes are all **CLOSED**, and this
re-run confirms the fixes by re-execution:

| Issue | Was | Re-run result |
|---|---|---|
| #120 | `aws-secure-deployment.md` used top-level `cert issue` | **fixed** — now `flowplane … dataplane cert issue edge-local` |
| #121 | jwt how-to §2 `match` had wrong PathMatch shape | **fixed** — doc's exact §2 body now nests `{"prefix":{"prefix":"/payments"}}`; accepted (`created "api-routes"`) |
| #122 | AI how-to omitted `FLOWPLANE_SECRET_ENCRYPTION_KEY` prereq | **fixed** — prereq line added; `secret create` works with the key set |
| #123 | AI cluster NACKed by Envoy (`auto_sni_san_validation` w/o validation context) | **fixed** — see below |

**#123 fix verified behaviorally with a live Envoy:** with verify-by-default
(`translate.rs` `upstream_tls_context`, `auto_sni_san_validation: has_validation && …`),
the AI route's cluster now loads. A plaintext Envoy attached to the dev CP and pulled the
snapshot with **0 NACKs this run**, and `ai-chat-route-b1` appears in Envoy's
`/clusters`; the data path returns `200`.

```text
$ curl -s -o /dev/null -w '%{http_code}' :10001/        -> 200
$ curl :9901/clusters | grep ai-chat                     -> ai-chat-route-b1::observability_name::ai-chat-route-b1
$ flowplane ops xds status                                -> RECENT NACK COUNT 0 (stale = no agent; old NACKs are from a prior run)
```

## Per-how-to results

| How-to | Result | Notes |
|---|---|---|
| `cli-auth-and-contexts.md` | pass | CLI surface (`auth`/`config`/contexts/precedence) matches `--help`; `whoami` works live |
| `jwt-auth-rate-limit-route.md` | pass | §1 chain, §2 (doc's exact body, #121 fixed), §3 If-Match (rev 1→2; stale→`revision_mismatch`). **§4 `401` confirmed live** (`/payments` no token → 401). `429` still needs a valid JWT (real JWKS) → inspection-only |
| `register-dataplane-mtls.md` | pass | §1 create, §2 `cert issue` returns `certificate_pem`/`private_key_pem`/`ca_certificate_pem` + SPIFFE `spiffe://flowplane.local/org/…/team/…/proxy/…`. Live `fp-agent` heartbeat (§3–§4) blocked by the sandbox (below) |
| `ai-gateway-route-budget.md` | pass | prereq (#122) + provider (TLS `base_url`) + route `status: active` + budget shadow→enforcing + usage. Cluster now loads in Envoy (#123 fixed). Live chat (§4) needs a real provider |
| `learn-and-publish-api-spec.md` | pass | api create → learn start → get (`capturing`) → stop → generate-spec → exact documented error `learning session has no raw observations to aggregate` |
| `secret-kek-rotation.md` | partial | CP **starts cleanly** with the full rotation env (active key + `…_KEY_ID` + retired `FLOWPLANE_SECRET_ENCRYPTION_KEYS` JSON) — env contract validated. The secret-rotate→new-key-id observation is **blocked** (requires a CP restart; see below) |
| `production-readiness.md` | pass + 1 finding | `flowplane db migrate` works with the doc's env block (TLS set) → `migrations applied`; `db migrate`/`serve`/`team list`/`dataplane list`/healthz shapes match. **New finding #132** (see below) |
| `aws-secure-deployment.md` | inspection | #120 cert command fixed; the rest is AWS infra (Terraform/ECS/RDS) — not runnable in this environment |

## New issue filed this run

- **#132** — `docs/reference/configuration.md` omits `FLOWPLANE_UPSTREAM_CA_BUNDLE` from
  its "every `FLOWPLANE_*` variable read by the control plane" catalogue, even though the
  control plane reads it (`crates/fp-xds/src/translate.rs:673`) and `production-readiness.md`
  documents it. Surfaced while testing the production-readiness how-to. Classification
  `doc-defect`, severity `minor`. (Introduced by the verify-by-default work, #125.)

## Blocked by the environment (not by the docs)

This sandbox SIGTERMs shell calls that keep a server child (Envoy/CP) alive more than a
few seconds, and makes CP **restarts** unreliable. Two checks could not be completed:

- `register-dataplane-mtls.md` §3–§4: live `fp-agent` `/healthz` + heartbeat advancing
  (needs a sustained Envoy admin + agent). The mTLS transport itself was verified in the
  prior pass; only the live heartbeat assertion is unverified.
- `secret-kek-rotation.md` step 6: observing `secret rotate` rewrite the ciphertext under
  the new `encryption_key_id` (needs the CP restarted under the rotated env, which the
  sandbox would not keep alive). The env contract (keyring accepted at startup) was
  verified.

Stated as blocked rather than assumed. No doc-diverging workarounds were applied.

## Counts

- How-to docs tested: 8 (all under `docs/how-to/`).
- Previously-filed defects re-verified as **fixed**: #120, #121, #122, #123.
- New issues this run: **1** (#132, `doc-defect` / `minor`).
- Blocked / inspection-only: live `fp-agent` heartbeat; KEK secret-rotate observation;
  AI live chat; jwt `429` (valid-JWT path); AWS infra.
