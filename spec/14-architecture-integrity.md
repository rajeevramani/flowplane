# 14 — Architecture Integrity Rules

This is the standing rulebook for keeping Flowplane v2 from drifting into the coupling patterns
that v1 exposed. Read it before adding a new endpoint, CLI command, worker, xDS behavior, learning
feature, AI gateway feature, or packaging workflow.

Recommendation: use these rules as a lightweight design checklist for every S7.7+ checkpoint. Do
not create more framework around them yet; the discipline is in naming the domain owner, seam, and
mutation path before editing code.

## 1. Non-Negotiables

1. **V1 is reference material, not source code.**
   Borrow outcomes, workflows, and lessons. Do not port V1 internals or recreate V1 coupling.

2. **PostgreSQL is the product source of truth.**
   No CRDs, no orchestrator state, no generated files, no compose state, and no Envoy state become
   authoritative product state.

3. **Envoy is the only dataplane.**
   The CP configures Envoy out-of-band through xDS/SDS. Request traffic does not pass through the
   CP.

4. **No orchestrator dependency.**
   Bare metal, VMs, plain containers, ECS-class managed platforms, Nomad, Cloud Run-style
   environments, and Kubernetes must all be viable. K8s manifests can be packaging, never the
   architecture.

5. **Tenant isolation is by construction.**
   Org/team context is explicit or unambiguous; storage/service paths carry validated scope; xDS
   identity comes from certificate registry binding, not claims in `node.id` or metadata.

6. **All product mutations go through `fp-core::services`.**
   Surfaces and workers must not invent alternate write paths. Named read-side exceptions are
   allowed only when recorded in D-016 or a later decision.

7. **Subsystems meet through events and contracts, not private calls.**
   xDS, learning, AI, MCP, and operators should communicate through domain events, published
   service APIs, and generated contracts.

8. **Fail closed at security boundaries.**
   Production xDS is mTLS-or-off; partial TLS config fails boot; dev plaintext is explicit and
   gated; secret values are write-only over HTTP.

9. **Envoy admin is not an operator/product API.**
   Envoy admin endpoints stay loopback-local to the dataplane unit. Product diagnostics and
   operator workflows must use CP surfaces backed by persisted diagnostics, audit, outbox, and
   xDS state. Only `fp-agent` may scrape Envoy admin, and only to relay curated telemetry to the
   CP over the outbound diagnostics channel.

10. **Contracts must not drift.**
   REST, OpenAPI, CLI, and MCP declarations should come from the same source where possible, with
   parity tests pinning the surface.

11. **The V2 UX must improve V1.**
    V1 defines the user outcome; V2 defines the architecture and experience.

## 2. Domain Ownership

Each new capability must have one primary domain owner. Cross-domain behavior happens through
events or explicit service calls, not by sharing tables casually.

| Domain | Owns | Does not own | Primary crates |
| --- | --- | --- | --- |
| Identity and tenancy | orgs, teams, users, agents, memberships, grants, active org selection, authz decisions | gateway config semantics, xDS translation | `fp-domain`, `fp-core`, `fp-storage`, `fp-api` auth middleware |
| Gateway config | clusters, endpoints, route configs, virtual hosts, route rules, listeners, filter IR | Envoy stream state, learning observations, MCP serving | `fp-domain`, `fp-core`, `fp-storage`, `fp-api`, CLI |
| Dataplane identity/lifecycle | dataplane records, proxy certificates, cert issue/register/revoke, bootstrap contract, liveness facts | Envoy process supervision details outside local dev helpers | `fp-domain`, `fp-core`, `fp-storage`, `fp-api`, `fp-xds`, `fp-agent` |
| xDS delivery | domain snapshot to typed IR to Envoy protos, ADS/SDS, ACK/NACK, quarantine, diagnostics ingest | product mutations, learning inference, AI policy decisions | `fp-xds` plus read-side storage exceptions |
| Secrets | encrypted-at-rest secret values, metadata reads, rotation, SDS projection | provider-specific AI request logic, arbitrary config blobs | `fp-domain`, `fp-core`, `fp-storage`, `fp-api`, `fp-xds` SDS |
| API lifecycle and learning | ApiDefinition, route bindings, capture sessions, observations, SpecVersions, publish/review lifecycle, generated tool rows | xDS transport mechanics, MCP protocol serving | `fp-domain`, `fp-core`, future `fp-learning` |
| AI gateway | AI providers, AI routes, budgets, token usage, translator contracts, provider failover | generic OpenAPI learning semantics, MCP session transport | future `fp-ai`, `fp-xds` ExtProc host, `fp-core` |
| MCP | Streamable HTTP sessions, tool registry, cp/ops/api tool serving, tool authz | defining gateway resources outside core services | future `fp-mcp`, shared declarations, `fp-core` |
| Observability and audit | logs, traces, metrics, audit rows, request correlation | product state authority | owning crate emits; storage/audit repo persists |
| CLI and local workflows | human workflows, config contexts, output/error rendering, local dev wrappers | direct product writes, hidden product state | `flowplane` CLI as REST client; direct DB only for `db migrate` |

## 3. Required Seams

### 3.1 Surface Seam

REST, CLI, and MCP are surfaces. They validate transport-specific shape, resolve context, call
`fp-core::services`, and render responses. They do not own business invariants.

Allowed:
- `fp-api` request context reads listed in D-016.
- CLI local orchestration for dev-only processes, as long as product state is still created through
  REST.

Not allowed:
- CLI writing the product database.
- REST handlers directly changing product tables outside a named D-016 exception.
- MCP tools bypassing the same authz and service paths REST uses.

### 3.2 Service Seam

`fp-core::services` is the mutation boundary.

Every mutation should perform, in one transaction where applicable:
- authorization
- domain validation
- row changes
- audit write
- outbox event append

If a mutation cannot use this pattern, record the exception before implementing it.

### 3.3 Storage Seam

`fp-storage` owns SQL, migrations, repositories, and outbox persistence. Repositories are
scoped-by-construction where data is tenant-owned.

Rules:
- team-owned rows use `team_id`
- org/team relationships use real FKs
- mutable resources carry optimistic revisions
- no name-string foreign keys where a real FK is possible
- no unscoped tenant query API except named platform-admin paths

### 3.4 Event Seam

Outbox events are the integration seam between subsystems.

Use events for:
- xDS rebuilds
- certificate revocation propagation
- learning session state changes
- spec publish/tool regeneration
- AI budget/usage reactions
- audit/diagnostic fan-in where asynchronous

Do not make xDS, learning, AI, or MCP call each other's private internals to stay in sync.

### 3.5 xDS/IR Seam

The xDS boundary is:

```text
domain snapshot -> typed IR -> Envoy protos -> ADS/SDS stream
```

Rules:
- filters, learning capture, and AI gateway behavior enter through typed IR
- encoded output must be deterministic
- DB/read errors serve last good snapshots, never fallback config
- NACKs quarantine changed resources rather than wiping dataplanes
- mTLS certificate registry binding is the authorization source for dataplanes

### 3.6 Dataplane Seam

The DP unit is Envoy plus `fp-agent`.

Rules:
- DP connects outbound to CP over xDS-family gRPC
- CP never dials Envoy admin ports
- operator/product workflows must not require direct Envoy admin access
- agent may scrape Envoy admin on loopback and relay curated telemetry
- non-dev xDS requires mTLS
- generated bootstrap carries resolved addresses/ports/cert paths explicitly

### 3.7 Contract Seam

REST routes, OpenAPI, CLI commands, MCP tools, and authz `(resource, action)` declarations should
stay aligned.

Every new externally visible capability needs:
- an API path or a recorded reason it is CLI-local only
- OpenAPI coverage when REST exists
- CLI coverage in the same slice from S7 onward
- MCP coverage only in S11 or later
- tests that prevent route/document/CLI drift

### 3.8 Test Execution Seam

Tests should be parallel-safe by default. As the gateway, learning loop, and live Envoy coverage
grow, serializing broad suites will become a real development tax.

Rules:
- prefer pure unit tests and table-driven translator/domain tests for most behavior
- integration tests sharing PostgreSQL must generate unique org/team/resource names and avoid
  assumptions about global row counts
- tests needing a TCP port must bind port `0` or allocate a per-test unique port; fixed ports are
  only for documented manual runbooks
- tests needing singleton product state must serialize only the critical section, preferably via a
  named advisory lock or local mutex with a comment explaining the shared resource
- live Envoy/CP tests should be smoke-sized and feature-focused; one test should not become a
  full release rehearsal unless it is explicitly an E2E gate
- do not pass `--test-threads=1` for a suite unless the suite owns unavoidable global external
  state and the reason is documented

## 4. Feature Placement Checklist

Before implementing a new capability, answer these in the change description or design note:

1. What domain owns this capability?
2. Is it a mutation, read model, async projection, local workflow, or packaging helper?
3. Which crate owns the domain type?
4. Which `fp-core::services` function owns the mutation?
5. What audit row and outbox event are emitted?
6. Which surfaces expose it now: REST, CLI, MCP, or local-only?
7. Does it need xDS/IR translation, or only product state?
8. What tenant scope is required, and where is it enforced?
9. What happens on DB error, authz denial, partial TLS/cert config, or cross-tenant input?
10. What test proves the seam does not drift?
11. Is the test parallel-safe? If not, what exact shared resource forces serialization?

If the answers are unclear, stop and update the spec or decision log first.

## 5. Red Flags

These patterns usually mean the architecture is drifting:

- a handler writes SQL directly because it is "just one field"
- a CLI command changes product state without REST
- a worker polls another subsystem's tables instead of consuming events
- a feature stores duplicate JSON and relational projections as separate authorities
- a route, tool, or CLI command has its own authz mapping outside shared declarations
- `node.id`, Host headers, path strings, or email addresses are treated as isolation boundaries
- generated Envoy config depends on default ports that were not resolved into the artifact
- a local Docker/Compose assumption becomes the only supported deployment path
- learning, AI, or MCP tools become live config before review/publish gates
- diagnostics require CP inbound access to a dataplane
- operator docs or CLI commands depend on `curl :9901/config_dump` as the normal validation path

## 6. Current S7.7/S8 Application

For S8.1 API lifecycle foundation:
- `api_definitions` are the config-first roots for imported APIs, learned APIs, generated tools,
  and later MCP/API tool serving.
- `api_route_bindings` link APIs to existing gateway route scope by typed same-team FKs; they do
  not duplicate route config JSON or create a second routing authority.
- `spec_versions` are append-only content rows. Review/publish state in later S8 slices must point
  at versions or add explicit lifecycle records; it must not mutate a spec body in place.
- `api_tools` are generated data rows attached to a concrete spec version. They are not live MCP
  serving behavior until S11.
- Retention policy rows are team-owned and may be API-scoped; raw observation retention in S8.5
  must read from this policy rather than hard-coding learning cleanup.
- Tests for API lifecycle storage use unique tenant/resource names and row-level locking where
  concurrency matters, so they can run with the normal parallel suite.

For S8.2 API REST/CLI foundation:
- REST and CLI expose API lifecycle through the same service mutation path; CLI never writes API
  rows directly.
- OpenAPI import creates an append-only `SpecVersion` and generated `api_tools` rows in the same
  transaction as the API create.
- Generated tools remain inert product data until S11 MCP serving; no API tool can become live
  config without the later review/publish/serving gates.
- Route binding names existing gateway resources by typed IDs. V2 does not infer clusters,
  listeners, upstreams, or route topology from partial OpenAPI data.

For S7.7 core gateway parity:
- `expose` is a CLI/API workflow over existing gateway services, not a separate config model.
- `dataplane bootstrap` wraps the existing bootstrap API and must keep dev/prod security explicit.
- `dataplane up/down`, if implemented, is local orchestration only; dataplane records/certs remain
  REST-backed product state.
- runbooks may describe Docker/Envoy commands, but they must not turn Docker host networking into
  an architectural requirement.

For S8 learning:
- `ApiDefinition` is the aggregate root for import, capture, learned specs, published specs, and
  generated tool rows.
- observations are data until reviewed/published.
- capture injection enters xDS through typed IR and is scoped to team-owned routes/listeners.
- MCP tools are generated projections from published specs; serving them waits for S11.
