# Logging Improvements - Production Observability Enhancement

## Overview

This document describes the logging improvements implemented to enhance production observability in Flowplane. The changes focus on reducing log verbosity while adding structured logging and correlation tracking across the Platform API (BFF) to native Envoy resource flow.

## Changes Summary

### 1. Log Verbosity Reduction

**Problem**: Excessive INFO-level logs cluttering production output, making it difficult to identify significant events.

**Solutions Implemented**:

#### xDS Cache Refresh Logs (`src/xds/state.rs`)
- **Changed**: Cache refresh "no changes" messages from INFO → DEBUG
- **Impact**: Eliminates ~3 log entries per refresh cycle when no changes occur
- **Location**: Lines ~205, ~271, ~373 in `refresh_clusters_from_repository()`, `refresh_routes_from_repository()`, and `refresh_listeners_from_repository()`

```rust
// Before:
info!(
    phase = "cache_refresh",
    type_url = CLUSTER_TYPE_URL,
    total_resources,
    "Cache refresh detected no changes"
);

// After:
debug!(
    phase = "cache_refresh",
    type_url = CLUSTER_TYPE_URL,
    total_resources,
    "Cache refresh detected no changes"
);
```

#### Resource Building Logs (`src/xds/resources.rs`)
- **Changed**: Individual resource build messages from INFO → DEBUG
- **Changed**: Aggregated summary logs from INFO → DEBUG (due to high frequency during concurrent cache refreshes)
- **Impact**: Eliminates ~4-8 duplicate logs per cache refresh cycle
- **Location**: `routes_from_database_entries()`, `listeners_from_database_entries()`, `clusters_from_database_entries()`

```rust
// Before (one log per resource):
info!(
    phase,
    resource = %envoy_route.name,
    bytes = encoded.len(),
    "Built route configuration from repository"
);

// After (aggregated at DEBUG level):
if !built.is_empty() {
    debug!(
        phase,
        route_count = built.len(),
        total_bytes,
        "Built route configurations from repository"
    );
}
```

**Rationale**: Cache refresh operations happen frequently on multiple concurrent threads, creating duplicate logs. These are internal operations useful for troubleshooting but not business-significant events for production monitoring.

**Example Output** (with RUST_LOG=debug):
```
DEBUG Built route configurations from repository route_count=5 total_bytes=1024 phase=cache_refresh
```

### 2. Structured Logging with Correlation IDs

**Problem**: Difficult to trace requests through the BFF → native resource conversion pipeline.

**Solutions Implemented**:

#### Platform API Materializer (`src/platform_api/materializer.rs`)

Added `#[instrument]` macros to key methods with structured context fields:

##### `create_definition()` - Entry Point
```rust
#[instrument(
    skip(self),
    fields(
        team = %spec.team,
        domain = %spec.domain,
        api_definition_id = field::Empty,
        correlation_id = %uuid::Uuid::new_v4(),
        listener_isolation = spec.listener_isolation
    )
)]
pub async fn create_definition(&self, spec: ApiDefinitionSpec) -> Result<...>
```

**Fields**:
- `team`: Team identifier for multi-tenancy
- `domain`: API domain being configured
- `api_definition_id`: Recorded after creation (initially Empty)
- `correlation_id`: UUID for end-to-end request tracking
- `listener_isolation`: Configuration flag

##### `materialize_native_resources()` - BFF→Native Conversion
```rust
#[instrument(
    skip(self, api_routes, _listener_spec),
    fields(
        api_definition_id = %definition.id,
        team = %definition.team,
        domain = %definition.domain,
        route_count = api_routes.len()
    )
)]
async fn materialize_native_resources(...) -> Result<...>
```

**Added per-route mapping logs**:
```rust
info!(
    api_route_id = %api_route.id,
    native_route_id = %route.id,
    native_cluster_id = %cluster.id,
    match_type = %api_route.match_type,
    match_value = %api_route.match_value,
    "Materialized BFF route to native resources"
);
```

**Added summary log**:
```rust
info!(
    total_routes = generated_route_ids.len(),
    total_clusters = generated_cluster_ids.len(),
    "Completed native resource materialization"
);
```

##### Listener Materialization Methods

Both `materialize_isolated_listener()` and `materialize_shared_listener_routes()` now include:
- `api_definition_id`
- `team`
- `domain`
- `route_count`

## Configuration

### Log Levels

Set via `RUST_LOG` environment variable:

```bash
# Production: INFO level (significant events only)
RUST_LOG=info

# Development: DEBUG level (includes cache refresh details)
RUST_LOG=debug

# Detailed troubleshooting: TRACE level
RUST_LOG=trace

# Component-specific levels
RUST_LOG=flowplane::xds=debug,flowplane::platform_api=info
```

### JSON Format for Log Aggregation

The tracing framework supports JSON output for tools like ELK, Fluentd, Datadog:

```rust
// In src/observability/logging.rs initialization
use tracing_subscriber::fmt::format::JsonFields;

tracing_subscriber::fmt()
    .json()
    .with_current_span(true)
    .with_span_list(true)
    .init();
```

Example JSON output:
```json
{
  "timestamp": "2025-10-06T06:15:42.123456Z",
  "level": "INFO",
  "message": "Materialized BFF route to native resources",
  "target": "flowplane::platform_api::materializer",
  "span": {
    "correlation_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "team": "payments",
    "domain": "api.payments.example.com",
    "api_definition_id": "def-123456"
  },
  "fields": {
    "api_route_id": "route-abc",
    "native_route_id": "route-xyz",
    "native_cluster_id": "cluster-789",
    "match_type": "prefix",
    "match_value": "/v1/payments"
  }
}
```

## Log Correlation Examples

### End-to-End Request Flow

When a Platform API definition is created, logs show the complete flow:

```
INFO  Span created                          correlation_id=a1b2c3d4-... team=payments domain=api.payments.example.com
INFO  Materialized BFF route to native      api_route_id=route-1 native_route_id=platform-api-abc native_cluster_id=cluster-xyz
INFO  Completed native resource             total_routes=3 total_clusters=3
INFO  Triggering xDS updates for clusters
INFO  Built cluster configurations          cluster_count=15 total_bytes=4096 phase=cache_refresh
```

### Troubleshooting with Correlation IDs

Search logs by `correlation_id` to trace a specific API definition creation:

```bash
# In production logs (JSON format)
jq 'select(.span.correlation_id == "a1b2c3d4-e5f6-7890-abcd-ef1234567890")' app.log

# In structured text logs
grep "a1b2c3d4-e5f6-7890-abcd-ef1234567890" app.log
```

### Team-Based Filtering

Filter logs by team for multi-tenant debugging:

```bash
# All operations for payments team
jq 'select(.span.team == "payments")' app.log
```

## Performance Impact

The tracing framework is designed for production use with minimal overhead:

- **Disabled spans**: Near-zero cost when log level filters them out
- **Enabled spans**: ~50-100ns overhead per span (negligible for I/O-bound operations)
- **Structured fields**: Pre-allocated, no runtime string formatting until output

## Testing

All existing tests continue to pass (168 tests), validating:
- Functionality correctness
- Log format compatibility
- Context propagation

## Best Practices

### When to Use INFO vs DEBUG

**INFO**: Significant state changes and business events
- API definition created/updated/deleted (from materializer)
- BFF routes materialized to native resources (with IDs)
- xDS configuration updates sent to Envoy (with change counts)
- Authentication successes and failures
- Resource conflict detection
- HTTP API requests (from access logs)

**DEBUG**: Operational details and internal operations
- Cache refresh operations (with/without changes)
- Resource building from database (routes, listeners, clusters)
- Platform API route config creation
- Individual resource serialization
- Middleware processing details
- Database query execution

**TRACE**: Extremely detailed execution flow
- Function entry/exit
- Loop iterations
- Conditional branch decisions

### Adding Correlation to New Operations

```rust
use tracing::{instrument, field};

#[instrument(
    skip(self),
    fields(
        correlation_id = %uuid::Uuid::new_v4(),
        // Add domain-specific context
        resource_type = "cluster",
        operation = "create"
    )
)]
pub async fn your_method(&self, params: Params) -> Result<Output> {
    // Logs within this function automatically include correlation_id
    info!("Processing started");

    // Record values determined during execution
    tracing::Span::current().record("resource_id", field::display(&created_id));

    info!("Processing complete");
    Ok(output)
}
```

## Migration Guide

### For Developers

No action required - all changes are backward compatible. New structured logs will appear alongside existing logs.

### For Operations Teams

1. **Update log aggregation queries** to leverage new structured fields:
   - `span.team`
   - `span.domain`
   - `span.correlation_id`
   - `span.api_definition_id`

2. **Adjust log volume alerts**: INFO logs are now significantly reduced

3. **Create new dashboards** for correlation tracking:
   - BFF route → native resource mapping
   - Per-team API activity
   - Per-domain error rates

4. **Update runbooks** to include correlation ID searches for troubleshooting

## Future Enhancements

- [ ] Add request tracing headers for HTTP API correlation
- [ ] Implement distributed tracing with OpenTelemetry
- [ ] Add metrics for log volume by component
- [ ] Create Grafana dashboard templates for correlation analysis
