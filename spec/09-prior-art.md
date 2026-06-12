# 09 — Prior Art: Envoy Gateway, Envoy AI Gateway, and Adjacent AI Gateways

This survey is the requirements input for Flowplane v2's native AI gateway capability
(provider routing, credentials, token-aware budgets, failover). Primary subjects were read
from source at commit HEAD of shallow clones taken 2026-06-12:

- `github.com/envoyproxy/ai-gateway` → `/tmp/envoy-ai-gateway`
- `github.com/envoyproxy/gateway` → `/tmp/envoy-gateway`

Provenance markers used throughout: **[src]** = read from repo source code, **[docs]** = the
project's published docs (the `site/` / `docs/` trees in the repos, which back
aigateway.envoyproxy.io and gateway.envoyproxy.io), **[web]** = external web search/fetch.
Verdicts are **borrow** (take the idea near-verbatim), **adapt** (take the shape, change the
substrate), **reject** (deliberately not doing this).

---

## 1. Envoy Gateway

Envoy Gateway (EG) is the Kubernetes Gateway API implementation that Envoy AI Gateway builds
on. It is the closest thing to a reference architecture for "control plane that compiles a
declarative resource model into xDS" — the same job Flowplane does with PostgreSQL instead of
the K8s API server.

### 1.1 Resource model

**[src]** EG implements the standard Gateway API kinds (GatewayClass, Gateway, HTTPRoute,
GRPCRoute, TCPRoute, UDPRoute, TLSRoute) and defines its own extension CRDs in
`api/v1alpha1`:

- **`Backend`** (`backend_types.go`) — a first-class backend resource (vs. K8s Service):
  endpoints of type FQDN, IP, or Unix socket; `appProtocols` (h2c/ws/wss); and a `fallback`
  bool that marks a backend as failover-only (Envoy priority > 0, with a 1.4x
  over-provisioning factor).
- **`BackendTrafficPolicy`** (BTP) — the workhorse policy: rate limit (local + global),
  retry, circuit breaker, health check (active + passive/outlier detection), load balancer,
  timeouts, fault injection, compression, response override, request buffering, telemetry.
- **`SecurityPolicy`** — downstream authn/z: JWT, OIDC, API key, basic auth, CORS, ext-authz,
  authorization rules.
- **`ClientTrafficPolicy`** — listener-side behavior (TLS, keepalive, client IP detection,
  HTTP/1/2 knobs); Gateway-scoped only.
- **`EnvoyPatchPolicy`** — RFC 6902 JSONPatch applied to generated xDS (Listener,
  RouteConfiguration, Cluster, ClusterLoadAssignment, Secret), with an int32 `priority` for
  ordering.
- **`EnvoyExtensionPolicy`** — attach Wasm / ExtProc / Lua / dynamic-module filters.
- **`HTTPRouteFilter`** — extended per-route filters: URL rewrite (incl. hostname rewrite,
  which ai-gateway uses), direct response, credential injection.

The critical field for the AI story is BTP's **rate limit cost specifier**
(`api/v1alpha1/ratelimit_types.go`) **[src]**:

```
RateLimitCost { Request, Response *RateLimitCostSpecifier }
RateLimitCostSpecifier { From: Number|Metadata, Number *uint64, Metadata { Namespace, Key } }
```

`cost.request.from: Number` charges a fixed cost on the request path;
`cost.response.from: Metadata` reads a per-request cost from Envoy dynamic metadata **after
the response completes** and decrements the rate-limit budget by it. This single generic
mechanism is what makes token-aware rate limiting possible without the rate limiter knowing
anything about LLMs. (Maps to Envoy's `apply_on_stream_done` / `hits_addend` rate-limit
features, Envoy ≥ 1.33.)

**Verdicts:**

- *Generic metadata-driven rate-limit cost* — **borrow**. Flowplane already ships
  `flowplane-rls`; extending its descriptor model with `cost.response.from = metadata(ns,
  key)` reuses everything we have and keeps the RLS LLM-agnostic. This is the single most
  important mechanism in this survey.
- *First-class `Backend` resource (FQDN/IP endpoints, app protocol, fallback flag)* —
  **adapt**. Flowplane's `Cluster` entity already plays this role; add the `fallback`/priority
  notion to cluster endpoints rather than introducing a new entity.
- *Separate ClientTrafficPolicy / BackendTrafficPolicy / SecurityPolicy split* — **adapt**.
  The split itself (listener-side vs. cluster-side vs. authn/z) is clean and matches
  Flowplane's existing listener-scope vs. route-scope filter attachment; we don't need new
  resource kinds, but the *taxonomy* should inform how Flowplane groups AI policy fields.
- *EnvoyPatchPolicy (raw JSONPatch escape hatch on xDS)* — **reject** for v2. Flowplane's
  contract is "every snapshot is a deterministic projection of DB state" with typed
  builders; an arbitrary-patch escape hatch undermines validation, team isolation (a patch
  can touch resources you don't own), and diagnosability. If we ever need it, it must be
  org-admin-only and audited. Worth recording as an explicit non-goal.

### 1.2 The IR / translation pipeline

**[src]** EG's pipeline (`internal/provider`, `internal/message`, `internal/gatewayapi`,
`internal/ir`, `internal/xds/translator`) is staged:

1. **Provider** — sources resources. Kubernetes provider (watch) *or* a **file provider**
   (`internal/provider/file`, fsnotify-based) that runs the identical pipeline without K8s.
   The provider abstraction proves the rest of the stack is substrate-agnostic.
2. **Message bus** — `watchable.Map`s (`internal/message/types.go`): `ProviderResources`,
   `XdsIR` (keyed per proxy), `InfraIR`, plus per-policy status maps. Each stage is a runner
   subscribing to upstream maps and publishing downstream — a reactive dataflow, not a
   monolithic reconcile.
3. **Gateway API translator** (`internal/gatewayapi/translator.go`) — resolves refs,
   validates, merges policies, computes status, and emits the **IR**.
4. **IR** (`internal/ir`) — two halves: `Xds` (HTTP/TCP/UDP listener trees with
   `TrafficFeatures` — timeout, retry, circuit breaker, health check, LB — attached
   per-route/cluster) and `Infra` (what proxy infrastructure must exist). The IR is
   deliberately neither Gateway API nor Envoy protobuf: user-intent semantics already
   resolved, Envoy mechanics not yet chosen.
5. **xDS translator** (`internal/xds/translator`) — IR → Envoy protos, with **extension
   hooks** (`PostRouteModify`, `PostClusterModify`, `PostVirtualHostModify`,
   `PostTranslateModify`) that let an external gRPC server mutate generated config. This is
   exactly how ai-gateway plugs in.
6. **Snapshot cache → ADS**, keyed per proxy.

**Verdicts:**

- *Explicit IR between domain model and Envoy protobuf* — **borrow**. Flowplane v1's
  `xds/state.rs` goes domain→protobuf in one hop; an IR layer gives us (a) a place to apply
  team merge/visibility before Envoy details exist, (b) goldens-friendly testing, (c) a seam
  where the AI-gateway feature can inject filters/clusters without special-casing the core
  builders. The two-IR split (xds-IR vs infra-IR) maps to Flowplane's "Envoy config" vs
  "dataplane registration/agent" concerns.
- *Watchable-map reactive pipeline* — **adapt**. The dataflow staging (dirty-marking →
  rebuild affected snapshots) is what Flowplane already does; full pub/sub maps are
  K8s-controller idiom we don't need. Keep dirty-set + rebuild, but make stage boundaries
  (resolve → IR → protobuf) explicit functions with typed inputs/outputs.
- *Provider abstraction (file provider proves substrate independence)* — **borrow** the
  lesson, not the code: keep the translator pure over an in-memory resource set so it can be
  driven from Postgres rows, fixtures in tests, or a future import tool identically.
- *Post-translation gRPC extension hooks* — **reject** as an external interface (Flowplane
  is not a platform for third-party controllers), but **adapt** internally: the AI-gateway
  subsystem should consume the same internal hook seam the IR exposes, rather than being
  woven through the core translator. ai-gateway existing *entirely* as an extension-server
  client of EG is strong evidence this layering works.

### 1.3 Policy attachment

**[src]** Policies attach via `targetRef`/`targetRefs` (direct references to Gateway or
Route) or `targetSelectors` (label selection). Gateway-scoped policy applies to all routes
under it; route-scoped policy overrides. A `mergeType` field (`StrategicMerge`, `JSONMerge`,
`Replace`) controls how a route policy composes with the gateway policy (and may not be set
on gateway-targeting policies). Conflicts (two policies targeting the same resource) are not
silently resolved: the loser gets `Accepted=False, reason=Conflicted` in status; merged
policies get a `Merged` condition naming the policy they merged with.

**Verdicts:**

- *Attachment + override hierarchy* — **adapt**. Flowplane already has listener-scope vs
  route-scope filter attachment; the lesson to import is the *explicit, named merge
  semantics* (don't invent ad-hoc "route wins" rules per filter type) and bounded scope
  levels.
- *Surfacing conflicts and merges as resource status, never silently* — **borrow**. Maps to
  Flowplane status fields + audit log entries.
- *Label-based `targetSelectors`* — **reject** for v2; Flowplane resources reference each
  other by ID, and selector indirection makes "what applies to this route" non-obvious — the
  opposite of what an MCP agent diagnosing config needs.

### 1.4 Status and conditions reporting

**[src]** (`internal/gatewayapi/status/`) Every resource gets typed conditions:
`Accepted`, `Programmed` (xDS actually generated), `ResolvedRefs` (all references valid);
policies additionally get per-ancestor status (one condition set per target), `Merged`,
`Overridden`, and `Conflicted` outcomes, written back asynchronously through the message
bus.

**Verdict: adapt (strongly).** The condition *vocabulary* is the part worth stealing:
Flowplane should report, per resource, (1) accepted/validated, (2) programmed — i.e.
included in a snapshot that a dataplane ACKed (Flowplane already has NACK/warming
diagnostics; this closes the loop into per-resource status), (3) refs-resolved. Stored as
columns/rows in Postgres, surfaced via REST/CLI/MCP (`fp route status`), not as K8s
conditions. The "at most one condition, Accepted/NotAccepted" simplification that
*ai-gateway* uses (see §2.9) shows the full Gateway API ancestor machinery is overkill even
for them; Flowplane needs the three-condition core, not the ancestor matrix.

### 1.5 What translates to a non-K8s, PostgreSQL control plane

The deep lesson from EG is that almost none of its value is K8s-specific: resource model +
validation + policy merge + IR + deterministic translation + status write-back is a pure
function over a resource set, and EG itself proves it by shipping a file provider and a
standalone mode. What does *not* translate: CRD admission (CEL validation annotations →
Flowplane's validation layer + DB constraints), ReferenceGrant cross-namespace machinery (→
Flowplane team grants), and controller-runtime reconcile loops (→ transactional service
layer + dirty-set rebuild, which is simpler and gives atomicity K8s can't).

---

## 2. Envoy AI Gateway

Envoy AI Gateway (AIGW) layers LLM capability on EG. Architecture **[src]**
(`docs/proposals/001-ai-gateway-proposal`, `internal/`): a K8s controller reconciles its
CRDs into EG resources (each AIGatewayRoute generates an HTTPRoute of the same name, plus an
`ai-eg-host-rewrite-*` HTTPRouteFilter); an **extension server** (gRPC client of EG's
post-translation hooks, `internal/extensionserver/`) injects the AI filter and tweaks
clusters in generated xDS; and the data-plane brain is an **ExtProc** (external processor)
binary injected as a sidecar container into each Envoy pod by a mutating webhook
(`internal/controller/gateway_mutator.go`). The ExtProc runs at *two* filter positions:
router-level (parse body, extract model, set routing headers) and upstream-level (per-chosen
backend translation, auth injection, response/token processing) — the upstream position is
what makes per-backend transformation correct under retries to a different backend.

### 2.1 Resource model

All four core kinds exist in `api/v1alpha1` (deprecated) and `api/v1beta1`
(field-identical promotion, verified by diff) **[src]**:

**`AIGatewayRoute`** — the unified-API entrypoint. Spec fields:

| Field | Semantics |
|---|---|
| `parentRefs` | Gateways to attach to (Gateway kind only, ≤16) |
| `hostnames` | Host-header scoping, same as HTTPRoute |
| `rules[]` (≤15) | Each: `matches[]` (header matchers only — notably on the synthetic `x-ai-eg-model` header, which the ExtProc extracts from the request body *before* routing), `backendRefs[]` (≤128, weighted), `timeouts` (default bumped to 60s; streaming caveat documented), `modelsOwnedBy`/`modelsCreatedAt` (metadata served on the synthesized OpenAI `/models` endpoint) |
| `rules[].backendRefs[]` | `name`/`namespace` (AIServiceBackend, or exactly one InferencePool from the Gateway API Inference Extension), `weight`, `priority` (Envoy endpoint priority — failover tier), `modelNameOverride` (rewrite the model field toward this backend — the "model virtualization" primitive), `headerMutation`, `bodyMutation` (set/remove top-level JSON fields, e.g. force `service_tier`) |
| `llmRequestCosts[]` (≤36) | `{metadataKey, type: InputToken\|OutputToken\|TotalToken\|CachedInputToken\|CacheCreationInputToken\|ReasoningToken\|CEL, cel}` — declares which token numbers get written to dynamic metadata under namespace `io.envoy.ai_gateway` (see §2.4) |

CEL cost expressions get variables `model`, `backend`, `input_tokens`, `output_tokens`,
`total_tokens`, `cached_input_tokens`, `cache_creation_input_tokens`, `reasoning_tokens`
**[src]** (`shared_types.go`), e.g.
`backend == 'bar.default' ? (input_tokens - cached_input_tokens) + cached_input_tokens * 0.1 + cache_creation_input_tokens * 1.25 + output_tokens : total_tokens`.

**`AIServiceBackend`** — one provider endpoint: `schema: VersionedAPISchema` (the API
dialect this backend *speaks*: name + optional `version` (Azure) + optional `prefix` (path
prefix for OpenAI/Anthropic-compatible vendors)), `backendRef` (must be an EG `Backend` —
i.e. the actual FQDN/IP endpoint lives in EG's resource), plus backend-level
header/body mutations. A TODO in source notes backend-level cost overrides as future work.

**`BackendSecurityPolicy`** — credential attachment via `targetRefs` to AIServiceBackends
(direct policy attachment; at most one per backend, enforced at reconcile). See §2.3.

**`GatewayConfig`** — per-Gateway ExtProc container settings (env, resources) and
`globalLLMRequestCosts` (gateway-wide cost defaults that route-level `llmRequestCosts`
override per `metadataKey`).

**`QuotaPolicy`** (v1alpha1 only, newer) — first-party token quotas, see §2.4.
**`MCPRoute`** — see §2.8.

**Composition with EG**: AIGW deliberately owns only the AI semantics and *delegates*
generic traffic policy: retries/fallback, circuit breaking, health checks and rate limiting
are configured by attaching EG `BackendTrafficPolicy` to the *generated* HTTPRoute (same
name as the AIGatewayRoute) — stated in the AIGatewayRoute doc comment **[src]**.

**Verdicts:**

- *Three-way split: route (unified API + costs) / backend (dialect + endpoint) / security
  policy (credential)* — **borrow** the decomposition. Flowplane mapping: `ai_route` (or an
  AI-flavored Route), `ai_provider` (wrapping a Cluster), credential = a reference into
  Flowplane's existing secrets subsystem. The key insight: **credentials are a separate,
  attachable, individually-rotatable object, not a field on the backend**.
- *Model-as-routing-key via body-extracted header (`x-ai-eg-model`)* — **borrow** the
  mechanism exactly: extract model from body once at router level, expose it as a header so
  *ordinary route matching* does model routing. Keeps the routing engine generic.
- *`modelNameOverride` + weights + priority on backendRefs* — **borrow**; it is the minimal
  complete vocabulary for canarying models, A/B, and tiered failover (incl. cross-provider
  model fallback, since each backendRef pairs override+provider).
- *Delegating generic traffic policy to the underlying gateway layer* — **borrow**
  structurally: Flowplane's AI routes must reuse Flowplane's existing cluster
  resilience/retry/rate-limit config rather than growing AI-specific duplicates.
- *Generating a visible intermediate resource (HTTPRoute) the user may also patch* —
  **reject**. In Flowplane the projection from AI-route to listener/route/cluster rows
  should be internal and atomic (one service-layer transaction), not a second user-visible
  resource layer with ownership ambiguity. AIGW's own docs hedge here ("implementation
  detail subject to change").
- *Synthesized `/models` endpoint* (aggregated from route rules, with ownedBy/createdAt,
  host-scoped model lists) — **borrow**; cheap and makes the gateway self-describing to
  OpenAI-compatible clients. Flowplane bonus: the same inventory feeds MCP tool listings.
- *15-rule limit, header-match-only rules* — artifacts of HTTPRoute; **reject** (no such
  constraint in a DB-backed model).

### 2.2 Provider support and request/response translation

**[docs]** (`site/docs/capabilities/llm-integrations/supported-providers.md`): OpenAI, AWS
Bedrock (Converse API), Azure OpenAI, Google Gemini (AI Studio), GCP Vertex AI (incl.
native Gemini), Anthropic on Vertex, Anthropic native, Anthropic on Bedrock, Groq, Grok,
Together, Cohere (native v2 rerank + compat), Mistral, DeepInfra, DeepSeek, Hunyuan, Tencent
LLM Knowledge Engine, Tetrate TARS, SambaNova, and self-hosted (vLLM etc.). The long tail is
handled by **one trick**: anything OpenAI-compatible is `{"name":"OpenAI", "prefix":"/their/path"}`
— only genuinely different dialects (Bedrock, Vertex, Azure versioning, Anthropic) get real
translators.

**[src]** Translation is **not** a per-provider plugin zoo; it's a typed matrix of
(ingress schema → egress schema) translators in `internal/translator/` (e.g.
`openai_awsbedrock.go`, `openai_gcpvertexai.go`, `anthropic_openai.go`,
`anthropic_awsbedrock.go`...). Two ingress dialects are accepted (OpenAI `/v1/chat/completions`,
`/v1/completions`, `/v1/embeddings`, images, audio, responses; and native Anthropic
`/v1/messages`); each translator implements:

```go
type Translator[ReqT, SpanT any] interface {
    RequestBody(raw []byte, body *ReqT, onRetryOrForce bool) (headers, mutatedBody, err)
    ResponseHeaders(headers) (newHeaders, err)
    ResponseBody(respHeaders, body io.Reader, endOfStream bool, span SpanT)
        (newHeaders, mutatedBody, tokenUsage metrics.TokenUsage, responseModel, err)
    ResponseError(respHeaders, body) (newHeaders, mutatedBody, err)
}
```

Note `ResponseBody` returning `tokenUsage` and `responseModel` — token metering is a
*byproduct of translation*, not a separate parsing pass. **Streaming**: translators are
incremental SSE state machines (`openai_helper.go` etc.) that re-emit translated event
streams chunk-by-chunk (including cross-dialect streaming, e.g. OpenAI chunks → Anthropic
SSE events with thinking/text block bookkeeping), accumulating cumulative usage as chunks
arrive. Gzip-encoded streams are handled by buffering compressed bytes and re-decompressing
from the start each chunk (`processor_impl.go: decodeStreamingContent`) — a known cost of
doing this in ExtProc.

**Verdicts:**

- *Unified OpenAI-compatible (+ native Anthropic) ingress with a (in,out) translator
  matrix* — **borrow** the model. For Flowplane in Rust: a `Translator` trait with exactly
  this four-method shape, `ResponseBody` returning `TokenUsage`. Start with OpenAI ingress;
  add Anthropic ingress early (it's what MCP-centric agent clients increasingly speak).
- *`prefix` field to absorb the OpenAI-compatible long tail* — **borrow** verbatim; it
  converts "supporting 12 more providers" into one config field.
- *Doing translation in ExtProc as a separate process* — **adapt**. AIGW is forced into
  ExtProc because EG owns Envoy config. Flowplane *is* the control plane and already ships
  ExtProc infrastructure for learning; the AI data path should be a Flowplane-owned
  ExtProc/sidecar service co-located with Envoy. Consider Envoy `dynamic_modules` (Rust SDK;
  AIGW's own issue tracker points this direction **[src]** comment in `filterapi/`) later to
  remove the extra hop, but ExtProc first — it's proven and we have the plumbing.
- *Forcing body buffering for translation on every AI request* — accept as inherent;
  mitigations (pass-through when ingress==egress schema and no mutation needed) are visible
  in AIGW's passthrough processor **[src]** — **borrow** that fast path.

### 2.3 Credential management

**[src]** (`backendsecurity_policy.go`, `internal/controller/rotators/`,
`internal/filterapi/filterconfig.go`):

- Six credential types, one per `type` discriminator: `APIKey` (→ `Authorization` header),
  `AnthropicAPIKey` (→ `x-api-key`), `AzureAPIKey` (→ `api-key`), `AWSCredentials` (static
  credentials-file secret, **or** OIDC federation: controller exchanges an OIDC token at STS
  for temporary keys, **or** SDK default chain — env vars, EKS Pod Identity, IRSA, IMDS),
  `AzureCredentials` (client secret or OIDC exchange via Entra ID), `GCPCredentials`
  (service-account key file or Workload Identity Federation with optional service-account
  impersonation; carries `projectName`/`region` used in Vertex URL templates).
- **Rotation**: controller-side `rotators` refresh exchanged credentials before expiry
  (`preRotationWindow`), writing back into K8s Secrets; expiry tracked via secret
  annotations.
- **Delivery**: the controller renders the *literal* key/token into the ExtProc's filter
  config (`filterapi.BackendAuth{ APIKey{Key}, AWSAuth{CredentialFileLiteral}, ... }`) —
  i.e. secrets travel inside the (mounted/pushed) filter configuration, and the ExtProc
  signs/injects per request (SigV4 for Bedrock, Bearer/api-key headers otherwise).
- Request signing happens in the upstream filter so retries against a *different* backend
  get re-signed correctly.

**Verdicts:**

- *Per-provider auth strategy enum incl. cloud-credential exchange* — **adapt**. Flowplane
  v2 should ship `api_key` (header-name-aware: Authorization/x-api-key/api-key) day one, and
  design the credential record with a `type` discriminator so AWS SigV4/OIDC-exchange
  variants can be added without schema churn. Cloud federation machinery itself is
  K8s/cloud-specific; implement only when Bedrock/Vertex translators land.
- *Rotation as a control-plane background job with pre-expiry window + expiry metadata on
  the stored secret* — **borrow**; it maps directly onto Flowplane's secrets subsystem
  (backends incl. Vault, encrypted at rest, audited). Add `expires_at` + `rotate_after` to
  the secret model.
- *Secrets embedded literally in pushed filter config* — **reject**. Flowplane has SDS and
  an authenticated mTLS channel to its own data-plane services; AI credentials should be
  delivered out-of-band of the config snapshot (SDS-style or fetched by the AI ExtProc over
  its authenticated channel with in-memory caching), never serialized into config artifacts.
  This is a place to be *better* than prior art, and it matters for the team-tenancy story
  (credential values never appear in anything a team-scoped reader can dump).

### 2.4 Token-aware rate limiting, cost, and budgets

This is the most load-bearing mechanism; recorded exactly. **[src]**

**Metering (where token counts come from):**

1. The upstream ExtProc filter buffers/streams the response body through the translator;
   `ResponseBody(...)` returns cumulative `TokenUsage` parsed from the provider's own usage
   accounting — JSON `usage` for unary; for SSE streams, parsed incrementally from chunks.
2. For OpenAI streaming, usage only exists if the client asked for it — so the gateway
   **forces `stream_options.include_usage = true`** in the request body whenever the client
   didn't set it (and remembers that it did, to strip the extra usage chunk semantics
   correctly). `processor_impl.go` comment: body mutation is forced "to ensure that the
   token usage is calculated correctly without being bypassed."
3. Usage covers input/output/total plus cached-read, cache-creation, and reasoning tokens.
4. At `endOfStream` (and only then), the ExtProc emits **Envoy dynamic metadata** under
   namespace **`io.envoy.ai_gateway`**: one numeric value per configured `llmRequestCost`
   entry (raw token type or CEL-computed), evaluated against route-level costs first, then
   GatewayConfig globals (`buildDynamicMetadata`, `processor_impl.go:547,788`).

**Enforcement (how counts hit limits):** EG's global rate limit (Envoy RLS + Redis) with
per-rule cost **[src example `examples/token_ratelimit/`]**:

```yaml
rateLimit.global.rules:
- clientSelectors: [{headers: [{name: x-tenant-id, type: Distinct}]}]
  limit: {requests: 10000, unit: Hour}      # "requests" is really the token budget
  cost:
    request:  {from: Number, number: 0}     # request path: check budget only, charge nothing
    response: {from: Metadata, metadata: {namespace: io.envoy.ai_gateway, key: llm_input_token}}
```

So: budget checked (≥1 available?) at request time, **decremented post-response by actual
usage**. Budgets can go negative on the last request — accepted semantics. Separate buckets
per token type = separate rules with different metadata keys. Per-tenant isolation =
`Distinct` selector on a caller-supplied header (`x-tenant-id`) — note this is
*convention, not identity*.

**First-party quotas (newer layer):** `QuotaPolicy` **[src]** attaches to AIServiceBackends:
`serviceQuota` + `perModelQuotas[]`, each `{costExpression: CEL over token vars, quota:
{limit, duration: 1s|1m|1h|1d sliding window}, bucketRules[] with clientSelectors,
shadowMode}` — `shadowMode` evaluates and meters but never enforces (dry-run for budget
rollout). The companion proposal 009 ("quota-aware routing") **[src docs/proposals]** runs
the rate-limit filter in **QuotaMode** at router level: it checks quota for *all* candidate
backends, never rejects, and writes `quotaModeViolations` into dynamic metadata; the router
ExtProc then picks the highest-priority backend with remaining quota (PT-vs-on-demand
spillover) — quota-exhaustion failover decided *upfront*, not by retry.

**Verdicts:**

- *Response-body-derived usage → dynamic metadata → generic rate-limit cost* — **borrow**
  wholesale. Flowplane implementation: AI ExtProc emits usage; `flowplane-rls` grows
  `cost.response.from_metadata` descriptors. The decomposition (LLM knowledge in the filter,
  budget math in the RLS) is exactly right.
- *Forcing `include_usage` on streams* — **borrow**; without it streaming traffic silently
  escapes metering.
- *Token-type vocabulary incl. cached/cache-creation/reasoning + CEL cost expressions* —
  **borrow** the vocabulary; **adapt** CEL: in Rust, use `cel-rust` or start with a fixed
  weighted-sum form (`Σ wᵢ·tokenᵢ`, weights per model/backend) which covers every CEL example
  AIGW ships, and add expressions later. Price-per-model tables (see LiteLLM §3.1) fill the
  same role with less machinery.
- *Sliding-window quotas with `shadowMode`* — **borrow** shadow mode (perfect fit for
  Flowplane's approval/dry-run culture); window mechanics live in flowplane-rls.
- *Check-then-settle budget semantics (request charges 0, response settles actuals)* —
  **borrow**, and document the overdraft property honestly. Optionally add an estimated
  pre-charge (request-size heuristic) reconciled at settle time — none of the surveyed
  gateways do this; cheap differentiator for hard budget caps.
- *Tenancy by caller-set header* — **reject**. Flowplane derives the descriptor from
  authenticated identity (team/token), not a spoofable header; budgets become first-class
  team/principal-scoped records in Postgres with the RLS enforcing them.
- *Quota-aware upfront routing (QuotaMode)* — **adapt later**: v2 ships priority+retry
  failover first; the "ask RLS about all candidates, route to first with budget" pattern is
  the right design for provisioned-throughput spillover when needed.

### 2.5 Failover, fallback, retries, health

**[src + docs]** Layered, mostly delegated:

- **Within a rule**: `backendRefs` weights (traffic split) + `priority` (0 = primary, 1+ =
  fallback tiers, mapped to Envoy endpoint priority levels; overrides EG Backend `fallback`).
- **Retry across tiers**: EG BackendTrafficPolicy on the generated HTTPRoute —
  `numRetries`, **`numAttemptsPerPriority`** (after N attempts at priority p, move to p+1),
  `retryOn.httpStatusCodes` + triggers (connect-failure, retriable-status-codes), per-retry
  backoff/timeout (`examples/provider_fallback/`).
- **Retry correctness**: on retry the upstream ExtProc *re-runs* request translation and
  auth for the newly selected backend (`forceBodyMutation = onRetry()`), so OpenAI→Bedrock
  fallback mid-retry actually works. This subtlety is easy to miss and essential.
- **Model fallback on one provider** ("model virtualization" **[docs]**): two
  AIServiceBackends pointing at the same provider with different `modelNameOverride`s at
  different priorities — request for `gpt-5-nano` retries as `gpt-5-nano-mini`.
- **Health**: no LLM-specific health checking; standard EG/Envoy active health checks +
  passive outlier detection via BTP, plus quota-exhaustion-as-unhealthy via QuotaMode
  (§2.4). Nothing measures provider quality (latency percentiles per model, error classes)
  for routing — that's LiteLLM/Portkey territory (§3).

**Verdicts:** **borrow** priority-tiers + attempts-per-priority + retriable-status-codes as
the failover vocabulary (Flowplane clusters already model Envoy priorities; the work is the
re-translate/re-sign-on-retry contract in the AI filter — make that an explicit requirement).
**Borrow** model virtualization (falls out of `modelNameOverride` + priority for free).
**Adapt** health: add passive provider health (429/5xx/timeout rates per provider backend,
surfaced in CP status & MCP diagnostics) — Flowplane's learning pipeline already consumes
ALS data, so provider scorecards are nearly free, and none of the Envoy-family gateways have
them.

### 2.6 Multitenancy and isolation posture

**[src]** AIGW is K8s-native: isolation = namespaces; cross-namespace backendRefs require
Gateway API `ReferenceGrant`; one BackendSecurityPolicy per backend; rate-limit tenancy via
the `x-tenant-id` header convention (§2.4); no notion of org/team/user, no per-tenant
credential visibility model, no per-tenant budget object — these are assembled by the
operator from namespaces + selectors. The `aigw` standalone CLI exposes
`AIGW_TENANT_ID`/`AIGW_SESSION_ID` env conventions **[docs `cmd/aigw/README.md`]**,
confirming tenancy is a labeling convention, not enforced identity.

**Verdict:** this is Flowplane's structural advantage, not a gap to copy. **Adapt** the
*shape* of what they leave to convention into first-class Flowplane records: AI providers
and credentials are team-scoped resources (existing model); budgets attach to
org/team/principal; the metering descriptor is derived from the authenticated data-path
identity that Flowplane's filter chains already establish at config time. ReferenceGrant ≈
Flowplane's existing cross-team grant semantics — no new mechanism needed.

### 2.7 Operational posture

**[src + docs]**

- **Deployment**: controller Deployment + mutating webhook that injects the ExtProc
  container (sidecar mode supported, ordered to shut down after Envoy) into EG-managed
  proxy pods; EG must be configured with AIGW's extension server. Three binaries:
  `controller`, `extproc`, `aigw`.
- **Upgrade story**: the filter config carries a `Version` + `UUID`; if the ExtProc binary
  version mismatches the config version (rolling upgrade skew), the ExtProc **keeps serving
  the last good config** and refuses the new one (`filterapi.Config` doc comment). Simple,
  effective config-skew safety.
- **Observability**: OTel GenAI semantic conventions — metrics `gen_ai.client.token.usage`,
  `gen_ai.server.request.duration`, `gen_ai.server.time_to_first_token`,
  `gen_ai.server.time_per_output_token` with attributes `gen_ai.operation.name`,
  `gen_ai.provider.name`, `gen_ai.original.model`, `gen_ai.request.model`,
  `gen_ai.response.model`, `gen_ai.token.type` (`internal/metrics/genai.go`); OTel tracing
  spans per LLM call; token-latency metrics computed in-stream. Access logs can include
  cost metadata.
- **Standalone mode**: `aigw run` embeds EG with its file provider, downloads Envoy, runs
  ExtProc in-process — full AI gateway on a laptop with no K8s, auto-generating config from
  `OPENAI_API_KEY`/`OPENAI_BASE_URL` if none exists **[src cmd/aigw]**.

**Verdicts:** **borrow** the GenAI OTel semconv names verbatim (instant Grafana/vendor
dashboard compatibility; Flowplane already ships OTel). **Borrow** version-gated filter
config (Flowplane AI filter should reject config from a mismatched CP version and keep last
good — composes with xDS ACK/NACK reporting). **Adapt** standalone DX: `flowplane init` +
`flowplane expose` already cover this; add the "zero-config from env vars" trick as
`flowplane ai expose --provider openai` sugar. TTFT/inter-token latency metrics — **borrow**
(needed for provider scorecards in §2.5 anyway).

### 2.8 MCPRoute (convergent evolution worth noting)

**[src]** (`api/v1alpha1/mcp_route.go`, proposal 006): AIGW added an `MCPRoute` kind — a
Streamable-HTTP MCP endpoint (default `/mcp`) that aggregates multiple backend MCP servers,
with per-backend `toolSelector` filtering, per-backend and route-level security policies
(incl. OAuth token exchange, proposals 010/011), implemented in the same ExtProc
(`internal/mcpproxy`). This validates Flowplane's thesis from the opposite direction (an
Envoy gateway growing MCP) — but it is MCP *proxying* only: no tool generation from specs,
no control-plane-as-MCP-server. Flowplane should track its security-policy work
(token exchange per backend MCP server) as prior art for the gateway-side MCP filter, and
**borrow** the `toolSelector` (allowlist of exposed tools per upstream) idea into
Flowplane's `api_*` tool projection.

---

## 3. Adjacent art (one stealable idea each)

All claims in this section are **[web]**, from each project's official docs (URLs were
verified by fetch on 2026-06-12).

**LiteLLM.** A Python proxy exposing one OpenAI-compatible API over a YAML `model_list`
(public `model_name` → provider model + keys + tpm/rpm); router strategies
`simple-shuffle`, `least-busy`, `usage-based-routing`, `latency-based-routing`, with
`fallbacks`, `allowed_fails`, and `cooldown_time` (bench a failing deployment for N
seconds). Governance is built on **virtual keys** minted by the proxy, each carrying
allowed models, rate limits, expiry, and budgets: `max_budget` (USD) + `budget_duration`
(reset window) attachable at key/user/team/model level (docs.litellm.ai: routing,
virtual_keys, users). **Steal:** the community price map
`model_prices_and_context_window.json` (per-model `input_cost_per_token` /
`output_cost_per_token` / `max_tokens` / provider, overridable per deployment) — Flowplane
budgets should support a currency layer via exactly such a per-model price table on top of
raw token budgets; and the `max_budget + budget_duration` pair as the budget primitive,
mapped onto Flowplane teams/tokens instead of proxy-minted keys.

**Portkey / OpenRouter.** Portkey's **Configs** are recursive JSON policies:
`strategy.mode ∈ {fallback, loadbalance, conditional}` over `targets[]`, where fallback
triggers on configurable `on_status_codes` (429/5xx), loadbalance uses weights, conditional
matches request metadata, and any target may nest another config; `retry` and caching attach
at any level (portkey.ai Configs docs). OpenRouter exposes per-request provider preferences:
`provider.order`, `allow_fallbacks`, `sort: price|throughput|latency`, `max_price`, and
model-suffix shortcuts `:nitro` (throughput) / `:floor` (price), plus an ordered `models[]`
fallback array (openrouter.ai routing docs). **Steal:** Portkey's single recursive
strategy/targets tree — one schema expressing fallback, LB, and conditional routing
composition — as the shape for Flowplane's AI-route policy document; and OpenRouter's tiny
`sort`/`max_price` surface as the eventual API for cost/latency-aware backend selection
(feeds from the provider scorecards in §2.5).

**Kong AI Gateway.** Provider abstraction as plugins: **ai-proxy** (one route = one
OpenAI-style facade over a provider) and **ai-proxy-advanced** (multi-target balancing:
weighted round-robin, consistent-hash, least-connections, lowest-latency, lowest-usage
(token count/cost), **semantic** (embed the prompt, route to the nearest target description
in a vector store), and priority tiers), plus an orthogonal plugin family —
**ai-rate-limiting-advanced** (limits on provider-reported token counts with per-model
costs), ai-semantic-cache, ai-prompt-guard / ai-semantic-prompt-guard, prompt decorator
(developer.konghq.com). **Steal:** the decomposition of AI policy into orthogonal,
independently attachable units (Flowplane's filter-attachment model is the same shape — AI
metering, guardrails, caching should be separate filters, not one monolith). Semantic
routing/caching: noted, deliberately out of scope for v2 (pulls a vector DB into the data
path).

**kgateway (ex-Gloo).** Models an LLM provider as a Gateway API `Backend` CR with
`spec.type: AI` and an `llm` block (OpenAI, Anthropic, Gemini, Vertex, Bedrock, Azure),
referenced from a *plain HTTPRoute*; all AI behavior (transformation, prompt guards with
regex/webhook + response masking, prompt enrichment, failover) runs in its own ExtProc
extension. Notably it now ships a second, Rust-based data plane (**agentgateway**) aimed at
LLM/MCP/agent traffic (kgateway.dev docs). **Steal:** "AI provider is just a typed Backend +
ordinary route" — confirmation that Flowplane should model providers as flavored Clusters
rather than a parallel routing universe; and prompt guards as separately attached policy
rather than inline route config. The agentgateway move also corroborates Rust-on-the-AI-data-path
as a viable direction for Flowplane's own filter.

---

## 4. Where Flowplane is differentiated

**The learning loop.** None of the surveyed systems observe traffic to produce
configuration. EG/AIGW are strictly feed-forward: a human writes resources, the pipeline
compiles them, status flows back — the gateway never learns. Flowplane's
observe→learn→spec→tools loop (ALS/ExtProc capture → schema inference with confidence →
OpenAPI 3.1 → generated MCP tools), and the v2 traffic-first direction (capture unmatched
traffic → propose routes/clusters under approval + dry-run), has no analogue in any of
these projects. The AI-gateway capability *compounds* this: the same capture path that
infers REST schemas can meter prompts/models/usage shapes per team, learn which models a
workload actually uses, and propose AI-route consolidation or budget policies from observed
spend — "learned policy proposals" rather than learned schemas only. AIGW's metering filter
shows how to extract the signals; only Flowplane has somewhere institutional to put them.

**MCP-native operation.** AIGW's MCPRoute proxies MCP backends through the data plane;
Kong/LiteLLM expose admin APIs an agent could call. Flowplane is the only system in this
survey where the *control plane itself* is an MCP server with a permission-gated tool
surface (`cp_*`, `ops_*`) and where learned upstream APIs become callable tools (`api_*`)
behind the same authorization gate. Combined with the AI gateway capability, this closes a
loop nobody else has: an agent can provision an LLM route, set a token budget, send traffic
through it, and read back its own usage and provider health — through one authenticated
surface. The K8s-native projects structurally cannot follow quickly: their "API" is the K8s
API server, so agent operation means RBAC + YAML, and their status model (conditions on
CRDs) is far less legible to an agent than Flowplane's typed diagnostics tools.

**Identity-anchored AI tenancy.** Smaller, but real: every surveyed gateway does token
budgeting against caller-supplied labels (headers, virtual keys at best). Flowplane already
has org/team/principal identity on both the operator path (OIDC) and the data path (SPIFFE
mTLS), so token budgets and provider-credential visibility can be enforced against
*authenticated* identity in one system of record — closer to LiteLLM's virtual-key economy
than to AIGW, but with real authn and an audited Postgres ledger underneath.

---

## 5. Summary table

| Idea | Source | Verdict | Rationale |
|---|---|---|---|
| Metadata-driven rate-limit cost (`cost.response.from: Metadata`) | EG `ratelimit_types.go` [src] | **Borrow** | Token budgets without teaching the RLS about LLMs; extends flowplane-rls cleanly |
| Explicit IR between domain model and Envoy protobuf | EG `internal/ir` [src] | **Borrow** | Testable seam; lets AI subsystem inject config without special-casing core builders |
| Reactive watchable-map pipeline | EG `internal/message` [src] | **Adapt** | Keep staged pure functions + dirty-set rebuild; skip pub/sub machinery |
| Status conditions: Accepted / Programmed / ResolvedRefs | EG status pkg [src] | **Adapt** | Three-condition core as Postgres-backed resource status; skip ancestor matrix |
| Policy attachment with explicit merge semantics + conflict status | EG [src] | **Adapt** | Named merge rules for Flowplane's scope levels; conflicts always surfaced |
| EnvoyPatchPolicy (raw xDS JSONPatch) | EG [src] | **Reject** | Breaks deterministic-projection contract and team isolation |
| Label-based targetSelectors | EG [src] | **Reject** | Indirection hurts agent/operator legibility |
| Route / backend / security-policy resource split | AIGW API [src] | **Borrow** | Credentials as separate attachable, rotatable objects |
| Model-from-body → routing header (`x-ai-eg-model`) | AIGW [src] | **Borrow** | Model routing via ordinary route matching |
| `modelNameOverride` + weight + priority per backendRef | AIGW [src] | **Borrow** | Minimal complete canary/AB/failover/model-fallback vocabulary |
| (ingress,egress) translator matrix + `prefix` for OpenAI-compatibles | AIGW translator [src/docs] | **Borrow** | Few real translators + one field absorbs the provider long tail |
| Translator returns TokenUsage from ResponseBody (incl. SSE) | AIGW [src] | **Borrow** | Metering as a byproduct of translation, one body pass |
| Force `stream_options.include_usage` on streams | AIGW [src] | **Borrow** | Streaming must not escape metering |
| Token-type vocabulary incl. cached/cache-creation/reasoning | AIGW [src] | **Borrow** | Matches real provider billing models |
| CEL cost expressions | AIGW [src] | **Adapt** | Start with weighted-sum + per-model price table; add expressions later |
| QuotaPolicy `shadowMode` | AIGW [src] | **Borrow** | Dry-run budgets; fits Flowplane approval culture |
| Quota-aware upfront routing (QuotaMode) | AIGW proposal 009 [src] | **Adapt later** | Right design for PT spillover; not v2-critical |
| Tenancy via `x-tenant-id` header | AIGW examples [src] | **Reject** | Derive descriptors from authenticated team/principal identity |
| Secrets literal in pushed filter config | AIGW filterapi [src] | **Reject** | Deliver via SDS-style authenticated channel; never in config artifacts |
| Credential rotation w/ pre-expiry window + expiry metadata | AIGW rotators [src] | **Borrow** | Maps onto Flowplane secrets subsystem |
| Retry re-translation/re-signing per newly selected backend | AIGW extproc [src] | **Borrow** | Cross-provider fallback is broken without it |
| `numAttemptsPerPriority` + retriable-status-codes failover | EG BTP / AIGW example [src] | **Borrow** | Clean tiered-failover semantics on existing Envoy priorities |
| GenAI OTel semconv metrics (`gen_ai.*`, TTFT, per-output-token) | AIGW metrics [src] | **Borrow** | Standard names = free dashboards |
| Version-gated filter config (keep last good on skew) | AIGW filterapi [src] | **Borrow** | Rolling-upgrade safety for the AI filter |
| Synthesized `/models` endpoint | AIGW [src] | **Borrow** | Self-describing gateway; feeds MCP tool inventory too |
| Generated user-visible intermediate resources (HTTPRoute) | AIGW [src] | **Reject** | Project internally + atomically instead |
| MCP `toolSelector` per backend | AIGW MCPRoute [src] | **Borrow** | Tool allowlisting for Flowplane's `api_*` projection |
| Provider health scorecards / quality-aware routing | (absent in Envoy family) | **Adapt** from LiteLLM/Portkey ideas | Passive per-provider health from data Flowplane already captures |
| Per-model price table → currency budgets | LiteLLM `model_prices_and_context_window.json` [web] | **Borrow** | Dollar budgets on top of token budgets; table is maintained upstream |
| `max_budget` + `budget_duration` budget primitive | LiteLLM [web] | **Adapt** | Attach to Flowplane team/principal records, not proxy-minted keys |
| Virtual keys as the budget anchor | LiteLLM [web] | **Reject** | Flowplane already has real identity; don't mint a parallel key economy |
| Recursive strategy/targets policy tree (fallback/LB/conditional) | Portkey Configs [web] | **Adapt** | Candidate shape for Flowplane's AI-route policy document |
| `sort: price\|throughput` + `max_price` selection | OpenRouter [web] | **Adapt later** | Needs provider scorecards first; small API, high leverage |
| Failing-deployment cooldown (`cooldown_time`) | LiteLLM [web] | **Borrow** | Cheap passive health on top of Envoy outlier detection |
| AI policy as orthogonal attachable filters (metering/guard/cache) | Kong plugin family [web] | **Borrow** | Matches Flowplane's filter-attachment model; avoid an AI monolith |
| Semantic routing / semantic cache | Kong [web] | **Reject** (v2) | Vector DB in the data path; out of scope |
| Provider = typed Backend + plain route | kgateway [web] | **Borrow** | Confirms "AI provider as flavored Cluster", not a parallel routing universe |
