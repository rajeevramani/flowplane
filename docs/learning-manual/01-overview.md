# Learning Feature Overview

The Flowplane Learning Feature provides automated API schema discovery through traffic observation. This system learns the structure of your APIs by analyzing actual request/response traffic, then aggregates this knowledge into consensus schemas that can be exported as OpenAPI specifications.

## Core Concepts

### Learning Gateway

The Learning Gateway is Flowplane's system for automatically discovering API schemas from live traffic. Instead of manually writing OpenAPI specifications, you can deploy Flowplane to observe your APIs in action and learn their structures automatically.

**Key Benefits:**
- Zero-effort schema discovery from production traffic
- Accurate schemas based on real data, not documentation
- Automatic detection of optional vs required fields
- Breaking change detection between schema versions
- Confidence scoring to assess schema reliability

### Learning Sessions

A learning session is a time-bounded period during which Flowplane captures and analyzes API traffic for specific routes. Sessions are the fundamental unit of schema learning.

**Session Lifecycle:**
```
pending → active → completing → completed
            ↓
         cancelled
            ↓
          failed
```

**Session States:**

| State | Description |
|-------|-------------|
| `pending` | Session created, not yet activated |
| `active` | Actively capturing traffic samples |
| `completing` | Target reached, aggregating schemas |
| `completed` | Successfully finished, schemas available |
| `cancelled` | Manually cancelled by user |
| `failed` | Error occurred during processing |

### Schema Inference

When traffic flows through an active learning session, Flowplane:

1. **Captures request/response metadata** via the Access Log Service (ALS)
2. **Captures request/response bodies** via the External Processing Service (ExtProc)
3. **Infers JSON schemas** from the captured bodies
4. **Stores observations** in the `inferred_schemas` table

Each observation contains:
- HTTP method and path
- Status code
- Request schema (if body present)
- Response schema (if body present)
- Timestamp

### Schema Aggregation

When a session completes (reaches target count or times out), Flowplane aggregates all observations into consensus schemas:

**Aggregation Process:**
1. **Group observations** by (method, path, status_code)
2. **Merge schemas** combining all observed fields
3. **Track field presence** to determine required vs optional
4. **Resolve type conflicts** using union types (`oneOf`)
5. **Calculate confidence scores** based on sample quality
6. **Detect breaking changes** from previous versions

**Confidence Score Calculation:**
```
confidence = (sample_score * 0.4) + (field_score * 0.4) + (type_score * 0.2)

Where:
- sample_score: ln(sample_count) / ln(100), max 1.0
- field_score: required_fields / total_fields
- type_score: stable_fields / total_fields
```

## Architecture Overview

### Traffic Flow

```
Client Request
    ↓
Envoy Proxy (configured by Flowplane)
    ↓
┌─────────────────────────────────────┐
│ Access Log Service (ALS)             │
│ - Records: method, path, status      │
│ - Filters by route pattern           │
│ - Filters by HTTP methods            │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│ External Processing (ExtProc)        │
│ - Captures: request body             │
│ - Captures: response body            │
│ - JSON content only                  │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│ Schema Inference Engine              │
│ - Parses JSON bodies                 │
│ - Generates JSON Schema              │
│ - Stores inferred schemas            │
└─────────────────────────────────────┘
    ↓
Backend Service (your API)
```

### Key Components

| Component | File | Purpose |
|-----------|------|---------|
| LearningSessionService | `src/services/learning_session_service.rs` | Session lifecycle management |
| SchemaAggregator | `src/services/schema_aggregator.rs` | Combines observations into schemas |
| LearningSessionRepository | `src/storage/repositories/learning_session.rs` | Database operations |
| LearningSessions Handler | `src/api/handlers/learning_sessions.rs` | REST API endpoints |
| AggregatedSchemas Handler | `src/api/handlers/aggregated_schemas.rs` | Schema API endpoints |

### Database Tables

**learning_sessions** - Stores session configuration and state
**inferred_schemas** - Stores individual observations
**aggregated_api_schemas** - Stores combined schemas with confidence scores

## Multi-Tenancy

All learning data is team-scoped:

- Sessions belong to a team
- Inferred schemas belong to a session (and thus a team)
- Aggregated schemas belong to a team
- Team isolation is enforced at the database query level

**Authorization Scopes:**
- `learning-sessions:read` - View sessions
- `learning-sessions:write` - Create/activate sessions
- `learning-sessions:delete` - Cancel sessions
- `schemas:read` - View aggregated schemas

## OpenAPI Export

Aggregated schemas can be exported as OpenAPI 3.1 specifications:

**Single Schema Export:**
```
GET /api/v1/aggregated-schemas/{id}/export?includeMetadata=true
```

**Multi-Schema Export:**
```
POST /api/v1/aggregated-schemas/export
{
  "schemaIds": [1, 2, 3],
  "title": "My API",
  "version": "1.0.0",
  "includeMetadata": true
}
```

**Flowplane Extensions:**
When `includeMetadata: true`, schemas include:
- `x-flowplane-confidence`: Confidence score (0.0-1.0)
- `x-flowplane-sample-count`: Number of samples
- `x-flowplane-first-observed`: First sample timestamp
- `x-flowplane-last-observed`: Last sample timestamp

## Quick Start

1. **Create a learning session:**
```bash
POST /api/v1/learning-sessions
{
  "routePattern": "^/api/v1/users/.*",
  "targetSampleCount": 100,
  "maxDurationSeconds": 3600
}
```

2. **Generate traffic** to the endpoints matching your route pattern

3. **Monitor progress:**
```bash
GET /api/v1/learning-sessions/{session_id}
```

4. **View aggregated schemas** (after completion):
```bash
GET /api/v1/aggregated-schemas?path=users
```

5. **Export as OpenAPI:**
```bash
GET /api/v1/aggregated-schemas/{id}/export
```

## Best Practices

**Choosing Route Patterns:**
- Use `^` to anchor at start: `^/api/users/.*`
- Use `.*` for wildcards, not `*`
- Test patterns with regex101.com before using

**Setting Sample Counts:**
- Simple CRUD APIs: 20-50 samples
- Complex APIs with optional fields: 100-200 samples
- For high confidence: 500+ samples

**Session Duration:**
- High-traffic routes: 30-60 minutes
- Low-traffic routes: 4-8 hours
- Always set a timeout to prevent stuck sessions

**Interpreting Confidence:**
- 90%+: Production-ready schema
- 70-89%: Good for documentation
- Below 70%: Increase sample count
