# Learning Workflow Guide

This guide walks you through the complete workflow for learning API schemas from live traffic.

## Table of Contents

1. [End-to-End Workflow Overview](#end-to-end-workflow-overview)
2. [Step 1: Create a Learning Session](#step-1-create-a-learning-session)
3. [Step 2: Generate Traffic](#step-2-generate-traffic)
4. [Step 3: Monitor Progress](#step-3-monitor-progress)
5. [Step 4: Review Learned Schemas](#step-4-review-learned-schemas)
6. [Step 5: Export to OpenAPI](#step-5-export-to-openapi)
7. [Best Practices](#best-practices)

---

## End-to-End Workflow Overview

```
Create Session (pending)
    ↓
Activate Session (active) → Register with ALS & ExtProc
    ↓
Generate Traffic → Capture request/response bodies
    ↓
Reach Target Samples → Auto-complete (completing → completed)
    ↓
Schema Aggregation → Combine into consensus schemas
    ↓
Export to OpenAPI → Download specification
```

---

## Step 1: Create a Learning Session

### Basic Session Creation

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

### Choosing the Route Pattern

The `routePattern` is a regular expression that matches against request paths:

| Pattern | Matches | Example |
|---------|---------|---------|
| `^/api/v1/users/.*` | All user endpoints | `/api/v1/users/123` |
| `^/api/v1/orders/[0-9]+$` | Order by ID only | `/api/v1/orders/12345` |
| `^/api/.*` | All API routes | `/api/v1/users`, `/api/v2/products` |
| `^/health$` | Exact match | `/health` only |

**Regex Tips:**
- Use `^` to anchor at start
- Use `$` to anchor at end
- `.*` matches any characters
- `[0-9]+` matches one or more digits
- Test patterns at regex101.com

### Setting Target Sample Count

| API Complexity | Recommended Count | Rationale |
|----------------|-------------------|-----------|
| Simple CRUD | 20-50 | Few fields, consistent structure |
| Medium API | 50-100 | Some optional fields |
| Complex API | 100-500 | Many optional fields, polymorphic |
| Dynamic API | 500-1000 | High variability |

### Filtering by HTTP Methods

```bash
curl -X POST http://localhost:8080/api/v1/learning-sessions \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routePattern": "^/api/v1/customers/.*",
    "httpMethods": ["GET", "POST"],
    "targetSampleCount": 100,
    "maxDurationSeconds": 7200
  }'
```

If `httpMethods` is null or omitted, all methods are captured.

### Deployment Tracking

Add metadata for CI/CD integration:

```json
{
  "routePattern": "^/api/v3/.*",
  "targetSampleCount": 200,
  "triggeredBy": "ci-cd-pipeline",
  "deploymentVersion": "v3.1.0"
}
```

---

## Step 2: Generate Traffic

Once your session is **active**, it captures matching traffic automatically.

### Traffic Flow Through Flowplane

```
Client Request
    ↓
Envoy Proxy
    ↓
[Access Log Service] → Records: method, path, status
    ↓
[ExtProc Service] → Captures: request/response bodies
    ↓
[Schema Inference] → Extracts JSON schema
    ↓
Backend Service
```

### Example: Generate Traffic for User API

```bash
# GET requests
curl http://localhost:8080/api/v1/users/123
curl http://localhost:8080/api/v1/users/456/profile

# POST request
curl -X POST http://localhost:8080/api/v1/users \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice", "email": "alice@example.com"}'

# PUT request
curl -X PUT http://localhost:8080/api/v1/users/123 \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice Updated", "age": 30}'

# Error response (404)
curl http://localhost:8080/api/v1/users/nonexistent
```

### What Gets Captured

- **Request metadata**: HTTP method, path, timestamp
- **Request body**: Full JSON payload (if present)
- **Response metadata**: Status code, timestamp
- **Response body**: Full JSON payload (if present)

> **Note:** Only JSON bodies are captured. Non-JSON content is skipped.

### Using Production Traffic

For production learning, traffic is captured automatically:
- User requests from your application
- Integration test traffic
- Load testing traffic
- Health checks and probes

---

## Step 3: Monitor Progress

### Check Session Status

```bash
curl http://localhost:8080/api/v1/learning-sessions/{session_id} \
  -H "Authorization: Bearer YOUR_TOKEN"
```

**Response:**
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "active",
  "targetSampleCount": 100,
  "currentSampleCount": 47,
  "progressPercentage": 47.0,
  "endsAt": "2025-01-15T11:30:00Z"
}
```

### Progress Percentage

```
progressPercentage = (currentSampleCount / targetSampleCount) * 100
```

Progress can exceed 100% if more samples are captured than the target.

### Session Status Lifecycle

| Status | Description | Next State |
|--------|-------------|------------|
| `pending` | Created, not active | → `active` |
| `active` | Capturing traffic | → `completing` |
| `completing` | Aggregating schemas | → `completed` |
| `completed` | Finished | Terminal |
| `cancelled` | User cancelled | Terminal |
| `failed` | Error occurred | Terminal |

### Auto-Completion Triggers

Session transitions from `active` to `completing` when:

1. **Target reached**: `currentSampleCount >= targetSampleCount`
2. **Timeout**: `now >= endsAt` (if `maxDurationSeconds` was set)

> The background worker checks completion every 30 seconds.

### List All Sessions

```bash
# All sessions
curl "http://localhost:8080/api/v1/learning-sessions" \
  -H "Authorization: Bearer YOUR_TOKEN"

# Filter by status
curl "http://localhost:8080/api/v1/learning-sessions?status=active" \
  -H "Authorization: Bearer YOUR_TOKEN"
```

---

## Step 4: Review Learned Schemas

### Schema Aggregation Process

After session completion:

1. **Group by endpoint**: (method, path, status_code)
2. **Merge schemas**: Combine all observations
3. **Track field presence**: Determine required vs optional
4. **Resolve type conflicts**: Use `oneOf` for mixed types
5. **Calculate confidence**: Based on samples, consistency, stability
6. **Detect breaking changes**: Compare with previous version

### List Aggregated Schemas

```bash
curl "http://localhost:8080/api/v1/aggregated-schemas?path=users&minConfidence=0.8" \
  -H "Authorization: Bearer YOUR_TOKEN"
```

### Understanding the Schema Structure

**Path Normalization:**
- `/api/v1/users/123` → `/api/v1/users/{id}`
- `/api/v1/orders/456/items/789` → `/api/v1/orders/{id}/items/{id}`

**Required Fields:**
Fields that appear in 100% of observations are marked `required`.

**Response Schemas:**
Each status code has its own schema:
```json
{
  "responseSchemas": {
    "200": {...},
    "404": {...},
    "500": {...}
  }
}
```

### Confidence Score

```
confidence = (sample_score * 0.4) + (field_score * 0.4) + (type_score * 0.2)
```

| Score Range | Interpretation |
|-------------|----------------|
| 0.95+ | Production-ready |
| 0.85-0.94 | High confidence |
| 0.70-0.84 | Medium confidence |
| < 0.70 | Needs more samples |

### Compare Schema Versions

```bash
curl "http://localhost:8080/api/v1/aggregated-schemas/5/compare?withVersion=1" \
  -H "Authorization: Bearer YOUR_TOKEN"
```

**Breaking Change Types:**
- `required_field_removed`: Required field was removed
- `incompatible_type_change`: Field type changed incompatibly
- `field_became_required`: Optional field became required

---

## Step 5: Export to OpenAPI

### Export Single Schema

```bash
curl "http://localhost:8080/api/v1/aggregated-schemas/1/export?includeMetadata=true" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -o schema.openapi.json
```

### Export Without Metadata

```bash
curl "http://localhost:8080/api/v1/aggregated-schemas/1/export?includeMetadata=false" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -o schema.openapi.json
```

This produces a clean OpenAPI spec without `x-flowplane-*` fields.

### Export Multiple Schemas

```bash
curl -X POST http://localhost:8080/api/v1/aggregated-schemas/export \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "schemaIds": [1, 2, 3, 4, 5],
    "title": "User Management API",
    "version": "v1.0.0",
    "description": "Complete API specification",
    "includeMetadata": true
  }' -o api.openapi.json
```

### Flowplane Extensions

When `includeMetadata: true`:

| Extension | Description |
|-----------|-------------|
| `x-flowplane-sample-count` | Number of samples |
| `x-flowplane-confidence` | Confidence score |
| `x-flowplane-first-observed` | First observation timestamp |
| `x-flowplane-last-observed` | Last observation timestamp |

### Use with Swagger UI

```bash
docker run -p 8081:8080 \
  -e SWAGGER_JSON=/openapi/api.openapi.json \
  -v $(pwd):/openapi \
  swaggerapi/swagger-ui
```

### Generate Client SDK

```bash
npx @openapitools/openapi-generator-cli generate \
  -i api.openapi.json \
  -g typescript-axios \
  -o ./src/generated/api-client
```

---

## Best Practices

### Route Pattern Design

**Match specific endpoints:**
```regex
^/api/v1/users$                    # Exact: /api/v1/users
^/api/v1/users/[0-9]+$             # User by ID
^/api/v1/users/[0-9]+/profile$     # User profile
```

**Match groups:**
```regex
^/api/v1/users/.*                  # All user endpoints
^/api/v[0-9]+/.*                   # All versioned APIs
```

**Common mistakes:**
- ❌ `/api/users` - Missing `^` anchor
- ❌ `^/api/users/.*$` - Too restrictive
- ✅ `^/api/users/.*` - Correct

### Traffic Patterns

**Development/Staging:**
- Sample count: 20-50
- Duration: 15-30 minutes
- Pattern: Specific endpoints

**Production (Low Traffic):**
- Sample count: 100-200
- Duration: 2-4 hours
- Pattern: Broader routes

**Production (High Traffic):**
- Sample count: 50-100
- Duration: 5-10 minutes
- Caution: Monitor storage impact

**Load Testing:**
- Sample count: 500-1000
- Duration: Test duration
- Benefit: Captures edge cases

### Handling Type Conflicts

When fields have inconsistent types:

```json
{"userId": {"type": {"oneOf": ["string", "integer"]}}}
```

**Solutions:**
1. Fix the API for consistent types
2. Filter by HTTP methods
3. Accept and document the variation

### Session Health Monitoring

```bash
# Sessions stuck at 0 progress
curl "http://localhost:8080/api/v1/learning-sessions?status=active" \
  -H "Authorization: Bearer $TOKEN" | jq '.[] | select(.progressPercentage == 0)'

# Low sample count after completion
curl "http://localhost:8080/api/v1/learning-sessions?status=completed" \
  -H "Authorization: Bearer $TOKEN" | jq '.[] | select(.currentSampleCount < 10)'
```

---

## Complete Example Script

```bash
#!/bin/bash
set -e

API_URL="http://localhost:8080/api/v1"
TOKEN="YOUR_TOKEN"

# Step 1: Create session
echo "Creating learning session..."
SESSION_ID=$(curl -s -X POST "$API_URL/learning-sessions" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routePattern": "^/api/v1/users/.*",
    "targetSampleCount": 50,
    "maxDurationSeconds": 1800,
    "httpMethods": ["GET", "POST", "PUT"]
  }' | jq -r '.id')

echo "Session created: $SESSION_ID"

# Step 2: Wait for completion
echo "Waiting for session to complete..."
while true; do
  STATUS=$(curl -s "$API_URL/learning-sessions/$SESSION_ID" \
    -H "Authorization: Bearer $TOKEN" | jq -r '.status')
  PROGRESS=$(curl -s "$API_URL/learning-sessions/$SESSION_ID" \
    -H "Authorization: Bearer $TOKEN" | jq -r '.progressPercentage')

  echo "Status: $STATUS, Progress: $PROGRESS%"

  if [ "$STATUS" = "completed" ]; then
    echo "Session completed!"
    break
  elif [ "$STATUS" = "failed" ]; then
    echo "Session failed!"
    exit 1
  fi

  sleep 10
done

# Step 3: Get schemas
echo "Fetching aggregated schemas..."
SCHEMA_IDS=$(curl -s "$API_URL/aggregated-schemas?path=users" \
  -H "Authorization: Bearer $TOKEN" | jq -r '.[].id' | tr '\n' ',' | sed 's/,$//')

# Step 4: Export OpenAPI
echo "Exporting OpenAPI specification..."
curl -s -X POST "$API_URL/aggregated-schemas/export" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"schemaIds\": [$SCHEMA_IDS],
    \"title\": \"User API\",
    \"version\": \"1.0.0\",
    \"includeMetadata\": false
  }" | jq '.' > user-api.openapi.json

echo "OpenAPI spec saved to user-api.openapi.json"
```
