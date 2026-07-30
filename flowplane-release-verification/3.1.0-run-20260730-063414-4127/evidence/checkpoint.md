# Flowplane release verification — checkpoints

## Identity block
- VERSION: 3.1.0 (selected: no argument supplied → latest non-draft/non-prerelease GitHub Release = v3.1.0)
- Doc tag: v3.1.0
- Annotated tag object sha: 85bd8cd192642e17b88888fe10466cb72d46fa8d
- Resolved tag commit (preflight): a9ea214fae7dda12aa79554942129b800d9a6fe3
- Docs archive URL: https://github.com/rajeevramani/flowplane/archive/refs/tags/v3.1.0.tar.gz
- Docs archive SHA-256: 85613d7b44119495bfc9d06a558d1e609eb8be22abaac09f354b7e390aeb30c1
- Host: Linux 6.18.5 x86_64 → executed platform linux/amd64
- Runtime: Docker Engine 29.3.1, Compose v5.1.1 (daemon started from pre-installed dockerd; root)

## Images (pinned digests)
- eval requested tag: ghcr.io/rajeevramani/flowplane:3.1.0-eval
  - digest: ghcr.io/rajeevramani/flowplane@sha256:62dbdb0d939fdf7fc4333b073cd06e0c83b99bb97bb54e33c9bead460e4cc768
  - reported version: flowplane 3.1.0
  - index platforms: linux/amd64 (sha256:bfc7a8de7f5ee73d5933c80a1190e9e08875145b11558124033b18e78b8b660e), linux/arm64 (sha256:e8378970721937b095bcd4fbb314341e94dd57b8f2c57dcb7ee6335a4318e2fc)
- hardened requested tag: ghcr.io/rajeevramani/flowplane:3.1.0
  - digest: ghcr.io/rajeevramani/flowplane@sha256:0d902ee5ae66c4aeb6ae22413a5107995f37b0a40fc7d60cdc51b77b44d1e11b
  - reported version: flowplane 3.1.0
  - index platforms: linux/amd64, linux/arm64

## Supporting images (env provenance)
- postgres:16 @ sha256:33f923b05f64ca54ac4401c01126a6b92afe839a0aa0a52bc5aeb5cc958e5f20
- hashicorp/http-echo:1.0.0 @ sha256:fcb75f691c8b0414d670ae570240cbf95502cc18a9ba57e982ecac589760a186
- envoyproxy/envoy:v1.37-latest @ sha256:1c2b79776c6e3b38e8b0113b825e6a599f9bfc08d680c199d80bf8964856c529

## Release assets (v3.1.0)
- compose.eval.yml (18235) — API digest sha256:ebb8be93...
- flowplane-3.1.0-linux-amd64.tar.gz (17872095) — sha256 4a12db0e...5597ff95 → SHA256SUMS OK
- flowplane-3.1.0-linux-arm64.tar.gz (17103532) — not host-arch, download/checksum-only
- flowplane-3.1.0-linux-amd64.cargo-metadata.sbom.json (2066821) — sha256 350bc9d8... → SHA256SUMS OK
- flowplane-3.1.0-linux-arm64.cargo-metadata.sbom.json (2066821) — same content hash as amd64 sbom
- SHA256SUMS (440)

## Binary tarball (amd64, checksum-verified)
- contents: bin/flowplane, bin/flowplane-agent, bin/flowplane-rls, bin/fp-agent (symlink→flowplane-agent), dataplane/, release-manifest.md
- flowplane --version → flowplane 3.1.0 ✓
- flowplane-agent --version → flowplane-agent 3.1.0 ✓
- flowplane-rls: no --version/--help/-h; always attempts startup and hits loopback plaintext-gRPC guard (exit 1). Executable present & runs. [NOTE - candidate finding if docs claim rls --help/--version]

## Packaging smokes (eval image, pinned digest)
- flowplane-agent --help → PASS (prints usage, exit 0)
- flowplane-rls --help → executes but no help; hits guard (present & runs) — PASS(present)/NOTE(no help flag)

## Compose bundle topology (compose.eval.yml)
- Services: postgres, shared-init(one-shot), pki(one-shot), flowplane-eval(CP serve), demo-upstream(http-echo), init(one-shot), flowplane-dashboard, envoy, flowplane-agent
- Gateway (Envoy listener): 127.0.0.1:10000
- API (REST/MCP): 127.0.0.1:8080
- Dashboard: 127.0.0.1:8081
- xDS: 18000 (internal, mTLS)
- Envoy service present ✓; uses envoyproxy/envoy:v1.37-latest; real xDS mTLS via bundled single-CA PKI

## Ports: 8080, 8081, 10000, 18000 free. (host 5432 in use by session PG; bundle PG has no host port)

## Manifest pages (34): README.md; docs/README.md; docs/concepts/{cli-contract,global-rate-limiting,tenancy-grants-xds}; docs/how-to/{ai-gateway-route-budget,aws-secure-deployment,bootstrap-platform,cli-auth-and-contexts,configure-oidc-provider,create-tenant-org-and-team,dataplane-egress-security,evaluate-platform,global-rate-limit,import-and-publish-openapi-spec,jwt-auth-rate-limit-route,learn-and-publish-api-spec,manage-users-teams-and-grants,onboard-api-team,production-readiness,register-dataplane-mtls,script-the-cli,secret-kek-rotation,trace-ai-requests,view-team-dashboard}; docs/reference/{adoption-evaluation-issue-map,cli,configuration,errors,filters,observability-alerts,rest-api}; docs/tutorials/{evaluate-no-clone,getting-started}

## PREFLIGHT: complete. Container runtime available. Proceeding to live phases.

## PHASE 1 — no-clone quick start: PASS (AS_WRITTEN)
- compose.eval.yml identical across raw.githubusercontent/release-asset/archive (sha256 ebb8be93...=API asset digest)
- stack up --no-build w/ pinned eval digest; all one-shots exit 0; CP/dashboard/pg healthy
- curl http://127.0.0.1:10000/ → HTTP 200, body "hello from the flowplane eval demo upstream", server: envoy, x-envoy-upstream-service-time present (through-gateway proven)
- auth whoami → authenticated dev-org owner
- cluster/listener/route list → demo-upstream cluster, demo listener :10000, demo-routes
- api create catalog --from-openapi → spec v1, tool_count 1
- api spec publish catalog 1 → tool_count 1; api status → published_spec_version_id set, tool_count 1
- mcp status → dynamic_enabled_tool_count 1, static_tool_count 35
- dp-eval live: heartbeat advancing, total_requests>0, total_errors 0

## PHASE 2 — dashboard acceptance: PASS (12/12)
1 starts (healthy container) ✓
2 /shared/dashboard-url created ✓
3 loopback + 128-bit per-launch nonce (http://127.0.0.1:8081/<32hex>/) ✓
4 no-nonce/wrong-nonce → 404 ✓
5 Overview renders (200, <title>Flowplane — default</title>) ✓
6 Overview dataplane total = 1 (positive) ✓
7 dp-eval Live + telemetry advances (Requests 11→43) ✓
8 Overview polls hx-trigger="load, every 10s" (matches doc "every 10 seconds") ✓
9 non-Overview panels hx-trigger="load once"/"...once" (on-demand, matches doc) ✓
10 Overview(root)/Resources/APIs/Learning/AI/MCP/Operations all 200 ✓ (note: Overview at nonce-root; /overview not a route by design)
11 off-loopback bind (container --listen 0.0.0.0:8081) still rejects foreign Host: evil.com→403, Origin: http://evil.com→403; correct Host→200 ✓
12 dev-token (621 chars) NOT present in Overview HTML; browser talks only to dashboard server ✓

## PHASE 3 (partial) 
- register-dataplane-mtls.md: CORE_EXECUTED PASS — bundle init/agent run the documented dataplane create→cert issue→bootstrap --mode mtls→flowplane-agent flow; step5 verify: stats overview live_dataplanes=1/stale=0/errors=0; ops xds status health=healthy, config_verified=1, dp-eval live=true, recent_nack=0
- bare-binary journey (production-readiness binary-acquisition): CORE_EXECUTED PASS — checksum-verified amd64 archive, scratch bin/ on PATH; flowplane --version=3.1.0; --help shows full surface incl rate-limit/dataplane; flowplane openapi serves OpenAPI 3.1.0 version 3.1.0

## PHASE 7 GUARDS: all PASS (fail closed)
1 hardened image + reachable DB + FLOWPLANE_DEV_MODE=true → "built without the dev-oidc feature (release artifact)"; exit1; never serves dev mode ✓
2 RLS non-loopback plaintext gRPC bind (0.0.0.0:50051) → refuse (needs GRPC_TLS triad or loopback) exit1 ✓
3 RLS partial gRPC TLS triad (only CERT) → refuse (all-or-none) exit1 ✓
4 RLS non-loopback admin bind no TLS/token → refuse exit1; +4b admin TLS w/o credential → refuse exit1 ✓
5 CP RLS admin token + non-loopback plaintext admin URL → "the bearer would cross a plaintext channel" exit1 ✓

## PHASE 4 — separate-instance integration: PASS
Harness: release compose.eval.yml (unmodified) + scratch override.rls.yml adding standalone flowplane-rls (mTLS split-node per global-rate-limit.md §Production). 6 distinct long-running instances:
- postgres, flowplane-eval(CP id 6714dc pid12948), rls(id f1eae5 pid12434), envoy(id c057b2 pid13392), flowplane-agent(id f155d4 pid13552), demo-upstream — all distinct IDs/PIDs; agent shares envoy netns (documented sidecar) but separate container; RLS separate from CP.
1 CP /healthz 200 {status ok,version 3.1.0}; /readyz 200 (database ok, xds_outbox_consumer ok) ✓
2 RLS admin HTTPS: /healthz 200, /readyz 200 (curl --cacert rls-ca, SAN=rls); plain HTTP→reset ✓
3 Envoy xDS: /ready LIVE; cds.update_success=1/rejected=0, lds.update_success=1/rejected=0 ✓
4 agent independent: heartbeats to CP, dp-eval live (earlier) ✓
5 CP dp live + advancing heartbeat ✓
6 request through Envoy→upstream 200 ✓
7 CP→RLS: log "applied CP rate-limit policy push policies=1"; Envoy→RLS: rate_limit_cluster w/ TLS transport_socket, ssl.handshake=4, upstream_rq_total=331 ✓
8 config mutation via CLI (route update + listener update, --revision) propagated to Envoy — composed domain appeared, NO restart (all restartCount=0) ✓
9 subsequent gateway request observed changed config (rate limiting active) ✓
RLS startup: grpc_security="mtls" admin_security="https + bearer"; CP: rate_limit_cluster injected mtls:true, rls_sync worker started reconcile_secs=60 authenticated:true

## PHASE 5 — RLS end-to-end: PASS (AS_WRITTEN §6 exact)
1 domain "checkout" created ✓
2 policy "per-client" descriptors{api_key:acme} 100/min ✓
3 route demo-routes descriptor api_key from x-api-key header (route update) ✓
4 CP→RLS sync: rls_sync 60s loop, "applied CP rate-limit policy push policies=1" ✓
5 Envoy: rate_limit_cluster (STRICT_DNS + envoy.transport_sockets.tls) + composed domain 16d8d7aa-...|04c9f1e3-...|checkout ✓
6 acme 101 reqs → EXACTLY 100x200 + 1x429 (matches doc) ✓
7 different value beta/gamma/delta unlimited — policy pins exact value api_key=acme (see NOTE) 
8 (independent counters: acme counter separate; unmatched values unlimited)
9 policy update 100→200→5 (revision-checked), reconcile push, acme@5 → EXACTLY 5x200+7x429; enforcement changed with ZERO restart (restartCount=0 all) ✓
3-boundary: CP(rls_sync+push) / Envoy(over_limit=8, mTLS to rate_limit_cluster) / RLS(331 gRPC decisions) — 429 attributable to standalone RLS ✓
NOTE(doc-imprecision, low/ambiguous): global-rate-limit.md line 21 "100 requests/minute per distinct api_key" + line ~263 "a different x-api-key value has its own counter" read as per-value limiting, but the policy descriptors {api_key:acme} matches only the exact value "acme" (line 190-191 does clarify "exact set ... for this policy to match"). Observed: only acme limited; other values unlimited. §6 executable claim passes exactly. Recorded as minor/ambiguous, mitigated by line 190-191 — not filed as issue.

## PHASE 6 — failure & recovery: PASS
RLS interruption (fail-open, failure_mode_deny:false): steady 5x200+5x429 → stop RLS → 20x200 (fail OPEN, requests proceed) → restart RLS → CP repush 07:03:20 → recovered 5x200+5x429; CP/Envoy/agent restartCount=0 ✓
RLS fail-CLOSED (failure_mode_deny:true): stop RLS → 6x500 (reject) ✓  [both documented modes observed]
CP interruption: steady gateway 200 + agent live → stop CP → existing Envoy traffic 6x200 (last-good) → restart CP (same PG, healthy 4s) → Envoy+agent reconnect (xds live:true health healthy nack 0) → expose demo2 :10002 reached Envoy (config_dump + netns curl body) ✓
