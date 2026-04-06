# Changelog

## v0.2.1 (2026-04-06)

### Schema Learning Engine

- **Enum detection** — String fields with low cardinality (<=10 unique values, >=10 samples) are automatically promoted to enums in the learned schema. Raw observed values are never exposed in public API output.
- **Path normalization** — Dynamic URL segments are replaced with contextually named parameters: `/users/123` becomes `/users/{userId}`, `/orders/2024-01-15` becomes `/orders/{orderDate}`. 47 protected keywords and 141 plural-to-singular mappings.
- **PATCH-aware requirements** — PATCH request bodies no longer mark fields as required (partial update semantics). Response schemas are unaffected.
- **Confidence scoring** — Improved edge case handling for oneOf schemas and empty field sets, aligned with specwatch reference implementation.
- **Domain model deduplication** — Structurally identical schemas across endpoints are extracted to `components/schemas` with `$ref` references in the OpenAPI export. Name derivation from path segments (e.g., `/customers/{id}` → `Customer`).
- **Auto-aggregate mode** — Learning sessions can periodically aggregate schemas while continuing to collect samples. Use `--auto-aggregate` on `learn start` and `learn stop` to trigger final aggregation.

### Named Learning Sessions

- Sessions can be named with `--name` on `learn start`. Auto-generated from route pattern if omitted.
- All learn subcommands (`get`, `stop`, `cancel`, `export`) accept a session name or UUID.
- Schema queries can be scoped to a specific session with `--session`.

### Schema CLI

New `flowplane schema` top-level command:

- `flowplane schema list` — list discovered schemas with confidence, sample count, and version
- `flowplane schema get <ID>` — inspect schema detail including request/response fields, types, and formats
- `flowplane schema export --all -o api.yaml` — export as OpenAPI 3.1 with domain model deduplication
- `flowplane learn export` — convenience shortcut for `schema export --all`

### MCP Tools

- **New tool**: `cp_stop_learning` — stop an active session and trigger final aggregation
- `cp_create_learning_session` — new `name` and `autoAggregate` parameters
- All learning tools accept session name or UUID for the `id` parameter

### Database Migrations

- `20260406000001_auto_aggregate_support.sql` — adds `auto_aggregate`, `snapshot_count` to `learning_sessions`; adds `session_id`, `snapshot_number` to `aggregated_api_schemas`
- `20260406000002_add_learning_session_name.sql` — adds `name` column to `learning_sessions` with partial unique index

### UI

- Auto-aggregate toggle on session create form
- Stop Session button (separate from Cancel) on session detail page
- Session name display in list and detail views
- Purple "Auto" badge for auto-aggregate sessions

### Documentation

- [Learning Quickstart](docs/learning-quickstart.md) — end-to-end guide for schema learning with MockBank
- Updated CLI reference, MCP docs, and Getting Started with new commands and tools

## v0.2.0 (2026-03-31)

See [release notes](https://github.com/rajeevramani/flowplane/releases/tag/v0.2.0).

## v0.1.1 (2026-03-15)

See [release notes](https://github.com/rajeevramani/flowplane/releases/tag/v0.1.1).
