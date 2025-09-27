# Tasks: Control Plane API Authentication System

**Input**: Design documents from `/Users/rajeevramani/workspace/projects/flowplane/specs/001-control-plane-auth/`
**Prerequisites**: plan.md ✓, research.md ✓, data-model.md ✓, contracts/ ✓, quickstart.md ✓

## Format: `[ID] [P?] Description`
- **[P]**: Can run in parallel (different files, no dependencies)
- Include exact file paths in descriptions

## Phase 3.1: Setup & Database Foundation
- [x] T001 Audit existing codebase for authentication-related code stubs in src/ and identify conflicts with new auth module design
- [x] T002 Refactor existing src/auth/mod.rs to preserve JWT service and add personal access token support (rename to src/auth/jwt.rs, create new structure)
- [x] T003 Add missing authentication dependency argon2 to Cargo.toml (verify existing jsonwebtoken/validator/uuid entries remain unchanged)
- [x] T004 [P] Configure clippy and rustfmt rules for auth module in .clippy.toml
- [x] T005 Create database migration 20241227000001_create_auth_tokens_table.sql with personal_access_tokens and token_scopes tables
- [ ] T006 Extend src/storage/repository_simple.rs to support auth event types in audit_log table and emit auth.token.seeded event when bootstrap token is provisioned
- [x] T007 Add TokenRepository trait definition to src/storage/repository_simple.rs for personal access token operations (no persistence logic yet)

## Phase 3.2: Tests First (TDD) ⚠️ MUST COMPLETE BEFORE 3.3
**CRITICAL: These tests MUST be written and MUST FAIL before ANY implementation**

### Contract Tests [P] - API Endpoint Contracts
- [ ] T008 [P] Contract test POST /api/v1/tokens in tests/auth/contract/test_tokens_create.rs
- [ ] T009 [P] Contract test GET /api/v1/tokens in tests/auth/contract/test_tokens_list.rs
- [ ] T010 [P] Contract test GET /api/v1/tokens/{id} in tests/auth/contract/test_tokens_get.rs
- [ ] T011 [P] Contract test PATCH /api/v1/tokens/{id} in tests/auth/contract/test_tokens_update.rs
- [ ] T012 [P] Contract test DELETE /api/v1/tokens/{id} in tests/auth/contract/test_tokens_revoke.rs
- [ ] T013 [P] Contract test POST /api/v1/tokens/{id}/rotate in tests/auth/contract/test_tokens_rotate.rs

### Integration Tests [P] - End-to-End Flows
- [x] T014 [P] Integration test token creation and validation flow in tests/auth/integration/test_token_lifecycle.rs
- [x] T015 [P] Integration test authentication middleware with bearer tokens in tests/auth/integration/test_auth_middleware.rs
- [x] T016 [P] Integration test scope-based authorization in tests/auth/integration/test_authorization.rs
- [x] T017 [P] Integration test audit logging for auth events in tests/auth/integration/test_audit_logging.rs
- [x] T018 [P] Integration test token expiration and revocation in tests/auth/integration/test_token_security.rs

## Phase 3.3: Models & Validation (ONLY after tests are failing)

### Token Models
- [x] T019 Define `TokenStatus`, `PersonalAccessToken`, `NewPersonalAccessToken`, and `UpdatePersonalAccessToken` structs in `src/auth/models.rs`
- [x] T020 Add serde/chrono conversions and helper methods (`as_str`, `has_scope`, etc.) in `src/auth/models.rs`
- [x] T021 Introduce `AuthContext` and `AuthError` (with `thiserror`) in `src/auth/models.rs`
- [x] T022 Create unit tests in `tests/auth/unit/test_models.rs` covering status parsing, scope checks, and error display

### Validation Layer
- [x] T023 Implement `CreateTokenRequest` and `UpdateTokenRequest` DTOs in `src/auth/validation.rs`
- [x] T024 Add validation helpers for name format, scope format, optional fields in `src/auth/validation.rs`
- [x] T025 Write unit tests in `tests/auth/unit/test_validation.rs` verifying valid/invalid payloads and scope patterns

## Phase 3.4: Persistence Layer

### Schema Support
- [x] T026 Add row structs/converters for `personal_access_tokens` and `token_scopes` in `src/storage/repository_simple.rs`
- [x] T027 Implement `TokenRepository::create_token` (insert token + scopes, return hydrated model)
- [x] T028 Implement `TokenRepository::list_tokens` with paging
- [x] T029 Implement `TokenRepository::get_token` (single token + scopes, not found handling)
- [x] T030 Implement `TokenRepository::update_metadata` (partial updates + scope replacement)
- [x] T031 Implement `TokenRepository::rotate_secret` and `update_last_used`
- [x] T032 Implement `TokenRepository::find_active_for_auth` & `count_tokens`
- [x] T033 Add repository unit tests in `tests/auth/unit/test_repository.rs` covering all methods using in-memory SQLite
- [x] T034 Extend `AuditLogRepository` with helpers for auth events and call from `src/openapi/defaults.rs` to emit `auth.token.seeded`

## Phase 3.5: Services
- [x] T035 Implement Argon2 hashing/verification helpers in `src/auth/token_service.rs`
- [x] T036 Implement bootstrap token seeding (`ensure_bootstrap_token`) with audit event emission
- [x] T037 Implement lifecycle operations (`create_token`, `list_tokens`, `get_token`, `update_token`, `revoke_token`, `rotate_token`) in `TokenService`
- [x] T038 Implement authentication/session builder in `AuthService` (parse bearer, verify hash, check status/expiry, build `AuthContext`)
- [x] T039 Implement background cleanup task in `src/auth/cleanup_service.rs` (scan for expired tokens, emit audits)
- [x] T040 Add unit tests for services in `tests/auth/unit/test_token_service.rs` and `tests/auth/unit/test_auth_service.rs`

## Phase 3.6: Middleware
- [x] T041 Implement bearer extraction layer in `src/auth/middleware.rs`
- [x] T042 Implement scope-checking middleware that inspects `AuthContext` and required scopes
- [x] T043 Inject middleware into router/state in `src/api/routes.rs` and guard all CP API routes with appropriate scopes
- [x] T044 Add middleware unit tests in `tests/auth/unit/test_middleware.rs`

- [x] T045 Add new auth router/module `src/api/auth_handlers.rs` defining POST/GET/PATCH/DELETE/POST rotate endpoints
- [x] T046 Implement response serialization (hide hashed secret, return last-used metadata) in handlers
- [x] T047 Wire new routes into `src/api/routes.rs` with proper scope guards
- [x] T048 Update existing cluster/route/listener handlers to require scopes (`clusters:read`, `clusters:write`, etc.)
- [x] T049 Update Swagger/OpenAPI (`src/api/docs.rs`) with security scheme, auth-tagged endpoints, and error models
- [x] T050 Replace contract tests (T008-T013) with real assertions hitting the new handlers via `axum-test`

## Phase 3.8: CLI & Operational Tooling
- [x] T051 Add `src/cli/auth.rs` module with `create-token`, `list-tokens`, `revoke-token`, `rotate-token` commands
- [x] T052 Wire new subcommands into `src/cli.rs` and ensure `main.rs` can bootstrap CLI auth flows
- [x] T053 Document bootstrap token retrieval and CLI usage in `docs/token-management.md` (new file)

## Phase 3.9: Observability & Security Hardening
- [x] T054 Emit structured audit events for each handler/service action (create/update/revoke/rotate/authenticate)
- [x] T055 Add tracing spans/log fields so token ID/correlation IDs appear in logs (update `token_service.rs` and middleware)
- [x] T056 Add metrics counters/gauges for auth events in `src/observability/metrics.rs`
- [x] T057 Add property-based tests for validation (proptest) in `tests/auth/unit/test_token_properties.rs`
- [x] T058 Add timing-attack regression tests in `tests/auth/unit/test_security.rs`
- [x] T059 Add load test harness in `tests/auth/integration/test_concurrent_auth.rs` to validate concurrency behaviour

## Phase 3.10: Documentation & Final Validation
- [x] T060 Update `docs/authentication.md` with PAT/JWT setup, scope tables, and error responses
- [x] T061 Update `docs/api.md` to include new endpoints and example requests/responses
- [x] T062 Update `README.md` with quick start instructions for securing the CP API
- [x] T063 Update `quickstart.md` flow to include token creation, bootstrap verification, and `auth.token.seeded` audit check
- [x] T064 Run full test suite, confirm coverage targets (>90% for auth modules)
- [x] T065 Verify performance budget: token validation <10ms, CP endpoints <100ms under auth middleware
- [x] T066 Conduct security review: ensure secrets never logged, error messages sanitized, scopes enforced everywhere

## Phase 3.11: Cleanup & Consolidation
- [x] T067 Review T001 audit outcomes and delete/replace stubbed auth code
- [x] T068 Remove temporary scaffolding/tests introduced for TDD once real implementations land
- [x] T069 Ensure all modules export minimal public surface (re-export only necessary types)
- [x] T070 Final linting (`cargo fmt`, `cargo clippy -D warnings`) and `cargo doc` review before release

## Dependencies
**Setup Foundation**:
- T001 (audit) must complete before T002-T007 (project structure setup)
- T002-T007 must complete before any other tasks (database and project structure)

**TDD Requirements**:
- T008-T018 (all tests) MUST complete before T019 onward (implementation phases)
- Tests must FAIL initially to ensure TDD compliance

**Cleanup Phase**:
- T067-T070 (cleanup) should run after T060-T066 (final validation) to remove obsolete code

**Implementation Order**:
- T019-T025 (models & validation) before T026-T034 (persistence)
- T026-T034 (persistence) before T035-T040 (services)
- T035-T040 (services) before T041-T044 (middleware)
- T041-T044 (middleware) before T045-T050 (API integration)
- T045-T050 (API) before T051-T053 (CLI/tooling)
- T051-T053 before T054-T066 (observability, docs, validation)

**Parallel Execution Safe**:
- All tasks marked [P] in Phases 3.1–3.2 can still run simultaneously
- Contract tests (T008-T013) can all run in parallel
- Integration tests (T014-T018) can all run in parallel
- Within Phase 3.3, modelling tasks (T019-T022) can run in parallel; validation tasks (T023-T025) depend on 3.3 completion
- Repo method implementations (T026-T033) should follow the listed order but unit tests (T033) can run once CRUD complete

## Parallel Example
```
# Launch contract tests together:
Task: "Contract test POST /api/v1/tokens in tests/auth/contract/test_tokens_create.rs"
Task: "Contract test GET /api/v1/tokens in tests/auth/contract/test_tokens_list.rs"
Task: "Contract test GET /api/v1/tokens/{id} in tests/auth/contract/test_tokens_get.rs"
Task: "Contract test PATCH /api/v1/tokens/{id} in tests/auth/contract/test_tokens_update.rs"
Task: "Contract test DELETE /api/v1/tokens/{id} in tests/auth/contract/test_tokens_revoke.rs"
Task: "Contract test POST /api/v1/tokens/{id}/rotate in tests/auth/contract/test_tokens_rotate.rs"

# Launch model tasks together:
Task: "Define token structs in src/auth/models.rs"
Task: "Add AuthContext/AuthError in src/auth/models.rs"
Task: "Implement validation helpers in src/auth/validation.rs"
Task: "Write validation unit tests in tests/auth/unit/test_validation.rs"
```

## Existing Code Integration Notes

**T002 Refactoring Strategy**: The existing `src/auth/mod.rs` contains JWT service and Role types that should be preserved:
- Move existing `AuthService` to `src/auth/jwt.rs`
- Preserve `Claims`, `Role` enum, and JWT functionality
- Create new `src/auth/mod.rs` that exports both JWT and personal access token functionality
- New structure coexists with existing JWT code - no functionality deletion
- Ensure public re-exports remain compatible so current callers of `auth::AuthService` keep compiling without changes

**Migration Integration**: Current migrations follow timestamp pattern (20241201000001_*, 20241201000002_*, etc.)
- Next available timestamp: 20241227000001_create_auth_tokens_table.sql
- Maintains proper sqlx migration ordering

## Notes
- All tasks include exact file paths for implementation
- TDD strictly enforced: tests must fail before implementation
- Constitutional compliance: security by default, stability preserved, comprehensive testing
- Backward compatibility maintained: existing APIs work unchanged
- Performance targets: <10ms token validation, <100ms API responses
