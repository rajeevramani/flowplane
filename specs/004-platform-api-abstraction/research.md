# Platform API Abstraction Implementation Research

## Overview
Research findings for implementing Platform API Abstraction feature following established patterns in the Flowplane control plane.

## Technology Decisions

### Decision: Follow existing Axum-based API patterns
**Rationale**: Consistent with established src/api/ structure using Axum router and middleware
**Pattern**:
- Handler functions with typed request/response models
- Middleware for authentication and RBAC
- Structured error handling with ApiError enum
- OpenAPI documentation with utoipa derives

### Decision: Use SQLx with compile-time checked queries
**Rationale**: Maintains consistency with existing database patterns and provides type safety
**Implementation**:
- Repository pattern for data access
- JSON column types for flexible configuration storage
- Migration-based schema evolution
- Version tracking for configuration changes

### Decision: Leverage existing xDS resource generation patterns
**Rationale**: Reuse proven builder patterns for Envoy resource creation
**Pattern**:
- Builder structs converting high-level configs to Envoy protobuf
- Centralized state management in XdsState
- Database-driven resource generation
- Atomic updates with rollback capabilities

### Decision: Extend existing OpenAPI import pipeline
**Rationale**: MVP-FR12 explicitly requires leveraging existing `/api/v1/gateways/openapi` flow
**Implementation**:
- Reuse parsing and validation logic from gateway handlers
- Extend plan generation for API definition workflows
- Leverage existing error handling and response patterns
- Build on existing OpenAPI → Envoy resource mapping

## API Design Strategy

### Decision: RESTful resource-based API design
**Rationale**: Consistent with existing API patterns and RESTful principles
**Endpoints**:
- `POST /v1/api-definitions` - Create new API definition
- `GET /v1/api-definitions/{id}` - Retrieve API definition
- `PUT /v1/api-definitions/{id}` - Update API definition
- `DELETE /v1/api-definitions/{id}` - Delete API definition
- `POST /v1/api-definitions/{id}/routes` - Add routes incrementally

### Decision: Strongly-typed request/response models
**Rationale**: Constitutional requirement for structured configs first
**Implementation**:
```rust
#[derive(Debug, Serialize, Deserialize, ToSchema, Validate)]
pub struct ApiDefinitionRequest {
    pub team: String,
    pub domain: String,
    pub routes: Vec<RouteConfig>,
    pub listener_isolation: bool,
    pub tls: Option<TlsConfig>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApiDefinitionResponse {
    pub id: String,
    pub listener: String,
    pub routes: Vec<String>,
    pub clusters: Vec<String>,
    pub bootstrap_uri: String,
}
```

## Database Schema Strategy

### Decision: Normalized schema with JSON flexibility
**Rationale**: Balance between structure and flexibility for evolving requirements
**Core Tables**:
- `api_definitions` - Core API definition metadata and ownership
- `api_routes` - Individual route configurations within an API
- `api_upstreams` - Upstream target configurations
- `api_overrides` - Route-level configuration overrides

**Schema Design**:
```sql
CREATE TABLE api_definitions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team VARCHAR NOT NULL,
    domain VARCHAR NOT NULL,
    listener_isolation BOOLEAN NOT NULL DEFAULT false,
    tls_config JSONB,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE api_routes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    api_definition_id UUID NOT NULL REFERENCES api_definitions(id) ON DELETE CASCADE,
    path_config JSONB NOT NULL,
    upstream_config JSONB NOT NULL,
    timeout_seconds INTEGER,
    override_config JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX api_definitions_team_domain_idx
ON api_definitions(team, domain);
```

## RBAC Integration Strategy

### Decision: Extend existing PAT-based authorization
**Rationale**: Leverage existing middleware and scope-based permission system
**Implementation**:
- Extend existing `AuthScope` enum with API definition scopes
- Team-based resource isolation through middleware
- Audit logging integration with existing patterns

**RBAC Patterns**:
```rust
pub enum AuthScope {
    // Existing scopes...
    ApiDefinitionCreate,
    ApiDefinitionRead,
    ApiDefinitionUpdate,
    ApiDefinitionDelete,
}

#[derive(Debug)]
pub struct TeamContext {
    pub user_id: String,
    pub team: String,
    pub scopes: Vec<AuthScope>,
}
```

## xDS Resource Generation Strategy

### Decision: Builder pattern with automatic resource mapping
**Rationale**: Consistent with existing cluster and route builders
**Implementation**:
- `ApiDefinitionBuilder` that generates Envoy listeners, routes, and clusters
- Integration with existing `XdsState` for resource management
- Atomic updates to prevent inconsistent states

**Builder Pattern**:
```rust
pub struct ApiDefinitionBuilder {
    pub definition: ApiDefinition,
    pub routes: Vec<ApiRoute>,
}

impl ApiDefinitionBuilder {
    pub fn build_listener(&self) -> Result<Listener, BuildError> {
        // Generate Envoy listener from API definition
    }

    pub fn build_routes(&self) -> Result<Vec<Route>, BuildError> {
        // Generate Envoy routes from API routes
    }

    pub fn build_clusters(&self) -> Result<Vec<Cluster>, BuildError> {
        // Generate Envoy clusters from upstream configs
    }
}
```

## Bootstrap Generation Strategy

### Decision: Leverage existing resource seeding patterns
**Rationale**: Reuse proven patterns for bootstrap generation and storage
**Implementation**:
- Extend existing bootstrap generation to include API definition resources
- S3/local storage for bootstrap artifacts
- Version tracking for bootstrap updates

## Collision Detection Strategy

### Decision: Database constraints with application-level validation
**Rationale**: Ensure data integrity while providing descriptive error messages
**Implementation**:
- Unique constraints on (team, domain, path_prefix) combinations
- Application-level validation for user-friendly error messages
- Conflict resolution suggestions in error responses

**Validation Logic**:
```rust
pub async fn validate_no_collision(
    pool: &PgPool,
    team: &str,
    domain: &str,
    path_prefix: &str,
) -> Result<(), CollisionError> {
    // Check for existing API definitions with overlapping paths
    // Return descriptive error with conflicting resource info
}
```

## Testing Strategy

### Decision: Comprehensive test suite following existing patterns
**Rationale**: Maintain high test coverage standards and follow TDD principles
**Test Structure**:
- Unit tests with in-memory SQLite for database operations
- Integration tests with TestApi helper for end-to-end scenarios
- Property-based tests for validation logic
- Mock external dependencies

**Testing Patterns**:
```rust
#[tokio::test]
async fn test_api_definition_creation() {
    let test_api = TestApi::new().await;

    let request = ApiDefinitionRequest {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        routes: vec![/* ... */],
        listener_isolation: false,
        tls: None,
    };

    let response = test_api
        .post("/v1/api-definitions")
        .json(&request)
        .send()
        .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    // Verify generated resources...
}
```

## Integration Points

### Decision: Minimize changes to existing systems
**Rationale**: Application stability constitutional requirement
**Integration Areas**:
- Extend existing API router with new handlers
- Add new database models without breaking existing schemas
- Enhance xDS state management without changing interfaces
- Build on existing authentication middleware

## Performance Considerations

### Decision: Async-first with connection pooling
**Rationale**: Support 1000+ concurrent API definitions performance goal
**Implementation**:
- Async handlers with proper error handling
- Database connection pooling with SQLx
- Caching for frequently accessed API definitions
- Efficient querying with proper indexing

## Security Architecture

### Decision: Defense-in-depth with multiple validation layers
**Rationale**: Constitutional security-by-default requirement
**Security Layers**:
- Request validation at API boundary
- RBAC enforcement in middleware
- Database-level constraints
- Audit logging for all operations
- Bootstrap access control

## Migration Strategy

### Decision: Phased rollout with feature flags
**Rationale**: Enable incremental delivery while maintaining stability
**Phases**:
1. Core API definition CRUD operations
2. Route management and collision detection
3. Bootstrap generation and xDS integration
4. Advanced features (filters, overrides)

## Dependencies

### Existing Dependencies (no additions needed)
- `axum` - HTTP framework
- `sqlx` - Database access
- `serde` - Serialization
- `uuid` - ID generation
- `validator` - Request validation
- `utoipa` - OpenAPI documentation

### New Internal Dependencies
- Extend existing error types for API definition errors
- New database migration for API definition tables
- New OpenAPI schemas for API definition endpoints

## Risk Mitigation

### Decision: Leverage existing patterns to minimize risk
**Mitigation Strategies**:
- Reuse proven database and API patterns
- Comprehensive test coverage before implementation
- Gradual rollout with monitoring
- Clear rollback procedures for each phase

## Next Steps for Phase 1

1. **Design data model** - Define detailed API definition and route entities
2. **Create API contracts** - OpenAPI specifications for all endpoints
3. **Generate contract tests** - Failing tests for TDD approach
4. **Design collision detection** - Algorithm for path/domain conflict detection
5. **Plan xDS integration** - Resource generation strategy

## Alternatives Considered

### Alternative: GraphQL API
**Rejected because**: REST API is more consistent with existing Flowplane patterns and simpler for MVP

### Alternative: Direct Envoy YAML configuration
**Rejected because**: Would not provide the abstraction layer required by the specification

### Alternative: Separate microservice
**Rejected because**: Would increase operational complexity and violate the single project architecture

This research provides a comprehensive foundation for implementing the Platform API Abstraction following established Flowplane patterns and constitutional requirements.