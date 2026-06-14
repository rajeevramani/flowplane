# 06 — Learning Pipeline

Behavioral specification of Flowplane v1's traffic-learning subsystem: capture of live HTTP
traffic through Envoy, JSON schema inference, per-endpoint aggregation, OpenAPI 3.1 export, and
generation of dynamic `api_*` MCP tools. Extracted from v1 source (paths cited inline, all
relative to the v1 repo root). Sections marked **[V2 DESIGN]** are proposals, not v1 behavior;
everything else is v1 fact.

Cross-references: data model overlaps spec/03 (persistence); security findings feed spec/08a;
gaps feed spec/08.

---

## 0.1 V2 config-first spine

S8 starts from durable API lifecycle state before any capture injection or inference:

- `api_definitions` are the team-owned roots for an API surface.
- `api_route_bindings` attach an API to existing gateway route scope with same-team typed FKs.
- `spec_versions` hold imported, learned, or manual OpenAPI content as append-only rows with
  deterministic hashes and per-API version numbers.
- `api_tools` are generated rows tied to one spec version; they remain data only until S11 MCP
  serving.
- retention policy rows define raw observation TTL and max retained spec versions for later ingest
  and cleanup.

This replaces v1's manual OpenAPI export/import bridge. Import, learn, review/publish, and tool
generation all converge on the same API definition and spec version tables.

S8.2 exposes this spine through REST and CLI. `flowplane api create NAME --from-openapi
openapi.json` creates an API definition, appends an imported spec version, and generates `api_tools`
rows from OpenAPI HTTP operations. Route binding is supported only to existing gateway route scope
by typed IDs; automatic gateway topology creation from OpenAPI is deferred until the
OpenAPI-to-cluster/listener/route mapping is explicit.

## 0.2 V2 S8.6 aggregation contract

S8.6 starts from a frozen observation set: a completed/stopped capture session or an explicit
immutable snapshot. Deterministic learned specs must not be generated directly from a still-mutating
`capturing` session.

`fp-domain::learning` owns the canonical contract:

- `LearnedSpecCandidate` is the learned OpenAPI candidate before persistence.
- `LearnedEndpointKey` is sorted by `host`, `method`, and `path_template`; grouping is host-aware
  and method-aware.
- One learned OpenAPI document cannot contain two endpoints that flatten to the same
  `(path_template, method)`. Host-distinct collisions must be split into separate learned specs or
  snapshots before rendering OpenAPI.
- `LearnedEndpointAggregate` carries request schema, response schemas, learned headers, and
  confidence metadata.
- `group_observations_by_endpoint()` is the pure S8.6b grouping primitive. It accepts one
  team/session observation set, preserves host separation, templates high-cardinality/id-like path
  segments, keeps stable low-cardinality literals, and buckets path overflow.
- Strong ID signals (UUIDs and all-digit segments) template unconditionally; weaker alphanumeric
  segments such as `v1`, `oauth2`, or `s3` stay literal while low-cardinality so API versions do
  not merge.
- S8.6c JSON schema inference ignores truncated and malformed bodies, infers request and
  per-status response schemas from valid JSON, represents mixed types with `oneOf`, and marks object
  fields required only when they meet the min-sample/frequency threshold. Optional sparse fields do
  not reduce confidence by themselves.
- S8.6d header learning is conservative: structural, auth, volatile tracing/proxy, and
  infrastructure headers are excluded; only allowlisted safe headers can be documented, and only
  after meeting min-sample/frequency thresholds. Header value size and learned header count are
  capped so header floods cannot create unbounded OpenAPI output.
- `LearnedConfidence` is stable review metadata with score, sample count, body coverage, path
  cardinality, truncation, and drop signals including dropped header count.
- `canonical_openapi()` emits OpenAPI 3.1 JSON with deterministic ordering for paths, methods,
  parameters, headers, responses, schemas, and `x-flowplane-learning` metadata.
- `spec_version_input()` returns `source_kind = learned`, `format = openapi3`, and validates
  against the same `SpecVersionInput::validate` path used by imported specs.

Lifecycle state is intentionally not part of `spec_versions` content. `source_kind = learned` is
source/type vocabulary only; candidate/rejected/published state belongs to the S8.7 lifecycle model.

Confidence metadata is embedded in the learned OpenAPI as the stable vendor extension
`x-flowplane-learning`. Because it is part of the candidate spec body, it participates in
`spec_hash`; later changes to confidence scoring intentionally produce a different learned
candidate version. S8.7 may copy selected review/publish state into separate tables, but it must not
mutate the immutable spec content.

---

## 1. End-to-end narrative

```
 operator/agent                Flowplane CP                       Envoy DP
 ──────────────                ────────────                       ────────
 1. routes/listener exist  ──► (prerequisite; learning observes only traffic
                                that already flows through a listener's HCM)
 2. create learning session ─► row in learning_sessions (status=pending)
 3. auto-activate           ─► pending→active; register session with ALS +
                               ExtProc in-memory registries; trigger LDS
                               refresh                            ──► listener re-pushed with:
                                                                      • http_grpc access log
                                                                      • ext_proc HTTP filter
 4. traffic flows           ◄───────────────────────────────────── client requests
 5. capture                 ◄─ ALS gRPC stream (metadata+headers)
                            ◄─ ExtProc gRPC stream (bodies ≤10KB)
 6. worker pipeline:           merge ALS entry + ExtProc body by
                               (session_id, x-request-id) → infer JSON
                               schema per body → normalize path →
                               batch-insert into inferred_schemas
 7. completion:                target samples reached OR timeout →
                               active→completing→completed; aggregate
                               inferred_schemas → aggregated_api_schemas;
                               unregister ALS/ExtProc; LDS refresh removes
                               capture config
 8. export (manual)         ─► GET/POST aggregated-schemas export → OpenAPI 3.1
 9. MCP tools (manual)      ─► enable MCP on a route → route_metadata enriched
                               from aggregated schema (confidence ≥ 0.8) →
                               mcp_tools row → api_* tool served by MCP gateway
```

### 1.1 Automatic vs manual steps (the integration map for v2)

| Step | Automatic in v1? | Trigger |
|---|---|---|
| Route/cluster/listener exist | **Manual** | Operator creates them (or `flowplane expose`, or OpenAPI import) before learning. Learning cannot create them. |
| Session creation | Manual | REST `POST /api/v1/teams/{team}/learning-sessions`, MCP `cp_create_learning_session`, CLI `flowplane learn start`. |
| Session activation | **Automatic** on REST create (handler calls `activate_session` immediately; falls back to `pending` on error — `src/api/handlers/learning_sessions.rs:294-308`). MCP create takes `autoStart`; pending sessions need explicit `.../activate` / `cp_activate_learning_session`. |
| Envoy config change (ALS + ExtProc injection) | **Automatic** on activation: `refresh_listeners_from_repository()` re-pushes ALL listeners with injected capture config (`src/services/learning_session_service.rs:171-191`). |
| Sample counting | Automatic — ALS increments `current_sample_count` per matched entry (`src/xds/services/access_log_service.rs:777-797`). |
| Inference + persistence | Automatic — background worker pool (`src/services/access_log_processor.rs`). |
| Completion check | Automatic — background loop every **30 s** calls `check_all_active_sessions()` (`src/cli/mod.rs:1122-1129`); also a one-time `sync_active_sessions_with_access_log_service()` at startup for restart recovery (`src/cli/mod.rs:1145`). |
| Aggregation | Automatic on completion/snapshot (`complete_session` → `SchemaAggregator::aggregate_session`). |
| OpenAPI export | **Manual** — REST export endpoints / `cp_export_schema_openapi` / `flowplane schema export`. Nothing is persisted; the document is generated on demand. |
| Learned schema → route metadata | **Manual** — only when MCP is enabled on a route (`POST .../routes/{id}/mcp/enable`) or refreshed (`.../mcp/refresh`). No automatic push of new aggregations into existing tools. |
| `api_*` MCP tool creation | **Manual** — `mcp/enable` (per-route) or `mcp/bulk-enable`. OpenAPI import creates `route_metadata` but NOT tools. |
| Learned spec → routes/clusters | **Does not exist** in v1 (see §6). |

---

## 2. Learning session lifecycle

Source: `src/services/learning_session_service.rs`, repository in
`src/storage/repositories/` (`LearningSessionRepository`).

### 2.1 States

`pending → active → completing → completed`, plus terminal `failed` and `cancelled`.
String serialization is lowercase (`"pending"`, `"active"`, `"completing"`, `"completed"`,
`"failed"`, `"cancelled"`).

### 2.2 Transitions

| Transition | Trigger | Side effects |
|---|---|---|
| (create) → pending | REST/MCP/CLI create | Row inserted; name auto-generated from route pattern if absent (`generate_session_name`: strip regex metachars, `/`→`-`, collapse dashes, truncate 48 chars; uniqueness via `-2`, `-3`… suffix up to 100, then UUID suffix). |
| pending → active | `activate_session` (auto on REST create; explicit `/activate` endpoint; MCP `cp_activate_learning_session`) | Sets `started_at`; registers session (id, team, compiled route-pattern regex, methods) with ALS in-memory list and (id, pattern) with ExtProc map; publishes `activated` webhook; triggers LDS refresh injecting capture config. Validation: only from `pending`. Invalid route-pattern regex fails conversion. |
| active → active (snapshot) | `check_completion` when `current_sample_count ≥ target` AND `auto_aggregate=true` AND not timed out | `snapshot_session`: runs aggregation tagged with `(session_id, snapshot_number = snapshot_count+1)`, atomically resets `current_sample_count` to 0 and increments `snapshot_count`; session stays active; `snapshot_completed` webhook. |
| active → completing | `check_completion` (target reached, non-auto-aggregate, OR timeout `ends_at ≤ now` regardless of auto_aggregate), or explicit stop (`/stop`, `cp_stop_learning`) | **Atomic conditional UPDATE** `transition_status(Active→Completing)`; losers of the race return current state — exactly one completer. |
| completing → completed | Same call continues | Unregister from ALS + ExtProc; LDS refresh removes capture config; run `SchemaAggregator::aggregate_session` (failure logged, completion proceeds — inferred rows survive); set `completed_at`; `completed` webhook. |
| any-non-terminal → cancelled | DELETE session endpoint / MCP delete / CLI `learn cancel` | `cancel_session`: status=cancelled, `error_message="Cancelled by user"`, unregister ALS/ExtProc, LDS refresh, webhook (uses the `failed` event constructor). REST handler rejects cancel on completed/cancelled/failed with 400. |
| any → failed | `fail_session(error_message)` (internal error paths) | Same unregister + LDS refresh + `failed` webhook. |

`ends_at` is computed from `max_duration_seconds` at creation (NULL = no timeout). Note: a
session with neither timeout nor target reachable runs indefinitely; auto-aggregate sessions run
indefinitely by design until stopped.

### 2.3 Restart recovery

On CP startup, all `active` sessions are re-registered with the ALS
(`sync_active_sessions_with_access_log_service`). **Gap:** the same sync does NOT re-register
with the ExtProc service — after a CP restart, body capture silently stops for in-flight
sessions until re-activation (v2 must sync both).

---

## 3. Capture path: what Envoy sends and how

Two independent gRPC channels from Envoy to the CP, correlated by `x-request-id`.

### 3.1 Config injected at activation (LDS)

On every `refresh_listeners_from_repository()`, after listeners are built from the DB, two
injection passes run over **all** listeners (`src/xds/filters/injection/learning_session.rs`):

1. **Access log** (`inject_access_logs`): per active session, an
   `envoy.access_loggers.http_grpc` `AccessLog` is appended to every HCM
   (`src/xds/access_log.rs`):
   - `log_name = "flowplane_learning_session_{session_id}"`, gRPC target cluster
     `flowplane_access_log_service` (a CP-built cluster pointing at the xDS gRPC port,
     `src/xds/resources.rs:1057`; carries mTLS transport socket when dataplane TLS is configured).
   - `buffer_size_bytes = 16384`; transport API v3; **no AccessLogFilter** — every request on the
     listener is logged while any session is active; filtering by route pattern happens CP-side.
   - `additional_request_headers_to_log`: content-type, content-length, accept, user-agent,
     authorization, proxy-authorization, x-api-key, x-auth-token, x-request-id,
     x-envoy-original-path. `additional_response_headers_to_log`: content-type, content-length,
     www-authenticate.
   - Duplicate check compares against `AccessLog.name` (`"envoy.access_loggers.http_grpc"`),
     which never contains the session id — effectively a no-op check; correctness relies on each
     refresh rebuilding listeners from clean stored config (smell, §8).
2. **ExtProc filter** (`inject_ext_proc` / `create_ext_proc_filter`): per active session an
   `envoy.filters.http.ext_proc.session_{session_id}` HTTP filter is inserted **before the
   router** in every HCM:
   - gRPC target `flowplane_ext_proc_service` cluster (same CP gRPC endpoint), timeout 5 s,
     `message_timeout_ms = 5000`.
   - `failure_mode_allow = true` and `is_optional = true` → **fail-open**: requests continue if
     the CP is down.
   - ProcessingMode: request/response headers SEND, request/response body **BUFFERED**, trailers
     SKIP.

Both injections are transient — stored listener configuration is never modified; injection
happens at xDS build time only while sessions are active.

### 3.2 ALS — metadata + headers (`src/xds/services/access_log_service.rs`)

Client-streaming gRPC (`StreamAccessLogs`). For each `HTTPAccessLogEntry`:

- Path = `x-envoy-original-path` header if present (pre-rewrite), else `request.path`. Method
  decoded from Envoy's `RequestMethod` enum (0=UNKNOWN, 1=GET … 9=PATCH).
- The entry is matched against the in-memory session list: **first session whose any
  route-pattern regex matches the path AND whose method filter (if any) matches** wins —
  registration-order priority, no specificity ranking. `(session_id, team)` returned atomically
  under one lock (race fix).
- Matched entries become `ProcessedLogEntry`:
  `{session_id, request_id (x-request-id), team, method(i32), path, request_headers,
  request_body: None, request_body_size, response_status, response_headers,
  response_body: None, response_body_size, start_time_seconds,
  duration_ms (time_to_last_downstream_tx_byte), trace_context}`.
  **ALS never carries body content** (Envoy's HTTP log protos don't include it); only sizes.
- Header hygiene (applied to both request and response headers, `filter_and_redact_headers`):
  - Drop infrastructure headers by prefix (`x-envoy-`, `x-forwarded-`, `x-b3-`, `x-trace-`,
    `x-amzn-`, `x-request-id`) and by exact name (server, date, connection, transfer-encoding,
    via, keep-alive, traceparent, tracestate, content-length).
  - **Cap at 20 headers** per direction.
  - Redact sensitive values (authorization, proxy-authorization, cookie, set-cookie, x-api-key,
    x-auth-token, x-csrf-token, x-session-id) → `"***"`, preserving the auth scheme for
    Authorization (`"Bearer ***"`, `"Basic ***"`) so export can derive securitySchemes.
- W3C `traceparent`/`tracestate` parsed into `TraceContext` (validated 32/16/2 hex fields) for
  trace correlation; not persisted into schemas.
- On successful queue (unbounded mpsc → worker pool), `current_sample_count` is incremented in
  the DB **per entry**. Unmatched entries are dropped (logged at info).

### 3.3 ExtProc — bodies (`src/xds/services/ext_proc_service.rs`)

Bidirectional ExtProc stream. Per request:

- On RequestHeaders: extract `:path` and `x-request-id`; match path against the per-session
  regex map (first match wins; **no method filter and no team check here**).
- Bodies accumulated chunk-by-chunk; at end-of-stream, truncated to **MAX_BODY_SIZE = 10 KB**
  (`request_truncated`/`response_truncated` flags). At response end-of-stream, a
  `CapturedBody {session_id, request_id, request_body?, response_body?, truncated flags}` is
  sent on an unbounded channel. Requires BOTH session match and `x-request-id`; otherwise bodies
  are discarded.
- All ExtProc responses are CONTINUE (no mutation, fail-open).
- **Smell:** the buffered bodies accumulate without limit until end_of_stream, then truncate —
  a streamed multi-MB body is held in CP memory before truncation (and in Envoy's buffer,
  bounded by the listener's buffer limits).

### 3.4 Raw Observation Size Contract (v2)

`ObservationIngest` separates stored payloads from quota accounting:

- `request_body` and `response_body` are optional captured payload excerpts. They may be
  truncated before ingest and remain subject to the storage validation cap.
- `request_body_truncated` and `response_body_truncated` only describe whether the stored
  payload is incomplete; they do not change byte accounting by themselves.
- `request_body_bytes` and `response_body_bytes`, when present, are the original L7 body sizes
  reported by the dataplane before Flowplane truncation. They must be non-negative and cannot be
  smaller than the stored payload excerpt.
- Capture-session `max_bytes` accounting uses the larger of the stored payload length, the
  existing persisted byte count for the request id, and the newly reported original byte count.
  This makes ALS-before-ExtProc and ExtProc-before-ALS arrival order safe, and prevents truncated
  payloads from under-counting real traffic volume.
- If no original size is reported, v2 preserves the compatibility fallback: stored string length
  is used for byte accounting.

---

## 4. Worker pipeline (`src/services/access_log_processor.rs`)

### 4.1 Configuration (`ProcessorConfig` defaults)

| Knob | Default | Meaning |
|---|---|---|
| `worker_count` | `num_cpus` (≥1) | Tokio tasks consuming both channels |
| `batch_size` | 100 | Inferred-schema rows per DB batch insert |
| `batch_flush_interval_secs` | 5 | Periodic flush of partial batches |
| `max_retries` | 3 | Batch-write retries |
| `initial_backoff_ms` | 100 | Exponential backoff (100→200→400 ms) |
| `max_queue_capacity` | 10,000 | Bounded schema channel → backpressure |
| `path_normalization` | `rest_defaults()` | §4.4 |
| `pending_entry_ttl_secs` | 15 | TTL for un-merged ALS/body entries |
| `pending_cleanup_interval_secs` | 5 | Cleanup tick |

### 4.2 Stages

1. **Merge** (per worker, `tokio::select!` over ALS channel + ExtProc channel):
   - Merge key = `"{session_id}:{request_id}"`. ALS entry arriving first is parked in
     `pending_logs`; body arriving first parked in `pending_bodies`; whichever arrives second
     completes the pair, copying non-empty bodies into the `ProcessedLogEntry`.
   - Missing `x-request-id` → entry processed immediately without bodies (warn + metric
     `record_missing_request_id`).
   - Duplicate request_id: old pending log is processed body-less before the new one is parked;
     duplicate pending body is replaced by the newer one.
   - **Cleanup task** (every 5 s): pending logs older than 15 s are **processed without bodies**
     (not dropped — bodyless endpoints must still reach the catalog); orphaned pending bodies are
     dropped. Graceful shutdown drains both channels, best-effort merging.
2. **Inference** (`process_entry`): for each of request/response body, if valid UTF-8 and valid
   JSON, run `SchemaInferenceEngine::infer_from_json` (§4.3) and serialize via
   `to_json_schema()`; non-JSON/binary/malformed bodies are skipped silently (debug log +
   metric). **A record is emitted for every entry even with no schemas** so bodyless endpoints
   (GET collections, DELETE 204) appear in the catalog.
3. **Normalization**: query string stripped, then `normalize_path` (§4.4) →
   `path_pattern`.
4. **Record assembly**: `InferredSchemaRecord {session_id, team, http_method (string),
   path_pattern, request_schema?, response_schema?, response_status_code,
   request_headers?, response_headers?}` — headers serialized as JSON array of
   `{"name", "example"}` objects.
5. **Batching**: `try_send` on bounded channel (cap 10,000). Full → **drop schema** (metric
   `record_schema_dropped`, warn) — load-shedding, not blocking. A dedicated batcher task
   accumulates to `batch_size` or flushes every 5 s; writes all rows in one transaction
   (`INSERT INTO inferred_schemas (...) VALUES (..., sample_count=1, confidence=1.0)`); on
   failure retries with exponential backoff up to `max_retries`, then drops the batch
   (logged + metric). Final flush on shutdown.

Metrics emitted throughout: `record_access_log_message`, `record_access_log_latency`,
`update_active_learning_sessions`, `update_processor_workers`,
`record_processor_entry_duration`, `record_schema_inferred(kind, ok)`,
`record_schema_dropped`, `record_schema_batch_write(size, ok, attempts)`,
`record_missing_request_id`.

### 4.3 Schema inference (`src/schema/inference.rs`)

Privacy-by-design: payload values are parsed, metadata extracted, value dropped (except enum
tracking below).

- **Types**: `string | number | integer | boolean | null | object | array | OneOf(Vec<Type>)`.
  Integer when `is_i64/is_u64`, else number. Type merge: equal→same; different→`OneOf` of the
  deduplicated union (sorted by `Debug` string for determinism).
- **String formats** (regex/heuristic, checked in order): email (`a@b.c` shape), UUID
  (36 chars, 8-4-4-4-12 hex), URI (`http://`/`https://` prefix), date-time (ISO 8601 with `T`
  and zone hint), date (`YYYY-MM-DD`), IPv4. No format → format omitted entirely (never
  `"none"`).
- **Enum tracking**: only for *unformatted* strings of length ≤ 100
  (`MAX_STRING_LENGTH_FOR_TRACKING`); the raw value is stored in transient `observed_values`
  (deduplicated, capped at 100 values per field during merges). `observed_values` is **stripped
  from every exported JSON schema** (`strip_observed_values`) — raw payload data never leaves
  the aggregation layer.
- **Numbers**: NO min/max constraints recorded from single observations (deliberate — avoids
  overfitting; constraints would be aggregation-time only and v1 never computes them).
- **Arrays**: `array_constraints {min_items = max_items = observed length}`; item schemas of
  all elements merged into one `items` schema.
- **Objects**: per-property recursive inference. Optional field-name anonymization
  (None/Hash(SHA-256→8 hex)/Sequential `field_N`) with reversible mapping — engine supports it
  but the pipeline uses `AnonymizationMode::None`.
- **Stats**: every schema node carries `{sample_count, presence_count, confidence =
  presence/sample}` (serde-flattened into the JSON schema as custom extension fields).
- **Merge** (`InferredSchema::merge`): type merge as above; constraints take min-of-mins /
  max-of-maxes; object properties merged key-wise (new keys inserted); array items merged;
  observed/enum values unioned; stats summed.
- Output is JSON Schema Draft 2020-12 (`$schema` injected) with Flowplane extension fields.

### 4.4 Path normalization (`src/services/path_normalizer.rs`)

Turns `/users/123` into `/users/{userId}` **before storage**, so all observations of one
endpoint group together.

- Config (`PathNormalizationConfig`): `enabled` (true), `min_param_length` (1),
  `max_param_length` (100), `literal_keywords`, `enable_plural_conversion`. Pipeline uses
  `rest_defaults()`: 47 protected keywords (api, v1..v5, admin, public, private, internal,
  health, status, metrics, docs, swagger, openapi, graphql, rest, rpc, ws, wss, auth, login,
  logout, register, callback, webhook(s), search, upload, download, export, import, batch, bulk,
  stream, feed(s), config, settings, preferences, notifications, events, actions, jobs, tasks,
  queue) + plural conversion ON. A `graphql_defaults()` preset exists (unused by the pipeline).
- Per segment, in order:
  1. empty / already `{param}` → pass through;
  2. literal if: keyword match (case-insensitive), version-like (`v` + digits/dots, len 2–5), or
     pure alphabetic;
  3. parameter detection by specificity: UUID → DateTime (`YYYY-MM-DDTHH:MM…`) → Date
     (`YYYY-MM-DD`) → AlphanumericCode (letters+digits mixed, ≥2 chars) → NumericId (`\d+`,
     any length, covers unix timestamps) → HyphenatedId (contains `-`/`_` and a digit).
- Parameter naming: previous *literal* segment (scanning backwards) singularized (141-entry
  lookup table + fallbacks `-ies`→`-y`, `-ses`→`-s` (not `-sses`), `-s`→strip (not `-ss`)) +
  type suffix `Id|Code|Date|Timestamp` → `{userId}`, `{orderDate}`, `{productCode}`. No
  preceding literal → generic `{id}|{code}|{date}|{timestamp}`. Two consecutive dynamic
  segments: second gets the generic placeholder.

---

## 5. Aggregation (`src/services/schema_aggregator.rs`)

Triggered on session completion or snapshot. Atomic: all endpoint aggregations for a session are
prepared read-only, then batch-inserted in one transaction (`create_batch`) — all or nothing.

1. **Grouping**: `inferred_schemas` rows for the session grouped by
   `(http_method, path_pattern, response_status_code)`.
2. **Schema merge** per group (`merge_schemas`): parse each stored JSON schema back into
   `InferredSchema`, fold with `merge()`; then:
   - `fix_field_stats_with_observations`: recount per-field `presence_count` against the actual
     observations recursively (nested objects counted against parent-presence, not global
     total).
   - **Required fields**: presence ratio ≥ **1.0 (100%)** → required; computed recursively;
     sorted for determinism. **PATCH requests get `required` cleared entirely** (partial
     updates).
   - **Enum promotion** (`promote_enums`): a string field becomes an enum iff
     `sample_count ≥ MIN_SAMPLES_FOR_ENUM (10)` AND distinct observed values
     ≤ `MAX_ENUM_CARDINALITY (10)`; promoted values sorted into `enum_values`;
     `observed_values` always cleared afterwards.
3. **Response map**: `{ "<status>": schema|null }` — null preserved for bodyless statuses.
4. **Headers**: union of `{name, example}` entries across observations, deduplicated by
   lowercase name (first example wins), sorted by name.
5. **Confidence score** (`calculate_confidence_score`):
   `confidence = 0.4·sample + 0.4·field_consistency + 0.2·type_stability`, clamped [0,1]:
   - sample = `ln(n)/ln(100)` clamped (1→0.0, 10→0.5, 100→1.0);
   - field_consistency = required_fields / total_fields, recursive, 1.0 when no fields;
   - type_stability = fields without `oneof` type / total fields, recursive, 1.0 when no fields.
6. **Breaking-change detection** vs `get_latest(team, path, method)` previous version
   (`src/services/schema_diff.rs`): change types `required_field_removed`,
   `incompatible_type_change`, `required_field_added`, `field_became_required`,
   `schema_type_changed`; each `{type, path ("$.user.email" prefixed with "request"/
   "response[STATUS]"), description, old_value?, new_value?}`. Stored as JSON array;
   `previous_version_id` links versions.
7. Insert into `aggregated_api_schemas` with `version = previous+1` (UNIQUE on
   team+path+method+version), `first_observed`/`last_observed` from observation timestamps,
   and optional `(session_id, snapshot_number)` tags.

Note: aggregation is append-only across sessions — re-learning the same endpoint produces a new
version; nothing prunes `inferred_schemas` rows (no TTL/cleanup; growth risk, §8).

---

## 6. Data model

Migrations: `migrations/20251018000001_create_learning_sessions_table.sql`,
`20251019000001_create_inferred_schemas_table.sql`,
`20251019000002_create_aggregated_api_schemas_table.sql`,
`20260406000001_auto_aggregate_support.sql`, `20260406000002_add_learning_session_name.sql`,
`20260225000002_add_header_columns_to_schema_tables.sql`,
`20260109000001_create_route_metadata_table.sql`, `20260109000004_create_mcp_tools_table.sql`,
`20260217000001_fix_route_metadata_learning_schema_id_type.sql`.

### `learning_sessions`
```
id TEXT PK (uuid) · team TEXT (team id) · route_pattern TEXT (regex) ·
cluster_name TEXT? · http_methods TEXT? (JSON array) ·
status TEXT default 'pending' · created_at/started_at/ends_at/completed_at TIMESTAMPTZ ·
target_sample_count BIGINT · current_sample_count BIGINT default 0 ·
triggered_by TEXT? · deployment_version TEXT? · configuration_snapshot TEXT? (JSON) ·
error_message TEXT? · updated_at ·
auto_aggregate BOOLEAN default FALSE · snapshot_count BIGINT default 0 ·
name TEXT? (UNIQUE (team,name) WHERE name IS NOT NULL)
```
Indexes on team, status, (team,status), route_pattern, created_at, (team,status,created_at DESC).
Note: `cluster_name` is stored and documented as a filter, but capture matching uses only
route_pattern + http_methods (cluster filter not enforced in ALS/ExtProc — smell, §8).

### `inferred_schemas` (one row per captured request)
```
id BIGSERIAL PK · team · session_id FK→learning_sessions ON DELETE CASCADE ·
http_method · path_pattern (normalized) ·
request_schema TEXT? (JSON Schema 2020-12 + extensions) · response_schema TEXT? ·
response_status_code BIGINT? ·
request_headers TEXT? / response_headers TEXT? (JSON: [{"name","example"}]) ·
sample_count BIGINT default 1 · confidence DOUBLE default 1.0 ·
first_seen_at · last_seen_at · created_at · updated_at
```

### `aggregated_api_schemas` (consensus per endpoint, versioned)
```
id BIGSERIAL PK · team · path · http_method ·
version BIGINT default 1 · previous_version_id BIGINT? FK self ·
request_schema TEXT? · response_schemas TEXT? ({"200": {...}, "404": null, ...}) ·
request_headers TEXT? / response_headers TEXT? (merged [{"name","example"}]) ·
sample_count · confidence_score DOUBLE · breaking_changes TEXT? (JSON array §5.6) ·
first_observed · last_observed · created_at · updated_at ·
session_id TEXT? FK→learning_sessions · snapshot_number BIGINT? ·
UNIQUE(team, path, http_method, version)
```

### `route_metadata` (bridge: routes ↔ schemas, feeds tool generation)
```
id TEXT PK · route_id FK→routes ON DELETE CASCADE, UNIQUE ·
operation_id? · summary? · description? · tags? · http_method? ·
request_body_schema TEXT? · response_schemas TEXT? ·
learning_schema_id BIGINT? FK→aggregated_api_schemas ON DELETE SET NULL ·
enriched_from_learning BOOLEAN default FALSE ·
source_type TEXT CHECK IN ('openapi','manual','learned') · confidence DOUBLE?
```

### `mcp_tools` (dynamic api_* tools)
```
id TEXT PK · team FK→teams · name (UNIQUE(team,name), e.g. "api_getUser") · description? ·
category CHECK ('control_plane','gateway_api') ·
source_type CHECK ('builtin','openapi','learned','manual') ·
input_schema TEXT (JSON Schema) · output_schema TEXT? ·
learned_schema_id BIGINT? FK→aggregated_api_schemas ON DELETE SET NULL ·
schema_source CHECK ('openapi','learned','manual','mixed')? ·
route_id? FK→routes ON DELETE CASCADE · http_method? · http_path? · cluster_name? ·
listener_port BIGINT? · host_header? · enabled BOOLEAN default TRUE · confidence DOUBLE?
```

### In-memory shapes (channel payloads)
- `ProcessedLogEntry` (§3.2), `CapturedBody` (§3.3), `InferredSchemaRecord` (§4.2 step 4).
- `InferredSchema` JSON: `{type, format?, numeric_constraints?, array_constraints?, items?,
  properties?, required?, field_mapping?, enum_values?, sample_count, presence_count,
  confidence}` (+transient `observed_values`, stripped on export; `oneof` encoded inside `type`).

---

## 7. User-facing operations

### 7.1 REST (team-scoped, `require_resource_access_resolved` with resource
`learning-sessions` / `aggregated-schemas`)

`src/api/handlers/learning_sessions.rs`:

| Op | Endpoint | Notes |
|---|---|---|
| Create | `POST /api/v1/teams/{team}/learning-sessions` (scope `learning-sessions:create`) | Body (camelCase): `routePattern` (regex, 1–500 chars, validated by compiling), `clusterName?`, `httpMethods?` (validated against 9 verbs), `targetSampleCount` (1–100,000), `maxDurationSeconds?`, `triggeredBy?`, `deploymentVersion?`, `configurationSnapshot?`, `autoAggregate?` (default false), `name?` (1–64; auto-generated otherwise; 409 on duplicate). **Auto-activates**; returns 201 with session (incl. `progressPercentage`). |
| List | `GET .../learning-sessions?status=&limit=&offset=` (`:read`) | |
| Get | `GET .../learning-sessions/{id}` (`:read`) | `{id}` accepts name or UUID; cross-team → 404 (no info leak). |
| Cancel | `DELETE .../learning-sessions/{id}` (`:delete`) | Only pending/active/completing; 400 on terminal states; 204. |
| Stop | `POST .../learning-sessions/{id}/stop` (`:execute`) | Active→completed via final aggregation (the way to end auto-aggregate sessions). |
| Activate | `POST .../learning-sessions/{id}/activate` (`:write`) | pending→active; completed sessions returned as-is (idempotent confirm). |

`src/api/handlers/aggregated_schemas.rs`:

| Op | Endpoint | Notes |
|---|---|---|
| List schemas | `GET .../aggregated-schemas` | Filters incl. min-confidence/session. |
| Get schema | `GET .../aggregated-schemas/{id}` | |
| Compare | `GET .../aggregated-schemas/{id}/compare?with=` | Field-level diff (schema_diff). |
| Export one | `GET .../aggregated-schemas/{id}/export?includeMetadata=true` | OpenAPI 3.1 doc for one endpoint (§7.4). Team filter applied in the query itself. |
| Export many | `POST .../aggregated-schemas/export` body `{schemaIds, title="Learned API", version="1.0.0", description?, includeMetadata=true}` | Unified spec (§7.4). |

MCP route-tool plumbing (`src/api/handlers/mcp_routes/mod.rs`):
`GET .../routes/{route_id}/mcp/status`, `POST .../mcp/enable`, `POST .../mcp/disable`,
`POST .../mcp/refresh` (re-pull learned schema if confidence ≥ 0.8),
`POST .../teams/{team}/mcp/bulk-enable`, `POST .../mcp/bulk-disable`.

OpenAPI import (`src/api/handlers/openapi_import.rs`): `POST` import, `GET` list/get imports,
`DELETE` import (removes created resources, triggers xDS refresh).

### 7.2 MCP control-plane tools (`src/mcp/tools/{learning,schemas,openapi}.rs`)

`cp_list_learning_sessions`, `cp_get_learning_session`, `cp_create_learning_session`
(args camelCase; `autoStart`, `autoAggregate`, returns `status` + `next_step` hint),
`cp_activate_learning_session`, `cp_stop_learning`, `cp_delete_learning_session`,
`ops_learning_session_health`; `cp_list_aggregated_schemas`, `cp_get_aggregated_schema`,
`cp_export_schema_openapi`; `cp_list_openapi_imports`, `cp_get_openapi_import`.
All route through the shared internal API layer (`src/internal_api/{learning,schemas,openapi}.rs`
— `list/get/resolve_session/create/stop/delete`, `list/get/get_version_history`, `list/get`)
which is the same layer the CLI/BFF uses.

### 7.3 CLI (docs: `docs/tutorials/quickstart-learning.md`,
`docs/how-to/learn-and-export-openapi.md`)

`flowplane learn start|get|list|stop|cancel`, `flowplane schema list|get|compare|export`
(`--all | --session NAME | --id 1,2,3`, `--min-confidence`, `--title/--version/--description`,
`-o file.yaml|json`). The CLI shells the REST endpoints above. The documented loop is:
expose backend → `learn start` → send traffic → auto-complete → `schema list` →
`schema export`. CI patterns: smoke-test capture + export-as-artifact + `oasdiff`.

### 7.4 OpenAPI 3.1 export semantics (`aggregated_schemas.rs::build_openapi_spec` /
`build_unified_openapi_spec`, `src/api/handlers/openapi_utils.rs`,
`src/openapi/domain_models.rs`)

- `openapi: "3.1.0"`; info from options (defaults `Learned API`/`1.0.0`) or per-schema title.
- Path/query handling: stored path parsed into base path + query params; query params become
  optional `in: query` parameters with type inferred from example value (integer/number/
  boolean/string) and example attached; `{param}` segments become required `in: path` string
  parameters; operationId + semantic summary generated from method+path.
- Multi-export groups records by (base_path, lowercased method) so all status codes across rows
  merge into one operation; first record supplies request schema/headers.
- Internal attributes stripped (`sample_count`, `presence_count`, `confidence`,
  `observed_values`, `field_mapping`...); internal `type:{oneof:[...]}` converted to standard
  `oneOf`; `array_constraints`/`numeric_constraints` → minItems/maxItems/uniqueItems/
  minimum/maximum/multipleOf; `includeMetadata=true` re-adds `x-flowplane-sample-count`,
  `x-flowplane-confidence`, `x-flowplane-first/last-observed` extensions.
- Bodyless statuses (`null` schema) emit a description-only response ("No Content" for 204);
  empty responses object gets a `default` placeholder.
- Observed request headers become optional `in: header` parameters EXCEPT authorization/x-api-key.
- **Security scheme detection** from redacted examples: `Bearer ***` → `bearerAuth (http/bearer)`,
  `Basic ***` → `basicAuth`, `x-api-key` → `apiKeyAuth (apiKey/header)`; added to
  `components.securitySchemes` + global `security`.
- **Domain-model deduplication** (multi-export only): every object schema (≥2 properties)
  appearing at ≥2 distinct endpoints — matched by a structural fingerprint over property
  names/types/nesting (ignoring stats/format/required/enum) — is hoisted to
  `components/schemas/<Name>` (name derived from path resource, singularized) and replaced by
  `$ref` at all occurrences.

### 7.5 Spec → `api_*` MCP tools (`src/services/mcp_service.rs`, `src/mcp/gateway/`)

- **Trigger**: explicit `mcp/enable` (or bulk-enable) on a route. Enrichment priority:
  (1) existing `route_metadata` (e.g. from OpenAPI import; missing fields auto-filled),
  (2) **learned**: `aggregated_api_schemas.get_latest(team, route.path_pattern, method)` if
  `confidence_score ≥ 0.8` → metadata gets request/response schemas,
  `learning_schema_id`, `enriched_from_learning=true`, `source_type='learned'`,
  (3) request-provided fields, (4) auto-generated fallback (`{method}_{path_parts}`).
  **Matching is by exact string equality** of the route's `path_pattern` against the learned
  `path` — the learned template (`/users/{userId}`) must textually match the route pattern,
  a major drift point (§10).
- **Generation** (`GatewayToolGenerator`): name `api_{operationId}` or
  `api_{path}_{method}_{routeId8}`; input schema = path params (string, required) merged with
  request-body properties/required; output schema = response_schemas; carries
  `http_method/http_path/listener_port/host_header(first non-wildcard vhost domain)/confidence`.
- **Refresh**: `mcp/refresh` re-queries the latest aggregated schema (≥0.8) and regenerates the
  stored tool; returns failure messages "confidence too low (<80%)" / "no learned schema".
  Nothing auto-refreshes when new aggregations land.
- **Serving**: MCP `tools/list` appends DB-backed `gateway_api` tools per team (requires
  `api:read`; agent grants can scope to specific route_ids); any `tools/call` with `api_*`
  prefix routes to `GatewayExecutor`, which substitutes `{param}` from arguments, sends the
  remaining arguments as JSON body, targets `http://{gateway_host|127.0.0.1}:{listener_port}`
  with the stored Host header — i.e., execution goes **through the Envoy dataplane**, not
  directly upstream.

---

## 8. OpenAPI import path (reverse direction) — v1 fact

`src/openapi/mod.rs::build_gateway_plan` + `src/api/handlers/openapi_import.rs`:

- Input: OpenAPI doc + `GatewayOptions {name, protocol, listener_mode (Existing{name} |
  New{name,address,port}), dataplane_id (required for New), team}`.
- From `servers[0]`: one upstream cluster; per path×method (GET/POST/PUT/DELETE/PATCH/HEAD/
  OPTIONS/TRACE): a `RouteRule` with `PathMatch::Template` (OpenAPI `{param}` templates map
  directly) + `:method` header matcher, action → the cluster. Existing-listener mode merges a
  virtual host into the listener's route config; New mode creates route config + listener
  (HCM + optional `x-flowplane-filters` global filters extension).
- Per-operation metadata (operationId, summary, description, tags, request-body schema with
  `$ref` resolution, response schemas keyed by status) is extracted and persisted as
  `route_metadata` rows (`source_type='openapi'`, `confidence=1.0`,
  `learning_schema_id=NULL`) keyed by generated route names.
- Import is tracked in `import_metadata` (+ `cluster_references`); delete-import removes the
  created resources. xDS refresh for clusters/listeners/routes is triggered automatically.
- **MCP tools are NOT created by import** — a separate `mcp/enable` (or bulk-enable) call is
  required; it will then find the imported `route_metadata` complete and generate tools with
  `source_type='openapi'`.

So v1's loop closure is: learn → export OpenAPI → (manually) import that OpenAPI →
routes/clusters/metadata → (manually) enable MCP → tools. Each arrow is a separate operator
action with no shared identity between the learned schema and the imported spec.

---

## 9. Traffic-first gap analysis

### 9.1 What v1 can and cannot do (fact)

- **Capture requires an existing listener.** ALS/ExtProc configs are injected into HCMs of
  listeners already in the DB. No listener → Envoy never accepts the connection → nothing to
  learn. There is no learning-specific catch-all listener.
- **Capture does NOT require a matching route.** The injected access log has `filter: None` and
  ALS receives every request traversing the HCM — including requests that hit no configured
  route (Envoy 404s) or the default gateway's black-hole route
  (`ensure_default_gateway_resources` creates `default-gateway-listener` :10000 with a
  catch-all `/` route to `default-gateway-cluster` → 127.0.0.1:65535, `src/openapi/defaults.rs`).
  A session with pattern `^/` on such a listener will record those requests — but with
  response_status 404/503 and no useful response schema. So v1 can *observe* unmatched-route
  traffic shapes (method, path, request headers/body) but learns garbage responses.
- **Session matching is path-regex only** — it is agnostic to whether a route matched.
- **v1 cannot generate routes or clusters from a learned spec.** The only path is the manual
  export→import round-trip (§8), which also requires the operator to supply the real upstream
  (`servers[0]` of the exported doc is absent — exports contain no `servers` block, so a
  straight re-import actually fails `MissingServers` until the operator edits the doc; v1's
  export and import formats are not round-trip compatible without manual editing).
- **Upstream identity is never learned.** Capture happens at the listener; the original
  destination of unrouted traffic (Host header aside) is unknown to the pipeline. Host header
  is not part of the inferred schema key (paths only), so two vhosts with identical paths merge.

### 9.2 [V2 DESIGN] Traffic-first onboarding (proposal)

Goal: point traffic at the gateway *before* any routes exist; let the system propose the
gateway config.

1. **Catch-all discovery listener (designed addition).** A per-team (or per-dataplane)
   "discovery" listener with: a wildcard virtual host, a terminal route returning 404/`x-fp:
   unrouted` (or optionally proxying to a declared default upstream), the learning access log +
   ExtProc filter installed *permanently* while discovery mode is on, and ALS entries tagged
   `route_matched: false` (Envoy exposes route name in access-log common properties — capture
   it; v1 ignores it). Discovery sessions match on `(listener, host, path)` instead of path
   regex alone, and the inferred-schema key must gain `host` (v1 keys only method+path —
   collision risk noted above).
2. **Learned spec → concrete resources (designed addition).** A new operation
   `POST /teams/{team}/learned-specs/{exportId}/materialize` (and MCP `cp_materialize_spec`):
   - Input: set of aggregated schema IDs (or session/snapshot), target upstream
     (host:port/TLS — mandatory operator input OR inferred from discovery-mode Host headers and
     presented as a suggestion), listener mode (existing/new, as in v1 import).
   - Output: a **plan** (dry-run by default) reusing the v1 import planner: one cluster per
     upstream, one route per (method, normalized path-template) with `PathMatch::Template`
     mapped 1:1 from the learned template, `route_metadata` rows with `source_type='learned'`
     and `learning_schema_id` set (preserving identity — unlike the v1 export/import round-trip
     which loses it), and optionally MCP tools.
   - Internally this is exactly v1's `GatewayPlan`, fed from `aggregated_api_schemas` instead of
     a parsed OpenAPI doc — eliminating the lossy export→import hop.
3. **Approval gate (designed addition).** Materialization is never automatic: plan is persisted
   (`pending_plans` table) with diff vs current resources; operator (or an authorized agent with
   a new `gateway-plans:approve` scope) approves/rejects; approval applies the plan in one
   transaction + xDS refresh + audit-log entry (risk-level high). Auto-approve may be allowed
   only for additive changes below a confidence threshold knob (default: require ≥0.8 confidence
   per endpoint, the same bar v1 uses for tool enrichment).
4. **Continuous mode**: with auto_aggregate sessions on the discovery listener, each snapshot
   can regenerate the plan diff ("3 new endpoints observed since last approval") — surfaced via
   webhook/MCP, never applied without approval.

Mark for spec/08: this design removes three of v1's four manual steps (route creation, export,
import) and leaves two human actions: start discovery, approve plan.

---

## 10. Security-relevant behavior (feeds 08a)

**Team scoping — where it holds:**
- Sessions, inferred and aggregated schemas all carry `team`; REST/MCP handlers resolve team
  from path/auth and verify membership (`verify_team_access` returns 404 cross-team; export
  query filters by team in SQL). ALS attributes each entry to the matched session's team
  atomically.

**Where it leaks or is weak (v1 facts):**
- **Cross-team capture**: ALS/ExtProc injection is applied to **all listeners**, and session
  matching is **path-regex only with first-match-wins**. Team A's session `^/api/.*` will
  capture team B's traffic on a shared (or any) listener whose paths match — headers and bodies
  included — and store them under team A. There is no listener/team affinity check at capture
  time. (Highest-severity finding of this subsystem.)
- **ExtProc has no method filter and no team check at all** (path regex only), so bodies can be
  captured for methods the session excluded; they are only dropped later if the ALS entry never
  matches.
- `cluster_name` filter is accepted at creation but not enforced during capture.

**Hostile-traffic / poisoning surface:**
- **Header injection into schemas**: header *names* observed on the wire become OpenAPI header
  parameters and merged header lists verbatim. An attacker sending requests with absurd or
  misleading custom headers (e.g., `x-admin-override`) gets them embedded in the exported spec
  and in tool generation context. Mitigations present: infrastructure-prefix/exact filtering,
  20-header cap per direction, sensitive-value redaction. No allowlist, no name-length cap, no
  frequency threshold (a single request's headers are exported).
- **Schema poisoning**: any client who can reach the listener during a session contributes
  observations with equal weight. One hostile request can add fields/types (degrading
  `required` analysis to drop legitimate required fields — 100% presence rule means one request
  *without* a field demotes it), inject `enum` values (cap: ≤100 chars per value, ≤100 tracked,
  promotion needs ≥10 samples and ≤10 distinct — so flooding >10 distinct values *suppresses*
  enum detection rather than injecting), and add bogus status codes. Confidence drops but
  nothing quarantines outliers.
- **Body limits**: 10 KB hard truncation in ExtProc (truncated JSON then fails to parse →
  inference skipped → no schema, only catalog entry). No per-session or global byte budget;
  unbounded ALS→worker channel means a flood of matched requests grows memory until workers
  catch up (the bounded 10k schema channel only protects the DB stage).
- **Path explosion**: normalization collapses IDs, but pure-alphabetic segments are always kept
  literal — `GET /api/<random-word>` floods `inferred_schemas`/`aggregated_api_schemas` with
  one endpoint per word. No cap on distinct path patterns per session, no cardinality alarm.
  `target_sample_count` (≤100,000) bounds total rows per non-auto-aggregate session; an
  auto-aggregate session is unbounded over time.
- **Sensitive data**: payload *values* are never stored except (a) unformatted strings ≤100
  chars in `observed_values` (transient, stripped from all exports, cleared at aggregation —
  but **persisted at rest inside `inferred_schemas.request_schema` JSON** until aggregation, and
  the rows are never deleted; raw enum candidates therefore live in the DB indefinitely),
  (b) one example value per header (redacted for the 8 sensitive names; other headers' values
  stored verbatim — e.g. a bespoke `x-tenant-secret` header value would persist and export as a
  parameter example... actually header examples are not exported as examples, only names; they
  ARE stored in DB and returned by schema get APIs).
- **Regex DoS**: route patterns are operator-supplied and compiled with the Rust `regex` crate
  (no catastrophic backtracking by construction); length capped at 500. Patterns are evaluated
  per request per session — many active sessions × broad patterns is a linear CPU cost on the
  ALS path.
- **Fail-open ExtProc** is availability-friendly but means body capture silently degrades; the
  15 s pending TTL converts that to schema-less catalog entries (visible only in logs/metrics).
- **Transport**: ALS/ExtProc clusters carry mTLS only when dataplane TLS is configured;
  startup warns otherwise (`src/xds/mod.rs:142-150`) — plaintext capture traffic (full bodies)
  on the wire in non-TLS deployments.

**[V2 DESIGN] required hardening**: enforce listener/team affinity at capture (session may only
match listeners owned by its team); key schemas by (team, listener/host, method, path);
per-session caps on distinct path patterns + alert; header-name allowlist or min-frequency
threshold before export; encrypt or TTL-expire `inferred_schemas` rows post-aggregation;
make ExtProc honor method filters.

---

## 11. Gaps and smells (feeds spec/08)

1. **Listener-wide capture blast radius**: activating any session re-pushes *every* listener
   with an access log + ExtProc filter on *every* HCM; all traffic on all listeners is shipped
   to the CP for the session's lifetime, filtered CP-side. Cost scales with total gateway
   traffic, not session scope. v2 should scope injection to listeners/routes the pattern can
   match, and use Envoy ALS filters.
2. **Ineffective duplicate check** in `inject_access_logs` (predicate tests `AccessLog.name`,
   which never contains the session id) — works only because listeners are rebuilt from clean
   config each refresh.
3. **ExtProc restart gap**: startup recovery re-registers sessions with ALS but not ExtProc
   (§2.3) — silent body loss.
4. **`cluster_name` is decorative** (stored, never enforced).
5. **Exact-string route↔schema matching** for enrichment: learned `path`
   (`/anything/customers/{customerId}`) must equal the route's `path_pattern` byte-for-byte.
   Parameter-name differences (`{id}` vs `{customerId}`) or prefix routes silently yield "no
   learned schema". This is the main drift point between routes, schemas, and tools.
6. **No tool/spec freshness**: new aggregations don't update existing `mcp_tools` or
   `route_metadata`; `mcp/refresh` is per-route and manual. Tools can serve stale schemas
   forever; `learned_schema_id` FK is SET NULL on schema delete but schemas are never deleted.
7. **Export/import not round-trippable**: exports lack `servers`, import requires it; identity
   (`learning_schema_id`) is lost crossing the boundary. The "closed loop" is three manual hops
   with re-keying at each.
8. **No retention**: `inferred_schemas` (per-request rows incl. raw enum candidates) and
   `aggregated_api_schemas` versions grow unbounded; docs recommend `flowplane down --volumes`
   to clear (!). No per-schema delete endpoint.
9. **First-match-wins session selection** in ALS (registration order) and HashMap-iteration
   order in ExtProc — overlapping sessions get nondeterministic attribution between the two
   services (ALS may pick session A, ExtProc session B → merge key never matches → bodies lost).
10. **Drop-on-backpressure** (schema channel full, batch retries exhausted) loses samples with
    only metrics as evidence; `current_sample_count` is incremented at ALS-accept time, so the
    session can "complete" having persisted fewer observations than the counter says.
11. **`required` = 100% presence** is brittle to a single hostile/partial request (also §10);
    confidence formula penalizes optional fields (field_consistency = required/total), so
    legitimately optional-rich APIs cap out at low confidence and never reach the 0.8 tool
    enrichment bar.
12. **Per-entry DB write** for sample counting (one UPDATE per matched request on the ALS hot
    path) — a throughput ceiling; v2 should batch.
13. **Glue duplication**: two singularize implementations (`path_normalizer.rs` and
    `openapi_utils.rs`), two header-name knowledge bases (ALS capture list vs export
    securityScheme detection), and three layers re-implementing session DTO mapping
    (REST handler, internal_api, MCP tools).
14. **Worker pool contention**: all workers share single-`Mutex` receivers and pending maps —
    effective parallelism is limited; the design is a worker pool in name.
15. **MCP create vs REST create asymmetry**: REST auto-activates, MCP requires `autoStart`
    or a second call — inconsistent UX for agents.

---

## 12. Constants quick reference

| Constant | Value | Where |
|---|---|---|
| Body capture limit | 10 KB (`MAX_BODY_SIZE`) | ext_proc_service.rs |
| ALS log buffer | 16 KB / Envoy default flush | xds/access_log.rs |
| Header cap per direction | 20 | access_log_service.rs `filter_headers` |
| Enum tracking: max value length / max tracked | 100 / 100 | schema/inference.rs |
| Enum promotion | ≥10 samples, ≤10 distinct | schema_aggregator.rs |
| Required-field threshold (aggregation) | 100% presence | schema_aggregator.rs |
| Engine `required_threshold` (unused in pipeline) | 0.95 | schema/inference.rs |
| Confidence weights | 0.4 sample (ln n/ln 100) + 0.4 consistency + 0.2 stability | schema_aggregator.rs |
| Tool-enrichment confidence bar | ≥ 0.8 | mcp_service.rs |
| target_sample_count bounds | 1–100,000 | learning_sessions.rs |
| route_pattern length | 1–500 | learning_sessions.rs |
| Session name | ≤48 generated / ≤64 user | learning_session_service.rs / handler |
| Completion check interval | 30 s | cli/mod.rs:1122 |
| Worker batch / flush / retries / backoff / queue | 100 / 5 s / 3 / 100 ms×2 / 10,000 | access_log_processor.rs |
| Pending merge TTL / cleanup tick | 15 s / 5 s | access_log_processor.rs |
| ExtProc message timeout | 5 s | learning_session.rs (injection) |
