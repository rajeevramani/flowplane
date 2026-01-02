# Flowplane Learning Feature Manual

Complete documentation for Flowplane's automated API schema discovery system.

## Overview

The Flowplane Learning Feature enables automatic API schema discovery through live traffic observation. Instead of manually writing OpenAPI specifications, you can deploy Flowplane to observe your APIs in action and learn their structures automatically.

**Key Capabilities:**
- Zero-effort schema discovery from production traffic
- Accurate schemas based on real data
- Automatic detection of required vs optional fields
- Breaking change detection between versions
- Confidence scoring to assess schema reliability
- Export to OpenAPI 3.1 specifications

## Documentation Index

### Getting Started

| Document | Description |
|----------|-------------|
| [01-overview.md](01-overview.md) | Core concepts, architecture, and quick start guide |

### API Reference

| Document | Description |
|----------|-------------|
| [02-sessions-api.md](02-sessions-api.md) | Learning Sessions API - create, monitor, and manage sessions |
| [03-schemas-api.md](03-schemas-api.md) | Aggregated Schemas API - query, compare, and export schemas |

### User Guides

| Document | Description |
|----------|-------------|
| [04-workflow-guide.md](04-workflow-guide.md) | Step-by-step workflow from session creation to OpenAPI export |
| [05-ui-guide.md](05-ui-guide.md) | Complete UI walkthrough with screenshots and tips |

### Reference

| Document | Description |
|----------|-------------|
| [06-troubleshooting.md](06-troubleshooting.md) | Common issues, error messages, and technical reference |

## Quick Start

### 1. Create a Learning Session

```bash
curl -X POST http://localhost:8080/api/v1/learning-sessions \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routePattern": "^/api/v1/users/.*",
    "targetSampleCount": 100,
    "maxDurationSeconds": 3600
  }'
```

### 2. Generate Traffic

Send requests to endpoints matching your route pattern. The session automatically captures:
- Request/response metadata (method, path, status)
- Request/response bodies (JSON only)

### 3. Monitor Progress

```bash
curl http://localhost:8080/api/v1/learning-sessions/{session_id} \
  -H "Authorization: Bearer YOUR_TOKEN"
```

### 4. Export Schemas

After session completion:

```bash
curl "http://localhost:8080/api/v1/aggregated-schemas/{schema_id}/export" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -o api.openapi.json
```

## Key Concepts

### Session Lifecycle

```
pending → active → completing → completed
            ↓
         cancelled
            ↓
          failed
```

### Confidence Score

Schemas receive a confidence score (0.0-1.0) based on:
- **Sample size** (40% weight) - More samples = higher confidence
- **Field consistency** (40% weight) - More required fields = higher confidence
- **Type stability** (20% weight) - No type conflicts = higher confidence

| Score | Interpretation |
|-------|----------------|
| 0.95+ | Production-ready |
| 0.85-0.94 | High confidence |
| 0.70-0.84 | Medium confidence |
| < 0.70 | Needs more samples |

### Schema Aggregation

When a session completes, individual observations are combined into consensus schemas:
1. Grouped by (method, path, status_code)
2. Fields appearing in 100% of samples marked as `required`
3. Type conflicts resolved using `oneOf`
4. Breaking changes detected from previous versions

## Authorization Scopes

| Operation | Required Scope |
|-----------|----------------|
| Create/view sessions | `learning-sessions:read/write` |
| Cancel sessions | `learning-sessions:delete` |
| View/export schemas | `schemas:read` |

## Best Practices

### Route Patterns
- Use `^` to anchor at start
- Use `.*` for wildcards
- Test patterns at regex101.com

### Sample Counts
- Simple APIs: 20-50 samples
- Complex APIs: 100-500 samples
- Maximum confidence: 500+ samples

### Timeouts
- High traffic: 30-60 minutes
- Low traffic: 4-8 hours
- Always set a timeout to prevent stuck sessions

## Source Code Reference

| Component | Location |
|-----------|----------|
| Session Service | `src/services/learning_session_service.rs` |
| Schema Aggregator | `src/services/schema_aggregator.rs` |
| Sessions Handler | `src/api/handlers/learning_sessions.rs` |
| Schemas Handler | `src/api/handlers/aggregated_schemas.rs` |
| Database Models | `src/storage/repositories/learning_session.rs` |

## Getting Help

- **API Errors**: See [06-troubleshooting.md](06-troubleshooting.md)
- **UI Issues**: See [05-ui-guide.md](05-ui-guide.md#troubleshooting-ui-issues)
- **Workflow Questions**: See [04-workflow-guide.md](04-workflow-guide.md)
