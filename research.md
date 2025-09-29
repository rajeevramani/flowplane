# Platform API Abstraction Research

## Overview

This document presents research findings from the existing Flowplane codebase to inform the implementation of the Platform API Abstraction feature. The analysis covers key patterns, components, and integration points that should be leveraged for the new feature implementation.

## 1. Existing API Patterns

### Architecture Overview
- **Framework**: Built on Axum with state-based dependency injection
- **Location**: `/src/api/` directory
- **Router Pattern**: Modular router construction with middleware layers
- **Error Handling**: Structured error types with HTTP status mapping

### Key Components

#### 1.1 Router Structure (`/src/api/routes.rs`)
```rust
pub struct ApiState {
    pub xds_state: Arc<XdsState>,
}

pub fn build_router(state: Arc<XdsState>) -> Router {
    let api_state = ApiState { xds_state: state.clone() };
    // Middleware and route construction
}
```

**Pattern**: Centralized state management with resource-specific handlers

#### 1.2 Handler Pattern (`/src/api/handlers.rs`)
```rust
pub async fn create_cluster_handler(
    State(state): State<ApiState>,
    Json(payload): Json<CreateClusterBody>,
) -> Result<(StatusCode, Json<ClusterResponse>), ApiError>
```

**Key Patterns**:
- Request validation using `validator` crate with `#[derive(Validate)]`
- OpenAPI documentation with `#[utoipa::path]` attributes
- Repository pattern abstraction for data access
- XDS state refresh after modifications
- Comprehensive error handling with `ApiError` enum

#### 1.3 Request/Response Models
- **Validation**: Uses `validator` crate with custom validation functions
- **Serialization**: Serde with camelCase API contracts
- **OpenAPI**: utoipa integration for automatic schema generation
- **Examples**: Detailed JSON examples in schemas for documentation

#### 1.4 Error Handling (`/src/api/error.rs`)
```rust
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Conflict(String),
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    ServiceUnavailable(String),
    Internal(String),
}
```

**Pattern**: Structured error types with automatic HTTP status mapping and consistent JSON error responses

## 2. Database Patterns

### Architecture Overview
- **ORM**: SQLx with compile-time checked queries
- **Migration**: SQL-based migrations in `/migrations/` directory
- **Repository Pattern**: Trait-based abstraction with concrete implementations

### Key Components

#### 2.1 Repository Pattern (`/src/storage/repository.rs`)
```rust
#[derive(Debug, Clone)]
pub struct ClusterRepository {
    pool: DbPool,
}

impl ClusterRepository {
    pub async fn create(&self, request: CreateClusterRequest) -> Result<ClusterData>
    pub async fn get_by_name(&self, name: &str) -> Result<ClusterData>
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ClusterData>>
    pub async fn update(&self, id: &str, request: UpdateClusterRequest) -> Result<ClusterData>
    pub async fn delete(&self, id: &str) -> Result<()>
    pub async fn exists_by_name(&self, name: &str) -> Result<bool>
}
```

**Key Patterns**:
- Standard CRUD operations with async/await
- UUID-based primary keys
- Version tracking for optimistic locking
- JSON configuration storage
- Comprehensive error handling with context

#### 2.2 Data Models
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterData {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub configuration: String, // JSON serialized
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

**Pattern**: Audit trail with timestamps, version tracking, and flexible JSON configuration storage

#### 2.3 Migration Pattern (`/migrations/20241201000001_create_clusters_table.sql`)
```sql
CREATE TABLE IF NOT EXISTS clusters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    service_name TEXT NOT NULL,
    configuration TEXT NOT NULL,  -- JSON serialized cluster config
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(name, version)
);

-- Performance indexes
CREATE INDEX IF NOT EXISTS idx_clusters_version ON clusters(version);
CREATE INDEX IF NOT EXISTS idx_clusters_service_name ON clusters(service_name);
CREATE INDEX IF NOT EXISTS idx_clusters_updated_at ON clusters(updated_at);
```

**Pattern**: SQLite-compatible schema with proper indexing and constraints

## 3. xDS Resource Generation

### Architecture Overview
- **Location**: `/src/xds/` directory with resource building in `/src/xds/resources.rs`
- **Pattern**: Builder pattern converting configuration specs to Envoy protocol buffers
- **Integration**: Database-driven resource generation with caching

### Key Components

#### 3.1 Resource Building (`/src/xds/resources.rs`)
```rust
pub struct BuiltResource {
    pub name: String,
    pub resource: Any,
}

pub fn clusters_from_database_entries(
    entries: Vec<ClusterData>,
    context: &str,
) -> Result<Vec<BuiltResource>>
```

**Key Patterns**:
- Conversion from JSON configuration to Envoy protobuf resources
- Comprehensive validation during resource building
- Context-aware logging for debugging
- Type-safe resource wrapping with `Any` protobuf type

#### 3.2 Configuration Specs (`/src/xds/cluster_spec.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSpec {
    pub endpoints: Vec<EndpointSpec>,
    pub connect_timeout_seconds: Option<u64>,
    pub use_tls: Option<bool>,
    pub lb_policy: Option<String>,
    pub health_checks: Vec<HealthCheckSpec>,
    pub circuit_breakers: Option<CircuitBreakersSpec>,
    // ... additional fields
}
```

**Pattern**: High-level configuration abstractions that map to Envoy resources

#### 3.3 State Management (`/src/xds/state.rs`)
```rust
impl XdsState {
    pub async fn refresh_clusters_from_repository(&self) -> Result<()>
    pub async fn refresh_routes_from_repository(&self) -> Result<()>
    pub async fn refresh_listeners_from_repository(&self) -> Result<()>
}
```

**Pattern**: Centralized state management with selective refresh capabilities

## 4. OpenAPI Import Pipeline

### Architecture Overview
- **Location**: `/src/openapi/` directory
- **Pattern**: Pipeline-based processing with validation and rollback
- **Integration**: Converts OpenAPI specs to Flowplane resources

### Key Components

#### 4.1 Gateway Plan Building (`/src/openapi/mod.rs`)
```rust
pub struct GatewayPlan {
    pub cluster_requests: Vec<CreateClusterRequest>,
    pub route_request: Option<CreateRouteRepositoryRequest>,
    pub listener_request: Option<CreateListenerRequest>,
    pub default_virtual_host: Option<VirtualHostConfig>,
    pub summary: GatewaySummary,
}

pub fn build_gateway_plan(
    openapi: OpenAPI,
    options: GatewayOptions,
) -> Result<GatewayPlan, GatewayError>
```

**Key Patterns**:
- Comprehensive plan generation before execution
- Support for both shared and dedicated listener modes
- Gateway tagging for resource tracking
- Structured error handling with specific error types

#### 4.2 Import Handler (`/src/api/gateway_handlers.rs`)
```rust
pub async fn create_gateway_from_openapi_handler(
    State(state): State<ApiState>,
    Query(params): Query<GatewayQuery>,
    request: Request<Body>,
) -> Result<(StatusCode, Json<GatewaySummary>), ApiError>
```

**Key Patterns**:
- Multi-format support (JSON/YAML)
- Atomic transactions with rollback on failure
- Resource conflict detection
- Comprehensive audit logging

#### 4.3 Rollback Pattern
```rust
async fn rollback_import(
    listener_repo: &ListenerRepository,
    route_repo: &RouteRepository,
    cluster_repo: &ClusterRepository,
    listener: Option<&str>,
    route: Option<&str>,
    clusters: &[String],
)
```

**Pattern**: Clean failure handling with resource cleanup

## 5. RBAC Implementation

### Architecture Overview
- **Location**: `/src/auth/` directory
- **Pattern**: Middleware-based authentication and authorization
- **Token System**: Personal Access Tokens with scope-based permissions

### Key Components

#### 5.1 Authentication Middleware (`/src/auth/middleware.rs`)
```rust
pub async fn authenticate(
    State(auth_service): State<AuthServiceState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError>

pub async fn ensure_scopes(
    State(required_scopes): State<ScopeState>,
    Extension(context): Extension<AuthContext>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError>
```

**Key Patterns**:
- Bearer token authentication
- Scope-based authorization
- Request context injection
- Comprehensive audit logging

#### 5.2 Token Model (`/src/auth/models.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PersonalAccessToken {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: TokenStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    // ... additional fields
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub token_id: String,
    pub token_name: String,
    scopes: HashSet<String>,
}
```

**Key Patterns**:
- Fine-grained scope system
- Token lifecycle management
- Request-scoped authentication context

#### 5.3 Route Protection Pattern
```rust
Router::new()
    .route("/api/v1/clusters", post(create_cluster_handler))
    .route_layer(scope_layer(vec!["clusters:write"]))
```

**Pattern**: Granular route-level permission enforcement

## 6. Bootstrap Generation

### Architecture Overview
- **Location**: `/src/openapi/defaults.rs` and `/src/auth/token_service.rs`
- **Pattern**: Automatic resource seeding and token generation

### Key Components

#### 6.1 Default Resource Creation (`/src/openapi/defaults.rs`)
```rust
pub async fn ensure_default_gateway_resources(state: &XdsState) -> Result<(), Error> {
    // Create default cluster, routes, and listener if they don't exist
    // Seed bootstrap admin token
}
```

**Pattern**: Idempotent resource initialization

#### 6.2 Bootstrap Token (`/src/auth/token_service.rs`)
```rust
pub async fn ensure_bootstrap_token(&self) -> Result<Option<TokenSecretResponse>> {
    if self.repository.count_tokens().await? > 0 {
        return Ok(None); // Already initialized
    }
    // Create admin token with full permissions
}
```

**Pattern**: One-time admin token generation with full scope permissions

## 7. Testing Patterns

### Architecture Overview
- **Location**: `/tests/` directory with unit and integration tests
- **Pattern**: Comprehensive test coverage with helper utilities

### Key Components

#### 7.1 Integration Test Pattern (`/tests/tls/integration/test_api_tls.rs`)
```rust
async fn setup_state() -> ApiState {
    let pool = create_pool(&create_test_config()).await.expect("pool");
    // Create test tables
    let state = XdsState::with_database(SimpleXdsConfig::default(), pool);
    ApiState { xds_state: Arc::new(state) }
}

#[tokio::test]
async fn create_cluster_applies_defaults_and_persists() {
    let state = setup_state().await;
    let response = create_cluster_handler(State(state.clone()), Json(body))
        .await
        .expect("handler response");
    // Assertions
}
```

**Key Patterns**:
- In-memory SQLite for isolated testing
- Async test support with tokio::test
- Helper functions for test setup
- Comprehensive assertions with error verification

#### 7.2 Unit Test Pattern (`/tests/auth/unit/test_auth_service.rs`)
```rust
async fn setup_services() -> (TokenService, AuthService) {
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool));

    let token_service = TokenService::new(repo.clone(), audit.clone());
    let auth_service = AuthService::new(repo, audit);

    (token_service, auth_service)
}
```

**Pattern**: Dependency injection with shared test setup

## 8. Validation Framework

### Architecture Overview
- **Location**: `/src/validation/` directory
- **Pattern**: Multi-layer validation with Envoy protocol validation

### Key Components

#### 8.1 Validation Layers (`/src/validation/mod.rs`)
```rust
pub fn validate_request<T: Validate>(request: &T) -> Result<()>
pub fn validate_path_with_match_type(path: &str, match_type: &PathMatchType) -> Result<(), ValidationError>
pub fn validate_uri_template(template: &str) -> Result<(), ValidationError>
```

**Key Patterns**:
- Three-layer validation: basic, protocol, business rules
- Envoy-types integration for protocol validation
- Custom validation functions with detailed error messages

## 9. Recommended Implementation Patterns

### For Platform API Abstraction

#### 9.1 New API Resources
Follow the established patterns:
- Create request/response models with validation in `/src/api/platform_handlers.rs`
- Add repository implementations in `/src/storage/platform_repository.rs`
- Create database migrations following the existing schema pattern
- Add appropriate scopes for RBAC (e.g., `platforms:read`, `platforms:write`)

#### 9.2 Resource Management
- Use the existing xDS state management pattern
- Follow the repository pattern for data persistence
- Implement rollback capabilities for atomic operations
- Add comprehensive logging and metrics

#### 9.3 Integration Points
- Leverage existing OpenAPI import pipeline patterns
- Use established validation framework
- Follow error handling patterns with `ApiError`
- Implement proper authentication/authorization middleware

#### 9.4 Testing Strategy
- Create integration tests following `/tests/` patterns
- Use in-memory SQLite for isolated testing
- Implement comprehensive unit tests for business logic
- Add validation tests for all request/response models

## 10. Key Reusable Components

### Immediately Usable
1. **Repository Pattern**: Extend for new platform resources
2. **API Error Handling**: Use existing `ApiError` enum
3. **Validation Framework**: Leverage for platform-specific validation
4. **Authentication/Authorization**: Extend scope system
5. **xDS State Management**: Follow refresh patterns
6. **Database Migration Pattern**: Use for new platform tables
7. **OpenAPI Documentation**: Leverage utoipa integration
8. **Testing Utilities**: Reuse setup patterns

### Integration Patterns
1. **Gateway Plan Building**: Adapt for platform resource planning
2. **Rollback Mechanisms**: Use for atomic platform operations
3. **Resource Tagging**: Apply for platform resource tracking
4. **Bootstrap Generation**: Extend for platform defaults

This research provides a comprehensive foundation for implementing the Platform API Abstraction feature while maintaining consistency with existing Flowplane patterns and practices.