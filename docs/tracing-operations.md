# Distributed Tracing Operations Guide

This guide covers the distributed tracing capabilities in Flowplane, including Envoy proxy tracing configuration, control plane instrumentation, and end-to-end trace correlation.

## Overview

Flowplane provides comprehensive distributed tracing support at multiple layers:

1. **Envoy Proxy Tracing**: Configure OpenTelemetry, Zipkin, or custom tracing providers via listener configuration
2. **Control Plane Tracing**: All API handlers, database operations, and xDS streams are instrumented with `#[instrument]` spans
3. **Access Log Correlation**: W3C TraceContext extraction from access logs for correlating Envoy traffic with application traces
4. **gRPC Tracing**: Automatic trace context propagation for xDS streams

## Architecture

```
┌───────────────────────────────────────────────────────────────────────────┐
│                     Distributed Trace Flow                                 │
└───────────────────────────────────────────────────────────────────────────┘

  Client Request                                         Backend Service
       │                                                       │
       │ traceparent: 00-<trace_id>-<span_id>-01              │
       ▼                                                       │
  ┌─────────────┐                                              │
  │   Envoy     │  ─── trace spans ───▶  [Collector]          │
  │   Proxy     │                        (OTLP/Zipkin)         │
  │             │  ─── access logs ──▶  [Flowplane CP]        │
  └──────┬──────┘                              │                │
         │                                     │                │
         │ W3C TraceContext                    │                │
         │ propagated                          │                │
         ▼                                     ▼                │
  ┌─────────────┐                     ┌──────────────┐         │
  │   Backend   │                     │   Jaeger /   │◀────────┘
  │   Service   │─── traces ─────────▶│   Grafana    │
  └─────────────┘                     └──────────────┘

```

## Envoy Listener Tracing Configuration

### OpenTelemetry Provider (Recommended)

Configure OpenTelemetry tracing for Envoy listeners via the REST API:

```bash
curl -X POST http://flowplane:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "traced-listener",
    "team": "production",
    "address": "0.0.0.0",
    "port": 8080,
    "filterChains": [{
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "main-routes",
        "tracing": {
          "provider": {
            "type": "open_telemetry",
            "service_name": "my-envoy-gateway",
            "grpc_cluster": "otel_collector",
            "max_cache_size": 1024
          },
          "randomSamplingPercentage": 10.0,
          "spawnUpstreamSpan": true,
          "customTags": {
            "environment": "production",
            "service.namespace": "api-gateway"
          }
        }
      }]
    }]
  }'
```

**OpenTelemetry Configuration Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `service_name` | string | Yes | Service name reported in traces |
| `grpc_cluster` | string | One required | Envoy cluster for OTLP gRPC exporter |
| `http_cluster` | string | One required | Envoy cluster for OTLP HTTP exporter |
| `http_path` | string | No | HTTP endpoint path (default: `/v1/traces`) |
| `max_cache_size` | u32 | No | Max spans to cache when collector is down |

**Note:** You must define the collector cluster in your clusters configuration:

```bash
curl -X POST http://flowplane:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "otel_collector",
    "team": "production",
    "endpointMode": "dns",
    "endpoints": ["otel-collector.monitoring:4317"]
  }'
```

### Zipkin Provider

For Zipkin-based tracing:

```json
{
  "tracing": {
    "provider": {
      "type": "zipkin",
      "collector_cluster": "zipkin_cluster",
      "collector_endpoint": "/api/v2/spans",
      "trace_id_128bit": true,
      "shared_span_context": true,
      "collector_endpoint_version": "http_json",
      "collector_hostname": "zipkin.monitoring"
    },
    "randomSamplingPercentage": 5.0
  }
}
```

**Zipkin Configuration Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `collector_cluster` | string | Envoy cluster for Zipkin collector |
| `collector_endpoint` | string | Zipkin endpoint path |
| `trace_id_128bit` | bool | Use 128-bit trace IDs (recommended) |
| `shared_span_context` | bool | Share context between client/server spans |
| `collector_endpoint_version` | string | `http_json` or `http_proto` |
| `collector_hostname` | string | Optional hostname override |

### Generic Provider

For custom tracing providers:

```json
{
  "tracing": {
    "provider": {
      "type": "generic",
      "name": "envoy.tracers.custom",
      "config": {
        "custom_key": "custom_value"
      }
    }
  }
}
```

### Tracing Configuration Options

These options apply to all provider types:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `randomSamplingPercentage` | f64 | None | Sample rate (0.0 - 100.0) |
| `spawnUpstreamSpan` | bool | false | Create spans for upstream calls |
| `customTags` | map | {} | Static tags added to all spans |

## Control Plane Tracing

### Enabling OpenTelemetry Export

Configure the control plane to export traces to an OpenTelemetry collector:

```bash
# Environment variables
export FLOWPLANE_ENABLE_TRACING=true
export FLOWPLANE_SERVICE_NAME=flowplane-control-plane
export OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc

# Start the control plane
cargo run --release
```

### Instrumented Components

The control plane automatically instruments:

**API Handlers:**
- All `/api/v1/clusters` operations
- All `/api/v1/route-configs` operations
- All `/api/v1/listeners` operations
- Token management endpoints
- OpenAPI import endpoints

**Database Operations:**
- All repository methods (`ClusterRepository`, `RouteRepository`, etc.)
- Transaction boundaries
- Connection pool metrics

**xDS Streams:**
- ADS stream establishment
- Delta discovery requests/responses
- Client ACK/NACK processing

### Trace Context Propagation

The control plane automatically:
1. Extracts trace context from incoming HTTP `traceparent` headers
2. Propagates context to database operations
3. Includes context in xDS stream handling
4. Logs trace IDs in structured logging output

Example log with trace context:

```json
{
  "timestamp": "2025-01-27T10:30:00Z",
  "level": "INFO",
  "target": "flowplane::api::handlers::clusters",
  "message": "Created cluster",
  "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
  "span_id": "00f067aa0ba902b7",
  "cluster_name": "backend-api"
}
```

## Access Log Trace Correlation

### W3C TraceContext Extraction

Flowplane's access log service automatically extracts W3C TraceContext headers from Envoy access logs:

```
traceparent: 00-<trace_id>-<span_id>-<trace_flags>
tracestate: <vendor>=<value>
```

**Format:**
- `00` - Version (always "00" currently)
- `trace_id` - 32 hex characters (128-bit trace ID)
- `span_id` - 16 hex characters (64-bit span ID)
- `trace_flags` - 2 hex characters ("01" = sampled)

### Correlation in Processed Logs

When processing access logs, trace context is available for:

1. **Query correlation**: Find all access logs for a specific trace
2. **Latency analysis**: Correlate Envoy-side timing with backend traces
3. **Error investigation**: Link failed requests to upstream trace spans

Example ProcessedLogEntry with trace context:

```rust
ProcessedLogEntry {
    session_id: "learning-session-123",
    request_id: Some("req-456"),
    path: "/api/users/42",
    response_status: 200,
    duration_ms: 45,
    trace_context: Some(TraceContext {
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
        span_id: "00f067aa0ba902b7",
        trace_flags: "01",
        trace_state: Some("vendor1=value1"),
    }),
    // ... other fields
}
```

### Querying by Trace ID

```sql
-- Find all access log entries for a trace
SELECT * FROM access_log_entries
WHERE trace_id = '4bf92f3577b34da6a3ce929d0e0e4736'
ORDER BY start_time_seconds;

-- Correlate with backend spans in Jaeger
-- Use the same trace_id in Jaeger UI search
```

## Integration Examples

### Full Stack Tracing Setup

**1. Deploy OpenTelemetry Collector:**

```yaml
# otel-collector.yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318

exporters:
  jaeger:
    endpoint: jaeger:14250
    tls:
      insecure: true

service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [jaeger]
```

**2. Configure Flowplane:**

```bash
# Control plane environment
FLOWPLANE_ENABLE_TRACING=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
```

**3. Configure Envoy Listeners:**

```bash
# Create collector cluster
curl -X POST http://flowplane:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "otel_collector",
    "team": "production",
    "endpointMode": "dns",
    "endpoints": ["otel-collector:4317"]
  }'

# Create traced listener
curl -X POST http://flowplane:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "gateway-listener",
    "team": "production",
    "address": "0.0.0.0",
    "port": 8080,
    "filterChains": [{
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "api-routes",
        "tracing": {
          "provider": {
            "type": "open_telemetry",
            "service_name": "api-gateway",
            "grpc_cluster": "otel_collector"
          },
          "randomSamplingPercentage": 100.0,
          "spawnUpstreamSpan": true
        }
      }]
    }]
  }'
```

**4. View Traces:**

Open Jaeger UI at `http://jaeger:16686` and search for:
- Service: `api-gateway` (Envoy traces)
- Service: `flowplane-control-plane` (Control plane traces)
- Trace ID: Copy from access logs or response headers

### Sampling Strategies

**Development (100% sampling):**
```json
{
  "randomSamplingPercentage": 100.0
}
```

**Production (1-10% sampling):**
```json
{
  "randomSamplingPercentage": 5.0,
  "customTags": {
    "environment": "production"
  }
}
```

**Error-focused (sample all errors):**
Configure Envoy to always sample 5xx responses using custom sampling rules in the tracing provider.

## Troubleshooting

### No Traces Appearing

1. **Verify collector connectivity:**
   ```bash
   # Check Envoy can reach collector
   kubectl exec -it envoy-pod -- curl -v otel-collector:4317
   ```

2. **Check sampling rate:**
   - Ensure `randomSamplingPercentage` > 0

3. **Verify cluster configuration:**
   ```bash
   curl http://flowplane:8080/api/v1/clusters/otel_collector
   ```

### Missing Trace Context in Access Logs

1. **Verify Envoy tracing is enabled:**
   Check listener configuration includes tracing block

2. **Check traceparent header propagation:**
   Add `%REQ(traceparent)%` to Envoy access log format

3. **Verify header extraction:**
   Check Flowplane logs for "Extracted W3C TraceContext" debug messages

### Trace Context Not Propagating

1. **Check W3C TraceContext headers:**
   ```bash
   curl -v -H "traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01" \
     http://envoy:8080/api/test
   ```

2. **Verify Envoy configuration:**
   Ensure `spawn_upstream_span: true` if you want upstream spans

## Performance Considerations

- **Sampling**: Use 1-10% sampling in production to reduce overhead
- **Custom Tags**: Limit to essential tags (each tag adds serialization cost)
- **Collector Buffering**: Configure `max_cache_size` to handle collector outages
- **gRPC vs HTTP**: gRPC export is generally more efficient than HTTP

## Related Documentation

- [Operations Guide](operations.md) - General operations and monitoring
- [Listener Cookbook](listener-cookbook.md) - Listener configuration examples
- [Filters Guide](filters.md) - HTTP filter configuration
