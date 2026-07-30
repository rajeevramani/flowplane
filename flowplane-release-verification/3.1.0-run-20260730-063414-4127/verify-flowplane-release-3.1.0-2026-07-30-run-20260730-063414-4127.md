# Flowplane Release Verification — v3.1.0

**Verdict: NO-GO** (two confirmed documentation/manifest defects; all behavioral, security, integration, identity, and coverage checks passed)

- Run ID: `run-20260730-063414-4127`
- Date: 2026-07-30
- Gate: local separate-instance integration (NOT a production/cloud certification)
- Executed platform: **linux/amd64** only

---

## 1. Release & documentation identity

| Field | Value |
|---|---|
| Version selection | No argument supplied → resolved latest non-draft/non-prerelease GitHub Release = **v3.1.0** |
| Requested VERSION | 3.1.0 |
| Documentation tag | v3.1.0 |
| Annotated tag object SHA | `85bd8cd192642e17b88888fe10466cb72d46fa8d` |
| Resolved tag commit (preflight) | `a9ea214fae7dda12aa79554942129b800d9a6fe3` |
| Resolved tag commit (verdict re-check) | `a9ea214fae7dda12aa79554942129b800d9a6fe3` — **stable, unmoved** |
| Docs archive URL | https://github.com/rajeevramani/flowplane/archive/refs/tags/v3.1.0.tar.gz |
| Docs archive SHA-256 | `85613d7b44119495bfc9d06a558d1e609eb8be22abaac09f354b7e390aeb30c1` (unchanged at cleanup) |
| Host | Linux 6.18.5 x86_64; Docker Engine 29.3.1; Compose v5.1.1 |

## 2. GitHub Release asset inventory, checksums, binary versions

All asset sizes matched the GitHub API; the amd64 tarball and its SBOM verified against `SHA256SUMS` (`sha256sum -c` → **OK**).

| Asset | Size | SHA-256 vs SHA256SUMS |
|---|---|---|
| compose.eval.yml | 18235 | identical across release-asset / raw.githubusercontent / archive (`ebb8be93…`) |
| flowplane-3.1.0-linux-amd64.tar.gz | 17872095 | `4a12db0e…5597ff95` → **OK** |
| flowplane-3.1.0-linux-amd64.cargo-metadata.sbom.json | 2066821 | `350bc9d8…` → **OK** |
| flowplane-3.1.0-linux-arm64.tar.gz | 17103532 | present (not host-arch; download/verify only) |
| flowplane-3.1.0-linux-arm64.cargo-metadata.sbom.json | 2066821 | present |
| SHA256SUMS | 440 | — |

**Binary tarball (amd64, checksum-verified, extracted to scratch, scratch `bin/` on PATH):**
- Contents: `bin/flowplane`, `bin/flowplane-agent`, `bin/flowplane-rls`, `bin/fp-agent` (symlink → flowplane-agent), `dataplane/`, `release-manifest.md`
- `flowplane --version` → **flowplane 3.1.0**; `flowplane-agent --version` → **flowplane-agent 3.1.0**
- `flowplane --help` shows full command surface; `flowplane openapi` serves OpenAPI **3.1.0**, info.version **3.1.0**
- `flowplane-rls` honors no `--version`/`--help`/`-h` — always proceeds to startup and hits its loopback plaintext-gRPC guard (executable present and runs). No doc claims `flowplane-rls --help/--version` works, so this is not a defect (behavioral note only).
- **F-002:** `release-manifest.md` names the SBOM `flowplane-3.1.0.cargo-metadata.sbom.json`, which is not a published asset (assets are arch-suffixed). Filed as issue #240.

## 3. Image identities (pinned digests; all runtime tests used immutable `@sha256` references)

| Image | Requested tag | Immutable digest | Reported version | Index platforms |
|---|---|---|---|---|
| eval | ghcr.io/rajeevramani/flowplane:3.1.0-eval | `@sha256:62dbdb0d939fdf7fc4333b073cd06e0c83b99bb97bb54e33c9bead460e4cc768` | flowplane 3.1.0 | linux/amd64, linux/arm64 |
| hardened | ghcr.io/rajeevramani/flowplane:3.1.0 | `@sha256:0d902ee5ae66c4aeb6ae22413a5107995f37b0a40fc7d60cdc51b77b44d1e11b` | flowplane 3.1.0 | linux/amd64, linux/arm64 |

Packaging smokes (pinned eval digest): `flowplane-agent --help` PASS; `flowplane-rls` present and executes (both shipped executables present → not FAIL).

**Supporting images (environment provenance):** postgres:16 `@sha256:33f923b0…`; hashicorp/http-echo:1.0.0 `@sha256:fcb75f69…`; envoyproxy/envoy:v1.37-latest `@sha256:1c2b7977…`.

## 4. Host architecture & executed platform
Host linux/amd64; both Flowplane images executed as their linux/amd64 child manifest. Multi-arch indices inspected (compose composition only). **This report certifies linux/amd64 only** — arm64 behavior is not claimed (requires a native arm64 run).

## 5. Preflight — PASS
Docker daemon started from the pre-installed `dockerd` (root; not an install/reconfigure). GitHub API, raw.githubusercontent, GHCR, Docker Hub all reachable. Ports 8080/8081/10000/18000 free (host 5432 is the session Postgres; the bundle Postgres has no host port). Compose bundle declares Envoy (`envoyproxy/envoy:v1.37-latest`), Postgres, and http-echo; documented gateway port 127.0.0.1:10000. Tag→commit resolved; docs archive downloaded, hashed, and reconciled into a 34-page manifest.

## 6. Phase 1 — No-clone quick start — PASS (AS_WRITTEN)
- `compose.eval.yml` byte-identical across raw.githubusercontent (documented download), release asset, and source archive.
- `FLOWPLANE_EVAL_IMAGE=<eval digest> docker compose -f compose.eval.yml up -d --no-build`: all one-shots exit 0; CP/dashboard/Postgres healthy.
- `curl http://127.0.0.1:10000/` → **HTTP 200**, body `hello from the flowplane eval demo upstream`, `server: envoy` + `x-envoy-upstream-service-time` (through-gateway to demo upstream proven).
- `auth whoami` authenticates (dev-org owner); `cluster/listener/route list` show the seeded demo resources.
- `api create catalog --from-openapi` → spec v1, tool_count 1; `api spec publish catalog 1` → published; `api status` → published_spec_version_id set, tool_count 1; `mcp status` → dynamic_enabled_tool_count 1.
- `dp-eval` live (heartbeat advancing, total_errors 0).

## 7. Phase 2 — Dashboard release acceptance — PASS (12/12)
Starts (healthy); `/shared/dashboard-url` created; URL is loopback + 128-bit per-launch nonce; no-nonce/wrong-nonce → **404**; Overview renders (`<title>Flowplane — default</title>`); Overview dataplane total = 1; `dp-eval` **Live** and telemetry advances (Requests 11→43); Overview polls `hx-trigger="load, every 10s"` (matches doc); non-Overview panels `hx-trigger="…once"` (on-demand); all seven screens reachable (Overview at nonce-root; Resources/APIs/Learning/AI/MCP/Operations → 200); off-loopback container bind still rejects foreign `Host: evil.com` → **403** and `Origin: http://evil.com` → **403** (correct Host → 200); dev token (621 chars) **absent** from Overview HTML.

## 8. Core documented user-journey results (Phase 3)
| Journey | Artifact path | Result |
|---|---|---|
| README no-clone quick start | pinned eval image | PASS |
| evaluate-no-clone.md | pinned eval image | PASS |
| view-team-dashboard.md | pinned eval image | PASS |
| register-dataplane-mtls.md | pinned eval image (bundle executes the documented `dataplane create`→`cert issue`→`bootstrap --mode mtls`→`flowplane-agent` flow) | PASS — verify step: `stats overview` live=1/stale=0/errors=0; `ops xds status` health=healthy, config_verified=1, live=true, 0 NACK |
| production-readiness.md (binary acquisition) | checksum-verified amd64 archive on scratch PATH | PASS — `flowplane 3.1.0`, full CLI surface, serves OpenAPI 3.1.0 |
| production-readiness.md (split-component) | separate CP + standalone RLS + Envoy + agent (Phase 4/5) | PASS |
| import-and-publish (base import→publish→tools) | pinned eval image (Phase 1) | PASS |

## 9. Separate-instance topology & endpoint map (Phase 4) — PASS
Harness: release `compose.eval.yml` **unmodified** + scratch `override.rls.yml` adding a standalone `flowplane-rls` over the documented split-node mTLS path (single private CA, three EC leaf certs, SAN `DNS:rls`). Six distinct long-running instances, all distinct container IDs/PIDs:

| Instance | Container id | Role |
|---|---|---|
| postgres | (bundle) | source of truth |
| flowplane-eval | 6714dc… (pid 12948) | control plane (`serve`) |
| rls | f1eae5… (pid 12434) | standalone `flowplane-rls` (mTLS gRPC + HTTPS+bearer admin) |
| envoy | c057b2… (pid 13392) | dataplane |
| flowplane-agent | f155d4… (pid 13552) | telemetry relay (shares Envoy netns; separate container) |
| demo-upstream | (bundle) | deterministic upstream |

Endpoint evidence:
1. CP `/healthz` 200 `{status ok, version 3.1.0}`; `/readyz` 200 (database ok, xds_outbox_consumer ok).
2. RLS admin over HTTPS with the private CA: `/healthz` 200, `/readyz` 200; plaintext HTTP → connection reset (HTTPS-only). RLS log: `grpc_security="mtls" admin_security="https + bearer"`.
3. Envoy xDS: `/ready` LIVE; `cds.update_success=1`/rejected 0, `lds.update_success=1`/rejected 0.
4. Agent reports to CP; `dp-eval` live with advancing heartbeat.
5. CP→RLS: `applied CP rate-limit policy push`; CP log `rate_limit_cluster will be injected into CDS, mtls:true` + `rls_sync worker started, reconcile_secs 60, authenticated:true`.
6. Envoy→RLS: `rate_limit_cluster` injected with `envoy.transport_sockets.tls`; `ssl.handshake=4`, `upstream_rq_total=331`.
7. Config mutation via CLI (`route update` + `listener update`, `--revision`) propagated to Envoy with **zero restarts** (all restartCount=0); subsequent gateway request observed the change.

## 10. RLS policy & enforcement (Phase 5) — PASS (AS_WRITTEN §6 exact)
- Domain `checkout` + policy `per-client` (descriptors `{api_key:acme}`, 100/min) created.
- Route descriptor (`api_key` from `x-api-key`) + `global_rate_limit` filter attached.
- CP→RLS sync: `applied CP rate-limit policy push policies=1`.
- Envoy composed tenant-namespaced domain: `16d8d7aa-7842-8166-88ac-c72392a9d771|04c9f1e3-3f4f-8e5d-8f75-5c5bb671fd58|checkout`.
- Enforcement: 101 requests `x-api-key: acme` → **exactly 100×200 + 1×429** (matches doc). A different value has an independent counter (correct — the policy matches the exact descriptor value `acme`).
- §9 propagation: policy limit 100→200→5 via revision-checked `policy update`; after reconcile, `acme` at limit 5 → **exactly 5×200 + 7×429**; enforcement changed with **zero component restarts**.
- Three-boundary attribution: CP (rls_sync + push), Envoy (`ratelimit.over_limit=8`, mTLS to `rate_limit_cluster`), RLS (331 gRPC decisions) — the 429s are attributable to the standalone RLS.
- Note (minor/ambiguous, not filed): the guide's framing "100 requests/minute per distinct api_key" / "a different value has its own counter" reads as per-value limiting, but the example policy pins the exact value `acme` (only `acme` is limited); mitigated by the doc's own line 190–191 ("the exact set the route must emit for this policy to match"). The executable §6 verify passes exactly.

## 11. Focused failure & recovery (Phase 6) — PASS
- **RLS interruption (fail-open, `failure_mode_deny:false`):** steady 5×200+5×429 → stop RLS → 20×200 (fail open) → restart RLS → CP repush → recovered 5×200+5×429; CP/Envoy/agent restartCount=0.
- **RLS fail-closed (`failure_mode_deny:true`):** stop RLS → 6×500 (reject). Both documented fail modes observed.
- **CP interruption:** steady gateway 200 + agent live → stop CP → existing Envoy traffic 6×200 (last-good) → restart CP (same Postgres, healthy in ~4s) → **Envoy auto-reconnected** (a new `expose demo2` reached Envoy: listener :10002 in config_dump + curl returns upstream body). The **flowplane-agent process exited (code 1)** when the CP diagnostics stream dropped and did **not** self-reconnect; the eval bundle sets no restart policy. Docs make no agent auto-reconnect promise (production-readiness frames process replacement as external/rolling-restart). Restarting the agent (the documented supervised model) reconnected cleanly (fresh heartbeat, live, healthy). Reported transparently as an operational characteristic, not a documentation divergence.

## 12. Focused guards (Phase 7) — all PASS (fail closed)
1. Hardened image + reachable DB + `FLOWPLANE_DEV_MODE=true` → `built without the dev-oidc feature (release artifact)`, exit 1 (never serves dev mode).
2. RLS non-loopback plaintext gRPC bind → refuse (needs TLS triad or loopback), exit 1.
3. RLS partial gRPC TLS triad → refuse (all-or-none), exit 1.
4. RLS non-loopback admin without TLS/token → refuse; admin TLS without credential → refuse, exit 1.
5. CP RLS admin token + non-loopback plaintext admin URL → `the bearer would cross a plaintext channel`, exit 1.

No guard failed open.

**CLI/REST reconciliation (core journeys):** `flowplane openapi` served by the running CP is OpenAPI 3.1.0 / info.version 3.1.0 with 77 paths; all core-journey REST surfaces present (`rate-limit-domains`/policies/override, `proxy-certificates`/issue/revoke, `dataplanes`, `listeners`, `route-configs`, `clusters`, `api-definitions`/specs/publish, `mcp/status`, `stats/overview`, `xds/status`). `--help` for the `route`/`listener`/`rate-limit` groups matched the documented update/`--revision` contract.

## 13. Documentation coverage reconciliation (34/34 pages bucketed)
- **CORE_EXECUTED (7):** README.md; tutorials/evaluate-no-clone.md; how-to/view-team-dashboard.md; how-to/register-dataplane-mtls.md; how-to/global-rate-limit.md; how-to/production-readiness.md; how-to/import-and-publish-openapi-spec.md
- **FOCUSED_EXECUTED (4):** reference/cli.md; reference/rest-api.md; reference/filters.md (global_rate_limit executed; **F-001 broken link found**); reference/configuration.md
- **STATIC_REVIEWED (22):** docs/README.md; reference/{adoption-evaluation-issue-map, errors, observability-alerts}; concepts/{cli-contract, global-rate-limiting, tenancy-grants-xds}; tutorials/getting-started.md; how-to/{cli-auth-and-contexts, script-the-cli, learn-and-publish-api-spec, jwt-auth-rate-limit-route, ai-gateway-route-budget, trace-ai-requests, bootstrap-platform, configure-oidc-provider, create-tenant-org-and-team, manage-users-teams-and-grants, dataplane-egress-security, evaluate-platform, secret-kek-rotation, onboard-api-team}
- **NOT_APPLICABLE (1):** how-to/aws-secure-deployment.md (cloud deploy outside this local gate; statically link/flag-reviewed, no defects)
- **BLOCKED (0).** No unbucketed pages.

Static review (3 parallel reviewers over all 34 pages): all internal links resolve; no current-release download/selection version strings other than 3.1.0; no port/env-var/image/CLI contradictions — **except F-001**. Version labels "1.1.0 topology" (concepts/global-rate-limiting) and "deferred for v1.0" (reference/observability-alerts) are historical/roadmap semantics, not current download instructions → not defects. A cli.md/cli-contract.md "any 5xx retryable" vs errors.md per-code note is a plausible two-layer distinction (CLI retry heuristic vs API hint) → recorded as ambiguous, not filed.

## 14. Explicitly excluded scope
AWS/Kubernetes/other-cloud deployment, cloud IAM, managed databases/load balancers/certificates, autoscaling, multi-region/DR, production capacity/availability, and a real external OIDC/IdP were **not executed** — reviewed statically only. Tested topology is **local separate-instance integration**, not a production/cloud certification. arm64 runtime behavior not certified (amd64 host only).

## 15. Ranked findings
| ID | Severity (rank) | Artifact | Location | Summary | Corrected diag |
|---|---|---|---|---|---|
| F-001 | Static documentation defect (#9) | docs archive `85613d7b…` (STATIC) | docs/reference/filters.md:241 | Dangling Markdown link `[filters reference / reserved names]` (no target/definition) renders as literal text | N/A (static) |
| F-002 | Published artifact/manifest defect (#5, low) | tarball `4a12db0e…` (release-manifest.md) | release-manifest.md | SBOM named `flowplane-3.1.0.cargo-metadata.sbom.json`; published SBOMs are arch-suffixed (no such asset) | N/A |

No security-severity finding. No behavioral/integration/RLS/recovery divergence. No identity/coverage failure.

## 16. GitHub issue publication
| Finding | State | Issue |
|---|---|---|
| F-001 | CREATED | https://github.com/rajeevramani/flowplane/issues/239 (labels: documentation, docs-verification) — read-back verified |
| F-002 | CREATED | https://github.com/rajeevramani/flowplane/issues/240 (labels: documentation, docs-verification) — read-back verified |
Dedup search found no existing open/closed issue for either root cause. Issues use neutral maintainer voice, no attribution, redaction-scanned before creation.

## 17. Cleanup
See the machine-readable JSON and the "Cleanup" note appended at run end. Only run-owned resources (`fpverify_run_20260730_063414_4127` project: containers, volumes, network) were removed; pulled images retained; the scratch directory removed after durable artifacts were copied and verified. Docs archive and binary tarball hashes unchanged from preflight.

## 18. Verdict
All mandatory **behavioral** release checks passed: no-clone quick start, dashboard acceptance, separate-instance CP/agent/RLS/Envoy integration, standalone-RLS 200/429 enforcement and propagation, RLS + CP recovery drills, and every fail-closed guard. Identity is consistent and stable (tag→commit unmoved; both images report 3.1.0; checksums verified). Coverage is complete (34/34 pages bucketed). **However**, two confirmed documentation/manifest defects were found (F-001 broken reference link in the shipped docs; F-002 SBOM name mismatch in the shipped tarball manifest). Under this gate a confirmed documentation defect is a failure, so the release cannot receive `GO`.

First failing GO criterion: **#17 "no FAIL result exists"** — a confirmed static documentation defect (F-001) exists.

`GATE: NO-GO — a confirmed documentation defect exists (F-001, docs/reference/filters.md:241 broken link; also F-002 release-manifest SBOM name), so GO criterion #17 "no FAIL result exists" is not met`
