# Issue Fix Workflow

Use this process when working through GitHub issues on the active integration branch.

## Roles

- GitHub issues are the source of truth for planned work, review feedback, and status.
- Codex owns implementation: read the issue, design the smallest viable change, code it, test it,
  update progress notes when needed, commit, merge, push, and hand the issue back for review.
- Claude reviews the design and code through GitHub issue comments. Review feedback becomes the
  next tracked change, not an offline side thread.
- Issues stay open while under review. A finished implementation is marked with
  `ready-for-review`; rejected work is marked `do-over` and sent back through the same workflow.

## Branching Model

- Integration branch: `main`
- Work branches: `issue/<issue-number>-<short-slug>`
- Merge path: issue branch -> `main`
- PRs are not required for this workflow.

Start each issue from the latest integration branch:

```bash
git checkout main
git pull --ff-only
git checkout -b issue/<issue-number>-<short-slug>
```

## Per-Issue Process

1. Read the GitHub issue and any linked context.
2. Validate whether the issue is real against the current code.
3. If the issue is invalid or already fixed, document the reason and do not change code.
4. If valid, make the smallest focused fix that fits the current architecture.
5. Add or update tests that would have caught the issue.
6. Run targeted tests first, then broader tests when the touched surface warrants it.
7. Commit on the issue branch with the issue number in the message:

```bash
git commit -m "Fix <short issue summary> (#<issue-number>)"
```

8. Merge directly back to the integration branch:

```bash
git checkout main
git pull --ff-only
git merge --no-ff issue/<issue-number>-<short-slug>
git push origin main
```

9. Set the GitHub issue to review by applying the `ready-for-review` label. Do not close the
   issue; review closes or redirects it.

This repository currently uses `ready-for-review` as the fix handoff marker. Apply it only after
the fix is merged into the integration branch, pushed, and validated. If the issue had a `do-over`
label, remove `do-over` when the corrected fix is ready for review. Leave the issue open until the
reviewer accepts the fix. If a GitHub Project status field is later configured, keep it in sync
with the label rather than inventing a second workflow.

## Conflict Policy

If the remote branch moved while the fix branch was in progress, integrate the remote changes and keep the local issue fix for conflicted files unless the remote clearly contains a newer equivalent fix.

Default conflict intent:

```text
prefer the issue branch/local fix for conflicts
```

Do not use destructive commands such as `git reset --hard` or broad checkout/restore operations unless explicitly requested.

## Issue Priority

Work issues in this order unless directed otherwise:

1. correctness, security, data integrity, tenant isolation
2. broken manual flow or CLI/API regression
3. failing tests or CI
4. docs and operator UX polish
5. refactors only when needed to land a real fix

## Done Criteria

An issue is done when:

- the issue has been validated
- the fix is committed on an issue branch
- `cargo clippy --all-targets --all-features -- -D warnings` passes, or a narrower clippy
  command is documented when the full workspace check is not practical locally
- tests relevant to the touched area pass; run `cargo test --workspace --all-features` when
  the touched surface is broad or shared
- the issue branch is merged into `main`
- the integration branch is pushed to `origin`
- the GitHub issue is open, has a completion comment, and has the `ready-for-review` label
