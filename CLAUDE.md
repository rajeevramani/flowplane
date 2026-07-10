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

### Vault genres — put artifacts in the right place

- **`backlog/seeds/*.md` = `/aidf:feature` kickoff fodder** — a *problem/feature* statement that
  seeds new feature work (Who-hurts / What-I-want / Success). NOT a home for decisions. A seed may
  be an ADR's *origin* (e.g. `round2-742e6d3-security-scan` → `FP-DEC-0005`), but the decision
  content never lives in the seed.
- **`decisions/FP-DEC-NNNN-<slug>.md` = architecture decisions** — use `templates/adr.md`
  (Context / Options / Decision / Consequences / References), `status: proposed → accepted →
  superseded`. An open, not-yet-decided question is still authored here as `status: proposed`
  (see `FP-DEC-0005`, `FP-DEC-0006`), not parked in `backlog/seeds/`.
- Rule of thumb: **"what should we build?" → seed; "which way did/should we decide?" → ADR.**
  If you catch yourself writing Options/Decision sections, it's an ADR, not a seed.

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
- `.config/nextest.toml` defines the `ci` profile (caps `test-threads` so the shared
  PG can't be connection-exhausted under nextest's global parallelism). Plain
  `cargo test --workspace --all-features` still works and runs doctests inline.
- There is **no Makefile / `make test`**, no `postgres_tests` feature, and no
  testcontainers — older docs/memories saying otherwise are stale (the test infra
  moved to the skip-if-unset shared-DB model).

## Codex as second reviewer (aidf design/code review)

`.aidf.yml` lists `codex_cmd: "codex --model gpt-5.5 --ask-for-approval never"`, but that
**interactive** form hangs (`Error: stdin is not a terminal`). Use the non-interactive
`exec` subcommand with stdin redirected from `/dev/null` — otherwise `codex exec` backgrounded
**hangs indefinitely** reading stdin (`"Reading additional input from stdin..."`, observed 55min
at 0 bytes). Canonical review invocation for this project:

```bash
timeout 600 codex exec --model gpt-5.5 -s read-only "<prompt>" > OUT 2>&1 < /dev/null
```

- `< /dev/null` (mandatory), `-s read-only` (reviewer must not mutate), `timeout 600` backstop.
- Codex **buffers all output and writes only on exit** — an empty output file ≠ stuck. Check
  process elapsed (`ps -o etime`) to tell hung vs working.
- Codex reads the repo itself; give it the design-doc path + constitution path, never pasted
  copies. It self-skips DB-backed tests when `FLOWPLANE_TEST_DATABASE_URL` is unset and reasons
  from source instead.

## Build docs

Build-process docs (lifecycle, feature threads, plans, ADRs, release evidence) are canonical in
`../flowplane-private-vault`, not in this repo. Behavioral spec lives in `spec/00–13` and `spec/15`.
