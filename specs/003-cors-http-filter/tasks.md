# CORS HTTP Filter Implementation Tasks

**Branch**: `003-cors-http-filter`
**Feature**: CORS HTTP Filter Support
**Implementation Strategy**: Test-Driven Development (TDD) following constitutional requirements

## Task Overview

Total: 18 tasks organized in dependency order (status in brackets)
- **Phase 1**: Foundation & Core Types (T001-T005)
- **Phase 2**: Validation & Security (T006-T010)
- **Phase 3**: HTTP Filter Integration (T011-T015)
- **Phase 4**: Schema Integration (T016-T017)
- **Phase 5**: Integration Testing & Validation (T018)

**Parallelizable Tasks**: Marked with [P] can be executed independently
**Sequential Tasks**: Must be completed in order due to dependencies

---

## Phase 1: Foundation & Core Types

### T001: Create CORS configuration types [P] ✅
**Description**: Define core CorsConfig and related types in src/xds/filters/http/cors.rs
**Dependencies**: None
**Deliverables**:
- `CorsConfig` struct with serde derives
- `OriginPattern` struct with match type enum
- `CorsPerRouteConfig` struct for route overrides
- Basic struct documentation
**Acceptance Criteria**:
- All structs compile with required derives: Debug, Clone, Serialize, Deserialize, ToSchema
- Serde serialization works for JSON representation
- Code follows existing filter patterns from jwt_auth.rs

### T002: Create CORS validation error types [P] ❌ *(skipped)*
**Description**: Define error types for CORS configuration validation
**Dependencies**: None
**Deliverables**:
- CORS-specific error variants in existing error enum
- Error messages for invalid configurations
- Security violation error types
**Acceptance Criteria**:
- *Not needed*. Reused existing `invalid_config` helpers; no new variants introduced.

### T003: Implement basic struct validation (failing tests) [P] ✅
**Description**: Create unit tests for CorsConfig validation that will initially fail
**Dependencies**: T001
**Deliverables**:
- Unit tests in `mod tests` section of cors.rs
- Tests for required fields validation
- Tests for field constraints (max_age limits, etc.)
**Acceptance Criteria**:
- Tests compile but fail (no implementation yet)
- Tests cover all validation rules from data-model.md
- Test names clearly describe validation scenarios

### T004: Implement protobuf conversion types ✅
**Description**: Create Envoy protobuf message structures for CORS
**Dependencies**: T001
**Deliverables**:
- envoy-types imports for CorsPolicy protobuf
- Type conversion helper functions
- Any protobuf creation logic
**Acceptance Criteria**:
- Uses envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy
- Integrates with existing any_from_message() helper
- Type URL correctly set

### T005: Implement basic CorsConfig validation logic ✅
**Description**: Make T003 tests pass by implementing validation
**Dependencies**: T003
**Deliverables**:
- Validation implementation for all CorsConfig fields
- Security validation (wildcard + credentials check)
- Duration and string validation inline
**Acceptance Criteria**:
- All unit tests from T003 pass
- Validation errors are descriptive
- Security constraints properly enforced

---

## Phase 2: Validation & Security

### T006: Implement origin pattern validation [P] ✅
**Description**: Validate OriginPattern configurations with security checks
**Dependencies**: T005
**Deliverables**:
- Regex pattern compilation validation
- Origin security validation
- Pattern matching constraint validation
**Acceptance Criteria**:
- Invalid regex patterns rejected with clear errors
- Unsafe wildcard patterns blocked
- StringMatcher compatibility verified

### T007: Implement CORS security validation [P] ✅
**Description**: Enforce CORS security best practices
**Dependencies**: T005
**Deliverables**:
- Wildcard origin + credentials validation
- Header name RFC compliance checks
- Max-age protobuf Duration constraints
**Acceptance Criteria**:
- Security violations clearly rejected
- RFC 7230 header validation implemented
- google.protobuf.Duration limits enforced

### T008: Create protobuf round-trip tests (failing) ✅
**Description**: Unit tests for CorsConfig ↔ protobuf conversion
**Dependencies**: T004
**Deliverables**:
- Tests for CorsConfig → protobuf conversion
- Tests for protobuf → CorsConfig parsing
- Round-trip integrity tests
**Acceptance Criteria**:
- Tests compile but fail initially
- Cover all configuration fields
- Test edge cases and error conditions

### T009: Implement CorsConfig protobuf conversion ✅
**Description**: Make T008 tests pass with protobuf conversion implementation
**Dependencies**: T008
**Deliverables**:
- `to_any()` method implementation
- `from_proto()` method implementation
- Error handling for conversion failures
**Acceptance Criteria**:
- All protobuf round-trip tests pass
- Uses existing any_from_message() helper
- Proper error handling for invalid protobuf data

### T010: Implement per-route configuration validation ✅
**Description**: Validate CorsPerRouteConfig with inheritance logic
**Dependencies**: T009
**Deliverables**:
- Per-route config validation
- Inheritance vs override logic validation
- Route-specific constraint validation
**Acceptance Criteria**:
- Route config validation follows listener config patterns
- Inheritance logic properly validated
- Clear errors for invalid route configurations

---

## Phase 3: HTTP Filter Integration

### T011: Register CORS in HttpFilterKind enum ✅
**Description**: Add CORS variant to HttpFilterKind in src/xds/filters/http/mod.rs
**Dependencies**: T010
**Deliverables**:
- CORS variant added to HttpFilterKind enum
- Required trait implementations
- Filter name and routing logic
**Acceptance Criteria**:
- Enum compiles with new CORS variant
- `default_name()` returns "envoy.filters.http.cors"
- `to_any()` method properly implemented
- `is_router()` returns false

### T012: Register CORS in HttpScopedConfig enum ✅
**Description**: Add per-route CORS configuration support
**Dependencies**: T011
**Deliverables**:
- CORS variant in HttpScopedConfig enum
- Per-route protobuf conversion
- Route override logic integration
**Acceptance Criteria**:
- HttpScopedConfig compiles with CORS variant
- Per-route to_any() implementation works
- Integration with existing scoped config logic

### T013: Create HTTP filter integration tests (failing) ✅
**Description**: Integration tests for CORS filter in HTTP filter chain following existing patterns
**Dependencies**: T012
**Deliverables**:
- Integration tests in src/xds/filters/http/cors.rs (mod integration_tests)
- Filter chain configuration tests
- xDS resource generation tests
**Acceptance Criteria**:
- Tests compile but fail initially
- Follow patterns from existing filter tests
- Test filter ordering and precedence

### T014: Implement filter integration logic ✅
**Description**: Make T013 tests pass with proper filter integration
**Dependencies**: T013
**Deliverables**:
- Filter integration with existing framework
- xDS resource generation for CORS
- Filter chain ordering logic
**Acceptance Criteria**:
- All integration tests pass
- CORS filter properly integrated in xDS resources
- No breaking changes to existing filters

### T015: Test filter precedence and ordering ✅
**Description**: Verify CORS filter works correctly with other filters
**Dependencies**: T014
**Deliverables**:
- Multi-filter test scenarios in existing test structure
- Filter ordering validation tests
- JWT + CORS integration tests
**Acceptance Criteria**:
- CORS filter works with JWT authentication
- Filter ordering is correct
- No conflicts with existing filters

---

## Phase 4: Schema Integration

### T016: Update filter configuration schemas ✅
**Description**: Extend existing filter configuration to support CORS
**Dependencies**: T015
**Deliverables**:
- CORS filter works with existing listener/route configuration APIs
- Schema validation for CORS configurations
- Integration with existing filter management
**Acceptance Criteria**:
- CORS configs accepted by existing endpoints
- Validation errors properly returned
- No new REST endpoints required

### T017: Add CORS to OpenAPI specification (STRETCH) ⭕ *(not in scope this release)*
**Description**: Include CORS schemas in main OpenAPI spec if filter documentation is in scope
**Dependencies**: T016
**Deliverables**:
- CORS schemas in main OpenAPI specification
- Documentation for CORS configuration fields
- Example CORS configurations
**Acceptance Criteria**:
- OpenAPI spec generates correctly with CORS schemas
- CORS configuration documented
- Examples validate against schemas
**Note**: Optional task - deferred; covered by README/docs updates instead of OpenAPI.

---

## Phase 5: Integration Testing & Validation

### T018: Final validation and comprehensive testing ✅
**Description**: Ensure all requirements met and documentation complete
**Dependencies**: T017
**Deliverables**:
- All 19 functional requirements verified
- Constitutional compliance confirmed
- Performance validation completed
- End-to-end testing using existing test infrastructure
**Acceptance Criteria**:
- All tests pass (unit and integration) – satisfied via `cargo test --tests`
- No breaking changes to existing functionality – regression suite green
- Documentation updated (`docs/filters.md`) with examples

---

## Execution Notes

### TDD Compliance
- All implementation tasks preceded by failing test tasks
- Tests written before implementation in every phase
- Red-Green-Refactor cycle strictly followed

### Constitutional Compliance
- **Test-First**: Tasks 3, 8, 13 create failing tests before implementation
- **Security by Default**: Tasks 6, 7 enforce security validation
- **Structured Configs**: Tasks 1, 4, 9 ensure strong typing
- **Application Stability**: All tasks preserve existing functionality

### Parallel Execution Opportunities
Tasks marked [P] can be executed in parallel:
- T001, T002, T003 (Foundation setup)
- T006, T007 (Independent validation logic)

### Risk Mitigation
- Early protobuf integration (T004, T009) to catch compatibility issues
- Security validation implemented before filter integration
- Comprehensive test coverage throughout implementation
- Filter integration validated with existing test patterns

### Success Criteria
- All functional requirements (FR-001 through FR-019) implemented (docs/spec verified)
- Constitutional principles maintained throughout
- No breaking changes to existing filter framework
- `cargo test --tests` green on branch
- CORS configuration documented in `docs/filters.md`
