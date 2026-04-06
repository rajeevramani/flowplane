# Learning Quickstart

Discover API schemas from live traffic and export as OpenAPI — zero source code or annotations required.

## Prerequisites

Complete the [Quickstart](quickstart.md) first (Flowplane + Envoy running in dev mode).

## 1. Start a backend

Start the MockBank demo API:

```bash
docker compose -f docker-compose-mockbackend.yml up -d
flowplane expose http://mockbank-api:3000 --name mockbank --path /v2/api
```

```
Exposed 'mockbank' -> http://mockbank-api:3000
  Port:   10001
  Paths:  /v2/api
```

Verify: `curl http://localhost:10001/v2/api/customers` should return JSON.

## 2. Start a learning session

```bash
flowplane learn start \
  --name mockbank-v1 \
  --route-pattern '^/v2/.*' \
  --target-sample-count 50
```

The session activates immediately and begins observing traffic through Envoy.

## 3. Send traffic

Send varied requests through the gateway — different endpoints, methods, and payloads:

```bash
# Collection GETs
for ep in customers accounts transactions cards loans transfers; do
  curl -s http://localhost:10001/v2/api/$ep > /dev/null
done

# Individual resource GETs (triggers path normalization)
for id in 1 2 3; do
  curl -s http://localhost:10001/v2/api/customers/$id > /dev/null
  curl -s http://localhost:10001/v2/api/accounts/$id > /dev/null
done

# Writes
curl -s -X POST http://localhost:10001/v2/api/customers \
  -H "Content-Type: application/json" \
  -d '{"firstName":"Test","lastName":"User","email":"test@example.com","status":"active"}' > /dev/null

curl -s -X PATCH http://localhost:10001/v2/api/customers/2 \
  -H "Content-Type: application/json" \
  -d '{"status":"inactive"}' > /dev/null

# Repeat to build up sample count
for i in $(seq 1 5); do
  for ep in customers accounts transactions cards loans transfers notifications branches atms; do
    curl -s http://localhost:10001/v2/api/$ep > /dev/null
  done
done
```

## 4. Check progress

```bash
flowplane learn get mockbank-v1
```

```
ID                                     Name          Status       Samples  Target   Progress
---------------------------------------------------------------------------------------------
a1b2c3d4-...                           mockbank-v1   completed    52       50       104.0%
```

When samples reach the target, the session completes automatically and schemas are aggregated.

## 5. List discovered schemas

```bash
flowplane schema list
```

```
   ID  Method  Path                                          Confidence  Samples Version
----------------------------------------------------------------------------------------
    1  GET     /v2/api/accounts                                  69.5%        5       1
    2  GET     /v2/api/customers                                 69.5%        5       1
    3  GET     /v2/api/customers/{customerId}                    66.0%        3       1
    4  POST    /v2/api/customers                                 60.0%        1       1
    5  PATCH   /v2/api/customers/{customerId}                    60.0%        1       1
   ...
```

Note the contextual parameter naming: `/customers/{customerId}`, not `/customers/{id}`.

## 6. Inspect a schema

```bash
flowplane schema get 3 -o table
```

```
Schema #3
--------------------------------------------------
  Path:            /v2/api/customers/{customerId}
  Method:          GET
  Confidence:      66.0%
  Samples:         3

  Response 200:
      id: integer
      firstName: string
      lastName: string
      email: string (email)
      dateOfBirth: string (date)
      status: string
      address: object
      ...
```

## 7. Export as OpenAPI

```bash
# All schemas to stdout (YAML)
flowplane schema export --all

# To a file
flowplane schema export --all -o mockbank-api.yaml

# Only high-confidence schemas
flowplane schema export --all --min-confidence 0.7 -o api.yaml

# From a specific session
flowplane learn export --session mockbank-v1 -o mockbank-v1.yaml

# Specific schemas by ID
flowplane schema export --id 1,2,3 -o subset.json
```

The exported spec includes:
- **OpenAPI 3.1** with paths, request/response schemas, and status codes
- **Domain model deduplication** — shared schemas as `$ref` in `components/schemas`
- **Format detection** — `date`, `email`, `uuid`, `ipv4`, `uri` annotations
- **Required fields** — inferred from field presence across samples (PATCH bodies excluded)

## What the learning engine detects

| Feature | How it works |
|---------|-------------|
| **Types** | JSON type inference: string, integer, number, boolean, object, array, null |
| **Formats** | String pattern detection: email, UUID, date, datetime, IPv4, URI |
| **Required fields** | Fields present in 100% of samples marked required (except PATCH bodies) |
| **Path parameters** | Dynamic segments named from context: `{userId}`, `{orderDate}`, `{productCode}` |
| **Enum values** | Low-cardinality strings (≤10 unique, ≥10 samples) promoted to enums |
| **Confidence** | Weighted score: sample size (40%) + field consistency (40%) + type stability (20%) |
| **Domain models** | Structurally identical schemas across endpoints → shared `$ref` |
| **Breaking changes** | Compared against previous schema version when re-learning |

## Auto-aggregate mode

For long-running observation, use auto-aggregate to get periodic snapshots:

```bash
flowplane learn start \
  --name mockbank-continuous \
  --route-pattern '^/v2/.*' \
  --target-sample-count 100 \
  --auto-aggregate
```

The session aggregates every 100 samples and keeps collecting. Stop when ready:

```bash
flowplane learn stop mockbank-continuous
```

## Next steps

- [MCP Tools](mcp.md) — use `cp_create_learning_session` and `cp_export_schema_openapi` from AI agents
- [Getting Started](getting-started.md) — full gateway setup walkthrough
- [Filters](filters.md) — add auth, rate limiting, CORS to your learned endpoints
