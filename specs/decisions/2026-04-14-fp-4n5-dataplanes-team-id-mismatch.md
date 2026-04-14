# fp-4n5 — `dataplanes.team` id/name mismatch breaks `touch_last_config_verify`

**Date**: 2026-04-14
**Bead**: fp-4n5 Task 1 (first commit)
**Status**: BLESSED (architect, via team-lead) — option A with rows-affected surfacing. Follow-up schema unification deferred to **fp-4g4** (P2).

## What

`FlowplaneDiagnosticsService::touch_last_config_verify`
(`src/xds/services/diagnostics_service.rs:181-217`) runs:

```sql
UPDATE dataplanes SET last_config_verify = $1, updated_at = $1
WHERE name = $2 AND team = $3
```

binding `$3 = authed_team`, which comes from the SPIFFE URI via
`extract_client_identity` → `parse_spiffe_uri` → `parse_team_from_spiffe_uri`.
The SPIFFE field is semantically a team **name** (e.g. `default`,
`engineering`, `payments` — see doc examples in
`src/secrets/vault.rs:301-306`).

But migration `20260207000002_switch_team_fk_to_team_id.sql:293-322`
rewrote `dataplanes.team` to store the team **id** (FK to `teams.id`).
`seed_dev_resources` at `src/startup.rs:123-135` inserts
`team = "dev-default-team-id"` (the id). The dev E2E harness mints the
agent's client cert with SPIFFE team field = `"default"` (the name) via
`TestHarnessConfig::with_mtls_identity("default", "dev-dataplane")`.

Result: the UPDATE's WHERE clause compares `"default"` against
`"dev-default-team-id"`. Zero rows affected. `last_config_verify` stays
NULL for the full test run. `classify_agent_status`
(`src/services/ops_service.rs:180-196`) returns `NOT_MONITORED`. The
`wait_for_agent_ok` helper in `tests/e2e/smoke/test_dev_mtls_docker.rs:145`
times out after 20s and every test in the suite fails at the first
`wait_for_agent_ok` call.

### Why the existing unit/integration tests missed this

`tests/diagnostics_service_integration.rs:454`
(`updates_last_config_verify_on_successful_report`) passes `TEST_TEAM_ID`
— a UUID — as the team argument to `issue_client_cert_for`, so the
SPIFFE URI it mints embeds a team **id** in the team slot. That
accidentally matches the `dataplanes.team` column after migration. It
masks the bug because the test inadvertently violates the "team field in
SPIFFE URI is a team name" convention that the rest of the codebase
(and all doc examples) assumes.

### Schema inconsistency is the underlying drift

`xds_nack_events.team` (migration
`20260225000001_create_xds_nack_events_table.sql:7`) is
`TEXT NOT NULL, -- Team name for isolation` — the **name**.
`dataplanes.team` is the **id** after `20260207000002`. The diagnostics
service touches both tables from the same `handle_envelope` call site
(`handle_envelope` → `touch_last_config_verify` + `persist_listener_state`
→ `NackEventRepository::insert`), so the same `authed_team` string has
to satisfy two different column semantics. One of them is wrong by
construction.

This exactly matches the drift pattern called out in
`2026-04-14-dev-prod-root-cause.md`: when the same invariant is enforced
in N places, N-1 of them will eventually skew.

## Alternatives considered

### A. Fix `touch_last_config_verify` to JOIN `teams` and resolve team name → id

Change the UPDATE to:

```sql
UPDATE dataplanes
SET last_config_verify = $1, updated_at = $1
FROM teams
WHERE dataplanes.team = teams.id
  AND dataplanes.name = $2
  AND teams.name = $3
```

**Pros**: surgical (one query), keeps the SPIFFE convention stable
(team-field-in-URI = team name), leaves `nack_events` path untouched
because it already expects a team name.

**Cons**: adds a JOIN on the hot path (cheap — `teams` is tiny and
indexed). Leaves the underlying schema drift in place — the next
developer who writes a `dataplanes` query keyed by team will hit the
same trap.

### B. Resolve team name → id inside `extract_client_identity`

Push the resolution upstream so `authed_team` is always a team id
everywhere.

**Pros**: fixes every future consumer of `authed_team` by construction.

**Cons**: huge blast radius — `xds_nack_events.team` currently stores
team NAME, so every `persist_listener_state` → `NackEventRepository`
path would now insert a team-id into a team-name column. Would require
simultaneously migrating the nack_events column. Also forces an extra
DB round-trip at the start of every diagnostics stream (cert → id
lookup), and extract_client_identity currently has no DB handle.

### C. Schema migration: unify `xds_nack_events.team` to store team_id (FK)

Matches the `dataplanes.team` semantic. Then implement option B.

**Pros**: eliminates the drift, all future `team` columns have the
same meaning.

**Cons**: migration touches a hot insert path and every repository
query that selects/filters by team. This is the right long-term fix but
it is out of scope for Task 1 of fp-4n5 (which is "get E2E green
before refactoring scaffolding" per `2026-04-14-fp-4n5-pre-implementation.md`
section G4). Should be filed as a follow-up bead.

### D. Hack the test to mint SPIFFE URIs with team id

Change `.with_mtls_identity("default", ...)` to
`.with_mtls_identity("dev-default-team-id", ...)` in
`test_dev_mtls_docker.rs`.

**Pros**: smallest diff.

**Cons**: production is equally broken. `flowplane init` +
`flowplane-agent` in real dev mode produce a SPIFFE URI from the team
**name** via `src/cli/dev_certs.rs`. So hacking the test makes the test
green while leaving real users' `last_config_verify` permanently NULL.
Masks the production bug. Rejected categorically.

## Why

**Option A** chosen, blessed by architect via team-lead.

- A closes the immediate bleed (E2E green) in one query.
- A does not pretend to eliminate the underlying schema drift.
- C is the correct long-term fix but touches enough code and tests
  that doing it in Task 1 violates G4 of
  `2026-04-14-fp-4n5-pre-implementation.md` (get E2E green before
  refactoring scaffolding). **Filed as follow-up bead fp-4g4** (P2,
  "Unify xds_nack_events.team storage to team_id").
- B without C leaves `nack_events` broken and is worse than doing
  nothing.
- D masks a real production bug — categorically rejected.

### Architect addition — rows-affected surfacing

The architect's one addition on top of option A: a zero-row UPDATE
must not silently succeed, because that's exactly how the bug stayed
invisible for weeks. The fix applied here surfaces zero-row updates as
a **warn-level log** (previously debug), keyed on
`dataplane` + `team_name`. The call site (`handle_envelope`) still
swallows the outcome so an audit-column hiccup cannot fail a NACK
persist — but the warn now makes the hidden-failure case observable in
the CP log stream, so future drift of this class is diagnosable at a
glance.

Upgrading this to a structured error return is possible but would
ripple into `handle_envelope`'s ReportOutcome contract for no concrete
test-visible benefit beyond the warn. Punt until/unless a richer
observer needs it.

## Gotchas

### G1: existing integration tests needed SPIFFE team arg updated

`updates_last_config_verify_on_successful_report` in
`tests/diagnostics_service_integration.rs:454` uses a UUID in the
SPIFFE URI's team slot. With option A, the UPDATE becomes
`... AND teams.name = $3` — and the seeded `teams` table from
`tests/common/test_db.rs:100` has rows with `name='test-team'`,
`id=TEST_TEAM_ID (UUID)`. So the test passing `TEST_TEAM_ID` (a UUID)
as the SPIFFE team will NOT match any `teams.name` row. **Option A
will break this test** unless it's also updated to use the team name
`"test-team"` in the SPIFFE URI.

This is actually a feature — the fix exposes the test's latent
semantic bug. Applied in the same commit: every
`issue_client_cert_for(TEST_TEAM_ID, ...)` call in
`tests/diagnostics_service_integration.rs` was migrated to a new
`TEST_TEAM_NAME` / `TEAM_A_NAME` constant (added to
`tests/common/test_db.rs`). `seed_dataplane(...)` still takes the
team **id** because it writes the `dataplanes.team` column which
holds team_id. The distinction — SPIFFE URI carries name, schema
stores id — is now explicit in the constants' doc comment so the
next test author can't recreate the confusion.

### G2: nack_events still requires team name from SPIFFE

Because `xds_nack_events.team` still stores team name, `authed_team`
must remain the team **name** end-to-end. Option A is compatible with
this; options B and C are not without simultaneous migration.

### G3: production impact

This bug means every real dev-mode user's `dataplanes.last_config_verify`
has been NULL the entire time fp-084 has existed. `flowplane xds status`
has been reporting `NOT_MONITORED` for their dev-dataplane even while
the agent was streaming healthy envelopes. Worth calling out in the
commit message.

### G4: JOIN-in-UPDATE is PostgreSQL-specific syntax

`UPDATE ... FROM ... WHERE` is standard PG syntax and is already used
in `20260207000002_switch_team_fk_to_team_id.sql` multiple times
(e.g. line 27-30). Safe to use here.
