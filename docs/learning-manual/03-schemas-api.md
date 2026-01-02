# Aggregated Schemas API Reference

This document provides a complete API reference for the Aggregated Schemas endpoints.

## Base URL

All endpoints are prefixed with `/api/v1/aggregated-schemas`

## Authentication

All endpoints require authentication via Bearer token with `schemas:read` scope.

---

## List Aggregated Schemas

Lists aggregated schemas for the authenticated user's team.

**Endpoint:** `GET /api/v1/aggregated-schemas`

**Required Scope:** `schemas:read`

### Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `path` | string | null | Filter by path substring |
| `method` | string | null | Filter by HTTP method |
| `minConfidence` | number | null | Minimum confidence score (0.0-1.0) |
| `limit` | integer | 50 | Max results to return |
| `offset` | integer | 0 | Pagination offset |

### Response

**Status:** `200 OK`

```json
[
  {
    "id": 1,
    "team": "my-team",
    "path": "/api/v1/users/{id}",
    "httpMethod": "GET",
    "version": 2,
    "previousVersionId": 1,
    "requestSchema": null,
    "responseSchemas": {
      "200": {
        "type": "object",
        "required": ["id", "name", "email"],
        "properties": {
          "id": {"type": "integer"},
          "name": {"type": "string"},
          "email": {"type": "string"},
          "age": {"type": "integer"}
        }
      },
      "404": {
        "type": "object",
        "required": ["error"],
        "properties": {
          "error": {"type": "string"},
          "message": {"type": "string"}
        }
      }
    },
    "sampleCount": 150,
    "confidenceScore": 0.92,
    "breakingChanges": null,
    "firstObserved": "2025-01-10T08:00:00Z",
    "lastObserved": "2025-01-15T14:30:00Z",
    "createdAt": "2025-01-15T14:35:00Z",
    "updatedAt": "2025-01-15T14:35:00Z"
  }
]
```

### Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique schema identifier |
| `team` | string | Team that owns this schema |
| `path` | string | Normalized API path (with `{id}` placeholders) |
| `httpMethod` | string | HTTP method |
| `version` | integer | Schema version number |
| `previousVersionId` | integer | ID of previous version (null if first) |
| `requestSchema` | object | JSON Schema for request body |
| `responseSchemas` | object | Map of status codes to JSON Schemas |
| `sampleCount` | integer | Total samples aggregated |
| `confidenceScore` | number | Confidence score (0.0-1.0) |
| `breakingChanges` | array | List of breaking changes from previous version |
| `firstObserved` | string | Timestamp of first observation |
| `lastObserved` | string | Timestamp of last observation |
| `createdAt` | string | When this schema version was created |
| `updatedAt` | string | When this schema was last modified |

### Examples

```bash
# Get all schemas
curl "http://localhost:8080/api/v1/aggregated-schemas" \
  -H "Authorization: Bearer $TOKEN"

# Filter by path
curl "http://localhost:8080/api/v1/aggregated-schemas?path=users" \
  -H "Authorization: Bearer $TOKEN"

# Filter by method and minimum confidence
curl "http://localhost:8080/api/v1/aggregated-schemas?method=POST&minConfidence=0.8" \
  -H "Authorization: Bearer $TOKEN"
```

---

## Get Aggregated Schema

Retrieves details for a specific aggregated schema.

**Endpoint:** `GET /api/v1/aggregated-schemas/{schema_id}`

**Required Scope:** `schemas:read`

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `schema_id` | integer | Schema identifier |

### Response

**Status:** `200 OK`

Returns the full schema object (same format as list response).

### Error Responses

**404 Not Found:**
```json
{
  "error": "Aggregated schema with ID '123' not found"
}
```

---

## Compare Schema Versions

Compares two versions of a schema to identify differences.

**Endpoint:** `GET /api/v1/aggregated-schemas/{schema_id}/compare`

**Required Scope:** `schemas:read`

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `schema_id` | integer | Current schema version ID |

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `withVersion` | integer | Yes | ID of schema version to compare against |

### Response

**Status:** `200 OK`

```json
{
  "currentSchema": {
    "id": 5,
    "version": 2,
    "path": "/api/v1/users/{id}",
    "httpMethod": "GET",
    ...
  },
  "comparedSchema": {
    "id": 1,
    "version": 1,
    ...
  },
  "differences": {
    "versionChange": 1,
    "sampleCountChange": 100,
    "confidenceChange": 0.15,
    "hasBreakingChanges": true,
    "breakingChanges": [
      {
        "type": "required_field_removed",
        "path": "response[200].properties.phoneNumber",
        "description": "Required field 'phoneNumber' was removed"
      }
    ]
  }
}
```

### Breaking Change Types

| Type | Description |
|------|-------------|
| `required_field_removed` | A required field was removed from the schema |
| `incompatible_type_change` | A field's type changed incompatibly |
| `field_became_required` | An optional field became required |
| `field_type_narrowed` | A field's type became more restrictive |

---

## Export Single Schema

Exports a single aggregated schema as an OpenAPI 3.1 specification.

**Endpoint:** `GET /api/v1/aggregated-schemas/{schema_id}/export`

**Required Scope:** `schemas:read`

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `schema_id` | integer | Schema identifier |

### Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `includeMetadata` | boolean | true | Include x-flowplane-* extensions |

### Response

**Status:** `200 OK`
**Content-Type:** `application/json`

```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "API Schema - GET /api/v1/users/{id}",
    "version": "2",
    "description": "Learned from 150 samples with 92.0% confidence"
  },
  "paths": {
    "/api/v1/users/{id}": {
      "get": {
        "summary": "GET /api/v1/users/{id}",
        "operationId": "get_api_v1_users_id",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "schema": {"type": "string"}
          }
        ],
        "responses": {
          "200": {
            "description": "Response with status 200",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["id", "name", "email"],
                  "properties": {
                    "id": {"type": "integer"},
                    "name": {"type": "string"},
                    "email": {"type": "string"},
                    "age": {"type": "integer"}
                  },
                  "x-flowplane-sample-count": 150,
                  "x-flowplane-confidence": 0.92,
                  "x-flowplane-first-observed": "2025-01-10T08:00:00Z",
                  "x-flowplane-last-observed": "2025-01-15T14:30:00Z"
                }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {}
  }
}
```

### Flowplane Extensions

When `includeMetadata=true`, the following extensions are added:

| Extension | Type | Description |
|-----------|------|-------------|
| `x-flowplane-sample-count` | integer | Number of samples aggregated |
| `x-flowplane-confidence` | number | Confidence score (0.0-1.0) |
| `x-flowplane-first-observed` | string | First observation timestamp |
| `x-flowplane-last-observed` | string | Last observation timestamp |

---

## Export Multiple Schemas

Combines multiple schemas into a single OpenAPI 3.1 specification.

**Endpoint:** `POST /api/v1/aggregated-schemas/export`

**Required Scope:** `schemas:read`

### Request Body

```json
{
  "schemaIds": [1, 2, 3, 4, 5],
  "title": "User Management API",
  "version": "1.0.0",
  "description": "Complete API specification for user management",
  "includeMetadata": true
}
```

### Request Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schemaIds` | integer[] | Yes | List of schema IDs to export |
| `title` | string | Yes | OpenAPI info.title |
| `version` | string | Yes | OpenAPI info.version |
| `description` | string | No | OpenAPI info.description |
| `includeMetadata` | boolean | No | Include x-flowplane-* extensions (default: true) |

### Response

**Status:** `200 OK`
**Content-Type:** `application/json`

```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "User Management API",
    "version": "1.0.0",
    "description": "Complete API specification for user management\n\nEndpoints: GET /api/v1/users/{id} (150 samples, 92% confidence); POST /api/v1/users (80 samples, 88% confidence)"
  },
  "paths": {
    "/api/v1/users": {
      "get": {...},
      "post": {...}
    },
    "/api/v1/users/{id}": {
      "get": {...},
      "put": {...},
      "delete": {...}
    }
  },
  "components": {
    "schemas": {}
  }
}
```

### Error Responses

**400 Bad Request:**
```json
{
  "error": "schemaIds array is required"
}
```

**404 Not Found:**
```json
{
  "error": "Schema with ID '999' not found"
}
```

---

## Schema Structure Details

### Path Normalization

Flowplane normalizes path parameters:

| Original Path | Normalized Path |
|---------------|-----------------|
| `/api/v1/users/123` | `/api/v1/users/{id}` |
| `/api/v1/orders/456/items/789` | `/api/v1/orders/{id}/items/{id}` |
| `/api/v1/teams/abc-team/members` | `/api/v1/teams/{id}/members` |

### Request Schema

The `requestSchema` field contains the learned JSON schema for request bodies:

```json
{
  "type": "object",
  "required": ["name", "email"],
  "properties": {
    "name": {"type": "string"},
    "email": {"type": "string"},
    "age": {"type": "integer"}
  }
}
```

- **required**: Fields present in 100% of observations
- **properties**: All observed fields

### Response Schemas

The `responseSchemas` field maps status codes to schemas:

```json
{
  "200": {...},
  "201": {...},
  "400": {...},
  "404": {...},
  "500": {...}
}
```

Each status code has its own learned schema based on observed responses.

### Type Conflicts

When a field has inconsistent types across observations:

```json
{
  "userId": {
    "type": {"oneOf": ["string", "integer"]}
  }
}
```

This indicates the API returns both strings and integers for `userId`.

---

## Confidence Score Interpretation

| Score | Interpretation | Recommended Use |
|-------|----------------|-----------------|
| 0.95+ | Very high confidence | Contract testing, validation |
| 0.85-0.94 | High confidence | Documentation, SDK generation |
| 0.70-0.84 | Medium confidence | Internal documentation |
| Below 0.70 | Low confidence | Needs more samples |

### Factors Affecting Confidence

1. **Sample Size (40% weight)**
   - More samples = higher confidence
   - Max contribution at 100+ samples

2. **Field Consistency (40% weight)**
   - Higher ratio of required fields = higher confidence
   - Many optional fields = lower confidence

3. **Type Stability (20% weight)**
   - No type conflicts = full contribution
   - Type conflicts (oneOf) reduce score

---

## Example Workflows

### Get High-Confidence Schemas for Documentation

```bash
curl "http://localhost:8080/api/v1/aggregated-schemas?minConfidence=0.85" \
  -H "Authorization: Bearer $TOKEN"
```

### Export All User Schemas

```bash
# 1. Get user-related schemas
SCHEMA_IDS=$(curl -s "http://localhost:8080/api/v1/aggregated-schemas?path=users" \
  -H "Authorization: Bearer $TOKEN" | jq '[.[].id]')

# 2. Export as unified OpenAPI
curl -X POST http://localhost:8080/api/v1/aggregated-schemas/export \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"schemaIds\": $SCHEMA_IDS,
    \"title\": \"User API\",
    \"version\": \"1.0.0\",
    \"includeMetadata\": false
  }" > user-api.openapi.json
```

### Compare Schema Versions

```bash
# Get current version
curl "http://localhost:8080/api/v1/aggregated-schemas/5" \
  -H "Authorization: Bearer $TOKEN"

# Compare with previous version
curl "http://localhost:8080/api/v1/aggregated-schemas/5/compare?withVersion=1" \
  -H "Authorization: Bearer $TOKEN"
```

### Use Exported Schema with Swagger UI

```bash
# Export schema
curl "http://localhost:8080/api/v1/aggregated-schemas/1/export?includeMetadata=false" \
  -H "Authorization: Bearer $TOKEN" -o openapi.json

# Start Swagger UI
docker run -p 8081:8080 \
  -e SWAGGER_JSON=/openapi/openapi.json \
  -v $(pwd):/openapi \
  swaggerapi/swagger-ui
```
