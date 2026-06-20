# Flowplane documentation

> Audience: end users (operators, platform engineers, API teams) · Status: stable

This directory is **user-facing product documentation only**. Everything here is
written for people *using* Flowplane, and every page **stands alone** — you never
need to read `spec/` or `internal/` to follow a doc here.

For engineering design records, decisions, progress, and release evidence, see
[`../internal/README.md`](../internal/README.md) and `../spec/`.

## Structure (Diátaxis)

The primary axis is **Diátaxis mode**, not audience. Audience and status are
per-document metadata (a header banner), not directories.

| Directory | Mode | What it holds |
|-----------|------|---------------|
| `tutorials/` | tutorial | Learning-oriented. One guided path to a first success. |
| `how-to/`    | how-to   | Task-oriented. One concrete problem solved for someone who knows the basics. |
| `reference/` | reference | Information-oriented. Dry, exhaustive: config/env vars, CLI, API, filters, errors. |
| `concepts/`  | explanation | Understanding-oriented. Why things fit together. (Diátaxis "explanation"; we name the dir `concepts/`.) |

## Cardinal rule

**User docs must stand alone.** A content page here may be *derived* from `spec/`
but must not require the reader to open `spec/` or `internal/` to succeed.

- ✅ Internal docs may link *into* `docs/`.
- ❌ **Content pages** here must **not** cite `spec/` or `internal/` as required reading.
- A short "design rationale" pointer in a `concepts/` page is the only exception,
  and it stays a pointer — not required reading.

### Enforcement (CI)

A CI check rejects links from `docs/**/*.md` into `../internal/` or `../spec/`,
**with two carve-outs** so navigation still works:

1. The index file `docs/README.md` is exempt (it links to sibling buckets for
   navigation — exactly what you're reading).
2. Links to a *bucket index* (`../internal/README.md`, `../spec/README.md`) are
   allowed from anywhere; only links into deeper `internal/` or `spec/` **content**
   are rejected.

Tracked in #116.

## Per-document banner

Every page starts with one metadata line:

```md
> Audience: operators · Status: stable
```

Use `Audience:` (operators / platform-engineers / api-teams / newcomers) and
`Status:` (stable / draft).

## Source-of-truth policy

- **Implementation truth** → code + tests.
- **User/operator truth** → the single canonical page for that task (e.g. one
  bootstrap how-to, one configuration reference). Do not restate it elsewhere.
- **Deployment examples** (AWS, later k8s/systemd) → platform-specific *delivery*
  only; they **link** to the canonical how-to/reference instead of duplicating it.
- **Design rationale** → `spec/` + issues (linked, not inlined).

When behavior changes: update the one canonical user page plus any deployment
example whose exact commands would otherwise be wrong. Do not sprinkle the change
across every file.

## Migration status

The Diátaxis directories are populated by epic #100 (sub-issues #101–#112).
Existing operator docs migrate **lazily** after this policy is ratified — not in a
big upfront move. Planned reclassification (tracked, not yet executed):

| Current file | Destination | Class |
|--------------|-------------|-------|
| `aws-secure-deployment.md` | `docs/how-to/` (deployment example) | user |
| `production-readiness.md` | `docs/how-to/` (likely an index → smaller pages) | user |
| `secret-kek-rotation.md` | `docs/how-to/` | user |
| `observability-alerts.md` | `docs/reference/` | user |
| `dev-dataplane.md` | `../internal/` | internal (dev workflow) |
| `failure-mode-matrix.md` | `../internal/` | internal (evidence) |
| `adversarial-surface-map.md` | `../internal/` | internal (evidence) |
| `release-packaging.md` | `../internal/release/` | internal |
