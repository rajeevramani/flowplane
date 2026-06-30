# Learn and publish an API spec version

> Audience: api-teams · Status: stable

**Task:** Run a learning session, then generate and publish a learned OpenAPI spec version.

This guide assumes you already have a context configured (`flowplane config set-context` / `use-context`), a team you can write to, and an existing **API definition** that the learning session will attach to. It covers the operator workflow and needs nothing else to complete.

## Learning vs discovery

- **Learning** captures traffic flowing through a route on an *already-configured* listener/route and attaches it to an existing API definition.
- **Discovery** spins up a *throwaway* listener that proxies to an upstream you name, captures that traffic, and creates new API definition(s) for you.

This recipe uses **learning** because we are enriching an existing API. If you instead need discovery, start it at `POST /api/v1/teams/{team}/learning-discovery-sessions` (CLI: `flowplane learn discover start`) and generate specs with the **plural** `.../spec-versions` endpoint (`flowplane learn discover generate-spec`). The rest of this guide is learning-only.

## 1. Start a learning session

A learning session must be attached to an API definition (by name via `api`, or by id via `api_definition_id`). Without it, spec generation in step 4 fails with `learning session is not attached to an API definition`.

**Endpoint**

```
POST /api/v1/teams/{team}/learning-sessions
```

**Body** (`StartLearningSessionBody`, unknown fields rejected):

```json
{
  "name": "orders-learn-2026-06",
  "api": "orders-api",
  "target_sample_count": 1000,
  "max_bytes": 10485760,
  "max_distinct_paths": 500
}
```

Field notes:

- `name` (required) — session name, used later as the `{session}` path segment.
- `api` — API definition **name** to attach to. Alternatively pass `api_definition_id` (UUID). Pass exactly one target: `api`, `api_definition_id`, or `route_config_id`.
- `route_config_id` — attach the learning session to a route config instead of directly to an API. Use this when you want to scope capture by listener / virtual host / route.
- `listener_id`, `virtual_host`, `route` — optional scoping only when `route_config_id` is the target. They cannot be combined with `api` or `api_definition_id`.
- `target_sample_count` (default `1000`), `max_bytes` (default `10485760` = 10 MiB), `max_distinct_paths` (default `500`), `max_duration_seconds` (optional) — capture stop limits.

When you attach the session with `api` / `api_definition_id`, Flowplane scopes capture through
that API definition's route binding. Do not also pass route-config scoping fields; the API rejects
mixed targets with `validation_failed`.

Returns `201` with a `LearningSessionView` (status, counters such as `sample_count` / `byte_count` / `path_count`).

**CLI**

```bash
flowplane learn start orders-learn-2026-06 \
  --team my-team \
  --api orders-api \
  --target-sample-count 1000
```

Route-config-scoped learning uses the route target instead:

```bash
flowplane learn start orders-route-learn-2026-06 \
  --team my-team \
  --route-config-id 019f0000-0000-7000-8000-000000000001 \
  --listener-id 019f0000-0000-7000-8000-000000000002 \
  --virtual-host default \
  --route all
```

## 2. Drive traffic and let it capture

Send representative requests through the route the session is watching (your normal clients, a smoke test, or a replay). The session records samples until it hits one of the configured limits (`target_sample_count`, `max_bytes`, `max_distinct_paths`, or `max_duration_seconds`).

Watch progress:

```bash
flowplane learn get orders-learn-2026-06 --team my-team
# GET /api/v1/teams/{team}/learning-sessions/{session}
```

Check `sample_count` / `path_count` — you need at least one observation, otherwise spec generation fails with `learning session has no raw observations to aggregate`.

## 3. Stop the session

Stopping transitions the session to **Completed**, which is required before generating a spec.

**Endpoint**

```
POST /api/v1/teams/{team}/learning-sessions/{session}/stop
```

**CLI**

```bash
flowplane learn stop orders-learn-2026-06 --team my-team
```

## 4. Generate the learned spec version

Aggregates the captured observations into an OpenAPI spec version on the attached API definition. Note the **singular** path segment for learning: `spec-version`.

**Endpoint**

```
POST /api/v1/teams/{team}/learning-sessions/{session}/spec-version
```

Returns `201` with a `LearnedSpecVersionView`:

```json
{
  "id": "…",
  "api_definition_id": "…",
  "version": 3,
  "source_kind": "learned",
  "format": "openapi3",
  "spec_hash": "…",
  "created_at": "2026-06-20T…"
}
```

Take note of `version` — you need it to publish in the next step.

**CLI**

```bash
flowplane learn generate-spec orders-learn-2026-06 --team my-team
```

## 5. Publish the spec version

Publishing marks the spec as the API's published version, regenerates the API's MCP tools from the OpenAPI operations, and returns the new tool count.

**Endpoint**

```
POST /api/v1/teams/{team}/api-definitions/{name}/specs/{version}/publish
```

`{name}` is the API definition name (`orders-api`), `{version}` is the integer version from step 4. A JSON body (`SpecReviewBody`) is required, but its only field — `reason` — is optional, so send `{}` or include a reason:

```json
{ "reason": "Promoting learned spec from June capture" }
```

Returns `200` with a `PublishSpecView` (`spec` summary + `tool_count`).

**CLI**

```bash
flowplane api spec publish orders-api 3 \
  --team my-team \
  --reason "Promoting learned spec from June capture"
```

To reject instead of publish, use `.../specs/{version}/reject` (`flowplane api spec reject <api> <version>`).

## 6. Verify the published spec and generated tools

Read the API status — it returns the API definition (with `published_spec_version_id` set), the `latest_spec` summary, and `tool_count`.

**Endpoint**

```
GET /api/v1/teams/{team}/api-definitions/{name}/status
```

**CLI**

```bash
flowplane api status orders-api --team my-team
```

Confirm:

- `api.published_spec_version_id` matches the spec you just published.
- `tool_count` is greater than zero (the published operations became MCP tools).

To inspect or toggle an individual generated tool, use `flowplane mcp status` / `flowplane mcp enable --api <tool-name>` / `flowplane mcp disable --api <tool-name>` (REST: `PATCH /api/v1/teams/{team}/mcp/tools/{name}`).

## Further reading

- [import-and-publish-openapi-spec](import-and-publish-openapi-spec.md) — the same publish gate, but for a spec you already have (imported via `--from-openapi`) rather than one learned from traffic.
- Design reference (optional): [`spec/06-learning.md`](../../spec/06-learning.md) — how captured traffic is aggregated into an OpenAPI spec. Not needed to complete this guide.
