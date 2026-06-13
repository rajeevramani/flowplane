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
| Endpoint priority | Missing | Add if needed for Envoy priority locality behavior; otherwise explicit deferral |
| Endpoint health status | Missing as user input | Decide whether operator-supplied health is valid or should remain Envoy-observed only |
| Connect timeout | Present | Keep |
| LB: round robin, least request, random, ring hash | Partial | Add per-policy option structs where Envoy supports knobs |
| LB: Maglev | Missing | Add typed Maglev config or explicitly defer with reason |
| LB: cluster provided | Missing | Likely defer unless a concrete extension needs it |
| DNS lookup family | Missing | Add explicit DNS family field for DNS clusters |
| Upstream TLS enablement | Present as `use_tls` | Keep explicit; do not reintroduce port-based inference |
| Upstream SNI / server name | Missing | Add typed upstream TLS config |
| Upstream CA/SAN validation | Missing | Add typed validation context or SDS reference |
| Upstream client certificate | Missing | Add only if required by gateway mTLS upstream use cases |
| Upstream HTTP/2 or gRPC protocol | Missing | Add explicit protocol options |
| HTTP health checks | Partial | Add host/method/expected status support where needed |
| TCP health checks | Missing | Add typed TCP health check variant or explicit deferral |
| Circuit breakers | Partial | Add Envoy threshold structure if V1 examples require priorities |
| Outlier detection | Partial | Add missing fields such as minimum host behavior where required |

## Route Parity

| V1 capability | V2 status | S7.8 action |
| --- | --- | --- |
| Virtual host names and domains | Present | Keep |
| Prefix and exact path match | Present | Keep |
| URI template match | Present | Verify translator behavior and add live test |
| Regex path match | Missing | Add typed regex matcher with validation |
| Header matchers | Missing | Add exact/regex/present/range or a deliberate supported subset |
| Query parameter matchers | Missing | Add and emit to Envoy; fix V1's parsed-but-not-emitted gap |
| Single-cluster action | Present | Keep |
| Weighted clusters | Missing | Add typed weighted cluster action with per-weight filter config support if needed |
| Prefix rewrite | Present | Keep |
| Template rewrite | Domain present; translation must be verified | Wire or test xDS translation before marking done |
| Timeout | Present | Keep |
| Retry policy | Missing | Add typed retry policy with duration units fixed |
| Redirect action | Missing | Add if V1 outcome is considered core |
| Direct response action | Not currently modeled | Decide whether this is core gateway parity or an S12 hardening feature |
| Per-route/vhost rate limits | Missing | Add route/vhost rate-limit actions once RLS/domain model is chosen |
| Typed per-filter config | Partial through overrides | Expand as filter catalog grows |

## Listener Parity

| V1 capability | V2 status | S7.8 action |
| --- | --- | --- |
| Address and port | Present | Keep |
| HTTP listener with RDS | Present | Keep |
| Protocol enum: HTTP, HTTP/2, HTTPS, TCP | Missing | Add explicit protocol/listener kind so TCP is first-class |
| HTTPS downstream TLS | Partial | Keep and expand only where Envoy fields are missing |
| Multiple filter chains | Missing | Add only if required by TLS/SNI or TCP parity; otherwise explicit deferral |
| Inline route config | Missing | Prefer RDS unless there is a concrete V2 UX need |
| TCP proxy | Missing | Add typed TCP listener action if V1 TCP gateway capability is core |
| Access logs | Missing as user config | Add typed access-log config; learning can later inject capture-specific logs through the same IR |
| Tracing | Missing | Add typed tracing config if V1 examples require it before S8 |
| Request ID behavior | Translator-owned | Verify and document default behavior |
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
| Global rate limit / RLS | Missing | Add or explicitly anchor to the rate-limit/AI-budget slice before S8 resumes |
| Rate limit quota | Missing | Decide if core or defer with explicit reason |
| ExtProc | Missing | Important for learning/AI gateway capture paths; design through typed IR, not post-hoc injection |
| OAuth2 | Missing | Decide if core gateway parity or later auth slice |
| Credential injector | Missing | Likely AI gateway/security slice, but document the deferral |
| Custom response | Missing | Decide if core operator parity |
| MCP filter/tool routing | Missing | S11, but any Envoy-facing filter shape should be reserved now |
| WASM | Missing | Likely defer unless there is a current extension use case |

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

## PROGRESS.md Mapping

S7.8 should be tracked as a pre-S8 workstream:

- S7.8a: finalize this parity matrix against V1 examples and current V2 code.
- S7.8b: cluster field parity.
- S7.8c: route field parity.
- S7.8d: listener field parity.
- S7.8e: filter parity decisions and required implementations.
- S7.8f: DB/API/CLI/OpenAPI parity examples and tests.
- S7.8g: live Envoy parity E2E, including ACK/NACK diagnostics.

S8 learning may resume only after S7.7 and S7.8 have enough coverage that learning can rely on the
gateway model instead of compensating for it.
