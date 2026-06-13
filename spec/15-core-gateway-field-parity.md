# 15 — Core Gateway Field Parity

Purpose: make V2's core gateway API, database model, CLI, and xDS output capable of representing
the gateway fields that V1 exposed to Envoy, while preserving the V2 architecture rules in
`spec/14-architecture-integrity.md`.

Parity here means **same operator and dataplane capability**, not identical V1 JSON shapes. V2 keeps
typed domain specs, service-layer validation, one PostgreSQL source of truth, reference tracking,
and explicit xDS translation. Known V1 defects are fixed rather than copied.

## Non-Negotiables

1. The authoritative state remains the V2 domain spec stored once in PostgreSQL. Extra tables may
   exist only as derived read models or indexes.
2. Every accepted field has domain validation, storage round-trip coverage, OpenAPI schema coverage,
   CLI/create-or-apply examples, and xDS translator tests.
3. Every behaviorally meaningful Envoy field has at least one live Envoy ACK or smoke test before
   S8 resumes.
4. V1 smells are not ported: no implicit TLS from port 443, no parsed-but-unemitted query matchers,
   no retry duration unit bug, no FK-by-name cascade authority, and no protobuf surgery after typed
   translation.
5. If a V1 field is intentionally deferred, this document must name the reason, later progress
   anchor, and user-visible impact.

## Current Verdict

V2 is not yet at V1 core gateway field parity. The current model is cleaner and safer, but narrower:

- clusters cover endpoints, basic load balancing, explicit TLS boolean, simple HTTP health checks,
  simple circuit breakers, and simple outlier detection.
- routes cover virtual hosts, prefix/exact/template path matches, single-cluster actions, rewrites,
  timeouts, and filter overrides.
- listeners cover a single HTTP/RDS shape, optional HTTP filters, and downstream TLS via file or SDS.
- filters cover a typed subset: CORS, local rate limit, header mutation, health check, compressor,
  JWT auth, ext authz, and RBAC.

That is enough for the S7 simple route-to-traffic path, but not enough as the foundation for learning
and AI gateway work. S8 should not infer, learn, or publish against a gateway model that cannot yet
represent the core fields operators can reasonably expect from V1.

## S7.8a Audit Baseline

Audit inputs:

- V1 domain/xDS surface: `src/xds/{cluster_spec,route,listener,secret}.rs`,
  `src/xds/filters/http/**`, `filter-schemas/built-in/*.yaml`, and V1 docs/reference material.
- V2 domain/xDS surface: `crates/fp-domain/src/gateway/**`,
  `crates/fp-xds/src/translate.rs`, `crates/fp-storage/src/repos/**`, and API/CLI resource paths.
- Existing V2 specs: `spec/03-domain-model.md`, `spec/04-xds.md`, and
  `spec/14-architecture-integrity.md`.

Verdict:

- **Must implement before S8:** fields needed for the core HTTP gateway model that V1 exposed and
  that learning/API binding will rely on: richer cluster policy/TLS/protocol/health fields, route
  match/action parity, listener kind/logging/tracing basics, and the filter hooks required by
  learning/rate limiting.
- **Can defer with explicit owner:** fields that are valid gateway features but not required for the
  S8 learning foundation: OAuth2, credential injection, custom response, WASM, MCP enforcement, and
  some advanced rate-limit quota behavior.
- **Reject or keep out of V2:** V1 implementation shortcuts: implicit upstream TLS on port 443,
  parsed-but-unemitted query matchers, retry duration unit bugs, dynamic Struct fallback for
  unknown filters, JSON/projection dual authority, and post-hoc listener/route protobuf surgery.

Implementation order:

1. Cluster parity, because routes and `expose` depend on a correct upstream model.
2. Route parity, because learning binds to route scopes and must observe the same path/header/query
   semantics that Envoy enforces.
3. Listener parity, because capture/logging/tracing and TCP/HTTPS decisions live at the HCM/listener
   boundary.
4. Filter parity decisions and the minimum implementation needed for learning/rate limiting.
5. API/CLI/OpenAPI examples plus live Envoy ACK/smoke coverage.

## Acceptance Shape

For each field group below, "done" means:

- API accepts a typed V2 request shape and rejects invalid combinations with actionable errors.
- storage round-trips the full spec without lossy projections.
- CLI can create/apply the resource from JSON or YAML examples.
- xDS translation emits the matching Envoy field or explicitly documents a deliberate no-op.
- tests cover validation, serialization, DB round-trip, xDS translation, and at least one live Envoy
  path for each behavior class.

## Cluster Parity

| V1 capability | V2 status | S7.8 action |
| --- | --- | --- |
| Endpoint host and port | Present | Keep; add examples that cover IP and DNS endpoints |
| Endpoint weight | Present | Keep; assert weighted endpoint translation |
| Endpoint priority | Missing | **Defer pre-S8.** V1 synced priority from raw Envoy-shaped projections; V2 should add this later only with a first-class priority/locality model |
| Endpoint health status | Missing as user input | **Defer as authored config.** Health should be observed/reported, not operator-authored, unless a later EDS locality model needs it |
| Connect timeout | Present | Keep |
| LB: round robin, least request, random, ring hash | Implemented in S7.8b | V2 now stores typed least-request/ring-hash options and emits Envoy LB config |
| LB: Maglev | Implemented in S7.8b | V2 now stores Maglev table size and emits Envoy Maglev LB config |
| LB: cluster provided | Missing | **Reject for pre-S8.** Only add with a concrete extension/custom LB use case |
| DNS lookup family | Implemented in S7.8b | Stored on `ClusterSpec` and emitted for DNS clusters; ignored for EDS/IP clusters |
| Upstream TLS enablement | Present as `use_tls` | Keep explicit; do not reintroduce port-based inference |
| Upstream SNI / server name | Implemented in S7.8b | Stored in typed upstream TLS config and emitted in `UpstreamTlsContext` |
| Upstream CA/SAN validation | Implemented in S7.8b | Reference-based SDS validation context only; no inline secret material |
| Upstream client certificate | Missing | **Defer pre-S8** unless a concrete upstream mTLS example becomes required |
| Upstream HTTP/2 or gRPC protocol | Implemented in S7.8b | Emits modern typed upstream HTTP protocol options with explicit HTTP/2 config |
| HTTP health checks | Implemented in S7.8b | Supports host, method, expected status ranges, thresholds, timeout, and interval |
| TCP health checks | Implemented in S7.8b | Stored and emitted as a typed TCP health check variant |
| Circuit breakers | Implemented in S7.8b | Supports Envoy default/high threshold structure |
| Outlier detection | Implemented in S7.8b | Adds minimum host behavior alongside existing outlier fields |

## Route Parity

| V1 capability | V2 status | S7.8 action |
| --- | --- | --- |
| Virtual host names and domains | Present | Keep |
| Prefix and exact path match | Present | Keep |
| URI template match | Verified in S7.8c | Domain and xDS translator tests prove Envoy receives URI-template match policy |
| Regex path match | Implemented in S7.8c | Typed regex path matcher with bounded validation and Envoy safe-regex emission |
| Header matchers | Implemented in S7.8c | Exact/prefix/suffix/contains/regex/present supported; range deferred unless needed |
| Query parameter matchers | Implemented in S7.8c | Exact/prefix/suffix/contains/regex/present supported and emitted |
| Single-cluster action | Present | Keep |
| Weighted clusters | Implemented in S7.8c | Typed weighted cluster action with dependency tracking across all target clusters |
| Prefix rewrite | Present | Keep |
| Template rewrite | Implemented in S7.8c | Emits Envoy URI-template rewrite policy for template routes |
| Timeout | Present | Keep |
| Retry policy | Implemented in S7.8c | Typed retry-on, retry count, per-try timeout seconds, and retriable status codes |
| Redirect action | Implemented in S7.8c | Typed host/scheme/path/prefix/status/strip-query redirect action |
| Direct response action | Not currently modeled | **Defer pre-S8** unless required by a core gateway example |
| Per-route/vhost rate limits | Implemented in S7.8c | First-class V2 descriptor hooks on VirtualHost and RouteAction, emitting Envoy `RateLimit` with `request_headers` and `generic_key` actions. Global RLS enforcement filter remains a filter-parity item |
| Typed per-filter config | Partial through overrides | Existing override path remains; expand as filter catalog grows |

## Listener Parity

| V1 capability | V2 status | S7.8 action |
| --- | --- | --- |
| Address and port | Present | Keep |
| HTTP listener with RDS | Present | Keep |
| Protocol enum: HTTP, HTTP/2, HTTPS, TCP | HTTP/HTTP2/HTTPS implemented in S7.8d | HTTP/HTTP2/HTTPS now have a typed listener protocol with HCM codec mapping and HTTPS TLS validation. TCP is explicitly deferred until V2 has a first-class TCP route/action model |
| HTTPS downstream TLS | Implemented for single HTTP filter chain | SDS and file-backed certificate sources remain the supported V2 model |
| Multiple filter chains | Missing | **Defer pre-S8** unless TLS/SNI routing requires it; V2 should avoid this complexity until needed |
| Inline route config | Missing | **Reject by default.** Prefer RDS as the V2 architecture; add inline only for a concrete UX need |
| TCP proxy | Deferred in S7.8d | Do not add TCP listener/proxy until the route/action domain can express TCP without overloading HTTP routes |
| Access logs | Basic file access logs implemented in S7.8d | Typed listener `access_logs` emit `envoy.access_loggers.file`; learning can later inject capture logs through the same IR |
| Tracing | Deferred in S7.8d | Track under observability/filter parity because provider-specific OTel/Zipkin/generic config needs API and cluster decisions |
| Request ID behavior | Implemented in S7.8d | Translator always enables HCM request-id generation and response echo for ALS/extproc correlation |
| HTTP filter chain order | Present for current subset | Keep router auto-append invariant |

## Filter Parity

| Filter | V2 status | S7.8 action |
| --- | --- | --- |
| CORS | Present | Keep; add examples |
| Local rate limit | Present | Keep; integrate with route/vhost overrides |
| Header mutation | Present | Keep |
| Health check | Present | Keep |
| Compressor | Present | Keep |
| JWT auth | Present | Keep |
| Ext authz | Present | Keep |
| RBAC | Present | Keep |
| Global rate limit / RLS | Implemented in S7.8e | Route/vhost descriptor hooks plus listener-chain `envoy.filters.http.ratelimit` client config. V2 intentionally stops at the Envoy IR/API surface here; RLS domain/policy storage is a separate policy-service slice, not a gateway parity blocker |
| Rate limit quota | Missing | **Defer pre-S8** unless quota behavior is needed for AI gateway budgeting immediately |
| ExtProc | Deferred to S8 learning capture | Required for body capture, but not for core gateway parity. It must enter through typed IR, not post-hoc injection |
| OAuth2 | Missing | **Defer pre-S8** to a later auth slice |
| Credential injector | Missing | **Defer pre-S8** to AI gateway/security slice |
| Custom response | Missing | **Defer pre-S8** unless core operator examples need it |
| MCP filter/tool routing | Missing | **Defer to S11**, but reserve a typed filter shape so S11 does not need raw patches |
| WASM | Missing | **Defer pre-S8** unless there is a current extension use case |

## Secrets And SDS

V2 is already stronger than V1 in the broad direction: secrets are write-only, encrypted, scoped, and
translated through SDS. S7.8 should still prove that gateway field parity does not bypass this model.

| Capability | V2 status | S7.8 action |
| --- | --- | --- |
| TLS certificate secrets | Present | Add gateway examples that consume them |
| Validation context secrets | Present | Add upstream and downstream validation examples |
| Session ticket keys | Present if already modeled | Verify coverage before claiming parity |
| Inline secret material in gateway specs | Must remain forbidden | Keep secret references separate from gateway specs |

## CLI And API Experience

Field parity should improve the V2 UX, not expose raw Envoy complexity everywhere.

- Low-level `cluster`, `route-config`, `listener`, and `filter` commands should support full typed
  specs from JSON/YAML files.
- `flowplane expose` should stay simple and generate the common case without requiring users to
  understand every field.
- Advanced fields should be available through `apply`/resource-specific create commands, with
  examples rather than long flag sets.
- OpenAPI examples should show at least one simple case and one advanced V1-parity case per resource.
- S7.8f pins this contract with an authenticated REST create/get round-trip for advanced
  route/listener specs, OpenAPI schema component assertions, and a CLI `apply` manifest test proving
  advanced typed specs are preserved rather than projected into a smaller shape.

## PROGRESS.md Mapping

S7.8 should be tracked as a pre-S8 workstream:

- S7.8a: finalize this parity matrix against V1 examples and current V2 code. **Done in this
  audit baseline; keep updating only when a field decision changes.**
- S7.8b: cluster field parity.
- S7.8c: route field parity.
- S7.8d: listener field parity.
- S7.8e: filter parity decisions and required implementations.
- S7.8f: DB/API/CLI/OpenAPI parity examples and tests.
- S7.8g: live Envoy parity E2E, including ACK/NACK diagnostics.

S7.8g is pinned in `scripts/e2e-envoy.sh`: the live Envoy run covers baseline traffic, restart
convergence, cross-team isolation, HTTP filter behavior, auth filters, SDS rotation, and an advanced
parity phase that ACKs/serves route/listener/filter config using regex/header/query matchers,
weighted clusters, retry policy, route RLS descriptors, HTTP/2 listener mode, file access logs, and
the global RLS HTTP filter.

S8 learning may resume only after S7.7 and S7.8 have enough coverage that learning can rely on the
gateway model instead of compensating for it.
