# Tasks: Platform API Abstraction

**Input**: Design documents from `/Users/rajeevramani/workspace/projects/flowplane/specs/004-platform-api-abstraction/`
**Prerequisites**: plan.md (required), research.md, data-model.md, contracts/platform-api.yaml, quickstart.md

## Execution Flow (main)
```
1. Review plan.md & spec.md to confirm MVP scope (POST /v1/api-definitions, POST /v1/api-definitions/{id}/routes, bootstrap delivery)
   → Note existing code layout: migrations/, src/storage/repository_simple.rs, src/api, src/xds, src/validation
2. Load design documents:
   → data-model.md: ApiDefinition, ApiRoute, PathConfig, UpstreamConfig, bootstrap metadata
   → contracts/platform-api.yaml: API create + append routes contracts, response shapes
   → research.md: Axum patterns, repository usage, xDS builders, audit logging
3. Derive task groups aligned with architecture:
   → Database & storage updates
   → Failing integration/unit tests (TDD)
   → Validation & business rules
   → Platform API materializer & bootstrap generation
   → HTTP handlers & router wiring
   → xDS resource generation / OpenAPI pipeline reuse
   → Audit logging & documentation updates
4. Apply task rules:
   → Mark [P] only when files are disjoint and there is no dependency
   → Tests land (and fail) before implementation tasks run
   → Reference exact file paths for every task
5. Number tasks sequentially (T001, T002, …)
```

## Format: `[ID] [P?] Description`
- **[P]**: Task can be executed in parallel (different files, independent work)
- Always include full relative file path(s) in each description

## Path Conventions
- Database migrations live in `migrations/`
- Persistence layer changes extend `src/storage/repository_simple.rs` and exports in `src/storage/mod.rs`
- Platform API orchestration code resides in new `src/platform_api/` module
- HTTP handlers belong under `src/api/` with routing adjustments in `src/api/routes.rs`
- Validation lives in `src/validation/requests/` and `src/validation/business_rules/`
- xDS resource builders and state live in `src/xds/`
- Tests go in `tests/platform_api/`

## Phase 3.1: Database & Storage Setup

- [x] T001 Create migration `migrations/20250115000001_create_api_definitions.sql` with `api_definitions` and `api_routes` tables (team ownership, domain, listener isolation flag, TLS JSON, bootstrap_uri, versioning, timestamps, route match/override columns, indexes enforcing domain/path uniqueness)
- [x] T002 Add `ApiDefinitionData`, `ApiRouteData`, create/query/update helpers, and `ApiDefinitionRepository` to `src/storage/repository_simple.rs` (persist definitions, append routes, fetch by id, update bootstrap metadata)
- [x] T003 Update `src/storage/mod.rs` exports to expose new Platform API repository types

## Phase 3.2: Tests First (TDD) ⚠️ MUST COMPLETE BEFORE 3.3+

- [x] T004 Create integration test `tests/platform_api/test_create_api_definition.rs` covering successful `POST /v1/api-definitions` (bootstrap URI returned, DB rows created, listener assignment recorded)
- [x] T005 [P] Create integration test `tests/platform_api/test_append_route.rs` for `POST /v1/api-definitions/{id}/routes` (route appended without resubmitting existing paths, version bump expected)
- [x] T006 [P] Create integration test `tests/platform_api/test_rbac_enforcement.rs` ensuring team scope enforcement and audit stub interactions for Platform API endpoints
- [x] T007 [P] Create unit test `tests/platform_api/test_collision_detection.rs` asserting domain/path collision prevention behaviour
- [x] T008 [P] Add reusable request fixtures in `tests/platform_api/fixtures/api_definition_examples.json` to support integration tests

## Phase 3.3: Validation & Business Rules

- [x] T009 Implement request validation helpers in `src/validation/requests/api_definition.rs` (payload schemas, timeout bounds, TLS references, override enums)
- [x] T010 Update `src/validation/requests/mod.rs` to register Platform API validation module
- [x] T011 Implement collision/business-rule utilities in `src/validation/business_rules/api_definition.rs` (domain-path dedupe, listener isolation toggle semantics)
- [x] T012 Update `src/validation/business_rules/mod.rs` to expose new Platform API rules

## Phase 3.4: Platform API Materializer

- [x] T013 Create module scaffold `src/platform_api/mod.rs` and wire it into `src/lib.rs`
- [x] T014 Implement `src/platform_api/materializer.rs` translating `ApiDefinition` + `ApiRoute` into underlying cluster/route/listener records via existing repositories (shared listener default, optional isolation flag)
- [x] T015 Implement `src/platform_api/bootstrap.rs` to build and persist bootstrap artefact metadata (writes file/URI, updates repository, returns response details)
- [x] T016 Integrate OpenAPI defaults by reusing `openapi::defaults` helpers inside the materializer for route/cluster templates (MVP-FR12 requirement)

## Phase 3.5: HTTP Handlers & Routing

- [x] T017 Implement Axum handlers in `src/api/platform_api_handlers.rs` for create API and append route endpoints (bind validation, call materializer, format responses)
- [x] T018 Update `src/api/mod.rs` to expose `platform_api_handlers`
- [x] T019 Wire new routes and scope guards into `src/api/routes.rs` (POST `/api/v1/api-definitions`, POST `/api/v1/api-definitions/{id}/routes`, optional bootstrap download endpoint placeholder returning 501)

## Phase 3.6: xDS & Bootstrap Integration

- [x] T020 Extend `src/xds/resources.rs` (or new `src/xds/resources/platform_api.rs`) to build Envoy clusters/routes/listeners from `ApiDefinitionData` + stored overrides, ensuring generated resources carry ownership metadata
- [x] T021 Update `src/xds/state.rs` to load Platform API definitions, apply generated resources to caches, and bump version/notify watchers after materializer commits
- [x] T022 Update `src/xds/services/database.rs` to incorporate Platform API resources into ADS responses (fall back to existing tables if no definitions)
- [x] T023 Add helper functions/tests in `src/xds/resources.rs` ensuring weighted cluster & header override support per route

## Phase 3.7: Audit Logging & Documentation

- [x] T024 Add audit logging calls for create/append operations using `AuditLogRepository` in `src/platform_api/materializer.rs`
- [x] T025 Update `specs/004-platform-api-abstraction/quickstart.md` with REST payload examples and bootstrap download instructions matching Appendix B
- [x] T026 Add user-facing documentation page `docs/api/platform-api.md` (or update existing doc) describing endpoints, RBAC scopes, collision errors, bootstrap handling

## Phase 3.8: Integration & Verification

- [x] T027 Run database migration smoke test in `tests/config_integration.rs` ensuring new tables appear in schema snapshot
- [x] T028 Add end-to-end test `tests/platform_api/test_lifecycle.rs` covering create → append route → bootstrap fetch using fixtures
- [x] T029 Execute full test suite (`cargo test`) and document follow-up tasks for performance validation if sub-100ms target not met

## Phase 3.9: Listener Isolation Semantics

Design goal: allow teams to either share the default gateway listener or provision a dedicated listener per API definition with an explicitly chosen port.

Behavior summary
- When `listenerIsolation` is false (default):
  - Do not create a new Listener resource.
  - Append the API’s VirtualHost to the default gateway RouteConfiguration (`default-gateway-routes`) so it is served by `default-gateway-listener`.
  - Acceptance: only the default listener exists; new vhosts appear without a port conflict; LDS/RDS updates propagate dynamically.
- When `listenerIsolation` is true:
  - Payload MUST include a listener `port` and `bindAddress` (explicitly provided, no defaulting). Optional fields: `protocol` (`HTTP`|`HTTPS`, default `HTTP`) and `tlsConfig` when `protocol=HTTPS`.
  - Create a dedicated RouteConfiguration and a dedicated Listener that binds to the requested address:port with the standard HTTP filter chain.
  - Validate that the port is not already in use by any existing listener (including the default gateway listener).
  - Multiple API definitions MAY share the same isolated listener by specifying the same `listener.name`; subsequent uses must match the original listener’s address/port/protocol or return 409.

API payload (delta to contracts)
- `POST /api/v1/api-definitions` accepts:
  - `listenerIsolation: boolean` (default false)
  - `listener` (required iff `listenerIsolation` is true):
    - `name: string` (optional; if provided and existing, API will reuse that listener)
    - `port: integer` (1–65535, required)
    - `bindAddress: string` (required; no defaulting)
    - `protocol: string` (optional, `HTTP` or `HTTPS`)
    - `tlsConfig: object` (optional; shape aligns with existing TLS docs when `protocol` = `HTTPS`)

Validation and errors
- 400 Bad Request:
  - `listenerIsolation` is true and `listener.port` is missing or out of range.
  - `listenerIsolation` is true and `listener.bindAddress` is missing/invalid.
  - Unsupported `protocol` or invalid TLS reference for HTTPS.
- 409 Conflict:
  - Another listener already uses the requested address/port.
  - Reusing `listener.name` with mismatched address/port/protocol.
  - Domain collision across API definitions when `listenerIsolation` is false and virtual host domains would conflict.

Observability
- Emit structured logs for:
  - Route merge into `default-gateway-routes` (count of vhosts added, total vhosts).
  - Dedicated listener creation (name, address:port).
  - xDS cache refresh deltas (type, added, removed, version).

Migration notes
- Existing definitions default to `listenerIsolation = false` unless explicitly set.
- The default gateway listener’s port is controlled centrally; teams opting into isolation must specify their own port and own the lifecycle.
 - Domain uniqueness is enforced per-listener. Duplicate domains across different isolated listeners are allowed (host+port differentiate).

Tasks
- [ ] T030 Update contract `specs/004-platform-api-abstraction/contracts/platform-api.yaml` to include `listenerIsolation` and `listener` object (port/address/protocol/tls), with examples for both modes.
- [ ] T031 Add request validation rules in `src/validation/requests/api_definition.rs` for isolation mode and listener fields (require bindAddress when isolated); extend business rules in `src/validation/business_rules/api_definition.rs` for port conflicts, listener.name reuse invariants, and domain collisions (per-listener uniqueness).
- [ ] T032 Update materializer `src/platform_api/materializer.rs` to branch on `listenerIsolation`:
  - false → append VirtualHost into default gateway route set; no Listener created.
  - true → create per-definition RouteConfiguration and Listener at requested port.
- [ ] T033 Adjust xDS builders `src/xds/resources.rs` (and/or service wiring in `src/xds/services/database.rs`) to:
  - Merge non-isolated API routes into `default-gateway-routes`.
  - Emit dedicated Listener+RouteConfiguration for isolated APIs.
- [ ] T034 Add port conflict detection against existing listeners in repository (and reserved default listener) inside `src/validation/business_rules/api_definition.rs`.
- [ ] T035 Tests:
  - `tests/platform_api/test_isolation_shared.rs`: create with `listenerIsolation=false` → no new listener, vhost merged, traffic via default listener.
  - `tests/platform_api/test_isolation_dedicated.rs`: create with `listenerIsolation=true` and port → listener created, LDS/RDS reflect it, traffic binds to specified port.
  - `tests/platform_api/test_isolation_conflicts.rs`: port collision → 409; missing port when isolated → 400.
- [ ] T036 Docs: update `specs/004-platform-api-abstraction/quickstart.md` and `docs/api/platform-api.md` with payloads for both modes and verification steps (Envoy config dumps, curl checks).
- [ ] T037 xDS delivery: ensure ADS stream pushes updates on cache changes for both SOTW and Delta paths (logging ACK/NACK) – code in `src/xds/services/stream.rs`.
- [ ] T038 Platform API upstream TLS: add TLS inference and configuration to platform-generated clusters:
  - Infer TLS when any upstream endpoint uses port 443 and endpoint host is a hostname (set UpstreamTlsContext and SNI to host).
  - Allow explicit override in route `upstreamTargets` for TLS on non-443 ports and custom `serverName`.
  - Tests to verify SNI and transport_socket wiring for HTTPS upstreams.
- [x] T039 Transactional materialization (atomic create): make `POST /api-definitions` effectively atomic when `listenerIsolation=true`.
   - Pre-check listener conflicts by name and address:port; fail fast on mismatch.
   - Create definition and routes; if isolated-listener creation fails, delete the definition (cascades to routes) to avoid partial writes.
   - Follow-up (optional): convert to true SQL transaction once repo-level tx helpers are standardized.
 - [ ] T041 Bootstrap API: add `GET /api/v1/api-definitions/{id}/bootstrap` that returns an Envoy bootstrap document (YAML/JSON) instead of writing a file on disk.
   - Query: `format=yaml|json` (default yaml), `scope=all|team|allowlist` (default all), `allowlist[]=name` (repeatable), `includeDefault=true|false` (default false when `scope=team`).
   - Include `node.id` and `node.metadata` in the bootstrap with scope information.
   - Utoipa docs + examples added under the "platform-api" tag.
 - [ ] T042 xDS scoping by node metadata: filter LDS/RDS/CDS responses per client stream using `node.metadata`.
   - `scope=team`: return only listeners owned by the team; include default gateway if `include_default=true`.
   - `scope=allowlist`: return only listeners whose names appear in `listener_allowlist`.
   - `scope=all` (or no metadata): return current behavior (all listeners).
 - [ ] T043 Listener ownership tagging: annotate stored listener configuration with non-propagating `flowplaneGateway.team = <team>` metadata and ensure stripping prior to protobuf build (used only for control-plane scoping logic).

## Dependencies

**Critical Dependencies**:
- Database layer tasks (T001-T003) before tests relying on schema fixtures
- Tests (T004-T008) must exist and fail before implementation tasks (T009+)
- Validation/business rules (T009-T012) before materializer (T014-T016)
- Materializer (T014-T016) before handlers (T017-T019)
- xDS integration (T020-T023) after materializer but before final verification (T027-T029)
- Audit/docs (T024-T026) after handlers/materializer available

**Parallel Execution Blocks**:
- T005, T006, T007, T008 (independent test files/fixtures)
- T009 & T011 operate on different modules but share conceptual dependency; keep sequential unless refactoring needs parallel review
- T020, T021, T022 touch different xDS files but depend on materializer output (no [P])

## MVP Functional Requirements Coverage

**MVP-FR1** (Create API endpoint) → T004, T017, T019

**MVP-FR2** (Generate Envoy resources from definitions) → T014, T020, T021

**MVP-FR3** (Return bootstrap artefact) → T004, T015, T017

**MVP-FR4** (Shared listener defaults + isolation toggle) → T014, T021

**MVP-FR5** (RBAC enforcement) → T006, T017, T019, T024

**MVP-FR6** (Collision detection) → T007, T011, T017

**MVP-FR7** (Incremental route addition) → T005, T017, T014

**MVP-FR8** (Per-route overrides e.g. CORS templates) → T014, T020, T023

**MVP-FR9** (Persist API definitions/metadata) → T002, T014, T015

**MVP-FR10** (Audit logging) → T024

**MVP-FR11** (Documentation of generated artefacts & payloads) → T025, T026

**MVP-FR12** (Reuse existing OpenAPI pipeline components) → T016

## Validation Checklist

- [x] Every API contract in `contracts/platform-api.yaml` has a failing test (T004, T005, T006)
- [x] All data entities defined in data-model.md have storage tasks (T001, T002)
- [x] Tests precede implementation (Phase 3.2 before 3.3+)
- [x] Parallel markers only appear on independent files
- [x] Each task references a concrete file path
- [x] Tasks avoid introducing new parallel edits on same file
- [x] Constitution requirements satisfied (TDD, validation early, security-by-default, reuse existing components)
- [x] MVP functional requirements mapped to tasks
- [x] Tasks align with current project structure (no nonexistent `src/services` paths)

## Notes

- Respect TDD: do not implement handlers/materializer before integration tests fail
- Keep generated Envoy YAML internal; responses expose only metadata/URIs per spec
- When generating bootstrap artefacts, reuse existing config writer helpers or introduce a minimal abstraction in `src/platform_api/bootstrap.rs`
- Ensure new modules follow repository error handling patterns (structured `FlowplaneError`)
- Remember to update RBAC scope constants if new scopes are introduced for Platform API endpoints

### Decisions
- bindAddress: required when `listenerIsolation=true` (no default). Clients must provide an explicit bind address.
- HTTPS upstreams: currently not supported for Platform API clusters; tracked by T038 to add TLS inference and overrides. HTTPS listeners (TLS termination) remain out-of-scope for this phase and will be aligned with the TLS enablement spec.
- Shared isolated listeners: allowed via `listener.name`. Subsequent uses must match address/port/protocol or return 409.
- Domain uniqueness: enforced per-listener; duplicates allowed across different isolated listeners.
- Reserved ports: `10000` (default gateway listener). Additional reserved ranges can be added later if needed.
 - Bootstrap team scope: `includeDefault=false` by default.
 - Bootstrap method: `GET` endpoint with query parameters (no POST templating required now).
 - [x] T040 API Docs: add Platform API endpoints to Utoipa docs (`src/api/docs.rs`).
   - Document `POST /api/v1/api-definitions` and `POST /api/v1/api-definitions/{id}/routes`.
   - Document `GET /api/v1/api-definitions` and `GET /api/v1/api-definitions/{id}`.
   - Expose request/response schemas and examples (including isolated listener fields).
