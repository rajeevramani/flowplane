
# Implementation Plan: Platform API Abstraction

**Branch**: `004-platform-api-abstraction` | **Date**: 2025-09-28 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/Users/rajeevramani/workspace/projects/flowplane/specs/004-platform-api-abstraction/spec.md`

## Implementation Approach

This plan follows the established Flowplane development workflow:
1. **Research Phase**: Understand existing patterns and dependencies
2. **Design Phase**: Create data models, API contracts, and documentation
3. **Implementation Phase**: Execute tasks following TDD principles
4. **Validation Phase**: Test coverage and constitutional compliance

All phases are executed manually following constitutional requirements for test-first development and security-by-default principles.

## Summary
Implement a high-level Platform API Abstraction that allows teams to expose services through Envoy without understanding low-level listener/route/cluster configuration. Primary requirement is a REST API (`POST /v1/api-definitions`) that accepts host/path matchers and upstream targets, automatically generates Envoy resources, provides downloadable bootstrap configurations, and maintains team-based RBAC with collision detection.

## Technical Context
**Language/Version**: Rust 1.75+ (existing Flowplane codebase)
**Primary Dependencies**: envoy-types crate, serde, tokio, hyper, sqlx, uuid, anyhow
**Storage**: Database (existing SQL schema with new tables for API definitions)
**Testing**: cargo test with unit tests, integration tests, property-based testing via proptest
**Target Platform**: Linux server (Envoy control plane)
**Project Type**: single (extending existing Rust control plane)
**Performance Goals**: Sub-100ms API response times, support for 1000+ concurrent API definitions
**Constraints**: Must maintain backward compatibility, leverage existing OpenAPI pipeline (MVP-FR12), RBAC enforcement required
**Scale/Scope**: Support hundreds of teams with thousands of API definitions, 80% adoption within 60 days

## Constitution Check
*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**I. Structured Configs First**: ✅ PASS - MVP-FR1/FR2 require strongly-typed API definition models with clear mapping to Envoy resources, avoiding raw YAML exposure to users

**II. Validation Early**: ✅ PASS - MVP-FR5, MVP-FR6 mandate RBAC validation and collision detection at API boundary with actionable error messages

**III. Test-First Development**: ✅ PASS - Constitutional requirement for TDD approach with comprehensive test coverage

**IV. Idempotent Resource Building**: ✅ PASS - MVP-FR2 requires generated Envoy resources to be derived from stored API definitions, supporting safe reconciliation

**V. Security by Default**: ✅ PASS - MVP-FR5 enforces team-based RBAC, MVP-FR10 requires audit logging, explicit ownership tagging (section 3)

**VI. Application Stability**: ✅ PASS - Feature extends existing infrastructure without breaking changes, maintains compatibility with current Envoy configuration

**VII. DRY Principle**: ✅ PASS - MVP-FR12 explicitly requires leveraging existing `/api/v1/gateways/openapi` import pipeline to avoid code duplication

**Gate Status**: ✅ ALL PASS - No constitutional violations detected

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
├── api/
│   ├── handlers/
│   │   ├── api_definitions.rs    # New Platform API endpoints
│   │   └── mod.rs               # Updated to include api_definitions
│   └── models/
│       ├── api_definition.rs    # New API definition request/response models
│       └── mod.rs               # Updated to include api_definition
├── xds/
│   ├── resources/
│   │   ├── api_generator.rs     # New resource generator for API definitions
│   │   └── mod.rs               # Updated to include api_generator
│   └── state.rs                 # Updated for API definition state management
├── db/
│   ├── models/
│   │   ├── api_definitions.rs   # Database models for API definitions
│   │   └── mod.rs               # Updated to include api_definitions
│   └── migrations/
│       └── 004_api_definitions.sql # New migration for API definition tables
└── services/
    ├── api_definition_service.rs # Business logic for API definitions
    └── mod.rs                   # Updated to include api_definition_service

tests/
├── platform_api/                # New feature-specific test directory
│   ├── integration/
│   │   ├── test_api_definitions.rs # Integration tests
│   │   └── test_bootstrap_generation.rs # Bootstrap generation tests
│   ├── unit/
│   │   ├── test_collision_detection.rs # Unit tests for collision logic
│   │   └── test_rbac_validation.rs # Unit tests for RBAC
│   └── fixtures/
│       └── api_definition_examples.json # Test data
└── support.rs                   # Shared test utilities
```

**Structure Decision**: Single Rust project extending existing control plane architecture. Core implementation in new API definition modules, leveraging existing xDS resource generation patterns and OpenAPI import pipeline per MVP-FR12. Feature-specific integration tests in `tests/platform_api/` directory following existing patterns.

## Phase 0: Outline & Research

**Status**: ✅ COMPLETED (leveraging existing research.md)

**Research findings**:
- **Existing API patterns**: Analyzed Axum-based handlers with middleware and validation
- **Database patterns**: Confirmed SQLx with repository pattern and JSON configuration storage
- **xDS integration**: Identified builder patterns for Envoy resource generation
- **OpenAPI pipeline**: Found reusable components in existing gateway import flow (MVP-FR12)
- **RBAC implementation**: Analyzed PAT-based authorization with scope enforcement
- **Bootstrap generation**: Understood resource seeding and artifact storage patterns

**Key decisions**:
- Extend existing API structure with new Platform API handlers
- Use database schema with normalized tables and JSON columns for flexibility
- Leverage existing xDS builder patterns for resource generation
- Build on OpenAPI import pipeline per explicit requirement MVP-FR12
- Implement team-based RBAC using existing middleware patterns

## Phase 1: Design & Contracts
*Prerequisites: research.md complete*

**Status**: ✅ COMPLETED (leveraging existing design artifacts)

**Deliverables**:
1. **data-model.md** - Complete entity definitions:
   - ApiDefinition, ApiRoute, PathConfig, UpstreamConfig entities
   - Database schema aligned with MVP requirements and appendix examples
   - Envoy resource mapping and collision detection strategies
   - Error handling and audit logging design per MVP-FR10

2. **contracts/platform-api.yaml** - OpenAPI specification:
   - REST endpoints matching appendix B payload examples
   - Complete CRUD operations for API definitions per MVP-FR1, MVP-FR7
   - Bootstrap download endpoints per MVP-FR3
   - Validation schemas aligned with functional requirements

3. **contracts/platform_api_contract_tests.rs** - Failing contract tests:
   - API definition lifecycle test scenarios
   - Collision detection tests per MVP-FR6
   - RBAC validation tests per MVP-FR5
   - Bootstrap generation tests per MVP-FR3

4. **quickstart.md** - User documentation:
   - Examples matching specification appendix B
   - Step-by-step workflows for MVP user journeys
   - Error handling guide for collision and RBAC scenarios
   - CI/CD integration patterns

**Ready for**: Task generation following TDD principles and constitutional requirements

## Phase 2: Task Planning Approach

**Task Generation Strategy**:
- Generate tasks from Phase 1 design artifacts and MVP functional requirements
- Follow TDD principles: failing tests first, then implementation
- Each MVP-FR requirement becomes implementation task
- Database migration and model creation following existing patterns
- Integration tests derived from user journeys (section 5)

**Ordering Strategy**:
- Constitutional requirement: Tests before implementation
- Dependency order: Database → Models → Services → API handlers → Integration
- MVP-FR12 priority: Leverage existing OpenAPI pipeline components first
- Parallel tasks: Independent model creation and validation logic
- Sequential tasks: Database migration → core entities → business logic → API layer

**Estimated Output**: 20-25 numbered tasks following established Flowplane patterns

**Key Implementation Areas**:
1. **Database Foundation**: Migrations and core entity models per section 9
2. **OpenAPI Integration**: Leverage existing pipeline per MVP-FR12
3. **Business Logic**: Service layer with collision detection (MVP-FR6) and RBAC (MVP-FR5)
4. **API Layer**: REST handlers matching appendix B payload examples
5. **xDS Integration**: Resource generation per MVP-FR2 and bootstrap creation per MVP-FR3

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
- [x] Phase 0: Research complete - leveraging existing research.md with Platform API patterns
- [x] Phase 1: Design complete - data-model.md, contracts/, quickstart.md aligned with spec requirements
- [x] Phase 2: Task planning approach defined - ready for manual task generation following MVP requirements
- [ ] Phase 3: Tasks generated - tasks.md creation
- [ ] Phase 4: Implementation complete - following TDD principles and MVP-FR requirements
- [ ] Phase 5: Validation passed - test coverage and constitutional compliance

**Gate Status**:
- [x] Initial Constitution Check: PASS - No violations detected, all requirements align with principles
- [x] Post-Design Constitution Check: PASS - Design artifacts confirm compliance with MVP-FR requirements
- [x] All technical context defined - No NEEDS CLARIFICATION items blocking MVP implementation
- [x] Complexity deviations documented - None required, leveraging existing patterns per MVP-FR12

---
*Based on Constitution v2.1.1 - See `/memory/constitution.md`*
