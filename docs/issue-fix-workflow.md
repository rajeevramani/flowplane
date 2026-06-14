# Issue Fix Workflow

Use this process when working through GitHub issues on the active integration branch.

## Branching Model

- Integration branch: `claude/optimistic-lamport-j38tuy`
- Work branches: `issue/<issue-number>-<short-slug>`
- Merge path: issue branch -> `claude/optimistic-lamport-j38tuy`
- PRs are not required for this workflow.

Start each issue from the latest integration branch:

```bash
git checkout claude/optimistic-lamport-j38tuy
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
git checkout claude/optimistic-lamport-j38tuy
git pull --ff-only
git merge --no-ff issue/<issue-number>-<short-slug>
git push origin claude/optimistic-lamport-j38tuy
```

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
- tests relevant to the touched area pass
- the issue branch is merged into `claude/optimistic-lamport-j38tuy`
- the integration branch is pushed to `origin`
