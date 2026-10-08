# Flowplane — agent conventions & guardrails

Project memory for agents working in this repo. Conventions, guardrails, and discovery pointers.

## Architecture-integrity constitution (read before any design)

The architecture-integrity constitution (formerly `spec/14-architecture-integrity.md`) is the
project's standing rulebook of invariants, decision constraints, boundaries, and prohibited moves.
It now lives canonically in the vault at:

    ../flowplane-private-vault/constitution.md

**Every design must align with it.** This pointer is discovery only — *enforcement* is the
`/aidf:design-review` gate: each design carries a `constitution:` alignment block, and the review
**fails closed** if that block is absent, `loaded: false`, has non-empty `violations`, or the
constitution was unavailable. `/aidf:feature` likewise loads it fail-closed before drafting.

## Decisions

- Repo `DECISIONS.md` (D-001..D-025) is a **FROZEN** log of code/build-coupled legacy decisions —
  historical record only.
- **New substantive decisions are authored ONLY as vault AIDF ADRs**
  (`../flowplane-private-vault/decisions/FP-DEC-NNNN-<slug>.md`). Do not append new substantive
  decisions to `DECISIONS.md`; a short repo pointer entry is allowed for code/build ergonomics,
  never a second record. One canonical home per decision.

## Testing

- **Runner: `cargo nextest`** (CI uses it; PR #176). Install once with
  `cargo install cargo-nextest --locked` (or `cargo binstall cargo-nextest`).
- DB-backed tests connect to a **shared external PostgreSQL** via
  `FLOWPLANE_TEST_DATABASE_URL`; they **skip themselves** when it is unset (no
  testcontainers, no per-test container spawning). Most also need
  `FLOWPLANE_SECRET_ENCRYPTION_KEY`. Typical local run:
  ```bash
  export FLOWPLANE_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/flowplane_test
  export FLOWPLANE_SECRET_ENCRYPTION_KEY=0123456789abcdef0123456789abcdef
  cargo nextest run --workspace --all-features          # CI adds --profile ci
  cargo test --workspace --all-features --doc           # nextest does NOT run doctests
  ```
- Exposure integration suites use only `FLOWPLANE_TEST_DATABASE_URL` (never generic
  `DATABASE_URL`). They visibly return early only when that test variable is unset,
  matching the repository convention; a configured but unavailable database is a failure.
  Fault-injection suites create/drop uniquely named scratch databases on that same
  test server, so the configured test role needs `CREATEDB` and ownership of its scratch
  databases. Point it only at a disposable test server. CI's PostgreSQL service role has
  those privileges. A qualifying gate must supply this prerequisite and verify that
  no database-backed cases returned early; capture variable names and role capabilities,
  not connection strings or secrets, in execution evidence.
- `.config/nextest.toml` defines the `ci` profile (caps `test-threads` so the shared
  PG can't be connection-exhausted under nextest's global parallelism). Plain
  `cargo test --workspace --all-features` still works and runs doctests inline.
- There is **no Makefile / `make test`**, no `postgres_tests` feature, and no
  testcontainers — older docs/memories saying otherwise are stale (the test infra
  moved to the skip-if-unset shared-DB model).

## Build docs

Build-process docs (lifecycle, feature threads, plans, ADRs, release evidence) are canonical in
`../flowplane-private-vault`, not in this repo. Behavioral spec lives in `spec/00–13` and `spec/15`.
