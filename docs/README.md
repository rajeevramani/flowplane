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

**User docs must stand alone.** The test is *completability*: a reader must be able
to finish a tutorial, how-to, or reference page — every step, every required value —
**without opening `spec/` or `internal/`**. A page may be *derived* from `spec/`, but
the spec must never be required reading to succeed.

What this allows and forbids:

- ✅ Internal docs may link *into* `docs/`.
- ✅ **Optional** "Further reading" / design-reference links into `spec/` are fine,
  as long as they are clearly marked optional and the task is complete without them.
  Put them under a `## Further reading` (or "Design references") heading, or label
  them inline as optional background.
- ✅ `concepts/` (explanation) pages may cite `spec/` freely — by design they are a
  bridge into the design records (see #112). They still must not pull in `internal/`.
- ❌ No page may make a `spec/` or `internal/` link **required reading** — i.e. a step
  the reader must follow to complete the task.
- ❌ No page may depend on an `internal/` artifact in a task step (e.g.
  `source internal/.env...`); inline a self-contained example instead.

### Enforcement (CI)

A CI check lists every `docs/**/*.md` link into `../internal/` or `../spec/` and
fails on the ones that are **not** allowed. Allowed:

1. `docs/README.md` (this index) and links to a *bucket index*
   (`../internal/README.md`, `../spec/README.md`).
2. Any link from a `docs/concepts/` page into `../spec/` (explanation bridge).
3. Links into `../spec/` that sit under a `## Further reading` / "Design references"
   heading (optional background).

Everything else — required-reading spec links in task steps, and **any** link into
`../internal/` from a task page — fails the check.

Tracked in #116, #118.

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

The Diátaxis directories are populated by epic #100 (sub-issues #101–#112). The
existing operator docs have been reclassified (#116):

| Old path | New location | Class |
|----------|--------------|-------|
| `docs/aws-secure-deployment.md` | `docs/how-to/aws-secure-deployment.md` | user |
| `docs/production-readiness.md` | `docs/how-to/production-readiness.md` | user |
| `docs/secret-kek-rotation.md` | `docs/how-to/secret-kek-rotation.md` | user |
| `docs/dev-dataplane.md` | `../internal/dev-dataplane.md` | internal (dev workflow) |
| `docs/failure-mode-matrix.md` | `../internal/failure-mode-matrix.md` | internal (evidence) |
| `docs/adversarial-surface-map.md` | `../internal/adversarial-surface-map.md` | internal (evidence) |
| `docs/release-packaging.md` | `../internal/release/release-packaging.md` | internal |

The boundary is enforced in CI by `scripts/ci/check-docs-boundary.py` (see
[Enforcement (CI)](#enforcement-ci)).
