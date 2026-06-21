# Internal engineering docs

> Internal — engineering/historical; may be stale; **not** product documentation.

This directory and `../spec/` hold material written for *building* Flowplane, not for using it: design records, decisions, progress ledgers, runbooks, and release evidence. End-user documentation lives in [`../docs/`](../docs/README.md).

## Classes of content here

`(current)` = always lived here. `(migrated from docs/ — #116)` = reclassified out of `docs/` when the docs-taxonomy policy was executed.

| Class | Examples | Maintenance |
|-------|----------|-------------|
| **Specs / design records** | `../spec/*` (current) | Point-in-time. Mark historical; do **not** chase current. |
| **Decisions** | `../DECISIONS.md` (current) | Append-only decision log. |
| **Progress / status** | `internal/PROGRESS.md`, `internal/QUESTIONS.md`, `../REWRITE-REPORT.md` (current) | Status ledgers; current only while a gate is active. |
| **Release evidence** | `internal/release-walkthrough.md`, `internal/failure-mode-matrix.md`, `internal/adversarial-surface-map.md`, `internal/release/release-packaging.md` (migrated from `docs/` — #116) | Supports release readiness; not product docs. |
| **Dev runbooks & workflows** | `internal/auth0-local-runbook.md`, `internal/prod-local-runbook.md`, `internal/dev-workflow-automation.md`, `internal/issue-fix-workflow.md`, `internal/user-onboarding.md`, `internal/dev-dataplane.md` (migrated from `docs/` — #116) | For contributors/dev environments. |

Full reclassification table and sequencing: [`../docs/README.md`](../docs/README.md) and #116.

## Rules

- **Historical is fine.** Internal docs describe what was true at the time. Do not spend effort keeping every old spec/progress note perfectly current — mark it historical instead. Only docs still used as **active release gates or status ledgers** get kept up to date.
- **Internal docs may link into `../docs/`.** The reverse is constrained: `docs/` **content pages** must not cite `internal/` or `spec/` as required reading. The CI check carves out two exceptions — the `docs/README.md` index, and links to a bucket index (`README.md`) — so navigation still works while deep content links are rejected. Same rule as `../docs/README.md` and #116.
- **No product truth originates here.** If a user needs it to operate Flowplane, it belongs in `../docs/` as a stand-alone page; internal docs link to it.

## Banner

Each internal page should carry, near the top:

```md
> Internal — engineering/historical; may be stale.
```
