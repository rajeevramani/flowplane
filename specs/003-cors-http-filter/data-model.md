# CORS HTTP Filter Data Model

## Core Entities

### CorsConfig
Primary configuration entity for CORS policies at listener level.

**Fields**:
- `allow_origins: Vec<OriginPattern>` - List of allowed origin patterns (required)
- `allow_methods: Vec<HttpMethod>` - Permitted HTTP methods (default: GET, POST)
- `allow_headers: Vec<String>` - Allowed request headers (default: content-type, authorization)
- `expose_headers: Vec<String>` - Headers exposed to client (default: empty)
- `allow_credentials: bool` - Whether credentials are supported (default: false)
- `max_age: Option<u32>` - Preflight cache duration in seconds (default: 86400)

**Validation Rules**:
- `allow_origins` must not be empty
- `max_age` must be ≤ 315,576,000,000 seconds (google.protobuf.Duration limit)
- Header names must be valid RFC 7230 tokens
- Methods must be valid HTTP methods
- Cannot use wildcard origins with `allow_credentials: true`

**State Transitions**: Immutable configuration, replaced atomically during updates

### CorsPerRouteConfig
Route-specific CORS configuration that overrides listener-level settings.

**Fields**:
- `enabled: bool` - Whether CORS is enabled for this route (default: true)
- `cors_policy: Option<CorsConfig>` - Route-specific policy override
- `inherit_from_listener: bool` - Use listener policy with route modifications (default: true)

**Validation Rules**:
- If `enabled: false`, other fields ignored
- If `cors_policy` provided, inherits validation rules from CorsConfig
- Cannot disable inheritance and provide no policy

**Relationships**:
- Belongs to Route entity
- Overrides listener-level CorsConfig when present

### OriginPattern
Represents allowed origin specification using Envoy's StringMatcher capabilities.

**Fields**:
- `match_type: OriginMatchType` - Type of origin matching
- `pattern: String` - The pattern string
- `case_sensitive: bool` - Whether matching is case sensitive (default: true)

**OriginMatchType enum**:
- `Exact` - Exact hostname match
- `Prefix` - Prefix match (e.g., "https://")
- `Suffix` - Suffix match (e.g., ".example.com")
- `SafeRegex` - Regular expression using safe regex engine

**Validation Rules**:
- Pattern cannot be empty after trimming
- SafeRegex patterns must compile successfully
- Prefix patterns should include protocol for security
- Cannot use overly broad patterns in production

### HttpMethod
Enumeration of supported HTTP methods for CORS.

**Values**:
- `GET`, `POST`, `PUT`, `DELETE`, `HEAD`, `OPTIONS`, `PATCH`, `TRACE`

**Validation Rules**:
- Must be recognized HTTP method
- OPTIONS automatically included for preflight support

### CorsViolationResponse
Response entity when CORS requirements are not met.

**Fields**:
- `error_type: CorsErrorType` - Type of CORS violation
- `message: String` - Human-readable error description
- `allowed_origins: Vec<String>` - Origins that would be allowed (for debugging)

**CorsErrorType enum**:
- `OriginNotAllowed` - Origin doesn't match any allowed patterns
- `MethodNotAllowed` - HTTP method not permitted
- `HeaderNotAllowed` - Request header not permitted
- `CredentialsNotSupported` - Credentials sent but not allowed
- `InvalidPreflightRequest` - Malformed OPTIONS request

### FilterIntegrationContext
Context for CORS filter integration with other HTTP filters.

**Fields**:
- `filter_name: String` - Canonical Envoy filter name ("envoy.filters.http.cors")
- `filter_order: u32` - Position in filter chain
- `dependencies: Vec<String>` - Required filters to run before CORS
- `per_route_enabled: bool` - Whether route-specific config is supported

**Relationships**:
- Part of HttpFilterChain
- Interacts with Router filter and JWT authentication filter

## Protobuf Mapping

### CorsConfig → envoy.extensions.filters.http.cors.v3.CorsPolicy
- `allow_origins` → `allow_origin_string_match` (repeated StringMatcher)
- `allow_methods` → `allow_methods` (string list)
- `allow_headers` → `allow_headers` (string list)
- `expose_headers` → `expose_headers` (string list)
- `allow_credentials` → `allow_credentials` (BoolValue)
- `max_age` → `max_age` (Duration)

### CorsPerRouteConfig → envoy.extensions.filters.http.cors.v3.CorsPolicy
Uses same protobuf message as listener config, but applied per-route via `typed_per_filter_config`.

## Error Handling

### Configuration Errors
- `EmptyOriginList` - No allowed origins specified
- `InvalidHeaderName` - Header name violates RFC 7230
- `InvalidMaxAge` - Duration exceeds protobuf limits
- `UnsafeWildcardWithCredentials` - Security violation
- `MalformedRegexPattern` - SafeRegex pattern compilation failure

### Runtime Errors
- `OriginMismatch` - Request origin not in allowed list
- `MethodNotPermitted` - HTTP method not allowed
- `PreflightValidationFailure` - OPTIONS request validation failed
- `CredentialPolicyViolation` - Credentials sent when not allowed

## Integration Points

### API Layer
- REST endpoints accept CorsConfig JSON
- Validation occurs at API boundary before persistence
- OpenAPI schema generation via ToSchema derive

### XDS Layer
- Converts CorsConfig to Envoy protobuf messages
- Handles both listener-level and route-level configuration
- Integrates with existing HTTP filter framework

### Testing Layer
- Unit tests for entity validation logic
- Integration tests for protobuf round-trip conversion
- Property-based tests for pattern matching edge cases

## Performance Considerations

### Memory Usage
- OriginPattern regex compilation cached at configuration time
- Route-specific configs use Cow<str> for shared string data
- Large origin lists use efficient HashSet lookups

### Validation Performance
- Pre-compile regex patterns during configuration parsing
- Validate header names against pre-computed valid character sets
- Cache validation results for repeated configurations

## Security Properties

### Origin Validation
- Strict pattern matching prevents origin spoofing
- Case-sensitive matching by default
- Regex patterns use safe engine to prevent ReDoS attacks

### Credential Handling
- Explicit opt-in for credential support
- Prevents wildcard origins with credentials enabled
- Clear separation between authenticated and anonymous CORS policies

### Configuration Security
- Validates against overly permissive patterns
- Warns about potential security implications
- Enforces reasonable cache duration limits

## Future Extensions

### Planned Enhancements
- **Header Mutation Integration**: CORS headers could be modified by header mutation filter
- **Advanced Pattern Matching**: Extensions beyond StringMatcher when Envoy supports them
- **Performance Optimization**: Shared validation trait extraction for all HTTP filters

### Compatibility
- Data model designed to support future CORS specification updates
- Extensible enum patterns allow new match types
- Protobuf mapping can evolve with Envoy API changes