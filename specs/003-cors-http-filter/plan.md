
# Implementation Plan: CORS HTTP Filter Support

**Branch**: `003-cors-http-filter` | **Date**: 2025-09-28 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/Users/rajeevramani/workspace/projects/flowplane/specs/003-cors-http-filter/spec.md`

## Implementation Approach

This plan follows the established Flowplane development workflow:
1. **Research Phase**: Understand existing patterns and dependencies
2. **Design Phase**: Create data models, API contracts, and documentation
3. **Implementation Phase**: Execute tasks following TDD principles
4. **Validation Phase**: Test coverage and constitutional compliance

All phases are executed manually following constitutional requirements for test-first development and security-by-default principles.

## Summary
Implement CORS (Cross-Origin Resource Sharing) HTTP filter support using Envoy's existing filter framework. Primary requirement is enabling web applications to configure cross-origin policies for secure browser-based API access across different domains. Technical approach leverages envoy-types crate for protobuf integration, extends existing HTTP filter registry (jwt_auth, local_rate_limit patterns), and provides route-specific override capabilities with comprehensive validation and testing.

## Technical Context
**Language/Version**: Rust 1.75+ (existing Flowplane codebase)
**Primary Dependencies**: envoy-types crate, serde, tokio, hyper, protobuf, anyhow
**Storage**: In-memory xDS state management (no additional persistence)
**Testing**: cargo test with unit tests, integration tests, property-based testing via proptest
**Target Platform**: Linux server (Envoy control plane)
**Project Type**: single (extending existing Rust control plane)
**Performance Goals**: Sub-millisecond filter config validation, minimal xDS update latency
**Constraints**: Must maintain Envoy protobuf compatibility, no breaking changes to existing filters
**Scale/Scope**: Support thousands of concurrent routes with CORS policies, integrate with existing HTTP filter framework

## Constitution Check
*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**I. Structured Configs First**: ✅ PASS - Feature explicitly requires strongly-typed config struct that round-trips to protobuf Any payload, following existing jwt_auth pattern

**II. Validation Early**: ✅ PASS - FR-008, FR-015, FR-019 mandate validation at API boundary with descriptive errors for invalid patterns, headers, and max-age values

**III. Test-First Development**: ✅ PASS - FR-017 requires comprehensive test coverage mirroring JWT suite (struct→proto conversions, negative validation cases, per-route overrides)

**IV. Idempotent Resource Building**: ✅ PASS - Integrates with existing HTTP filter framework that already provides idempotent resource building

**V. Security by Default**: ✅ PASS - FR-014, FR-015 ensure secure CORS configurations, prevent overly permissive policies, validate against web security best practices

**VI. Application Stability**: ✅ PASS - FR-013 mandates compatibility with existing filter configurations without breaking changes

**VII. DRY Principle**: ✅ PASS - User description explicitly calls for auditing shared helper traits/macros from jwt_auth and local_rate_limit to factor out duplicated validation

**Gate Status**: ✅ ALL PASS - No constitutional violations detected

**Post-Design Re-evaluation**: ✅ CONFIRMED
- **Structured Configs**: data-model.md defines strongly-typed CorsConfig with protobuf mapping
- **Validation Early**: API contracts include comprehensive validation with descriptive errors
- **Test-First**: Contract tests created (failing) before implementation, following TDD
- **Security by Default**: Security patterns documented, unsafe configurations rejected
- **DRY Principle**: Common validation helpers identified for extraction from existing filters
- **Application Stability**: No breaking changes to existing filter framework

## Project Structure

### Documentation (this feature)
```
specs/[###-feature]/
├── plan.md              # This file (/plan command output)
├── research.md          # Phase 0 output (/plan command)
├── data-model.md        # Phase 1 output (/plan command)
├── quickstart.md        # Phase 1 output (/plan command)
├── contracts/           # Phase 1 output (/plan command)
└── tasks.md             # Phase 2 output (/tasks command - NOT created by /plan)
```

### Source Code (repository root)
```
src/
├── xds/
│   ├── filters/
│   │   └── http/
│   │       ├── cors.rs          # New CORS filter implementation (with inline tests)
│   │       ├── jwt_auth.rs      # Existing (reference pattern)
│   │       ├── local_rate_limit.rs # Existing (reference pattern)
│   │       └── mod.rs           # Updated to include cors
│   ├── state.rs                 # xDS state management
│   └── resources/              # Envoy resource builders
├── api/
│   └── handlers/               # REST API handlers (existing structure)
└── config/
    └── validation.rs           # Configuration validation logic

# Existing protobuf helpers (used by CORS implementation):
# - src/xds/filters/mod.rs::any_from_message() - Creates Envoy Any from protobuf
# - src/xds/filters/mod.rs::TypedConfig - Generic protobuf Any representation

tests/
├── tls/                        # Existing feature-specific tests
├── cors/                       # New feature-specific test directory
│   ├── integration/
│   │   └── test_api.rs         # End-to-end API tests
│   ├── unit/
│   │   └── test_validation.rs  # Unit tests for validation logic
│   └── fixtures/
│       └── cors_configs.json   # Test configuration samples
└── support.rs                  # Shared test utilities
```

**Structure Decision**: Single Rust project extending existing HTTP filter framework. Core implementation in `src/xds/filters/http/cors.rs` with inline tests following patterns from `jwt_auth.rs` and `local_rate_limit.rs`. Feature-specific integration tests in `tests/cors/` directory following existing `tests/tls/` pattern. Uses existing protobuf helpers from `src/xds/filters/` rather than creating new utils module.

## Phase 0: Outline & Research

**Completed**: ✅ research.md created with comprehensive analysis

**Research findings**:
- **HTTP filter patterns**: Analyzed existing jwt_auth and local_rate_limit implementations
- **envoy-types integration**: Confirmed protobuf conversion approach using existing helpers
- **Validation patterns**: Identified common validation logic to factor out
- **Testing approach**: Established inline tests + feature-specific integration tests pattern
- **Security constraints**: Documented CORS-specific security requirements

**Key decisions**:
- Follow HttpFilterKind enum registration pattern
- Use existing `any_from_message()` helper from `src/xds/filters/mod.rs`
- Implement per-route configuration via HttpScopedConfig
- Extract common validation helpers to reduce duplication

## Phase 1: Design & Contracts
*Prerequisites: research.md complete*

**Completed**: ✅ Core design artifacts created

**Deliverables**:
1. **data-model.md** - Complete entity definitions:
   - CorsConfig, CorsPerRouteConfig, OriginPattern
   - Validation rules and protobuf mapping
   - Error handling and security properties

2. **contracts/cors-api.yaml** - OpenAPI specification:
   - Listener-level CORS configuration endpoints
   - Per-route CORS override endpoints
   - Comprehensive request/response schemas with examples
   - Error response definitions

3. **contracts/cors_contract_tests.rs** - Failing contract tests:
   - API endpoint validation scenarios
   - Security violation test cases
   - Round-trip configuration tests
   - Error handling validation

4. **quickstart.md** - User documentation:
   - Step-by-step configuration examples
   - Advanced CORS patterns (subdomain wildcards, credentials)
   - Testing and troubleshooting guide
   - Common configuration patterns

**Ready for**: Task generation and implementation following TDD principles

## Phase 2: Task Planning Approach

**Task Generation Strategy**:
- Generate tasks from Phase 1 design artifacts (data model, contracts, quickstart)
- Follow TDD principles: failing tests first, then implementation
- Each contract test becomes a task
- Each entity requires implementation task
- Integration tests derived from user scenarios

**Ordering Strategy**:
- Constitutional requirement: Tests before implementation
- Dependency order: Core types → validation → API integration → advanced features
- Parallel tasks: Independent test files and validation logic
- Sequential tasks: Core filter → registration → API endpoints

**Estimated Output**: 20-25 numbered tasks following established Flowplane patterns

**Next Step**: Manual task generation in tasks.md following research and design artifacts

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

**Phase Status**:
- [x] Phase 0: Research complete - research.md created with comprehensive analysis
- [x] Phase 1: Design complete - data-model.md, contracts/, quickstart.md created
- [x] Phase 2: Task planning approach defined - ready for manual task generation
- [x] Phase 3: Tasks generated - tasks.md with 22 TDD-ordered tasks created
- [ ] Phase 4: Implementation complete - following TDD principles
- [ ] Phase 5: Validation passed - test coverage and constitutional compliance

**Gate Status**:
- [x] Initial Constitution Check: PASS - No violations detected
- [x] Post-Design Constitution Check: PASS - Design artifacts confirm compliance
- [x] All technical context defined - No NEEDS CLARIFICATION items
- [x] Complexity deviations documented - None required

---
*Based on Constitution v2.1.1 - See `/memory/constitution.md`*
