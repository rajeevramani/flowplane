# Feature Specification: CORS HTTP Filter Support

**Feature Branch**: `003-cors-http-filter`
**Created**: 2025-09-27
**Status**: Draft
**Input**: User description: "For the upcoming release we should prioritize landing CORS support using the existing HTTP filter framework, with a deliberate eye toward extending it for future work like header_mutation once the initial integration is stable. That means spiking Envoy's envoy.types.pb.envoy.extensions.filters.http.cors.v3.CorsPolicy schema inside our src/xds/filters/http registry: define a strongly typed config struct that round-trips cleanly to the protobuf Any payload, register it in HttpFilterKind, and ensure per-route overrides fit our scoped-config plumbing. While implementing this, audit the shared helper traits/macros we already use for jwt_auth and local_rate_limit so we can factor out any duplicated validation (e.g., optional defaults, enum coercions) before bolting on CORS-specific rules like origin pattern parsing and preflight TTL bounds. Finally, add regression coverage mirroring the JWT suite: happy-path struct→proto conversions, Envoy proto→struct parsing where applicable, and negative tests that surface invalid inputs (empty allow lists, malformed headers, out-of-range max-age) so the REST API rejects bad configs early.. let's continue leaning on the envoy-types crate for every proto conversion. It already mirrors Envoy's published descriptors, so we avoid maintaining our own generated protos or drifting from upstream field semantics. In practice that means our new CORS structs should serialize into the envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy message, just like jwt_auth wraps envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::*. We can keep the ergonomics layer (Serde structs, validation, to_any helpers) in our codebase, but the actual protobuf types should come straight from envoy-types so upgrades stay mechanical and compatibility with Envoy's xDS API is guaranteed."

## User Scenarios & Testing

### Primary User Story
As a platform operator configuring API gateways, I want to enable Cross-Origin Resource Sharing (CORS) policies for web applications, so that browser-based clients can securely access APIs from different domains while maintaining proper security boundaries and preflight request handling.

### Acceptance Scenarios
1. **Given** I have a web application hosted on domain-a.com, **When** I configure CORS to allow requests from domain-b.com, **Then** browsers can successfully make cross-origin requests with proper preflight handling
2. **Given** I need different CORS policies for different API routes, **When** I configure route-specific CORS settings, **Then** each route enforces its own origin restrictions and header policies independently
3. **Given** I want to allow specific HTTP methods and headers, **When** I configure CORS allowed methods and headers, **Then** only the specified methods and headers are permitted in cross-origin requests
4. **Given** I configure CORS with credential support, **When** browsers make authenticated cross-origin requests, **Then** the system properly handles credentials and includes appropriate response headers
5. **Given** I set CORS preflight cache TTL, **When** browsers send OPTIONS preflight requests, **Then** the cache duration is respected to optimize subsequent requests

### Edge Cases
- What happens when an origin doesn't match any configured CORS patterns?
- How does the system handle malformed origin headers or invalid preflight requests?
- What occurs when conflicting CORS policies are configured at different levels (global vs route-specific)?
- How are wildcard origin patterns validated and applied securely?
- What validation prevents unsafe CORS configurations that could compromise security?

## Requirements

### Functional Requirements

- **FR-001**: System MUST support configuring CORS policies to specify which origins are allowed to make cross-origin requests
- **FR-002**: System MUST allow configuration of permitted HTTP methods for cross-origin requests (GET, POST, PUT, DELETE, etc.)
- **FR-003**: System MUST support specifying which request headers are allowed in cross-origin requests
- **FR-004**: System MUST support configuration of response headers that are exposed to cross-origin clients
- **FR-005**: System MUST handle preflight OPTIONS requests according to CORS specification with configurable cache duration
- **FR-006**: System MUST support credential-aware CORS policies that control whether cookies and authentication headers are allowed
- **FR-007**: System MUST allow CORS policies to be configured at both global listener level and per-route level with route-specific overrides taking precedence
- **FR-008**: System MUST validate CORS configuration inputs and reject invalid patterns, malformed headers, or unsafe wildcard configurations
- **FR-009**: System MUST support origin matching via Envoy's StringMatcher options (exact hostnames, prefix/suffix patterns, safe regex for subdomains)
- **FR-010**: System MUST enforce maximum age limits for preflight cache duration to prevent excessive client-side caching
- **FR-011**: System MUST integrate with the existing HTTP filter framework for consistent configuration management
- **FR-012**: System MUST provide clear error messages when CORS policies are violated or misconfigured
- **FR-013**: System MUST maintain compatibility with existing filter configurations without breaking changes
- **FR-014**: System MUST support common CORS header patterns while preventing security vulnerabilities from overly permissive configurations
- **FR-015**: System MUST provide configuration validation that ensures CORS policies comply with web security best practices
- **FR-016**: CORS filter conversions MUST use envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy via the existing helper pipeline to maintain Envoy compatibility
- **FR-017**: System MUST include comprehensive test coverage with struct-to-proto round trips, negative validation cases, and per-route override scenarios mirroring JWT and local rate limit test suites
- **FR-018**: Origin pattern matching MUST be limited to Envoy's StringMatcher capabilities (exact, prefix, suffix, safe_regex) and not extend beyond what Envoy's allow_origin_string_match supports
- **FR-019**: Preflight cache max-age values MUST conform to google.protobuf.Duration constraints (non-negative values up to 315,576,000,000 seconds) and be validated during configuration parsing to prevent runtime errors

### Key Entities

- **CORS Policy**: Represents a complete CORS configuration including allowed origins, methods, headers, credentials setting, and preflight cache duration
- **Origin Pattern**: Represents an allowed origin specification that can be exact domain, wildcard pattern, or protocol-specific rule
- **Preflight Configuration**: Represents settings for handling OPTIONS preflight requests including cache duration and allowed request characteristics
- **CORS Violation Response**: Represents the system's response when a request doesn't meet configured CORS requirements
- **Filter Integration Context**: Represents how CORS policies integrate with other HTTP filters and route-specific configurations

## Review & Acceptance Checklist

### Content Quality
- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

### Requirement Completeness
- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Future Work

While this specification focuses on CORS filter implementation to establish stable integration patterns with the HTTP filter framework, the following extensions are planned for subsequent releases:

- **Header Mutation Filter**: Next priority filter implementation using the patterns established by CORS, enabling dynamic request/response header modification with route-specific overrides
- **Filter Chain Optimization**: Performance improvements and shared validation patterns identified during CORS implementation
- **Advanced Origin Matching**: Potential extensions to origin pattern matching beyond StringMatcher capabilities, pending Envoy upstream developments

This deliberate scoping to CORS first ensures the framework remains extensible while validating core integration patterns before adding complexity.

## Execution Status

- [x] User description parsed
- [x] Key concepts extracted
- [x] Ambiguities marked
- [x] User scenarios defined
- [x] Requirements generated
- [x] Entities identified
- [x] Review checklist passed