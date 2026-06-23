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

## Build docs

Build-process docs (lifecycle, feature threads, plans, ADRs, release evidence) are canonical in
`../flowplane-private-vault`, not in this repo. Behavioral spec lives in `spec/00–13` and `spec/15`.
