
# Implementation Plan: Control Plane API Authentication System

**Branch**: `001-control-plane-auth` | **Date**: 2025-09-26 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/Users/rajeevramani/workspace/projects/flowplane/specs/001-control-plane-auth/spec.md`

## Execution Flow (/plan command scope)
```
1. Load feature spec from Input path
   → If not found: ERROR "No feature spec at {path}"
2. Fill Technical Context (scan for NEEDS CLARIFICATION)
   → Detect Project Type from file system structure or context (web=frontend+backend, mobile=app+api)
   → Set Structure Decision based on project type
3. Fill the Constitution Check section based on the content of the constitution document.
4. Evaluate Constitution Check section below
   → If violations exist: Document in Complexity Tracking
   → If no justification possible: ERROR "Simplify approach first"
   → Update Progress Tracking: Initial Constitution Check
5. Execute Phase 0 → research.md
   → If NEEDS CLARIFICATION remain: ERROR "Resolve unknowns"
6. Execute Phase 1 → contracts, data-model.md, quickstart.md, agent-specific template file (e.g., `CLAUDE.md` for Claude Code, `.github/copilot-instructions.md` for GitHub Copilot, `GEMINI.md` for Gemini CLI, `QWEN.md` for Qwen Code or `AGENTS.md` for opencode).
7. Re-evaluate Constitution Check section
   → If new violations: Refactor design, return to Phase 1
   → Update Progress Tracking: Post-Design Constitution Check
8. Plan Phase 2 → Describe task generation approach (DO NOT create tasks.md)
9. STOP - Ready for /tasks command
```

**IMPORTANT**: The /plan command STOPS at step 7. Phases 2-4 are executed by other commands:
- Phase 2: /tasks command creates tasks.md
- Phase 3-4: Implementation execution (manual or via tools)

## Summary
Implement comprehensive API authentication for Flowplane control plane using personal access tokens with scope-based authorization. Features include secure token generation (one-time reveal, hashed storage), granular permissions (clusters:read/write, routes:read/write, listeners:read/write), middleware-based request authentication, complete audit logging with correlation IDs, and extensible architecture for future JWT/OIDC federation. Maintains backward compatibility and follows constitutional principles for security, stability, and Rust excellence.

## Technical Context
**Language/Version**: Rust 1.75+
**Primary Dependencies**: Axum (HTTP server), SQLx (database), Argon2 (password hashing), jsonwebtoken (JWT support), tokio (async runtime), serde (serialization), validator (input validation), uuid (token IDs), thiserror (error handling)
**Storage**: SQLite (default), PostgreSQL (production) - extend existing database with auth tables
**Testing**: cargo test, proptest (property-based testing), integration tests with TestServer, contract tests for API endpoints
**Target Platform**: Linux server, macOS development
**Project Type**: single - Rust control plane application with authentication middleware
**Performance Goals**: <10ms token validation, <100ms auth endpoint responses, support 10k+ concurrent authenticated connections
**Constraints**: Zero breaking changes to existing API, backward compatibility required, secure by default, audit trail mandatory
**Scale/Scope**: Enterprise authentication system, support hundreds of tokens, thousands of daily auth operations, complete audit logging

## Constitution Check
*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**✅ I. Structured Configs First**: Personal access tokens will use structured models (TokenConfig, ScopeDefinition, AuthRequest) with strong typing and validation before database storage.

**✅ II. Validation Early**: All token creation, scope validation, and bearer token parsing will validate inputs at API boundary with descriptive errors before any processing.

**✅ III. Test-First Development**: Authentication middleware, token lifecycle operations, and scope checking will be developed using TDD with comprehensive test coverage (>90%).

**✅ IV. Idempotent Resource Building**: Token operations (create/revoke/validate) will be atomic and safe to retry. Auth state will be derivable from database without side effects.

**✅ V. Security by Default**: Secure token generation, Argon2 hashing, timing-attack protection, no secrets in logs, and proper HTTP status codes (401/403) by default.

**✅ VI. Application Stability**: Authentication is additive - existing API endpoints continue working unchanged. New auth middleware is optional/configurable initially.

**✅ VII. DRY Principle & Rust Excellence**: Common auth logic extracted into traits (Authenticator, Authorizer), proper error types with thiserror, leverage type system for compile-time guarantees.

## Project Structure

### Documentation (this feature)
```
specs/001-control-plane-auth/
├── plan.md              # This file (/plan command output)
├── research.md          # Phase 0 output (/plan command)
├── data-model.md        # Phase 1 output (/plan command)
├── quickstart.md        # Phase 1 output (/plan command)
├── contracts/           # Phase 1 output (/plan command)
│   ├── auth-tokens.yaml # Personal access token API
│   ├── auth-middleware.yaml # Authentication middleware spec
│   └── audit-events.yaml   # Audit logging contracts
└── tasks.md             # Phase 2 output (/tasks command - NOT created by /plan)
```

### Source Code (repository root)
```
src/
├── auth/                    # Authentication module (NEW)
│   ├── models.rs           # Token, Scope, Session models
│   ├── middleware.rs       # Axum authentication middleware
│   ├── service.rs          # Token lifecycle operations
│   ├── validation.rs       # Token and scope validation
│   └── mod.rs              # Module exports
├── storage/                 # Database layer (EXTEND)
│   ├── repositories/
│   │   ├── auth_tokens.rs  # Token storage operations (NEW)
│   │   └── audit_log.rs    # Audit logging (EXTEND)
│   └── migrations/
│       └── 004_auth_tokens.sql # Auth tables (NEW)
├── api/                     # REST API (EXTEND)
│   └── auth.rs             # Token management endpoints (NEW)
├── xds/                     # Existing xDS modules (NO CHANGES)
└── main.rs                  # Application entry point (EXTEND)

tests/
├── auth/                    # Authentication tests (NEW)
│   ├── contract/           # API contract tests
│   ├── integration/        # End-to-end auth flows
│   └── unit/               # Token validation, middleware tests
├── contract/                # Existing contract tests (EXTEND)
├── integration/             # Existing integration tests (EXTEND)
└── unit/                    # Existing unit tests (NO CHANGES)

migrations/                  # Database migrations (EXTEND)
└── 004_auth_tokens.sql     # Token and audit tables (NEW)
```

**Structure Decision**: Single project architecture extending existing Flowplane structure. Authentication functionality is added as a new `src/auth/` module with supporting database migrations and comprehensive test coverage. Existing modules remain unchanged to preserve application stability.

## Phase 0: Outline & Research
1. **Extract unknowns from Technical Context** above:
   - For each NEEDS CLARIFICATION → research task
   - For each dependency → best practices task
   - For each integration → patterns task

2. **Generate and dispatch research agents**:
   ```
   For each unknown in Technical Context:
     Task: "Research {unknown} for {feature context}"
   For each technology choice:
     Task: "Find best practices for {tech} in {domain}"
   ```

3. **Consolidate findings** in `research.md` using format:
   - Decision: [what was chosen]
   - Rationale: [why chosen]
   - Alternatives considered: [what else evaluated]

**Output**: research.md with all NEEDS CLARIFICATION resolved

## Phase 1: Design & Contracts
*Prerequisites: research.md complete*

1. **Extract entities from feature spec** → `data-model.md`:
   - Entity name, fields, relationships
   - Validation rules from requirements
   - State transitions if applicable

2. **Generate API contracts** from functional requirements:
   - For each user action → endpoint
   - Use standard REST/GraphQL patterns
   - Output OpenAPI/GraphQL schema to `/contracts/`

3. **Generate contract tests** from contracts:
   - One test file per endpoint
   - Assert request/response schemas
   - Tests must fail (no implementation yet)

4. **Extract test scenarios** from user stories:
   - Each story → integration test scenario
   - Quickstart test = story validation steps

5. **Update agent file incrementally** (O(1) operation):
   - Run `.specify/scripts/bash/update-agent-context.sh claude`
     **IMPORTANT**: Execute it exactly as specified above. Do not add or remove any arguments.
   - If exists: Add only NEW tech from current plan
   - Preserve manual additions between markers
   - Update recent changes (keep last 3)
   - Keep under 150 lines for token efficiency
   - Output to repository root

**Output**: data-model.md, /contracts/*, failing tests, quickstart.md, agent-specific file

## Phase 2: Task Planning Approach
*This section describes what the /tasks command will do - DO NOT execute during /plan*

**Task Generation Strategy**:
- Database migrations and schema creation (foundation layer)
- Contract tests for all auth API endpoints [P] (TDD requirement)
- Data model implementation with validation [P] (core entities)
- Authentication middleware with scope checking
- Token service layer (CRUD operations with audit logging)
- API endpoint handlers with proper error responses
- Integration tests covering end-to-end auth flows
- Documentation and quickstart validation

**Ordering Strategy**:
- **Phase 1**: Database foundation (migrations, repositories)
- **Phase 2**: Contract tests [P] → Model implementations [P] (TDD parallel)
- **Phase 3**: Service layer → Middleware → API handlers (dependency order)
- **Phase 4**: Integration tests → Documentation → Performance validation

**Specific Task Categories**:
1. **Database Tasks**: Auth table migrations, repository extensions
2. **Test Tasks [P]**: Contract tests for token API, middleware tests, integration scenarios
3. **Model Tasks [P]**: PersonalAccessToken, TokenScope, AuthContext implementations
4. **Service Tasks**: TokenService, AuthService, validation logic
5. **API Tasks**: Authentication middleware, token management endpoints
6. **Integration Tasks**: End-to-end flows, quickstart validation, performance benchmarks

**Estimated Output**: 28-32 numbered, ordered tasks with 12-15 marked [P] for parallel execution

**Code Cleanup Strategy**:
- Initial audit task (T001) to discover existing auth code stubs or incomplete implementations
- Final cleanup phase (T057-T060) to remove/consolidate obsolete code after new implementation
- Deduplication of any overlapping functionality discovered during development
- Migration of hardcoded auth logic to new centralized system

**IMPORTANT**: This phase is executed by the /tasks command, NOT by /plan

## Phase 3+: Future Implementation
*These phases are beyond the scope of the /plan command*

**Phase 3**: Task execution (/tasks command creates tasks.md)  
**Phase 4**: Implementation (execute tasks.md following constitutional principles)  
**Phase 5**: Validation (run tests, execute quickstart.md, performance validation)

## Complexity Tracking
*Fill ONLY if Constitution Check has violations that must be justified*

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| [e.g., 4th project] | [current need] | [why 3 projects insufficient] |
| [e.g., Repository pattern] | [specific problem] | [why direct DB access insufficient] |


## Progress Tracking
*This checklist is updated during execution flow*

**Phase Status**:
- [x] Phase 0: Research complete (/plan command)
- [x] Phase 1: Design complete (/plan command)
- [x] Phase 2: Task planning complete (/plan command - describe approach only)
- [ ] Phase 3: Tasks generated (/tasks command)
- [ ] Phase 4: Implementation complete
- [ ] Phase 5: Validation passed

**Gate Status**:
- [x] Initial Constitution Check: PASS
- [x] Post-Design Constitution Check: PASS
- [x] All NEEDS CLARIFICATION resolved
- [ ] Complexity deviations documented

---
*Based on Constitution v1.1.0 - See `.specify/memory/constitution.md`*
