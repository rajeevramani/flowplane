## Summary

`docs/reference/filters.md` contains a dangling Markdown link in the global rate-limit "reserved names" note. The link text `[filters reference / reserved names]` has no destination and no matching reference-style link definition anywhere in the file, so it renders as literal bracketed text rather than a working link. A reader following the note to find the "reserved names" material has nothing to click.

## Release identity

- Requested version: 3.1.0
- Git tag / commit: `v3.1.0` → `a9ea214fae7dda12aa79554942129b800d9a6fe3`
- Artifact under test: documentation from the `v3.1.0` source archive (SHA-256 `85613d7b44119495bfc9d06a558d1e609eb8be22abaac09f354b7e390aeb30c1`)
- Executed platform: linux/amd64 (documentation defect is platform-independent)

## Finding / Test IDs

- F-001 (release-verification static documentation reconciliation, Phase 7)

## Affected documentation

- `docs/reference/filters.md`, line 241, in the "External cluster" / reserved-name note that precedes the `Validation:` list.

Exact text:

```
The reserved name `rate_limit_cluster` (and the `rate_limit_` prefix) cannot be used for a
user-created cluster — see [filters reference / reserved names] and
[Global rate limiting](../concepts/global-rate-limiting.md).
```

## Prerequisites

None — verifiable by reading the tagged file.

## Steps to reproduce

1. Fetch the documentation at the release tag:
   `curl -fsSLO https://github.com/rajeevramani/flowplane/archive/refs/tags/v3.1.0.tar.gz`
2. Extract and open `docs/reference/filters.md`.
3. Inspect line 241 and search the whole file for a link definition:
   `grep -n "reserved names\]:" docs/reference/filters.md` → no matches.

## Expected behavior

The note should link to the actual "reserved names" material (either an in-page anchor/section or the correct target document), so the reference resolves.

## Actual behavior

`[filters reference / reserved names]` has neither an inline `(target)` nor a reference definition; it renders as literal text `[filters reference / reserved names]`. The adjacent `[Global rate limiting](../concepts/global-rate-limiting.md)` link is correct, which highlights the broken one by contrast.

## Error evidence

```
$ grep -n "filters reference / reserved names" docs/reference/filters.md
241:user-created cluster — see [filters reference / reserved names] and
$ grep -n "reserved names\]:" docs/reference/filters.md
(no output)
```

## Corrected diagnostic or workaround

Not applicable (static rendering defect; no runtime command to correct). Fix is to supply a valid link target or reword the sentence.

## Impact

Documentation-only. The `global_rate_limit` filter, reserved `rate_limit_cluster` name, and validation all behave correctly at runtime (verified end to end). The defect is a broken cross-reference in the reference page that governs the global rate-limit filter.

## Acceptance criteria

- `docs/reference/filters.md` line 241 links to a real target (in-page anchor or correct doc), or the clause is reworded to remove the dangling link.
- No dangling `[...]` Markdown links remain in the file (`grep` for `]` with no following `(` or `:` on link-shaped text returns nothing actionable).

## Scope not tested

Only the internal-link integrity of this note was assessed; the surrounding filter semantics were separately verified working.
