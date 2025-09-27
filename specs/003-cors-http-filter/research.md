# CORS HTTP Filter Implementation Research

## Overview
Research findings for implementing CORS filter following established patterns in the Flowplane HTTP filter framework.

## Technology Decisions

### Decision: Follow existing HTTP filter registration pattern
**Rationale**: Consistent with jwt_auth and local_rate_limit implementations
**Pattern**:
- Add `Cors(cors::CorsConfig)` to `HttpFilterKind` enum
- Implement required methods: `default_name()`, `to_any()`, `is_router()`
- Use canonical Envoy filter name: `"envoy.filters.http.cors"`

### Decision: Use envoy-types crate for protobuf integration
**Rationale**: Maintains compatibility with Envoy xDS API, avoids maintaining custom proto generation
**Implementation**:
- Import `envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy`
- Convert config to `EnvoyAny` using `any_from_message()` helper
- Type URL: `"type.googleapis.com/envoy.extensions.filters.http.cors.v3.CorsPolicy"`

### Decision: Implement per-route configuration support
**Rationale**: FR-007 requires route-specific overrides, following existing scoped config pattern
**Implementation**:
- Add `Cors(CorsPerRouteConfig)` to `HttpScopedConfig` enum
- Support both listener-level and route-level configuration
- Route overrides take precedence per existing framework behavior

## Validation Strategy

### Decision: Extract common validation helpers
**Rationale**: DRY principle compliance, reduce code duplication across filters
**Identified patterns to factor out**:
- Duration conversion (milliseconds to protobuf Duration)
- Empty string validation with trim
- Collection emptiness checks
- Optional field handling with defaults

**Proposed helper implementations**:
```rust
// Duration conversion helper
trait ProtobufDuration {
    fn to_proto_duration(&self) -> ProtoDuration;
    fn from_proto_duration(proto: &ProtoDuration) -> Result<Self, Error>;
}

// String validation macro
macro_rules! validate_non_empty_string {
    ($field:expr, $field_name:expr) => {
        if $field.trim().is_empty() {
            return Err(invalid_config(format!("{} cannot be empty", $field_name)));
        }
    };
}
```

### Decision: Validate against Envoy protobuf constraints
**Rationale**: FR-019 requires google.protobuf.Duration compliance, prevent runtime errors
**Constraints**:
- Max-age values: non-negative up to 315,576,000,000 seconds
- Origin patterns: limited to StringMatcher capabilities (exact, prefix, suffix, safe_regex)
- Header validation: RFC-compliant header names and values

## Testing Approach

### Decision: Mirror JWT authentication test suite structure
**Rationale**: FR-017 explicitly requires test coverage mirroring existing patterns
**Test categories**:
1. **Unit tests**: Struct-to-proto conversions, validation logic
2. **Round-trip tests**: Config → protobuf → config integrity
3. **Negative tests**: Invalid configurations rejected with clear errors
4. **Integration tests**: End-to-end API to xDS generation
5. **Per-route tests**: Scoped configuration override scenarios

**Test file locations**:
- Unit tests: `tests/unit/xds/filters/http/cors.rs`
- Integration tests: `tests/integration/cors_filter_api.rs`
- Test fixtures: `tests/fixtures/cors_configs.json`

## Architecture Integration

### Decision: Extend existing HTTP filter framework without changes
**Rationale**: FR-013 mandates no breaking changes to existing filter configurations
**Integration points**:
- **Registration**: Add to `src/xds/filters/http/mod.rs` enum
- **API endpoints**: Integrate with existing filter configuration endpoints
- **XDS generation**: Automatic inclusion in Envoy resource builders

### Decision: Follow established configuration patterns
**Rationale**: Consistency with existing filters, leverages proven patterns
**Required derives**: `Debug, Clone, Serialize, Deserialize, ToSchema`
**Error handling**: Return `crate::Error::Config(String)` for validation failures

## Implementation Files

### Core implementation
- `src/xds/filters/http/cors.rs` - Main CORS filter implementation
- `src/xds/filters/http/mod.rs` - Updated to include CORS in enum

### Shared validation helpers (new)
- `src/utils/filter_validation.rs` - Extracted common validation patterns
- Factor out from `jwt_auth.rs` and `local_rate_limit.rs`

### Testing files
- `tests/unit/xds/filters/http/cors.rs` - Unit tests
- `tests/integration/cors_filter_api.rs` - API integration tests
- `tests/fixtures/cors_configs.json` - Test data

## Dependencies

### Existing dependencies (no additions needed)
- `envoy-types` - Protobuf message definitions
- `serde` - Serialization framework
- `anyhow` - Error handling
- `tokio` - Async runtime
- `utoipa` - OpenAPI schema generation

### Validation constraints
- Must not introduce new external dependencies
- Must maintain compatibility with existing Rust version (1.75+)
- Must support existing test framework (`cargo test`)

## Security Considerations

### Decision: Implement secure-by-default CORS policies
**Rationale**: FR-015 requires web security best practices compliance
**Security measures**:
- Reject overly permissive wildcard origins in production
- Validate origin patterns for common security vulnerabilities
- Enforce reasonable max-age limits for preflight caching
- Provide clear warnings for potentially unsafe configurations

### Decision: Follow existing authentication integration patterns
**Rationale**: CORS often used with authenticated APIs, must integrate cleanly
**Integration**:
- Support credential-aware CORS policies (FR-006)
- Maintain compatibility with existing JWT authentication filter
- Ensure filter ordering doesn't create security gaps

## Performance Requirements

### Decision: Sub-millisecond configuration validation
**Rationale**: Control plane performance goals, minimize xDS update latency
**Implementation approach**:
- Pre-compile regex patterns during configuration parsing
- Cache validation results where appropriate
- Minimize allocations in hot path validation

### Decision: Support thousands of concurrent route configurations
**Rationale**: Scale requirements for production deployment
**Architecture considerations**:
- Efficient in-memory representation
- Minimize clone overhead for route-specific configs
- Leverage Rust's zero-cost abstractions

## Alternatives Considered

### Alternative: Custom protobuf generation
**Rejected because**: envoy-types crate provides maintained compatibility with Envoy upstream
**Trade-off**: Slightly less control over protobuf details vs. guaranteed compatibility

### Alternative: Separate validation library
**Rejected because**: Would introduce new dependency, existing patterns sufficient
**Trade-off**: More comprehensive validation vs. maintaining lightweight dependencies

### Alternative: Runtime configuration validation
**Rejected because**: Constitutional requirement for early validation at API boundary
**Trade-off**: Flexibility vs. fail-fast principle compliance

## Next Steps for Phase 1

1. **Design data model** - Define CorsConfig and CorsPerRouteConfig structs
2. **Create API contracts** - OpenAPI specifications for CORS endpoints
3. **Generate contract tests** - Failing tests for TDD approach
4. **Extract validation helpers** - Factor out common patterns from existing filters
5. **Update agent context** - Refresh Claude Code context with new patterns