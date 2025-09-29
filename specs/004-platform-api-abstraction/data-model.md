# Platform API Abstraction Data Model

## Core Entities

### ApiDefinition
Primary entity representing a team's high-level API configuration that abstracts Envoy resource management.

**Fields**:
- `id: UUID` - Unique identifier for the API definition
- `team: String` - Team name for ownership and RBAC enforcement (required)
- `domain: String` - Domain name for the API (e.g., "payments.flowplane.dev") (required)
- `listener_isolation: bool` - Whether to use dedicated listener (default: false, shared listener)
- `tls_config: Option<TlsConfig>` - Optional TLS configuration for HTTPS
- `metadata: Map<String, Value>` - Additional metadata for audit and management
- `created_at: DateTime<Utc>` - Creation timestamp
- `updated_at: DateTime<Utc>` - Last modification timestamp
- `version: i32` - Configuration version for tracking changes

**Validation Rules**:
- `team` must not be empty and match existing team names
- `domain` must be valid DNS name format
- Combination of (team, domain) must be unique across all API definitions
- `tls_config` if provided must have valid certificate references

**State Transitions**:
- Created → Active (when routes added)
- Active → Updated (when configuration changes)
- Active → Deleted (removes routes but preserves clusters until explicit deletion)

**Relationships**:
- Has many ApiRoute entities
- Belongs to Team (external reference)
- Generates multiple Envoy resources (listeners, clusters)

### ApiRoute
Individual route configuration within an API definition, supporting incremental route addition.

**Fields**:
- `id: UUID` - Unique identifier for the route
- `api_definition_id: UUID` - Foreign key to parent API definition (required)
- `path_config: PathConfig` - Path matching and rewriting configuration (required)
- `upstream_config: UpstreamConfig` - Backend service configuration (required)
- `timeout_seconds: Option<i32>` - Request timeout override (default: 30)
- `override_config: Option<RouteOverrideConfig>` - Route-specific filter overrides
- `deployment_note: Option<String>` - Optional note for tracking deployments
- `created_at: DateTime<Utc>` - Creation timestamp

**Validation Rules**:
- `api_definition_id` must reference existing API definition
- `path_config` must not conflict with existing routes in same domain
- `upstream_config` must have valid endpoint specification
- `timeout_seconds` if provided must be between 1 and 300 seconds

**Relationships**:
- Belongs to ApiDefinition
- Maps to Envoy Route configuration
- References UpstreamTarget entities

### PathConfig
Configuration for URL path matching and optional rewriting within routes.

**Fields**:
- `match_type: PathMatchType` - Type of path matching (prefix, exact, regex)
- `pattern: String` - The path pattern to match (required)
- `rewrite: Option<PathRewrite>` - Optional path rewriting configuration
- `case_sensitive: bool` - Whether matching is case sensitive (default: true)

**PathMatchType enum**:
- `Prefix` - Match path prefix (e.g., "/api/v1/")
- `Exact` - Exact path match (e.g., "/healthz")
- `SafeRegex` - Regular expression matching using safe regex engine

**PathRewrite structure**:
- `prefix: Option<String>` - Replace matched prefix with this value
- `regex: Option<RegexRewrite>` - Advanced regex-based rewriting

**Validation Rules**:
- `pattern` cannot be empty and must be valid for the match type
- `SafeRegex` patterns must compile successfully
- `rewrite` configurations must be compatible with match type
- Path patterns within same domain must not create conflicts

### UpstreamConfig
Configuration for backend service targets with support for multiple upstreams and traffic weights.

**Fields**:
- `targets: Vec<UpstreamTarget>` - List of backend targets (required, min 1)
- `load_balancing: LoadBalancingPolicy` - How to distribute traffic across targets
- `health_check: Option<HealthCheckConfig>` - Health checking configuration
- `circuit_breaker: Option<CircuitBreakerConfig>` - Circuit breaker settings

**Validation Rules**:
- `targets` must contain at least one valid upstream target
- Traffic weights across targets should sum to 100% (with tolerance for rounding)
- All targets must be reachable and valid endpoint formats

**Load Balancing Options**:
- `RoundRobin` - Distribute requests evenly across healthy targets
- `WeightedRoundRobin` - Use target weights for traffic distribution
- `LeastRequest` - Route to target with fewest active requests

### UpstreamTarget
Individual backend service endpoint with optional traffic weight for rollout strategies.

**Fields**:
- `name: String` - Identifier for the upstream target (required)
- `endpoint: String` - Service endpoint (host:port or service discovery name) (required)
- `weight: Option<u32>` - Traffic weight for this target (default: equal distribution)
- `tls_enabled: bool` - Whether to use TLS for upstream connection (default: false)
- `metadata: Map<String, String>` - Additional target metadata

**Validation Rules**:
- `name` must be unique within the upstream configuration
- `endpoint` must be valid network address or service name
- `weight` if provided must be between 1 and 100
- `endpoint` must be resolvable or valid service discovery reference

**Use Cases**:
- Single target: Simple 1:1 API to service mapping
- Multiple targets: Blue/green deployments, canary releases, A/B testing

### RouteOverrideConfig
Route-specific configuration overrides that extend or replace listener-level settings.

**Fields**:
- `cors: Option<CorsOverride>` - CORS policy override
- `auth: Option<AuthOverride>` - Authentication requirement override
- `rate_limit: Option<RateLimitOverride>` - Rate limiting override
- `timeout: Option<TimeoutOverride>` - Timeout configuration override
- `headers: Option<HeaderOverride>` - Header manipulation override

**Override Behavior**:
- Route overrides take precedence over listener-level configurations
- Unspecified overrides inherit from listener defaults
- Route overrides are scoped to the specific route only

### TlsConfig
TLS configuration for API definitions requiring HTTPS termination.

**Fields**:
- `mode: TlsMode` - TLS termination mode
- `cert_source: CertificateSource` - Certificate source configuration
- `key_source: KeySource` - Private key source configuration
- `client_auth: Option<ClientAuthConfig>` - Mutual TLS configuration

**TlsMode enum**:
- `Terminate` - TLS termination at proxy
- `Passthrough` - TLS passthrough to upstream
- `Mutual` - Mutual TLS with client certificate validation

**Certificate Sources**:
- `SecretManager` - AWS Secrets Manager, Azure Key Vault, etc.
- `File` - File system path to certificate
- `Inline` - Certificate content directly in configuration

### CollisionDetection
System for detecting and preventing conflicting API definitions.

**Collision Types**:
- `DomainConflict` - Same domain used by different teams
- `PathConflict` - Overlapping path patterns within same domain
- `ListenerConflict` - Port conflicts for dedicated listeners

**Resolution Strategies**:
- Block conflicting creation with descriptive error
- Suggest alternative configurations
- Provide escalation path for legitimate shared APIs

### AuditEvent
Audit logging for API definition lifecycle events.

**Fields**:
- `event_id: UUID` - Unique event identifier
- `event_type: AuditEventType` - Type of operation
- `actor: String` - User or system performing the action
- `resource_id: UUID` - API definition or route affected
- `request_payload: Value` - Original request data
- `changeset: Value` - Summary of changes made
- `timestamp: DateTime<Utc>` - When the event occurred

**AuditEventType enum**:
- `ApiDefinitionCreated`
- `ApiDefinitionUpdated`
- `ApiDefinitionDeleted`
- `RouteAdded`
- `RouteUpdated`
- `RouteDeleted`

## Database Schema Mapping

### Primary Tables

```sql
-- Core API definition
CREATE TABLE api_definitions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team VARCHAR NOT NULL,
    domain VARCHAR NOT NULL,
    listener_isolation BOOLEAN NOT NULL DEFAULT false,
    tls_config JSONB,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    version INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT api_definitions_team_domain_unique UNIQUE (team, domain)
);

-- Individual routes within API definitions
CREATE TABLE api_routes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    api_definition_id UUID NOT NULL REFERENCES api_definitions(id) ON DELETE CASCADE,
    path_config JSONB NOT NULL,
    upstream_config JSONB NOT NULL,
    timeout_seconds INTEGER,
    override_config JSONB,
    deployment_note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT api_routes_timeout_range CHECK (timeout_seconds IS NULL OR (timeout_seconds >= 1 AND timeout_seconds <= 300))
);

-- Audit events for compliance and debugging
CREATE TABLE api_audit_events (
    event_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR NOT NULL,
    actor VARCHAR NOT NULL,
    resource_id UUID,
    request_payload JSONB,
    changeset JSONB,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Indexes for Performance

```sql
-- Performance indexes
CREATE INDEX api_definitions_team_idx ON api_definitions(team);
CREATE INDEX api_definitions_domain_idx ON api_definitions(domain);
CREATE INDEX api_routes_api_definition_id_idx ON api_routes(api_definition_id);
CREATE INDEX api_audit_events_timestamp_idx ON api_audit_events(timestamp);
CREATE INDEX api_audit_events_resource_id_idx ON api_audit_events(resource_id);

-- Collision detection indexes
CREATE INDEX api_routes_path_search_idx ON api_routes USING GIN ((path_config->'pattern'));
```

## Envoy Resource Mapping

### ApiDefinition → Envoy Listener
```yaml
# Generated Envoy Listener
name: "team-{team}-http"  # or dedicated listener
address:
  socket_address:
    address: "0.0.0.0"
    port_value: 8080  # or allocated port for dedicated
filter_chains:
- filters:
  - name: "envoy.filters.network.http_connection_manager"
    typed_config:
      "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
      stat_prefix: "team_{team}"
      route_config:
        name: "team_{team}_routes"
        virtual_hosts: [{generated from routes}]
```

### ApiRoute → Envoy Route
```yaml
# Generated Envoy Route
match:
  prefix: "{path_config.pattern}"  # or exact/safe_regex
route:
  cluster: "{upstream_config.name}"
  timeout: "{timeout_seconds}s"
  prefix_rewrite: "{path_config.rewrite.prefix}"  # if specified
```

### UpstreamTarget → Envoy Cluster
```yaml
# Generated Envoy Cluster
name: "{upstream_config.name}"
type: STRICT_DNS  # or EDS for service discovery
load_assignment:
  cluster_name: "{upstream_config.name}"
  endpoints:
  - lb_endpoints:
    - endpoint:
        address:
          socket_address:
            address: "{target.endpoint.host}"
            port_value: {target.endpoint.port}
      load_balancing_weight: {target.weight}
```

## Error Handling

### Validation Errors
- `InvalidTeamName` - Team name not found or invalid format
- `DomainFormatError` - Invalid DNS domain format
- `PathPatternError` - Invalid path pattern for match type
- `UpstreamEndpointError` - Invalid or unreachable upstream endpoint
- `WeightSumError` - Traffic weights don't sum to valid percentage

### Collision Errors
- `DomainCollisionError` - Domain already in use by another team
- `PathCollisionError` - Path pattern conflicts with existing route
- `ListenerPortConflictError` - Dedicated listener port unavailable

### Authorization Errors
- `InsufficientPermissionsError` - User lacks required RBAC scope
- `TeamAccessDeniedError` - User not member of specified team
- `ResourceOwnershipError` - Attempting to modify another team's resource

## Integration Points

### xDS Resource Generation
- ApiDefinition + Routes → Envoy Listener configuration
- UpstreamConfig → Envoy Cluster configuration
- Automatic resource naming and tagging for ownership tracking

### Bootstrap Generation
- Generated Envoy resources compiled into downloadable bootstrap
- S3 or local storage for bootstrap artifacts
- Version tracking for configuration updates

### Existing Systems Integration
- Leverage existing OpenAPI import pipeline for validation
- Extend existing RBAC middleware for team-based authorization
- Integrate with existing audit logging infrastructure

## Performance Considerations

### Query Optimization
- Efficient indexes for collision detection queries
- JSONB indexes for path pattern searching
- Connection pooling for database access

### Caching Strategy
- Cache frequently accessed API definitions
- Invalidate cache on configuration updates
- Bootstrap artifact caching with TTL

### Scalability
- Support for 1000+ API definitions per team
- Efficient bulk operations for route management
- Async processing for resource generation

## Security Properties

### Data Protection
- No sensitive data in logs or audit trails
- TLS certificate references only (not content)
- Encrypted database storage for sensitive configurations

### Access Control
- Team-based resource isolation
- Scope-based permission validation
- Audit trail for all modifications

### Configuration Security
- Validation against malicious path patterns
- Upstream endpoint reachability verification
- Rate limiting for API definition operations

## Future Extensions

### Planned Enhancements
- **Traffic Shadowing**: Shadow traffic to new upstream targets
- **Canary Orchestration**: Automated rollout with success metrics
- **OpenAPI Import**: Bulk route creation from OpenAPI specifications
- **Service Discovery Integration**: Dynamic upstream target resolution

### Extension Points
- Plugin system for custom validation rules
- Webhook integration for external approval workflows
- Metrics collection for API performance monitoring
- Integration with external identity providers for team management

This data model provides a comprehensive foundation for the Platform API Abstraction while maintaining flexibility for future enhancements and ensuring consistency with existing Flowplane patterns.